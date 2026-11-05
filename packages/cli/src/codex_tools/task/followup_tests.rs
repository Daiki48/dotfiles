//! Issue #84: 明示的な後続task再利用と兄弟task区別の回帰検証。
use super::*;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(Context);

impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "task-followup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let state = root.join("state");
        store::private_dir(&state).unwrap();
        Self(Context {
            home: root.clone(),
            codex_home: root.clone(),
            repository: root,
            github: "example/product".into(),
            store: Store {
                root: state,
                host: "a".repeat(64),
            },
        })
    }

    fn submit(&self, id: &str) -> Task {
        let id = format!("task-{id}");
        submit_with_policy(
            &self.0,
            id.clone(),
            "許可済みの既存機能実装依頼".into(),
            "元の依頼: 既存機能の不具合を直す".into(),
            None,
            vec![],
            vec!["cpu".into()],
            "high".into(),
            vec!["self-hosted-ci".into()],
            ci::GateMode::GithubFreePrivate,
            recovery::Boundary::Finish,
        )
        .unwrap();
        self.0.load(&id).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.home);
    }
}

fn assessment(followups: Vec<recovery::Followup>) -> recovery::Assessment {
    recovery::Assessment {
        cause: "既存taskとの重複を避けて残作業を分割する".into(),
        evidence: "既存task stateと今回の報告を照合した".into(),
        next_action: "元依頼の範囲内で必要なtaskだけ登録する".into(),
        resume_when: "必要なtaskの状態と依存関係を確認できる".into(),
        decision: None,
        watch_paths: vec![],
        followups,
    }
}

fn followup(
    objective: &str,
    after_previous: bool,
    reuse_task_id: Option<&str>,
    distinct_from: Vec<(&str, &str)>,
) -> recovery::Followup {
    recovery::Followup {
        objective: objective.into(),
        authorization: "元依頼内の既存作業を完了する".into(),
        after_previous,
        purpose: recovery::FollowupPurpose::Continuation,
        reuse_task_id: reuse_task_id.map(str::to_owned),
        distinct_from: distinct_from
            .into_iter()
            .map(|(task_id, reason)| recovery::FollowupDistinction {
                task_id: task_id.into(),
                reason: reason.into(),
            })
            .collect(),
    }
}

fn new_parent(fixture: &Fixture, id: &str) -> Task {
    fixture.submit(id)
}

fn create_child(fixture: &Fixture, parent: &mut Task, objective: &str) -> String {
    register_followups(
        &fixture.0,
        parent,
        &assessment(vec![followup(objective, false, None, vec![])]),
    )
    .unwrap();
    parent.recovery.children.last().unwrap().clone()
}

#[test]
fn exact_hash_re_report_remains_idempotent_without_distinction_even_with_siblings() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "hash-retry-with-siblings");
    let first = followup("最初の既存作業", false, None, vec![]);
    let second = followup("二つ目の既存作業", false, None, vec![]);
    register_followups(
        &fixture.0,
        &mut parent,
        &assessment(vec![first.clone(), second]),
    )
    .unwrap();
    let ids_before = child_ids(&fixture);
    let children_before = parent.recovery.children.clone();
    let child_before = serialized_child(&fixture, &children_before[0]);

    register_followups(&fixture.0, &mut parent, &assessment(vec![first])).unwrap();

    assert_eq!(child_ids(&fixture), ids_before);
    assert_eq!(parent.recovery.children, children_before);
    assert_eq!(
        serialized_child(&fixture, &children_before[0]),
        child_before
    );
}

fn child_ids(fixture: &Fixture) -> Vec<String> {
    fixture
        .0
        .store
        .list()
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect()
}

fn serialized_child(fixture: &Fixture, id: &str) -> serde_json::Value {
    serde_json::to_value(fixture.0.store.load(id).unwrap()).unwrap()
}

