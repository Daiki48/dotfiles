//! CI共有予約とrepository単位の納品予約。時刻やprocess終了で所有を解放しない。
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
        return Err(error("CI直列枠のschemaが不正です"));
    }
    let mut identities = HashSet::new();
    for scope in state.owner.iter().chain(state.suspended.iter()) {
        scope.validate()?;
        if !identities.insert((scope.repository.to_ascii_lowercase(), &scope.task, scope.pr)) {
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
    admit(scope, repair_parent, true)
}

// 旧state.jsonはCI予約として維持し、同じlockでrepository予約と照合する。
fn repository_root(root: &Path, repository: &str) -> Result<PathBuf> {
    let path = root.join(format!("repository-{}", repo_key(repository)?));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(&path) {
        Ok(()) => (),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(error("納品予約directoryを作成できません")),
    }
    safe_directory(&path)?;
    Ok(path)
}

fn check_admission(state: &State, scope: &Scope, parent: Option<&str>, ci: bool) -> Result<()> {
    validate(state)?;
    scope.validate()?;
    // 旧CI予約も同一repositoryの排他として扱う。修復中の親は復帰を待つ。
    if state
        .suspended
        .iter()
        .any(|owner| owner.repository.eq_ignore_ascii_case(&scope.repository))
        && !state
            .owner
            .as_ref()
            .is_some_and(|owner| owner.same_pr(scope))
    {
        return Err(error(BUSY));
    }
    if ci
        || state
            .owner
            .as_ref()
            .is_some_and(|owner| owner.repository.eq_ignore_ascii_case(&scope.repository))
    {
        claim_state(&mut state.clone(), scope.clone(), parent)?;
    }
    Ok(())
}

pub(super) fn admit(scope: Scope, repair_parent: Option<&str>, ci: bool) -> Result<()> {
    admit_at(&root()?, scope, repair_parent, ci)
}

fn admit_at(root: &Path, scope: Scope, parent: Option<&str>, ci: bool) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut shared = read(root)?;
    check_admission(&shared, &scope, parent, ci)?;
    let path = repository_root(root, &scope.repository)?;
    let mut delivery = read(&path)?;
    claim_state(&mut delivery, scope.clone(), parent)?;
    // 中途失敗も予約を保持して再開する。CI不要時は共有ownerを書き換えない。
    atomic_json(
        &path.join("state.json"),
        &serde_json::to_value(delivery).map_err(|_| error("納品予約を保存できません"))?,
    )?;
    if ci {
        claim_state(&mut shared, scope, parent)?;
        atomic_json(
            &root.join("state.json"),
            &serde_json::to_value(shared).map_err(|_| error("CI予約を保存できません"))?,
        )?;
    }
    Ok(())
}

fn repair_state(state: &mut State, parent: &Scope, child: Scope) -> Result<()> {
    validate(state)?;
    child.validate()?;
    if state.owner.as_ref() == Some(&child) && state.suspended.last() == Some(parent) {
        return Ok(());
    }
    if state.owner.as_ref() != Some(parent)
        || !state.suspended.is_empty()
        || parent.repository.eq_ignore_ascii_case(&child.repository)
        || parent.same_pr(&child)
    {
        return Err(error("修復枠の親が変化したか、別の修復が進行中です"));
    }
    state.suspended.push(parent.clone());
    state.owner = Some(child);
    validate(state)
}

/// APIとreceipt検証を終えた固定scopeだけをCASで交換する。親を解放しない。
pub(super) fn claim_repair(parent: &Scope, child: Scope) -> Result<()> {
    let root = root()?;
    update(&root, |state| {
        repair_state(state, parent, child.clone())?;
        let path = repository_root(&root, &child.repository)?;
        let mut delivery = read(&path)?;
        claim_state(&mut delivery, child, None)?;
        atomic_json(
            &path.join("state.json"),
            &serde_json::to_value(delivery).map_err(|_| error("修復納品予約を保存できません"))?,
        )
    })
}

pub(super) fn release(repository: &str, task: &str, pr: u64) -> Result<()> {
    release_at(&root()?, repository, task, pr)
}

