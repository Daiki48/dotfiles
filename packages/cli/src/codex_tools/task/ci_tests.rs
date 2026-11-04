//! GitHub APIの境界でlocal gateの選択と拒否を検証する。
use super::*;
use std::collections::HashMap;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";
const OTHER: &str = "dddddddddddddddddddddddddddddddddddddddd";

struct Api(HashMap<String, Value>);

impl Api {
    fn new() -> Self {
        let mut api = Self(HashMap::new());
        api.set(
            "pulls/1",
            json!({
                "head":{"sha":HEAD},"base":{"sha":OTHER,"ref":"main"},
                "merge_commit_sha":MERGE,"merged":false
            }),
        );
        api.set(
            "git/ref/heads/main",
            json!({
                "ref":"refs/heads/main","object":{"type":"commit","sha":BASE}
            }),
        );
        api.set("rules/branches/main", json!([]));
        for sha in [BASE, HEAD, MERGE] {
            api.set(
                &format!("git/commits/{sha}"),
                json!({"sha":sha,"tree":{"sha":sha}}),
            );
            api.set(
                &format!("git/trees/{sha}?recursive=1"),
                json!({
                    "sha":sha,"truncated":false,"tree":[]
                }),
            );
            api.set(
                &format!("commits/{sha}/check-runs?filter=latest&per_page=100"),
                json!([{"total_count":0,"check_runs":[]}]),
            );
            api.set(&format!("commits/{sha}/statuses?per_page=100"), json!([[]]));
        }
        api.set(
            &format!("compare/{MERGE}...{BASE}"),
            json!({"merge_base_commit":{"sha":MERGE}}),
        );
        api
    }

    fn set(&mut self, endpoint: &str, value: Value) {
        self.0
            .insert(format!("repos/example/product/{endpoint}"), value);
    }

    fn fetch(&self, args: &[String]) -> Result<Value, String> {
        self.0
            .get(&args[1])
            .cloned()
            .ok_or_else(|| "API unavailable".into())
    }

    fn observe(&self, main: Option<GateMode>) -> Result<Observation, String> {
        observe_with("example/product", 1, main, &mut |args| self.fetch(args))
    }

    fn workflow(&mut self, sha: &str) {
        self.set(
            &format!("git/trees/{sha}?recursive=1"),
            json!({
                "sha":sha,"truncated":false,
                "tree":[{"path":".github/workflows/ci.yml","type":"blob"}]
            }),
        );
    }

    fn check(&mut self, sha: &str, status: &str, conclusion: Option<&str>) {
        self.set(
            &format!("commits/{sha}/check-runs?filter=latest&per_page=100"),
            json!([{
                "total_count":1,"check_runs":[{
                    "id":1,"name":"ci","head_sha":sha,"status":status,"conclusion":conclusion,
                    "app":{"id":15368,"slug":"github-actions"},"check_suite":{"id":4}
                }]
            }]),
        );
        self.set(
            "actions/runs?check_suite_id=4&per_page=100",
            json!([{
                "total_count":1,"workflow_runs":[{
                    "id":7,"run_attempt":1,"check_suite_id":4,"head_sha":HEAD,
                    "status":status,"conclusion":conclusion
                }]
            }]),
        );
    }
}

#[test]
fn workflow_free_live_base_and_fixed_head_select_local_without_claiming_ci_success() {
    let mut api = Api::new();
    // PR payloadの古いbase SHAにはworkflowがあっても、live branch refを正本にする。
    api.workflow(OTHER);
    let observed = api.observe(None).unwrap();
    assert_eq!(observed.base, BASE);
    assert_eq!(observed.head, HEAD);
    assert_eq!(observed.target, HEAD);
    assert_eq!(observed.state, CiState::LocalValidation);
    assert_eq!(observed.gate_mode, GateMode::LocalValidation);
    assert!(observed.allows_delivery());
    assert!(observed.workflows.unwrap().main.is_none());
}

