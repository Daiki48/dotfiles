//! 外部通信だけをfakeにし、正規receipt保存とCI認可を実Gitで通すfixture。
use super::*;
use serde_json::json;
use std::cell::RefCell;

thread_local! {
    static API: RefCell<Option<Api>> = const { RefCell::new(None) };
}

struct Api {
    head: String,
    base: String,
    receipt: PathBuf,
    labels: usize,
    calls: usize,
    unresolved: bool,
    unavailable: bool,
    workflows: bool,
    truncated: bool,
}

/// 他moduleの環境変数変更と混ざらない専用test processで実処理を検証する。
pub(crate) fn isolated_process() -> bool {
    let thread = std::thread::current();
    let name = thread.name().expect("named test thread");
    if env::var("CODEX_AUTHORIZATION_FIXTURE_TEST").as_deref() == Ok(name) {
        return true;
    }
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("CODEX_AUTHORIZATION_FIXTURE_TEST", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

pub(super) fn active() -> bool {
    API.with(|api| api.borrow().is_some())
}

pub(super) fn github(args: &[String]) -> Option<Result<Value>> {
    API.with(|api| {
        let mut api = api.borrow_mut();
        let api = api.as_mut()?;
        api.calls += 1;
        Some((|| {
            if api.unavailable {
                return Err(error("synthetic API failure"));
            }
            if args.first().map(String::as_str) == Some("pr")
                && args.get(1).map(String::as_str) == Some("view")
                && args.get(2).map(String::as_str) == Some("7")
            {
                return Ok(json!({
                    "number":7,"state":"OPEN","isDraft":true,
                    "headRefOid":api.head,"headRefName":"feat/fixture","baseRefName":"main",
                    "mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
                    "mergedAt":null,"mergeCommit":null,
                    "headRepository":{"nameWithOwner":"owner/repo"},"headRepositoryOwner":{"login":"owner"},
                    "isCrossRepository":false,"autoMergeRequest":null
                }));
            }
            match args.get(1).map(String::as_str) {
                Some("repos/owner/repo") => Ok(json!({"default_branch":"main"})),
                Some(endpoint)
                    if endpoint
                        == format!(
                            "repos/owner/repo/commits/{}/check-runs?filter=latest&per_page=100",
                            api.head
                        ) =>
                {
                    Ok(json!([{"total_count":0,"check_runs":[]}]))
                }
                Some("repos/owner/repo/pulls/7") => Ok(json!({
                    "number":7,"state":"open",
                    "merged":false,"merge_commit_sha":null,
                    "head":{"sha":api.head,"repo":{"full_name":"owner/repo"}},
                    "base":{"sha":api.base,"ref":"main","repo":{"full_name":"owner/repo"}}
                })),
                Some("repos/owner/repo/git/ref/heads/main") => Ok(json!({
                    "ref":"refs/heads/main","object":{"type":"commit","sha":api.base}
                })),
                Some(endpoint) if endpoint.starts_with("repos/owner/repo/git/commits/") => {
                    let sha = endpoint.rsplit('/').next().unwrap();
                    Ok(json!({"sha":sha,"tree":{"sha":sha}}))
                }
                Some(endpoint) if endpoint.starts_with("repos/owner/repo/git/trees/") => {
                    let sha = endpoint
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .split('?')
                        .next()
                        .unwrap();
                    let tree = if api.workflows {
                        json!([{"path":".github/workflows/ci.yml"}])
                    } else {
                        json!([])
                    };
                    Ok(json!({"sha":sha,"truncated":api.truncated,"tree":tree}))
                }
                Some(endpoint) if endpoint.starts_with("repos/owner/repo/labels/") => {
                    Ok(json!({"name":"ci-approved"}))
                }
                Some("repos/owner/repo/pulls?state=open&per_page=100") => {
                    Ok(json!([[{"number":7,"labels":[]}]]))
                }
                Some("repos/owner/repo/issues/7/labels") => {
                    assert!(args.iter().any(|arg| arg == "POST"));
                    let receipt = read_json_file(&api.receipt, "fixture receipt")?;
                    assert_eq!(receipt["head_sha"], api.head);
                    assert_eq!(receipt["independent_review_passed"], true);
                    assert_eq!(receipt["tests_passed"], true);
                    api.labels += 1;
                    Ok(json!([]))
                }
                Some("graphql") => {
                    let threads = args
                        .iter()
                        .any(|arg| arg == &format!("query={THREAD_QUERY}"));
                    let key = if threads { "reviewThreads" } else { "reviews" };
                    let nodes = if threads && api.unresolved {
                        json!([{"isResolved":false}])
                    } else {
                        json!([])
                    };
                    Ok(json!({"data":{"repository":{"pullRequest":{
                        "reviewDecision":null,
                        key:{"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}
                    }}}}))
                }
                _ => Err(error("unexpected synthetic GitHub request")),
            }
        })())
    })
}

pub(crate) struct Fixture {
    pub root: PathBuf,
    pub worktree: PathBuf,
    pub home: PathBuf,
    pub head: String,
    pub base: String,
    pub receipt: PathBuf,
}

impl Fixture {
    pub fn git(&self, root: &Path, args: &[&str]) -> String {
        let output = Command::new(GIT_BINARY)
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git: {:?}", args);
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    pub fn receipt(&self) -> Value {
        read_json_file(&self.receipt, "fixture receipt").unwrap()
    }

    pub fn record(&self, plan: &str, approved: bool) {
        write_review(
            &self.root,
            "task-fixture",
            7,
            &self.head,
            "high",
            plan,
            3,
            approved,
            true,
            true,
            false,
            STRICT_GATE_MODE,
        )
        .unwrap();
    }

    pub fn labels(&self) -> usize {
        API.with(|api| api.borrow().as_ref().unwrap().labels)
    }

    pub fn calls(&self) -> usize {
        API.with(|api| api.borrow().as_ref().unwrap().calls)
    }

    pub fn unresolved(&self) {
        API.with(|api| api.borrow_mut().as_mut().unwrap().unresolved = true);
    }

    pub fn unavailable(&self) {
        API.with(|api| api.borrow_mut().as_mut().unwrap().unavailable = true);
    }

    pub fn stale_base(&self) {
        API.with(|api| api.borrow_mut().as_mut().unwrap().base = "b".repeat(40));
    }

    pub fn without_workflows(&self) {
        API.with(|api| api.borrow_mut().as_mut().unwrap().workflows = false);
    }

    pub fn incomplete_tree(&self) {
        API.with(|api| api.borrow_mut().as_mut().unwrap().truncated = true);
    }

    pub fn corrupt_receipt(&self) {
        atomic_json(&self.receipt, &json!({"version":999})).unwrap();
    }
}

pub(crate) fn with_fixture<T>(test: impl FnOnce(&Fixture) -> T) -> T {
    super::tests::with_codex_home(|home| {
        let managed = managed_root("owner/repo").unwrap();
        let mut fixture = Fixture {
            root: home.join("parent"),
            worktree: managed.join("task-fixture"),
            home: home.into(),
            head: String::new(),
            base: String::new(),
            receipt: PathBuf::new(),
        };
        fs::create_dir(&fixture.root).unwrap();
        fixture.git(&fixture.root, &["init", "--initial-branch=main"]);
        fixture.git(&fixture.root, &["config", "user.name", "Synthetic Tester"]);
        fixture.git(
            &fixture.root,
            &["config", "user.email", "tester@example.invalid"],
        );
        fs::write(fixture.root.join("source.txt"), "base\n").unwrap();
        fixture.git(&fixture.root, &["add", "source.txt"]);
        fixture.git(&fixture.root, &["commit", "-m", "synthetic base"]);
        fixture.base = fixture.git(&fixture.root, &["rev-parse", "HEAD"]);
        fixture.git(
            &fixture.root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        fixture.git(
            &fixture.root,
            &["update-ref", "refs/remotes/origin/main", &fixture.base],
        );
        fixture.git(
            &fixture.root,
            &[
                "worktree",
                "add",
                "-b",
                "feat/fixture",
                fixture.worktree.to_str().unwrap(),
            ],
        );
        fs::write(fixture.worktree.join("source.txt"), "head\n").unwrap();
        fixture.git(&fixture.worktree, &["add", "source.txt"]);
        fixture.git(&fixture.worktree, &["commit", "-m", "synthetic change"]);
        fixture.head = fixture.git(&fixture.worktree, &["rev-parse", "HEAD"]);
        let common = fixture.root.join(".git");
        atomic_json(
            &snapshot_path("owner/repo", "task-fixture").unwrap(),
            &json!({
                "version":MANIFEST_VERSION,"status":"ready","task_id":"task-fixture",
                "repository":fixture.root,"common_git_dir":common,"github_name":"owner/repo",
                "branch":"feat/fixture","base":"origin/main","base_oid":fixture.base,
                "worktree":fixture.worktree,"created_at":"synthetic fixture","detail":""
            }),
        )
        .unwrap();
        fixture.receipt = receipt_path("owner/repo", "task-fixture", &fixture.head).unwrap();
        API.with(|api| {
            *api.borrow_mut() = Some(Api {
                head: fixture.head.clone(),
                base: fixture.base.clone(),
                receipt: fixture.receipt.clone(),
                labels: 0,
                calls: 0,
                unresolved: false,
                unavailable: false,
                workflows: true,
                truncated: false,
            })
        });
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                API.with(|api| *api.borrow_mut() = None);
            }
        }
        let _reset = Reset;
        test(&fixture)
    })
}
