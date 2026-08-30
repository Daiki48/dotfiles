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
    runner_manifest_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CandidateKind {
    Trash,
    BuildCache,
}

impl CandidateKind {
    fn label(self) -> &'static str {
        match self {
            Self::Trash => "期限切れtrash",
            Self::BuildCache => "再生成可能build cache",
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
    if !(1..=3650).contains(&config.trash_retention_days)
        || !(1..=3650).contains(&config.build_cache_retention_days)
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
    })
}

fn nearest_repository_root(start: &Path, boundary: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        if current == boundary {
            return None;
        }
        current = current.parent()?;
    }
}

fn discover_under(root: &Path, include_build_cache: bool) -> Result<Vec<Candidate>> {
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
            if name == ".codex-trash" {
                let active_scope =
                    nearest_repository_root(&directory, root).unwrap_or_else(|| path.clone());
                for child in sorted_entries(&path)? {
                    let child_path = child.path();
                    candidates.push(Candidate {
                        snapshot: scan_path(&child_path)?,
                        path: child_path,
                        kind: CandidateKind::Trash,
                        active_scope: active_scope.clone(),
                    });
                }
                continue;
            }
            if include_build_cache && name == "target" && directory.join("Cargo.toml").is_file() {
                candidates.push(Candidate {
                    snapshot: scan_path(&path)?,
                    path,
                    kind: CandidateKind::BuildCache,
                    active_scope: directory.clone(),
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

fn observe_open_paths() -> Result<BTreeSet<PathBuf>> {
    let lsof = trust::trusted_system_binary("/usr/bin/lsof", "lsof").map_err(anyhow::Error::msg)?;
    let mut command = Command::new(lsof);
    process::clear_environment(&mut command);
    command
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .args(["-nP", "-F", "n"]);
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
    let mut paths = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(value) = line.strip_prefix('n') else {
            continue;
        };
        let path = PathBuf::from(value.strip_suffix(" (deleted)").unwrap_or(value));
        if path.is_absolute() {
            paths.insert(path);
        }
    }
    Ok(paths)
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
        CandidateKind::BuildCache => build_retention_days,
    };
    let age = now
        .duration_since(candidate.snapshot.newest_modified)
        .unwrap_or_default();
    if age < DAY.saturating_mul(u32::try_from(retention_days).unwrap_or(u32::MAX)) {
        return Disposition::SkipRecent;
    }
    match candidate.kind {
        CandidateKind::Trash => Disposition::Confirm,
        CandidateKind::BuildCache => Disposition::AutoDelete,
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
    let current = scan_path(&candidate.path)?;
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

    let mut candidates = Vec::new();
    for root in &config.scan_roots {
        if !root.exists() {
            println!("SKIP missing scan root: {}", root.display());
            continue;
        }
        candidates.extend(discover_under(root, true)?);
    }
    if let Some(root) = codex_worktree_root()
        && root.exists()
        && !config.scan_roots.iter().any(|scan| root.starts_with(scan))
    {
        candidates.extend(discover_under(&root, false)?);
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    let open_paths = observe_open_paths().context(
        "open file observation failed; no cleanup was performed because active sessions cannot be excluded",
    )?;

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
            config.build_cache_retention_days,
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
        let latest_open_paths = observe_open_paths().context(
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
            runner_storage::audit_registered(&manifest_dir).map_err(anyhow::Error::msg)?
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
    writeln!(output, "clean-disk: reclaimed {}", human_bytes(reclaimed))?;
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

        let candidates = discover_under(&fixture.0, true).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|item| {
            item.kind == CandidateKind::BuildCache && item.path == repository.join("target")
        }));
        assert!(candidates.iter().any(|item| {
            item.kind == CandidateKind::Trash && item.path == repository.join(".codex-trash/old")
        }));
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
