use super::*;
use serde_json::json;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WORKFLOW: &str = ".github/workflows/ci.yml";

fn scope() -> Value {
    json!({"repository":"example/product","task_id":"task-example","pr":7,"head_sha":HEAD,"plan_id":"example-plan-v1","plan_version":1})
}

pub(crate) fn sample_request() -> Value {
    json!({
        "version":1,"repository":"example/product","task_id":"task-example","pr":7,
        "head_sha":HEAD,"base_sha":BASE,"plan_id":"example-plan-v1","plan_version":1,
        "reason":"CI service outage","authorization":"CI基盤障害の判断を継続委任する作業規約",
        "local_validation":[{"kind":"test","command":"cargo test --locked","result":"passed"},{"kind":"build","command":"cargo build --locked","result":"passed"}],
        "independent_review":"固定headを独立review済み。actionable=0",
        "known_test_failures":false,"acknowledge_workflow_runs":true,
        "public_summary":"互換fieldの非公開検証メモ。公開本文は定型文だけ。",
        "waivers":[
            {"kind":"check-run","name":"unit","source":"app:42","states":["failure","queued","in_progress"],"cause":"admission-failure","evidence":"jobが開始前に拒否されたことを確認"},
            {"kind":"workflow","name":WORKFLOW,"source":"actions:pull_request","states":["failure","queued","in_progress","missing"],"cause":"service-outage","evidence":"基盤障害を確認"},
            {"kind":"workflow","name":WORKFLOW,"source":"actions:push","states":["failure","queued","in_progress","missing"],"cause":"service-outage","evidence":"同じ基盤のmain実行にも適用"},
            {"kind":"workflow","name":WORKFLOW,"source":"actions","states":["missing"],"cause":"runner-unavailable","evidence":"実行が作成されない状態を確認"}
        ]
    })
}

fn snapshot(state: &str) -> Snapshot {
    snapshot_with("example/product", HEAD, &[expected(WORKFLOW, "pull_request")], &mut |endpoint| {
        if endpoint.contains("check-runs?") {
            Ok(json!([{"total_count":1,"check_runs":[{"id":1,"head_sha":HEAD,"name":"unit","app":{"id":42},"status":if matches!(state,"queued"|"in_progress") {state} else {"completed"},"conclusion":if matches!(state,"queued"|"in_progress") {Value::Null} else {json!(state)}}]}]))
        } else if endpoint.contains("/statuses?") { Ok(json!([[]])) }
        else {
            Ok(json!([{"total_count":1,"workflow_runs":[{"id":10,"head_sha":HEAD,"path":WORKFLOW,"event":"pull_request","status":if matches!(state,"queued"|"in_progress") {state} else {"completed"},"conclusion":if matches!(state,"queued"|"in_progress") {Value::Null} else {json!(state)}}]}]))
        }
    }).unwrap()
}

#[test]
fn workflow_admission_failure_and_pending_are_observed_then_explicitly_waived() {
    let request = request(&sample_request(), &scope()).unwrap();
    for state in ["failure", "queued", "in_progress", "success"] {
        let snapshot = snapshot(state);
        assert_eq!(snapshot.entries[0].state, state);
        assert!(coverage(&request, &snapshot, HEAD).is_ok());
        let mut unapproved = request.clone();
        unapproved.waivers.clear();
        assert_eq!(
            coverage(&unapproved, &snapshot, HEAD).is_ok(),
            state == "success"
        );
    }
}

#[test]
fn missing_workflow_run_needs_a_separate_explicit_waiver() {
    let observed = snapshot_with(
        "example/product",
        HEAD,
        &[
            expected(WORKFLOW, "pull_request"),
            expected(WORKFLOW, "push"),
        ],
        &mut |endpoint| {
            Ok(if endpoint.contains("check-runs?") {
                json!([{"total_count":0,"check_runs":[]}])
            } else if endpoint.contains("actions/runs?") {
                json!([{"total_count":0,"workflow_runs":[]}])
            } else {
                json!([[]])
            })
        },
    )
    .unwrap();
    assert_eq!(observed.entries[0].state, "missing");
    let mut approval = request(&sample_request(), &scope()).unwrap();
    assert!(coverage(&approval, &observed, HEAD).is_ok());
    approval
        .waivers
        .retain(|waiver| waiver.source != "actions:pull_request");
    assert!(coverage(&approval, &observed, HEAD).is_err());
}

