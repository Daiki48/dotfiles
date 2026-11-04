//! 保存stateからの複数task継続を通す。外部CI/label/cleanupだけをfixture化する。
use super::tests::{Fixture, successful_observation};
use super::*;

fn reopen(context: &Context) -> Context {
    Context {
        home: context.home.clone(),
        codex_home: context.codex_home.clone(),
        repository: context.repository.clone(),
        github: context.github.clone(),
        store: Store {
            root: context.store.root.clone(),
            host: context.store.host.clone(),
        },
    }
}

fn report(
    context: &Context,
    task: &mut Task,
    outcome: &str,
    assessment: Option<recovery::Assessment>,
) {
    task.generation += 1;
    task.run = Some(Run {
        generation: task.generation,
        phase: task.phase.clone(),
        started: now(),
    });
    context.store.save(task).unwrap();
    let report = worker::Report {
        generation: task.generation,
        outcome: outcome.into(),
        summary: "固定対象の検証・対応結果".into(),
        pr: task.pr.or_else(|| {
            (task.phase == Phase::Implementation).then_some(if task.id == "task-one" {
                1
            } else {
                2
            })
        }),
        head: task
            .head
            .clone()
            .or_else(|| (task.phase == Phase::Implementation).then_some("a".repeat(40))),
        risk: "high".into(),
        assessment,
    };
    store::atomic_json(
        &worker::report_path(&context.store, &task.id, task.generation).unwrap(),
        &report,
    )
    .unwrap();
    // 親のin-memory stateを使わず、別のContextから保存済み世代と報告を回収する。
    let reopened = reopen(context);
    *task = reopened.load(&task.id).unwrap();
    apply_report_with(&reopened, task, |_, task| {
        Ok(task.worktree.as_ref().is_some_and(|path| !path.exists()))
    })
    .unwrap();
    reopened.store.save(task).unwrap();
}

#[test]
fn persisted_two_task_delivery_continues_after_scope_replan_and_admission_failure() {
    let fixture = Fixture::new();
    fixture.submit("task-one");
    fixture.submit("task-two");
    fixture.submit("task-human");
    let context = &fixture.0;
    let mut one = context.load("task-one").unwrap();
    let mut two = context.load("task-two").unwrap();
    let mut human = context.load("task-human").unwrap();
    let assessment = recovery::Assessment {
        cause: "親のfile分割".into(),
        evidence: "既存仕様で隣接実装が必要".into(),
        next_action: "同じ機能の範囲で再分割して検証".into(),
        resume_when: "local検証とreviewを通す".into(),
        decision: None,
        watch_paths: vec![],
        followups: vec![],
    };
    report(
        context,
        &mut one,
        "awaiting_decision",
        Some(assessment.clone()),
    );
    assert_eq!(one.phase, Phase::Diagnosis);
    report(context, &mut one, "needs_changes", Some(assessment.clone()));
    assert_eq!(one.phase, Phase::Implementation);
    human.phase = Phase::Diagnosis;
    human.recovery.origin = Some(Phase::Implementation);
    let mut decision = assessment;
    decision.decision = Some(recovery::Decision {
        kind: "product".into(),
        question: "新しい仕様を採用するか".into(),
        impact: "利用者の表示挙動が変わる".into(),
        recommendation: "現仕様を維持する".into(),
    });
    report(context, &mut human, "awaiting_decision", Some(decision));
    assert_eq!(human.phase, Phase::AwaitingDecision);

    for task in [&mut one, &mut two] {
        assert!(ready(context, task, &[task.clone(), human.clone()]).unwrap());
        report(context, task, "ready", None);
        assert_eq!(task.phase, Phase::Review);
        task.base = Some("b".repeat(40));
        let mut admission = successful_observation();
        admission.state = ci::CiState::Failure;
        record_ci_result(task, Ok(admission.clone())).unwrap();
        assert_eq!(task.phase, Phase::Review);
        report(context, task, "ready", None);
        task.ci_labels = vec!["ci-approved".into()];
        task.base = None;
        record_ci_result(task, Ok(admission.clone())).unwrap();
        assert_eq!(task.phase, Phase::CiAuthorization);
        task.next_check = 0;
        let mut labels = 0;
        authorize_ci_with(task, |task| {
            assert_eq!(task.ci_labels, ["ci-approved"]);
            assert_eq!(task.head, Some("a".repeat(40)));
            labels += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(labels, 1);
        assert_eq!(task.phase, Phase::Integration);
        record_ci_result(task, Ok(admission)).unwrap();
        assert!(!ready(context, task, std::slice::from_ref(task)).unwrap());
        let mut pending = successful_observation();
        pending.state = ci::CiState::Pending;
        record_ci_result(task, Ok(pending)).unwrap();
        assert!(!ready(context, task, std::slice::from_ref(task)).unwrap());
        record_ci_result(task, Ok(successful_observation())).unwrap();
        assert!(ready(context, task, std::slice::from_ref(task)).unwrap());
        report(context, task, "ready", None);
        assert_eq!(task.phase, Phase::MainCi);
        let mut merged = successful_observation();
        merged.merged = true;
        merged.merge_commit = Some("c".repeat(40));
        merged.target = "c".repeat(40);
        record_ci_result(task, Ok(merged)).unwrap();
        assert_eq!(task.phase, Phase::Finish);
        assert!(ready(context, task, std::slice::from_ref(task)).unwrap());
        let workspace = context.repository.join(&task.id);
        std::fs::create_dir(&workspace).unwrap();
        task.worktree = Some(workspace.clone());
        // fixture helperの物理回収結果を完了確認callbackで再照合する。
        std::fs::remove_dir(&workspace).unwrap();
        report(context, task, "ready", None);
        assert_eq!(task.phase, Phase::Completed);
        assert!(!workspace.exists());
    }
    assert_eq!(context.load("task-one").unwrap().phase, Phase::Completed);
    assert_eq!(context.load("task-two").unwrap().phase, Phase::Completed);
    assert_eq!(
        context.load("task-human").unwrap().phase,
        Phase::AwaitingDecision
    );
}

#[test]
fn interrupted_merge_is_observed_before_retrying_any_write_or_reusing_pr_ci() {
    let fixture = Fixture::new();
    fixture.submit("task-one");
    let mut task = fixture.0.load("task-one").unwrap();
    report(&fixture.0, &mut task, "ready", None);
    task.base = Some("b".repeat(40));
    report(&fixture.0, &mut task, "ready", None);
    task.phase = Phase::Integration;
    task.diagnose("helperの終了報告前にworkerが中断しました");
    let mut observation = successful_observation();
    observation.merged = true;
    observation.merge_commit = Some("c".repeat(40));
    record_ci_result(&mut task, Ok(observation)).unwrap();
    assert_eq!(task.phase, Phase::MainCi);
    assert_eq!(task.merge_commit, Some("c".repeat(40)));
    assert!(task.ci_evidence.is_none());
    assert!(integration_barrier(&task));
    assert!(!ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
}
