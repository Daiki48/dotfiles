//! タスク調整用にGitHub Actionsとcommit statusを読み取り専用で観測する。
//!
//! APIを利用できない場合にCI成功とみなすことはない。

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::codex_tools::delivery::task_gh_json;

#[cfg(test)]
#[path = "ci_tests.rs"]
mod gate_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CiState {
    Pending,
    Success,
    // CI成功ではなく、固定headのlocal検証とreview receiptを要求する状態。
    LocalValidation,
    Failure,
    Missing,
    Stale,
    Cancelled,
    TimedOut,
    Skipped,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum GateMode {
    #[default]
    Strict,
    LocalValidation,
    GithubFreePrivate,
    EmergencyCi,
}

impl GateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::LocalValidation => "local-validation",
            Self::GithubFreePrivate => "github-free-private",
            Self::EmergencyCi => "emergency-ci",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkflowTree {
    commit: String,
    tree: String,
    workflows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkflowEvidence {
    base: WorkflowTree,
    head: WorkflowTree,
    main: Option<WorkflowTree>,
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
    pub observed_at: u64,
    pub required: Vec<RequiredCheck>,
    pub snapshot: Snapshot,
    #[serde(default)]
    pub gate_mode: GateMode,
    #[serde(default)]
    pub workflows: Option<WorkflowEvidence>,
    #[serde(default)]
    pub emergency: Option<Value>,
}

impl Observation {
    pub fn allows_delivery(&self) -> bool {
        if self.gate_mode == GateMode::EmergencyCi {
            return self.state != CiState::Stale
                && self.emergency.as_ref().is_some_and(|proof| {
                    proof["approved"] == true
                        && proof["head"] == self.head
                        && proof["base"] == self.base
                        && proof["target"] == self.target
                        && proof["merged"] == self.merged
                        && proof["receipt_sha256"].as_str().is_some_and(|digest| {
                            digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit())
                        })
                        && if self.merged {
                            self.merge_commit.as_ref() == Some(&self.target)
                        } else {
                            proof["approved_base"] == self.base && self.target == self.head
                        }
                });
        }
        match (self.gate_mode, self.state) {
            (GateMode::Strict, CiState::Success) => true,
            (GateMode::GithubFreePrivate, CiState::Success) => {
                free_private_actions_success(&self.snapshot, &self.target)
            }
            (GateMode::LocalValidation, CiState::LocalValidation) => {
                self.workflows.as_ref().is_some_and(|evidence| {
                    evidence.base.commit == self.base
                        && evidence.head.commit == self.head
                        && evidence.base.workflows.is_empty()
                        && evidence.head.workflows.is_empty()
                        && if self.merged {
                            evidence.main.as_ref().is_some_and(|main| {
                                main.commit == self.target && main.workflows.is_empty()
                            })
                        } else {
                            self.target == self.head && evidence.main.is_none()
                        }
                })
            }
            _ => false,
        }
    }
}

pub(super) fn emergency_observation(proof: Value) -> Result<Observation, String> {
    let field = |key: &str| {
        proof[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "緊急CI観測が不正です".to_string())
    };
    let head = field("head")?;
    let base = field("base")?;
    let target = field("target")?;
    for sha in [&head, &base, &target] {
        validate_sha(sha)?;
    }
    let merged = proof["merged"].as_bool().ok_or("緊急CI観測が不正です")?;
    let entries = proof["target_ci"]["entries"]
        .as_array()
        .ok_or("緊急CI観測が不正です")?;
    // failure/pending/missingを成功へ変換せず、承認証拠を別fieldへ保持する。
    let states: Vec<_> = entries
        .iter()
        .map(|entry| entry["state"].as_str().unwrap_or(""))
        .collect();
    if states.is_empty()
        || states.iter().any(|state| {
            !matches!(
                *state,
                "success"
                    | "skipped"
                    | "neutral"
                    | "failure"
                    | "error"
                    | "startup_failure"
                    | "timed_out"
                    | "cancelled"
                    | "missing"
                    | "queued"
                    | "in_progress"
                    | "pending"
                    | "requested"
                    | "waiting"
            )
        })
    {
        return Err("緊急CIの実測が不足または不明です".into());
    }
    let state = if states
        .iter()
        .any(|state| matches!(*state, "failure" | "error" | "startup_failure"))
    {
        CiState::Failure
    } else if states.contains(&"timed_out") {
        CiState::TimedOut
    } else if states.contains(&"cancelled") {
        CiState::Cancelled
    } else if states.contains(&"missing") {
        CiState::Missing
    } else if !states.contains(&"success")
        && states
            .iter()
            .all(|state| matches!(*state, "skipped" | "neutral"))
    {
        CiState::Skipped
    } else if states
        .iter()
        .all(|state| matches!(*state, "success" | "skipped" | "neutral"))
    {
        CiState::Success
    } else {
        CiState::Pending
    };
    let fingerprint = format!(
        "emergency:{:x}",
        Sha256::digest(serde_json::to_vec(&proof).map_err(|_| "緊急CI観測が不正です")?)
    );
    let observation = Observation {
        state,
        head,
        base,
        target: target.clone(),
        fingerprint,
        merge_commit: merged.then_some(target),
        merged,
        observed_at: super::store::now(),
        required: vec![],
        snapshot: Snapshot {
            runs: vec![],
            statuses: vec![],
        },
        gate_mode: GateMode::EmergencyCi,
        workflows: None,
        emergency: Some(proof),
    };
    if !observation.allows_delivery() {
        return Err("固定deliveryの緊急承認が不足しています".into());
    }
    Ok(observation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RequiredCheck {
    context: String,
    integration_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CheckRun {
    id: i64,
    name: String,
    head_sha: String,
    status: String,
    conclusion: Option<String>,
    app_id: Option<i64>,
    app_slug: Option<String>,
    check_suite_id: Option<i64>,
    html_url: Option<String>,
    details_url: Option<String>,
    run_id: Option<i64>,
    run_attempt: Option<i64>,
    workflow_run: Option<WorkflowRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CommitStatus {
    id: i64,
    context: String,
    state: String,
    sha: String,
    updated_at: String,
    creator_id: Option<i64>,
    creator_login: Option<String>,
    url: Option<String>,
    target_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    runs: Vec<CheckRun>,
    statuses: Vec<CommitStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkflowRun {
    id: i64,
    run_attempt: i64,
    check_suite_id: i64,
    head_sha: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PullRequest {
    head: String,
    base: String,
    base_ref: String,
    merge_commit: Option<String>,
    merged: bool,
}

/// 現在のPR、または`main`指定時は実merge commitを指定gateで観測する。
///
/// API・認証・rate limitの失敗はエラーとして返し、`Missing`や成功には置き換えない。
pub(super) fn observe(
    root: &Path,
    repository: &str,
    pr: u64,
    main: Option<GateMode>,
    policy: GateMode,
) -> Result<Observation, String> {
    observe_with_policy(repository, pr, main, policy, &mut |args| {
        task_gh_json(root, args)
    })
}

pub(super) fn observe_repair(
    root: &Path,
    repository: &str,
    original_merge: &str,
    pr: u64,
    head: &str,
    main: GateMode,
    policy: GateMode,
) -> Result<Observation, String> {
    observe_repair_with(
        repository,
        original_merge,
        pr,
        head,
        main,
        policy,
        &mut |args| task_gh_json(root, args),
    )
}

type Fetch<'a> = dyn FnMut(&[String]) -> Result<Value, String> + 'a;

#[cfg(test)]
fn observe_with(
    repository: &str,
    pr: u64,
    main: Option<GateMode>,
    fetch: &mut Fetch<'_>,
) -> Result<Observation, String> {
    // 既存fixture用の互換wrapper。main receiptがなければ従来の自動選択を保つ。
    observe_with_selection(repository, pr, main, main, fetch)
}

fn observe_with_policy(
    repository: &str,
    pr: u64,
    main: Option<GateMode>,
    policy: GateMode,
    fetch: &mut Fetch<'_>,
) -> Result<Observation, String> {
    if policy == GateMode::EmergencyCi || main == Some(GateMode::EmergencyCi) {
        return Err("緊急CI観測には対象taskの承認receiptが必要です".into());
    }
    // Strict is the default policy: workflow-free repositories retain the local gate.
    // Preserve an existing local receipt, but never reinterpret a FreePrivate receipt.
    let selection = if policy == GateMode::Strict {
        if main == Some(GateMode::GithubFreePrivate) {
            return Err("github-free-private receiptをstrict policyへ変更できません".into());
        }
        None
    } else {
        Some(policy)
    };
    observe_with_selection(repository, pr, main, selection, fetch)
}

fn observe_with_selection(
    repository: &str,
    pr: u64,
    main: Option<GateMode>,
    policy: Option<GateMode>,
    fetch: &mut Fetch<'_>,
) -> Result<Observation, String> {
    validate_repository(repository)?;
    if pr == 0 {
        return Err("PR番号は1以上である必要があります".into());
    }
    if main.is_some_and(|receipt| policy.is_some_and(|policy| receipt != policy)) {
        return Err("mainのCI gate modeが受付policyと一致しません".into());
    }

    let requested_mode = main.or(policy);
    if requested_mode == Some(GateMode::GithubFreePrivate) {
        validate_github_free_private_repository(repository, fetch)?;
    }

    let before = fetch_pull_request(
        repository,
        pr,
        requested_mode == Some(GateMode::GithubFreePrivate),
        fetch,
    )?;
    let base = workflow_tree(repository, &before.base, fetch)?;
    let head = workflow_tree(repository, &before.head, fetch)?;
    let workflow_free = base.workflows.is_empty() && head.workflows.is_empty();
    let gate_mode = if let Some(gate) = requested_mode {
        // merge前に選んだreceiptのgateを保持し、mainの変化で切り替えない。
        if gate == GateMode::LocalValidation && !workflow_free {
            return Err("local-validationのbase/headにworkflowが存在します".into());
        }
        gate
    } else if workflow_free {
        GateMode::LocalValidation
    } else {
        GateMode::Strict
    };
    let rules = if gate_mode == GateMode::GithubFreePrivate {
        // FreePrivateはRules APIに依存しない。403などのAPI失敗を別modeへ変換しない。
        Vec::new()
    } else {
        required_checks(repository, &before.base_ref, fetch)?
    };
    let (target, snapshot, main_tree) = if main.is_some() {
        let target = before
            .merge_commit
            .clone()
            .filter(|_| before.merged)
            .ok_or("merge実体を確認できません")?;
        // merge済みフラグだけでなく、対象commitがlive mainに到達済みか確認する。
        let comparison = get(
            fetch,
            format!("repos/{repository}/compare/{target}...{}", before.base),
        )?;
        if nested_sha(
            &comparison,
            &["merge_base_commit", "sha"],
            "mainのmerge base",
        )? != target
        {
            return Err("merge commitがlive mainへ到達していません".into());
        }
        let tree = workflow_tree(repository, &target, fetch)?;
        let snapshot = fetch_snapshot(repository, &target, fetch)?;
        (target, snapshot, Some(tree))
    } else {
        let head_snapshot = fetch_snapshot(repository, &before.head, fetch)?;
        let merge_snapshot = before
            .merge_commit
            .as_deref()
            .filter(|candidate| gate_mode == GateMode::Strict && *candidate != before.head)
            .map(|candidate| fetch_snapshot(repository, candidate, fetch))
            .transpose()?;
        let (target, snapshot) = if matches!(
            gate_mode,
            GateMode::LocalValidation | GateMode::GithubFreePrivate
        ) {
            // local/free-privateはtest mergeで固定headの証跡を隠さない。
            (before.head.clone(), head_snapshot)
        } else {
            select_target(
                &before.head,
                before.merge_commit.as_deref(),
                head_snapshot,
                merge_snapshot,
            )
        };
        (target, snapshot, None)
    };

    // rules/check/statusを個別に照会した後、PRを再取得する。
    // 観測中にGitHub上のPRが変わっていたら、このsnapshotを信頼しない。
    let after = fetch_pull_request(
        repository,
        pr,
        requested_mode == Some(GateMode::GithubFreePrivate),
        fetch,
    )?;
    if gate_mode == GateMode::GithubFreePrivate {
        // 観測中にprivate/merge policyが変わっていないことも確認する。
        validate_github_free_private_repository(repository, fetch)?;
    }
    let changed = before != after;
    let mut state = if changed {
        CiState::Stale
    } else {
        classify(&rules, &snapshot.runs, &snapshot.statuses, &target)
    };
    if gate_mode == GateMode::GithubFreePrivate
        && state == CiState::Success
        && !free_private_actions_success(&snapshot, &target)
    {
        state = if has_only_skipped_or_neutral_checks(&snapshot, &target) {
            CiState::Skipped
        } else {
            CiState::Missing
        };
    }
    if state == CiState::Success
        && snapshot
            .runs
            .iter()
            .filter_map(|run| run.workflow_run.as_ref())
            .any(|run| {
                run.status != "completed"
                    || !matches!(
                        run.conclusion.as_deref(),
                        Some("success" | "skipped" | "neutral")
                    )
            })
    {
        return Err("checkとActions runの成功証跡が一致しません。再観測します".into());
    }
    let workflows = WorkflowEvidence {
        base,
        head,
        main: main_tree,
    };
    if gate_mode == GateMode::LocalValidation
        && !changed
        && workflows
            .main
            .as_ref()
            .is_none_or(|tree| tree.workflows.is_empty())
        && (state == CiState::Success
            || (state == CiState::Missing && rules.is_empty() && !contains_any_status(&snapshot)))
    {
        state = CiState::LocalValidation;
    }
    let fingerprint = format!(
        "{}:{:x}",
        snapshot_fingerprint(&after, &target, state == CiState::Stale, &snapshot),
        Sha256::digest(
            serde_json::to_vec(&(gate_mode, &workflows))
                .expect("workflow evidence is serializable")
        )
    );
    Ok(Observation {
        state,
        head: after.head,
        base: after.base,
        target,
        fingerprint,
        merge_commit: after.merge_commit,
        merged: after.merged,
        observed_at: super::store::now(),
        required: rules,
        snapshot,
        gate_mode,
        workflows: Some(workflows),
        emergency: None,
    })
}

fn observe_repair_with(
    repository: &str,
    original_merge: &str,
    pr: u64,
    head: &str,
    main: GateMode,
    policy: GateMode,
    fetch: &mut Fetch<'_>,
) -> Result<Observation, String> {
    validate_sha(original_merge).map_err(|_| "元merge SHAが不正です".to_string())?;
    validate_sha(head).map_err(|_| "修復PR head SHAが不正です".to_string())?;
    let original_merge = original_merge.to_ascii_lowercase();
    let head = head.to_ascii_lowercase();
    let observation = observe_with_policy(repository, pr, Some(main), policy, fetch)?;

    if observation.head != head {
        return Err("修復PR headが固定したchild SHAと一致しません".into());
    }
    if !observation.merged
        || observation.merge_commit.as_deref() != Some(observation.target.as_str())
        || observation.target == original_merge
    {
        return Err("修復PRの実mergeを確認できません".into());
    }
    if !observation.allows_delivery() {
        return Err("修復mergeのCI証跡が成功していません".into());
    }

    let comparison = get(
        fetch,
        format!(
            "repos/{repository}/compare/{original_merge}...{}",
            observation.target
        ),
    )?;
    if nested_sha(
        &comparison,
        &["merge_base_commit", "sha"],
        "元mergeと修復mergeのmerge base",
    )? != original_merge
    {
        return Err("修復mergeが元mergeの子孫ではありません".into());
    }
    Ok(observation)
}

fn fetch_pull_request(
    repository: &str,
    pr: u64,
    require_same_repository: bool,
    fetch: &mut Fetch<'_>,
) -> Result<PullRequest, String> {
    let value = fetch(&[
        "api".into(),
        format!("repos/{repository}/pulls/{pr}"),
        "--method".into(),
        "GET".into(),
    ])?;
    if require_same_repository
        && (!repository_identity_matches(value.pointer("/head/repo/full_name"), repository)
            || !repository_identity_matches(value.pointer("/base/repo/full_name"), repository))
    {
        return Err("github-free-privateのPR head/base repository identityが一致しません".into());
    }
    let base_ref = nested_text(&value, &["base", "ref"], "PR base branch")?;
    // PR payloadのbase.shaはmerge後も古いことがある。対象branchの実refを正本にする。
    let reference = get(
        fetch,
        format!(
            "repos/{repository}/git/ref/heads/{}",
            encoded_path_component(&base_ref)
        ),
    )?;
    if reference.get("ref").and_then(Value::as_str)
        != Some(format!("refs/heads/{base_ref}").as_str())
        || reference.pointer("/object/type").and_then(Value::as_str) != Some("commit")
    {
        return Err("live base refを確認できません".into());
    }
    Ok(PullRequest {
        head: nested_sha(&value, &["head", "sha"], "PR head")?,
        base: nested_sha(&reference, &["object", "sha"], "live base")?,
        base_ref,
        merge_commit: optional_sha(value.get("merge_commit_sha"), "merge commit")?,
        merged: value
            .get("merged")
            .and_then(Value::as_bool)
            .ok_or_else(|| "GitHub PRのmerged状態を確認できません".to_string())?,
    })
}

fn repository_identity_matches(value: Option<&Value>, repository: &str) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|identity| identity.eq_ignore_ascii_case(repository))
}

fn validate_github_free_private_repository(
    repository: &str,
    fetch: &mut Fetch<'_>,
) -> Result<(), String> {
    let value = get(fetch, format!("repos/{repository}"))?;
    let expected: [(&str, Value); 8] = [
        ("full_name", Value::String(repository.to_owned())),
        ("private", Value::Bool(true)),
        ("visibility", Value::String("private".into())),
        ("default_branch", Value::String("main".into())),
        ("archived", Value::Bool(false)),
        ("disabled", Value::Bool(false)),
        ("allow_merge_commit", Value::Bool(true)),
        ("allow_auto_merge", Value::Bool(false)),
    ];
    if expected
        .iter()
        .any(|(key, expected)| value.get(*key) != Some(expected))
    {
        return Err("github-free-privateのlive repository policyが一致しません".into());
    }
    Ok(())
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

fn get(fetch: &mut Fetch<'_>, endpoint: String) -> Result<Value, String> {
    fetch(&["api".into(), endpoint, "--method".into(), "GET".into()])
}

fn workflow_tree(
    repository: &str,
    sha: &str,
    fetch: &mut Fetch<'_>,
) -> Result<WorkflowTree, String> {
    validate_sha(sha)?;
    let commit = get(fetch, format!("repos/{repository}/git/commits/{sha}"))?;
    if nested_sha(&commit, &["sha"], "workflow対象commit")? != sha {
        return Err("workflow対象commitが一致しません".into());
    }
    let tree = nested_sha(&commit, &["tree", "sha"], "workflow対象tree")?;
    let value = get(
        fetch,
        format!("repos/{repository}/git/trees/{tree}?recursive=1"),
    )?;
    if nested_sha(&value, &["sha"], "workflow tree")? != tree
        || value.get("truncated").and_then(Value::as_bool) != Some(false)
    {
        return Err("workflow treeが不完全または一致しません".into());
    }
    let entries = value
        .get("tree")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() <= 100_000)
        .ok_or("workflow treeの応答形式が不正です")?;
    let mut workflows = Vec::new();
    for entry in entries {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or("workflow treeのpathが不正です")?;
        // deliveryと同じ直下のYAML判定を使う。symlink等も不在には変換しない。
        if crate::codex_tools::delivery::workflow_yaml_path(path) {
            workflows.push(path.to_owned());
        }
    }
    workflows.sort();
    Ok(WorkflowTree {
        commit: sha.into(),
        tree,
        workflows,
    })
}

fn required_checks(
    repository: &str,
    branch: &str,
    fetch: &mut Fetch<'_>,
) -> Result<Vec<RequiredCheck>, String> {
    if branch.is_empty() || branch.len() > 256 || branch.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err("対象branchを安全に確認できません".into());
    }
    let endpoint = format!(
        "repos/{repository}/rules/branches/{}",
        encoded_path_component(branch)
    );
    let value = fetch(&["api".into(), endpoint, "--method".into(), "GET".into()])?;
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

fn fetch_snapshot(repository: &str, sha: &str, fetch: &mut Fetch<'_>) -> Result<Snapshot, String> {
    validate_sha(sha)?;
    let checks = fetch(&[
        "api".into(),
        format!("repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"),
        "--paginate".into(),
        "--slurp".into(),
        "--method".into(),
        "GET".into(),
    ])?;
    let mut runs = parse_check_runs(&checks, sha)?;
    let suites: std::collections::BTreeSet<_> = runs
        .iter()
        .filter(|run| run.app_slug.as_deref() == Some("github-actions"))
        .filter_map(|run| run.check_suite_id)
        .collect();
    for suite in suites {
        // check_suite.idはActions run IDではない。公式APIで対応するrunとattemptを取得する。
        let workflows = fetch(&[
            "api".into(),
            format!("repos/{repository}/actions/runs?check_suite_id={suite}&per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
            "--method".into(),
            "GET".into(),
        ])?;
        attach_workflow_run(&mut runs, suite, &workflows)?;
    }
    let statuses = fetch(&[
        "api".into(),
        format!("repos/{repository}/commits/{sha}/statuses?per_page=100"),
        "--paginate".into(),
        "--slurp".into(),
        "--method".into(),
        "GET".into(),
    ])?;
    let statuses = parse_statuses(&statuses, sha)?;
    Ok(Snapshot { runs, statuses })
}

fn optional_text(value: Option<&Value>, max: usize) -> Result<Option<String>, String> {
    value
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .filter(|text| text.len() <= max && !text.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or_else(|| "CI証跡の文字列が不正です".into())
        })
        .transpose()
}

fn attach_workflow_run(runs: &mut [CheckRun], suite: i64, value: &Value) -> Result<(), String> {
    let mut observed = Vec::new();
    let mut expected = None;
    for page in pages(value, "workflow runs")? {
        let total = page
            .get("total_count")
            .and_then(Value::as_u64)
            .filter(|total| *total < 1000)
            .ok_or("workflow run総数を確認できません")?;
        if expected.is_some_and(|previous| previous != total) {
            return Err("workflow run取得中に総数が変化しました".into());
        }
        expected = Some(total);
        let entries = page
            .get("workflow_runs")
            .and_then(Value::as_array)
            .ok_or("workflow runの形式が不正です")?;
        for entry in entries {
            let run: WorkflowRun = serde_json::from_value(json!({
                "id": entry.get("id"), "run_attempt": entry.get("run_attempt"),
                "check_suite_id": entry.get("check_suite_id"), "head_sha":entry.get("head_sha"),
                "status":entry.get("status"), "conclusion":entry.get("conclusion"),
                "html_url":optional_text(entry.get("html_url"), 4096)?,
            }))
            .map_err(|_| "workflow runの識別情報が不正です")?;
            validate_sha(&run.head_sha)?;
            if run.id <= 0 || run.run_attempt <= 0 || run.check_suite_id != suite {
                return Err("workflow runの識別情報が一致しません".into());
            }
            observed.push(run);
        }
    }
    if expected != Some(observed.len() as u64) || observed.len() != 1 {
        return Err("workflow runを一意に確認できません".into());
    }
    if let Some(workflow) = observed.pop() {
        for run in runs.iter_mut().filter(|run| {
            run.check_suite_id == Some(suite) && run.app_slug.as_deref() == Some("github-actions")
        }) {
            run.run_id = Some(workflow.id);
            run.run_attempt = Some(workflow.run_attempt);
            run.workflow_run = Some(workflow.clone());
        }
    }
    Ok(())
}

fn pages<'a>(value: &'a Value, key: &str) -> Result<Vec<&'a Value>, String> {
    let pages = value
        .as_array()
        .ok_or_else(|| format!("{key}応答のページ形式が不正です"))?;
    // 上限に達した場合、最終ページが満杯だと次ページの有無を証明できない。
    // 不完全な結果を現状の全件として扱わず、安全側に倒す。
    if pages.is_empty() {
        return Err(format!("{key}応答にページがありません"));
    }
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
            let check_suite_id = run
                .get("check_suite")
                .and_then(|suite| suite.get("id"))
                .and_then(Value::as_i64);
            parsed.push(CheckRun {
                id,
                name: name.to_owned(),
                head_sha: head_sha.to_ascii_lowercase(),
                status: status.to_owned(),
                conclusion,
                app_id,
                app_slug: optional_text(run.get("app").and_then(|app| app.get("slug")), 256)?,
                check_suite_id,
                html_url: optional_text(run.get("html_url"), 4096)?,
                details_url: optional_text(run.get("details_url"), 4096)?,
                run_id: None,
                run_attempt: None,
                workflow_run: None,
            });
        }
    }
    // 古い応答を空のcheck一覧へ変換するとlocal gateで通過してしまう。
    if parsed.iter().any(|run| run.head_sha != sha) {
        return Err("check-runsの対象SHAが一致しません".into());
    }
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
                creator_id: status
                    .get("creator")
                    .and_then(|creator| creator.get("id"))
                    .and_then(Value::as_i64),
                creator_login: optional_text(
                    status
                        .get("creator")
                        .and_then(|creator| creator.get("login")),
                    256,
                )?,
                url: optional_text(status.get("url"), 4096)?,
                target_url: optional_text(status.get("target_url"), 4096)?,
            });
        }
    }
    Ok(parsed)
}

