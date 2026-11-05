//! Issue #84: 再判定・監視・再開を、状態遷移として検証する。
use super::*;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn explicit_legacy_policy_update_invalidates_review_and_refuses_merged_or_active_tasks() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("policy-update", "medium", recovery::Boundary::Finish);
    task.phase = Phase::Blocked;
    fixture.0.store.save(&task).unwrap();
    resume_with_options(
        &fixture.0,
        &task.id,
        "既存の明示されたrepository方針を引き継ぐ".into(),
        Some("github-free-private".into()),
        Some("既存Free/private経路を使う".into()),
    )
    .unwrap();
    task = fixture.0.load(&task.id).unwrap();
    assert_eq!(task.requested_gate, ci::GateMode::GithubFreePrivate);
    assert_eq!(task.risk, "high");
    assert_eq!(task.phase, Phase::Implementation);
    assert!(task.review.is_none() && task.ci_evidence.is_none());
    task.merge_commit = Some("a".repeat(40));
    fixture.0.store.save(&task).unwrap();
    assert!(
        resume_with_options(
            &fixture.0,
            &task.id,
            "変更要求".into(),
            Some("strict".into()),
            None
        )
        .is_err()
    );
    task.merge_commit = None;
    task.run = Some(Run {
        generation: task.generation,
        phase: task.phase.clone(),
        started: now(),
    });
    fixture.0.store.save(&task).unwrap();
    assert!(resume_with_options(&fixture.0, &task.id, "追加指示".into(), None, None).is_err());
}

#[test]
fn shared_resource_release_wakes_monitoring_without_a_new_writer_while_held() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let mut task = fixture.submit("resource-wait", "high", recovery::Boundary::Finish);
    store::private_dir(&fixture.0.codex_home.join("worktrees")).unwrap();
    let root = fixture.0.codex_home.join("worktrees/.task-resources");
    store::private_dir(&root).unwrap();
    let path = root.join(format!("{:x}", Sha256::digest(b"cpu")));
    let lease = Lock::try_acquire(&path).unwrap().unwrap();
    let inherited_fd = unsafe { libc::dup(lease.fd()) };
    assert!(inherited_fd >= 3);
    let descendant = unsafe { Lock::from_inherited(inherited_fd, &path) }.unwrap();
    descendant.close_on_exec().unwrap();
    task.recovery.origin = Some(Phase::Implementation);
    task.monitor("別repositoryの共有資源を監視");
    monitor_task(&fixture.0, &mut task);
    task.recovery.next_observation = 0;
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Waiting);
    drop(lease);
    // 親のdropだけでは子に残る同じopen file descriptionのlockは解放されない。
    task.recovery.next_observation = 0;
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Waiting);
    drop(descendant);
    // 並列testのfork→exec間にもCLOEXECのFDが一時的に残る。実際の解放後の復帰を観測する。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        task.recovery.next_observation = 0;
        monitor_task(&fixture.0, &mut task);
        if task.phase == Phase::Diagnosis || std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(task.phase, Phase::Diagnosis);
    assert!(task.run.is_none());
}

struct Fixture(Context);

impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "task-autonomy-{}-{}",
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

    fn submit(&self, id: &str, risk: &str, stop_after: recovery::Boundary) -> Task {
        let id = format!("task-{id}");
        submit_with_policy(
            &self.0,
            id.clone(),
            "許可済みの既存機能実装依頼".into(),
            "元の依頼: 既存機能の不具合を直す".into(),
            None,
            vec![],
            vec!["cpu".into()],
            risk.into(),
            vec!["self-hosted-ci".into()],
            if risk == "high" {
                ci::GateMode::GithubFreePrivate
            } else {
                ci::GateMode::Strict
            },
            stop_after,
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

fn assessment() -> recovery::Assessment {
    recovery::Assessment {
        cause: "実装とレビューのfile分割が一致しない".into(),
        evidence: "レビュー指摘は同じ機能の別fileを対象にしている".into(),
        next_action: "既存仕様を維持して責務を再分割する".into(),
        resume_when: "変更箇所の受入条件を再検証できる".into(),
        decision: None,
        watch_paths: vec![],
        followups: vec![],
    }
}

fn report(
    generation: u64,
    outcome: &str,
    assessment: Option<recovery::Assessment>,
) -> worker::Report {
    worker::Report {
        generation,
        outcome: outcome.into(),
        summary: format!("worker outcome: {outcome}"),
        pr: None,
        head: None,
        risk: "high".into(),
        assessment,
    }
}

fn write_report(fixture: &Fixture, task: &Task, run: &Run, report: &worker::Report) {
    store::atomic_json(
        &worker::report_path(&fixture.0.store, &task.id, run.generation).unwrap(),
        report,
    )
    .unwrap();
}

fn strict_observation(state: ci::CiState, merged: bool) -> ci::Observation {
    let head = "a".repeat(40);
    let base = "b".repeat(40);
    let merge = "c".repeat(40);
    serde_json::from_value(json!({
        "state":state,
        "head":head,
        "base":base,
        "target":if merged { merge.clone() } else { head.clone() },
        "fingerprint":format!("{state:?}"),
        "merge_commit":if merged { Some(merge) } else { None::<String> },
        "merged":merged,
        "observed_at":now(),
        "required":[],
        "snapshot":{"runs":[],"statuses":[]},
        "gate_mode":"strict"
    }))
    .unwrap()
}

fn ready_for_integration(fixture: &Fixture, id: &str) -> Task {
    let mut task = fixture.submit(id, "high", recovery::Boundary::Finish);
    make_ready_for_integration(&mut task);
    task
}

fn make_ready_for_integration(task: &mut Task) {
    task.phase = Phase::Integration;
    task.pr = Some(18);
    task.head = Some("a".repeat(40));
    task.base = Some("b".repeat(40));
    task.review = Some(worker::Report {
        generation: 1,
        outcome: "ready".into(),
        summary: "固定HEADの独立レビュー済み".into(),
        pr: task.pr,
        head: task.head.clone(),
        risk: task.risk.clone(),
        assessment: None,
    });
    task.review_independent = true;
    let observation = strict_observation(ci::CiState::Success, false);
    task.ci_state = Some(observation.state);
    task.ci_fingerprint = observation.fingerprint.clone();
    task.ci_evidence = Some(observation);
}

#[test]
fn legacy_stops_are_reclassified_and_diagnosis_can_be_scheduled() {
    let fixture = Fixture::new();
    for (id, phase) in [
        ("legacy-blocked", Phase::Blocked),
        ("legacy-awaiting", Phase::AwaitingDecision),
    ] {
        let mut task = fixture.submit(id, "high", recovery::Boundary::Finish);
        task.phase = phase.clone();
        task.resume_phase = Phase::Implementation;
        task.checkpoint = "legacy technical blockage with no typed decision".into();
        let mut legacy = serde_json::to_value(&task).unwrap();
        legacy.as_object_mut().unwrap().remove("recovery");
        let mut loaded: Task = serde_json::from_value(legacy).unwrap();
        recover_legacy(&mut loaded);

        assert_eq!(loaded.phase, Phase::Diagnosis, "{id}");
        assert!(ready(&fixture.0, &loaded, std::slice::from_ref(&loaded)).unwrap());
    }
}

#[test]
fn technical_awaiting_decision_report_enters_diagnosis_but_typed_product_decision_waits() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("awaiting-technical", "high", recovery::Boundary::Finish);
    task.run = Some(Run {
        generation: 1,
        phase: Phase::Implementation,
        started: now(),
    });
    let run = task.run.clone().unwrap();
    write_report(
        &fixture,
        &task,
        &run,
        &report(1, "awaiting_decision", Some(assessment())),
    );
    apply_report(&fixture.0, &mut task).unwrap();
    assert_eq!(task.phase, Phase::Diagnosis);
    assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());

    let mut product = fixture.submit("awaiting-product", "high", recovery::Boundary::Finish);
    let mut decision = assessment();
    decision.decision = Some(recovery::Decision {
        kind: "product".into(),
        question: "公開APIの互換性を変えるか".into(),
        impact: "既存clientの動作が変わる".into(),
        recommendation: "現行契約を維持する".into(),
    });
    product.recovery.origin = Some(Phase::Implementation);
    apply_diagnosis(
        &fixture.0,
        &mut product,
        &report(1, "awaiting_decision", Some(decision)),
    )
    .unwrap();
    assert_eq!(product.phase, Phase::AwaitingDecision);
}

