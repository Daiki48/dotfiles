//! 通常セッションがCI待ちで完了責任を失わないためのStop hook。
//! 権限を付与せず、helperで開始した当該sessionの作業だけを追跡する。

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::codex_tools::task::store::{Lock, atomic_json, private_dir, read_json};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    task_id: String,
    github: String,
    repository: PathBuf,
    common_git_dir: PathBuf,
    worktree: PathBuf,
    branch: String,
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writer: Option<Writer>,
    // 再開しても最後の停止理由は監査用に保持する。
    exception: Option<Exception>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exception {
    boundary: Boundary,
    reason: String,
    at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Writer {
    pid: u32,
    start: u64,
    boot: String,
    namespace: PathBuf,
}

// 同じ PID が再利用されても別の CLI を旧担当とみなさない。
fn process(pid: u32) -> io::Result<(String, u32, u64)> {
    read_process(&Path::new("/proc").join(pid.to_string()))
}

fn read_process(directory: &Path) -> io::Result<(String, u32, u64)> {
    let stat = fs::read_to_string(directory.join("stat"))?;
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let (_, rest) = stat.split_once('(').ok_or_else(invalid)?;
    let (name, fields) = rest.rsplit_once(')').ok_or_else(invalid)?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let parent = fields
        .get(1)
        .and_then(|v| v.parse().ok())
        .ok_or_else(invalid)?;
    let start = fields
        .get(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(invalid)?;
    Ok((name.to_owned(), parent, start))
}

impl Writer {
    fn for_process(pid: u32, start: u64) -> Option<Self> {
        Some(Self {
            pid,
            start,
            boot: fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?,
            namespace: fs::read_link("/proc/self/ns/pid").ok()?,
        })
    }
    fn current() -> Option<Self> {
        let mut pid = std::process::id();
        while pid > 1 {
            let (name, parent, start) = process(pid).ok()?;
            if matches!(name.as_str(), "codex" | "claude") {
                return Self::for_process(pid, start);
            }
            pid = parent;
        }
        None
    }

    fn exited(&self) -> bool {
        if fs::read_to_string("/proc/sys/kernel/random/boot_id").is_ok_and(|boot| boot != self.boot)
        {
            return true;
        }
        if fs::read_link("/proc/self/ns/pid").ok().as_ref() != Some(&self.namespace) {
            return false;
        }
        match process(self.pid) {
            Ok((_, _, start)) => start != self.start,
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        }
    }
}

// 旧記録には PID がない。通常ホストで、その記録より前から生存する
// 同一 UID の Codex が一つもないことを確認できた場合だけ移行する。
#[cfg(target_os = "linux")]
fn legacy_writer_exited(record: &Path) -> bool {
    legacy_writer_exited_in(record, Path::new("/proc"))
}

#[cfg(target_os = "linux")]
fn legacy_writer_exited_in(record: &Path, processes: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::time::UNIX_EPOCH;
    let inspect = || -> Option<bool> {
        let init = fs::read_to_string(processes.join("1/comm")).ok()?;
        if !matches!(init.trim(), "systemd" | "init") {
            return None;
        }
        let written = fs::metadata(record)
            .ok()?
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs();
        let boot: u64 = fs::read_to_string(processes.join("stat"))
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .parse()
            .ok()?;
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks <= 0 {
            return None;
        }
        let uid = unsafe { libc::geteuid() };
        for item in fs::read_dir(processes).ok()? {
            let item = item.ok()?;
            let Some(_) = item
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let metadata = match item.metadata() {
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                result => result.ok()?,
            };
            if metadata.uid() != uid {
                continue;
            }
            let (name, _, start) = match read_process(&item.path()) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                result => result.ok()?,
            };
            if (name.starts_with("codex") || name == "claude")
                && boot + start / ticks as u64 <= written + 1
            {
                return Some(false);
            }
        }
        Some(true)
    };
    inspect() == Some(true)
}

#[cfg(not(target_os = "linux"))]
fn legacy_writer_exited(_: &Path) -> bool {
    false
}

pub(super) fn current_session() -> Option<String> {
    let claude = std::env::var("CLAUDE_CODE_SESSION_ID").ok();
    let codex = std::env::var("CODEX_THREAD_ID").ok();
    if claude.is_some() && codex.is_some() {
        let mut pid = std::process::id();
        while pid > 0 {
            let (name, parent, _) = process(pid).ok()?;
            match name.as_str() {
                "codex" => return codex,
                "claude" => break,
                _ => pid = parent,
            }
        }
    }
    claude.map(|id| format!("claude-{id}")).or(codex)
}

#[derive(Clone, Debug, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Boundary {
    UserStop,
    HumanDecision,
    Changes,
    Pr,
    Merge,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    version: u32,
    session_id: String,
    bindings: Vec<Binding>,
}