#[test]
fn explicit_reuse_keeps_paused_and_completed_children_byte_for_byte_unchanged() {
    for (parent_suffix, phase) in [("paused", Phase::Paused), ("completed", Phase::Completed)] {
        let fixture = Fixture::new();
        let mut parent = new_parent(&fixture, parent_suffix);
        let id = create_child(&fixture, &mut parent, "既存の検証結果を整理する");
        let mut child = fixture.0.store.load(&id).unwrap();
        match phase {
            Phase::Paused => child.stop(Phase::Paused, "fixtureで停止状態を設定"),
            Phase::Completed => child.phase = Phase::Completed,
            _ => unreachable!(),
        }
        fixture.0.store.save(&child).unwrap();
        let before = serialized_child(&fixture, &id);
        let ids_before = child_ids(&fixture);
        let children_before = parent.recovery.children.clone();
        let paraphrase = followup("既存の検証内容をまとめ直す", false, Some(&id), vec![]);

        register_followups(&fixture.0, &mut parent, &assessment(vec![paraphrase])).unwrap();

        assert_eq!(parent.recovery.children, children_before);
        assert_eq!(child_ids(&fixture), ids_before);
        assert_eq!(serialized_child(&fixture, &id), before);
    }
}

#[test]
fn existing_siblings_require_exact_nonempty_distinctions_for_new_hash_children() {
    let invalid_distinctions = [
        vec![],
        vec![("FIRST", "既存taskとは別の目的")], // 2つ目の既存taskを欠落
        vec![
            ("FIRST", "既存taskとは別の目的"),
            ("FIRST", "同じtaskを重複して指定"),
            ("SECOND", "もう1件とも別の目的"),
        ],
        vec![
            ("FIRST", "既存taskとは別の目的"),
            ("SECOND", "もう1件とも別の目的"),
            ("task-foreign", "親に所属しないtaskを指定"),
        ],
        vec![("FIRST", ""), ("SECOND", "もう1件とも別の目的")],
    ];

    for (case, bad) in invalid_distinctions.into_iter().enumerate() {
        let fixture = Fixture::new();
        let mut parent = new_parent(&fixture, &format!("siblings-{case}"));
        register_followups(
            &fixture.0,
            &mut parent,
            &assessment(vec![
                followup("既存のtask 1", false, None, vec![]),
                followup("既存のtask 2", false, None, vec![]),
            ]),
        )
        .unwrap();
        let before = child_ids(&fixture);
        let old_children = parent.recovery.children.clone();
        let bad = bad
            .into_iter()
            .map(|(id, reason)| {
                let id = match id {
                    "FIRST" => old_children[0].as_str(),
                    "SECOND" => old_children[1].as_str(),
                    other => other,
                };
                (id, reason)
            })
            .collect();
        let invalid = followup("新しい独立作業を追加する", false, None, bad);

        assert!(register_followups(&fixture.0, &mut parent, &assessment(vec![invalid])).is_err());

        assert_eq!(parent.recovery.children, old_children);
        assert_eq!(child_ids(&fixture), before);
    }
}

#[test]
fn two_distinct_creates_keep_after_previous_dependency_order() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "ordered-create");
    let existing_id = create_child(&fixture, &mut parent, "先に登録済みの別作業");
    let distinctions = vec![(existing_id.as_str(), "以前のtaskとは対象と成果物が異なる")];

    register_followups(
        &fixture.0,
        &mut parent,
        &assessment(vec![
            followup("独立した資料を調査する", false, None, distinctions.clone()),
            followup("調査結果を文書へ反映する", true, None, distinctions),
        ]),
    )
    .unwrap();

    assert_eq!(parent.recovery.children.len(), 3);
    let first = fixture.0.store.load(&parent.recovery.children[1]).unwrap();
    let second = fixture.0.store.load(&parent.recovery.children[2]).unwrap();
    assert!(first.dependencies.is_empty());
    assert_eq!(second.dependencies, vec![first.id]);
}

