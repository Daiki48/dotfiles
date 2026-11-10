//! 継続委任に基づくCI基盤障害の固定delivery証拠。CIの実測値は変更しない。
use super::*;
use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "emergency_tests.rs"]
pub(super) mod tests;

pub(super) const GATE: &str = "emergency-ci";
pub(super) const VERSION: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalValidation {
    kind: String,
    command: String,
    result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Waiver {
    kind: String,
    name: String,
    source: String,
    states: Vec<String>,
    cause: String,
    evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Request {
    version: i64,
    repository: String,
    task_id: String,
    pr: i64,
    head_sha: String,
    base_sha: String,
    plan_id: String,
    plan_version: i64,
    reason: String,
    authorization: String,
    local_validation: Vec<LocalValidation>,
    independent_review: String,
    known_test_failures: bool,
    acknowledge_workflow_runs: bool,
    public_summary: String,
    waivers: Vec<Waiver>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    kind: String,
    name: String,
    source: String,
    state: String,
    id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    sha: String,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedWorkflow {
    path: String,
    source: String,
}

fn expected(path: &str, event: &str) -> ExpectedWorkflow {
    ExpectedWorkflow {
        path: path.into(),
        source: format!("actions:{event}"),
    }
}

fn text_valid(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

fn waived_state(value: &str) -> bool {
    matches!(
        value,
        "failure"
            | "error"
            | "startup_failure"
            | "cancelled"
            | "timed_out"
            | "queued"
            | "in_progress"
            | "pending"
            | "requested"
            | "waiting"
            | "missing"
    )
}

fn success(value: &str) -> bool {
    matches!(value, "success" | "skipped" | "neutral")
}

fn request(value: &Value, scope: &Value) -> Result<Request> {
    let request: Request = serde_json::from_value(value.clone())
        .map_err(|_| error("緊急承認requestのschemaが不正です"))?;
    for key in [
        "repository",
        "task_id",
        "pr",
        "head_sha",
        "plan_id",
        "plan_version",
    ] {
        if value.get(key) != scope.get(key) {
            return Err(error(
                "緊急承認のrepository/task/PR/head/Planが一致しません",
            ));
        }
    }
    if request.version != 1
        || oid(&request.base_sha, "緊急承認base")?.len() != 40
        || request.known_test_failures
        || !request.acknowledge_workflow_runs
        || !text_valid(&request.reason, 4096)
        || !text_valid(&request.authorization, 8192)
        || !text_valid(&request.independent_review, 8192)
        || !text_valid(&request.public_summary, 4096)
        || request.local_validation.is_empty()
        || request.local_validation.len() > 64
        || request.local_validation.iter().any(|v| {
            !matches!(
                v.kind.as_str(),
                "test" | "build" | "lint" | "format" | "typecheck"
            ) || !text_valid(&v.command, 4096)
                || v.result != "passed"
        })
        || !["test", "build"]
            .iter()
            .all(|kind| request.local_validation.iter().any(|v| v.kind == *kind))
        || request.waivers.is_empty()
        || request.waivers.len() > 128
    {
        return Err(error(
            "緊急CI証拠には対象・理由・委任根拠・成功したlocal検証・独立review・workflow起動の確認が必要です。既知のtest失敗は免除できません",
        ));
    }
    let mut identities = HashSet::new();
    for waiver in &request.waivers {
        if !matches!(
            waiver.kind.as_str(),
            "check-run" | "commit-status" | "workflow" | "ci"
        ) || !text_valid(&waiver.name, 512)
            || !text_valid(&waiver.source, 128)
            || !text_valid(&waiver.evidence, 4096)
            || !matches!(
                waiver.cause.as_str(),
                "admission-failure" | "runner-unavailable" | "service-outage"
            )
            || waiver.states.is_empty()
            || waiver.states.len() > 12
            || waiver.states.iter().any(|state| !waived_state(state))
            || waiver.states.iter().collect::<HashSet<_>>().len() != waiver.states.len()
            || !identities.insert((&waiver.kind, &waiver.name, &waiver.source))
        {
            return Err(error(
                "CI免除は名前・提供元・状態と基盤障害の根拠を明示してください。test失敗や未知状態は対象外です",
            ));
        }
    }
    Ok(request)
}

pub(super) fn validate_receipt(payload: &Value) -> Result<()> {
    let emergency = get(payload, "emergency")?;
    expected_keys(emergency, &["request", "observed_ci"], "緊急receipt")?;
    let request = request(get(emergency, "request")?, payload)?;
    let snapshot: Snapshot = serde_json::from_value(get(emergency, "observed_ci")?.clone())
        .map_err(|_| error("緊急receiptの観測証拠が不正です"))?;
    coverage(&request, &snapshot, &request.head_sha)
}

fn coverage(request: &Request, snapshot: &Snapshot, sha: &str) -> Result<()> {
    if snapshot.sha != sha || snapshot.entries.is_empty() || snapshot.entries.len() > MAX_ITEMS {
        return Err(error("CI観測の対象SHAまたは件数が不正です"));
    }
    for entry in &snapshot.entries {
        if !success(&entry.state)
            && !request.waivers.iter().any(|waiver| {
                waiver.kind == entry.kind
                    && waiver.name == entry.name
                    && waiver.source == entry.source
                    && waiver.states.contains(&entry.state)
            })
        {
            return Err(error(
                "証拠にないCI check・提供元・状態を観測しました。原因・検証・reviewを再確認して固定証拠を更新してください",
            ));
        }
    }
    Ok(())
}

fn workflow_paths(root: &Path, sha: &str) -> Result<Vec<String>> {
    Ok(git(
        root,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            sha,
            "--",
            ".github/workflows",
        ],
        true,
    )?
    .split('\0')
    .filter(|path| workflow_yaml_path(path))
    .map(str::to_string)
    .collect())
}

fn workflow_expectations(
    root: &Path,
    base: &str,
    head: &str,
    merged_target: bool,
) -> Result<Vec<ExpectedWorkflow>> {
    let mut paths: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for reference in [base, head] {
        for path in workflow_paths(root, reference)? {
            let source = git(root, &["show", &format!("{reference}:{path}")], true)?;
            let events = super::super::workflow_policy::trigger_events(&source).map_err(error)?;
            let expected = paths.entry(path).or_default();
            expected.extend(events.into_iter().filter(|event| {
                event == "push"
                    || !merged_target
                        && ["pull_request", "pull_request_target"].contains(&event.as_str())
            }));
        }
    }
    Ok(paths
        .into_iter()
        .flat_map(|(path, events)| {
            if events.is_empty() {
                vec![ExpectedWorkflow {
                    path,
                    source: "actions".into(),
                }]
            } else {
                events
                    .into_iter()
                    .map(|event| expected(&path, &event))
                    .collect()
            }
        })
        .collect())
}

fn pages(value: &Value, field: Option<&str>) -> Result<Vec<Value>> {
    let pages = value
        .as_array()
        .ok_or_else(|| error("CI paginationが不正です"))?;
    if pages.is_empty() || pages.len() >= MAX_PAGES {
        return Err(error("CI paginationを完全取得できません"));
    }
    let mut result = Vec::new();
    let mut total = None;
    for page in pages {
        let items = if let Some(field) = field {
            let count = int_value(page, "total_count")?;
            if count < 0 || total.is_some_and(|value| value != count) {
                return Err(error("CI一覧が取得中に変化しました"));
            }
            total = Some(count);
            get(page, field)?
        } else {
            page
        };
        let items = items.as_array().ok_or_else(|| error("CI一覧が不正です"))?;
        result.extend(items.iter().cloned());
        if result.len() > MAX_ITEMS {
            return Err(error("CI一覧の上限を超えています"));
        }
    }
    if total.is_some_and(|total| total as usize != result.len()) {
        return Err(error("CI一覧の全件取得を確認できません"));
    }
    Ok(result)
}

type Fetch<'a> = dyn FnMut(&str) -> Result<Value> + 'a;

fn snapshot_with(
    repository: &str,
    sha: &str,
    workflows: &[ExpectedWorkflow],
    fetch: &mut Fetch<'_>,
) -> Result<Snapshot> {
    let checks = pages(
        &fetch(&format!(
            "repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"
        ))?,
        Some("check_runs"),
    )?;
    let statuses = pages(
        &fetch(&format!(
            "repos/{repository}/commits/{sha}/statuses?per_page=100"
        ))?,
        None,
    )?;
    let runs = pages(
        &fetch(&format!(
            "repos/{repository}/actions/runs?head_sha={sha}&per_page=100"
        ))?,
        Some("workflow_runs"),
    )?;
    let mut entries = BTreeMap::new();
    let mut insert = |entry: Entry| -> Result<()> {
        if entry.id < 1
            || !text_valid(&entry.name, 512)
            || !text_valid(&entry.source, 128)
            || !(success(&entry.state)
                || waived_state(&entry.state)
                || entry.state == "action_required"
                || entry.state == "stale")
        {
            return Err(error("CI観測のidentity/stateが不正です"));
        }
        let key = (entry.kind.clone(), entry.name.clone(), entry.source.clone());
        if entries
            .get(&key)
            .is_none_or(|old: &Entry| old.id < entry.id)
        {
            entries.insert(key, entry);
        }
        Ok(())
    };
    for check in checks {
        if str_value(&check, "head_sha")? != sha {
            return Err(error("checkのheadが不一致です"));
        }
        let status = str_value(&check, "status")?;
        let state = if status == "completed" {
            str_value(&check, "conclusion")?
        } else {
            status
        };
        let app = int_value(get(&check, "app")?, "id")?;
        if app < 1 {
            return Err(error("check提供元が不明です"));
        }
        insert(Entry {
            kind: "check-run".into(),
            name: str_value(&check, "name")?,
            source: format!("app:{app}"),
            state,
            id: int_value(&check, "id")?,
        })?;
    }
    for status in statuses {
        let creator = int_value(get(&status, "creator")?, "id")?;
        if creator < 1 {
            return Err(error("status提供元が不明です"));
        }
        insert(Entry {
            kind: "commit-status".into(),
            name: str_value(&status, "context")?,
            source: format!("user:{creator}"),
            state: str_value(&status, "state")?,
            id: int_value(&status, "id")?,
        })?;
    }
    for run in runs {
        if str_value(&run, "head_sha")? != sha {
            return Err(error("workflow runのheadが不一致です"));
        }
        let status = str_value(&run, "status")?;
        let state = if status == "completed" {
            str_value(&run, "conclusion")?
        } else {
            status
        };
        let path = str_value(&run, "path")?;
        let path = path.split('@').next().unwrap_or("");
        if !workflow_yaml_path(path) {
            return Err(error("workflow runのpathが不正です"));
        }
        insert(Entry {
            kind: "workflow".into(),
            name: path.into(),
            source: format!("actions:{}", str_value(&run, "event")?),
            state,
            id: int_value(&run, "id")?,
        })?;
    }
    for workflow in workflows {
        if !entries.values().any(|entry| {
            entry.kind == "workflow"
                && entry.name == workflow.path
                && entry.source == workflow.source
        }) {
            entries.insert(
                (
                    "workflow".into(),
                    workflow.path.clone(),
                    workflow.source.clone(),
                ),
                Entry {
                    kind: "workflow".into(),
                    name: workflow.path.clone(),
                    source: workflow.source.clone(),
                    state: "missing".into(),
                    id: 0,
                },
            );
        }
    }
    if entries.is_empty() {
        entries.insert(
            ("ci".into(), "no-observations".into(), "github".into()),
            Entry {
                kind: "ci".into(),
                name: "no-observations".into(),
                source: "github".into(),
                state: "missing".into(),
                id: 0,
            },
        );
    }
    Ok(Snapshot {
        sha: sha.into(),
        entries: entries.into_values().collect(),
    })
}

fn snapshot(
    root: &Path,
    repository: &str,
    sha: &str,
    workflows: &[ExpectedWorkflow],
) -> Result<Snapshot> {
    snapshot_with(repository, sha, workflows, &mut |endpoint| {
        gh_json(
            root,
            &[
                "api".into(),
                endpoint.into(),
                "--paginate".into(),
                "--slurp".into(),
            ],
        )
    })
}

pub(super) fn prepare(root: &Path, payload: &Value, path: &Path, live: &str) -> Result<Value> {
    if !path.is_absolute() {
        return Err(error(
            "緊急承認requestはprivateな絶対pathで指定してください",
        ));
    }
    let value = read_json_file(path, "緊急承認request")?;
    let request = request(&value, payload)?;
    if request.base_sha != live {
        return Err(error("緊急承認baseがlive mainと一致しません"));
    }
    let expected = workflow_expectations(root, &request.base_sha, &request.head_sha, false)?;
    let observed = snapshot(root, &request.repository, &request.head_sha, &expected)?;
    coverage(&request, &observed, &request.head_sha)?;
    Ok(object([
        ("request", value),
        (
            "observed_ci",
            serde_json::to_value(observed).map_err(|_| error("CI観測を保存できません"))?,
        ),
    ]))
}

pub(super) fn merge_status(status: &str, draft: bool, allow_draft: bool, emergency: bool) -> bool {
    status == "CLEAN"
        || (draft && allow_draft && status == "DRAFT")
        || (emergency && status == "UNSTABLE")
}

fn audit_path(receipt: &Value) -> Result<PathBuf> {
    Ok(receipt_path(
        &str_value(receipt, "repository")?,
        &str_value(receipt, "task_id")?,
        &str_value(receipt, "head_sha")?,
    )?
    .with_extension("emergency.json"))
}

pub(super) fn digest(receipt: &Value) -> Result<String> {
    Ok(sha256(
        serde_json::to_vec(receipt).map_err(|_| error("receipt digestを計算できません"))?,
    ))
}

pub(super) fn replacement_scope(existing: &Value, proposed: &Value) -> Result<()> {
    for key in [
        "task_id",
        "repository",
        "pr",
        "head_sha",
        "plan_id",
        "plan_version",
        "changed_files",
        "tests_passed",
        "actionable",
    ] {
        if existing.get(key) != proposed.get(key) {
            return Err(error("receipt移行でも対象・Plan・検証範囲は変更できません"));
        }
    }
    Ok(())
}

fn verify_merge(root: &Path, request: &Request, merge: &str) -> Result<()> {
    let parents = git(root, &["rev-list", "--parents", "-n", "1", merge], true)?;
    if parents.split_whitespace().collect::<Vec<_>>()
        != [merge, &request.base_sha, &request.head_sha]
    {
        return Err(error(
            "実mergeの親が承認済みbase/headと一致しません。別baseへCI免除を拡張しません",
        ));
    }
    let merge_tree = git(root, &["rev-parse", &format!("{merge}^{{tree}}")], true)?;
    let head_tree = git(
        root,
        &["rev-parse", &format!("{}^{{tree}}", request.head_sha)],
        true,
    )?;
    if merge_tree != head_tree {
        return Err(error("実mergeのtreeがlocal検証済みheadと異なります"));
    }
    Ok(())
}

pub(super) fn observe(root: &Path, receipt: &Value, view: &Value, live: &str) -> Result<Value> {
    validate_receipt(receipt)?;
    let request = request(get(get(receipt, "emergency")?, "request")?, receipt)?;
    let merged = str_value(view, "state")? == "MERGED";
    let target = if merged {
        let merge = oid(&str_value(get(view, "mergeCommit")?, "oid")?, "実merge")?;
        verify_merge(root, &request, &merge)?;
        if !git_ancestor(root, &merge, live)? {
            return Err(error("実mergeがlive mainへ到達していません"));
        }
        merge
    } else {
        if request.base_sha != live {
            return Err(error("base更新で緊急承認が失効しました"));
        }
        request.head_sha.clone()
    };
    let expected = workflow_expectations(root, &request.base_sha, &request.head_sha, false)?;
    let head = snapshot(root, &request.repository, &request.head_sha, &expected)?;
    coverage(&request, &head, &request.head_sha)?;
    let actual = if merged {
        snapshot(
            root,
            &request.repository,
            &target,
            &workflow_expectations(root, &request.base_sha, &request.head_sha, true)?,
        )?
    } else {
        head.clone()
    };
    coverage(&request, &actual, &target)?;
    // 並行して統合された別変更のCIは、今回の免除対象にしない。
    if merged && live != target {
        check_required_ci(root, &request.repository, live)?;
    }
    let result = object([
        ("approved", Value::Bool(true)),
        ("receipt_sha256", string(digest(receipt)?)),
        ("head", string(&request.head_sha)),
        ("base", string(live)),
        ("approved_base", string(&request.base_sha)),
        ("target", string(&target)),
        ("merged", Value::Bool(merged)),
        (
            "head_ci",
            serde_json::to_value(head).map_err(|_| error("CI観測を保存できません"))?,
        ),
        (
            "target_ci",
            serde_json::to_value(actual).map_err(|_| error("CI観測を保存できません"))?,
        ),
    ]);
    atomic_json(&audit_path(receipt)?, &result)?;
    Ok(result)
}

pub(super) fn git_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = run(
        &git_command(&[
            "merge-base".into(),
            "--is-ancestor".into(),
            ancestor.into(),
            descendant.into(),
        ])?,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(error("Git ancestryを確認できません")),
    }
}

pub(super) fn read_audit(receipt: &Value) -> Result<Value> {
    let value = read_json_file(&audit_path(receipt)?, "緊急CI観測")?;
    if str_value(&value, "receipt_sha256")? != digest(receipt)? {
        return Err(error("緊急CI観測の承認が不一致です"));
    }
    Ok(value)
}

pub(super) fn note(receipt: &Value) -> Result<String> {
    let request = request(get(get(receipt, "emergency")?, "request")?, receipt)?;
    Ok(format!(
        "<!-- codex-emergency-ci:{}:{} -->\n委任されたCI判断の範囲で、基盤障害の根拠・固定headのlocal検証・独立reviewを記録してCI待機を免除します。\n\n{}\n\nCIの実測値は変更していません。workflowと既存CDは既存triggerに従い起動し得ます。GitHubの保護要件とCI以外の検証は維持します。",
        request.task_id, request.head_sha, request.public_summary
    ))
}

pub(super) fn publish_note(root: &Path, receipt: &Value) -> Result<()> {
    let body = note(receipt)?;
    let repo = str_value(receipt, "repository")?;
    let pr = int_value(receipt, "pr")?;
    let endpoint = format!("repos/{repo}/issues/{pr}/comments");
    let issue_url = format!("https://api.github.com/repos/{repo}/issues/{pr}");
    let login = str_value(&work_log::api(root, "GET", "user", None)?, "login")?;
    let marker = body.lines().next().unwrap_or("");
    let record_path = audit_path(receipt)?.with_extension("note.json");
    let previous = if record_path.exists() {
        Some(read_json_file(&record_path, "緊急CI公開記録")?)
    } else {
        None
    };
    let comments = pages(
        &gh_json(
            root,
            &[
                "api".into(),
                format!("{endpoint}?per_page=100"),
                "--paginate".into(),
                "--slurp".into(),
            ],
        )?,
        None,
    )?;
    let mut found = None;
    for comment in comments {
        if str_value(&comment, "body")?.starts_with(marker) {
            work_log::validate_comment(&comment, &issue_url, &login)?;
            if found.is_some() {
                return Err(error("緊急CI記録の所有者または一意性を確認できません"));
            }
            found = Some(comment);
        }
    }
    let id = match found {
        Some(comment) => {
            if str_value(&comment, "body")? != body {
                if previous.as_ref().is_none_or(|old| {
                    old.get("id") != comment.get("id") || old.get("body") != comment.get("body")
                }) {
                    return Err(error("既存の緊急CI公開記録に所有不明の変更があります"));
                }
                work_log::api(
                    root,
                    "PATCH",
                    &format!(
                        "repos/{repo}/issues/comments/{}",
                        int_value(&comment, "id")?
                    ),
                    Some(&body),
                )?;
            }
            int_value(&comment, "id")?
        }
        None => work_log::validate_comment(
            &work_log::api(root, "POST", &endpoint, Some(&body))?,
            &issue_url,
            &login,
        )?,
    };
    let saved = work_log::api(
        root,
        "GET",
        &format!("repos/{repo}/issues/comments/{id}"),
        None,
    )?;
    if work_log::validate_comment(&saved, &issue_url, &login)? != id
        || str_value(&saved, "body")? != body
    {
        return Err(error("緊急CI公開記録の保存を確認できません"));
    }
    atomic_json(
        &record_path,
        &object([("id", Value::from(id)), ("body", string(body))]),
    )?;
    Ok(())
}