#[test]
fn workflow_on_either_side_requires_the_existing_remote_gate() {
    for sha in [BASE, HEAD] {
        let mut api = Api::new();
        api.workflow(sha);
        let observed = api.observe(None).unwrap();
        assert_eq!(observed.gate_mode, GateMode::Strict);
        assert_eq!(observed.state, CiState::Missing);
        assert!(!observed.allows_delivery());
        api.check(MERGE, "completed", Some("success"));
        let observed = api.observe(None).unwrap();
        assert_eq!(observed.target, MERGE);
        assert!(observed.allows_delivery());
    }
}

#[test]
fn local_gate_never_hides_actual_head_checks_with_test_merge_success() {
    for (status, conclusion, expected) in [
        ("completed", Some("success"), CiState::LocalValidation),
        ("completed", Some("failure"), CiState::Failure),
        ("in_progress", None, CiState::Pending),
        ("completed", Some("cancelled"), CiState::Cancelled),
        ("completed", Some("timed_out"), CiState::TimedOut),
        ("completed", Some("skipped"), CiState::Skipped),
        ("completed", Some("stale"), CiState::Stale),
    ] {
        let mut api = Api::new();
        api.check(MERGE, "completed", Some("success"));
        api.check(HEAD, status, conclusion);
        let observed = api.observe(None).unwrap();
        assert_eq!(observed.target, HEAD);
        assert_eq!(observed.state, expected);
        assert_eq!(
            observed.allows_delivery(),
            expected == CiState::LocalValidation
        );
    }
    let mut api = Api::new();
    api.set(
        "rules/branches/main",
        json!([{
            "type":"required_status_checks",
            "parameters":{"required_status_checks":[{"context":"required","integration_id":15368}]}
        }]),
    );
    assert_eq!(api.observe(None).unwrap().state, CiState::Missing);
}

#[test]
fn missing_or_malformed_primary_evidence_is_an_error() {
    let baseline = Api::new();
    let mut used = Vec::new();
    observe_with("example/product", 1, None, &mut |args| {
        used.push(args[1].clone());
        baseline.fetch(args)
    })
    .unwrap();
    for endpoint in used {
        let mut api = Api::new();
        api.0.remove(&endpoint);
        assert!(api.observe(None).is_err(), "{endpoint}");
    }
    // paginate/slurpがページを返さなかった応答は、空のcheck/status一覧ではない。
    for endpoint in [
        format!("commits/{HEAD}/check-runs?filter=latest&per_page=100"),
        format!("commits/{HEAD}/statuses?per_page=100"),
    ] {
        let mut api = Api::new();
        api.set(&endpoint, json!([]));
        assert!(api.observe(None).is_err(), "{endpoint}");
    }
    for tree in [
        json!({"sha":HEAD,"truncated":true,"tree":[]}),
        json!({"sha":OTHER,"truncated":false,"tree":[]}),
        json!({"sha":HEAD,"tree":[]}),
        json!({"sha":HEAD,"truncated":false}),
    ] {
        let mut api = Api::new();
        api.set(&format!("git/trees/{HEAD}?recursive=1"), tree);
        assert!(api.observe(None).is_err());
    }
    let mut api = Api::new();
    api.check(HEAD, "completed", Some("success"));
    let checks = api
        .0
        .get_mut(&format!(
            "repos/example/product/commits/{HEAD}/check-runs?filter=latest&per_page=100"
        ))
        .unwrap();
    checks[0]["check_runs"][0]["head_sha"] = json!(OTHER);
    assert!(api.observe(None).is_err());
}

#[test]
fn push_or_live_base_change_during_observation_invalidates_the_snapshot() {
    for endpoint in ["pulls/1", "git/ref/heads/main"] {
        let api = Api::new();
        let mut reads = 0;
        let observed = observe_with("example/product", 1, None, &mut |args| {
            let mut value = api.fetch(args)?;
            if args[1].ends_with(endpoint) {
                reads += 1;
                if reads == 2 {
                    if endpoint == "pulls/1" {
                        value["head"]["sha"] = json!(OTHER);
                    } else {
                        value["object"]["sha"] = json!(OTHER);
                    }
                }
            }
            Ok(value)
        })
        .unwrap();
        assert_eq!(observed.state, CiState::Stale);
        assert!(!observed.allows_delivery());
    }
}

