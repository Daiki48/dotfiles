//! CI共有予約とrepository単位の納品予約。時刻やprocess終了で所有を解放しない。
use super::ci_capacity::{Demand, Policy};
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

pub(super) fn capacity_status() -> Result<Value> {
    let worktrees = codex_home()?.join("worktrees");
    if worktrees.exists() || worktrees.is_symlink() {
        safe_directory(&worktrees)?;
    }
    Ok(Policy::load(&worktrees.join(".delivery-lane"))?.status())
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

#[derive(Clone, Copy)]
pub(super) struct CiRequest<'a> {
    pub checkout: Option<&'a Path>,
    pub finishing: bool,
}

struct Admission {
    policy: Option<Policy>,
    demand: Demand,
    finishing: bool,
}

fn prepare(
    root: &Path,
    scope: &Scope,
    request: Option<CiRequest<'_>>,
) -> Result<Option<Admission>> {
    request
        .map(|request| {
            let policy = if request.finishing {
                None
            } else {
                Some(Policy::load(root)?)
            };
            let demand = policy.as_ref().map_or(Demand::Exclusive, |policy| {
                policy.demand(request.checkout, scope)
            });
            Ok(Admission {
                policy,
                demand,
                finishing: request.finishing,
            })
        })
        .transpose()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    lane: State,
    demand: Demand,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shared {
    version: u32,
    reservations: Vec<Reservation>,
}

impl Shared {
    fn read(root: &Path) -> Result<Self> {
        let path = root.join("state.json");
        if !path.exists() && !path.is_symlink() {
            return Ok(Self {
                version: 2,
                reservations: vec![],
            });
        }
        let json = read_json_file(&path, "共有CI予約")?;
        let shared = if json["version"] == 1 {
            let legacy: State =
                serde_json::from_value(json).map_err(|_| error("旧共有CI予約が不正です"))?;
            validate(&legacy)?;
            Self {
                version: 2,
                reservations: legacy
                    .owner
                    .is_some()
                    .then_some(Reservation {
                        lane: legacy,
                        demand: Demand::Exclusive,
                    })
                    .into_iter()
                    .collect(),
            }
        } else {
            serde_json::from_value(json).map_err(|_| error("共有CI予約が不正です"))?
        };
        shared.validate()?;
        Ok(shared)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 2 || self.reservations.len() > 3 {
            return Err(error("共有CI予約のschemaが不正です"));
        }
        let mut repos = HashSet::new();
        let mut jobs = 0;
        for reservation in &self.reservations {
            validate(&reservation.lane)?;
            let owner = reservation
                .lane
                .owner
                .as_ref()
                .ok_or_else(|| error("共有CI予約の所有者がありません"))?;
            let family: HashSet<_> = std::iter::once(owner)
                .chain(&reservation.lane.suspended)
                .map(|scope| scope.repository.to_ascii_lowercase())
                .collect();
            if family.into_iter().any(|repo| !repos.insert(repo)) {
                return Err(error("repositoryの共有CI予約が重複しています"));
            }
            match reservation.demand {
                Demand::Exclusive if self.reservations.len() == 1 => {}
                Demand::Jobs(weight @ 1..=3) => jobs += weight,
                _ => return Err(error("共有CI予約のjob数が不正です")),
            }
        }
        if jobs > 3 {
            return Err(error("共有CI予約のjob数が上限を超えています"));
        }
        Ok(())
    }

    fn save(&self, root: &Path) -> Result<()> {
        self.validate()?;
        // 排他だけで運用するホストは旧形式のまま読み書きできる。
        let json = match self.reservations.as_slice() {
            [] => serde_json::to_value(State::default()),
            [
                Reservation {
                    lane,
                    demand: Demand::Exclusive,
                },
            ] => serde_json::to_value(lane),
            _ => serde_json::to_value(self),
        }
        .map_err(|_| error("共有CI予約を保存できません"))?;
        atomic_json(&root.join("state.json"), &json)
    }

    fn repository_index(&self, repository: &str) -> Option<usize> {
        self.reservations.iter().position(|entry| {
            entry
                .lane
                .owner
                .iter()
                .chain(&entry.lane.suspended)
                .any(|scope| scope.repository.eq_ignore_ascii_case(repository))
        })
    }

    fn admit(
        &mut self,
        scope: &Scope,
        parent: Option<&str>,
        request: Option<&Admission>,
    ) -> Result<()> {
        let index = self.repository_index(&scope.repository);
        let existing = index.map(|i| &self.reservations[i]);
        let lane = existing.map(|entry| entry.lane.clone()).unwrap_or_default();
        check_repository(&lane, scope, parent)?;
        let Some(request) = request else {
            return Ok(());
        };
        let own = lane
            .owner
            .as_ref()
            .is_some_and(|owner| owner.same_pr(scope));
        if request.finishing && own {
            return Ok(());
        }
        let demand = if existing.is_some() && !own {
            // 修復元の実行中CIが資源を解放したとは扱わない。
            Demand::Exclusive
        } else {
            request.demand
        };
        self.reserve_with(
            index,
            demand,
            |jobs| {
                request
                    .policy
                    .as_ref()
                    .is_some_and(|policy| policy.allows(jobs))
            },
            |state| claim_state(state, scope.clone(), parent),
        )
    }

    fn reserve_with(
        &mut self,
        index: Option<usize>,
        demand: Demand,
        allows: impl FnOnce(u8) -> bool,
        claim: impl FnOnce(&mut State) -> Result<()>,
    ) -> Result<()> {
        let existing = index.map(|i| &self.reservations[i]);
        let mut lane = existing.map(|entry| entry.lane.clone()).unwrap_or_default();
        claim(&mut lane)?;
        let demand = match (existing.map(|entry| entry.demand), demand) {
            (Some(Demand::Exclusive), _) | (_, Demand::Exclusive) => Demand::Exclusive,
            (Some(Demand::Jobs(previous)), Demand::Jobs(requested)) => {
                Demand::Jobs(previous.max(requested))
            }
            (None, demand) => demand,
        };
        let others = self
            .reservations
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != index)
            .map(|(_, entry)| entry.demand);
        if !match demand {
            Demand::Exclusive => others.count() == 0,
            Demand::Jobs(weight) => {
                let jobs = others.fold(weight, |total, demand| match demand {
                    Demand::Jobs(other) => total.saturating_add(other),
                    Demand::Exclusive => u8::MAX,
                });
                (1..=3).contains(&weight) && jobs <= 3 && allows(jobs)
            }
        } {
            return Err(error(BUSY));
        }
        let reservation = Reservation { lane, demand };
        if let Some(i) = index {
            self.reservations[i] = reservation;
        } else {
            self.reservations.push(reservation);
        }
        Ok(())
    }
}

