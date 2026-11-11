//! Repository単位の納品予約。時刻やprocess終了で所有を解放しない。
use super::*;
use serde::{Deserialize, Serialize};

pub(crate) const BUSY: &str = "同じrepositoryの先行PRの納品完了を待っています";

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
            || oid(&self.head, "納品予約head").is_err()
            || oid(&self.base, "納品予約base").is_err()
            || !super::super::task::valid_ci_labels(&self.labels)
        {
            return Err(error("納品予約の対象が不正です"));
        }
        Ok(())
    }
    fn same_pr(&self, other: &Self) -> bool {
        self.repository.eq_ignore_ascii_case(&other.repository)
            && self.task == other.task
            && self.pr == other.pr
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
        return Err(error("納品予約のschemaが不正です"));
    }
    let mut identities = HashSet::new();
    for scope in state.owner.iter().chain(state.suspended.iter()) {
        scope.validate()?;
        if !identities.insert((scope.repository.to_ascii_lowercase(), &scope.task, scope.pr)) {
            return Err(error("納品予約の修復経路が循環しています"));
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
            && owner.repository.eq_ignore_ascii_case(&scope.repository)
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
        if !owner.repository.eq_ignore_ascii_case(repository)
            || owner.task != task
            || owner.pr != pr
        {
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
        Err(_) => return Err(error("納品予約のprivate directoryを作成できません")),
    }
    safe_directory(&root)?;
    Ok(root)
}

#[derive(Deserialize)]
struct LegacyReservation {
    lane: State,
}

fn legacy_repository_state(root: &Path, repository: &str) -> Result<State> {
    let path = root.join("state.json");
    if !path.exists() && !path.is_symlink() {
        return Ok(State::default());
    }
    let value = read_json_file(&path, "旧納品予約")?;
    let lanes = match value["version"].as_u64() {
        Some(1) => {
            vec![serde_json::from_value(value).map_err(|_| error("旧納品予約のschemaが不正です"))?]
        }
        Some(2) => {
            let reservations: Vec<LegacyReservation> =
                serde_json::from_value(value["reservations"].clone())
                    .map_err(|_| error("旧納品予約のschemaが不正です"))?;
            reservations
                .into_iter()
                .map(|reservation| reservation.lane)
                .collect()
        }
        _ => return Err(error("旧納品予約のschemaが不正です")),
    };
    let mut state = State::default();
    for lane in lanes {
        validate(&lane)?;
        let mut scopes = lane
            .suspended
            .into_iter()
            .chain(lane.owner)
            .filter(|scope| scope.repository.eq_ignore_ascii_case(repository))
            .collect::<Vec<_>>();
        if let Some(owner) = scopes.pop() {
            if state.owner.is_some() {
                return Err(error("旧納品予約でrepositoryの所有が重複しています"));
            }
            state.owner = Some(owner);
            state.suspended = scopes;
        }
    }
    Ok(state)
}

fn repository_path(root: &Path, repository: &str) -> Result<PathBuf> {
    Ok(root.join(format!("repository-{}", repo_key(repository)?)))
}

fn repository_state(root: &Path, repository: &str) -> Result<State> {
    let dir = repository_path(root, repository)?;
    if dir.exists() || dir.is_symlink() {
        safe_directory(&dir)?;
        let path = dir.join("state.json");
        if path.exists() || path.is_symlink() {
            let state: State = serde_json::from_value(read_json_file(&path, "納品予約")?)
                .map_err(|_| error("納品予約のschemaが不正です"))?;
            validate(&state)?;
            if state
                .owner
                .iter()
                .chain(&state.suspended)
                .any(|scope| !scope.repository.eq_ignore_ascii_case(repository))
            {
                return Err(error("納品予約のrepositoryが一致しません"));
            }
            return Ok(state);
        }
    }
    // 旧共有ファイルしか残っていない場合も、同じrepositoryの所有を引き継ぐ。
    legacy_repository_state(root, repository)
}

fn save_repository(root: &Path, repository: &str, state: &State) -> Result<()> {
    let dir = repository_path(root, repository)?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(&dir) {
        Ok(()) => (),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(error("納品予約directoryを作成できません")),
    }
    safe_directory(&dir)?;
    atomic_json(
        &dir.join("state.json"),
        &serde_json::to_value(state).map_err(|_| error("納品予約を保存できません"))?,
    )
}

pub(super) fn admit(scope: Scope, repair_parent: Option<&str>) -> Result<()> {
    admit_at(&root()?, scope, repair_parent)
}

fn admit_at(root: &Path, scope: Scope, parent: Option<&str>) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let repository = scope.repository.clone();
    let mut state = repository_state(root, &repository)?;
    claim_state(&mut state, scope, parent)?;
    save_repository(root, &repository, &state)
}

pub(super) fn release(repository: &str, task: &str, pr: u64) -> Result<()> {
    release_at(&root()?, repository, task, pr)
}