#[test]
fn technical_file_scope_returns_to_implementation_and_three_repairs_trigger_diagnosis() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("scope-return", "high", recovery::Boundary::Finish);
    task.phase = Phase::Review;
    task.recovery.origin = Some(Phase::Review);
    apply_diagnosis(
        &fixture.0,
        &mut task,
        &report(1, "needs_changes", Some(assessment())),
    )
    .unwrap();
    assert_eq!(task.phase, Phase::Implementation);

    task.phase = Phase::Review;
    task.failures = 0;
    for generation in 1..=3 {
        task.generation = generation;
        task.run = Some(Run {
            generation,
            phase: Phase::Review,
            started: now(),
        });
        let run = task.run.clone().unwrap();
        write_report(
            &fixture,
            &task,
            &run,
            &report(generation, "needs_changes", None),
        );
        apply_report(&fixture.0, &mut task).unwrap();
    }
    assert_eq!(task.phase, Phase::Diagnosis);
    assert_eq!(task.failures, 3);
    assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
}

#[test]
fn repeated_wait_observation_does_not_schedule_a_worker_but_changed_evidence_reopens_diagnosis() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("watch-state", "high", recovery::Boundary::Finish);
    let watched = fixture.0.repository.join("acceptance.txt");
    fs::write(&watched, "initial evidence").unwrap();
    task.worktree = Some(fixture.0.repository.clone());
    let mut waiting = assessment();
    waiting.watch_paths = vec!["acceptance.txt".into()];
    task.recovery.assessment = Some(waiting);
    task.phase = Phase::Waiting;
    task.recovery.origin = Some(Phase::Implementation);
    task.recovery.observation = waiting_observation(&fixture.0, &task);
    task.recovery.next_observation = 0;
    let unchanged = task.recovery.observation.clone();
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Waiting);
    assert_eq!(task.recovery.observation, unchanged);
    assert!(!ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());

    fs::write(&watched, "changed evidence that is longer").unwrap();
    task.recovery.next_observation = 0;
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Diagnosis);
    assert!(task.recovery.attempts == 0);
}

#[test]
fn implementation_ci_wait_observes_changes_and_reenters_diagnosis_without_resume() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("implementation-ci-wait", "high", recovery::Boundary::Finish);
    let run = Run {
        generation: 1,
        phase: Phase::Implementation,
        started: now(),
    };
    task.generation = run.generation;
    task.run = Some(run.clone());
    let mut waiting = report(run.generation, "waiting", Some(assessment()));
    waiting.pr = Some(18);
    waiting.head = Some("a".repeat(40));
    write_report(&fixture, &task, &run, &waiting);
    apply_report(&fixture.0, &mut task).unwrap();
    assert_eq!(task.phase, Phase::Diagnosis);
    apply_diagnosis(&fixture.0, &mut task, &waiting).unwrap();
    assert_eq!(task.phase, Phase::Waiting);
    assert_eq!(task.effective_phase(), &Phase::Implementation);
    assert!(task.run.is_none());
    let labels = task.ci_labels.clone();
    let requested_gate = task.requested_gate;
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        Ok(strict_observation(ci::CiState::Pending, false))
    })
    .unwrap();
    task.recovery.observation = waiting_observation(&fixture.0, &task);
    task.recovery.next_observation = 0;
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Waiting);

    task.next_check = 0;
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        Ok(strict_observation(ci::CiState::Success, false))
    })
    .unwrap();
    assert_eq!(task.phase, Phase::Waiting);
    assert_eq!(task.ci_state, Some(ci::CiState::Success));
    task.recovery.next_observation = 0;
    monitor_task(&fixture.0, &mut task);
    assert_eq!(task.phase, Phase::Diagnosis);
    assert!(task.review.is_none());
    assert!(!task.review_independent);
    assert!(!review_ready(&task));
    assert_eq!(task.ci_labels, labels);
    assert_eq!(task.requested_gate, requested_gate);
    assert_eq!(task.stop_after, recovery::Boundary::Finish);

    let mut diagnosis = report(1, "ready", Some(assessment()));
    diagnosis.pr = task.pr;
    diagnosis.head = task.head.clone();
    apply_diagnosis(&fixture.0, &mut task, &diagnosis).unwrap();
    assert_eq!(task.phase, Phase::Implementation);
    assert!(!review_ready(&task));
}

