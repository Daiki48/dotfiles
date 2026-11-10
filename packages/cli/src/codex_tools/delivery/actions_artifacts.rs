//! 明示登録されたGitHub Actions artifactの棚卸しと安全な容量整理。
//!
//! Artifact APIはrun間の参照グラフを提供しない。そのため自動整理対象は、
//! 管理者が参照なし・再生成可能と登録した通常CI用途だけに限定する。
//! 判定不能/API失敗は保持し、workflow runやcacheは削除しない。
use super::*;
use serde::{Deserialize, Serialize};

const POLICY_VERSION: i64 = 1;
const DEFAULT_RETENTION_DAYS: i64 = 7;
const MAX_RETENTION_DAYS: i64 = 400;
const MAX_PAGES: usize = 100;
const MAX_ARTIFACTS: usize = 10_000;
const LEDGER_VERSION: i64 = 1;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: i64,
    repositories: Vec<RepositoryPolicy>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RepositoryPolicy {
    repository: String,
    #[serde(default)]
    retention_days: Option<i64>,
    usages: Vec<Usage>,
    clearances: Vec<Clearance>,
    recovery_jobs: Vec<RecoveryJob>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Usage {
    name_prefix: String,
    purpose: String,
    #[serde(default)]
    retention_days: Option<i64>,
}

/// artifact/run/headを固定する短時間のprivate clearance。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Clearance {
    artifact_id: i64,
    run_id: i64,
    head_sha: String,
    references: Vec<String>,
    regenerable: bool,
    sole_evidence: bool,
    /// private確認日時。cleanupでは5分以内だけ有効。
    verified_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RecoveryJob {
    workflow_path: String,
    job_name: String,
    ci_only: bool,
    deployment_free: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FailureEvidence {
    version: i64,
    repository: String,
    task_id: String,
    pr: i64,
    head_sha: String,
    base_sha: String,
    plan_id: String,
    plan_version: i64,
    run_id: i64,
    job_id: i64,
    cause: String,
    product_test_failures: bool,
    observed_at: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryEvidence {
    version: i64,
    repository: String,
    task_id: String,
    pr: i64,
    head_sha: String,
    base_sha: String,
    plan_id: String,
    plan_version: i64,
    run_id: i64,
    job_id: i64,
    storage_recovered: bool,
    source: String,
    observed_at: String,
    product_test_failures: bool,
    summary: String,
}

// GitHub APIはartifact参照edgeを返さない。clearance発行時にworkflow参照と外部consumerを確認し、5分以内に整理する。

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct Artifact {
    id: i64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    created_at: String,
    expires_at: String,
    workflow_run: RunRef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct RunRef {
    id: i64,
    head_sha: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct Run {
    id: i64,
    head_sha: String,
    status: String,
    event: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    eligible: bool,
    reason: String,
    retention_days: i64,
}

fn load_policy(path: &Path) -> Result<(Policy, String)> {
    let value = external_private_json(path, "artifact policy")?;
    let policy: Policy =
        serde_json::from_value(value).map_err(|_| error("artifact policyのschemaが不正です"))?;
    validate_policy(&policy)?;
    let bytes =
        serde_json::to_vec(&policy).map_err(|_| error("artifact policyを固定できません"))?;
    Ok((policy, sha256(bytes)))
}

fn validate_policy(policy: &Policy) -> Result<()> {
    if policy.version != POLICY_VERSION
        || policy.repositories.is_empty()
        || policy.repositories.len() > 100
    {
        return Err(error(
            "artifact policyのversionまたはrepository登録が不正です",
        ));
    }
    let mut repositories = HashSet::new();
    for repository in &policy.repositories {
        if !repository_name(&repository.repository)
            || !repositories.insert(repository.repository.to_ascii_lowercase())
            || repository.usages.is_empty()
            || repository.usages.len() > 100
            || repository
                .retention_days
                .is_some_and(|days| !(1..=MAX_RETENTION_DAYS).contains(&days))
            || repository.recovery_jobs.len() > 100
        {
            return Err(error("artifact repository登録が不正です"));
        }
        let mut prefixes = HashSet::new();
        for usage in &repository.usages {
            if usage.name_prefix.is_empty()
                || usage.name_prefix.len() > 200
                || usage.name_prefix.chars().any(char::is_control)
                || !prefixes.insert(usage.name_prefix.clone())
                || !matches!(
                    usage.purpose.as_str(),
                    "ordinary-ci"
                        | "deployment-candidate"
                        | "saved-plan"
                        | "rollback"
                        | "audit"
                        | "verification"
                        | "other"
                )
                || usage
                    .retention_days
                    .is_some_and(|days| !(1..=MAX_RETENTION_DAYS).contains(&days))
            {
                return Err(error("artifact use登録が不正です"));
            }
        }
        if repository.clearances.len() > MAX_ARTIFACTS {
            return Err(error("artifact clearanceが上限を超えました"));
        }
        let mut clearance_ids = HashSet::new();
        for clearance in &repository.clearances {
            if clearance.artifact_id < 1
                || clearance.run_id < 1
                || oid(&clearance.head_sha, "clearance head").is_err()
                || clearance.references.len() > 100
                || clearance.references.iter().any(|reference| {
                    reference.len() > 500 || reference.chars().any(char::is_control)
                })
                || !clearance_ids.insert(clearance.artifact_id)
                || utc_epoch(&clearance.verified_at).is_err()
            {
                return Err(error("artifact clearanceが不正です"));
            }
        }
        let mut recovery_jobs = HashSet::new();
        for job in &repository.recovery_jobs {
            if !workflow_yaml_path(&job.workflow_path)
                || job.job_name.is_empty()
                || job.job_name.len() > 200
                || job.job_name.chars().any(char::is_control)
                || !job.ci_only
                || !job.deployment_free
                || !recovery_jobs.insert((&job.workflow_path, &job.job_name))
            {
                return Err(error("CI復旧job登録が不正です"));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn classify(
    artifact: &Artifact,
    usage: Option<&Usage>,
    clearance: Option<&Clearance>,
    run: Option<&Run>,
    open_pr_sha: &HashSet<String>,
    active_run_sha: &HashSet<String>,
    deployment_sha: &HashSet<String>,
    repo_retention_days: Option<i64>,
    now_epoch: i64,
) -> Candidate {
    let hold = |reason: &str, days| Candidate {
        eligible: false,
        reason: reason.into(),
        retention_days: days,
    };
    let Some(usage) = usage else {
        return hold("用途が未登録", DEFAULT_RETENTION_DAYS);
    };
    let days = usage
        .retention_days
        .or(repo_retention_days)
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    if usage.purpose != "ordinary-ci" {
        return hold("保護用途", days);
    }
    let Some(clearance) = clearance.filter(|entry| {
        entry.artifact_id == artifact.id
            && entry.run_id == artifact.workflow_run.id
            && entry.head_sha == artifact.workflow_run.head_sha
    }) else {
        return hold("artifact単位のprivate clearanceがない", days);
    };
    if !clearance.references.is_empty() {
        return hold("artifactに参照がある", days);
    }
    if !clearance.regenerable {
        return hold("artifactを再生成できる証拠がない", days);
    }
    if clearance.sole_evidence {
        return hold("唯一の証拠", days);
    }
    let Ok(verified_epoch) = utc_epoch(&clearance.verified_at) else {
        return hold("private clearanceの日時が不正", days);
    };
    if verified_epoch > now_epoch || now_epoch > verified_epoch.saturating_add(300) {
        return hold("private clearanceが5分で失効", days);
    }
    if artifact.id <= 0
        || artifact.size_in_bytes == 0
        || artifact.workflow_run.id <= 0
        || oid(&artifact.workflow_run.head_sha, "artifact head").is_err()
    {
        return hold("artifact識別情報が不正", days);
    }
    if artifact.expired {
        return hold("GitHub上で期限切れ", days);
    }
    let Ok(created) = utc_epoch(&artifact.created_at) else {
        return hold("作成日時を判定できない", days);
    };
    if utc_epoch(&artifact.expires_at).is_err() {
        return hold("保持期限を判定できない", days);
    }
    if now_epoch < created.saturating_add(days.saturating_mul(86_400)) {
        return hold("登録保持期間内", days);
    }
    if open_pr_sha.contains(&artifact.workflow_run.head_sha) {
        return hold("未統合PRに所属", days);
    }
    if !active_run_sha.is_empty() {
        return hold("repositoryでCI進行中", days);
    }
    if deployment_sha.contains(&artifact.workflow_run.head_sha) {
        return hold("同じheadに配備参照がある", days);
    }
    let Some(run) = run else {
        return hold("所属runを取得できない", days);
    };
    if run.id != artifact.workflow_run.id
        || run.head_sha != artifact.workflow_run.head_sha
        || normalized_workflow_path(&run.path).is_err()
    {
        return hold("所属runのidentityを確認できない", days);
    }
    if run.status != "completed" {
        return hold("進行中run", days);
    }
    if !["push", "pull_request"].contains(&run.event.as_str()) {
        return hold("通常CI以外または用途不明のrun", days);
    }
    Candidate {
        eligible: true,
        reason: "登録保持期間を超過し、通常CI・参照なし・再生成可能を確認".into(),
        retention_days: days,
    }
}

/// suffixはAPI対象に使わず、run/head/attemptで所属を照合する。ref文字種を独自制限しない。
fn normalized_workflow_path(raw: &str) -> Result<&str> {
    let (path, reference) = raw
        .split_once('@')
        .map_or((raw, None), |(path, reference)| (path, Some(reference)));
    if !workflow_yaml_path(path)
        || reference.is_some_and(|reference| {
            reference.is_empty()
                || reference
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
        })
    {
        return Err(error("workflow pathまたはref suffixを確認できません"));
    }
    Ok(path)
}

/// job応答にattempt fieldはないため、attempt専用一覧で所属を確認する。
fn attempt_job_with(
    repository: &str,
    run: i64,
    attempt: i64,
    job: i64,
    head: &str,
    fetch: &mut impl FnMut(&str) -> Result<Value>,
) -> Result<()> {
    let mut seen = 0;
    let mut expected = None;
    for page in 1..=MAX_PAGES {
        let response = fetch(&format!(
            "repos/{repository}/actions/runs/{run}/attempts/{attempt}/jobs?per_page=100&page={page}"
        ))?;
        let count = int_value(&response, "total_count")?;
        let jobs = get(&response, "jobs")?
            .as_array()
            .ok_or_else(|| error("attempt jobsのschemaが不正です"))?;
        if count < 0
            || count as usize > MAX_ARTIFACTS
            || jobs.len() > 100
            || expected.is_some_and(|old| old != count)
        {
            return Err(error("attempt jobsの完全な一覧を確認できません"));
        }
        expected = Some(count);
        seen += jobs.len();
        if seen > count as usize {
            return Err(error("attempt jobsの件数が一致しません"));
        }
        for item in jobs {
            if int_value(item, "id")? == job {
                if int_value(item, "run_id")? != run
                    || str_value(item, "head_sha")? != head
                    || str_value(item, "status")? != "completed"
                {
                    return Err(error("固定attemptのjob identity/stateが一致しません"));
                }
                return Ok(());
            }
        }
        if seen >= count as usize || jobs.len() < 100 {
            break;
        }
    }
    Err(error("固定attemptに対象jobがありません"))
}

fn clearance_for<'a>(
    repository: &'a RepositoryPolicy,
    artifact: &Artifact,
) -> Option<&'a Clearance> {
    repository.clearances.iter().find(|entry| {
        entry.artifact_id == artifact.id
            && entry.run_id == artifact.workflow_run.id
            && entry.head_sha == artifact.workflow_run.head_sha
    })
}

fn utc_epoch(value: &str) -> std::result::Result<i64, ()> {
    // APIのZ表記だけを受理する。offset/fraction表記は誤比較を避けて保持する。
    if value.len() < 20
        || !value.is_ascii()
        || !value.ends_with('Z')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return Err(());
    }
    if value.len() > 20
        && (value.as_bytes()[19] != b'.'
            || value.as_bytes()[20..value.len() - 1].is_empty()
            || !value.as_bytes()[20..value.len() - 1]
                .iter()
                .all(u8::is_ascii_digit))
    {
        return Err(());
    }
    let number = |range: std::ops::Range<usize>| {
        let bytes = &value.as_bytes()[range];
        if !bytes.iter().all(u8::is_ascii_digit) {
            return Err(());
        }
        std::str::from_utf8(bytes)
            .map_err(|_| ())?
            .parse::<i64>()
            .map_err(|_| ())
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 60 {
        return Err(());
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=month_days).contains(&day) {
        return Err(());
    }
    let days = days_from_civil(year, month, day);
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn api_get(root: &Path, endpoint: &str) -> Result<Value> {
    gh_json(
        root,
        &[
            "api".into(),
            "--hostname".into(),
            "github.com".into(),
            endpoint.into(),
        ],
    )
}

fn api_status(root: &Path, method: &str, endpoint: &str) -> Result<(u16, bool)> {
    let sandbox = GhSandbox::create(root)?;
    let args = vec![
        "api".into(),
        "--hostname".into(),
        "github.com".into(),
        "--include".into(),
        "--method".into(),
        method.into(),
        endpoint.into(),
    ];
    let args = sandbox.snapshot_args(&args)?;
    let mut command = vec![trusted_binary(GH_BINARY, "GitHub CLI")?];
    command.extend(args);
    let response = run_with_config(
        &command,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
        Some(&sandbox.path),
    )?;
    let status = response
        .stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("HTTP/")?
                .split_whitespace()
                .nth(1)?
                .parse::<u16>()
                .ok()
        })
        .next_back()
        .ok_or_else(|| error("GitHub APIのHTTP statusを確認できません"))?;
    Ok((status, response.status.success()))
}

fn artifact_pages(root: &Path, repository: &str) -> Result<Vec<Artifact>> {
    let mut artifacts = Vec::new();
    let mut expected = None;
    for page in 1..=MAX_PAGES {
        let endpoint = format!("repos/{repository}/actions/artifacts?per_page=100&page={page}");
        let response = api_get(root, &endpoint)?;
        let total = int_value(&response, "total_count")?;
        let page_artifacts = get(&response, "artifacts")?
            .as_array()
            .ok_or_else(|| error("artifact一覧のschemaが不正です"))?;
        if total < 0
            || expected.is_some_and(|value| value != total)
            || total as usize > MAX_ARTIFACTS
            || artifacts.len().saturating_add(page_artifacts.len()) > MAX_ARTIFACTS
        {
            return Err(error("artifact一覧が上限を超えました"));
        }
        expected = Some(total);
        for item in page_artifacts {
            artifacts.push(
                serde_json::from_value(item.clone())
                    .map_err(|_| error("artifact情報のschemaが不正です"))?,
            );
        }
        if artifacts.len() == total as usize {
            return Ok(artifacts);
        }
        if artifacts.len() > total as usize || page_artifacts.len() < 100 {
            return Err(error("artifact一覧が不完全または取得中に変化しました"));
        }
    }
    Err(error("artifact一覧のページ数が上限を超えました"))
}

fn open_pr_heads(root: &Path, repository: &str) -> Result<HashSet<String>> {
    let mut result = HashSet::new();
    for page in 1..=MAX_PAGES {
        let endpoint = format!("repos/{repository}/pulls?state=open&per_page=100&page={page}");
        let response = api_get(root, &endpoint)?;
        let items = response
            .as_array()
            .ok_or_else(|| error("PR一覧のschemaが不正です"))?;
        if items.is_empty() {
            return Ok(result);
        }
        if items.len() > 100 {
            return Err(error("PR一覧のschemaが不正です"));
        }
        for item in items {
            let head = str_value(get(item, "head")?, "sha")?;
            result.insert(oid(&head, "PR head")?);
        }
        if items.len() < 100 {
            return Ok(result);
        }
    }
    Err(error("PR一覧のページ数が上限を超えました"))
}

fn active_run_heads(root: &Path, repository: &str) -> Result<HashSet<String>> {
    let mut result = HashSet::new();
    let mut expected = None;
    let mut seen = 0;
    for page in 1..=MAX_PAGES {
        let endpoint = format!("repos/{repository}/actions/runs?per_page=100&page={page}");
        let response = api_get(root, &endpoint)?;
        let count = int_value(&response, "total_count")?;
        let items = get(&response, "workflow_runs")?
            .as_array()
            .ok_or_else(|| error("workflow run一覧のschemaが不正です"))?;
        if count < 0
            || expected.is_some_and(|value| value != count)
            || count as usize > MAX_ARTIFACTS
            || items.len() > 100
        {
            return Err(error("workflow run一覧が上限を超えました"));
        }
        expected = Some(count);
        seen += items.len();
        for item in items {
            let head = oid(&str_value(item, "head_sha")?, "workflow run head")?;
            let status = str_value(item, "status")?;
            if status != "completed" {
                result.insert(head);
            }
        }
        if result.len() > MAX_ARTIFACTS {
            return Err(error("進行中workflow runが上限を超えました"));
        }
        if seen == count as usize {
            return Ok(result);
        }
        if seen > count as usize || items.len() < 100 {
            return Err(error("workflow run一覧が不完全または取得中に変化しました"));
        }
    }
    Err(error("workflow run一覧のページ数が上限を超えました"))
}

fn deployment_heads(root: &Path, repository: &str) -> Result<HashSet<String>> {
    let mut result = HashSet::new();
    for page in 1..=MAX_PAGES {
        let endpoint = format!("repos/{repository}/deployments?per_page=100&page={page}");
        let response = api_get(root, &endpoint)?;
        let items = response
            .as_array()
            .ok_or_else(|| error("deployment一覧のschemaが不正です"))?;
        if items.len() > 100 {
            return Err(error("deployment一覧のschemaが不正です"));
        }
        for item in items {
            let id = int_value(item, "id")?;
            if id < 1 {
                return Err(error("deployment IDが不正です"));
            }
            result.insert(oid(&str_value(item, "sha")?, "deployment sha")?);
        }
        if items.len() < 100 {
            return Ok(result);
        }
    }
    Err(error("deployment一覧のページ数が上限を超えました"))
}

fn live_protected_heads(
    root: &Path,
    repository: &str,
) -> Result<(HashSet<String>, HashSet<String>, HashSet<String>)> {
    Ok((
        open_pr_heads(root, repository)?,
        active_run_heads(root, repository)?,
        deployment_heads(root, repository)?,
    ))
}

fn get_run(root: &Path, repository: &str, artifact: &Artifact) -> Result<Run> {
    let endpoint = format!(
        "repos/{repository}/actions/runs/{}",
        artifact.workflow_run.id
    );
    let value = api_get(root, &endpoint)?;
    let run: Run = serde_json::from_value(value).map_err(|_| error("所属runのschemaが不正です"))?;
    Ok(run)
}

fn matching_usage<'a>(repository: &'a RepositoryPolicy, name: &str) -> Option<&'a Usage> {
    let mut matches = repository
        .usages
        .iter()
        .filter(|usage| name.starts_with(&usage.name_prefix));
    let usage = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(usage)
    }
}

fn private_root() -> Result<PathBuf> {
    let path = codex_home()?.join("worktrees").join(".actions-artifacts");
    let parent = path
        .parent()
        .ok_or_else(|| error("private artifact rootが不正です"))?;
    safe_directory(parent)?;
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(error("private artifact rootを作成できません")),
    }
    safe_directory(&path)?;
    Ok(path)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: i64,
    records: Vec<Value>,
}

fn ledger_path(root: &Path, repo: &str) -> Result<PathBuf> {
    let key = repo_key(repo)?;
    Ok(root.join(format!("{key}.json")))
}

fn append_record(root: &Path, path: &Path, policy_hash: &str, value: Value) -> Result<()> {
    let mut ledger = if path.exists() {
        serde_json::from_value::<Ledger>(read_json_file(path, "artifact private ledger")?)
            .map_err(|_| error("artifact private ledgerのschemaが不正です"))?
    } else {
        Ledger {
            version: LEDGER_VERSION,
            records: Vec::new(),
        }
    };
    if ledger.version != LEDGER_VERSION {
        return Err(error("artifact private ledgerを安全に更新できません"));
    }
    let previous = serde_json::to_value(&ledger).map_err(|_| error("整理履歴を保存できません"))?;
    let mut value = value;
    value["policy_sha256"] = string(policy_hash);
    ledger.records.push(value);
    let mut encoded = serde_json::to_value(&ledger)
        .map_err(|_| error("artifact private ledgerを保存できません"))?;
    if serde_json::to_vec(&encoded)
        .map_err(|_| error("整理履歴のsizeを検査できません"))?
        .len()
        > MAX_FILE_BYTES as usize / 2
    {
        // atomic_jsonの整形分も含めread上限を超えないよう、余裕を持って履歴を分割する。
        let archive = path.with_extension(format!(
            "archive-{}.json",
            sha256(serde_json::to_vec(&previous).map_err(|_| error("履歴digestを計算できません"))?)
        ));
        if archive.exists() || archive.is_symlink() {
            if read_json_file(&archive, "整理履歴archive")? != previous {
                return Err(error("整理履歴archiveが一致しません"));
            }
        } else {
            atomic_json(&archive, &previous)?;
        }
        let mut latest = BTreeMap::new();
        for entry in previous["records"]
            .as_array()
            .ok_or_else(|| error("整理履歴が不正です"))?
        {
            if let Some(id) = entry["artifact_id"].as_i64()
                && matches!(
                    entry["action"].as_str(),
                    Some("delete-intent" | "delete-result")
                )
            {
                latest.insert(id, entry.clone());
            }
        }
        let newest = ledger
            .records
            .pop()
            .ok_or_else(|| error("整理記録がありません"))?;
        ledger.records = latest
            .into_values()
            .filter(|entry| entry["action"] == "delete-intent" && entry["state"] == "pending")
            .collect();
        ledger.records.push(newest);
        encoded =
            serde_json::to_value(ledger).map_err(|_| error("未確定intentを保存できません"))?;
    }
    if serde_json::to_vec_pretty(&encoded)
        .map_err(|_| error("整理履歴のsizeを検査できません"))?
        .len()
        + 1
        > MAX_FILE_BYTES as usize
    {
        return Err(error(
            "未確定intentが記録上限を超えています。削除せず保持します",
        ));
    }
    atomic_json(path, &encoded)?;
    sync_directory(root, "artifact private ledger")
}

fn reconcile_delete_intents(
    root: &Path,
    ledger_root: &Path,
    path: &Path,
    policy_hash: &str,
    repository: &str,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let ledger: Ledger = serde_json::from_value(read_json_file(path, "artifact private ledger")?)
        .map_err(|_| error("artifact private ledgerのschemaが不正です"))?;
    let mut latest: HashMap<i64, Value> = HashMap::new();
    for entry in ledger.records {
        if let (Some(id), Some(action)) = (
            entry.get("artifact_id").and_then(Value::as_i64),
            entry.get("action").and_then(Value::as_str),
        ) && (action == "delete-intent" || action == "delete-result")
        {
            latest.insert(id, entry);
        }
    }
    for (id, entry) in latest {
        if str_value(&entry, "action")? != "delete-intent"
            || str_value(&entry, "state")? != "pending"
        {
            continue;
        }
        let endpoint = format!("repos/{repository}/actions/artifacts/{id}");
        let (status, success) = api_status(root, "GET", &endpoint)?;
        let state = match status {
            404 => {
                if artifact_pages(root, repository)?
                    .iter()
                    .any(|item| item.id == id)
                {
                    return Err(error("artifact取得と一覧が一致しません。結果を保留します"));
                }
                "deleted-confirmed"
            }
            200 if success => "request-not-applied",
            _ => {
                return Err(error(
                    "以前のartifact DELETE結果が判定できません。保護して停止しました",
                ));
            }
        };
        append_record(
            ledger_root,
            path,
            policy_hash,
            object([
                ("at", string(now())),
                ("action", string("delete-result")),
                ("artifact_id", Value::from(id)),
                ("artifact_name", get(&entry, "artifact_name")?.clone()),
                ("run_id", get(&entry, "run_id")?.clone()),
                ("state", string(state)),
            ]),
        )?;
    }
    Ok(())
}

fn report_repository(
    policy: &RepositoryPolicy,
    artifacts: &[Artifact],
    runs: &HashMap<i64, Run>,
    open_prs: &HashSet<String>,
    active_runs: &HashSet<String>,
    deployments: &HashSet<String>,
    now: i64,
) -> Result<Value> {
    let mut total_bytes = 0u64;
    let mut reducible_bytes = 0u64;
    let mut rows = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        total_bytes = total_bytes.saturating_add(artifact.size_in_bytes);
        let usage = matching_usage(policy, &artifact.name);
        let run = runs.get(&artifact.workflow_run.id);
        let candidate = classify(
            artifact,
            usage,
            clearance_for(policy, artifact),
            run,
            open_prs,
            active_runs,
            deployments,
            policy.retention_days,
            now,
        );
        if candidate.eligible {
            reducible_bytes = reducible_bytes.saturating_add(artifact.size_in_bytes);
        }
        rows.push(object([
            ("id", Value::from(artifact.id)),
            ("name", string(&artifact.name)),
            ("size_in_bytes", Value::from(artifact.size_in_bytes)),
            ("created_at", string(&artifact.created_at)),
            ("expires_at", string(&artifact.expires_at)),
            ("run_id", Value::from(artifact.workflow_run.id)),
            (
                "run_status",
                run.map(|value| string(&value.status))
                    .unwrap_or(Value::Null),
            ),
            ("eligible", Value::Bool(candidate.eligible)),
            ("reason", string(candidate.reason)),
            ("retention_days", Value::from(candidate.retention_days)),
        ]));
    }
    Ok(object([
        ("repository", string(&policy.repository)),
        ("artifact_count", Value::from(artifacts.len())),
        ("total_bytes", Value::from(total_bytes)),
        ("reducible_bytes", Value::from(reducible_bytes)),
        ("artifacts", Value::Array(rows)),
    ]))
}

fn inventory(root: &Path, policy: &Policy, policy_hash: &str) -> Result<Value> {
    let mut repositories = Vec::new();
    for repository in &policy.repositories {
        let artifacts = artifact_pages(root, &repository.repository)?;
        let mut runs = HashMap::new();
        for artifact in &artifacts {
            if matching_usage(repository, &artifact.name).is_some()
                && !runs.contains_key(&artifact.workflow_run.id)
                && let Ok(run) = get_run(root, &repository.repository, artifact)
            {
                runs.insert(artifact.workflow_run.id, run);
            }
        }
        // live APIの取得失敗時は削減見込みを0にし、全件を保護する。
        let (report, live_ok) = match live_protected_heads(root, &repository.repository) {
            Ok((open_prs, active_runs, deployments)) => (
                report_repository(
                    repository,
                    &artifacts,
                    &runs,
                    &open_prs,
                    &active_runs,
                    &deployments,
                    epoch_now(),
                )?,
                true,
            ),
            Err(_) => (
                report_repository(
                    repository,
                    &artifacts,
                    &runs,
                    &HashSet::new(),
                    &HashSet::new(),
                    &HashSet::new(),
                    epoch_now(),
                )?,
                false,
            ),
        };
        if !live_ok {
            let mut report = report;
            report["reducible_bytes"] = Value::from(0);
            if let Some(rows) = report.get_mut("artifacts").and_then(Value::as_array_mut) {
                for row in rows.iter_mut() {
                    row["eligible"] = Value::Bool(false);
                    row["reason"] = string("live run/PR/deployment状態を取得できない");
                }
            }
            repositories.push(report);
        } else {
            repositories.push(report);
        }
    }
    Ok(object([
        ("policy_sha256", string(policy_hash)),
        (
            "default_retention_days",
            Value::from(DEFAULT_RETENTION_DAYS),
        ),
        ("repositories", Value::Array(repositories)),
    ]))
}

fn cleanup(root: &Path, policy_path: &Path, policy: &Policy, policy_hash: &str) -> Result<Value> {
    let private = private_root()?;
    let policy_archive = private.join(format!("policy-{policy_hash}.json"));
    let policy_value =
        serde_json::to_value(policy).map_err(|_| error("整理policyを保存できません"))?;
    if policy_archive.exists() || policy_archive.is_symlink() {
        if read_json_file(&policy_archive, "整理policy証拠")? != policy_value {
            return Err(error("整理policy証拠が一致しません"));
        }
    } else {
        atomic_json(&policy_archive, &policy_value)?;
    }
    let mut deleted = Vec::new();
    let mut held = Vec::new();
    for repository in &policy.repositories {
        let repo_path = ledger_path(&private, &repository.repository)?;
        let _lock =
            super::super::task::store::Lock::acquire_short(&repo_path.with_extension("lock"))
                .map_err(error)?;
        reconcile_delete_intents(
            root,
            &private,
            &repo_path,
            policy_hash,
            &repository.repository,
        )?;
        let artifacts = artifact_pages(root, &repository.repository)?;
        let (open_prs, active_runs, deployments) =
            live_protected_heads(root, &repository.repository)?;
        for artifact in artifacts {
            let usage = matching_usage(repository, &artifact.name);
            let run = if usage.is_some() {
                get_run(root, &repository.repository, &artifact).ok()
            } else {
                None
            };
            let candidate = classify(
                &artifact,
                usage,
                clearance_for(repository, &artifact),
                run.as_ref(),
                &open_prs,
                &active_runs,
                &deployments,
                repository.retention_days,
                epoch_now(),
            );
            if !candidate.eligible {
                held.push(object([
                    ("repository", string(&repository.repository)),
                    ("artifact_id", Value::from(artifact.id)),
                    ("reason", string(candidate.reason)),
                ]));
                continue;
            }

            // 先に記録し、結果不明のDELETEを無条件に再送しない。
            append_record(
                &private,
                &repo_path,
                policy_hash,
                object([
                    ("at", string(now())),
                    ("action", string("delete-intent")),
                    ("artifact_id", Value::from(artifact.id)),
                    ("artifact_name", string(&artifact.name)),
                    ("run_id", Value::from(artifact.workflow_run.id)),
                    ("state", string("pending")),
                ]),
            )?;
            let (_, current_policy_hash) = load_policy(policy_path)?;
            if current_policy_hash != policy_hash {
                return Err(error("artifact policyがcleanup中に変更されました"));
            }
            // 削除直前にartifact、所属run、open PR、全run、deploymentを再取得する。
            let endpoint = format!(
                "repos/{}/actions/artifacts/{}",
                repository.repository, artifact.id
            );
            verified_delete(
                || {
                    let current_value = api_get(root, &endpoint)?;
                    let current: Artifact = serde_json::from_value(current_value)
                        .map_err(|_| error("削除直前artifact情報のschemaが不正です"))?;
                    if current != artifact {
                        return Err(error("削除対象artifactが棚卸し後に変化しました"));
                    }
                    let current_run = get_run(root, &repository.repository, &current)?;
                    let (current_open_prs, current_active_runs, current_deployments) =
                        live_protected_heads(root, &repository.repository)?;
                    let final_candidate = classify(
                        &current,
                        matching_usage(repository, &current.name),
                        clearance_for(repository, &current),
                        Some(&current_run),
                        &current_open_prs,
                        &current_active_runs,
                        &current_deployments,
                        repository.retention_days,
                        epoch_now(),
                    );
                    if !final_candidate.eligible {
                        return Err(error(
                            "削除直前の保護条件が変化しました。artifactを保持しました",
                        ));
                    }
                    append_record(
                        &private,
                        &repo_path,
                        policy_hash,
                        object([
                            ("at", string(now())),
                            ("action", string("delete-rechecked")),
                            ("artifact_id", Value::from(current.id)),
                            (
                                "artifact",
                                serde_json::to_value(&current)
                                    .map_err(|_| error("artifact証拠を保存できません"))?,
                            ),
                            (
                                "run",
                                serde_json::to_value(&current_run)
                                    .map_err(|_| error("run証拠を保存できません"))?,
                            ),
                            ("protection_checked", Value::Bool(true)),
                        ]),
                    )?;
                    // 個別artifact endpointのみを呼び、workflow run/cache/Packagesは操作しない。
                    Ok(())
                },
                || api_status(root, "DELETE", &endpoint),
            )?;
            let record = object([
                ("at", string(now())),
                ("action", string("delete-result")),
                ("artifact_id", Value::from(artifact.id)),
                ("artifact_name", string(&artifact.name)),
                ("run_id", Value::from(artifact.workflow_run.id)),
                ("size_in_bytes", Value::from(artifact.size_in_bytes)),
                ("state", string("deleted")),
            ]);
            append_record(&private, &repo_path, policy_hash, record.clone())?;
            deleted.push(record);
        }
    }
    Ok(object([
        ("deleted", Value::Array(deleted)),
        ("held", Value::Array(held)),
        ("private_recorded", Value::Bool(true)),
        (
            "note",
            string(
                "GitHub storage usage may take 6–12 hours to refresh; billing accrual is not reversed.",
            ),
        ),
    ]))
}

/// 再照合の失敗時にはDELETEへ進まない。結果不明なら上位のintentを保持する。
fn verified_delete(
    recheck: impl FnOnce() -> Result<()>,
    send: impl FnOnce() -> Result<(u16, bool)>,
) -> Result<()> {
    recheck()?;
    let (status, success) = send()?;
    if status != 204 || !success {
        return Err(error(
            "artifact削除結果を確認できません。private intentを保持しました",
        ));
    }
    Ok(())
}

fn rerun_once(
    record: &mut Value,
    mut save: impl FnMut(&Value) -> Result<()>,
    send: impl FnOnce() -> Result<(u16, bool)>,
) -> Result<()> {
    if str_value(record, "phase")? != "storage-recovered" || int_value(record, "rerun_count")? != 0
    {
        return Err(error(
            "対象jobのrerunは一度だけです。結果不明時は再送しません",
        ));
    }
    record["phase"] = string("rerun-requested");
    record["rerun_count"] = Value::from(1);
    record["rerun_api_result"] = string("pending");
    record["rerun_requested_at"] = string(now());
    save(record)?;
    let (status, success) = send()?;
    if status != 201 || !success {
        return Err(error(
            "rerun要求の結果が不明です。保存済みscopeを監視し再送しません",
        ));
    }
    record["rerun_api_result"] = string("accepted");
    save(record)
}

fn evidence_matches(evidence: &RecoveryEvidence, record: &Value) -> bool {
    evidence.repository == record["repository"]
        && evidence.task_id == record["task_id"]
        && evidence.pr == record["pr"]
        && evidence.head_sha == record["head_sha"]
        && evidence.base_sha == record["base_sha"]
        && evidence.plan_id == record["plan_id"]
        && evidence.plan_version == record["plan_version"]
        && evidence.run_id == record["run_id"]
        && evidence.job_id == record["job_id"]
}

fn verify_merge_identity(
    root: &Path,
    repository: &str,
    base: &str,
    head: &str,
    merge: &str,
) -> Result<()> {
    let commit: Value = api_get(root, &format!("repos/{repository}/git/commits/{merge}"))?;
    let parents = get(&commit, "parents")?
        .as_array()
        .ok_or_else(|| error("実merge parentsを確認できません"))?;
    if parents.len() != 2
        || str_value(&parents[0], "sha")? != base
        || str_value(&parents[1], "sha")? != head
    {
        return Err(error("実mergeの親が固定base/headと一致しません"));
    }
    let merge_tree = str_value(get(&commit, "tree")?, "sha")?;
    let head_commit = api_get(root, &format!("repos/{repository}/git/commits/{head}"))?;
    if str_value(get(&head_commit, "tree")?, "sha")? != merge_tree {
        return Err(error("実merge treeが検証済みheadと一致しません"));
    }
    let comparison = api_get(root, &format!("repos/{repository}/compare/{merge}...main"))?;
    if !matches!(
        str_value(&comparison, "status")?.as_str(),
        "ahead" | "identical"
    ) {
        return Err(error("実mergeがmainへ到達していません"));
    }
    Ok(())
}

fn verify_pr_identity(
    root: &Path,
    repository: &str,
    pr: i64,
    head: &str,
    base: &str,
    run_head: &str,
) -> Result<Option<String>> {
    let view = gh_json(
        root,
        &[
            "pr".into(),
            "view".into(),
            pr.to_string(),
            "--repo".into(),
            repository.into(),
            "--json".into(),
            "state,headRefOid,baseRefName,baseRefOid,mergeCommit".into(),
        ],
    )?;
    if str_value(&view, "headRefOid")? != head || str_value(&view, "baseRefName")? != "main" {
        return Err(error("capacity recovery PR identityが一致しません"));
    }
    match str_value(&view, "state")?.as_str() {
        "OPEN" => {
            if str_value(&view, "baseRefOid")? != base || run_head != head {
                return Err(error("open PRのbase/headと失敗runが一致しません"));
            }
            Ok(None)
        }
        "MERGED" => {
            let merge = oid(&str_value(get(&view, "mergeCommit")?, "oid")?, "実merge")?;
            if run_head != merge {
                return Err(error("merge後run headが実merge SHAと一致しません"));
            }
            verify_merge_identity(root, repository, base, head, &merge)?;
            Ok(Some(merge))
        }
        _ => Err(error(
            "capacity recovery PRがopenまたはmergedではありません",
        )),
    }
}

fn proof_current(value: &Value, key: &str, now_epoch: i64, age: i64) -> Result<()> {
    let observed =
        utc_epoch(&str_value(value, key)?).map_err(|_| error("private evidence日時が不正です"))?;
    if observed > now_epoch || now_epoch.saturating_sub(observed) > age {
        return Err(error("private evidenceが未来または期限切れです"));
    }
    Ok(())
}

/// 容量障害のprivate再開state。workflow dispatchやdelivery receipt変更はせず、対象jobを一度だけ再実行する。
fn recovery(root: &Path, args: &[String]) -> Result<Value> {
    let action = args
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| error("recovery actionが必要です"))?;
    if !["record", "confirm", "rerun"].contains(&action) {
        return Err(error("recovery actionはrecord|confirm|rerunに限ります"));
    }
    let mut values = BTreeMap::new();
    let mut index = 2;
    while index < args.len() {
        let key = args[index].as_str();
        if !key.starts_with("--")
            || index + 1 >= args.len()
            || values.insert(key, args[index + 1].as_str()).is_some()
        {
            return Err(error("recovery optionが不正です"));
        }
        index += 2;
    }
    let allowed: &[&str] = match action {
        "record" | "confirm" => &[
            "--policy",
            "--repository",
            "--run-id",
            "--job-id",
            "--head",
            "--evidence",
        ],
        _ => &["--policy", "--repository", "--run-id", "--job-id", "--head"],
    };
    if values.keys().any(|key| !allowed.contains(key)) {
        return Err(error("recovery optionが許可されていません"));
    }
    let required = |name: &str| {
        values
            .get(name)
            .copied()
            .ok_or_else(|| error("recoveryの対象情報が不足しています"))
    };
    let (policy, policy_hash) = load_policy(&PathBuf::from(required("--policy")?))?;
    let repo = required("--repository")?;
    let repo_policy = policy
        .repositories
        .iter()
        .find(|entry| entry.repository == repo)
        .ok_or_else(|| error("recovery対象repositoryは未登録です"))?;
    let run_id = required("--run-id")?
        .parse::<i64>()
        .map_err(|_| error("run IDが不正です"))?;
    let job_id = required("--job-id")?
        .parse::<i64>()
        .map_err(|_| error("job IDが不正です"))?;
    if run_id < 1 || job_id < 1 {
        return Err(error("run/job IDが不正です"));
    }
    let supplied_head = oid(required("--head")?, "recovery head")?;
    let state_dir = private_root()?;
    let key = sha256(format!("{repo}:{run_id}:{job_id}:{supplied_head}"));
    let record_path = state_dir.join(format!("recovery-{key}.json"));
    let _lock = super::super::task::store::Lock::acquire_short(&record_path.with_extension("lock"))
        .map_err(error)?;
    let run = api_get(root, &format!("repos/{repo}/actions/runs/{run_id}"))?;
    let job = api_get(root, &format!("repos/{repo}/actions/jobs/{job_id}"))?;
    if int_value(&run, "id")? != run_id
        || str_value(&run, "status")? != "completed"
        || int_value(&job, "id")? != job_id
        || int_value(&job, "run_id")? != run_id
        || str_value(&job, "status")? != "completed"
    {
        return Err(error(
            "recovery run/job identityまたは完了状態が一致しません",
        ));
    }
    let raw_workflow_path = str_value(&run, "path")?;
    let workflow_path = normalized_workflow_path(&raw_workflow_path)?;
    let job_name = str_value(&job, "name")?;
    let registered = repo_policy.recovery_jobs.iter().any(|item| {
        item.workflow_path == workflow_path
            && item.job_name == job_name
            && item.ci_only
            && item.deployment_free
    });
    if !registered || !["push", "pull_request"].contains(&str_value(&run, "event")?.as_str()) {
        return Err(error(
            "対象workflow/jobはdeployment-free CIとして登録されていません",
        ));
    }
    let run_head = oid(&str_value(&run, "head_sha")?, "run head")?;
    let run_attempt = int_value(&run, "run_attempt")?;
    if run_attempt < 1 || str_value(&job, "head_sha")? != run_head {
        return Err(error("workflow run attemptが不正です"));
    }
    attempt_job_with(
        repo,
        run_id,
        run_attempt,
        job_id,
        &run_head,
        &mut |endpoint| api_get(root, endpoint),
    )?;
    let mut record = if record_path.exists() {
        read_json_file(&record_path, "capacity recovery record")?
    } else {
        Value::Null
    };
    match action {
        "record" => {
            if !record.is_null()
                || str_value(&run, "conclusion")? != "failure"
                || str_value(&job, "conclusion")? != "failure"
            {
                return Err(error("新しいfailure run/jobだけを記録できます"));
            }
            let created = utc_epoch(&str_value(&run, "created_at")?)
                .map_err(|_| error("run作成日時を判定できません"))?;
            if epoch_now().saturating_sub(created) > 30 * 86_400 {
                return Err(error("workflow runの再実行期限を過ぎています"));
            }
            let evidence_value = external_private_json(
                &PathBuf::from(required("--evidence")?),
                "capacity failure evidence",
            )?;
            let evidence: FailureEvidence = serde_json::from_value(evidence_value.clone())
                .map_err(|_| error("capacity failure evidenceのschemaが不正です"))?;
            if evidence.version != 1
                || evidence.repository != repo
                || evidence.run_id != run_id
                || evidence.job_id != job_id
                || evidence.head_sha != supplied_head
                || evidence.cause != "capacity-exhausted"
                || evidence.product_test_failures
                || evidence.summary.trim().is_empty()
                || evidence.summary.len() > 2_000
            {
                return Err(error("容量障害証拠の対象または製品test状態が不正です"));
            }
            task_id(&evidence.task_id)?;
            oid(&evidence.base_sha, "failure base")?;
            if !(1..=999_999_999).contains(&evidence.pr)
                || !plan_id(&evidence.plan_id)
                || !(1..=999_999).contains(&evidence.plan_version)
            {
                return Err(error("failure evidence PR/Planが不正です"));
            }
            if utc_epoch(&evidence.observed_at).is_err() {
                return Err(error("容量障害の観測日時が不正です"));
            }
            proof_current(&evidence_value, "observed_at", epoch_now(), 86_400)?;
            let merge = verify_pr_identity(
                root,
                repo,
                evidence.pr,
                &evidence.head_sha,
                &evidence.base_sha,
                &run_head,
            )?;
            record = object([
                ("version", Value::from(1)),
                ("policy_sha256", string(policy_hash)),
                (
                    "recovery_jobs_sha256",
                    string(sha256(
                        serde_json::to_vec(&repo_policy.recovery_jobs).unwrap_or_default(),
                    )),
                ),
                ("repository", string(repo)),
                ("task_id", string(&evidence.task_id)),
                ("pr", Value::from(evidence.pr)),
                ("head_sha", string(&evidence.head_sha)),
                ("base_sha", string(&evidence.base_sha)),
                ("merge_sha", merge.map(string).unwrap_or(Value::Null)),
                ("plan_id", string(&evidence.plan_id)),
                ("plan_version", Value::from(evidence.plan_version)),
                ("run_id", Value::from(run_id)),
                ("job_id", Value::from(job_id)),
                ("run_attempt", Value::from(run_attempt)),
                ("workflow_path", string(workflow_path)),
                ("job_name", string(job_name)),
                ("failure_evidence", evidence_value.clone()),
                (
                    "failure_evidence_sha256",
                    string(sha256(
                        serde_json::to_vec(&evidence_value).unwrap_or_default(),
                    )),
                ),
                ("product_test_failures", Value::Bool(false)),
                ("phase", string("awaiting-storage-refresh")),
                ("created_at", string(now())),
                ("rerun_count", Value::from(0)),
            ]);
            atomic_json(&record_path, &record)?;
            Ok(object([
                ("phase", string("awaiting-storage-refresh")),
                ("tracked_privately", Value::Bool(true)),
                (
                    "expected_billing_refresh_window_hours",
                    Value::Array(vec![Value::from(6), Value::from(12)]),
                ),
            ]))
        }
        "confirm" | "rerun" => {
            if record.is_null()
                || str_value(&record, "repository")? != repo
                || str_value(&record, "head_sha")? != supplied_head
                || int_value(&record, "run_id")? != run_id
                || int_value(&record, "job_id")? != job_id
            {
                return Err(error("固定対象のcapacity recovery recordがありません"));
            }
            if action == "confirm" {
                let evidence_value = external_private_json(
                    &PathBuf::from(required("--evidence")?),
                    "capacity recovery evidence",
                )?;
                let evidence: RecoveryEvidence = serde_json::from_value(evidence_value.clone())
                    .map_err(|_| error("capacity recovery evidenceのschemaが不正です"))?;
                if evidence.version != 1
                    || !evidence.storage_recovered
                    || evidence.product_test_failures
                    || evidence.source.trim().is_empty()
                    || evidence.summary.trim().is_empty()
                    || evidence.summary.len() > 2_000
                    || !evidence_matches(&evidence, &record)
                    || str_value(&record, "phase")? != "awaiting-storage-refresh"
                {
                    return Err(error("storage recovery evidenceが対象/状態に一致しません"));
                }
                let observed = utc_epoch(&evidence.observed_at)
                    .map_err(|_| error("storage observation日時が不正です"))?;
                let created = utc_epoch(&str_value(&record, "created_at")?)
                    .map_err(|_| error("recovery作成日時が不正です"))?;
                if observed < created
                    || observed > epoch_now()
                    || epoch_now().saturating_sub(observed) > 900
                {
                    return Err(error(
                        "storage回復証拠は記録後かつ15分以内である必要があります",
                    ));
                }
                record["phase"] = string("storage-recovered");
                record["storage_source"] = string(&evidence.source);
                record["storage_observed_at"] = string(&evidence.observed_at);
                record["storage_evidence_sha256"] = string(sha256(
                    serde_json::to_vec(&evidence_value).unwrap_or_default(),
                ));
                record["storage_evidence"] = evidence_value;
                atomic_json(&record_path, &record)?;
                return Ok(object([
                    ("phase", string("storage-recovered")),
                    ("targeted_rerun_ready", Value::Bool(true)),
                ]));
            }
            if str_value(&record, "phase")? != "storage-recovered"
                || int_value(&record, "rerun_count")? != 0
                || run_attempt != int_value(&record, "run_attempt")?
                || str_value(&record, "recovery_jobs_sha256")?
                    != sha256(serde_json::to_vec(&repo_policy.recovery_jobs).unwrap_or_default())
                || str_value(&run, "conclusion")? != "failure"
                || str_value(&job, "conclusion")? != "failure"
            {
                return Err(error(
                    "対象jobのretryは回復確認後に一度だけ実行できます。API不明時は再送しません",
                ));
            }
            proof_current(
                get(&record, "storage_evidence")?,
                "observed_at",
                epoch_now(),
                900,
            )?;
            let merge = verify_pr_identity(
                root,
                repo,
                int_value(&record, "pr")?,
                &str_value(&record, "head_sha")?,
                &str_value(&record, "base_sha")?,
                &run_head,
            )?;
            if merge
                != record
                    .get("merge_sha")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            {
                return Err(error("PR merge identityが固定recordと異なります"));
            }
            let endpoint = format!("repos/{repo}/actions/jobs/{job_id}/rerun");
            rerun_once(
                &mut record,
                |value| atomic_json(&record_path, value),
                || api_status(root, "POST", &endpoint),
            )?;
            Ok(object([
                ("phase", string("rerun-requested")),
                ("rerun_count", Value::from(1)),
                (
                    "delivery_resume",
                    string(
                        "固定job check成功後に同一PR/head/base/Planと実merge親/tree/main到達を再検証して正規deliveryを再開",
                    ),
                ),
            ]))
        }
        _ => Err(error("recovery actionが不正です")),
    }
}

