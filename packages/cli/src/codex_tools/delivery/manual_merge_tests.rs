use super::*;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REPOSITORY: &str = "example/product";
const TASK: &str = "task-fixture";
const PR: i64 = 12;
const PLAN: &str = "fixture-plan-v1";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn approval_value() -> Value {
    json!({
        "version": 1,
        "approved": true,
        "repository": REPOSITORY,
        "task_id": TASK,
        "pr": PR,
        "plan_id": PLAN,
        "plan_version": 1,
        "head_sha": HEAD,
        "base_sha": BASE,
        "merge_sha": MERGE,
    })
}

fn receipt_value() -> Value {
    json!({
        "version": RECEIPT_VERSION,
        "kind": "review",
        "task_id": TASK,
        "repository": REPOSITORY,
        "pr": PR,
        "head_sha": HEAD,
        "risk": "medium",
        "plan_id": PLAN,
        "plan_version": 1,
        "actionable": 0,
        "decision": "autonomous",
        "gate_mode": STRICT_GATE_MODE,
        "tests_passed": true,
        "independent_review_passed": false,
        "specialist_review_passed": false,
        "changed_files": [],
        "created_at": "synthetic fixture",
    })
}

fn parse_approval(value: Value) -> Approval {
    serde_json::from_value(value).expect("synthetic approval should deserialize")
}

fn empty_pages() -> Value {
    json!([{"total_count": 0, "workflow_runs": []}])
}

fn fetch_empty(endpoint: &str) -> Result<Value> {
    if endpoint.contains("actions/runs?") {
        Ok(empty_pages())
    } else if endpoint.contains("check-runs?") {
        Ok(json!([{"total_count": 0, "check_runs": []}]))
    } else if endpoint.contains("statuses?") {
        Ok(json!([[]]))
    } else {
        Err(error("unexpected synthetic endpoint"))
    }
}

#[test]
fn approval_ci_observation_preserves_complete_empty_results() {
    let mut calls = Vec::new();
    let observation = observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
        calls.push(endpoint.to_string());
        fetch_empty(endpoint)
    })
    .unwrap();

    assert_eq!(observation.workflow_runs, 0);
    assert_eq!(observation.actions_check_runs, 0);
    assert_eq!(observation.other_check_runs, 0);
    assert_eq!(observation.commit_statuses, 0);
    assert_eq!(calls.len(), 3);
    assert!(
        calls
            .iter()
            .any(|endpoint| endpoint.contains("actions/runs?"))
    );
    assert!(
        calls
            .iter()
            .any(|endpoint| endpoint.contains("check-runs?"))
    );
    assert!(calls.iter().any(|endpoint| endpoint.contains("statuses?")));
}

#[test]
fn approval_ci_observation_counts_completed_successful_sources() {
    let observation = observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
        if endpoint.contains("actions/runs?") {
            Ok(json!([{
                "total_count": 1,
                "workflow_runs": [{
                    "head_sha": HEAD,
                    "status": "completed",
                    "conclusion": "success"
                }]
            }]))
        } else if endpoint.contains("check-runs?") {
            Ok(json!([{
                "total_count": 2,
                "check_runs": [
                    {
                        "head_sha": HEAD,
                        "status": "completed",
                        "conclusion": "skipped",
                        "app": {"id": 15368},
                        "completed_at": "synthetic timestamp"
                    },
                    {
                        "head_sha": HEAD,
                        "status": "completed",
                        "conclusion": "neutral",
                        "app": {"id": 42},
                        "completed_at": "synthetic timestamp"
                    }
                ]
            }]))
        } else {
            Ok(json!([[{"state": "success"}]]))
        }
    })
    .unwrap();

    assert_eq!(observation.workflow_runs, 1);
    assert_eq!(observation.actions_check_runs, 1);
    assert_eq!(observation.other_check_runs, 1);
    assert_eq!(observation.commit_statuses, 1);
}

