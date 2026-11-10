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
    if !repair_needed(&observed)? {
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
    let child_view = pr_view(&root, &child_repo, request.child_pr)?;
    assert_child_open(
        &child_view,
        &child_repo,
        &str_value(&manifest, "branch")?,
        &request.child_head,
    )?;
    let child_ci_wait = if str_value(&child_view, "mergeStateStatus")? == "BLOCKED" {
        blocked_ci_evidence(&root, &child_repo, &request.child_head)?
    } else {
        Value::Null
    };
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
            ("child_ci_wait", child_ci_wait),
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

fn blocked_ci_evidence(root: &Path, repository: &str, head: &str) -> Result<Value> {
    let data = graphql(
        root,
        repository,
        "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){ref(qualifiedName:\"refs/heads/main\"){name branchProtectionRule{id}}}}",
        &[],
    )?;
    let branch = get(get(&data, "repository")?, "ref")?;
    if str_value(branch, "name")? != "main" {
        return Err(error("修復base refを確認できません"));
    }
    let protection = get(branch, "branchProtectionRule")?;
    let rules = gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/rules/branches/main?per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
        ],
    )?;
    let checks = gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/commits/{head}/check-runs?per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
        ],
    )?;
    blocked_ci_only(protection, &rules, &checks, head)?;
    Ok(object([
        ("head", string(head)),
        ("observed_at", string(now())),
        ("protection", protection.clone()),
        ("rules", rules),
        ("checks", checks),
    ]))
}

/// BLOCKEDの理由をCI待ちへ限定する。配備・approval・未知ruleは枠取得前に保持する。
fn blocked_ci_only(
    protection: &Value,
    rule_pages: &Value,
    check_pages: &Value,
    head: &str,
) -> Result<()> {
    if !protection.is_null() {
        return Err(error("classic protectionのBLOCKED理由を確定できません"));
    }
    let pages = rule_pages
        .as_array()
        .filter(|pages| pages.len() <= MAX_PAGES)
        .ok_or_else(|| error("適用Ruleset一覧が不完全です"))?;
    let mut required = HashSet::new();
    for page in pages {
        for rule in page.as_array().ok_or_else(|| error("適用ruleが不正です"))? {
            match str_value(rule, "type")?.as_str() {
                "deletion" | "non_fast_forward" => (),
                "pull_request" => {
                    let p = get(rule, "parameters")?;
                    let keys = [
                        "dismiss_stale_reviews_on_push",
                        "require_code_owner_review",
                        "require_last_push_approval",
                        "required_approving_review_count",
                        "required_review_thread_resolution",
                        "allowed_merge_methods",
                        "require_extra_approval_for_unattributed_changes",
                    ];
                    if object_ref(p)?
                        .keys()
                        .any(|key| !keys.contains(&key.as_str()))
                        || int_value(p, "required_approving_review_count")? != 0
                        || bool_value(p, "require_code_owner_review")?
                        || bool_value(p, "require_last_push_approval")?
                        || p.get("require_extra_approval_for_unattributed_changes")
                            .is_some_and(|value| value != false)
                        || p["allowed_merge_methods"] != serde_json::json!(["merge"])
                    {
                        return Err(error("修復PRにCI以外のreview/merge条件があります"));
                    }
                }
                "required_status_checks" => {
                    let p = get(rule, "parameters")?;
                    if !bool_value(p, "strict_required_status_checks_policy")?
                        || object_ref(p)?.keys().any(|key| {
                            ![
                                "strict_required_status_checks_policy",
                                "required_status_checks",
                                "do_not_enforce_on_create",
                            ]
                            .contains(&key.as_str())
                        })
                    {
                        return Err(error("required CI ruleを確定できません"));
                    }
                    for check in get(p, "required_status_checks")?
                        .as_array()
                        .ok_or_else(|| error("required CI一覧が不正です"))?
                    {
                        if int_value(check, "integration_id")? != 15368 {
                            return Err(error("required CI提供元が確認できません"));
                        }
                        let context = str_value(check, "context")?;
                        if context.is_empty() {
                            return Err(error("required CI名が不正です"));
                        }
                        required.insert(context);
                    }
                }
                _ => {
                    return Err(error(
                        "配備または未知のruleがBLOCKEDを阻害する可能性があります",
                    ));
                }
            }
        }
    }
    if required.is_empty() {
        return Err(error("未完了required CIを確認できません"));
    }
    let pages = check_pages
        .as_array()
        .filter(|pages| pages.len() <= MAX_PAGES)
        .ok_or_else(|| error("CI一覧が不完全です"))?;
    let mut count = 0;
    let mut total = None;
    let mut passed = HashSet::new();
    let mut waiting = false;
    for page in pages {
        let n = int_value(page, "total_count")?;
        if n < 0 || n as usize > MAX_ITEMS || total.is_some_and(|old| old != n) {
            return Err(error("CI一覧が取得中に変化しました"));
        }
        total = Some(n);
        for check in get(page, "check_runs")?
            .as_array()
            .ok_or_else(|| error("CI一覧が不正です"))?
        {
            count += 1;
            if str_value(check, "head_sha")? != head {
                return Err(error("CI対象headが一致しません"));
            }
            let status = str_value(check, "status")?;
            let name = str_value(check, "name")?;
            let actions = int_value(get(check, "app")?, "id")? == 15368;
            match status.as_str() {
                "completed"
                    if matches!(
                        str_value(check, "conclusion")?.as_str(),
                        "success" | "skipped" | "neutral"
                    ) =>
                {
                    if actions {
                        passed.insert(name);
                    }
                }
                "queued" | "in_progress" | "pending" | "requested" | "waiting"
                    if get(check, "conclusion")?.is_null() =>
                {
                    if actions && required.contains(&name) {
                        waiting = true;
                    }
                }
                _ => return Err(error("CI待ち以外の失敗または未知状態があります")),
            }
        }
    }
    if total != Some(count) {
        return Err(error("CI一覧が不完全です"));
    }
    if waiting || required.iter().any(|name| !passed.contains(name)) {
        Ok(())
    } else {
        Err(error("BLOCKED理由をrequired CI待ちと確認できません"))
    }
}

