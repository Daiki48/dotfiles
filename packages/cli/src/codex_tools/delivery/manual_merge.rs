use super::*;
use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "manual_merge_tests.rs"]
mod tests;

pub(super) const STATE_VERSION: i64 = 3;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    version: i64,
    approved: bool,
    repository: String,
    task_id: String,
    pr: i64,
    plan_id: String,
    plan_version: i64,
    head_sha: String,
    base_sha: String,
    merge_sha: String,
}

impl Approval {
    fn validate(&self, receipt: &Value) -> Result<()> {
        if self.version != 1
            || !self.approved
            || int_value(receipt, "version")? != RECEIPT_VERSION
            || self.repository != str_value(receipt, "repository")?
            || self.task_id != str_value(receipt, "task_id")?
        {
            return Err(error("手動merge回収の承認対象またはreceiptが一致しません"));
        }
        match_cli_receipt(
            receipt,
            Some(self.pr),
            Some(&self.head_sha),
            Some(&self.plan_id),
            Some(self.plan_version),
            STRICT_GATE_MODE,
        )?;
        oid(&self.base_sha, "承認済みbase")?;
        oid(&self.merge_sha, "承認済みmerge")?;
        if self.base_sha == self.head_sha {
            return Err(error("手動merge回収のbaseとheadが同じです"));
        }
        Ok(())
    }

    fn verify_pr(&self, view: &Value, branch: &str) -> Result<()> {
        assert_recovery_pr(view, &self.repository, branch, &self.head_sha)?;
        if str_value(get(view, "mergeCommit")?, "oid")? != self.merge_sha {
            return Err(error("PRの実mergeが承認対象と一致しません"));
        }
        Ok(())
    }

    fn verify_merge(&self, root: &Path) -> Result<()> {
        emergency::verify_merge_tree(root, &self.base_sha, &self.head_sha, &self.merge_sha)?;
        let message = git(root, &["show", "-s", "--format=%B", &self.merge_sha], true)?;
        if !message.contains("[skip ci]")
            || !emergency::git_ancestor(root, &self.base_sha, &self.head_sha)?
        {
            return Err(error(
                "実mergeのskip指定または検証済みheadのbaseが一致しません",
            ));
        }
        Ok(())
    }

    fn verify_main(&self, root: &Path) -> Result<String> {
        let live = synced_main(root, &self.repository)?;
        if !emergency::git_ancestor(root, &self.merge_sha, &live)? {
            return Err(error("承認済みmergeがlive mainに含まれていません"));
        }
        if live != self.merge_sha {
            check_required_ci(root, &self.repository, &live)?;
        }
        Ok(live)
    }