fn valid_session(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn session_path(home: &Path, id: &str, create: bool) -> Result<PathBuf> {
    if !valid_session(id) {
        return Err("session IDが不正です".into());
    }
    let root = home.join("worktrees");
    let directory = root.join(".sessions");
    reject_symlink_components(&directory).map_err(|e| e.to_string())?;
    if create {
        private_dir(&root)?;
        private_dir(&directory)?;
    } else if directory.exists() {
        safe_directory(&directory).map_err(|e| e.to_string())?;
    }
    let path = directory.join(format!("{id}.json"));
    if path.is_symlink() {
        return Err("session記録のsymlinkを拒否しました".into());
    }
    Ok(path)
}

fn load(path: &Path, id: &str) -> Result<Session> {
    let session: Session = read_json(path)?;
    if session.version != 1 || session.session_id != id || session.bindings.len() > 64 {
        return Err("session完了記録のidentityが不正です".into());
    }
    Ok(session)
}

fn repository(binding: &Binding) -> Repository {
    Repository {
        root: binding.repository.clone(),
        common_git_dir: binding.common_git_dir.clone(),
        github_name: binding.github.clone(),
        origin_url: String::new(),
        default_branch: String::new(),
        default_oid: String::new(),
    }
}

fn manifest(binding: &Binding) -> Result<Manifest> {
    if !valid_task_id(&binding.task_id) {
        return Err("完了記録のtask IDが不正です".into());
    }
    let repository = repository(binding);
    let (_, path, _) =
        managed_paths(&repository, &binding.task_id, false).map_err(|e| e.to_string())?;
    reject_symlink_components(path.parent().ok_or("manifestの親がありません")?)
        .map_err(|e| e.to_string())?;
    let mut file = crate::codex_tools::task::store::open_private(&path, false)?;
    let mut input = String::new();
    Read::by_ref(&mut file)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut input)
        .map_err(|_| "manifestを読み取れません")?;
    if input.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("manifestが上限を超えています".into());
    }
    let manifest = validated_manifest(
        &repository,
        &path,
        parse_manifest(&input).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if manifest.branch != binding.branch || Path::new(&manifest.worktree) != binding.worktree {
        return Err("担当作業場のidentityが変化しています".into());
    }
    Ok(manifest)
}

fn completed(binding: &Binding) -> Result<bool> {
    let manifest = manifest(binding)?;
    if binding
        .worktree
        .try_exists()
        .map_err(|_| "worktreeを検査できません")?
        || binding.worktree.is_symlink()
    {
        return Ok(false);
    }
    if completed_delivery(&manifest) {
        return Ok(true);
    }
    // retireはbranch削除後にこの状態を保存する。単なるdirectory不在では解除しない。
    Ok(manifest.status == "retired"
        && valid_oid(&manifest.detail)
        && matches!(
            crate::codex_tools::artifacts::validate(
                &binding.worktree,
                &binding.task_id,
                &binding.common_git_dir
            ),
            Ok(None)
        ))
}

