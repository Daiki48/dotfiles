//! タスク調整用にGitHub Actionsとcommit statusを読み取り専用で観測する。
//!
//! APIを利用できない場合にCI成功とみなすことはない。

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::codex_tools::delivery::task_gh_json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CiState {
    Pending,
    Success,
    Failure,
    Missing,
    Stale,
    Cancelled,
    TimedOut,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Observation {
    pub state: CiState,
    pub head: String,
    pub base: String,
    pub target: String,
    pub fingerprint: String,
    pub merge_commit: Option<String>,
    pub merged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredCheck {
    context: String,
    integration_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckRun {
    id: i64,
    name: String,
    head_sha: String,
    status: String,
    conclusion: Option<String>,
    app_id: Option<i64>,
    run_id: Option<i64>,
    run_attempt: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitStatus {
    id: i64,
    context: String,
    state: String,
    sha: String,
    updated_at: String,
}

#[derive(Debug, Clone)]
struct Snapshot {
    runs: Vec<CheckRun>,
    statuses: Vec<CommitStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PullRequest {
    head: String,
    base: String,
    base_ref: String,
    merge_commit: Option<String>,
    merged: bool,
}

/// 現在のPR、または`main`指定時はそのcommitを観測する。
///
/// API・認証・rate limitの失敗はエラーとして返し、`Missing`や成功には置き換えない。
pub(super) fn observe(
    root: &Path,
    repository: &str,
    pr: u64,
    main: Option<&str>,
) -> Result<Observation, String> {
    validate_repository(repository)?;
    if pr == 0 {
        return Err("PR番号は1以上である必要があります".into());
    }

    if let Some(sha) = main {
        validate_sha(sha)?;
    }

    let before = fetch_pull_request(root, repository, pr)?;

    let rules = required_checks(root, repository, &before.base_ref)?;
    let (target, snapshot) = if let Some(sha) = main {
        let target = sha.to_ascii_lowercase();
        let snapshot = fetch_snapshot(root, repository, &target)?;
        (target, snapshot)
    } else {
        let head_snapshot = fetch_snapshot(root, repository, &before.head)?;
        let merge_snapshot = before
            .merge_commit
            .as_deref()
            .filter(|candidate| *candidate != before.head)
            .map(|candidate| fetch_snapshot(root, repository, candidate))
            .transpose()?;
        select_target(
            &before.head,
            before.merge_commit.as_deref(),
            head_snapshot,
            merge_snapshot,
        )
    };

    // rules/check/statusを個別に照会した後、PRを再取得する。
    // 観測中にGitHub上のPRが変わっていたら、このsnapshotを信頼しない。
    let after = fetch_pull_request(root, repository, pr)?;
    let changed = before != after;
    let main_sha_mismatch = main.is_some_and(|sha| {
        after.merge_commit.as_deref() != Some(sha.to_ascii_lowercase().as_str())
    });
    let state = if changed || main_sha_mismatch {
        CiState::Stale
    } else {
        classify(&rules, &snapshot.runs, &snapshot.statuses, &target)
    };
    let fingerprint = snapshot_fingerprint(&after, &target, state == CiState::Stale, &snapshot);
    Ok(Observation {
        state,
        head: after.head,
        base: after.base,
        target,
        fingerprint,
        merge_commit: after.merge_commit,
        merged: after.merged,
    })
}

fn fetch_pull_request(root: &Path, repository: &str, pr: u64) -> Result<PullRequest, String> {
    let value = task_gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/pulls/{pr}"),
            "--method".into(),
            "GET".into(),
        ],
    )?;
    Ok(PullRequest {
        head: nested_sha(&value, &["head", "sha"], "PR head")?,
        base: nested_sha(&value, &["base", "sha"], "PR base")?,
        base_ref: nested_text(&value, &["base", "ref"], "PR base branch")?,
        merge_commit: optional_sha(value.get("merge_commit_sha"), "merge commit")?,
        merged: value
            .get("merged")
            .and_then(Value::as_bool)
            .ok_or_else(|| "GitHub PRのmerged状態を確認できません".to_string())?,
    })
}

fn select_target(
    head: &str,
    merge_commit: Option<&str>,
    head_snapshot: Snapshot,
    merge_snapshot: Option<Snapshot>,
) -> (String, Snapshot) {
    if let (Some(candidate), Some(snapshot)) = (merge_commit, merge_snapshot)
        && candidate != head
        && contains_any_status(&snapshot)
    {
        return (candidate.to_owned(), snapshot);
    }
    (head.to_owned(), head_snapshot)
}

fn validate_repository(repository: &str) -> Result<(), String> {
    let Some((owner, name)) = repository.split_once('/') else {
        return Err("GitHub repository名が不正です".into());
    };
    if owner.is_empty()
        || name.is_empty()
        || owner == "."
        || owner == ".."
        || name == "."
        || name == ".."
        || name.contains('/')
        || !repository
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./".contains(&byte))
    {
        return Err("GitHub repository名が不正です".into());
    }
    Ok(())
}

fn validate_sha(value: &str) -> Result<(), String> {
    if value.len() < 40 || value.len() > 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("対象commit SHAが不正です".into());
    }
    Ok(())
}

fn nested_sha(value: &Value, path: &[&str], label: &str) -> Result<String, String> {
    let mut current = value;
    for part in path {
        current = current
            .get(*part)
            .ok_or_else(|| format!("{label}を確認できません"))?;
    }
    let sha = current
        .as_str()
        .ok_or_else(|| format!("{label}を確認できません"))?;
    validate_sha(sha).map_err(|_| format!("{label}を確認できません"))?;
    Ok(sha.to_ascii_lowercase())
}

fn nested_text(value: &Value, path: &[&str], label: &str) -> Result<String, String> {
    let mut current = value;
    for part in path {
        current = current
            .get(*part)
            .ok_or_else(|| format!("{label}を確認できません"))?;
    }
    let text = current
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= 256)
        .ok_or_else(|| format!("{label}を確認できません"))?;
    Ok(text.to_owned())
}

fn optional_sha(value: Option<&Value>, label: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let sha = value
        .as_str()
        .ok_or_else(|| format!("{label}を確認できません"))?;
    validate_sha(sha).map_err(|_| format!("{label}を確認できません"))?;
    Ok(Some(sha.to_ascii_lowercase()))
}

fn encoded_path_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn required_checks(
    root: &Path,
    repository: &str,
    branch: &str,
) -> Result<Vec<RequiredCheck>, String> {
    if branch.is_empty() || branch.len() > 256 || branch.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err("対象branchを安全に確認できません".into());
    }
    let endpoint = format!(
        "repos/{repository}/rules/branches/{}",
        encoded_path_component(branch)
    );
    let value = task_gh_json(
        root,
        &["api".into(), endpoint, "--method".into(), "GET".into()],
    )?;
    parse_required_rules(&value)
}

