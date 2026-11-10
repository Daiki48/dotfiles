//! 明示opt-inされたrepositoryだけに適用する、host CIの重み付き容量policy。
use super::*;
use serde::{Deserialize, Serialize};

const CAPACITY_FILE: &str = "capacity.json";
const CAPACITY_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Demand {
    Exclusive,
    Jobs(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    max_jobs: u8,
    disk_path: String,
    cpu_per_job: u32,
    memory_mib_per_job: u64,
    disk_mib_per_job: u64,
    reserve_cpus: u32,
    reserve_memory_mib: u64,
    reserve_disk_mib: u64,
    repositories: Vec<RepositoryOptIn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryOptIn {
    repository: String,
    workflow_tree: String,
    jobs: u8,
}

impl Config {
    fn validate(&self) -> Result<()> {
        if self.version != CAPACITY_VERSION
            || !(1..=3).contains(&self.max_jobs)
            || !(1..=64).contains(&self.cpu_per_job)
            || !(1..=1_048_576).contains(&self.memory_mib_per_job)
            || !(1..=16_777_216).contains(&self.disk_mib_per_job)
            || self.reserve_cpus > 256
            || self.reserve_memory_mib > 1_048_576
            || self.reserve_disk_mib > 16_777_216
            || self.repositories.len() > 256
        {
            return Err(error("CI容量policyのlimitsが不正です"));
        }

        let mut repositories = HashSet::new();
        for entry in &self.repositories {
            if !repository_name(&entry.repository)
                || oid(&entry.workflow_tree, "CI容量workflow tree").is_err()
                || !(1..=self.max_jobs).contains(&entry.jobs)
                || !repositories.insert(entry.repository.to_ascii_lowercase())
            {
                return Err(error("CI容量policyのrepository opt-inが不正です"));
            }
        }
        Ok(())
    }

    fn disk_path(&self) -> Result<PathBuf> {
        let path = Path::new(&self.disk_path);
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(error("CI容量policyのdisk_pathは絶対pathが必要です"));
        }
        let canonical =
            fs::canonicalize(path).map_err(|_| error("CI容量policyのdisk_pathを確認できません"))?;
        let metadata = fs::metadata(&canonical)
            .map_err(|_| error("CI容量policyのdisk_pathを確認できません"))?;
        if canonical.as_path() != path || !metadata.is_dir() {
            return Err(error(
                "CI容量policyのdisk_pathはcanonical directoryが必要です",
            ));
        }
        Ok(canonical)
    }
}

/// 計測に失敗したときはeffectiveをNoneにし、weighted demandを許可しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Policy {
    config: Option<Config>,
    effective_jobs: Option<u8>,
}

impl Policy {
    pub(super) fn load(root: &Path) -> Result<Self> {
        let root_metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::unconfigured());
            }
            Err(_) => return Err(error("CI容量policy directoryを確認できません")),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(error("CI容量policy directoryが不正です"));
        }
        safe_directory(root)?;

        let path = root.join(CAPACITY_FILE);
        match fs::symlink_metadata(&path) {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::unconfigured());
            }
            Err(_) => return Err(error("CI容量policy fileを確認できません")),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error("CI容量policy fileが不正です"));
            }
            Ok(_) => {}
        }

        let config: Config = serde_json::from_value(read_json_file(&path, "CI容量policy")?)
            .map_err(|_| error("CI容量policyのschemaが不正です"))?;
        config.validate()?;
        let disk = config.disk_path()?;
        let effective_jobs = super::super::task::resources::ResourceSnapshot::measure(&disk)
            .ok()
            .map(|snapshot| {
                effective_capacity(
                    snapshot.cpus,
                    snapshot.memory_mib,
                    snapshot.disk_mib,
                    read_loadavg_1m(),
                    &config,
                )
            });
        Ok(Self {
            config: Some(config),
            effective_jobs,
        })
    }

    fn unconfigured() -> Self {
        Self {
            config: None,
            effective_jobs: None,
        }
    }

    /// policy上限を返す。有効容量はallows()とstatus()で別途確認する。
    /// opt-inがない場合は既存の単一排他枠を表す1を返す。
    pub(super) fn max_jobs(&self) -> u8 {
        self.config.as_ref().map_or(1, |config| config.max_jobs)
    }

    /// 同じ予約規則かを確認する。resource snapshotは短時間でも変動するため比較しない。
    pub(super) fn same_config(&self, other: &Self) -> bool {
        self.config == other.config
    }

    pub(super) fn demand(&self, checkout: Option<&Path>, scope: &lane::Scope) -> Demand {
        let (Some(config), Some(checkout)) = (&self.config, checkout) else {
            return Demand::Exclusive;
        };
        if !repository_name(&scope.repository) {
            return Demand::Exclusive;
        }
        let Some(entry) = config
            .repositories
            .iter()
            .find(|entry| entry.repository.eq_ignore_ascii_case(&scope.repository))
        else {
            return Demand::Exclusive;
        };
        let expected_tree = match oid(&entry.workflow_tree, "CI容量workflow tree") {
            Ok(tree) => tree,
            Err(_) => return Demand::Exclusive,
        };
        if [&scope.base, &scope.head].into_iter().any(|revision| {
            workflow_tree(checkout, revision)
                .is_none_or(|tree| !tree.eq_ignore_ascii_case(&expected_tree))
        }) {
            return Demand::Exclusive;
        }
        Demand::Jobs(entry.jobs)
    }

    /// `jobs`は、候補を含む全weighted reservationの合計。
    pub(super) fn allows(&self, jobs: u8) -> bool {
        jobs > 0
            && self.config.is_some()
            && self.effective_jobs.is_some_and(|capacity| jobs <= capacity)
    }

    /// repository名、workflow tree、disk pathを含めない読み取り専用の集約値。
    pub(super) fn status(&self) -> Value {
        object([
            ("configured", Value::Bool(self.config.is_some())),
            ("supported_max_jobs", Value::from(3)),
            ("max_jobs", Value::from(self.max_jobs())),
            (
                "effective_max_jobs",
                self.effective_jobs.map(Value::from).unwrap_or(Value::Null),
            ),
        ])
    }
}