#[test]
fn approval_ci_observation_rejects_api_and_pagination_failures() {
    let mut calls = 0;
    assert!(
        observe_ci(REPOSITORY, HEAD, &mut |_| {
            calls += 1;
            Err(error("synthetic API failure"))
        })
        .is_err()
    );
    assert_eq!(calls, 1);

    for malformed in [
        json!([]),
        json!([{"total_count": 1, "workflow_runs": []}]),
        json!([{"total_count": 0, "workflow_runs": []}, {"total_count": 1, "workflow_runs": []}]),
        json!([{"total_count": 1, "workflow_runs": []}, {"total_count": 2, "workflow_runs": []}]),
    ] {
        assert!(
            observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
                if endpoint.contains("actions/runs?") {
                    Ok(malformed.clone())
                } else {
                    fetch_empty(endpoint)
                }
            })
            .is_err()
        );
    }
}

#[test]
fn approval_ci_observation_rejects_bad_sha_state_provider_and_status() {
    let invalid_workflows = [
        json!({"head_sha": BASE, "status": "completed", "conclusion": "success"}),
        json!({"head_sha": HEAD, "status": "in_progress", "conclusion": "success"}),
        json!({"head_sha": HEAD, "status": "completed", "conclusion": "failure"}),
        json!({"head_sha": HEAD, "status": "completed", "conclusion": "pending"}),
    ];
    for run in invalid_workflows {
        assert!(
            observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
                if endpoint.contains("actions/runs?") {
                    Ok(json!([{"total_count": 1, "workflow_runs": [run.clone()]}]))
                } else {
                    fetch_empty(endpoint)
                }
            })
            .is_err()
        );
    }

    let invalid_checks = [
        json!({"head_sha": BASE, "status": "completed", "conclusion": "success", "app": {"id": 15368}, "completed_at": "time"}),
        json!({"head_sha": HEAD, "status": "queued", "conclusion": "success", "app": {"id": 15368}, "completed_at": "time"}),
        json!({"head_sha": HEAD, "status": "completed", "conclusion": "failure", "app": {"id": 15368}, "completed_at": "time"}),
        json!({"head_sha": HEAD, "status": "completed", "conclusion": "success", "app": {"id": 0}, "completed_at": "time"}),
        json!({"head_sha": HEAD, "status": "completed", "conclusion": "success", "app": {"id": 42}, "completed_at": ""}),
    ];
    for check in invalid_checks {
        assert!(
            observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
                if endpoint.contains("check-runs?") {
                    Ok(json!([{"total_count": 1, "check_runs": [check.clone()]}]))
                } else {
                    fetch_empty(endpoint)
                }
            })
            .is_err()
        );
    }

    assert!(
        observe_ci(REPOSITORY, HEAD, &mut |endpoint| {
            if endpoint.contains("statuses?") {
                Ok(json!([[{"state": "failure"}]]))
            } else {
                fetch_empty(endpoint)
            }
        })
        .is_err()
    );
}

#[test]
fn approval_validation_binds_every_cli_target_to_the_receipt() {
    let receipt = receipt_value();
    assert!(parse_approval(approval_value()).validate(&receipt).is_ok());

    for (field, replacement) in [
        ("repository", json!("example/other")),
        ("task_id", json!("task-other")),
        ("pr", json!(PR + 1)),
        ("plan_id", json!("other-plan-v1")),
        ("plan_version", json!(2)),
        ("head_sha", json!(BASE)),
    ] {
        let mut changed_approval = approval_value();
        changed_approval[field] = replacement;
        assert!(
            parse_approval(changed_approval).validate(&receipt).is_err(),
            "{field}"
        );
    }

    for (field, replacement) in [
        ("repository", json!("example/other")),
        ("task_id", json!("task-other")),
        ("pr", json!(PR + 1)),
        ("plan_id", json!("other-plan-v1")),
        ("plan_version", json!(2)),
        ("head_sha", json!(BASE)),
    ] {
        let mut changed_receipt = receipt.clone();
        changed_receipt[field] = replacement;
        assert!(
            parse_approval(approval_value())
                .validate(&changed_receipt)
                .is_err(),
            "{field}"
        );
    }

    for field in ["approved", "base_sha", "merge_sha"] {
        let mut changed_approval = approval_value();
        changed_approval[field] = match field {
            "approved" => json!(false),
            "base_sha" => json!(HEAD),
            _ => json!("not-a-commit"),
        };
        assert!(
            parse_approval(changed_approval).validate(&receipt).is_err(),
            "{field}"
        );
    }
}