fn parse_required_rules(value: &Value) -> Result<Vec<RequiredCheck>, String> {
    let rules = value
        .as_array()
        .ok_or_else(|| "branch rulesの応答形式が不正です".to_string())?;
    if rules.len() > 256 {
        return Err("branch rulesの件数が上限を超えています".into());
    }
    let mut required = Vec::new();
    for rule in rules {
        if rule.get("type").and_then(Value::as_str) != Some("required_status_checks") {
            continue;
        }
        let checks = rule
            .get("parameters")
            .and_then(|parameters| parameters.get("required_status_checks"))
            .and_then(Value::as_array)
            .ok_or_else(|| "required checksの設定を確認できません".to_string())?;
        if checks.len() > 256 {
            return Err("required checksの件数が上限を超えています".into());
        }
        for check in checks {
            let context = check
                .get("context")
                .and_then(Value::as_str)
                .filter(|context| !context.is_empty() && context.len() <= 256)
                .ok_or_else(|| "required check名を確認できません".to_string())?;
            let integration_id = check
                .get("integration_id")
                .or_else(|| check.get("app_id"))
                .filter(|id| !id.is_null())
                .map(|id| {
                    id.as_i64()
                        .filter(|number| *number > 0)
                        .ok_or_else(|| "required check提供元を確認できません".to_string())
                })
                .transpose()?;
            required.push(RequiredCheck {
                context: context.to_owned(),
                integration_id,
            });
        }
    }
    required.sort_by(|left, right| {
        left.context
            .cmp(&right.context)
            .then(left.integration_id.cmp(&right.integration_id))
    });
    required.dedup();
    Ok(required)
}

