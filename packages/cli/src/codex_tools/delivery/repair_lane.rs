//! 別repositoryの基盤修復だけへ共有枠を貸し、親の所有と証拠を保持する。
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    parent_checkout: PathBuf,
    parent: lane::Scope,
    parent_receipt_sha256: String,
    parent_merge: String,
    child_task: String,
    child_pr: i64,
    child_head: String,
    child_plan: String,
    child_plan_version: i64,
    child_receipt_sha256: String,
    purpose: String,
    authorization: String,
    product_test_failures: bool,
    parent_failure_evidence: String,
    reason: String,
}

pub(super) fn entry(args: &[String]) -> Result<Value> {
    if args.len() != 2 || args[0] != "--request" || !Path::new(&args[1]).is_absolute() {
        return Err(error("repair-lane --request <private絶対path> が必要です"));
    }
    let request: Request =
        serde_json::from_value(external_private_json(Path::new(&args[1]), "修復枠request")?)
            .map_err(|_| error("修復枠request schemaが不正です"))?;
    if request.version != 1
        || request.purpose != "repair-ci-infrastructure"
        || request.authorization.trim().is_empty()
        || request.authorization.len() > 8192
        || request.product_test_failures
        || request.parent_failure_evidence.trim().is_empty()
        || request.parent_failure_evidence.len() > 8192
        || request.reason.trim().is_empty()
        || request.reason.len() > 4096
    {
        return Err(error("修復理由とversionが必要です"));
    }
    let root = env::current_dir().map_err(|_| error("修復checkoutを確認できません"))?;
    let parent_root = validate_absolute_path(&request.parent_checkout, "先行checkout")?;
    let parent_repo = repository(&parent_root)?;
    let child_repo = repository(&root)?;
    if parent_repo != request.parent.repository || child_repo == parent_repo {
        return Err(error("別repositoryの修復scopeを確認できません"));
    }
    task_id(&request.child_task)?;
    // 固定順でlockし、同じ親子への並行更新を避ける。
    let mut locks = [
        (parent_repo.as_str(), request.parent.task.as_str()),
        (child_repo.as_str(), request.child_task.as_str()),
    ];
    locks.sort();
    let _first = task_lock(locks[0].0, locks[0].1)?;
    let _second = task_lock(locks[1].0, locks[1].1)?;
    let parent = load_receipt(
        &parent_root,
        &request.parent.task,
        &request.parent.head,
        Some(&parent_repo),
    )?;
    let child = load_receipt(
        &root,
        &request.child_task,
        &request.child_head,
        Some(&child_repo),
    )?;
    if emergency::digest(&parent)? != request.parent_receipt_sha256
        || emergency::digest(&child)? != request.child_receipt_sha256
        || receipt_gate_mode(&parent)? != emergency::GATE
        || int_value(&parent, "pr")? as u64 != request.parent.pr
    {
        return Err(error("修復枠の固定receiptが一致しません"));
    }
    let (manifest, _) = manifest(&parent_root, &request.parent.task, &parent_repo)?;
    let state = load_state(
        &parent_repo,
        &request.parent.task,
        &parent,
        &str_value(&manifest, "branch")?,
    )?;
    emergency::replacement_stage(&state, true)?;
    let view = pr_view(&parent_root, &parent_repo, request.parent.pr as i64)?;
    assert_recovery_pr(
        &view,
        &parent_repo,
        &str_value(&manifest, "branch")?,
        &request.parent.head,
    )?;
    if str_value(get(&view, "mergeCommit")?, "oid")? != oid(&request.parent_merge, "親merge")? {
        return Err(error("親の実mergeが変化しました"));
    }
    validate_receipt_review_history(&parent_root, &parent)?;
    review_safety(&parent_root, &parent_repo, request.parent.pr as i64)?;
    let live = fetch_main(&parent_root, &parent_repo)?;
    let observed = emergency::observe_for_repair(&parent_root, &parent, &view, &live)?;
    let entries = observed["target_ci"]["entries"]
        .as_array()
        .ok_or_else(|| error("親CI観測が不正です"))?;
    if !entries.iter().any(|entry| {
        entry["state"]
            .as_str()
            .is_some_and(|s| !matches!(s, "success" | "skipped" | "neutral"))
    }) {
        return Err(error("先行PRに未回復の基盤障害がありません"));
    }
    match_cli_receipt(
        &child,
        Some(request.child_pr),
        Some(&request.child_head),
        Some(&request.child_plan),
        Some(request.child_plan_version),
        &receipt_gate_mode(&child)?,
    )?;
    if risk_level(&str_value(&child, "risk")?) < risk_level("high")
        || child["independent_review_passed"] != true
    {
        return Err(error("基盤修復にはhigh以上と独立reviewが必要です"));
    }
    let (manifest, path) = super::manifest(&root, &request.child_task, &child_repo)?;
    worktree(&root, &manifest, &path)?;
    worktree_clean_head(&path, &request.child_head)?;
    validate_receipt_evidence(&path, &manifest, &child)?;
    validate_receipt_review_history(&root, &child)?;
    review_safety(&root, &child_repo, request.child_pr)?;
    assert_recovery_pr_open(
        &pr_view(&root, &child_repo, request.child_pr)?,
        &child_repo,
        &str_value(&manifest, "branch")?,
        &request.child_head,
    )?;
    let base = fetch_main(&root, &child_repo)?;
    if !emergency::git_ancestor(&root, &base, &request.child_head)? {
        return Err(error("修復headが最新baseを含んでいません"));
    }
    let scope = lane::Scope {
        repository: child_repo.clone(),
        task: request.child_task.clone(),
        pr: request.child_pr as u64,
        head: request.child_head.clone(),
        base,
        labels: vec![],
    };
    // 証拠を先に永続化。枠交換の途中停止は同じrequestで再開する。
    let proof = receipt_path(&child_repo, &request.child_task, &request.child_head)?
        .with_extension("lane-repair.json");
    atomic_json(
        &proof,
        &object([
            (
                "parent",
                serde_json::to_value(&request.parent)
                    .map_err(|_| error("親scopeを保存できません"))?,
            ),
            ("parent_merge", string(request.parent_merge)),
            (
                "parent_receipt_sha256",
                string(request.parent_receipt_sha256),
            ),
            ("child_receipt_sha256", string(request.child_receipt_sha256)),
            ("purpose", string(request.purpose)),
            ("authorization", string(request.authorization)),
            ("product_test_failures", Value::Bool(false)),
            (
                "parent_failure_evidence",
                string(request.parent_failure_evidence),
            ),
            ("observed", observed),
            ("reason", string(request.reason)),
            (
                "child",
                serde_json::to_value(&scope).map_err(|_| error("修復scopeを保存できません"))?,
            ),
        ]),
    )?;
    lane::claim_repair(&request.parent, scope)?;
    Ok(object([("status", string("repair-admitted"))]))
}