fn bind(home: &Path, id: &str, binding: Binding) -> Result<()> {
    let path = session_path(home, id, true)?;
    let _lock = Lock::acquire_short(&path.with_extension("lock"))?;
    let mut session = if path
        .try_exists()
        .map_err(|_| "session記録を検査できません")?
    {
        load(&path, id)?
    } else {
        Session {
            version: 1,
            session_id: id.into(),
            bindings: vec![],
        }
    };
    if let Some(old) = session
        .bindings
        .iter_mut()
        .find(|old| old.github == binding.github && old.task_id == binding.task_id)
    {
        if old.worktree != binding.worktree
            || old.branch != binding.branch
            || old.repository != binding.repository
            || old.common_git_dir != binding.common_git_dir
        {
            return Err("既存sessionの担当identityが一致しません".into());
        }
        old.active = true;
        if binding.writer.is_some() {
            old.writer = binding.writer;
        }
    } else {
        if session.bindings.len() >= 64 {
            return Err("sessionの担当数が上限です".into());
        }
        session.bindings.push(binding);
    }
    atomic_json(&path, &session)
}

pub(super) fn bind_current(cwd: &Path, task_id: &str) -> Result<()> {
    let Some(id) = current_session() else {
        return Ok(());
    };
    if let Ok(marker) = std::env::var("CODEX_TASK_WORKER") {
        crate::codex_tools::task::completion_worker_scope(task_id, &marker)?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    let _worker = crate::codex_tools::task::standalone_writer_lease(task_id)?;
    let repo = inspect_repository(cwd, false, false).map_err(|e| e.to_string())?;
    // base取り込みの担当確認からGit操作まで、新しいsessionの担当追加を待たせる。
    let (_, _, lock_path) = managed_paths(&repo, task_id, false).map_err(|e| e.to_string())?;
    let _lifecycle = ExclusiveLock::acquire(&lock_path).map_err(|e| e.to_string())?;
    let manifest = load_manifests(&repo)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|(_, m)| m.task_id == task_id && m.status == "ready")
        .map(|(_, m)| m)
        .ok_or("担当manifestを確認できません")?;
    reject_other_active_sessions(
        &codex_home().map_err(|e| e.to_string())?,
        &manifest,
        Some(&id),
    )?;
    bind(
        &codex_home().map_err(|e| e.to_string())?,
        &id,
        Binding {
            task_id: task_id.into(),
            github: repo.github_name,
            repository: repo.root,
            common_git_dir: repo.common_git_dir,
            worktree: manifest.worktree.into(),
            branch: manifest.branch,
            active: true,
            writer: Writer::current(),
            exception: None,
        },
    )
}

pub(super) fn lock_merge_binding(
    home: &Path,
    id: &str,
    repository: &Repository,
    target: &Manifest,
) -> Result<Lock> {
    let path = session_path(home, id, false)?;
    let lock = Lock::acquire_short(&path.with_extension("lock"))?;
    let session = load(&path, id)?;
    let binding = session
        .bindings
        .iter()
        .find(|binding| {
            binding.active
                && !binding.writer.as_ref().is_some_and(Writer::exited)
                && binding.task_id == target.task_id
                && binding.repository == repository.root
                && binding.common_git_dir == repository.common_git_dir
                && binding.github == repository.github_name
        })
        .ok_or("現在のsessionの有効な担当記録がありません")?;
    if manifest(binding)? != *target {
        return Err("現在のsessionと担当worktreeが一致しません".into());
    }
    Ok(lock)
}

/// lifecycle lockの保持中に呼ぶ。担当解除はatomicで、別sessionの新規担当は同じlockを通る。
pub(super) fn reject_other_active_sessions(
    home: &Path,
    target: &Manifest,
    owner: Option<&str>,
) -> Result<()> {
    if conflicting_session(home, owner, |binding| {
        binding.worktree == Path::new(&target.worktree)
            || binding.task_id == target.task_id
                && binding.common_git_dir == Path::new(&target.common_git_dir)
    })? {
        return Err(
            "別sessionに有効な担当記録があります。担当を照合してから再試行してください".into(),
        );
    }
    Ok(())
}