fn assert_reuse_rejected_after_mutation(mutate: fn(&mut Task)) {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "contract-mismatch");
    let id = create_child(&fixture, &mut parent, "契約照合用の既存作業");
    let mut child = fixture.0.store.load(&id).unwrap();
    mutate(&mut child);
    let path = fixture.0.store.path(&id).unwrap();
    store::atomic_json(&path, &child).unwrap();
    let before = fs::read(&path).unwrap();
    let paraphrase = followup("同じ成果を別表現で報告する", false, Some(&id), vec![]);

    assert!(register_followups(&fixture.0, &mut parent, &assessment(vec![paraphrase])).is_err());

    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn explicit_reuse_checks_parent_ownership_and_every_inherited_contract_field() {
    assert_reuse_rejected_after_mutation(|child| {
        child.recovery.parent = Some("task-other-parent".into())
    });
    assert_reuse_rejected_after_mutation(|child| child.repository.push("-other"));
    assert_reuse_rejected_after_mutation(|child| child.github = "example/other".into());
    assert_reuse_rejected_after_mutation(|child| child.host = "b".repeat(64));
    assert_reuse_rejected_after_mutation(|child| child.authorization = "別の認可契約".into());
    assert_reuse_rejected_after_mutation(|child| child.requested_gate = ci::GateMode::Strict);
    assert_reuse_rejected_after_mutation(|child| child.ci_labels.clear());
    assert_reuse_rejected_after_mutation(|child| child.resources = vec!["other".into()]);
    assert_reuse_rejected_after_mutation(|child| child.risk = "critical".into());
    assert_reuse_rejected_after_mutation(|child| child.stop_after = recovery::Boundary::Pr);
}

#[test]
fn explicit_reuse_rejects_foreign_and_nonchild_ids_without_mutating_the_target() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "foreign-reuse");
    let foreign = fixture.submit("unrelated");
    let before = serialized_child(&fixture, &foreign.id);
    let ids_before = child_ids(&fixture);
    let attempt = followup(
        "別taskの目的を言い換えて再利用する",
        false,
        Some(&foreign.id),
        vec![],
    );

    assert!(register_followups(&fixture.0, &mut parent, &assessment(vec![attempt])).is_err());

    assert!(parent.recovery.children.is_empty());
    assert_eq!(child_ids(&fixture), ids_before);
    assert_eq!(serialized_child(&fixture, &foreign.id), before);
}

#[test]
fn all_followups_are_preflighted_before_a_valid_create_then_invalid_reuse_or_distinction() {
    for invalid_is_reuse in [true, false] {
        let fixture = Fixture::new();
        let mut parent = new_parent(
            &fixture,
            if invalid_is_reuse {
                "preflight-reuse"
            } else {
                "preflight-distinct"
            },
        );
        let sibling = create_child(&fixture, &mut parent, "登録済みの兄弟task");
        let ids_before = child_ids(&fixture);
        let children_before = parent.recovery.children.clone();
        let valid = followup(
            "先行する新しいtask",
            false,
            None,
            vec![(sibling.as_str(), "既存の兄弟taskと目的を区別できる")],
        );
        let invalid = if invalid_is_reuse {
            followup(
                "別の既存taskとして扱う",
                false,
                Some("task-not-a-child"),
                vec![],
            )
        } else {
            followup("必要な区別を欠いたtask", false, None, vec![])
        };

        assert!(
            register_followups(&fixture.0, &mut parent, &assessment(vec![valid, invalid])).is_err()
        );

        assert_eq!(parent.recovery.children, children_before);
        assert_eq!(child_ids(&fixture), ids_before);
    }
}

#[test]
fn old_followup_json_deserializes_with_default_reuse_fields() {
    let old = json!({
        "objective": "既存形式の後続作業",
        "authorization": "元依頼の認可",
        "after_previous": false
    });
    let decoded: recovery::Followup = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.reuse_task_id, None);
    assert!(decoded.distinct_from.is_empty());
    assert_eq!(decoded.purpose, recovery::FollowupPurpose::Continuation);
}

