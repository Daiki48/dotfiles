//! 保存済みworker報告から正規receipt・CI認可までを通す回帰検証。
use super::super::delivery::authorization_tests::{Fixture, isolated_process, with_fixture};
use super::*;

fn reviewed_task(fixture: &Fixture) -> (Context, Task) {
    let context = Context {
        home: fixture.home.clone(),
        codex_home: fixture.home.clone(),
        repository: fixture.root.clone(),
        github: "owner/repo".into(),
        store: Store {
            root: fixture.home.join("tasks"),
            host: "a".repeat(64),
        },
    };
    store::private_dir(&context.store.root).unwrap();
    submit(
        &context,
        "task-fixture".into(),
        "合成変更の検証".into(),
        Some("feat/fixture".into()),
        vec![],
        vec![],
        "high".into(),
        vec!["ci-approved".into()],
    )
    .unwrap();
    let mut task = context.load("task-fixture").unwrap();
    task.pr = Some(7);
    task.head = Some(fixture.head.clone());
    task.worktree = Some(fixture.worktree.clone());
    task.base = Some(fixture.base.clone());
    workflow_tests::report(&context, &mut task, "ready", None);
    assert_eq!(task.phase, Phase::Review);
    assert!(!fixture.receipt.exists());
    workflow_tests::report(&context, &mut task, "ready", None);
    assert_eq!(task.phase, Phase::CiAuthorization);
    assert!(review_ready(&task));
    assert!(!fixture.receipt.exists());
    (context, task)
}

#[test]
fn reviewed_task_records_a_real_receipt_before_ci_authorization_and_resumes() {
    if !isolated_process() {
        return;
    }
    with_fixture(|fixture| {
        let (context, mut task) = reviewed_task(fixture);
        authorize_ci(&context, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Integration, "{}", task.checkpoint);
        assert_eq!(fixture.labels(), 1);
        let receipt = fixture.receipt();
        assert_eq!(receipt["plan_id"], "codex-task-task-fixture-v1");
        assert_eq!(receipt["plan_version"], 1);
        assert_eq!(receipt["risk"], "high");
        assert_eq!(receipt["gate_mode"], "strict-ruleset");
        assert_eq!(receipt["changed_files"], json!(["source.txt"]));
        let bytes = std::fs::read(&fixture.receipt).unwrap();
        // receipt保存後に認可処理が中断した場合も、同じreceiptを再利用する。
        task.phase = Phase::CiAuthorization;
        authorize_ci(&context, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Integration);
        assert_eq!(std::fs::read(&fixture.receipt).unwrap(), bytes);
    });
}

#[test]
fn ci_authorization_preserves_existing_plan_version_and_human_approval() {
    if !isolated_process() {
        return;
    }
    with_fixture(|fixture| {
        let (context, mut task) = reviewed_task(fixture);
        fixture.record("existing-plan-v2", true);
        let bytes = std::fs::read(&fixture.receipt).unwrap();
        authorize_ci(&context, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Integration, "{}", task.checkpoint);
        assert_eq!(fixture.labels(), 1);
        assert_eq!(fixture.receipt()["plan_id"], "existing-plan-v2");
        assert_eq!(fixture.receipt()["plan_version"], 3);
        assert_eq!(fixture.receipt()["decision"], "human-approved");
        assert_eq!(std::fs::read(&fixture.receipt).unwrap(), bytes);
    });
}

#[test]
fn automatic_ci_without_labels_records_before_authorization() {
    if !isolated_process() {
        return;
    }
    with_fixture(|fixture| {
        let (context, mut task) = reviewed_task(fixture);
        task.ci_labels.clear();
        authorize_ci(&context, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Integration, "{}", task.checkpoint);
        assert_eq!(fixture.receipt()["head_sha"], fixture.head);
        assert_eq!(fixture.labels(), 0);
    });
}