pub(crate) fn worker_available(home: &Path, root: &Path, github: &str, id: &str) -> Result<bool> {
    conflicting_session(home, None, |binding| {
        binding.repository == root && binding.github == github && binding.task_id == id
    })
    .map(|busy| !busy)
}

fn conflicting_session(
    home: &Path,
    owner: Option<&str>,
    matches: impl Fn(&Binding) -> bool,
) -> Result<bool> {
    let directory = home.join("worktrees/.sessions");
    reject_symlink_components(&directory).map_err(|e| e.to_string())?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("担当session一覧を安全に確認できません".into()),
    };
    safe_directory(&directory).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "担当session一覧を安全に確認できません")?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return Err("担当session記録が通常fileではありません".into());
        }
        let id = path
            .file_stem()
            .and_then(OsStr::to_str)
            .filter(|id| valid_session(id))
            .ok_or("担当session記録の名前が不正です")?;
        if Some(id) == owner {
            continue;
        }
        let session = load(&path, id)?;
        if session.bindings.iter().any(|binding| {
            binding.active
                && matches(binding)
                && !binding
                    .writer
                    .as_ref()
                    .map_or_else(|| legacy_writer_exited(&path), Writer::exited)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Parser)]
#[command(name = "codex-worktree completion")]
struct Args {
    #[arg(long)]
    task_id: String,
    #[arg(long, value_enum, requires = "reason")]
    boundary: Option<Boundary>,
    #[arg(long, requires = "boundary")]
    reason: Option<String>,
}