fn fetch_snapshot(root: &Path, repository: &str, sha: &str) -> Result<Snapshot, String> {
    validate_sha(sha)?;
    let checks = task_gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
            "--method".into(),
            "GET".into(),
        ],
    )?;
    let runs = parse_check_runs(&checks, sha)?;
    let statuses = task_gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/commits/{sha}/statuses?per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
            "--method".into(),
            "GET".into(),
        ],
    )?;
    let statuses = parse_statuses(&statuses, sha)?;
    Ok(Snapshot { runs, statuses })
}

fn pages<'a>(value: &'a Value, key: &str) -> Result<Vec<&'a Value>, String> {
    let pages = value
        .as_array()
        .ok_or_else(|| format!("{key}応答のページ形式が不正です"))?;
    // 上限に達した場合、最終ページが満杯だと次ページの有無を証明できない。
    // 不完全な結果を現状の全件として扱わず、安全側に倒す。
    if pages.len() >= 20 {
        return Err(format!("{key}応答のページ数が安全上限に達しました"));
    }
    Ok(pages.iter().collect())
}

fn parse_check_runs(value: &Value, sha: &str) -> Result<Vec<CheckRun>, String> {
    let mut parsed = Vec::new();
    let mut total = 0usize;
    let mut declared_total = None;
    for page in pages(value, "check-runs")? {
        let runs = page
            .get("check_runs")
            .and_then(Value::as_array)
            .ok_or_else(|| "check-runsの応答形式が不正です".to_string())?;
        let page_total = page
            .get("total_count")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "check-runs総数を確認できません".to_string())?;
        if declared_total.is_some_and(|previous| previous != page_total) {
            return Err("check-runs取得中に総数が変化しました".into());
        }
        declared_total = Some(page_total);
        total = total.saturating_add(runs.len());
        if total > 10_000 {
            return Err("check-runsの件数が上限を超えています".into());
        }
        for run in runs {
            let id = run
                .get("id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| "check-run IDを確認できません".to_string())?;
            let head_sha = run
                .get("head_sha")
                .and_then(Value::as_str)
                .ok_or_else(|| "check-run SHAを確認できません".to_string())?;
            validate_sha(head_sha).map_err(|_| "check-run SHAを確認できません".to_string())?;
            let name = run
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 256)
                .ok_or_else(|| "check-run名を確認できません".to_string())?;
            let status = run
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| "check-run状態を確認できません".to_string())?;
            let conclusion = run
                .get("conclusion")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let app_id = run
                .get("app")
                .and_then(|app| app.get("id"))
                .and_then(Value::as_i64);
            let run_id = run
                .get("check_suite")
                .and_then(|suite| suite.get("id"))
                .and_then(Value::as_i64);
            let run_attempt = run.get("run_attempt").and_then(Value::as_i64);
            parsed.push(CheckRun {
                id,
                name: name.to_owned(),
                head_sha: head_sha.to_ascii_lowercase(),
                status: status.to_owned(),
                conclusion,
                app_id,
                run_id,
                run_attempt,
            });
        }
    }
    // endpointは指定commitに限定されるが、不正または古い応答で通過しないよう
    // SHAが完全一致するrunだけを残す。
    parsed.retain(|run| run.head_sha == sha);
    if declared_total.is_some_and(|expected| total != expected) {
        return Err("check-runsのページ取得が不完全です".into());
    }
    Ok(parsed)
}

