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
    } else {
        if session.bindings.len() >= 64 {
            return Err("sessionの担当数が上限です".into());
        }
        session.bindings.push(binding);
    }
    atomic_json(&path, &session)
}

pub(super) fn bind_current(cwd: &Path, task_id: &str) -> Result<()> {
    let Some(id) = std::env::var_os("CODEX_THREAD_ID") else {
        return Ok(());
    };
    let id = id.to_str().ok_or("session IDがUTF-8ではありません")?;
    if let Ok(marker) = std::env::var("CODEX_TASK_WORKER") {
        crate::codex_tools::task::completion_worker_scope(task_id, &marker)?;
        return Ok(());
    }
    let repo = inspect_repository(cwd, false, false).map_err(|e| e.to_string())?;
    let manifest = load_manifests(&repo)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|(_, m)| m.task_id == task_id && m.status == "ready")
        .map(|(_, m)| m)
        .ok_or("担当manifestを確認できません")?;
    bind(
        &codex_home().map_err(|e| e.to_string())?,
        id,
        Binding {
            task_id: task_id.into(),
            github: repo.github_name,
            repository: repo.root,
            common_git_dir: repo.common_git_dir,
            worktree: manifest.worktree.into(),
            branch: manifest.branch,
            active: true,
            exception: None,
        },
    )
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
    let id = std::env::var("CODEX_THREAD_ID").map_err(|_| "現在のCodex session IDが必要です")?;
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
        println!(
            "{}",
            serde_json::to_string_pretty(binding).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

#[derive(Deserialize)]
struct StopInput {
    session_id: String,
    hook_event_name: String,
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
    let session = load(&path, &input.session_id)?;
    let mut pending = Vec::new();
    for binding in session.bindings.iter().filter(|b| b.active) {
        if !completed(binding)? {
            pending.push(binding.task_id.as_str());
        }
    }
    if pending.is_empty() {
        return Ok(json!({}));
    }
    // 任意のログ・Issue本文・assistant最終文は継続promptへ挿入しない。
    Ok(json!({"decision":"block", "reason":format!(
        "担当task {} はfinish・Issue記録・main同期・物理cleanupまで未完了です。CI開始・待機・成功だけで終了しないでください。現在の固定headと担当を照合し、CIを監視して失敗時は診断・許可内の修復、成功後はreview・deliver・main CI・finishまで続けてください。外部待機中は次回観測までtoolで待機し、同条件の無限retryをしないでください。新writerやtaskを追加せず既存担当を保護してください。利用者が明示停止・途中納品を指定した場合、または真正な人間判断が必要な場合だけ、codex-worktree completionで境界と具体的根拠を保存して返してください。技術的失敗やCI待ちは解除理由になりません。", pending.join(", "))}))
}

pub(crate) fn hook_entrypoint() -> i32 {
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
        let input: StopInput =
            serde_json::from_slice(&bytes).map_err(|_| "Stop入力の形式が不正です")?;
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
            },
        )
        .unwrap()
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
                    hook_event_name: "Stop".into()
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