/// CI起動の枠を借りる段階ではrequired checks未完了のBLOCKEDを許可する。merge許可ではない。
fn assert_child_open(view: &Value, repository: &str, branch: &str, head: &str) -> Result<()> {
    let draft = bool_value(view, "isDraft")?;
    let status = str_value(view, "mergeStateStatus")?;
    if str_value(view, "state")? != "OPEN"
        || str_value(view, "headRefOid")? != head
        || str_value(view, "headRefName")? != branch
        || str_value(view, "baseRefName")? != "main"
        || view.get("isCrossRepository") != Some(&Value::Bool(false))
        || !same_head_repository(view, repository)
        || !view.get("autoMergeRequest").is_some_and(Value::is_null)
        || str_value(view, "mergeable")? != "MERGEABLE"
        || !(matches!(status.as_str(), "CLEAN" | "UNSTABLE" | "BLOCKED")
            || draft && status == "DRAFT")
    {
        return Err(error("修復PRの固定identity/conflict/stateを確認できません"));
    }
    Ok(())
}

fn repair_needed(observed: &Value) -> Result<bool> {
    if observed["approved"] != false {
        return Err(error("修復専用の非delivery観測が必要です"));
    }
    let target = observed["target_ci"]["entries"]
        .as_array()
        .ok_or_else(|| error("親CI観測が不正です"))?;
    let live = if observed["live_ci"].is_null() {
        None
    } else {
        Some(
            observed["live_ci"]["entries"]
                .as_array()
                .ok_or_else(|| error("最新main観測が不正です"))?,
        )
    };
    Ok(target
        .iter()
        .chain(live.into_iter().flatten())
        .any(|entry| {
            entry["state"]
                .as_str()
                .is_some_and(|s| !matches!(s, "success" | "skipped" | "neutral"))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_admission_requires_only_ruleset_ci_wait_and_rejects_deployment_or_unknown_blockers()
    {
        let rules = serde_json::json!([[{"type":"pull_request","parameters":{"required_approving_review_count":0,"require_code_owner_review":false,"require_last_push_approval":false,"required_review_thread_resolution":true,"allowed_merge_methods":["merge"]}},{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"unit","integration_id":15368}]}}]]);
        let checks = serde_json::json!([{"total_count":1,"check_runs":[{"name":"unit","head_sha":"a".repeat(40),"status":"queued","conclusion":null,"app":{"id":15368}}]}]);
        assert!(blocked_ci_only(&Value::Null, &rules, &checks, &"a".repeat(40)).is_ok());
        assert!(
            blocked_ci_only(
                &Value::Null,
                &rules,
                &serde_json::json!([{"total_count":0,"check_runs":[]}]),
                &"a".repeat(40)
            )
            .is_ok()
        );
        let mut deployment = rules.clone();
        deployment[0].as_array_mut().unwrap().push(serde_json::json!({"type":"required_deployments","parameters":{"required_deployment_environments":["production"]}}));
        assert!(blocked_ci_only(&Value::Null, &deployment, &checks, &"a".repeat(40)).is_err());
        assert!(
            blocked_ci_only(
                &serde_json::json!({"id":"synthetic-protection"}),
                &rules,
                &checks,
                &"a".repeat(40)
            )
            .is_err()
        );
        assert!(
            blocked_ci_only(
                &Value::Null,
                &serde_json::json!([[{"type":"unknown-rule"}]]),
                &checks,
                &"a".repeat(40)
            )
            .is_err()
        );
        let mut passed = checks.clone();
        passed[0]["check_runs"][0]["status"] = string("completed");
        passed[0]["check_runs"][0]["conclusion"] = string("success");
        assert!(blocked_ci_only(&Value::Null, &rules, &passed, &"a".repeat(40)).is_err());
        passed[0]["check_runs"][0]["conclusion"] = string("failure");
        assert!(blocked_ci_only(&Value::Null, &rules, &passed, &"a".repeat(40)).is_err());
        assert!(
            blocked_ci_only(
                &Value::Null,
                &rules,
                &serde_json::json!([]),
                &"a".repeat(40)
            )
            .is_err()
        );
    }
    #[test]
    fn repair_can_start_ci_while_required_checks_are_blocked_but_cannot_merge() {
        let mut view = serde_json::json!({"state":"OPEN","headRefOid":"a".repeat(40),"headRefName":"fix/infra","baseRefName":"main","isCrossRepository":false,"headRepository":{"nameWithOwner":"example/infrastructure"},"headRepositoryOwner":{"login":"example"},"autoMergeRequest":null,"mergeable":"MERGEABLE","isDraft":true,"mergeStateStatus":"BLOCKED"});
        assert!(
            assert_child_open(
                &view,
                "example/infrastructure",
                "fix/infra",
                &"a".repeat(40)
            )
            .is_ok()
        );
        assert!(
            assert_recovery_pr_open(
                &view,
                "example/infrastructure",
                "fix/infra",
                &"a".repeat(40)
            )
            .is_err()
        );
        for state in ["CONFLICTING", "UNKNOWN"] {
            view["mergeable"] = string(state);
            assert!(
                assert_child_open(
                    &view,
                    "example/infrastructure",
                    "fix/infra",
                    &"a".repeat(40)
                )
                .is_err()
            );
        }
    }
    #[test]
    fn live_main_failure_can_request_repair_without_waiving_either_commit() {
        let mut observed = serde_json::json!({"approved":false,"target_ci":{"entries":[{"state":"success"}]},"live_ci":{"sha":"a".repeat(40),"entries":[{"state":"failure"}]}});
        assert!(repair_needed(&observed).unwrap());
        observed["live_ci"]["entries"][0]["state"] = string("pending");
        assert!(repair_needed(&observed).unwrap());
        observed["live_ci"]["entries"][0]["state"] = string("success");
        assert!(!repair_needed(&observed).unwrap());
        observed["approved"] = Value::Bool(true);
        assert!(repair_needed(&observed).is_err());
    }
}
