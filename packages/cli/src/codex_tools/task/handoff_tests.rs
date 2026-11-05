//! CI後handoffで一般的な作業branchを受け入れる。
use super::tests::Fixture;
use super::*;

fn register_on_branch(fixture: &Fixture, id: &str, objective: &str, branch: &str) -> Result<Value> {
    submit_with_policy(
        &fixture.0,
        id.into(),
        objective.into(),
        "許可済みの同一作業範囲".into(),
        Some(branch.into()),
        vec![],
        vec![],
        "high".into(),
        vec![],
        ci::GateMode::Strict,
        recovery::Boundary::Finish,
    )
}

#[test]
fn docs_and_fix_branches_persist_and_remain_idempotent_per_task_id() {
    let fixture = Fixture::new();
    for (id, objective, branch, other_branch) in [
        (
            "task-docs",
            "更新済み文書を引き継ぐ",
            "docs/guide-update",
            "fix/guide-update",
        ),
        (
            "task-fix",
            "managed CI修復を引き継ぐ",
            "fix/managed-ci",
            "docs/managed-ci",
        ),
    ] {
        let created = register_on_branch(&fixture, id, objective, branch).unwrap();
        assert_eq!(created["existing"], false);
        let persisted = fixture.0.load(id).unwrap();
        assert_eq!(persisted.branch, branch);

        let repeated = register_on_branch(&fixture, id, objective, branch).unwrap();
        assert_eq!(repeated["existing"], true);
        let before = serde_json::to_value(fixture.0.load(id).unwrap()).unwrap();

        assert!(register_on_branch(&fixture, id, objective, other_branch).is_err());

        assert_eq!(
            serde_json::to_value(fixture.0.load(id).unwrap()).unwrap(),
            before
        );
    }
}

fn submit_args(id: &str, branch: &str) -> Vec<String> {
    vec![
        "submit".into(),
        "--task-id".into(),
        id.into(),
        "--objective".into(),
        "handoff".into(),
        "--branch".into(),
        branch.into(),
    ]
}

#[test]
fn invocation_and_task_state_share_the_safe_branch_policy() {
    let fixture = Fixture::new();
    fixture.submit("task-safe");
    let valid_task = fixture.0.load("task-safe").unwrap();
    for branch in ["docs/guide-update", "fix/managed-ci"] {
        assert_eq!(invocation_reason(&submit_args("task-safe", branch)), None);
    }

    for branch in [
        "main",
        "master",
        "codex/worker",
        "docs/../escape",
        "fix/new\nline",
    ] {
        assert!(
            invocation_reason(&submit_args("task-safe", branch)).is_some(),
            "{branch:?}"
        );
        let mut invalid_task = valid_task.clone();
        invalid_task.branch = branch.into();
        assert!(invalid_task.validate().is_err(), "{branch:?}");
    }
}