#[test]
fn implementation_ci_wait_preserves_failure_and_invalidates_success_on_api_error() {
    let fixture = Fixture::new();
    for state in [
        ci::CiState::Failure,
        ci::CiState::Cancelled,
        ci::CiState::TimedOut,
    ] {
        let mut task = fixture.submit(
            &format!("implementation-ci-{state:?}").to_lowercase(),
            "high",
            recovery::Boundary::Finish,
        );
        task.pr = Some(18);
        task.head = Some("a".repeat(40));
        task.phase = Phase::Waiting;
        task.recovery.origin = Some(Phase::Implementation);
        observe_ci_with(&fixture.0, &mut task, |_, _| {
            Ok(strict_observation(state, false))
        })
        .unwrap();
        assert_eq!(task.phase, Phase::Waiting);
        assert_eq!(task.ci_state, Some(state));
        assert!(!task.ci_evidence.as_ref().unwrap().allows_delivery());

        task.next_check = 0;
        observe_ci_with(&fixture.0, &mut task, |_, _| {
            Ok(strict_observation(ci::CiState::Success, false))
        })
        .unwrap();
        task.next_check = 0;
        observe_ci_with(&fixture.0, &mut task, |_, _| {
            Err("temporary API error".into())
        })
        .unwrap();
        assert_eq!(task.phase, Phase::Waiting);
        assert!(task.ci_state.is_none());
        assert!(task.ci_evidence.is_none());
        assert!(task.ci_observation_error_at.is_some());
        assert!(task.next_check > now());
    }
}

#[test]
fn implementation_ci_observation_respects_missing_identity_stop_and_backoff() {
    let fixture = Fixture::new();
    let mut task = fixture.submit(
        "implementation-ci-boundaries",
        "high",
        recovery::Boundary::Finish,
    );
    task.phase = Phase::Waiting;
    task.recovery.origin = Some(Phase::Implementation);
    observe_ci_with(&fixture.0, &mut task, |_, _| panic!("PRなしでは観測しない")).unwrap();
    task.pr = Some(18);
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        panic!("HEADなしでは観測しない")
    })
    .unwrap();
    task.head = Some("a".repeat(40));
    task.next_check = now() + 120;
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        panic!("backoff中は観測しない")
    })
    .unwrap();
    task.next_check = 0;
    task.run = Some(Run {
        generation: task.generation,
        phase: Phase::Implementation,
        started: now(),
    });
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        panic!("worker稼働中は観測しない")
    })
    .unwrap();
    task.run = None;
    for phase in [
        Phase::Implementation,
        Phase::Paused,
        Phase::AwaitingDecision,
        Phase::BoundaryReached,
        Phase::Completed,
    ] {
        task.phase = phase;
        observe_ci_with(&fixture.0, &mut task, |_, _| {
            panic!("外部待機以外は拡張しない")
        })
        .unwrap();
    }
}

#[test]
fn implementation_ci_wait_rejects_changed_head_and_keeps_review_invalid() {
    let fixture = Fixture::new();
    let mut task = fixture.submit(
        "implementation-ci-head-drift",
        "high",
        recovery::Boundary::Finish,
    );
    task.pr = Some(18);
    task.head = Some("d".repeat(40));
    task.phase = Phase::Waiting;
    task.recovery.origin = Some(Phase::Implementation);
    observe_ci_with(&fixture.0, &mut task, |_, _| {
        Ok(strict_observation(ci::CiState::Success, false))
    })
    .unwrap();
    assert_eq!(task.phase, Phase::Diagnosis);
    assert_eq!(task.head, Some("d".repeat(40)));
    assert!(task.ci_evidence.is_none());
    assert!(!review_ready(&task));
}

#[test]
fn watch_metadata_rejects_parent_traversal_and_symlinks() {
    let fixture = Fixture::new();
    let file = fixture.0.repository.join("evidence.txt");
    fs::write(&file, "safe").unwrap();
    assert!(watch_metadata(&fixture.0.repository, "../outside").is_err());

    #[cfg(unix)]
    {
        let alias = fixture.0.repository.join("alias.txt");
        std::os::unix::fs::symlink(&file, &alias).unwrap();
        assert!(watch_metadata(&fixture.0.repository, "alias.txt").is_err());
    }
}

#[test]
fn diagnosis_attempts_are_bounded_for_same_evidence_and_reset_for_new_evidence() {
    let fixture = Fixture::new();
    let mut task = fixture.submit("attempt-cap", "high", recovery::Boundary::Finish);
    task.recovery.origin = Some(Phase::Implementation);
    let same = assessment();
    for _ in 0..4 {
        task.record_assessment(&same);
        apply_diagnosis(
            &fixture.0,
            &mut task,
            &report(1, "needs_changes", Some(same.clone())),
        )
        .unwrap();
    }
    assert_eq!(task.phase, Phase::Waiting);
    assert_eq!(task.recovery.attempts, 4);

    let mut new_evidence = same;
    new_evidence.evidence = "再現条件の検証結果が増えた".into();
    task.record_assessment(&new_evidence);
    assert_eq!(task.recovery.attempts, 4);
    task.head = Some("b".repeat(40));
    task.record_assessment(&new_evidence);
    assert_eq!(task.recovery.attempts, 4);
    new_evidence.evidence = "新headの検証で既知の失敗が解消した".into();
    task.ci_fingerprint = "new-observation".into();
    task.record_assessment(&new_evidence);
    assert_eq!(task.recovery.attempts, 0);
}