#[test]
fn main_local_gate_requires_actual_merge_ancestry_workflow_and_checks() {
    let mut api = Api::new();
    assert!(api.observe(Some(GateMode::LocalValidation)).is_err());
    api.0.get_mut("repos/example/product/pulls/1").unwrap()["merged"] = json!(true);
    let observed = api.observe(Some(GateMode::LocalValidation)).unwrap();
    assert_eq!(observed.target, MERGE);
    assert!(observed.allows_delivery());
    assert_eq!(observed.workflows.unwrap().main.unwrap().commit, MERGE);
    api.check(MERGE, "completed", Some("success"));
    assert!(
        api.observe(Some(GateMode::LocalValidation))
            .unwrap()
            .allows_delivery()
    );
    api.check(MERGE, "in_progress", None);
    assert!(
        !api.observe(Some(GateMode::LocalValidation))
            .unwrap()
            .allows_delivery()
    );
    api.check(MERGE, "completed", Some("failure"));
    assert_eq!(
        api.observe(Some(GateMode::LocalValidation)).unwrap().state,
        CiState::Failure
    );
    api.workflow(MERGE);
    assert!(
        !api.observe(Some(GateMode::LocalValidation))
            .unwrap()
            .allows_delivery()
    );
    api.set(
        &format!("compare/{MERGE}...{BASE}"),
        json!({"merge_base_commit":{"sha":OTHER}}),
    );
    assert!(api.observe(Some(GateMode::LocalValidation)).is_err());
}

#[test]
fn main_local_gate_rejects_workflows_on_merge_or_current_main_and_unknown_apis() {
    for sha in [BASE, HEAD, MERGE] {
        let mut api = Api::new();
        api.0.get_mut("repos/example/product/pulls/1").unwrap()["merged"] = json!(true);
        api.workflow(sha);
        assert!(
            !api.observe(Some(GateMode::LocalValidation))
                .is_ok_and(|o| o.allows_delivery())
        );
    }
    let mut api = Api::new();
    api.0.get_mut("repos/example/product/pulls/1").unwrap()["merged"] = json!(true);
    let mut used = Vec::new();
    observe_with(
        "example/product",
        1,
        Some(GateMode::LocalValidation),
        &mut |args| {
            used.push(args[1].clone());
            api.fetch(args)
        },
    )
    .unwrap();
    for endpoint in used {
        let mut unavailable = Api(api.0.clone());
        unavailable.0.remove(&endpoint);
        assert!(
            unavailable
                .observe(Some(GateMode::LocalValidation))
                .is_err(),
            "{endpoint}"
        );
    }
}

#[test]
fn local_gate_checks_commit_status_and_actions_run_results_without_fallback() {
    for (status, expected) in [
        ("success", CiState::LocalValidation),
        ("failure", CiState::Failure),
        ("pending", CiState::Pending),
        ("unknown", CiState::Failure),
    ] {
        let mut api = Api::new();
        api.set(
            &format!("commits/{HEAD}/statuses?per_page=100"),
            json!([[{
                "id":1,"context":"external","state":status,"updated_at":"2026-11-04T00:00:00Z"
            }]]),
        );
        assert_eq!(api.observe(None).unwrap().state, expected);
    }
    let mut api = Api::new();
    api.check(HEAD, "completed", Some("success"));
    let endpoint = "repos/example/product/actions/runs?check_suite_id=4&per_page=100";
    api.0.get_mut(endpoint).unwrap()[0]["workflow_runs"][0]["conclusion"] = json!("failure");
    assert!(api.observe(None).is_err());
    api.0.remove(endpoint);
    assert!(api.observe(None).is_err());
}

#[test]
fn main_preserves_the_premerge_receipt_gate() {
    let mut api = Api::new();
    api.0.get_mut("repos/example/product/pulls/1").unwrap()["merged"] = json!(true);
    assert_eq!(
        api.observe(Some(GateMode::Strict)).unwrap().state,
        CiState::Missing
    );
    api.workflow(BASE);
    assert!(api.observe(Some(GateMode::LocalValidation)).is_err());
}