#[test]
fn worker_receives_existing_child_scope_and_pause_state_without_changing_it() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "prompt");
    let id = create_child(&fixture, &mut parent, "後続受入の不足条件を整理する");
    let mut child = fixture.0.load(&id).unwrap();
    child.stop(Phase::Paused, "停止を維持する");
    fixture.0.store.save(&child).unwrap();
    let before = fs::read(fixture.0.store.path(&id).unwrap()).unwrap();

    let prompt = worker::prompt_with_followups(&fixture.0, &parent).unwrap();

    assert!(prompt.contains(&id));
    assert!(prompt.contains("\"objective\":\"後続受入の不足条件を整理する\""));
    assert!(prompt.contains("\"phase\":\"paused\""));
    assert!(prompt.contains("reuse_task_id"));
    assert!(prompt.contains("distinct_from"));
    assert!(prompt.contains("\"plan_id\":\"codex-task-task-prompt-v1\""));
    assert_eq!(
        fs::read(fixture.0.store.path(&id).unwrap()).unwrap(),
        before
    );
}

#[test]
fn oversized_later_objective_rejects_the_batch_before_any_registration() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "oversized-later-objective");
    parent.objective = "o".repeat(31_000);
    fixture.0.store.save(&parent).unwrap();
    let before = child_ids(&fixture);
    let plan = assessment(vec![
        followup("短い後続", false, None, vec![]),
        followup(&"n".repeat(8_000), false, None, vec![]),
    ]);

    assert!(register_followups(&fixture.0, &mut parent, &plan).is_err());

    assert!(parent.recovery.children.is_empty());
    assert_eq!(child_ids(&fixture), before);
}

#[test]
fn interrupted_main_repair_registration_reuses_its_owned_unlinked_child() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "interrupted-repair-registration");
    parent.phase = Phase::MainCi;
    parent.merge_commit = Some("c".repeat(40));
    parent.ci_state = Some(ci::CiState::Failure);
    parent.recovery.barrier = true;
    let mut repair = followup("失敗したmain CIの原因を修復する", false, None, vec![]);
    repair.purpose = recovery::FollowupPurpose::RepairMainCi;
    let plan = assessment(vec![repair]);
    register_followups(&fixture.0, &mut parent, &plan).unwrap();
    let id = parent.recovery.repair_task.clone().unwrap();
    let mut child = fixture.0.load(&id).unwrap();
    child.recovery.parent = None;
    child.recovery.repairs_main = false;
    fixture.0.store.save(&child).unwrap();
    parent.recovery.children.clear();
    parent.recovery.repair_task = None;
    let before = child_ids(&fixture);

    register_followups(&fixture.0, &mut parent, &plan).unwrap();

    assert_eq!(child_ids(&fixture), before);
    assert_eq!(parent.recovery.children, vec![id.clone()]);
    assert_eq!(parent.recovery.repair_task, Some(id.clone()));
    let child = fixture.0.load(&id).unwrap();
    assert_eq!(child.recovery.parent, Some(parent.id));
    assert!(child.recovery.repairs_main);
}

fn diagnosis_report(outcome: &str, assessment: recovery::Assessment) -> worker::Report {
    worker::Report {
        generation: 1,
        outcome: outcome.into(),
        summary: "診断結果に基づく再計画".into(),
        pr: None,
        head: None,
        risk: "high".into(),
        assessment: Some(assessment),
    }
}

fn review_and_failed_ci(task: &mut Task) {
    task.pr = Some(18);
    task.head = Some("a".repeat(40));
    task.base = Some("b".repeat(40));
    task.review = Some(worker::Report {
        generation: task.generation,
        outcome: "ready".into(),
        summary: "以前の独立レビュー".into(),
        pr: task.pr,
        head: task.head.clone(),
        risk: task.risk.clone(),
        assessment: None,
    });
    task.review_independent = true;
    task.ci_state = Some(ci::CiState::Failure);
    task.ci_fingerprint = "previous-failed-ci".into();
}

