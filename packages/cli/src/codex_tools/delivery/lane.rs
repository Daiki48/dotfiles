//! 同一host/CODEX_HOMEのCI承認からfinishまでを直列化する。時刻やprocess終了で所有を解放しない。
use super::*;
use serde::{Deserialize, Serialize};

pub(crate) const BUSY: &str = "先行PRのCI・統合・main同期・cleanupの完了を待っています";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    pub repository: String,
    pub task: String,
    pub pr: u64,
    pub head: String,
    pub base: String,
    pub labels: Vec<String>,
}

impl Scope {
    fn validate(&self) -> Result<()> {
        if !repository_name(&self.repository)
            || task_id(&self.task).is_err()
            || self.pr == 0
            || self.pr > 999_999_999
            || oid(&self.head, "CI直列枠head").is_err()
            || oid(&self.base, "CI直列枠base").is_err()
            || !super::super::task::valid_ci_labels(&self.labels)
        {
            return Err(error("CI直列枠の対象が不正です"));
        }
        Ok(())
    }
    fn same_pr(&self, other: &Self) -> bool {
        self.repository == other.repository && self.task == other.task && self.pr == other.pr
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    owner: Option<Scope>,
    suspended: Vec<Scope>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            owner: None,
            suspended: vec![],
        }
    }
}

fn validate(state: &State) -> Result<()> {
    if state.version != 1
        || state.suspended.len() > 4
        || state.owner.is_none() && !state.suspended.is_empty()
    {
        return Err(error("CI直列枠のschemaが不正です"));
    }
    let mut identities = HashSet::new();
    for scope in state.owner.iter().chain(state.suspended.iter()) {
        scope.validate()?;
        if !identities.insert((&scope.repository, &scope.task, scope.pr)) {
            return Err(error("CI直列枠の修復経路が循環しています"));
        }
    }
    Ok(())
}

fn claim_state(state: &mut State, scope: Scope, repair_parent: Option<&str>) -> Result<()> {
    validate(state)?;
    scope.validate()?;
    if let Some(owner) = &state.owner
        && !owner.same_pr(&scope)
    {
        if repair_parent == Some(owner.task.as_str())
            && owner.repository == scope.repository
            && state.suspended.len() < 4
        {
            state.suspended.push(owner.clone());
        } else {
            return Err(error(BUSY));
        }
    }
    let mut scope = scope;
    if let Some(owner) = &state.owner
        && owner.same_pr(&scope)
    {
        scope.labels.extend(owner.labels.iter().cloned());
        scope.labels.sort();
        scope.labels.dedup();
    }
    state.owner = Some(scope);
    validate(state)
}

fn release_state(state: &mut State, repository: &str, task: &str, pr: u64) -> Result<()> {
    validate(state)?;
    if let Some(owner) = &state.owner {
        if owner.repository != repository || owner.task != task || owner.pr != pr {
            return Err(error(BUSY));
        }
        state.owner = state.suspended.pop();
    }
    Ok(())
}

fn root() -> Result<PathBuf> {
    root_at(&codex_home()?)
}

fn root_at(home: &Path) -> Result<PathBuf> {
    let worktrees = home.join("worktrees");
    safe_directory(&worktrees)?;
    let root = worktrees.join(".delivery-lane");
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(&root) {
        Ok(()) => (),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(error("CI直列枠のprivate directoryを作成できません")),
    }
    safe_directory(&root)?;
    Ok(root)
}

fn read(root: &Path) -> Result<State> {
    let path = root.join("state.json");
    if !path.exists() && !path.is_symlink() {
        return Ok(State::default());
    }
    let state = serde_json::from_value(read_json_file(&path, "CI直列枠")?)
        .map_err(|_| error("CI直列枠のschemaが不正です"))?;
    validate(&state)?;
    Ok(state)
}

fn update(root: &Path, change: impl FnOnce(&mut State) -> Result<()>) -> Result<()> {
    // flockは短い状態更新だけに保持し、API・CI・worker待機中は保持しない。
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut state = read(root)?;
    change(&mut state)?;
    validate(&state)?;
    atomic_json(
        &root.join("state.json"),
        &serde_json::to_value(state).map_err(|_| error("CI直列枠を保存できません"))?,
    )
}

pub(super) fn claim(scope: Scope, repair_parent: Option<&str>) -> Result<()> {
    update(&root()?, |state| claim_state(state, scope, repair_parent))
}