fn recovery_value(receipt: &Value) -> Recovery {
    Recovery {
        approval: parse_approval(approval_value()),
        receipt_sha256: emergency::digest(receipt).unwrap(),
        observed_at: "synthetic timestamp".into(),
        head_ci: CiObservation {
            workflow_runs: 0,
            actions_check_runs: 0,
            other_check_runs: 0,
            commit_statuses: 0,
        },
        merge_ci: CiObservation {
            workflow_runs: 0,
            actions_check_runs: 0,
            other_check_runs: 0,
            commit_statuses: 0,
        },
    }
}

#[test]
fn recovery_state_rejects_receipt_and_target_mutation() {
    let receipt = receipt_value();
    let recovery = recovery_value(&receipt);
    let mut state = json!({
        "stage": "main_synced",
        "manual_merge": serde_json::to_value(&recovery).unwrap(),
    });
    assert!(Recovery::from_state(&state, &receipt).is_ok());

    let mut changed_receipt = receipt.clone();
    changed_receipt["tests_passed"] = json!(false);
    assert!(Recovery::from_state(&state, &changed_receipt).is_err());

    for (field, replacement) in [
        ("repository", json!("example/other")),
        ("task_id", json!("task-other")),
        ("pr", json!(PR + 1)),
        ("plan_id", json!("other-plan-v1")),
        ("plan_version", json!(2)),
        ("head_sha", json!(BASE)),
    ] {
        let mut changed_state = state.clone();
        changed_state["manual_merge"]["approval"][field] = replacement;
        assert!(
            Recovery::from_state(&changed_state, &receipt).is_err(),
            "{field}"
        );
    }

    state["stage"] = json!("merged");
    assert!(Recovery::from_state(&state, &receipt).is_err());
}

struct GitFixture(PathBuf);

impl Drop for GitFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_fixture() -> GitFixture {
    let path = std::env::temp_dir().join(format!(
        "codex-manual-merge-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    let output = Command::new(GIT_BINARY)
        .args(["init", "-b", "main"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    git(&path, &["config", "user.name", "Synthetic Fixture"], true).unwrap();
    git(
        &path,
        &["config", "user.email", "synthetic@example.test"],
        true,
    )
    .unwrap();
    fs::write(path.join("file.txt"), "base\n").unwrap();
    git(&path, &["add", "file.txt"], true).unwrap();
    git(&path, &["commit", "-m", "synthetic base"], true).unwrap();
    fs::write(path.join("file.txt"), "head\n").unwrap();
    git(&path, &["commit", "-am", "synthetic head"], true).unwrap();
    GitFixture(path)
}

fn commit_tree(root: &Path, tree: &str, first_parent: &str, second_parent: &str) -> String {
    git(
        root,
        &[
            "commit-tree",
            tree,
            "-p",
            first_parent,
            "-p",
            second_parent,
            "-m",
            "synthetic [skip ci] merge",
        ],
        true,
    )
    .unwrap()
    .trim()
    .to_string()
}

#[test]
fn approval_merge_validation_requires_ordered_parents_and_head_tree() {
    let fixture = git_fixture();
    let root = &fixture.0;
    let head = git(root, &["rev-parse", "HEAD"], true)
        .unwrap()
        .trim()
        .to_string();
    let base = git(root, &["rev-parse", "HEAD^"], true)
        .unwrap()
        .trim()
        .to_string();
    let tree = git(root, &["rev-parse", "HEAD^{tree}"], true)
        .unwrap()
        .trim()
        .to_string();
    let base_tree = git(root, &["rev-parse", "HEAD^^{tree}"], true)
        .unwrap()
        .trim()
        .to_string();

    let valid_merge = commit_tree(root, &tree, &base, &head);
    let mut value = approval_value();
    value["base_sha"] = json!(base);
    value["head_sha"] = json!(head);
    value["merge_sha"] = json!(valid_merge);
    assert!(parse_approval(value.clone()).verify_merge(root).is_ok());

    let reversed = commit_tree(root, &tree, &head, &base);
    value["merge_sha"] = json!(reversed);
    assert!(parse_approval(value.clone()).verify_merge(root).is_err());

    let wrong_tree = commit_tree(root, &base_tree, &base, &head);
    value["merge_sha"] = json!(wrong_tree);
    assert!(parse_approval(value).verify_merge(root).is_err());
}