fn release_at(root: &Path, repository: &str, task: &str, pr: u64) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut state = read(root)?;
    let path = repository_root(root, repository)?;
    let mut delivery = read(&path)?;
    if delivery.owner.as_ref().is_some_and(|owner| {
        owner.repository.eq_ignore_ascii_case(repository) && owner.task == task && owner.pr == pr
    }) {
        release_state(&mut delivery, repository, task, pr)?;
        atomic_json(
            &path.join("state.json"),
            &serde_json::to_value(delivery).map_err(|_| error("納品予約を保存できません"))?,
        )?;
    }
    // completed済みfinishの再実行で、後続PRの所有を変更しない。
    if state.owner.as_ref().is_some_and(|owner| {
        owner.repository.eq_ignore_ascii_case(repository) && owner.task == task && owner.pr == pr
    }) {
        release_state(&mut state, repository, task, pr)?;
        atomic_json(
            &root.join("state.json"),
            &serde_json::to_value(state).map_err(|_| error("CI予約を保存できません"))?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
fn available(
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

pub(super) fn admission_available(
    home: &Path,
    scope: &Scope,
    parent: Option<&str>,
    ci: bool,
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
    if let Err(cause) = check_admission(&read(&root)?, scope, parent, ci) {
        return if cause.to_string() == BUSY {
            Ok(false)
        } else {
            Err(cause)
        };
    }
    let path = root.join(format!("repository-{}", repo_key(&scope.repository)?));
    if !path.exists() && !path.is_symlink() {
        return Ok(true);
    }
    safe_directory(&path)?;
    match claim_state(&mut read(&path)?, scope.clone(), parent) {
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
    fn ci_free_delivery_in_another_repository_does_not_touch_the_ci_owner() {
        let (home, root) = fixture();
        let first = scope("task-first", 1);
        admit_at(&root, first.clone(), None, true).unwrap();
        let before = fs::read(root.join("state.json")).unwrap();
        let mut other = scope("task-second", 2);
        other.repository = "example/another-product".into();
        assert!(!admission_available(&home, &other, None, true).unwrap());
        assert!(admit_at(&root, other.clone(), None, true).is_err());
        assert!(admission_available(&home, &other, None, false).unwrap());
        admit_at(&root, other.clone(), None, false).unwrap();
        admit_at(&root, other.clone(), None, false).unwrap(); // crash後の再開
        let mut conflict = other.clone();
        conflict.task = "task-conflict".into();
        conflict.pr = 3;
        assert!(!admission_available(&home, &conflict, None, false).unwrap());
        assert!(admit_at(&root, conflict.clone(), None, false).is_err());
        release_at(&root, &other.repository, &other.task, other.pr).unwrap();
        admit_at(&root, conflict.clone(), None, false).unwrap();
        release_at(&root, &other.repository, &other.task, other.pr).unwrap(); // completed再実行
        assert_eq!(
            read(&repository_root(&root, &other.repository).unwrap())
                .unwrap()
                .owner,
            Some(conflict)
        );
        assert_eq!(fs::read(root.join("state.json")).unwrap(), before);
        assert_eq!(read(&root).unwrap().owner, Some(first));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn gate_changes_keep_existing_ci_responsibility_and_legacy_owners_block_same_repository() {
        let (home, root) = fixture();
        let first = scope("task-first", 1);
        update(&root, |state| claim_state(state, first.clone(), None)).unwrap(); // 旧stateのみ
        let mut conflict = scope("task-second", 2);
        conflict.repository = "Example/Product".into();
        assert!(admit_at(&root, conflict.clone(), None, false).is_err());
        admit_at(&root, first.clone(), None, false).unwrap();
        assert_eq!(read(&root).unwrap().owner, Some(first.clone()));
        release_at(&root, &first.repository, &first.task, first.pr).unwrap();
        admit_at(&root, conflict.clone(), None, false).unwrap();
        conflict.head = "c".repeat(40);
        admit_at(&root, conflict.clone(), None, true).unwrap();
        assert_eq!(read(&root).unwrap().owner, Some(conflict));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repair_parent_cannot_bypass_a_suspended_ci_reservation() {
        let (home, root) = fixture();
        let parent = scope("task-parent", 1);
        admit_at(&root, parent.clone(), None, true).unwrap();
        let mut child = scope("task-repair", 2);
        child.repository = "example/infrastructure".into();
        update(&root, |state| repair_state(state, &parent, child.clone())).unwrap();
        assert!(!admission_available(&home, &parent, None, false).unwrap());
        assert!(admit_at(&root, parent.clone(), None, false).is_err());
        admit_at(&root, child.clone(), None, true).unwrap();
        release_at(&root, &child.repository, &child.task, child.pr).unwrap();
        assert_eq!(read(&root).unwrap().owner, Some(parent.clone()));
        admit_at(&root, parent, None, false).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn unknown_ci_or_repository_schema_never_allows_admission() {
        let (home, root) = fixture();
        let owner = scope("task-first", 1);
        atomic_json(
            &root.join("state.json"),
            &serde_json::json!({"version":99,"owner":null,"suspended":[]}),
        )
        .unwrap();
        assert!(admit_at(&root, owner.clone(), None, false).is_err());
        assert!(admission_available(&home, &owner, None, false).is_err());
        atomic_json(
            &root.join("state.json"),
            &serde_json::to_value(State::default()).unwrap(),
        )
        .unwrap();
        let path = repository_root(&root, &owner.repository).unwrap();
        atomic_json(
            &path.join("state.json"),
            &serde_json::json!({"version":99,"owner":null,"suspended":[]}),
        )
        .unwrap();
        assert!(admit_at(&root, owner.clone(), None, false).is_err());
        assert!(admission_available(&home, &owner, None, false).is_err());
        fs::remove_dir_all(home).unwrap();
    }

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

    #[test]
    fn cross_repository_repair_compares_fixed_parent_and_restores_it() {
        let mut state = State::default();
        let parent = scope("task-parent", 1);
        claim_state(&mut state, parent.clone(), None).unwrap();
        let mut child = scope("task-repair", 2);
        child.repository = "example/infrastructure".into();
        let mut stale = parent.clone();
        stale.head = "d".repeat(40);
        assert!(repair_state(&mut state, &stale, child.clone()).is_err());
        repair_state(&mut state, &parent, child.clone()).unwrap();
        repair_state(&mut state, &parent, child.clone()).unwrap();
        let mut nested = scope("task-nested", 3);
        nested.repository = "example/tools".into();
        assert!(repair_state(&mut state, &child, nested).is_err());
        assert!(release_state(&mut state, &parent.repository, &parent.task, parent.pr).is_err());
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        release_state(&mut state, &child.repository, &child.task, child.pr).unwrap();
        assert_eq!(state.owner, Some(parent));
    }
}