#[test]
fn followups_are_idempotent_and_inherit_the_parent_contract_without_new_scope() {
    let fixture = Fixture::new();
    let mut parent = fixture.submit("followup-parent", "high", recovery::Boundary::Finish);
    let mut plan = assessment();
    plan.followups.push(recovery::Followup {
        objective: "既存PRの失敗原因を調べ、元依頼内で修復案を実装する".into(),
        authorization: "元依頼は既存機能の不具合修正を許可している".into(),
        after_previous: false,
        purpose: recovery::FollowupPurpose::Continuation,
        reuse_task_id: None,
        distinct_from: vec![],
    });
    register_followups(&fixture.0, &mut parent, &plan).unwrap();
    register_followups(&fixture.0, &mut parent, &plan).unwrap();
    assert_eq!(parent.recovery.children.len(), 1);
    let child = fixture.0.load(&parent.recovery.children[0]).unwrap();
    assert_eq!(child.recovery.parent.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.authorization, parent.authorization);
    assert_eq!(child.risk, parent.risk);
    assert_eq!(child.resources, parent.resources);
    assert_eq!(child.ci_labels, parent.ci_labels);
    assert_eq!(child.requested_gate, parent.requested_gate);
    assert_eq!(child.stop_after, parent.stop_after);
    let prompt = worker::prompt(&child);
    assert!(prompt.contains(&parent.objective));
    assert!(prompt.contains("今回の部分作業: 既存PRの失敗原因"));
    assert!(prompt.contains("新規権限・費用・製品判断・不可逆操作を追加しない"));
    assert!(!prompt.contains("無関係な新機能"));
}

#[test]
fn main_ci_failure_preserves_barrier_except_for_its_repair_child() {
    let fixture = Fixture::new();
    let mut failed = fixture.submit("main-ci-failed", "high", recovery::Boundary::Finish);
    failed.phase = Phase::MainCi;
    failed.head = Some("a".repeat(40));
    failed.merge_commit = Some("c".repeat(40));
    record_ci_result(
        &mut failed,
        Ok(strict_observation(ci::CiState::Failure, true)),
    )
    .unwrap();
    assert_eq!(failed.phase, Phase::Diagnosis);
    assert!(failed.recovery.barrier);

    let mut plan = assessment();
    plan.followups = vec![
        recovery::Followup {
            objective: "失敗したmain CIの原因を修復する".into(),
            authorization: "元の不具合修正".into(),
            after_previous: false,
            purpose: recovery::FollowupPurpose::RepairMainCi,
            reuse_task_id: None,
            distinct_from: vec![],
        },
        recovery::Followup {
            objective: "元依頼に含まれる独立した文書を更新する".into(),
            authorization: "元の文書更新".into(),
            after_previous: false,
            purpose: recovery::FollowupPurpose::Continuation,
            reuse_task_id: None,
            distinct_from: vec![],
        },
    ];
    register_followups(&fixture.0, &mut failed, &plan).unwrap();
    let mut repair = fixture
        .0
        .load(failed.recovery.repair_task.as_ref().unwrap())
        .unwrap();
    let mut unrelated = fixture
        .0
        .load(
            failed
                .recovery
                .children
                .iter()
                .find(|id| *id != &repair.id)
                .unwrap(),
        )
        .unwrap();
    assert!(!unrelated.recovery.repairs_main);
    make_ready_for_integration(&mut repair);
    make_ready_for_integration(&mut unrelated);
    let tasks = vec![failed.clone(), repair.clone(), unrelated.clone()];
    assert!(ready(&fixture.0, &repair, &tasks).unwrap());
    assert!(!ready(&fixture.0, &unrelated, &tasks).unwrap());

    // 自己申告のflagや別の修復taskへの差し替えでは例外を広げない。
    unrelated.recovery.repairs_main = true;
    assert!(!repair_exempt(&unrelated, &failed, &tasks));
    let mut duplicate = plan.clone();
    duplicate.followups[1].purpose = recovery::FollowupPurpose::RepairMainCi;
    assert!(register_followups(&fixture.0, &mut failed, &duplicate).is_err());
    duplicate.followups = vec![duplicate.followups[1].clone()];
    assert!(register_followups(&fixture.0, &mut failed, &duplicate).is_err());

    // 修復PR自体のmain CI失敗では、指定された修復の連鎖だけを通す。
    repair.recovery.barrier = true;
    let mut nested = fixture.submit("nested-main-repair", "high", recovery::Boundary::Finish);
    nested.recovery.parent = Some(repair.id.clone());
    nested.recovery.repairs_main = true;
    repair.recovery.repair_task = Some(nested.id.clone());
    repair.recovery.children.push(nested.id.clone());
    let nested_tasks = vec![failed.clone(), repair.clone(), nested.clone()];
    assert!(repair_exempt(&nested, &failed, &nested_tasks));
    assert!(repair_exempt(&nested, &repair, &nested_tasks));

    // 独立したImplementation作業はmain CIのbarrierで停止しない。
    let implementation = fixture.submit(
        "independent-implementation",
        "high",
        recovery::Boundary::Finish,
    );
    assert!(ready(&fixture.0, &implementation, &tasks).unwrap());
}