#[test]
fn unknown_check_provider_state_and_target_never_inherit_a_waiver() {
    let approval = request(&sample_request(), &scope()).unwrap();
    for change in ["name", "source", "state", "sha"] {
        let mut observed = snapshot("failure");
        match change {
            "name" => observed.entries[0].name = "integration".into(),
            "source" => observed.entries[0].source = "app:99".into(),
            "state" => observed.entries[0].state = "action_required".into(),
            _ => observed.sha = BASE.into(),
        }
        assert!(coverage(&approval, &observed, HEAD).is_err(), "{change}");
    }
}

#[test]
fn known_test_failures_and_missing_authorization_or_local_evidence_are_rejected() {
    for field in [
        "known_test_failures",
        "authorization",
        "reason",
        "independent_review",
        "local_validation",
        "acknowledge_workflow_runs",
        "waivers",
        "test-cause",
    ] {
        let mut input = sample_request();
        match field {
            "known_test_failures" => input[field] = json!(true),
            "acknowledge_workflow_runs" => input[field] = json!(false),
            "local_validation" => input[field][0]["result"] = json!("failed"),
            "waivers" => input[field] = json!([]),
            "test-cause" => input["waivers"][0]["cause"] = json!("test-failure"),
            _ => input[field] = json!(""),
        }
        assert!(request(&input, &scope()).is_err(), "{field}");
    }
}

#[test]
fn approval_is_bound_to_repository_task_pr_head_and_plan() {
    for field in [
        "repository",
        "task_id",
        "pr",
        "head_sha",
        "plan_id",
        "plan_version",
    ] {
        let mut other = scope();
        other[field] = if matches!(field, "pr" | "plan_version") {
            json!(8)
        } else {
            json!("different")
        };
        assert!(request(&sample_request(), &other).is_err(), "{field}");
    }
    let mut input = sample_request();
    input["base_sha"] = json!("unknown");
    assert!(request(&input, &scope()).is_err());
}

#[test]
fn api_failure_incomplete_pagination_and_wrong_sha_remain_errors() {
    let mut calls = 0;
    assert!(
        snapshot_with("example/product", HEAD, &[], &mut |_| {
            calls += 1;
            Err(error("API unavailable"))
        })
        .is_err()
    );
    assert_eq!(calls, 1);
    assert!(
        pages(
            &json!([{"total_count":2,"check_runs":[]}]),
            Some("check_runs")
        )
        .is_err()
    );
    assert!(
        snapshot_with("example/product", HEAD, &[], &mut |_| Ok(
            json!([{"total_count":1,"check_runs":[{"id":1,"head_sha":BASE}]}])
        ))
        .is_err()
    );
}

#[test]
fn unstable_is_ci_only_and_never_allows_blocked_unknown_or_conflicts() {
    assert!(merge_status("UNSTABLE", false, false, true));
    assert!(!merge_status("UNSTABLE", false, false, false));
    for state in ["BLOCKED", "UNKNOWN", "DIRTY", "BEHIND", "HAS_HOOKS"] {
        assert!(!merge_status(state, false, false, true));
    }
    assert!(!merge_status("DRAFT", true, false, true));
}

#[test]
fn public_note_excludes_private_approval_and_raw_ci() {
    let mut receipt = scope();
    let private = "synthetic-private-marker /private-fixture/runtime";
    let mut request = sample_request();
    for key in [
        "reason",
        "authorization",
        "independent_review",
        "public_summary",
    ] {
        request[key] = json!(private);
    }
    request["local_validation"][0]["command"] = json!(private);
    request["waivers"][0]["name"] = json!(private);
    request["waivers"][0]["evidence"] = json!(private);
    receipt["task_id"] = json!("task-private-fixture");
    request["task_id"] = receipt["task_id"].clone();
    receipt["emergency"] = json!({"request":request});
    let note = note(&receipt).unwrap();
    assert!(note.contains("local test/build"));
    assert!(!note.contains("synthetic-private-marker"));
    assert!(!note.contains("/private-fixture/runtime"));
    assert!(!note.contains("task-private-fixture"));
    assert!(!note.contains("authorization"));
    assert!(!note.contains("jobが開始前"));
    assert!(!note.contains("app:42"));
}