fn workflow_tree(checkout: &Path, revision: &str) -> Option<String> {
    let revision = oid(revision, "CI容量workflow revision").ok()?;
    let spec = format!("{revision}:.github/workflows");
    let tree = git(checkout, &["rev-parse", "--verify", &spec], true)
        .ok()?
        .trim()
        .to_owned();
    let tree = oid(&tree, "CI容量workflow tree").ok()?;
    let kind = git(checkout, &["cat-file", "-t", &tree], true).ok()?;
    (kind.trim() == "tree").then_some(tree)
}

fn read_loadavg_1m() -> Option<f64> {
    let contents = fs::read_to_string("/proc/loadavg").ok()?;
    parse_loadavg_1m(&contents)
}

fn parse_loadavg_1m(contents: &str) -> Option<f64> {
    let load = contents.split_whitespace().next()?.parse::<f64>().ok()?;
    (load.is_finite() && load >= 0.0).then_some(load)
}

fn cpu_headroom(cpus: usize, loadavg_1m: Option<f64>) -> usize {
    let Some(load) = loadavg_1m.filter(|load| load.is_finite() && *load >= 0.0) else {
        return 0;
    };
    let load_cpus = load.ceil().min(cpus as f64) as usize;
    cpus.saturating_sub(load_cpus)
}