#[test]
fn ci_authorization_external_recovery_is_retried_without_requiring_a_ci_fingerprint_change() {
    let fixture = Fixture::new();
    let mut task = ready_for_integration(&fixture, "authorization-retry");
    task.phase = Phase::CiAuthorization;
    task.next_check = 0;
    authorize_ci_with(&mut task, |_| Err("一時的なAPI実行拒否".into())).unwrap();
    assert_eq!(task.phase, Phase::Diagnosis);
    let mut diagnosis = report(task.generation, "waiting", Some(assessment()));
    diagnosis.pr = task.pr;
    diagnosis.head = task.head.clone();
    apply_diagnosis(&fixture.0, &mut task, &diagnosis).unwrap();
    assert_eq!(task.phase, Phase::Waiting);
    let original_fingerprint = task.ci_fingerprint.clone();
    task.recovery.next_authorization = 0;
    retry_ci_authorization_with(&mut task, |_| Err("一時的なAPI実行拒否".into())).unwrap();
    assert_eq!(task.phase, Phase::Waiting);
    assert!(task.recovery.next_authorization > now());
    assert!(task.run.is_none());
    task.recovery.next_authorization = 0;
    assert_eq!(original_fingerprint, task.ci_fingerprint);
    retry_ci_authorization_with(&mut task, |_| Ok(())).unwrap();
    assert_eq!(task.phase, Phase::Integration);
    assert!(task.ci_evidence.is_none());
    assert!(!ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
    task.phase = Phase::AwaitingDecision;
    retry_ci_authorization_with(&mut task, |_| {
        panic!("人間判断の待機中は認可操作を行わない")
    })
    .unwrap();
}

#[test]
fn aged_review_overtakes_new_implementation_and_pr_boundary_prevents_delivery() {
    let fixture = Fixture::new();
    let mut old_review = fixture.submit("old-review", "high", recovery::Boundary::Finish);
    let mut new_impl = fixture.submit("new-implementation", "high", recovery::Boundary::Finish);
    old_review.phase = Phase::Review;
    old_review.created = 1;
    old_review.recovery.last_scheduled = 1;
    new_impl.phase = Phase::Implementation;
    new_impl.created = now();
    let current = now();
    assert!(
        recovery::scheduling_score(&old_review, current)
            > recovery::scheduling_score(&new_impl, current)
    );

    let mut bounded = fixture.submit("pr-boundary", "medium", recovery::Boundary::Pr);
    bounded.run = Some(Run {
        generation: 1,
        phase: Phase::Implementation,
        started: now(),
    });
    let run = bounded.run.clone().unwrap();
    let ready_report = worker::Report {
        generation: 1,
        outcome: "ready".into(),
        summary: "PR作成済み".into(),
        pr: Some(42),
        head: Some("d".repeat(40)),
        risk: "medium".into(),
        assessment: None,
    };
    write_report(&fixture, &bounded, &run, &ready_report);
    apply_report(&fixture.0, &mut bounded).unwrap();
    assert_eq!(bounded.phase, Phase::BoundaryReached);
    assert_eq!(bounded.pr, Some(42));
    assert_eq!(bounded.stop_after, recovery::Boundary::Pr);
}
