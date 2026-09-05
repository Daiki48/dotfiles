use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::codex_tools::{process, runner_storage, trust};

const CONFIG_RELATIVE: &str = ".config/clean-disk.json";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_SCAN_ROOTS: usize = 16;
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_TREE_ENTRIES: usize = 2_000_000;
const MAX_DISCOVERY_DEPTH: usize = 5;
const MAX_TREE_DEPTH: usize = 64;
const OPEN_PATH_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const OPEN_PATH_TIMEOUT: Duration = Duration::from_secs(30);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanDiskConfig {
    schema_version: u64,
    scan_roots: Vec<PathBuf>,
    trash_retention_days: u64,
    build_cache_retention_days: u64,
    #[serde(default = "default_codex_retention")]
    codex_build_cache_retention_days: u64,
    runner_manifest_dir: Option<PathBuf>,
}

fn default_codex_retention() -> u64 {
    1
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CandidateKind {
    Trash,
    BuildCache,
    CodexBuildCache,
}

impl CandidateKind {
    fn label(self) -> &'static str {
        match self {
            Self::Trash => "期限切れtrash",
            Self::BuildCache => "再生成可能build cache",
            Self::CodexBuildCache => "Codex再生成可能build cache",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeSnapshot {
    allocated_bytes: u64,
    entries: usize,
    newest_modified: SystemTime,
    device: u64,
    inode: u64,
    contains_repository: bool,
}

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    kind: CandidateKind,
    active_scope: PathBuf,
    snapshot: TreeSnapshot,
}

#[derive(Debug, Eq, PartialEq)]
enum Disposition {
    AutoDelete,
    Confirm,
    SkipRecent,
    BlockedActive,
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_scan_root(path: &Path) -> Result<()> {
    if !normalized_absolute(path) {
        anyhow::bail!(
            "scan root must be a normalized absolute path: {}",
            path.display()
        );
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect scan root {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("scan root must be a regular directory: {}", path.display());
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("cannot resolve scan root {}", path.display()))?;
    if canonical != path {
        anyhow::bail!(
            "scan root must not contain symlinks or aliases: {}",
            path.display()
        );
    }
    Ok(())
}

fn resolve_config(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.is_absolute() {
            anyhow::bail!("--config must be an absolute path");
        }
        return Ok(path.to_path_buf());
    }
    let home = home::home_dir().context("cannot determine HOME")?;
    let home_config = home.join(CONFIG_RELATIVE);
    if home_config.exists() || home_config.is_symlink() {
        return Ok(home_config);
    }
    let repository_config = env::current_dir()
        .context("cannot determine current directory")?
        .join(CONFIG_RELATIVE);
    if repository_config.exists() || repository_config.is_symlink() {
        return Ok(repository_config);
    }
    anyhow::bail!(
        "clean-disk configuration was not found at {} or {}",
        home_config.display(),
        repository_config.display()
    )
}

fn read_config(path: &Path) -> Result<CleanDiskConfig> {
    let resolved = fs::canonicalize(path)
        .with_context(|| format!("cannot resolve clean-disk config {}", path.display()))?;
    let metadata = fs::symlink_metadata(&resolved)
        .with_context(|| format!("cannot inspect clean-disk config {}", resolved.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_CONFIG_BYTES
    {
        anyhow::bail!("clean-disk config must be a bounded regular file");
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o022 != 0
        || metadata.nlink() != 1
    {
        anyhow::bail!("clean-disk config has an unsafe owner, mode, or link count");
    }
    let config: CleanDiskConfig = serde_json::from_slice(
        &fs::read(&resolved).with_context(|| format!("cannot read {}", resolved.display()))?,
    )
    .with_context(|| format!("invalid clean-disk config {}", resolved.display()))?;
    if config.schema_version != 1 {
        anyhow::bail!("unsupported clean-disk config schema");
    }
    if config.scan_roots.is_empty() || config.scan_roots.len() > MAX_SCAN_ROOTS {
        anyhow::bail!("clean-disk scan root count is invalid");
    }
    for (index, left) in config.scan_roots.iter().enumerate() {
        for right in &config.scan_roots[index + 1..] {
            if paths_overlap(left, right) {
                anyhow::bail!(
                    "clean-disk scan roots must not overlap: {} and {}",
                    left.display(),
                    right.display()
                );
            }
        }
    }
    if !(1..=3650).contains(&config.trash_retention_days)
        || !(1..=3650).contains(&config.build_cache_retention_days)
        || !(1..=3650).contains(&config.codex_build_cache_retention_days)
    {
        anyhow::bail!("clean-disk retention days must be between 1 and 3650");
    }
    if let Some(path) = &config.runner_manifest_dir
        && !normalized_absolute(path)
    {
        anyhow::bail!("runner_manifest_dir must be a normalized absolute path");
    }
    Ok(config)
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("cannot read directory {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("cannot enumerate directory {}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn scan_path(path: &Path) -> Result<TreeSnapshot> {
    reject_mounted_tree(path)?;
    let root = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect cleanup candidate {}", path.display()))?;
    #[cfg(unix)]
    let root_device = root.dev();
    #[cfg(not(unix))]
    let root_device = 0;
    #[cfg(unix)]
    let root_inode = root.ino();
    #[cfg(not(unix))]
    let root_inode = 0;
    let mut allocated_bytes = 0u64;
    let mut entries = 0usize;
    let mut contains_repository = false;
    let mut newest_modified = root.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let mut stack = vec![(path.to_path_buf(), 0usize)];
    while let Some((current, depth)) = stack.pop() {
        if depth > MAX_TREE_DEPTH {
            anyhow::bail!(
                "cleanup candidate exceeds the maximum tree depth: {}",
                path.display()
            );
        }
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("cannot inspect cleanup entry {}", current.display()))?;
        contains_repository |= current.file_name().is_some_and(|name| name == ".git");
        #[cfg(unix)]
        if metadata.dev() != root_device {
            anyhow::bail!(
                "cleanup candidate crosses a filesystem boundary: {}",
                current.display()
            );
        }
        entries += 1;
        if entries > MAX_TREE_ENTRIES {
            anyhow::bail!(
                "cleanup candidate exceeds the entry safety limit: {}",
                path.display()
            );
        }
        #[cfg(unix)]
        {
            allocated_bytes = allocated_bytes.saturating_add(metadata.blocks().saturating_mul(512));
        }
        #[cfg(not(unix))]
        {
            allocated_bytes = allocated_bytes.saturating_add(metadata.len());
        }
        newest_modified =
            newest_modified.max(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        if metadata.file_type().is_symlink() || metadata.is_file() {
            continue;
        }
        if !metadata.is_dir() {
            anyhow::bail!(
                "cleanup candidate contains a special file: {}",
                current.display()
            );
        }
        for entry in sorted_entries(&current)?.into_iter().rev() {
            stack.push((entry.path(), depth + 1));
        }
    }
    Ok(TreeSnapshot {
        allocated_bytes,
        entries,
        newest_modified,
        device: root_device,
        inode: root_inode,
        contains_repository,
    })
}

pub(crate) fn validate_artifact_tree(path: &Path) -> Result<()> {
    validate_scan_root(path)?;
    scan_path(path)?;
    Ok(())
}

fn reject_mounted_tree(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // bind mountは同じdeviceにも作れるため、st_devだけでは検出できない。
        let mounts = fs::read_to_string("/proc/self/mountinfo")
            .context("cleanup候補のmount状態を確認できません")?;
        validate_mountinfo(path, &mounts)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_mountinfo(path: &Path, mounts: &str) -> Result<()> {
    for line in mounts.lines() {
        let encoded = line
            .split_whitespace()
            .nth(4)
            .context("invalid mountinfo")?;
        let mount = encoded
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        if Path::new(&mount).starts_with(path) {
            anyhow::bail!("cleanup候補にmountが残っています: {}", path.display());
        }
    }
    Ok(())
}

pub(crate) fn remove_artifact_tree(path: &Path) -> Result<()> {
    validate_artifact_tree(path)?;
    ensure_artifact_idle(path)?;
    let candidate = Candidate {
        path: path.into(),
        kind: CandidateKind::BuildCache,
        active_scope: path.into(),
        snapshot: scan_path(path)?,
    };
    remove_candidate(&candidate)
}

/// 0700のtask専用領域は同一UIDの検証processだけに使う。
/// 通常のclean-diskと違い他ユーザーの領域を扱わないためsudoを要求しない。
pub(crate) fn ensure_artifact_idle(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let lsof =
            trust::trusted_system_binary("/usr/bin/lsof", "lsof").map_err(anyhow::Error::msg)?;
        let mut command = Command::new(lsof);
        process::clear_environment(&mut command);
        command.env("PATH", "/usr/bin:/bin").env("LANG", "C").args([
            "-nP",
            "-F0n",
            "-a",
            "-u",
            &unsafe { libc::geteuid() }.to_string(),
        ]);
        let result =
            process::run_host_with_limit(&mut command, OPEN_PATH_TIMEOUT, OPEN_PATH_CAPTURE_BYTES)?;
        if !result.status.success() {
            anyhow::bail!("task成果物の使用状況を確認できません（lsof）");
        }
        validate_lsof_warnings(&result.stderr, &BTreeSet::from([path.into()]))?;
        if overlaps(path, &parse_lsof_paths(&result.stdout)?) {
            anyhow::bail!("task成果物を使用中のprocessがあります。終了してから再実行してください");
        }
        Ok(())
    }
    #[cfg(not(unix))]
    anyhow::bail!("task成果物のprocess検証はこのOSで未対応です")
}

fn nearest_repository_root(start: &Path, ignored_root: Option<&Path>) -> Result<Option<PathBuf>> {
    let mut current = start;
    loop {
        if Some(current) != ignored_root {
            match fs::symlink_metadata(current.join(".git")) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    anyhow::bail!("repositoryの.gitがsymlinkです: {}", current.display());
                }
                Ok(_) => return Ok(Some(current.to_path_buf())),
                Err(cause) if cause.kind() == io::ErrorKind::NotFound => {}
                Err(cause) => return Err(cause.into()),
            }
        }
        let Some(parent) = current.parent() else {
            return Ok(None);
        };
        current = parent;
    }
}

fn cache_is_ignored(repository: &Path, path: &Path) -> Result<bool> {
    let relative = path.strip_prefix(repository)?;
    let git = trust::trusted_system_binary("/usr/bin/git", "git").map_err(anyhow::Error::msg)?;
    let query = |args: &[&str]| -> Result<process::Output> {
        let mut command = Command::new(&git);
        process::clear_environment(&mut command);
        command
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(repository);
        if args.first() == Some(&"ls-files") {
            command.arg("--literal-pathspecs");
        }
        command.args(args).arg(relative);
        Ok(process::run_host_with_limit(
            &mut command,
            OPEN_PATH_TIMEOUT,
            OPEN_PATH_CAPTURE_BYTES,
        )?)
    };
    let tracked = query(&["ls-files", "-z", "--"])?;
    if !tracked.status.success() {
        anyhow::bail!("cacheのGit追跡状態を確認できません");
    }
    if !tracked.stdout.is_empty() {
        return Ok(false);
    }
    let ignored = query(&["check-ignore", "-q", "--"])?;
    match ignored.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!("cacheのGit ignore状態を確認できません"),
    }
}

fn discover_under(root: &Path, codex: bool) -> Result<Vec<Candidate>> {
    validate_scan_root(root)?;
    let mut candidates = Vec::new();
    let mut visited = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_DISCOVERY_DEPTH {
            continue;
        }
        for entry in sorted_entries(&directory)? {
            visited += 1;
            if visited > MAX_DISCOVERY_ENTRIES {
                anyhow::bail!(
                    "scan root exceeds the discovery entry safety limit: {}",
                    root.display()
                );
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("cannot inspect discovered path {}", path.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if codex
                && (name.to_string_lossy().starts_with(".podman")
                    || (path.join("storage.lock").exists() && path.join("db.sql").exists()))
            {
                println!("NativeCleanupRequired\tPodman管理領域\t{}", path.display());
                continue;
            }
            if codex && name == ".artifacts" {
                println!(
                    "TaskArtifacts\tfinish / clean-artifactsで回収\t{}",
                    path.display()
                );
                continue;
            }
            if name == ".codex-trash"
                || (codex
                    && (matches!(name.to_str(), Some(".preserved" | ".artifact-backups"))
                        || name.to_string_lossy().starts_with(".codex-trash-")))
            {
                let active_scope = match nearest_repository_root(&directory, codex.then_some(root))
                {
                    Ok(repository) => repository.unwrap_or_else(|| path.clone()),
                    Err(cause) => {
                        println!("RetainUnverifiable\t{}\t{cause}", path.display());
                        continue;
                    }
                };
                for child in sorted_entries(&path)? {
                    let child_path = child.path();
                    let snapshot = match scan_path(&child_path) {
                        Ok(snapshot) => snapshot,
                        Err(cause) => {
                            println!("RetainUnverifiable\t{}\t{cause}", child_path.display());
                            continue;
                        }
                    };
                    candidates.push(Candidate {
                        snapshot,
                        path: child_path,
                        kind: CandidateKind::Trash,
                        active_scope: active_scope.clone(),
                    });
                }
                continue;
            }
            if name == "target" && (codex || directory.join("Cargo.toml").is_file()) {
                let repository = match nearest_repository_root(&directory, codex.then_some(root)) {
                    Ok(repository) => repository,
                    Err(cause) => {
                        println!("RetainUnverifiable\t{}\t{cause}", path.display());
                        continue;
                    }
                };
                if let Some(repository) = &repository {
                    match cache_is_ignored(repository, &path) {
                        Ok(true) => {}
                        Ok(false) => {
                            println!("RetainTrackedOrUnignored\t{}", path.display());
                            continue;
                        }
                        Err(cause) => {
                            println!("RetainUnverifiable\t{}\t{cause}", path.display());
                            continue;
                        }
                    }
                }
                let snapshot = match scan_path(&path) {
                    Ok(snapshot) => snapshot,
                    Err(cause) => {
                        println!("RetainUnverifiable\t{}\t{cause}", path.display());
                        continue;
                    }
                };
                if snapshot.contains_repository {
                    println!("RetainRepository\t{}", path.display());
                    continue;
                }
                candidates.push(Candidate {
                    snapshot,
                    path,
                    kind: if codex {
                        CandidateKind::CodexBuildCache
                    } else {
                        CandidateKind::BuildCache
                    },
                    active_scope: repository.unwrap_or_else(|| directory.clone()),
                });
                continue;
            }
            if matches!(name.to_str(), Some(".git" | "node_modules" | "target")) {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    Ok(candidates)
}

fn validate_lsof_warnings(stderr: &[u8], protected_scopes: &BTreeSet<PathBuf>) -> Result<()> {
    let stderr = std::str::from_utf8(stderr)
        .context("lsof warning output is not valid UTF-8; open file observation is incomplete")?;
    let mut lines = stderr.lines().filter(|line| !line.trim().is_empty());
    while let Some(line) = lines.next() {
        let Some(detail) = line.strip_prefix("lsof: WARNING: can't stat() ") else {
            anyhow::bail!(
                "lsof returned an unrecognized warning; open file observation is incomplete"
            );
        };
        let Some((_, path)) = detail.split_once(" file system ") else {
            anyhow::bail!(
                "lsof returned an unrecognized warning; open file observation is incomplete"
            );
        };
        let inaccessible = Path::new(path);
        if !normalized_absolute(inaccessible)
            || protected_scopes
                .iter()
                .any(|scope| paths_overlap(scope, inaccessible))
        {
            anyhow::bail!(
                "lsof could not inspect a filesystem that overlaps a cleanup scope: {}",
                inaccessible.display()
            );
        }
        if lines.next().map(str::trim) != Some("Output information may be incomplete.") {
            anyhow::bail!(
                "lsof returned an incomplete warning; open file observation is incomplete"
            );
        }
    }
    Ok(())
}

fn parse_lsof_paths(stdout: &[u8]) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for field in stdout.split(|byte| *byte == 0) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        let Some(value) = field.strip_prefix(b"n") else {
            continue;
        };
        let value = value.strip_suffix(b" (deleted)").unwrap_or(value);
        #[cfg(unix)]
        let path = {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(value.to_vec()))
        };
        #[cfg(not(unix))]
        let path =
            PathBuf::from(std::str::from_utf8(value).context(
                "lsof path output is not valid UTF-8; open file observation is incomplete",
            )?);
        if path.is_absolute() {
            paths.insert(path);
        }
    }
    Ok(paths)
}

fn observe_open_paths(protected_scopes: &BTreeSet<PathBuf>) -> Result<BTreeSet<PathBuf>> {
    if protected_scopes.is_empty() {
        return Ok(BTreeSet::new());
    }
    let lsof = trust::trusted_system_binary("/usr/bin/lsof", "lsof").map_err(anyhow::Error::msg)?;
    #[cfg(unix)]
    let requires_elevation = unsafe { libc::geteuid() } != 0;
    #[cfg(not(unix))]
    let requires_elevation = false;
    let mut command = if requires_elevation {
        let sudo =
            trust::trusted_system_binary("/usr/bin/sudo", "sudo").map_err(anyhow::Error::msg)?;
        let mut command = Command::new(sudo);
        command.args(["--", &lsof]);
        command
    } else {
        Command::new(lsof)
    };
    process::clear_environment(&mut command);
    command
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .args(["-nP", "-F0n"]);
    let output =
        process::run_host_with_limit(&mut command, OPEN_PATH_TIMEOUT, OPEN_PATH_CAPTURE_BYTES)
            .context("failed to inspect open files")?;
    if !output.status.success() {
        anyhow::bail!(
            "lsof failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    validate_lsof_warnings(&output.stderr, protected_scopes)?;
    parse_lsof_paths(&output.stdout)
}

fn overlaps(scope: &Path, open_paths: &BTreeSet<PathBuf>) -> bool {
    open_paths
        .iter()
        .any(|open| open == scope || open.starts_with(scope))
}

fn disposition(
    candidate: &Candidate,
    open_paths: &BTreeSet<PathBuf>,
    now: SystemTime,
    trash_retention_days: u64,
    build_retention_days: u64,
) -> Disposition {
    if overlaps(&candidate.active_scope, open_paths) {
        return Disposition::BlockedActive;
    }
    let retention_days = match candidate.kind {
        CandidateKind::Trash => trash_retention_days,
        CandidateKind::BuildCache | CandidateKind::CodexBuildCache => build_retention_days,
    };
    let age = now
        .duration_since(candidate.snapshot.newest_modified)
        .unwrap_or_default();
    if age < DAY.saturating_mul(u32::try_from(retention_days).unwrap_or(u32::MAX)) {
        return Disposition::SkipRecent;
    }
    match candidate.kind {
        CandidateKind::Trash => Disposition::Confirm,
        CandidateKind::BuildCache | CandidateKind::CodexBuildCache => Disposition::AutoDelete,
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn remove_candidate(candidate: &Candidate) -> Result<()> {
    if matches!(
        candidate.kind,
        CandidateKind::BuildCache | CandidateKind::CodexBuildCache
    ) {
        let parent = candidate
            .path
            .parent()
            .context("cacheの親pathがありません")?;
        if let Some(repository) = nearest_repository_root(parent, codex_worktree_root().as_deref())?
            && !cache_is_ignored(&repository, &candidate.path)?
        {
            anyhow::bail!("cacheが追跡対象またはignore対象外になりました");
        }
    }
    let current = scan_path(&candidate.path)?;
    if matches!(
        candidate.kind,
        CandidateKind::BuildCache | CandidateKind::CodexBuildCache
    ) && current.contains_repository
    {
        anyhow::bail!("cache内のGit repositoryを保持します");
    }
    if current != candidate.snapshot {
        anyhow::bail!(
            "candidate changed after inspection: {}",
            candidate.path.display()
        );
    }
    let metadata = fs::symlink_metadata(&candidate.path)?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(&candidate.path)?;
    } else if metadata.is_dir() {
        fs::remove_dir_all(&candidate.path)?;
    } else {
        anyhow::bail!(
            "candidate became a special file: {}",
            candidate.path.display()
        );
    }
    Ok(())
}

fn confirm<R: BufRead, W: Write>(input: &mut R, output: &mut W, prompt: &str) -> Result<bool> {
    write!(output, "{prompt} [y/N] ")?;
    output.flush()?;
    let mut value = String::new();
    input.read_line(&mut value)?;
    Ok(matches!(value.trim(), "y" | "Y"))
}

fn codex_worktree_root() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home::home_dir().map(|home| home.join(".codex")))
        .map(|home| home.join("worktrees"))
}

pub(crate) fn run(explicit_config: Option<&Path>, dry_run: bool) -> Result<()> {
    let config_path = resolve_config(explicit_config)?;
    let config = read_config(&config_path)?;
    println!("clean-disk: {} を使用します", config_path.display());

    let codex_root = codex_worktree_root();
    if let Some(root) = &codex_root
        && config
            .scan_roots
            .iter()
            .any(|scan| paths_overlap(scan, root))
    {
        anyhow::bail!(
            "configured scan roots must not overlap the Codex worktree root: {}",
            root.display()
        );
    }
    let mut candidates = Vec::new();
    for root in &config.scan_roots {
        if !root.exists() {
            println!("SKIP missing scan root: {}", root.display());
            continue;
        }
        candidates.extend(discover_under(root, false)?);
    }
    if let Some(root) = codex_root
        && root.exists()
    {
        candidates.extend(discover_under(&root, true)?);
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    let protected_scopes = candidates
        .iter()
        .map(|candidate| candidate.active_scope.clone())
        .collect::<BTreeSet<_>>();
    // dry-runは権限昇格せずinventoryを返す。利用状況は未確認と明示し、適用時に必ず再検証する。
    let open_paths = if dry_run {
        BTreeSet::new()
    } else {
        observe_open_paths(&protected_scopes).context(
        "open file observation failed; no cleanup was performed because active sessions cannot be excluded",
    )?
    };
    if dry_run {
        println!("DRY RUN: 使用中processは未確認です。表示は削除許可ではありません");
    }

    let now = SystemTime::now();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut reclaimed = 0u64;
    let mut approved_candidates = Vec::new();
    for candidate in &candidates {
        let decision = disposition(
            candidate,
            &open_paths,
            now,
            config.trash_retention_days,
            if candidate.kind == CandidateKind::CodexBuildCache {
                config.codex_build_cache_retention_days
            } else {
                config.build_cache_retention_days
            },
        );
        writeln!(
            output,
            "{:?}\t{}\t{}\t{}",
            decision,
            candidate.kind.label(),
            human_bytes(candidate.snapshot.allocated_bytes),
            candidate.path.display()
        )?;
        if dry_run {
            continue;
        }
        let approved = match decision {
            Disposition::AutoDelete => true,
            Disposition::Confirm if interactive => confirm(
                &mut input,
                &mut output,
                &format!(
                    "{}（{}）を削除しますか?",
                    candidate.path.display(),
                    human_bytes(candidate.snapshot.allocated_bytes)
                ),
            )?,
            Disposition::Confirm | Disposition::SkipRecent | Disposition::BlockedActive => false,
        };
        if approved {
            approved_candidates.push(candidate);
        }
    }
    if !dry_run && !approved_candidates.is_empty() {
        let approved_scopes = approved_candidates
            .iter()
            .map(|candidate| candidate.active_scope.clone())
            .collect::<BTreeSet<_>>();
        let latest_open_paths = observe_open_paths(&approved_scopes).context(
            "final open file observation failed; no approved cleanup was performed because active sessions cannot be excluded",
        )?;
        for candidate in approved_candidates {
            if overlaps(&candidate.active_scope, &latest_open_paths) {
                writeln!(output, "BlockedActiveRecheck\t{}", candidate.path.display())?;
                continue;
            }
            remove_candidate(candidate)
                .with_context(|| format!("failed to remove {}", candidate.path.display()))?;
            reclaimed = reclaimed.saturating_add(candidate.snapshot.allocated_bytes);
        }
    }

    let manifest_dir = config
        .runner_manifest_dir
        .clone()
        .unwrap_or(runner_storage::default_config_dir().map_err(anyhow::Error::msg)?);
    if manifest_dir.exists() {
        for (id, report) in
            runner_storage::audit_registered_if_any(&manifest_dir).map_err(anyhow::Error::msg)?
        {
            writeln!(
                output,
                "RunnerReview\t{} candidates\t{}\t{}",
                report.candidate_count(),
                human_bytes(report.reclaimable_bytes()),
                id
            )?;
            if dry_run || report.candidate_count() == 0 || !interactive {
                continue;
            }
            if confirm(
                &mut input,
                &mut output,
                &format!(
                    "runner target {id} のadapter cleanupを適用しますか? controller停止などのgateはadapterが再検証します"
                ),
            )? {
                let (_, applied, _) = runner_storage::apply_registered(&manifest_dir, &id, &report)
                    .map_err(anyhow::Error::msg)?;
                reclaimed = reclaimed.saturating_add(applied.reclaimable_bytes());
            }
        }
    }
    writeln!(
        output,
        "clean-disk: removed allocated bytes {}（共有extentを含み、実際の空き増分とは異なります）",
        human_bytes(reclaimed)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "clean-disk-{label}-{suffix}-{}",
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn active_scope_is_a_hard_block_and_cannot_become_a_prompt() {
        let scope = PathBuf::from("/tmp/project");
        let candidate = Candidate {
            path: scope.join("target"),
            kind: CandidateKind::BuildCache,
            active_scope: scope.clone(),
            snapshot: TreeSnapshot {
                allocated_bytes: 1,
                entries: 1,
                newest_modified: SystemTime::UNIX_EPOCH,
                device: 1,
                inode: 1,
                contains_repository: false,
            },
        };
        let open = BTreeSet::from([scope.join("src/main.rs")]);
        assert_eq!(
            disposition(&candidate, &open, SystemTime::now(), 3, 30),
            Disposition::BlockedActive
        );
    }

    #[test]
    fn expired_trash_requires_confirmation_but_build_cache_is_automatic() {
        let snapshot = TreeSnapshot {
            allocated_bytes: 1,
            entries: 1,
            newest_modified: SystemTime::UNIX_EPOCH,
            device: 1,
            inode: 1,
            contains_repository: false,
        };
        let trash = Candidate {
            path: PathBuf::from("/tmp/.codex-trash/old"),
            kind: CandidateKind::Trash,
            active_scope: PathBuf::from("/tmp/.codex-trash/old"),
            snapshot: snapshot.clone(),
        };
        let build = Candidate {
            path: PathBuf::from("/tmp/project/target"),
            kind: CandidateKind::BuildCache,
            active_scope: PathBuf::from("/tmp/project"),
            snapshot,
        };
        let open = BTreeSet::new();
        assert_eq!(
            disposition(&trash, &open, SystemTime::now(), 3, 30),
            Disposition::Confirm
        );
        assert_eq!(
            disposition(&build, &open, SystemTime::now(), 3, 30),
            Disposition::AutoDelete
        );
    }

    #[test]
    fn confirmation_defaults_to_no() {
        let mut empty = io::Cursor::new(b"\n".to_vec());
        let mut yes = io::Cursor::new(b"y\n".to_vec());
        let mut output = Vec::new();
        assert!(!confirm(&mut empty, &mut output, "delete?").unwrap());
        assert!(confirm(&mut yes, &mut output, "delete?").unwrap());
    }

    #[test]
    fn unverifiable_candidates_do_not_hide_safe_candidates() {
        use std::os::unix::net::UnixListener;
        let root = TestDirectory::new("partial-inventory");
        let trash = root.0.join(".codex-trash");
        fs::create_dir_all(trash.join("unsafe")).unwrap();
        let _socket = UnixListener::bind(trash.join("unsafe/agent.sock")).unwrap();
        fs::create_dir_all(root.0.join("broken/target")).unwrap();
        fs::write(
            root.0.join("broken/.git"),
            "gitdir: /nonexistent-clean-disk-repo",
        )
        .unwrap();
        fs::create_dir(root.0.join("target")).unwrap();
        let found = discover_under(&root.0, true).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.0.join("target"));
        assert!(trash.join("unsafe/agent.sock").exists());
        assert!(root.0.join("broken/target").exists());
    }

    #[test]
    fn lsof_warning_is_allowed_only_for_a_disjoint_filesystem() {
        let scopes = BTreeSet::from([PathBuf::from("/media/storage/dev")]);
        let warning = b"lsof: WARNING: can't stat() fuse.portal file system /run/user/1000/doc\n      Output information may be incomplete.\n";
        assert!(validate_lsof_warnings(warning, &scopes).is_ok());

        let overlapping = b"lsof: WARNING: can't stat() fuse.portal file system /media/storage\n      Output information may be incomplete.\n";
        assert!(validate_lsof_warnings(overlapping, &scopes).is_err());
    }

    #[test]
    fn unknown_lsof_warning_fails_closed() {
        let scopes = BTreeSet::from([PathBuf::from("/media/storage/dev")]);
        assert!(validate_lsof_warnings(b"permission denied\n", &scopes).is_err());
        assert!(validate_lsof_warnings(b"", &scopes).is_ok());
    }

    #[test]
    fn nul_terminated_lsof_paths_are_parsed_without_text_loss() {
        let output = b"p10\0\nn/tmp/project/src/main.rs\0\nnpipe\0";
        assert_eq!(
            parse_lsof_paths(output).unwrap(),
            BTreeSet::from([PathBuf::from("/tmp/project/src/main.rs")])
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_lsof_paths_remain_observable() {
        use std::os::unix::ffi::OsStringExt;

        let paths = parse_lsof_paths(b"p10\0\nn/tmp/project/\xff\0").unwrap();
        assert!(paths.contains(&PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/project/\xff".to_vec()
        ))));
    }

    #[test]
    fn overlapping_scan_roots_are_detected_component_wise() {
        assert!(paths_overlap(
            Path::new("/tmp/products"),
            Path::new("/tmp/products/project")
        ));
        assert!(!paths_overlap(
            Path::new("/tmp/product"),
            Path::new("/tmp/products")
        ));
    }

    #[test]
    fn discovery_is_limited_to_trash_entries_and_cargo_targets() {
        let fixture = TestDirectory::new("discovery");
        let repository = fixture.0.join("repository");
        fs::create_dir(&repository).unwrap();
        fs::write(
            repository.join("Cargo.toml"),
            b"[package]\nname='fixture'\n",
        )
        .unwrap();
        fs::create_dir(repository.join("target")).unwrap();
        fs::write(repository.join("target/artifact"), b"artifact").unwrap();
        fs::create_dir(repository.join("other")).unwrap();
        fs::create_dir(repository.join(".codex-trash")).unwrap();
        fs::create_dir(repository.join(".codex-trash/old")).unwrap();
        fs::write(repository.join(".codex-trash/old/data"), b"trash").unwrap();

        let candidates = discover_under(&fixture.0, false).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|item| {
            item.kind == CandidateKind::BuildCache && item.path == repository.join("target")
        }));
        assert!(candidates.iter().any(|item| {
            item.kind == CandidateKind::Trash && item.path == repository.join(".codex-trash/old")
        }));
    }

    #[test]
    fn codex_targets_and_legacy_trash_are_discovered_without_cargo_manifest() {
        let fixture = TestDirectory::new("codex-discovery");
        for relative in [
            "repo/task-example/target",
            "repo/.artifact-backups/old",
            ".codex-trash-legacy/old",
            ".podman-example/vfs/dir",
            "repo/.artifacts/task-example",
        ] {
            fs::create_dir_all(fixture.0.join(relative)).unwrap();
        }
        let candidates = discover_under(&fixture.0, true).unwrap();
        assert_eq!(candidates.len(), 3);
        let target = candidates
            .iter()
            .find(|c| c.kind == CandidateKind::CodexBuildCache)
            .unwrap();
        assert!(target.path.ends_with("repo/task-example/target"));
        assert_eq!(
            disposition(target, &BTreeSet::new(), SystemTime::now(), 3, 1),
            Disposition::SkipRecent
        );
        assert_eq!(
            disposition(target, &BTreeSet::new(), SystemTime::now() + DAY * 2, 3, 1),
            Disposition::AutoDelete
        );
        remove_candidate(target).unwrap();
        assert!(!target.path.exists());
        assert!(fixture.0.join("repo/.artifacts/task-example").exists());
        assert!(fixture.0.join(".podman-example").exists());
    }

    #[test]
    fn a_worktree_named_target_is_not_a_cache() {
        let fixture = TestDirectory::new("target-is-source");
        fs::create_dir_all(fixture.0.join("target/.git")).unwrap();
        fs::write(fixture.0.join("target/source"), "preserve").unwrap();
        assert!(discover_under(&fixture.0, true).unwrap().is_empty());
        assert!(fixture.0.join("target/source").exists());
    }

    #[test]
    fn repository_at_or_above_scan_root_protects_source() {
        let fixture = TestDirectory::new("ancestor-repository");
        assert!(
            Command::new("/usr/bin/git")
                .args(["init", "--quiet"])
                .arg(&fixture.0)
                .status()
                .unwrap()
                .success()
        );
        for directory in [&fixture.0, &fixture.0.join("src")] {
            fs::create_dir_all(directory.join("target")).unwrap();
            fs::write(directory.join("Cargo.toml"), "[package]").unwrap();
            fs::write(directory.join("target/source"), "preserve").unwrap();
        }
        assert!(discover_under(&fixture.0, false).unwrap().is_empty());
        assert!(
            discover_under(&fixture.0.join("src"), false)
                .unwrap()
                .is_empty()
        );
        assert!(fixture.0.join("src/target/source").exists());
    }

    #[test]
    fn dangling_git_symlink_keeps_cache_out_of_candidates() {
        use std::os::unix::fs::symlink;
        let fixture = TestDirectory::new("dangling-git");
        fs::create_dir_all(fixture.0.join("repo/target")).unwrap();
        symlink("/nonexistent-clean-disk-git", fixture.0.join("repo/.git")).unwrap();
        assert!(discover_under(&fixture.0, true).unwrap().is_empty());
        assert!(fixture.0.join("repo/target").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_rejects_candidate_and_descendant_mounts_even_on_same_device() {
        let path = Path::new("/tmp/cache with space");
        for mount in [
            "/tmp/cache\\040with\\040space",
            "/tmp/cache\\040with\\040space/bind",
        ] {
            let mounts = format!("20 1 0:1 / {mount} rw - btrfs /dev/test rw");
            assert!(validate_mountinfo(path, &mounts).is_err());
        }
        assert!(
            validate_mountinfo(path, "20 1 0:1 / /tmp/cache-other rw - btrfs /dev/test rw").is_ok()
        );
        assert!(scan_path(Path::new("/proc")).is_err());
    }

    #[test]
    fn tracked_or_unignored_cache_is_never_a_candidate() {
        let fixture = TestDirectory::new("tracked-target");
        let repo = fixture.0.join("repository");
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("/usr/bin/git")
                    .current_dir(&repo)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "--quiet"]);
        fs::create_dir(repo.join("target")).unwrap();
        fs::write(repo.join("target/source.txt"), "preserve").unwrap();
        assert!(discover_under(&fixture.0, true).unwrap().is_empty());
        fs::write(repo.join(".gitignore"), "/target/\n").unwrap();
        let candidates = discover_under(&fixture.0, true).unwrap();
        assert_eq!(candidates.len(), 1);
        git(&["add", "-f", "--", "target/source.txt"]);
        assert!(remove_candidate(&candidates[0]).is_err());
        assert!(discover_under(&fixture.0, true).unwrap().is_empty());
    }

    #[test]
    fn deletion_revalidates_the_complete_tree_before_mutation() {
        let fixture = TestDirectory::new("revalidation");
        let target = fixture.0.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("artifact"), b"first").unwrap();
        let candidate = Candidate {
            path: target.clone(),
            kind: CandidateKind::BuildCache,
            active_scope: fixture.0.clone(),
            snapshot: scan_path(&target).unwrap(),
        };
        fs::write(target.join("new-artifact"), b"changed").unwrap();
        assert!(remove_candidate(&candidate).is_err());
        assert!(target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_candidate_never_follows_its_target() {
        use std::os::unix::fs::symlink;

        let fixture = TestDirectory::new("symlink-delete");
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();
        let link = fixture.0.join("candidate-link");
        symlink(&outside, &link).unwrap();
        let candidate = Candidate {
            path: link.clone(),
            kind: CandidateKind::Trash,
            active_scope: link.clone(),
            snapshot: scan_path(&link).unwrap(),
        };
        remove_candidate(&candidate).unwrap();
        assert!(!link.exists());
        assert!(outside.join("keep").exists());
    }
}