pub(super) fn entry(args: &[String]) -> Result<Value> {
    let result = entry_inner(args);
    let private = private_root()?;
    let events = private.join("operations.json");
    let _lock = super::super::task::store::Lock::acquire_short(&events.with_extension("lock"))
        .map_err(error)?;
    append_record(
        &private,
        &events,
        "operation-summary",
        object([
            ("at", string(now())),
            (
                "action",
                string(args.first().map(String::as_str).unwrap_or("invalid")),
            ),
            ("success", Value::Bool(result.is_ok())),
        ]),
    )?;
    result
}

fn entry_inner(args: &[String]) -> Result<Value> {
    if let Some(reason) = invocation_reason(args) {
        return Err(error(reason));
    }
    if args.first().map(String::as_str) == Some("recovery") {
        let cwd = env::current_dir().map_err(|_| error("checkoutを確認できません"))?;
        return recovery(&cwd, args);
    }
    if args.len() != 3
        || !["inventory", "cleanup"].contains(&args[0].as_str())
        || args[1] != "--policy"
    {
        return Err(error(
            "使い方: codex-delivery actions-artifacts inventory|cleanup --policy <private-path>",
        ));
    }
    let cwd = env::current_dir().map_err(|_| error("checkoutを確認できません"))?;
    let policy_path = PathBuf::from(&args[2]);
    let (policy, policy_hash) = load_policy(&policy_path)?;
    match args[0].as_str() {
        "inventory" => inventory(&cwd, &policy, &policy_hash),
        "cleanup" => cleanup(&cwd, &policy_path, &policy, &policy_hash),
        _ => Err(error("不明なartifact commandです")),
    }
}