#[test]
fn scheduler_records_receipt_after_first_base_observation_without_labels() {
    if !isolated_process() {
        return;
    }
    for risk in ["low", "medium", "high"] {
        for workflows in [false, true] {
            with_fixture(|fixture| {
                let (context, mut task) = reviewed_task(fixture);
                task.risk = risk.into();
                task.review.as_mut().unwrap().risk = risk.into();
                task.review_independent = risk == "high";
                task.base = None;
                task.ci_labels.clear();
                if !workflows {
                    fixture.without_workflows();
                }
                let gate = if workflows {
                    ci::GateMode::Strict
                } else {
                    ci::GateMode::LocalValidation
                };
                let mut value = serde_json::to_value(tests::successful_observation()).unwrap();
                value["head"] = json!(fixture.head);
                value["target"] = json!(fixture.head);
                value["base"] = json!(fixture.base);
                if !workflows {
                    value["state"] = json!("local_validation");
                    value["gate_mode"] = json!(gate.as_str());
                    let tree = |sha: &str| json!({"commit":sha,"tree":sha,"workflows":[]});
                    value["workflows"] = json!({
                        "base":tree(&fixture.base),"head":tree(&fixture.head),"main":null
                    });
                }
                let observation: ci::Observation = serde_json::from_value(value).unwrap();
                assert!(observation.allows_delivery());

                // reconcileと同じ認可・観測・再認可の順序で初回base取得を通す。
                authorize_ci(&context, &mut task).unwrap();
                assert_eq!(fixture.calls(), 0);
                observe_ci_with(&context, &mut task, |_, _| Ok(observation.clone())).unwrap();
                authorize_ci(&context, &mut task).unwrap();
                assert_eq!(task.phase, Phase::CiAuthorization);
                assert_eq!(task.base.as_ref(), Some(&fixture.base));
                assert!(!fixture.receipt.exists());

                // 次の観測期限に認可が先行し、receiptを保存してから統合へ進む。
                task.next_check = 0;
                authorize_ci(&context, &mut task).unwrap();
                assert_eq!(task.phase, Phase::Integration, "{}", task.checkpoint);
                let receipt = fixture.receipt();
                assert_eq!(receipt["head_sha"], fixture.head);
                assert_eq!(receipt["risk"], risk);
                assert_eq!(receipt["tests_passed"], true);
                assert_eq!(receipt["independent_review_passed"], risk == "high");
                assert_eq!(
                    receipt["gate_mode"],
                    if workflows {
                        "strict-ruleset"
                    } else {
                        gate.as_str()
                    }
                );
                assert_eq!(fixture.labels(), 0);
                observe_ci_with(&context, &mut task, |_, _| Ok(observation)).unwrap();
                assert!(task.ci_evidence.as_ref().unwrap().allows_delivery());
            });
        }
    }
}

#[test]
fn receipt_gate_uses_complete_live_workflow_trees() {
    if !isolated_process() {
        return;
    }
    with_fixture(|fixture| {
        let select =
            || ci::authorization_gate(&fixture.root, "owner/repo", 7, &fixture.head, &fixture.base);
        assert_eq!(select().unwrap(), ci::GateMode::Strict);
        fixture.without_workflows();
        assert_eq!(select().unwrap(), ci::GateMode::LocalValidation);
        fixture.incomplete_tree();
        assert!(select().is_err());
        assert!(!fixture.receipt.exists());
        assert_eq!(fixture.labels(), 0);
    });
}

#[test]
fn receipt_or_live_target_failure_never_adds_ci_labels() {
    if !isolated_process() {
        return;
    }
    for failure in ["dirty", "receipt", "api", "threads", "base", "head"] {
        with_fixture(|fixture| {
            let (context, mut task) = reviewed_task(fixture);
            match failure {
                "dirty" => std::fs::write(fixture.worktree.join("source.txt"), "dirty\n").unwrap(),
                "receipt" => fixture.corrupt_receipt(),
                "api" => fixture.unavailable(),
                "threads" => fixture.unresolved(),
                "base" => fixture.stale_base(),
                "head" => {
                    task.head = Some(fixture.base.clone());
                    task.review.as_mut().unwrap().head = task.head.clone();
                }
                _ => unreachable!(),
            }
            authorize_ci(&context, &mut task).unwrap();
            assert_eq!(task.phase, Phase::Diagnosis, "{failure}");
            assert_eq!(fixture.labels(), 0, "{failure}");
            assert!(
                task.events
                    .iter()
                    .any(|event| event.kind == "ci_authorization_wait")
            );
        });
    }
}

#[test]
fn missing_review_pause_and_optional_ci_policy_do_not_record_or_authorize() {
    if !isolated_process() {
        return;
    }
    for reason in ["review", "pause", "optional"] {
        with_fixture(|fixture| {
            let (context, mut task) = reviewed_task(fixture);
            match reason {
                "review" => task.review_independent = false,
                "pause" => task.pause_requested = true,
                "optional" => {
                    task.requested_gate = ci::GateMode::OptionalCiLocal;
                    task.gate_mode = ci::GateMode::OptionalCiLocal;
                    task.ci_labels.clear();
                }
                _ => unreachable!(),
            }
            authorize_ci(&context, &mut task).unwrap();
            assert_eq!(task.phase, Phase::CiAuthorization);
            assert!(!fixture.receipt.exists());
            assert_eq!(fixture.calls(), 0);
            assert_eq!(fixture.labels(), 0);
        });
    }
}
