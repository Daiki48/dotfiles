//! 実Codex/namespace/共有config/構造化出力を通す明示実行のread-only受入。
//! test binaryはinstalled helperと別物なので、helper byte照合は実helperのdoctorで別途確認する。
//! productionは必ずpreflight後に同じexecutorを呼ぶ。
use super::*;

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