fn release_at(root: &Path, repository: &str, task: &str, pr: u64) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut state = repository_state(root, repository)?;
    // 完了済みfinishの再実行は後続PRの所有を変更しない。
    if state
        .owner
        .as_ref()
        .is_some_and(|owner| owner.task == task && owner.pr == pr)
    {
        release_state(&mut state, repository, task, pr)?;
        save_repository(root, repository, &state)?;
    }
    Ok(())
}

pub(super) fn admission_available(
    home: &Path,
    scope: &Scope,
    parent: Option<&str>,
) -> Result<bool> {
    scope.validate()?;
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
    match claim_state(
        &mut repository_state(&root, &scope.repository)?,
        scope.clone(),
        parent,
    ) {
        Ok(()) => Ok(true),
        Err(cause) if cause.to_string() == BUSY => Ok(false),
        Err(cause) => Err(cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, PathBuf) {
        let home = std::env::temp_dir().join(format!(
            "delivery-admission-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(home.join("worktrees"))
            .unwrap();
        let root = root_at(&home).unwrap();
        (home, root)
    }

    #[test]
    fn legacy_reservations_only_affect_their_own_repository() {
        for version in [1, 2] {
            let (home, root) = fixture();
            let owner = scope("task-first", 1);
            let lane = serde_json::json!({"version":1,"owner":owner,"suspended":[]});
            let state = if version == 1 {
                lane
            } else {
                serde_json::json!({"version":2,"reservations":[{"lane":lane,"demand":{"jobs":2}}]})
            };
            let legacy_path = root.join("state.json");
            atomic_json(&legacy_path, &state).unwrap();
            let before = fs::read(&legacy_path).unwrap();
            fs::write(root.join("capacity.json"), b"obsolete capacity fixture").unwrap();

            let mut other = scope("task-other", 2);
            other.repository = "example/other".into();
            assert!(admission_available(&home, &other, None).unwrap());
            admit_at(&root, other.clone(), None).unwrap();
            let mut conflict = scope("task-conflict", 3);
            conflict.repository = "Example/Product".into();
            assert!(!admission_available(&home, &conflict, None).unwrap());
            assert!(admit_at(&root, conflict.clone(), None).is_err());

            release_at(&root, &owner.repository, &owner.task, owner.pr).unwrap();
            assert!(admission_available(&home, &conflict, None).unwrap());
            admit_at(&root, conflict.clone(), None).unwrap();
            release_at(&root, &owner.repository, &owner.task, owner.pr).unwrap();
            assert_eq!(
                repository_state(&root, &conflict.repository).unwrap().owner,
                Some(conflict)
            );
            assert_eq!(
                repository_state(&root, &other.repository).unwrap().owner,
                Some(other)
            );
            assert_eq!(fs::read(&legacy_path).unwrap(), before);
            fs::remove_dir_all(home).unwrap();
        }
    }

    #[test]
    fn legacy_cross_repository_repair_preserves_each_owner() {
        let (home, root) = fixture();
        let parent = scope("task-parent", 1);
        let mut child = scope("task-repair", 2);
        child.repository = "example/infrastructure".into();
        atomic_json(
            &root.join("state.json"),
            &serde_json::json!({"version":1,"owner":child,"suspended":[parent]}),
        )
        .unwrap();
        admit_at(&root, parent.clone(), None).unwrap();
        admit_at(&root, child.clone(), None).unwrap();
        release_at(&root, &child.repository, &child.task, child.pr).unwrap();
        assert_eq!(
            repository_state(&root, &parent.repository).unwrap().owner,
            Some(parent)
        );
        assert!(
            repository_state(&root, &child.repository)
                .unwrap()
                .owner
                .is_none()
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn invalid_repository_state_blocks_only_that_repository() {
        let (home, root) = fixture();
        let owner = scope("task-first", 1);
        admit_at(&root, owner.clone(), None).unwrap();
        let path = repository_path(&root, &owner.repository).unwrap();
        atomic_json(
            &path.join("state.json"),
            &serde_json::json!({"version":99,"owner":null,"suspended":[]}),
        )
        .unwrap();
        assert!(admission_available(&home, &owner, None).is_err());
        assert!(admit_at(&root, owner, None).is_err());
        let mut other = scope("task-other", 2);
        other.repository = "example/other".into();
        assert!(admission_available(&home, &other, None).unwrap());
        admit_at(&root, other, None).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn concurrent_admission_is_persisted_until_the_matching_finish() {
        let (home, root) = fixture();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (1..=8)
            .map(|pr| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    admit_at(&root, scope(&format!("task-{pr}"), pr), None).is_ok()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let owner = repository_state(&root, "example/product")
            .unwrap()
            .owner
            .unwrap();
        let other = scope("task-other", 99);
        assert!(!admission_available(&home, &other, None).unwrap());
        release_at(&root, &other.repository, &other.task, other.pr).unwrap();
        assert!(!admission_available(&home, &other, None).unwrap());
        release_at(&root, &owner.repository, &owner.task, owner.pr).unwrap();
        assert!(admission_available(&home, &other, None).unwrap());
        fs::remove_dir_all(home).unwrap();
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
    fn repository_state_conflict_does_not_disclose_owner() {
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