fn parse_statuses(value: &Value, sha: &str) -> Result<Vec<CommitStatus>, String> {
    validate_sha(sha)?;
    let sha = sha.to_ascii_lowercase();
    let mut parsed = Vec::new();
    let mut total = 0usize;
    for page in pages(value, "commit status")? {
        let statuses = page
            .as_array()
            .ok_or_else(|| "commit statusの応答形式が不正です".to_string())?;
        total = total.saturating_add(statuses.len());
        if total > 10_000 {
            return Err("commit statusの件数が上限を超えています".into());
        }
        for status in statuses {
            let id = status
                .get("id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| "commit status IDを確認できません".to_string())?;
            let context = status
                .get("context")
                .and_then(Value::as_str)
                .filter(|context| !context.is_empty() && context.len() <= 256)
                .ok_or_else(|| "commit status名を確認できません".to_string())?;
            let state = status
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| "commit status状態を確認できません".to_string())?;
            let updated_at = status
                .get("updated_at")
                .or_else(|| status.get("created_at"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 64)
                .ok_or_else(|| "commit status更新時刻を確認できません".to_string())?;
            parsed.push(CommitStatus {
                id,
                context: context.to_owned(),
                state: state.to_owned(),
                // list endpointのpathでstatusはすべてこのrefに限定される。
                // 各status objectには`sha`が含まれない。
                sha: sha.clone(),
                updated_at: updated_at.to_owned(),
            });
        }
    }
    Ok(parsed)
}

fn contains_any_status(snapshot: &Snapshot) -> bool {
    !snapshot.runs.is_empty() || !snapshot.statuses.is_empty()
}

fn classify(
    required: &[RequiredCheck],
    runs: &[CheckRun],
    statuses: &[CommitStatus],
    target: &str,
) -> CiState {
    let current_runs: Vec<_> = runs.iter().filter(|run| run.head_sha == target).collect();
    let current_statuses: Vec<_> = statuses
        .iter()
        .filter(|status| status.sha == target)
        .collect();
    if current_runs.is_empty() && current_statuses.is_empty() {
        return CiState::Missing;
    }

    let mut saw_pending = false;
    let mut saw_failure = false;
    let mut saw_cancelled = false;
    let mut saw_timeout = false;
    let mut saw_stale = false;
    let mut saw_skipped = false;
    let mut saw_missing = false;
    let mut saw_success = false;

    for requirement in required {
        let matching_runs: Vec<_> = current_runs
            .iter()
            .filter(|run| {
                run.name == requirement.context
                    && requirement
                        .integration_id
                        .is_none_or(|integration| run.app_id == Some(integration))
            })
            .collect();
        let mut matching_statuses: Vec<_> = statuses
            .iter()
            .filter(|status| status.context == requirement.context && status.sha == target)
            .collect();
        matching_statuses.sort_by_key(|status| std::cmp::Reverse(status.id));
        matching_statuses.truncate(1);
        let has_required_source = !matching_runs.is_empty()
            || (requirement.integration_id.is_none() && !matching_statuses.is_empty());
        if !has_required_source {
            // 同じcontextでも、別appのcheckでapp指定ルールを満たしてはならない。
            saw_missing = true;
            continue;
        }
    }

    // check runはcommit SHAで絞られる。完了したcheckのconclusionだけが確定値で、
    // 実行中や古いconclusionを成功判定に使わない。
    for run in current_runs {
        if run.status != "completed" {
            saw_pending = true;
            continue;
        }
        match run.conclusion.as_deref() {
            Some("success") => saw_success = true,
            Some("skipped" | "neutral") => saw_skipped = true,
            Some("failure" | "action_required" | "startup_failure") => saw_failure = true,
            Some("cancelled") => saw_cancelled = true,
            Some("timed_out") => saw_timeout = true,
            Some("stale") => saw_stale = true,
            Some(_) | None => saw_failure = true,
        }
    }

    // commit status endpointはcontextごとに最新のstatusを先頭へ返す。
    // 全ページからcontextごとの最大IDを採用する。
    let mut latest_statuses = current_statuses;
    latest_statuses.sort_by(|left, right| {
        left.context
            .cmp(&right.context)
            .then(right.id.cmp(&left.id))
    });
    latest_statuses.dedup_by(|left, right| left.context == right.context);
    for status in latest_statuses {
        match status.state.as_str() {
            "success" => saw_success = true,
            "pending" | "expected" => saw_pending = true,
            "failure" | "error" => saw_failure = true,
            _ => saw_failure = true,
        }
    }

    if saw_failure {
        CiState::Failure
    } else if saw_cancelled {
        CiState::Cancelled
    } else if saw_timeout {
        CiState::TimedOut
    } else if saw_pending {
        CiState::Pending
    } else if saw_missing {
        CiState::Missing
    } else if saw_stale {
        CiState::Stale
    } else if saw_success {
        // GitHub上ではskipped/neutralも完了扱いだが、同じsnapshot内で
        // 実際に検証が成功したことを確認してから通過させる。
        CiState::Success
    } else if saw_skipped {
        CiState::Skipped
    } else {
        CiState::Missing
    }
}

fn snapshot_fingerprint(
    pr: &PullRequest,
    target: &str,
    stale: bool,
    snapshot: &Snapshot,
) -> String {
    let mut runs = snapshot.runs.clone();
    runs.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.app_id.cmp(&right.app_id))
            .then(left.id.cmp(&right.id))
    });
    let mut statuses = snapshot.statuses.clone();
    statuses.sort_by(|left, right| {
        left.context
            .cmp(&right.context)
            .then(left.updated_at.cmp(&right.updated_at))
            .then(left.id.cmp(&right.id))
    });
    let value = json!({
        "head": pr.head,
        "base": pr.base,
        "target": target,
        "merge_commit": pr.merge_commit,
        "merged": pr.merged,
        "base_ref": pr.base_ref,
        "stale": stale,
        "check_runs": runs.iter().map(|run| json!({
            "id": run.id,
            "name": run.name,
            "head_sha": run.head_sha,
            "status": run.status,
            "conclusion": run.conclusion,
            "app_id": run.app_id,
            "run_id": run.run_id,
            "run_attempt": run.run_attempt,
        })).collect::<Vec<_>>(),
        "statuses": statuses.iter().map(|status| json!({
            "id": status.id,
            "context": status.context,
            "state": status.state,
            "sha": status.sha,
            "updated_at": status.updated_at,
        })).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&value).expect("JSON value is always serializable");
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = concat!("aaaaaaaaaa", "aaaaaaaaaa", "aaaaaaaaaa", "aaaaaaaaaa");
    const MERGE: &str = concat!("bbbbbbbbbb", "bbbbbbbbbb", "bbbbbbbbbb", "bbbbbbbbbb");
    const STALE: &str = concat!("cccccccccc", "cccccccccc", "cccccccccc", "cccccccccc");

    fn required(context: &str, integration_id: Option<i64>) -> RequiredCheck {
        RequiredCheck {
            context: context.into(),
            integration_id,
        }
    }

    fn run(name: &str, sha: &str, app: i64, status: &str, conclusion: Option<&str>) -> CheckRun {
        CheckRun {
            id: 1,
            name: name.into(),
            head_sha: sha.into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_owned),
            app_id: Some(app),
            run_id: Some(7),
            run_attempt: Some(1),
        }
    }

    fn status(context: &str, sha: &str, state: &str) -> CommitStatus {
        CommitStatus {
            id: 1,
            context: context.into(),
            state: state.into(),
            sha: sha.into(),
            updated_at: "2026-11-04T00:00:00Z".into(),
        }
    }

    #[test]
    fn classifies_only_the_current_sha() {
        let required = [required("ci", Some(15368))];
        let runs = [run("ci", STALE, 15368, "completed", Some("success"))];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Missing);
    }

    #[test]
    fn a_matching_commit_status_failure_is_not_hidden_by_a_successful_check() {
        let required = [required("ci", Some(15368))];
        let runs = [run("ci", HEAD, 15368, "completed", Some("success"))];
        assert_eq!(
            classify(&required, &runs, &[status("ci", HEAD, "failure")], HEAD),
            CiState::Failure
        );
    }

    #[test]
    fn distinguishes_missing_pending_failure_cancelled_and_timeout() {
        let required = [required("ci", Some(15368))];
        assert_eq!(classify(&required, &[], &[], HEAD), CiState::Missing);
        assert_eq!(
            classify(
                &required,
                &[run("ci", HEAD, 15368, "in_progress", None)],
                &[],
                HEAD
            ),
            CiState::Pending
        );
        assert_eq!(
            classify(
                &required,
                &[run("ci", HEAD, 15368, "completed", Some("failure"))],
                &[],
                HEAD
            ),
            CiState::Failure
        );
        assert_eq!(
            classify(
                &required,
                &[run("ci", HEAD, 15368, "completed", Some("cancelled"))],
                &[],
                HEAD
            ),
            CiState::Cancelled
        );
        assert_eq!(
            classify(
                &required,
                &[run("ci", HEAD, 15368, "completed", Some("timed_out"))],
                &[],
                HEAD
            ),
            CiState::TimedOut
        );
    }

    #[test]
    fn an_empty_required_set_is_not_success() {
        assert_eq!(classify(&[], &[], &[], HEAD), CiState::Missing);
        assert_eq!(
            classify(
                &[],
                &[run("ci", HEAD, 15368, "completed", Some("success"))],
                &[],
                HEAD,
            ),
            CiState::Success
        );
    }

    #[test]
    fn optional_actions_must_also_finish_without_failure() {
        let required = [required("build", Some(15368))];
        let runs = [
            run("build", HEAD, 15368, "completed", Some("success")),
            run("docs", HEAD, 15368, "completed", Some("failure")),
        ];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Failure);

        let runs = [
            run("build", HEAD, 15368, "completed", Some("success")),
            run("docs", HEAD, 15368, "in_progress", None),
        ];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Pending);
    }

    #[test]
    fn only_completed_check_runs_can_contribute_a_success_conclusion() {
        let required = [required("ci", Some(15368))];
        let run = run("ci", HEAD, 15368, "in_progress", Some("success"));
        assert_eq!(classify(&required, &[run], &[], HEAD), CiState::Pending);
    }

    #[test]
    fn every_required_context_must_be_observed() {
        let required = [
            required("build", Some(15368)),
            required("lint", Some(15368)),
        ];
        let runs = [run("build", HEAD, 15368, "completed", Some("success"))];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Missing);
    }

    #[test]
    fn skipped_only_is_not_reported_as_success() {
        let required = [required("ci", None)];
        let runs = [run("ci", HEAD, 15368, "completed", Some("skipped"))];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Skipped);
    }

    #[test]
    fn a_successful_check_allows_other_checks_to_be_skipped() {
        let required = [required("build", None), required("docs", None)];
        let runs = [
            run("build", HEAD, 15368, "completed", Some("success")),
            run("docs", HEAD, 15368, "completed", Some("skipped")),
        ];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Success);
    }

    #[test]
    fn only_the_highest_id_commit_status_for_a_context_is_used() {
        let required = [required("ci", None)];
        let statuses = [
            CommitStatus {
                id: 1,
                context: "ci".into(),
                state: "failure".into(),
                sha: HEAD.into(),
                updated_at: "2026-11-03T00:00:00Z".into(),
            },
            CommitStatus {
                id: 2,
                context: "ci".into(),
                state: "success".into(),
                sha: HEAD.into(),
                // timestampが前後してもAPI順とIDから最新statusを選ぶ。
                updated_at: "2026-11-02T00:00:00Z".into(),
            },
        ];
        assert_eq!(classify(&required, &[], &statuses, HEAD), CiState::Success);
    }

    #[test]
    fn a_required_check_must_come_from_the_expected_app() {
        let required = [required("ci", Some(15368))];
        let runs = [run("ci", HEAD, 999, "completed", Some("success"))];
        assert_eq!(classify(&required, &runs, &[], HEAD), CiState::Missing);
    }

    #[test]
    fn test_merge_sha_is_used_when_it_has_checks() {
        let merge = run("ci", MERGE, 15368, "completed", Some("success"));
        let (selected, selected_snapshot) = select_target(
            HEAD,
            Some(MERGE),
            Snapshot {
                runs: vec![],
                statuses: vec![],
            },
            Some(Snapshot {
                runs: vec![merge],
                statuses: vec![],
            }),
        );
        assert_eq!(selected, MERGE);
        assert_eq!(
            classify(
                &[required("ci", Some(15368))],
                &selected_snapshot.runs,
                &selected_snapshot.statuses,
                &selected,
            ),
            CiState::Success
        );
    }

    #[test]
    fn falls_back_to_head_when_test_merge_has_no_checks() {
        let (selected, _) = select_target(
            HEAD,
            Some(MERGE),
            Snapshot {
                runs: vec![],
                statuses: vec![],
            },
            Some(Snapshot {
                runs: vec![],
                statuses: vec![],
            }),
        );
        assert_eq!(selected, HEAD);
    }

    #[test]
    fn reruns_change_the_event_fingerprint() {
        let before = Snapshot {
            runs: vec![run("ci", HEAD, 15368, "completed", Some("failure"))],
            statuses: vec![],
        };
        let mut after = before.clone();
        after.runs[0].id = 2;
        after.runs[0].run_attempt = Some(2);
        let pr = PullRequest {
            head: HEAD.into(),
            base: STALE.into(),
            base_ref: "main".into(),
            merge_commit: None,
            merged: false,
        };
        assert_ne!(
            snapshot_fingerprint(&pr, HEAD, false, &before),
            snapshot_fingerprint(&pr, HEAD, false, &after)
        );
    }

    #[test]
    fn parser_rejects_wrong_sha_and_malformed_provider() {
        let value = json!([{
            "total_count": 1,
            "check_runs": [{
                "id": 1,
                "name": "ci",
                "head_sha": STALE,
                "status": "completed",
                "conclusion": "success",
                "app": {"id": 15368},
                "check_suite": {"id": 4},
                "run_attempt": 1
            }]
        }]);
        assert!(parse_check_runs(&value, HEAD).unwrap().is_empty());

        let rules = json!([{
            "type": "required_status_checks",
            "parameters": {"required_status_checks": [{"context": "ci", "integration_id": -1}]}
        }]);
        assert!(parse_required_rules(&rules).is_err());
    }

    #[test]
    fn parses_commit_statuses_without_sha_from_the_ref_scoped_endpoint() {
        let value = json!([[
            {
                "id": 2,
                "state": "success",
                "context": "ci/build",
                "created_at": "2026-11-04T00:00:00Z",
                "updated_at": "2026-11-04T00:00:00Z"
            },
            {
                "id": 1,
                "state": "failure",
                "context": "ci/build",
                "created_at": "2026-11-03T00:00:00Z",
                "updated_at": "2026-11-03T00:00:00Z"
            }
        ]]);
        let statuses = parse_statuses(&value, HEAD).unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(statuses.iter().all(|status| status.sha == HEAD));
        assert_eq!(
            classify(&[required("ci/build", None)], &[], &statuses, HEAD),
            CiState::Success
        );
    }

    #[test]
    fn path_components_are_encoded_and_repository_names_are_validated() {
        assert_eq!(encoded_path_component("feature/test"), "feature%2Ftest");
        assert_eq!(encoded_path_component("x?query"), "x%3Fquery");
        assert!(validate_repository("owner/repo").is_ok());
        assert!(validate_repository("owner/repo/other").is_err());
        assert!(validate_repository("owner/repo?state=open").is_err());
    }
}