pub(super) fn release(repository: &str, task: &str, pr: u64) -> Result<()> {
    update(&root()?, |state| {
        // completed済みfinishの再実行で、後続PRの所有を変更しない。
        if state.owner.as_ref().is_some_and(|owner| {
            owner.repository == repository && owner.task == task && owner.pr == pr
        }) {
            release_state(state, repository, task, pr)?;
        }
        Ok(())
    })
}

pub(super) fn available(
    home: &Path,
    repository: &str,
    task: &str,
    pr: u64,
    repair_parent: Option<&str>,
) -> Result<bool> {
    let worktrees = home.join("worktrees");
    if !worktrees.exists() && !worktrees.is_symlink() {
        return Ok(true);
    }
    safe_directory(&worktrees)?;
    let root = worktrees.join(".delivery-lane");
    if !root.exists() && !root.is_symlink() {
        return Ok(true);
    }
    safe_directory(&root)?;
    let state = read(&root)?;
    Ok(state.owner.is_none_or(|owner| {
        owner.repository == repository
            && (owner.task == task && owner.pr == pr || repair_parent == Some(owner.task.as_str()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_admission_is_persisted_until_the_matching_finish() {
        let root = std::env::temp_dir().join(format!(
            "delivery-lane-{}-{}",
            std::process::id(),
            now().replace(':', "-")
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("worktrees"))
            .unwrap();
        let path = root_at(&root).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (1..=8)
            .map(|pr| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    update(&path, |state| {
                        claim_state(state, scope(&format!("task-{pr}"), pr), None)
                    })
                    .is_ok()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let owner = read(&path).unwrap().owner.unwrap();
        assert!(!available(&root, "example/other", "task-other", 99, None).unwrap());
        assert!(available(&root, &owner.repository, &owner.task, owner.pr, None).unwrap());
        assert!(
            update(&path, |state| release_state(
                state,
                "example/product",
                "task-other",
                99
            ))
            .is_err()
        );
        assert_eq!(read(&path).unwrap().owner, Some(owner.clone()));
        update(&path, |state| {
            release_state(state, &owner.repository, &owner.task, owner.pr)
        })
        .unwrap();
        assert!(available(&root, "example/other", "task-other", 99, None).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    fn scope(task: &str, pr: u64) -> Scope {
        Scope {
            repository: "example/product".into(),
            task: task.into(),
            pr,
            head: "a".repeat(40),
            base: "b".repeat(40),
            labels: vec!["ci-approved".into()],
        }
    }

    #[test]
    fn one_pr_keeps_the_lane_through_restart_updates_and_finish() {
        let mut state = State::default();
        claim_state(&mut state, scope("task-first", 1), None).unwrap();
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            claim_state(&mut state, scope("task-second", 2), None)
                .unwrap_err()
                .to_string(),
            BUSY
        );
        let mut updated = scope("task-first", 1);
        updated.head = "c".repeat(40);
        claim_state(&mut state, updated.clone(), None).unwrap();
        assert_eq!(state.owner.as_ref().unwrap().head, updated.head);
        assert!(release_state(&mut state, "example/product", "task-second", 2).is_err());
        release_state(&mut state, "example/product", "task-first", 1).unwrap();
        claim_state(&mut state, scope("task-second", 2), None).unwrap();
    }

    #[test]
    fn another_repository_is_serialized_without_disclosing_its_identity() {
        let mut state = State::default();
        claim_state(&mut state, scope("task-first", 1), None).unwrap();
        let mut other = scope("task-second", 2);
        other.repository = "example/another-product".into();
        let error = claim_state(&mut state, other, None)
            .unwrap_err()
            .to_string();
        assert_eq!(error, BUSY);
        assert!(!error.contains("product"));
    }

    #[test]
    fn verified_main_repair_temporarily_owns_the_lane_then_restores_the_parent() {
        let mut state = State::default();
        claim_state(&mut state, scope("task-parent", 1), None).unwrap();
        assert!(claim_state(&mut state, scope("task-repair", 2), None).is_err());
        claim_state(&mut state, scope("task-repair", 2), Some("task-parent")).unwrap();
        assert!(claim_state(&mut state, scope("task-unrelated", 3), None).is_err());
        release_state(&mut state, "example/product", "task-repair", 2).unwrap();
        assert_eq!(state.owner.as_ref().unwrap().task, "task-parent");
        release_state(&mut state, "example/product", "task-parent", 1).unwrap();
        assert!(state.owner.is_none());
    }
}