#[cfg(test)]
fn update(root: &Path, change: impl FnOnce(&mut State) -> Result<()>) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut state = read(root)?;
    change(&mut state)?;
    validate(&state)?;
    atomic_json(
        &root.join("state.json"),
        &serde_json::to_value(state).unwrap(),
    )
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

fn check_repository(state: &State, scope: &Scope, parent: Option<&str>) -> Result<()> {
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
    if state
        .owner
        .as_ref()
        .is_some_and(|owner| owner.repository.eq_ignore_ascii_case(&scope.repository))
    {
        claim_state(&mut state.clone(), scope.clone(), parent)?;
    }
    Ok(())
}

pub(super) fn admit(
    scope: Scope,
    repair_parent: Option<&str>,
    ci: Option<CiRequest<'_>>,
) -> Result<()> {
    admit_at(&root()?, scope, repair_parent, ci)
}

fn admit_at(
    root: &Path,
    scope: Scope,
    parent: Option<&str>,
    ci: Option<CiRequest<'_>>,
) -> Result<()> {
    // 固定workflowの確認はlockの外で行う。受付直前に設定と空き資源を再照合する。
    let mut request = prepare(root, &scope, ci)?;
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    if let Some(policy) = request.as_mut().and_then(|request| request.policy.as_mut()) {
        let fresh = Policy::load(root)?;
        if !policy.same_config(&fresh) {
            return Err(error(BUSY));
        }
        *policy = fresh;
    }
    let mut shared = Shared::read(root)?;
    shared.admit(&scope, parent, request.as_ref())?;
    let path = repository_root(root, &scope.repository)?;
    let mut delivery = read(&path)?;
    claim_state(&mut delivery, scope.clone(), parent)?;
    // 中途失敗も予約を保持して再開する。CI不要時は共有ownerを書き換えない。
    atomic_json(
        &path.join("state.json"),
        &serde_json::to_value(delivery).map_err(|_| error("納品予約を保存できません"))?,
    )?;
    if ci.is_some() {
        shared.save(root)?;
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
    claim_repair_at(&root()?, parent, child)
}

fn claim_repair_at(root: &Path, parent: &Scope, child: Scope) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut shared = Shared::read(root)?;
    let index = shared
        .repository_index(&parent.repository)
        .ok_or_else(|| error("修復枠の親が変化しました"))?;
    if shared
        .repository_index(&child.repository)
        .is_some_and(|other| other != index)
    {
        return Err(error(BUSY));
    }
    // 親のCIが停止したと推測せず、別の共有予約が残る間は移譲しない。
    shared.reserve_with(
        Some(index),
        Demand::Exclusive,
        |_| false,
        |state| repair_state(state, parent, child.clone()),
    )?;
    let path = repository_root(root, &child.repository)?;
    let mut delivery = read(&path)?;
    claim_state(&mut delivery, child, None)?;
    atomic_json(
        &path.join("state.json"),
        &serde_json::to_value(delivery).map_err(|_| error("修復納品予約を保存できません"))?,
    )?;
    shared.save(root)
}

pub(super) fn release(repository: &str, task: &str, pr: u64) -> Result<()> {
    release_at(&root()?, repository, task, pr)
}