fn effective_capacity(
    cpus: usize,
    memory_mib: u64,
    disk_mib: u64,
    loadavg_1m: Option<f64>,
    config: &Config,
) -> u8 {
    let cpu_jobs = (cpu_headroom(cpus, loadavg_1m) as u64)
        .saturating_sub(u64::from(config.reserve_cpus))
        / u64::from(config.cpu_per_job);
    let memory_jobs =
        memory_mib.saturating_sub(config.reserve_memory_mib) / config.memory_mib_per_job;
    let disk_jobs = disk_mib.saturating_sub(config.reserve_disk_mib) / config.disk_mib_per_job;
    u64::from(config.max_jobs)
        .min(cpu_jobs)
        .min(memory_jobs)
        .min(disk_jobs) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_value() -> Value {
        serde_json::json!({
            "version": 1,
            "max_jobs": 3,
            "disk_path": "/synthetic/runner",
            "cpu_per_job": 2,
            "memory_mib_per_job": 1024,
            "disk_mib_per_job": 2048,
            "reserve_cpus": 1,
            "reserve_memory_mib": 512,
            "reserve_disk_mib": 1024,
            "repositories": [{
                "repository": "example/project",
                "workflow_tree": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "jobs": 2
            }]
        })
    }

    fn git_fixture() -> (PathBuf, String, String, String) {
        let root = std::env::temp_dir().join(format!(
            "ci-capacity-fixture-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let run = |args: &[&str]| {
            let output = Command::new(GIT_BINARY)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(args)
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        run(&["init", "-q", "-b", "main"]);
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/ci.yml"), "jobs: {}\n").unwrap();
        run(&["add", "--", ".github/workflows/ci.yml"]);
        run(&["commit", "--allow-empty", "-qm", "synthetic fixture"]);
        let base = run(&["rev-parse", "HEAD"]);
        let base_tree = run(&["rev-parse", "HEAD:.github/workflows"]);
        fs::write(
            root.join(".github/workflows/ci.yml"),
            "jobs: {check: {runs-on: synthetic}}\n",
        )
        .unwrap();
        run(&["add", "--", ".github/workflows/ci.yml"]);
        run(&["commit", "-qm", "synthetic workflow change"]);
        let changed = run(&["rev-parse", "HEAD"]);
        (root, base, base_tree, changed)
    }

    #[test]
    fn policy_schema_is_strict_and_repository_names_are_case_insensitively_unique() {
        let valid: Config = serde_json::from_value(config_value()).unwrap();
        assert!(valid.validate().is_ok());

        let mut unknown = config_value();
        unknown["unrecognized"] = Value::Bool(true);
        assert!(serde_json::from_value::<Config>(unknown).is_err());

        let mut duplicate = config_value();
        duplicate["repositories"] = serde_json::json!([
            {"repository":"example/project","workflow_tree":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","jobs":1},
            {"repository":"Example/Project","workflow_tree":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","jobs":1}
        ]);
        assert!(
            serde_json::from_value::<Config>(duplicate)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn policy_limits_reject_unsupported_capacity_and_bad_workflow_ids() {
        for (field, value) in [("max_jobs", 0), ("max_jobs", 4), ("cpu_per_job", 0)] {
            let mut invalid = config_value();
            invalid[field] = Value::from(value);
            assert!(
                serde_json::from_value::<Config>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut invalid = config_value();
        invalid["repositories"][0]["workflow_tree"] = Value::String("not-an-oid".into());
        assert!(
            serde_json::from_value::<Config>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut invalid = config_value();
        invalid["repositories"][0]["jobs"] = Value::from(4);
        assert!(
            serde_json::from_value::<Config>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn effective_capacity_uses_the_tightest_synthetic_resource_headroom() {
        let config: Config = serde_json::from_value(config_value()).unwrap();
        assert_eq!(effective_capacity(8, 8_192, 16_384, Some(0.0), &config), 3);
        assert_eq!(effective_capacity(4, 8_192, 16_384, Some(0.0), &config), 1);
        assert_eq!(effective_capacity(8, 2_560, 16_384, Some(0.0), &config), 2);
        assert_eq!(effective_capacity(8, 8_192, 5_120, Some(0.0), &config), 2);
        assert_eq!(effective_capacity(1, 8_192, 16_384, Some(0.0), &config), 0);
        assert_eq!(effective_capacity(8, 8_192, 16_384, Some(1.01), &config), 2);
        assert_eq!(effective_capacity(8, 8_192, 16_384, None, &config), 0);
    }

    #[test]
    fn loadavg_parser_requires_a_finite_nonnegative_one_minute_value() {
        assert_eq!(parse_loadavg_1m("1.01 0.50 0.25 1/42 123"), Some(1.01));
        assert_eq!(parse_loadavg_1m("0 0 0 1/42 123"), Some(0.0));
        assert_eq!(parse_loadavg_1m("-0.1 0 0 1/42 123"), None);
        assert_eq!(parse_loadavg_1m("NaN 0 0 1/42 123"), None);
        assert_eq!(parse_loadavg_1m("infinity 0 0 1/42 123"), None);
        assert_eq!(parse_loadavg_1m(""), None);
        assert_eq!(cpu_headroom(4, Some(1.01)), 2);
        assert_eq!(cpu_headroom(4, None), 0);
    }

    #[test]
    fn unavailable_capacity_never_allows_weighted_reservations() {
        let policy = Policy::unconfigured();
        assert!(!policy.allows(1));
        assert!(!policy.allows(0));
        assert_eq!(policy.max_jobs(), 1);
        assert_eq!(policy.status()["configured"], Value::Bool(false));
        assert_eq!(policy.status()["effective_max_jobs"], Value::Null);
    }

    #[test]
    fn demand_requires_exact_base_and_head_workflow_trees_even_without_measurements() {
        let (checkout, base, tree, changed) = git_fixture();
        let mut config: Config = serde_json::from_value(config_value()).unwrap();
        config.repositories[0].workflow_tree = tree;
        let policy = Policy {
            config: Some(config),
            // demand is a tree/config decision; allows() separately enforces live capacity.
            effective_jobs: None,
        };
        let mut scope = lane::Scope {
            repository: "EXAMPLE/PROJECT".into(),
            task: "task-fixture".into(),
            pr: 1,
            head: base.clone(),
            base: base.clone(),
            labels: vec![],
        };
        assert_eq!(policy.demand(Some(&checkout), &scope), Demand::Jobs(2));
        assert!(!policy.allows(2));
        scope.head = changed.clone();
        assert_eq!(policy.demand(Some(&checkout), &scope), Demand::Exclusive);
        scope.head = base;
        scope.base = changed;
        assert_eq!(policy.demand(Some(&checkout), &scope), Demand::Exclusive);
        assert_eq!(policy.demand(None, &scope), Demand::Exclusive);
        fs::remove_dir_all(checkout).unwrap();
    }
}