#[test]
fn current_emergency_receipt_accepts_autonomous_delegation_with_full_evidence() {
    let mut value = super::super::tests::receipt_v6("high", false);
    for (key, entry) in scope().as_object().unwrap() {
        value[key] = entry.clone();
    }
    value["version"] = json!(VERSION);
    value["gate_mode"] = json!(GATE);
    value["decision"] = json!("autonomous");
    value["emergency"] = json!({"request":sample_request(), "observed_ci":snapshot("failure")});
    assert!(
        super::super::receipt(
            &value,
            Path::new("/unused"),
            "task-example",
            HEAD,
            Some("example/product")
        )
        .is_ok()
    );
}

#[test]
fn only_exact_merge_parents_and_tested_tree_are_eligible() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let path = env::temp_dir().join(format!(
        "codex-emergency-merge-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    let fixture = Fixture(path);
    let root = &fixture.0;
    assert!(
        Command::new(GIT_BINARY)
            .args(["init", "-b", "main"])
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success()
    );
    git(root, &["config", "user.name", "Example"], true).unwrap();
    git(
        root,
        &["config", "user.email", "example@example.test"],
        true,
    )
    .unwrap();
    fs::write(root.join("file"), "base").unwrap();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(root.join(WORKFLOW), "on: [push, pull_request]\n").unwrap();
    git(root, &["add", "--", "file", WORKFLOW], true).unwrap();
    git(root, &["commit", "-m", "base"], true).unwrap();
    let base = git(root, &["rev-parse", "HEAD"], true)
        .unwrap()
        .trim()
        .to_string();
    fs::write(root.join("file"), "head").unwrap();
    fs::remove_file(root.join(WORKFLOW)).unwrap();
    git(root, &["add", "--", "file", WORKFLOW], true).unwrap();
    git(root, &["commit", "-m", "head"], true).unwrap();
    let head = git(root, &["rev-parse", "HEAD"], true)
        .unwrap()
        .trim()
        .to_string();
    let tree = git(root, &["rev-parse", "HEAD^{tree}"], true)
        .unwrap()
        .trim()
        .to_string();
    assert!(workflow_paths(root, &head).unwrap().is_empty());
    let expected = workflow_expectations(root, &base, &head, false).unwrap();
    assert!(expected.contains(&super::expected(WORKFLOW, "pull_request")));
    assert!(expected.contains(&super::expected(WORKFLOW, "push")));
    let mut approval = request(&sample_request(), &scope()).unwrap();
    approval.base_sha = base.clone();
    approval.head_sha = head.clone();
    let merge = git(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            &base,
            "-p",
            &head,
            "-m",
            "merge",
        ],
        true,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(verify_merge(root, &approval, &merge).is_ok());
    let reversed = git(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            &head,
            "-p",
            &base,
            "-m",
            "wrong parents",
        ],
        true,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(verify_merge(root, &approval, &reversed).is_err());
    let wrong_tree = git(root, &["rev-parse", &format!("{base}^{{tree}}")], true)
        .unwrap()
        .trim()
        .to_string();
    let wrong = git(
        root,
        &[
            "commit-tree",
            &wrong_tree,
            "-p",
            &base,
            "-p",
            &head,
            "-m",
            "wrong tree",
        ],
        true,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(verify_merge(root, &approval, &wrong).is_err());
}

#[test]
fn successful_push_does_not_hide_a_missing_pull_request_event() {
    let observed = snapshot_with("example/product", HEAD,
        &[expected(WORKFLOW, "push"), expected(WORKFLOW, "pull_request")],
        &mut |endpoint| Ok(if endpoint.contains("check-runs?") {
            json!([{"total_count":0,"check_runs":[]}])
        } else if endpoint.contains("/statuses?") { json!([[]]) }
        else { json!([{"total_count":1,"workflow_runs":[{"id":20,"head_sha":HEAD,"path":WORKFLOW,"event":"push","status":"completed","conclusion":"success"}]}]) })
    ).unwrap();
    assert!(
        observed
            .entries
            .iter()
            .any(|entry| entry.source == "actions:pull_request" && entry.state == "missing")
    );
    let mut approval = request(&sample_request(), &scope()).unwrap();
    assert!(coverage(&approval, &observed, HEAD).is_ok());
    approval
        .waivers
        .retain(|waiver| waiver.source != "actions:pull_request");
    assert!(coverage(&approval, &observed, HEAD).is_err());
}