    fn observe_ci(&self, root: &Path) -> Result<[CiObservation; 2]> {
        let mut fetch = |endpoint: &str| emergency::ci_pages(root, endpoint);
        Ok([
            observe_ci(&self.repository, &self.head_sha, &mut fetch)?,
            observe_ci(&self.repository, &self.merge_sha, &mut fetch)?,
        ])
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CiObservation {
    workflow_runs: usize,
    actions_check_runs: usize,
    other_check_runs: usize,
    commit_statuses: usize,
}

fn observe_ci(
    repository: &str,
    sha: &str,
    fetch: &mut impl FnMut(&str) -> Result<Value>,
) -> Result<CiObservation> {
    let workflows = emergency::pages(
        &fetch(&format!(
            "repos/{repository}/actions/runs?head_sha={sha}&per_page=100"
        ))?,
        Some("workflow_runs"),
    )?;
    let checks = emergency::pages(
        &fetch(&format!(
            "repos/{repository}/commits/{sha}/check-runs?filter=all&per_page=100"
        ))?,
        Some("check_runs"),
    )?;
    let statuses = emergency::pages(
        &fetch(&format!(
            "repos/{repository}/commits/{sha}/statuses?per_page=100"
        ))?,
        None,
    )?;
    for run in workflows.iter().chain(&checks) {
        if str_value(run, "head_sha")? != sha
            || str_value(run, "status")? != "completed"
            || !matches!(
                str_value(run, "conclusion")?.as_str(),
                "success" | "skipped" | "neutral"
            )
        {
            return Err(error("手動merge回収に失敗・未完了・不明なCIが存在します"));
        }
    }
    let mut actions_checks = 0;
    for check in &checks {
        let app = int_value(get(check, "app")?, "id")?;
        if app < 1 || str_value(check, "completed_at")?.is_empty() {
            return Err(error("手動merge回収のcheck提供元または完了時刻が不明です"));
        }
        actions_checks += usize::from(app == 15368);
    }
    for status in &statuses {
        if str_value(status, "state")? != "success" {
            return Err(error("手動merge回収に成功以外のcommit statusが存在します"));
        }
    }
    Ok(CiObservation {
        workflow_runs: workflows.len(),
        actions_check_runs: actions_checks,
        other_check_runs: checks.len() - actions_checks,
        commit_statuses: statuses.len(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recovery {
    approval: Approval,
    receipt_sha256: String,
    observed_at: String,
    head_ci: CiObservation,
    merge_ci: CiObservation,
}

impl Recovery {
    pub(super) fn from_state(state: &Value, receipt: &Value) -> Result<Self> {
        let recovery: Self = serde_json::from_value(get(state, "manual_merge")?.clone())
            .map_err(|_| error("手動merge回収の記録が不正です"))?;
        recovery.approval.validate(receipt)?;
        if recovery.receipt_sha256 != emergency::digest(receipt)?
            || str_value(state, "stage")? == "merge_started"
            || str_value(state, "stage")? == "merged"
        {
            return Err(error("手動merge回収のreceiptまたはstageが変更されました"));
        }
        Ok(recovery)
    }

    pub(super) fn verify(&self, root: &Path, view: &Value, branch: &str) -> Result<String> {
        self.verify_pr(view, branch)?;
        private_repository(root, &self.approval.repository)?;
        let live = self.approval.verify_main(root)?;
        self.approval.verify_merge(root)?;
        self.approval.observe_ci(root)?;
        Ok(live)
    }

    pub(super) fn verify_pr(&self, view: &Value, branch: &str) -> Result<()> {
        self.approval.verify_pr(view, branch)
    }

    pub(super) fn note(&self) -> String {
        format!(
            "対象別承認により手動マージ済みの PR を回収。承認時の Actions 観測は PR head が workflow {} 件 / check {} 件、実マージが workflow {} 件 / check {} 件。",
            self.head_ci.workflow_runs,
            self.head_ci.actions_check_runs,
            self.merge_ci.workflow_runs,
            self.merge_ci.actions_check_runs,
        )
    }
}

fn private_repository(root: &Path, repository: &str) -> Result<()> {
    let value = gh_json(root, &["api".into(), format!("repos/{repository}")])?;
    if !str_value(&value, "full_name")?.eq_ignore_ascii_case(repository)
        || value.get("private") != Some(&Value::Bool(true))
        || str_value(&value, "visibility")? != "private"
        || str_value(&value, "default_branch")? != "main"
        || value.get("archived") != Some(&Value::Bool(false))
        || value.get("disabled") != Some(&Value::Bool(false))
    {
        return Err(error(
            "手動merge回収のlive private repositoryが一致しません",
        ));
    }
    Ok(())
}

pub(super) fn recover(root: &Path, args: &CliArgs, path: &Path) -> Result<Value> {
    with_deadline(|| {
        let repository = repository(root)?;
        let _lock = task_lock(&repository, &args.task)?;
        let (manifest, worktree_path) = manifest(root, &args.task, &repository)?;
        let branch = str_value(&manifest, "branch")?;
        let receipt = load_receipt(root, &args.task, &args.head, Some(&repository))?;
        match_cli_receipt(
            &receipt,
            Some(args.pr),
            Some(&args.head),
            Some(&args.plan),
            Some(args.plan_version),
            STRICT_GATE_MODE,
        )?;
        let approval: Approval =
            serde_json::from_value(external_private_json(path, "手動merge回収の承認")?)
                .map_err(|_| error("手動merge回収の承認schemaが不正です"))?;
        approval.validate(&receipt)?;
        let state_path = state_path(&repository, &args.task)?;
        if state_path.exists() || state_path.is_symlink() {
            return Err(error(
                "delivery stateが既に存在します。既存finish経路で再開してください",
            ));
        }
        worktree(root, &manifest, &worktree_path)?;
        worktree_clean_head(&worktree_path, &args.head)?;
        validate_receipt_evidence(&worktree_path, &manifest, &receipt)?;
        approval.verify_pr(&pr_view(root, &repository, args.pr)?, &branch)?;
        private_repository(root, &repository)?;
        let live = approval.verify_main(root)?;
        approval.verify_merge(root)?;
        if !emergency::git_ancestor(root, &str_value(&manifest, "base_oid")?, &approval.base_sha)? {
            return Err(error("実mergeのbaseがmanaged taskのbaseを含んでいません"));
        }
        let [head_ci, merge_ci] = approval.observe_ci(root)?;
        review_safety(root, &repository, args.pr)?;
        approval.verify_pr(&pr_view(root, &repository, args.pr)?, &branch)?;
        assert_synced_main_at(root, &repository, &live)?;
        worktree_clean_head(&worktree_path, &args.head)?;

        let recovery = Recovery {
            approval,
            receipt_sha256: emergency::digest(&receipt)?,
            observed_at: now(),
            head_ci,
            merge_ci,
        };
        save_recovery(root, &receipt, &branch, &live, recovery)
    })
}

pub(super) fn save_recovery(
    root: &Path,
    receipt: &Value,
    branch: &str,
    live: &str,
    recovery: Recovery,
) -> Result<Value> {
    let approval = &recovery.approval;
    let path = state_path(&approval.repository, &approval.task_id)?;
    let mut state = new_state(
        &approval.repository,
        &approval.task_id,
        approval.pr,
        &approval.head_sha,
        branch,
        "main_synced",
    );
    state["version"] = Value::from(STATE_VERSION);
    state["manual_merge"] =
        serde_json::to_value(recovery).map_err(|_| error("手動merge回収の記録を保存できません"))?;
    // 同じtaskの予約は再取得できる。元のreceiptは変更せずstateを一度で保存する。
    admit_delivery(root, receipt, live, &[])?;
    atomic_json(&path, &state)?;
    Ok(state)
}