pub(super) fn command(args: &[OsString]) -> Result<()> {
    let args = Args::try_parse_from(
        std::iter::once(OsString::from("completion")).chain(args.iter().cloned()),
    )
    .map_err(|e| e.to_string())?;
    let id = current_session().ok_or("現在の CLI session IDが必要です")?;
    let path = session_path(&codex_home().map_err(|e| e.to_string())?, &id, false)?;
    let _lock = Lock::acquire_short(&path.with_extension("lock"))?;
    let mut session = load(&path, &id)?;
    let cwd = std::env::current_dir().map_err(|_| "current directoryを取得できません")?;
    let repo = inspect_repository(&cwd, false, false).map_err(|e| e.to_string())?;
    let binding = session
        .bindings
        .iter_mut()
        .find(|b| {
            b.task_id == args.task_id && b.repository == repo.root && b.github == repo.github_name
        })
        .ok_or("現在のsessionが担当するtaskではありません")?;
    manifest(binding)?;
    if let Some(boundary) = args.boundary {
        let reason = args.reason.unwrap_or_default();
        if reason.trim().is_empty() || reason.len() > 2000 || reason.chars().any(char::is_control) {
            return Err("境界の根拠を2000 byte以内の1行で指定してください".into());
        }
        binding.active = false;
        binding.exception = Some(Exception {
            boundary,
            reason,
            at: format_timestamp(),
        });
        atomic_json(&path, &session)?;
        println!("指定境界を記録しました。task完了を意味しません。");
    } else {
        let mut summary = serde_json::to_value(binding).map_err(|e| e.to_string())?;
        if let Some(fields) = summary.as_object_mut() {
            fields.remove("writer");
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

#[derive(Deserialize)]
struct StopInput {
    session_id: String,
    hook_event_name: String,
    #[serde(default)]
    stop_hook_active: bool,
}

fn check(home: &Path, input: &StopInput) -> Result<Value> {
    if input.hook_event_name != "Stop" {
        return Err("Stop以外のeventは受理しません".into());
    }
    let path = session_path(home, &input.session_id, false)?;
    if !path
        .try_exists()
        .map_err(|_| "session記録を検査できません")?
    {
        return Ok(json!({}));
    }
    let _lock = Lock::acquire_short(&path.with_extension("lock"))?;
    let mut session = load(&path, &input.session_id)?;
    let mut pending = Vec::new();
    let mut transferred = false;
    for binding in session.bindings.iter_mut().filter(|b| b.active) {
        if completed(binding)? {
            continue;
        }
        if crate::codex_tools::task::closed_unmerged(
            home,
            &binding.repository,
            &binding.worktree,
            &binding.task_id,
            &binding.branch,
        )? {
            continue;
        }
        if crate::codex_tools::task::monitoring(
            home,
            &binding.repository,
            &binding.worktree,
            &binding.task_id,
            &binding.branch,
        )? {
            binding.active = false;
            transferred = true;
        } else {
            pending.push(binding.task_id.clone());
        }
    }
    if transferred {
        atomic_json(&path, &session)?;
    }
    if pending.is_empty() || input.stop_hook_active {
        return Ok(json!({}));
    }
    // 任意のログ・Issue本文・assistant最終文は継続promptへ挿入しない。
    Ok(json!({"decision":"block", "reason":format!(
        "担当task {} の完了記録を確認できません。実行可能な工程は続けてください。外部待機なら同じtaskを監視するcoordinatorと次回観測を照合し、状況を返してください。引き継げない場合は理由と再開点を返してください。", pending.join(", "))}))
}

pub(crate) fn hook_entrypoint(claude: bool) -> i32 {
    let result = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Stop入力を読み取れません")?;
        if bytes.len() > 1024 * 1024 {
            return Err("Stop入力が上限を超えています".into());
        }
        let mut input: StopInput =
            serde_json::from_slice(&bytes).map_err(|_| "Stop入力の形式が不正です")?;
        if claude {
            input.session_id = format!("claude-{}", input.session_id);
        }
        check(&codex_home().map_err(|e| e.to_string())?, &input)
    })();
    let output = result.unwrap_or_else(|_| json!({"decision":"block", "reason":
        "session完了記録を安全に検証できません。完了と報告せず、managed helperの状態・所有・設定を診断してください。状態の削除やhook無効化で迂回せず、修復不能なら具体的な根拠を利用者へ伝えてください。"}));
    println!("{output}");
    0
}

#[cfg(test)]
mod tests {
    use super::super::tests::TemporaryRepository;
    use super::*;

    fn fixture() -> (TemporaryRepository, Binding) {
        let fixture = TemporaryRepository::new();
        let path = create_worktree(
            &fixture.repository,
            "fix/completion",
            "task-completion",
            true,
        )
        .unwrap();
        let repo = inspect_repository(&fixture.repository, true, false).unwrap();
        let binding = Binding {
            task_id: "task-completion".into(),
            github: repo.github_name,
            repository: repo.root,
            common_git_dir: repo.common_git_dir,
            worktree: path,
            branch: "fix/completion".into(),
            active: true,
            writer: process(std::process::id())
                .ok()
                .and_then(|(_, _, start)| Writer::for_process(std::process::id(), start)),
            exception: None,
        };
        (fixture, binding)
    }

    fn stop(home: &Path, id: &str) -> Value {
        check(
            home,
            &StopInput {
                session_id: id.into(),
                hook_event_name: "Stop".into(),
                stop_hook_active: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn merge_base_requires_one_active_session_or_no_standalone_session_for_worker() {
        let (fixture, mut binding) = fixture();
        let repository = repository(&binding);
        let target = manifest(&binding).unwrap();
        let home = &fixture.codex_home;
        assert!(lock_merge_binding(home, "absent", &repository, &target).is_err());
        bind(home, "owner", binding.clone()).unwrap();
        let _session = lock_merge_binding(home, "owner", &repository, &target).unwrap();
        assert!(reject_other_active_sessions(home, &target, Some("owner")).is_ok());
        assert!(reject_other_active_sessions(home, &target, None).is_err());
        bind(home, "other", binding.clone()).unwrap();
        assert!(reject_other_active_sessions(home, &target, Some("owner")).is_err());

        let other_path = session_path(home, "other", false).unwrap();
        let mut other = load(&other_path, "other").unwrap();
        other.bindings[0].active = false;
        atomic_json(&other_path, &other).unwrap();
        assert!(reject_other_active_sessions(home, &target, Some("owner")).is_ok());
        binding.task_id = "task-unrelated".into();
        binding.worktree.set_file_name("task-unrelated");
        bind(home, "unrelated", binding).unwrap();
        assert!(reject_other_active_sessions(home, &target, Some("owner")).is_ok());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn legacy_writer_requires_an_unambiguous_host_process_scan() {
        let (fixture, _) = fixture();
        let root = fixture.codex_home.join("synthetic-proc");
        fs::create_dir_all(root.join("1")).unwrap();
        fs::write(root.join("1/comm"), "systemd\n").unwrap();
        fs::write(root.join("stat"), "btime 1\n").unwrap();
        let record = fixture.codex_home.join("legacy-record");
        fs::write(&record, "{}").unwrap();
        let pid = root.join("20");
        fs::create_dir(&pid).unwrap();
        let stat = format!("20 (codex) S {}2", "0 ".repeat(18));
        fs::write(pid.join("stat"), &stat).unwrap();
        assert!(!legacy_writer_exited_in(&record, &root));
        fs::write(pid.join("stat"), stat.replace("(codex)", "(sh)")).unwrap();
        assert!(legacy_writer_exited_in(&record, &root));
        fs::write(root.join("1/comm"), "isolated").unwrap();
        assert!(!legacy_writer_exited_in(&record, &root));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn limited_writer_can_be_replaced_in_both_directions_with_dirty_worktree() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for (old, next) in [
            ("codex-example", "claude-example"),
            ("claude-example", "codex-example"),
        ] {
            let (fixture, mut binding) = fixture();
            let next_writer = binding.writer.clone();
            let tracked = binding.worktree.join("README.md");
            let partial = binding.worktree.join("unverified.txt");
            fs::write(&tracked, "未検証の変更").unwrap();
            fs::write(&partial, "取得済みの途中結果").unwrap();
            // 制限時に終了する CLI を模擬し、旧担当からの解放操作を要求しない。
            let mut child = Command::new("sh")
                .args(["-c", "read answer; exit 75"])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let pid = child.id();
            binding.writer = Some(Writer {
                pid,
                start: process(pid).unwrap().2,
                namespace: fs::read_link("/proc/self/ns/pid").unwrap(),
                boot: fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap(),
            });
            let home = &fixture.codex_home;
            bind(home, old, binding.clone()).unwrap();
            let target = manifest(&binding).unwrap();
            assert!(reject_other_active_sessions(home, &target, Some(next)).is_err());
            child.stdin.take().unwrap().write_all(b"limited\n").unwrap();
            assert_eq!(child.wait().unwrap().code(), Some(75));
            assert!(reject_other_active_sessions(home, &target, Some(next)).is_ok());
            let resumed =
                super::super::resume(&binding.repository, &binding.task_id, true).unwrap();
            assert_eq!(resumed, binding.worktree);
            binding.writer = next_writer;
            bind(home, next, binding).unwrap();
            assert_eq!(fs::read_to_string(tracked).unwrap(), "未検証の変更");
            assert_eq!(fs::read_to_string(partial).unwrap(), "取得済みの途中結果");
            assert!(reject_other_active_sessions(home, &target, Some(old)).is_err());
        }
    }

    #[test]
    fn ci_start_success_and_merge_do_not_release_session_before_finish_cleanup() {
        let (fixture, binding) = fixture();
        bind(&fixture.codex_home, "owner", binding.clone()).unwrap();
        let receipt = binding
            .worktree
            .parent()
            .unwrap()
            .join(".state/task-completion.delivery.json");
        for stage in ["reviewed", "merged", "cleanup_complete", "completed"] {
            atomic_json(
                &receipt,
                &json!({"version": 1, "kind": "delivery", "stage": stage,
                "task_id": binding.task_id, "repository": binding.github,
                "branch": binding.branch, "head_sha": "a".repeat(40)}),
            )
            .unwrap();
            assert_eq!(stop(&fixture.codex_home, "owner")["decision"], "block");
        }
        // 完了receiptがあってもworktreeが存在する間は継続。無関係sessionは対象外。
        assert_eq!(stop(&fixture.codex_home, "reader"), json!({}));
        fs::remove_dir_all(&binding.worktree).unwrap();
        assert_eq!(stop(&fixture.codex_home, "owner"), json!({}));
        fs::create_dir(&binding.worktree).unwrap();
        assert_eq!(stop(&fixture.codex_home, "owner")["decision"], "block");
    }

    #[test]
    fn missing_worktree_without_finish_proof_keeps_continuing() {
        let (fixture, binding) = fixture();
        bind(&fixture.codex_home, "owner", binding.clone()).unwrap();
        fs::remove_dir_all(&binding.worktree).unwrap();
        for _ in 0..3 {
            assert_eq!(stop(&fixture.codex_home, "owner")["decision"], "block");
        }
    }

    #[test]
    fn status_response_keeps_the_pending_binding_for_next_turn() {
        let (fixture, binding) = fixture();
        let home = &fixture.codex_home;
        bind(home, "example", binding.clone()).unwrap();
        assert_eq!(stop(home, "example")["decision"], "block");
        let input = StopInput {
            session_id: "example".into(),
            hook_event_name: "Stop".into(),
            stop_hook_active: true,
        };
        assert_eq!(check(home, &input).unwrap(), json!({}));
        assert!(!completed(&binding).unwrap());
        assert_eq!(stop(home, "example")["decision"], "block");
    }

    #[test]
    fn explicit_exception_is_not_completion_and_resume_reactivates_it() {
        let (fixture, mut binding) = fixture();
        binding.active = false;
        binding.exception = Some(Exception {
            boundary: Boundary::UserStop,
            reason: "利用者が引継ぎ停止を指示".into(),
            at: format_timestamp(),
        });
        bind(&fixture.codex_home, "owner", binding.clone()).unwrap();
        assert_eq!(stop(&fixture.codex_home, "owner"), json!({}));
        assert!(!completed(&binding).unwrap());
        binding.active = true;
        bind(&fixture.codex_home, "owner", binding).unwrap();
        assert_eq!(stop(&fixture.codex_home, "owner")["decision"], "block");
        let path = session_path(&fixture.codex_home, "owner", false).unwrap();
        assert!(
            load(&path, "owner").unwrap().bindings[0]
                .exception
                .is_some()
        );
        assert!(
            Args::try_parse_from([
                "completion",
                "--task-id",
                "task-completion",
                "--boundary",
                "ci-wait",
                "--reason",
                "pending"
            ])
            .is_err()
        );
    }

    #[test]
    fn session_storage_rejects_aliases_corruption_and_changed_identity() {
        let (fixture, binding) = fixture();
        assert!(session_path(&fixture.codex_home, "../../owner", false).is_err());
        bind(&fixture.codex_home, "owner", binding.clone()).unwrap();
        let mut changed = binding;
        changed.branch = "fix/other".into();
        assert!(bind(&fixture.codex_home, "owner", changed).is_err());
        let path = session_path(&fixture.codex_home, "owner", false).unwrap();
        assert!(load(&path, "other").is_err());
        fs::write(&path, "{}").unwrap();
        assert!(
            check(
                &fixture.codex_home,
                &StopInput {
                    session_id: "owner".into(),
                    hook_event_name: "Stop".into(),
                    stop_hook_active: false,
                }
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink("missing", &path).unwrap();
            assert!(session_path(&fixture.codex_home, "owner", false).is_err());
        }
    }
}