#[test]
fn needs_changes_with_reused_child_resumes_parent_without_mutating_child_or_reusing_review() {
    for (suffix, phase) in [
        ("pending", Phase::Implementation),
        ("paused", Phase::Paused),
        ("completed", Phase::Completed),
    ] {
        let fixture = Fixture::new();
        let mut parent = new_parent(&fixture, &format!("resume-{suffix}"));
        let original_objective = "親完了後にだけ開始する後続作業";
        register_followups(
            &fixture.0,
            &mut parent,
            &assessment(vec![followup(original_objective, true, None, vec![])]),
        )
        .unwrap();
        let child_id = parent.recovery.children[0].clone();
        let mut child = fixture.0.store.load(&child_id).unwrap();
        match phase {
            Phase::Implementation => {}
            Phase::Paused => {
                child.stop(Phase::Paused, "停止状態をfixtureで設定");
                child.pause_requested = true;
            }
            Phase::Completed => child.phase = Phase::Completed,
            _ => unreachable!(),
        }
        fixture.0.store.save(&child).unwrap();
        let child_path = fixture.0.store.path(&child_id).unwrap();
        let child_before = fs::read(&child_path).unwrap();
        assert_eq!(child.dependencies, vec![parent.id.clone()]);

        parent.phase = Phase::Diagnosis;
        parent.recovery.origin = Some(Phase::Review);
        review_and_failed_ci(&mut parent);
        let mut next = followup(
            "後続作業の完了条件を別の表現で整理する",
            false,
            Some(&child_id),
            vec![],
        );
        next.authorization = "同じ元依頼から後続目的を確認した".into();

        apply_diagnosis(
            &fixture.0,
            &mut parent,
            &diagnosis_report("needs_changes", assessment(vec![next])),
        )
        .unwrap();

        assert_eq!(parent.phase, Phase::Implementation);
        assert!(parent.review.is_none());
        assert!(!parent.review_independent);
        assert_ne!(parent.ci_state, Some(ci::CiState::Success));
        assert_eq!(parent.recovery.children, vec![child_id]);
        assert_eq!(fs::read(child_path).unwrap(), child_before);
        let tasks = vec![parent.clone(), child.clone()];
        assert!(ready(&fixture.0, &parent, &tasks).unwrap());
        assert!(!ready(&fixture.0, &child, &tasks).unwrap());
    }
}

#[test]
fn needs_changes_with_new_independent_child_returns_parent_to_implementation() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "resume-new-child");
    parent.phase = Phase::Diagnosis;
    parent.recovery.origin = Some(Phase::Review);
    review_and_failed_ci(&mut parent);
    let before = child_ids(&fixture);

    apply_diagnosis(
        &fixture.0,
        &mut parent,
        &diagnosis_report(
            "needs_changes",
            assessment(vec![followup(
                "親の修正に続く独立した検証作業",
                false,
                None,
                vec![],
            )]),
        ),
    )
    .unwrap();

    assert_eq!(parent.phase, Phase::Implementation);
    assert!(parent.review.is_none());
    assert!(!parent.review_independent);
    assert_ne!(parent.ci_state, Some(ci::CiState::Success));
    assert_eq!(parent.recovery.children.len(), 1);
    let child = fixture.0.store.load(&parent.recovery.children[0]).unwrap();
    assert_eq!(child.recovery.parent.as_deref(), Some(parent.id.as_str()));
    assert!(child_ids(&fixture).len() > before.len());
}