fn release_at(root: &Path, repository: &str, task: &str, pr: u64) -> Result<()> {
    let _lock =
        super::super::task::store::Lock::acquire_short(&root.join("state.lock")).map_err(error)?;
    let mut shared = Shared::read(root)?;
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
    if let Some(index) = shared.reservations.iter().position(|entry| {
        entry.lane.owner.as_ref().is_some_and(|owner| {
            owner.repository.eq_ignore_ascii_case(repository)
                && owner.task == task
                && owner.pr == pr
        })
    }) {
        let lane = &mut shared.reservations[index].lane;
        release_state(lane, repository, task, pr)?;
        if lane.owner.is_none() {
            shared.reservations.remove(index);
        }
        shared.save(root)?;
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
    ci: Option<CiRequest<'_>>,
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
    let request = prepare(&root, scope, ci)?;
    if let Err(cause) = Shared::read(&root)?.admit(scope, parent, request.as_ref()) {
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

    fn legacy_request(ci: bool) -> Option<CiRequest<'static>> {
        ci.then_some(CiRequest {
            checkout: None,
            finishing: false,
        })
    }

    fn admit_at(root: &Path, scope: Scope, parent: Option<&str>, ci: bool) -> Result<()> {
        super::admit_at(root, scope, parent, legacy_request(ci))
    }

    fn admission_available(
        home: &Path,
        scope: &Scope,
        parent: Option<&str>,
        ci: bool,
    ) -> Result<bool> {
        super::admission_available(home, scope, parent, legacy_request(ci))
    }

    #[test]
    fn weighted_reservations_survive_restarts_and_release_only_the_matching_owner() {
        let (home, root) = fixture();
        let first = scope("task-first", 1);
        let mut second = scope("task-second", 2);
        second.repository = "example/another-product".into();
        let mut third = scope("task-third", 3);
        third.repository = "example/third-product".into();
        let reserve = |shared: &mut Shared, scope: &Scope, weight, capacity| {
            shared.reserve_with(
                shared.repository_index(&scope.repository),
                Demand::Jobs(weight),
                |total| total <= capacity,
                |lane| claim_state(lane, scope.clone(), None),
            )
        };
        let mut shared = Shared::read(&root).unwrap();
        reserve(&mut shared, &first, 2, 3).unwrap();
        assert!(reserve(&mut shared, &second, 2, 3).is_err());
        assert!(reserve(&mut shared, &second, 1, 2).is_err());
        reserve(&mut shared, &second, 1, 3).unwrap();
        reserve(&mut shared, &first, 1, 3).unwrap();
        assert!(reserve(&mut shared, &third, 1, 3).is_err());
        shared.save(&root).unwrap();
        let mut loaded = Shared::read(&root).unwrap();
        assert_eq!(loaded.reservations[0].demand, Demand::Jobs(2));
        assert_eq!(loaded.reservations[1].demand, Demand::Jobs(1));
        let finishing = Admission {
            policy: None,
            demand: Demand::Exclusive,
            finishing: true,
        };
        loaded.admit(&first, None, Some(&finishing)).unwrap();
        assert_eq!(loaded.reservations[0].demand, Demand::Jobs(2));
        release_at(&root, &first.repository, &first.task, 99).unwrap();
        assert_eq!(Shared::read(&root).unwrap().reservations.len(), 2);
        release_at(&root, &first.repository, &first.task, first.pr).unwrap();
        let mut loaded = Shared::read(&root).unwrap();
        assert_eq!(loaded.reservations.len(), 1);
        assert_eq!(loaded.reservations[0].lane.owner.as_ref(), Some(&second));
        reserve(&mut loaded, &third, 2, 3).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn weighted_repair_waits_for_other_reservations_then_preserves_the_parent() {
        let (home, root) = fixture();
        let parent = scope("task-parent", 1);
        let mut other = scope("task-other", 2);
        other.repository = "example/second-product".into();
        let mut child = scope("task-child", 3);
        child.repository = "example/infrastructure".into();
        let mut shared = Shared::read(&root).unwrap();
        for (owner, weight) in [(&parent, 2), (&other, 1)] {
            shared
                .reserve_with(
                    None,
                    Demand::Jobs(weight),
                    |total| total <= 3,
                    |lane| claim_state(lane, owner.clone(), None),
                )
                .unwrap();
        }
        shared.save(&root).unwrap();
        assert!(claim_repair_at(&root, &parent, child.clone()).is_err());
        assert_eq!(Shared::read(&root).unwrap().reservations.len(), 2);
        release_at(&root, &other.repository, &other.task, other.pr).unwrap();
        claim_repair_at(&root, &parent, child.clone()).unwrap();
        claim_repair_at(&root, &parent, child.clone()).unwrap();
        assert!(!admission_available(&home, &parent, None, false).unwrap());
        release_at(&root, &child.repository, &child.task, child.pr).unwrap();
        let restored = Shared::read(&root).unwrap();
        assert_eq!(restored.reservations[0].lane.owner.as_ref(), Some(&parent));
        assert_eq!(restored.reservations[0].demand, Demand::Exclusive);
        fs::remove_dir_all(home).unwrap();
    }

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
                    admit_at(&path, scope(&format!("task-{pr}"), pr), None, true).is_ok()
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
