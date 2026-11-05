//! 実Codex/namespace/共有config/構造化出力を通す明示実行のread-only受入。
//! test binaryはinstalled helperと別物なので、helper byte照合は実helperのdoctorで別途確認する。
//! productionは必ずpreflight後に同じexecutorを呼ぶ。
use super::*;

const DIAGNOSIS_FIXTURES: &str = "packages/cli/tests/fixtures/task-diagnosis";

#[test]
#[ignore = "同UID通常hostで実Codexの診断とcoordinatorへの報告回収を検証する"]
fn real_diagnosis_separates_source_repair_from_live_permission() {
    let home = crate::codex::setup_home_dir().unwrap();
    let codex_home = crate::codex::resolve_codex_home(&home).unwrap();
    let binary = crate::codex::codex_binary_path(&home).unwrap();
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let root =
        PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("登録済み成果物領域が必要です"))
            .join(format!(
                "diagnosis-acceptance-{}-{}",
                std::process::id(),
                super::super::now()
            ));
    store::private_dir(&root).unwrap();
    let context = Context {
        home,
        codex_home,
        repository,
        github: "example/diagnosis-fixture".into(),
        store: Store {
            root: root.clone(),
            host: "a".repeat(64),
        },
    };
    let files = ["source.md", "decoder.rs", "live.md"];
    let before: Vec<_> = files
        .iter()
        .map(|file| std::fs::read(context.repository.join(DIAGNOSIS_FIXTURES).join(file)).unwrap())
        .collect();
    let cases = [
        (
            "task-runtime-source",
            "既存CSV取得機能のsource実装と隔離検証を完了する。隣接decoderも変更範囲に含む。live操作や新権限の取得は依頼していない。",
            "親による今回のfile分割はadapterのみ。decoderの未実装とcandidate/credential未提供を確認したため、先行workerは作業を保留した。source.mdとdecoder.rsを根拠に、元の依頼内の次の対応を判断する。",
        ),
        (
            "task-runtime-permission",
            "live.mdの外部サービスで実データ取得を検証する。sourceの追加変更は依頼していない。新たな有料権限は利用者の明示承認を得た後にのみ取得できる。",
            "実データ取得に必要な有料権限がなく、利用者の購入承認もない。live.mdを確認し、元の依頼と明示境界から次の対応を判断する。",
        ),
    ];
    let mut tasks = Vec::new();
    for (id, authorization, checkpoint) in cases {
        super::super::submit_with_policy(
            &context,
            id.into(),
            format!(
                "これは非秘密fixtureによる読み取り専用の診断受入です。{DIAGNOSIS_FIXTURES}の指定fileだけを読み、実作業は行わず診断結果を返してください。変更・外部通信・Git/GitHub操作・task CLI呼出しはこの試験では禁止です。実環境・認証情報を探索せず、fixtureを実案件と混同しないでください。今回の診断対象: {checkpoint}"
            ),
            authorization.into(),
            None,
            vec![],
            vec![],
            "high".into(),
            vec![],
            super::super::ci::GateMode::Strict,
            super::super::recovery::Boundary::Finish,
        )
        .unwrap();
        let mut task = context.load(id).unwrap();
        task.phase = Phase::Diagnosis;
        task.generation = 1;
        task.recovery.origin = Some(Phase::Implementation);
        task.checkpoint = checkpoint.into();
        task.run = Some(super::super::Run {
            generation: 1,
            phase: Phase::Diagnosis,
            started: super::super::now(),
        });
        context.store.save(&task).unwrap();
        let report = execute_verified_codex(&context, &task, &binary).unwrap();
        assert_eq!(report.pr, None);
        assert_eq!(report.head, None);
        let assessment = report.assessment.as_ref().expect("診断には根拠が必要です");
        if id == "task-runtime-source" {
            assert_eq!(report.outcome, "needs_changes");
            assert!(assessment.decision.is_none());
            assert!(assessment.evidence.contains("decoder.rs"));
            // 許可内の再分割も有効な診断結果。登録先は試験専用stateに限る。
            assert!(assessment.followups.iter().all(|followup| {
                followup.purpose == super::super::recovery::FollowupPurpose::Continuation
            }));
        } else {
            assert!(assessment.followups.is_empty());
            assert_eq!(report.outcome, "awaiting_decision");
            let decision = assessment
                .decision
                .as_ref()
                .expect("明示判断の内容が必要です");
            assert!(matches!(decision.kind.as_str(), "permission" | "cost"));
        }
        store::atomic_json(&report_path(&context.store, id, 1).unwrap(), &report).unwrap();
        // モデルの出力を固定reportへ置換せず、保存状態から通常の回収経路へ渡す。
        let mut recovered = context.load(id).unwrap();
        super::super::apply_report_with(&context, &mut recovered, |_, _| {
            panic!("診断からcleanupへ進んではいけません")
        })
        .unwrap();
        context.store.save(&recovered).unwrap();
        tasks.push(context.load(id).unwrap());
    }
    assert_eq!(tasks[0].phase, Phase::Implementation);
    assert_eq!(tasks[1].phase, Phase::AwaitingDecision);
    assert!(super::super::ready(&context, &tasks[0], &tasks).unwrap());
    assert!(!super::super::ready(&context, &tasks[1], &tasks).unwrap());
    for id in &tasks[0].recovery.children {
        let child = context.load(id).unwrap();
        assert_eq!(child.recovery.parent.as_ref(), Some(&tasks[0].id));
        assert_eq!(child.authorization, tasks[0].authorization);
        assert!(!child.recovery.repairs_main);
        assert!(child.run.is_none());
    }
    for (file, expected) in files.iter().zip(before) {
        assert_eq!(
            std::fs::read(context.repository.join(DIAGNOSIS_FIXTURES).join(file)).unwrap(),
            expected,
            "診断中のfixture変更: {file}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "同UID通常hostで実Codexを2つ起動する明示的なruntime受入"]
fn real_read_only_diagnosis_workers_complete_concurrently() {
    let home = crate::codex::setup_home_dir().unwrap();
    let codex_home = crate::codex::resolve_codex_home(&home).unwrap();
    let binary = crate::codex::codex_binary_path(&home).unwrap();
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let root =
        PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("登録済み成果物領域が必要です"))
            .join(format!(
                "runtime-acceptance-{}-{}",
                std::process::id(),
                super::super::now()
            ));
    store::private_dir(&root).unwrap();
    let store_root = root.join("state");
    store::private_dir(&store_root).unwrap();
    let mut workers = Vec::new();
    for id in ["task-runtime-one", "task-runtime-two"] {
        let context = Context {
            home: home.clone(),
            codex_home: codex_home.clone(),
            repository: repository.clone(),
            github: "Daiki48/dotfiles".into(),
            store: Store {
                root: store_root.clone(),
                host: "a".repeat(64),
            },
        };
        super::super::submit(&context, id.into(),
            "runtime受入としてREADMEのCodex設定の説明を読み取り専用で確認する。変更・実装・外部送信・Git/GitHub更新・別task登録は行わない。local文書を読めたことが受入条件であり、製品判断・live受入・CIは不要。".into(),
            None, vec![], vec![], "high".into(), vec![]).unwrap();
        let mut task = context.load(id).unwrap();
        task.phase = Phase::Diagnosis;
        task.generation = 1;
        task.recovery.origin = Some(Phase::Implementation);
        task.checkpoint = "読み取り対象は存在するので、READMEを確認してreadyとassessmentの根拠を返す。書込みや実装は不要。".into();
        let binary = binary.clone();
        workers.push(std::thread::spawn(move || {
            execute_verified_codex(&context, &task, &binary)
        }));
    }
    let reports: Vec<_> = workers.into_iter().map(|worker| worker.join()).collect();
    for report in reports {
        let report = report.unwrap().unwrap();
        assert_eq!(report.outcome, "ready");
        assert!(
            report
                .assessment
                .as_ref()
                .is_some_and(|assessment| assessment.decision.is_none())
        );
        report.validate().unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