#[test]
fn ready_and_waiting_reports_with_continuations_remain_waiting() {
    for outcome in ["ready", "waiting"] {
        let fixture = Fixture::new();
        let mut parent = new_parent(&fixture, &format!("keep-waiting-{outcome}"));
        let child_id = create_child(&fixture, &mut parent, "既存の外部待機中作業");
        parent.phase = Phase::Diagnosis;
        parent.recovery.origin = Some(Phase::Implementation);

        apply_diagnosis(
            &fixture.0,
            &mut parent,
            &diagnosis_report(
                outcome,
                assessment(vec![followup(
                    "同じ外部条件を待つ後続作業",
                    false,
                    Some(&child_id),
                    vec![],
                )]),
            ),
        )
        .unwrap();

        assert_eq!(parent.phase, Phase::Waiting, "{outcome}");
        assert!(parent.run.is_none());
        assert_eq!(parent.recovery.children, vec![child_id]);
    }
}

#[test]
fn main_ci_repair_followup_keeps_waiting_barrier_and_repair_ownership() {
    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "main-ci-barrier");
    parent.phase = Phase::Diagnosis;
    parent.recovery.origin = Some(Phase::MainCi);
    parent.merge_commit = Some("c".repeat(40));
    parent.ci_state = Some(ci::CiState::Failure);
    parent.recovery.barrier = true;
    let mut repair = followup("失敗したmain CIを修復する", false, None, vec![]);
    repair.purpose = recovery::FollowupPurpose::RepairMainCi;

    apply_diagnosis(
        &fixture.0,
        &mut parent,
        &diagnosis_report("needs_changes", assessment(vec![repair])),
    )
    .unwrap();

    assert_eq!(parent.phase, Phase::Waiting);
    assert!(parent.recovery.barrier);
    let repair_id = parent.recovery.repair_task.as_ref().unwrap();
    assert_eq!(parent.recovery.children, vec![repair_id.clone()]);
    let child = fixture.0.store.load(repair_id).unwrap();
    assert_eq!(child.recovery.parent.as_deref(), Some(parent.id.as_str()));
    assert!(child.recovery.repairs_main);
}

#[test]
fn repeated_causes_and_typed_decisions_are_not_bypassed_by_continuations() {
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    let mut parent = new_parent(&fixture, "repeat-cause");
    let child_id = create_child(&fixture, &mut parent, "繰り返し原因の既存child");
    parent.phase = Phase::Diagnosis;
    parent.recovery.origin = Some(Phase::Review);
    let repeated = assessment(vec![followup(
        "同じchildを使う反復原因",
        false,
        Some(&child_id),
        vec![],
    )]);
    parent.recovery.evidence = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", repeated.cause, repeated.evidence))
    );
    parent.recovery.progress_observation = format!(
        "{:?}|{}|{:?}",
        parent.head, parent.ci_fingerprint, parent.ci_state
    );
    parent.recovery.attempts = 3;

    apply_diagnosis(
        &fixture.0,
        &mut parent,
        &diagnosis_report("needs_changes", repeated),
    )
    .unwrap();

    assert_eq!(parent.phase, Phase::Waiting);
    assert_eq!(parent.recovery.attempts, 4);
    assert_eq!(parent.recovery.children, vec![child_id]);

    let decision_fixture = Fixture::new();
    let mut awaiting = new_parent(&decision_fixture, "decision-gate");
    awaiting.phase = Phase::Diagnosis;
    awaiting.recovery.origin = Some(Phase::Implementation);
    let before = child_ids(&decision_fixture);
    let mut plan = assessment(vec![followup(
        "製品判断待ちでは登録しない後続作業",
        false,
        None,
        vec![],
    )]);
    plan.decision = Some(recovery::Decision {
        kind: "product".into(),
        question: "互換性契約を変更するか".into(),
        impact: "既存clientへの影響がある".into(),
        recommendation: "判断を待つ".into(),
    });

    apply_diagnosis(
        &decision_fixture.0,
        &mut awaiting,
        &diagnosis_report("awaiting_decision", plan),
    )
    .unwrap();

    assert_eq!(awaiting.phase, Phase::AwaitingDecision);
    assert!(awaiting.recovery.children.is_empty());
    assert_eq!(child_ids(&decision_fixture), before);
}