pub(super) fn invocation_reason(args: &[String]) -> Option<String> {
    let invalid =
        || Some("actions-artifacts引数またはprivate evidenceを検証できません".to_string());
    if args.first().map(String::as_str) == Some("recovery") {
        if args.len() < 2 || !["record", "confirm", "rerun"].contains(&args[1].as_str()) {
            return invalid();
        }
        let mut values = BTreeMap::new();
        let mut index = 2;
        while index < args.len() {
            if !args[index].starts_with("--")
                || index + 1 >= args.len()
                || values
                    .insert(args[index].as_str(), args[index + 1].as_str())
                    .is_some()
            {
                return invalid();
            }
            index += 2;
        }
        let action = args[1].as_str();
        let required_keys: &[&str] = match action {
            "record" | "confirm" => &[
                "--policy",
                "--repository",
                "--run-id",
                "--job-id",
                "--head",
                "--evidence",
            ],
            _ => &["--policy", "--repository", "--run-id", "--job-id", "--head"],
        };
        if values.len() != required_keys.len()
            || required_keys.iter().any(|key| !values.contains_key(key))
        {
            return invalid();
        }
        let Some(policy_path) = values.get("--policy") else {
            return invalid();
        };
        if !Path::new(policy_path).is_absolute() {
            return invalid();
        }
        let Some(repo) = values.get("--repository") else {
            return invalid();
        };
        if !repository_name(repo) {
            return invalid();
        }
        for name in ["--run-id", "--job-id"] {
            let Some(value) = values.get(name) else {
                return invalid();
            };
            if value.parse::<i64>().ok().is_none_or(|number| number < 1) {
                return invalid();
            }
        }
        let Some(head) = values.get("--head") else {
            return invalid();
        };
        if oid(head, "recovery head").is_err() {
            return invalid();
        }
        if action != "rerun" {
            let Some(evidence) = values.get("--evidence") else {
                return invalid();
            };
            if !Path::new(evidence).is_absolute() {
                return invalid();
            }
        }
        return None;
    }
    if args.len() != 3
        || !["inventory", "cleanup"].contains(&args[0].as_str())
        || args[1] != "--policy"
    {
        return invalid();
    }
    if Path::new(&args[2]).is_absolute() {
        None
    } else {
        invalid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_api_shape_uses_ref_suffix_and_attempt_jobs_without_job_attempt_field() {
        assert_eq!(
            normalized_workflow_path(".github/workflows/ci.yml@main").unwrap(),
            ".github/workflows/ci.yml"
        );
        assert_eq!(
            normalized_workflow_path(".github/workflows/ci.yml").unwrap(),
            ".github/workflows/ci.yml"
        );
        for raw in [
            ".github/workflows/ci.yml@",
            ".github/workflows/ci.yml@main\nother",
            "../ci.yml@main",
        ] {
            assert!(normalized_workflow_path(raw).is_err());
        }
        for reference in [
            "refs/heads/feature/retry+storage",
            "feature/main@other",
            "日本語",
        ] {
            assert_eq!(
                normalized_workflow_path(&format!(".github/workflows/ci.yml@{reference}")).unwrap(),
                ".github/workflows/ci.yml"
            );
        }
        // 公式responseの構造を使うが、識別子とheadはすべて合成値。
        let payload = serde_json::json!({"total_count":1,"jobs":[{"id":101,"run_id":202,"head_sha":"a".repeat(40),"status":"completed","conclusion":"failure","name":"unit-tests"}]});
        assert!(
            attempt_job_with(
                "example/product",
                202,
                3,
                101,
                &"a".repeat(40),
                &mut |endpoint| {
                    assert!(endpoint.contains("/runs/202/attempts/3/jobs?"));
                    Ok(payload.clone())
                }
            )
            .is_ok()
        );
        assert!(
            attempt_job_with(
                "example/product",
                202,
                4,
                101,
                &"a".repeat(40),
                &mut |_| Ok(serde_json::json!({"total_count":0,"jobs":[]}))
            )
            .is_err()
        );
        assert!(
            attempt_job_with(
                "example/product",
                202,
                3,
                101,
                &"b".repeat(40),
                &mut |_| Ok(payload.clone())
            )
            .is_err()
        );
        assert!(
            attempt_job_with("example/product", 202, 3, 101, &"a".repeat(40), &mut |_| {
                Err(error("synthetic API unavailable"))
            })
            .is_err()
        );
    }

    #[test]
    fn ledger_rotation_preserves_history_pending_intents_and_readable_state() {
        let root = env::temp_dir().join(format!(
            "codex-artifact-ledger-{}-{}",
            std::process::id(),
            now().replace(':', "-")
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let path = root.join("ledger.json");
        append_record(
            &root,
            &path,
            "synthetic",
            serde_json::json!({"artifact_id":101,"action":"delete-intent","state":"pending"}),
        )
        .unwrap();
        for _ in 0..4 {
            append_record(
                &root,
                &path,
                "synthetic",
                serde_json::json!({"action":"summary","padding":"x".repeat(90_000)}),
            )
            .unwrap();
            assert!(fs::metadata(&path).unwrap().len() <= MAX_FILE_BYTES);
        }
        let current = read_json_file(&path, "fixture").unwrap();
        assert!(
            current["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["artifact_id"] == 101 && entry["state"] == "pending")
        );
        let archives: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path != &root.join("ledger.json"))
            .collect();
        assert!(!archives.is_empty());
        for archive in archives {
            assert!(read_json_file(&archive, "history fixture").is_ok());
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn live_conflict_or_api_error_never_calls_delete_transport() {
        let requests = std::cell::Cell::new(0);
        for reason in ["synthetic reference changed", "synthetic API unavailable"] {
            assert!(
                verified_delete(
                    || Err(error(reason)),
                    || {
                        requests.set(requests.get() + 1);
                        Ok((204, true))
                    }
                )
                .is_err()
            );
        }
        assert_eq!(requests.get(), 0);
        for response in [
            Ok((500, false)),
            Ok((200, true)),
            Err(error("synthetic transport timeout")),
        ] {
            assert!(
                verified_delete(
                    || Ok(()),
                    || {
                        requests.set(requests.get() + 1);
                        response
                    }
                )
                .is_err()
            );
        }
        assert!(verified_delete(|| Ok(()), || Ok((204, true))).is_ok());
    }

    #[test]
    fn uncertain_rerun_is_persisted_before_post_and_never_repeated() {
        let persisted = std::cell::RefCell::new(serde_json::json!({}));
        let requests = std::cell::Cell::new(0);
        let mut state = serde_json::json!({"phase":"storage-recovered","rerun_count":0});
        assert!(
            rerun_once(
                &mut state,
                |value| {
                    *persisted.borrow_mut() = value.clone();
                    Ok(())
                },
                || {
                    assert_eq!(persisted.borrow()["rerun_count"], 1);
                    assert_eq!(persisted.borrow()["rerun_api_result"], "pending");
                    requests.set(requests.get() + 1);
                    Err(error("synthetic response lost after server accepted"))
                }
            )
            .is_err()
        );
        let mut restarted = persisted.borrow().clone();
        assert!(
            rerun_once(
                &mut restarted,
                |_| Ok(()),
                || {
                    requests.set(requests.get() + 1);
                    Ok((201, true))
                }
            )
            .is_err()
        );
        assert_eq!(requests.get(), 1);
        let mut state = serde_json::json!({"phase":"storage-recovered","rerun_count":0});
        assert!(
            rerun_once(
                &mut state,
                |_| Err(error("synthetic disk failure")),
                || {
                    requests.set(requests.get() + 1);
                    Ok((201, true))
                }
            )
            .is_err()
        );
        assert_eq!(requests.get(), 1);
    }

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn fixture() -> (Artifact, Usage, Clearance, Run, i64) {
        let now = utc_epoch("2026-11-10T00:00:00Z").unwrap();
        let artifact = Artifact {
            id: 17,
            name: "ci-test-results-linux".into(),
            size_in_bytes: 4096,
            expired: false,
            created_at: "2026-10-01T00:00:00Z".into(),
            expires_at: "2027-01-01T00:00:00Z".into(),
            workflow_run: RunRef {
                id: 31,
                head_sha: HEAD.into(),
            },
        };
        let usage = Usage {
            name_prefix: "ci-test-".into(),
            purpose: "ordinary-ci".into(),
            retention_days: None,
        };
        let clearance = Clearance {
            artifact_id: 17,
            run_id: 31,
            head_sha: HEAD.into(),
            references: vec![],
            regenerable: true,
            sole_evidence: false,
            verified_at: "2026-11-09T23:59:00Z".into(),
        };
        let run = Run {
            id: 31,
            head_sha: HEAD.into(),
            status: "completed".into(),
            event: "pull_request".into(),
            path: ".github/workflows/ci.yml".into(),
        };
        (artifact, usage, clearance, run, now)
    }

    #[test]
    fn ordinary_ci_requires_expired_policy_window_and_fresh_exact_clearance() {
        let (artifact, usage, clearance, run, now) = fixture();
        let candidate = classify(
            &artifact,
            Some(&usage),
            Some(&clearance),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        );
        assert!(candidate.eligible);
        assert_eq!(candidate.retention_days, 7);
        let shorter = Usage {
            retention_days: Some(3),
            ..usage.clone()
        };
        assert_eq!(
            classify(
                &artifact,
                Some(&shorter),
                Some(&clearance),
                Some(&run),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .retention_days,
            3
        );
        let repo_override = classify(
            &artifact,
            Some(&usage),
            Some(&clearance),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            Some(60),
            now,
        );
        assert_eq!(repo_override.retention_days, 60);
        assert!(!repo_override.eligible);
    }

    #[test]
    fn protected_use_and_unknown_or_stale_clearance_never_become_deletion_candidates() {
        let (artifact, usage, clearance, run, now) = fixture();
        let mut cases = Vec::new();
        cases.push(classify(
            &artifact,
            None,
            Some(&clearance),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        let protected = Usage {
            purpose: "saved-plan".into(),
            ..usage.clone()
        };
        cases.push(classify(
            &artifact,
            Some(&protected),
            Some(&clearance),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        let mut referenced = clearance.clone();
        referenced.references.push("downstream-workflow".into());
        cases.push(classify(
            &artifact,
            Some(&usage),
            Some(&referenced),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        let mut sole = clearance.clone();
        sole.sole_evidence = true;
        cases.push(classify(
            &artifact,
            Some(&usage),
            Some(&sole),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        let mut stale = clearance.clone();
        stale.verified_at = "2026-11-09T23:50:00Z".into();
        cases.push(classify(
            &artifact,
            Some(&usage),
            Some(&stale),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        let mut not_regenerable = clearance.clone();
        not_regenerable.regenerable = false;
        cases.push(classify(
            &artifact,
            Some(&usage),
            Some(&not_regenerable),
            Some(&run),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            None,
            now,
        ));
        assert!(cases.iter().all(|candidate| !candidate.eligible));
    }

    #[test]
    fn live_pr_ci_deployment_and_api_uncertainty_protect_artifacts() {
        let (artifact, usage, clearance, run, now) = fixture();
        let mut pending = run.clone();
        pending.status = "in_progress".into();
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&clearance),
                Some(&pending),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&clearance),
                None,
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        let open_pr = HashSet::from([HEAD.to_string()]);
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&clearance),
                Some(&run),
                &open_pr,
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        let active_other = HashSet::from([OTHER.to_string()]);
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&clearance),
                Some(&run),
                &HashSet::new(),
                &active_other,
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        let deployed = HashSet::from([HEAD.to_string()]);
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&clearance),
                Some(&run),
                &HashSet::new(),
                &HashSet::new(),
                &deployed,
                None,
                now
            )
            .eligible
        );
    }

    #[test]
    fn malformed_identity_expiry_and_timestamp_are_kept() {
        let (artifact, usage, clearance, run, now) = fixture();
        let mut wrong_id = clearance.clone();
        wrong_id.artifact_id += 1;
        assert!(
            !classify(
                &artifact,
                Some(&usage),
                Some(&wrong_id),
                Some(&run),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        let mut expired = artifact.clone();
        expired.expired = true;
        assert!(
            !classify(
                &expired,
                Some(&usage),
                Some(&clearance),
                Some(&run),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        let mut bad_date = artifact.clone();
        bad_date.created_at = "2026-02-31T00:00:00Z".into();
        assert!(
            !classify(
                &bad_date,
                Some(&usage),
                Some(&clearance),
                Some(&run),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
                now
            )
            .eligible
        );
        assert!(utc_epoch("2026-11-10T00:00:00.123Z").is_ok());
        assert!(utc_epoch("2026-02-31T00:00:00Z").is_err());
        assert!(utc_epoch("2026-11-10T00:00:00.あZ").is_err());
    }

    #[test]
    fn recovery_retry_gate_is_one_shot_and_evidence_is_target_bound() {
        let mut record = object([
            ("repository", string("example/product")),
            ("task_id", string("task-example")),
            ("pr", Value::from(5)),
            ("head_sha", string(HEAD)),
            ("base_sha", string(OTHER)),
            ("plan_id", string("capacity-recovery-v1")),
            ("plan_version", Value::from(1)),
            ("run_id", Value::from(9)),
            ("job_id", Value::from(10)),
        ]);
        let evidence = RecoveryEvidence {
            version: 1,
            repository: "example/product".into(),
            task_id: "task-example".into(),
            pr: 5,
            head_sha: HEAD.into(),
            base_sha: OTHER.into(),
            plan_id: "capacity-recovery-v1".into(),
            plan_version: 1,
            run_id: 9,
            job_id: 10,
            storage_recovered: true,
            source: "synthetic usage dashboard".into(),
            observed_at: "2026-11-10T00:00:00Z".into(),
            product_test_failures: false,
            summary: "synthetic proof".into(),
        };
        assert!(evidence_matches(&evidence, &record));
        record["plan_version"] = Value::from(2);
        assert!(!evidence_matches(&evidence, &record));
    }
}