fn contains_any_status(snapshot: &Snapshot) -> bool {
    !snapshot.runs.is_empty() || !snapshot.statuses.is_empty()
}

fn free_private_actions_success(snapshot: &Snapshot, target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    snapshot.runs.iter().any(|check| {
        check.head_sha == target
            && check.app_slug.as_deref() == Some("github-actions")
            && check.status == "completed"
            && check.conclusion.as_deref() == Some("success")
            && check.check_suite_id.is_some()
            && check.workflow_run.as_ref().is_some_and(|run| {
                run.head_sha.to_ascii_lowercase() == target
                    && run.status == "completed"
                    && run.conclusion.as_deref() == Some("success")
                    && check.check_suite_id == Some(run.check_suite_id)
                    && check.run_id == Some(run.id)
                    && check.run_attempt == Some(run.run_attempt)
            })
    })
}

fn has_only_skipped_or_neutral_checks(snapshot: &Snapshot, target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    let runs: Vec<_> = snapshot
        .runs
        .iter()
        .filter(|run| run.head_sha == target)
        .collect();
    !runs.is_empty()
        && runs.iter().all(|run| {
            run.status == "completed"
                && matches!(run.conclusion.as_deref(), Some("skipped" | "neutral"))
        })
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
        "check_runs": runs,
        "statuses": statuses,
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

    #[test]
    fn evidence_preserves_provider_urls_and_real_workflow_run_identity() {
        let value = json!([{"total_count":1,"check_runs":[{
            "id":10,"name":"ci","head_sha":MERGE,"status":"completed","conclusion":"success",
            "app":{"id":15368,"slug":"github-actions"},"check_suite":{"id":4},
            "html_url":"https://github.com/example/product/runs/10",
            "details_url":"https://github.com/example/product/actions/runs/99/job/10"
        }]}]);
        let mut runs = parse_check_runs(&value, MERGE).unwrap();
        assert_eq!(runs[0].check_suite_id, Some(4));
        assert_eq!(runs[0].run_id, None);
        let workflows = json!([{"total_count":1,"workflow_runs":[{
            "id":99,"run_attempt":2,"check_suite_id":4,"head_sha":HEAD,
            "status":"completed","conclusion":"success",
            "html_url":"https://github.com/example/product/actions/runs/99"
        }]}]);
        attach_workflow_run(&mut runs, 4, &workflows).unwrap();
        assert_eq!(runs[0].run_id, Some(99));
        assert_eq!(runs[0].run_attempt, Some(2));
        assert_eq!(runs[0].workflow_run.as_ref().unwrap().head_sha, HEAD);
        let statuses = parse_statuses(
            &json!([[{
                "id":3,"context":"external","state":"success","updated_at":"2026-11-04T00:00:00Z",
                "creator":{"id":123,"login":"ci-provider"},
                "url":"https://api.github.com/repos/example/product/statuses/3",
                "target_url":"https://ci.example.test/results/3"
            }]]),
            MERGE,
        )
        .unwrap();
        assert_eq!(statuses[0].creator_id, Some(123));
        assert_eq!(statuses[0].creator_login.as_deref(), Some("ci-provider"));
        assert_eq!(statuses[0].sha, MERGE);
        let observation = Observation {
            state: CiState::Success,
            head: HEAD.into(),
            base: STALE.into(),
            target: MERGE.into(),
            fingerprint: "test".into(),
            merge_commit: Some(MERGE.into()),
            merged: false,
            observed_at: 42,
            required: vec![required("ci", Some(15368))],
            snapshot: Snapshot { runs, statuses },
            gate_mode: GateMode::Strict,
            workflows: None,
            emergency: None,
        };
        let saved = serde_json::to_value(&observation).unwrap();
        assert_eq!(saved["snapshot"]["runs"][0]["run_id"], 99);
        assert_eq!(
            saved["snapshot"]["runs"][0]["html_url"],
            "https://github.com/example/product/runs/10"
        );
        assert_eq!(
            saved["snapshot"]["statuses"][0]["target_url"],
            "https://ci.example.test/results/3"
        );
        assert_eq!(
            serde_json::from_value::<Observation>(saved).unwrap(),
            observation
        );
        let mut wrong = workflows;
        wrong[0]["workflow_runs"][0]["check_suite_id"] = json!(5);
        assert!(attach_workflow_run(&mut [], 4, &wrong).is_err());
        assert!(
            attach_workflow_run(&mut [], 4, &json!([{"total_count":1,"workflow_runs":[]}]))
                .is_err()
        );
    }

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
            app_slug: Some("github-actions".into()),
            check_suite_id: Some(4),
            html_url: None,
            details_url: None,
            run_id: Some(7),
            run_attempt: Some(1),
            workflow_run: None,
        }
    }

    fn status(context: &str, sha: &str, state: &str) -> CommitStatus {
        CommitStatus {
            id: 1,
            context: context.into(),
            state: state.into(),
            sha: sha.into(),
            updated_at: "2026-11-04T00:00:00Z".into(),
            creator_id: None,
            creator_login: None,
            url: None,
            target_url: None,
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
                creator_id: None,
                creator_login: None,
                url: None,
                target_url: None,
            },
            CommitStatus {
                id: 2,
                context: "ci".into(),
                state: "success".into(),
                sha: HEAD.into(),
                // timestampが前後してもAPI順とIDから最新statusを選ぶ。
                updated_at: "2026-11-02T00:00:00Z".into(),
                creator_id: None,
                creator_login: None,
                url: None,
                target_url: None,
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
        assert!(parse_check_runs(&value, HEAD).is_err());

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
