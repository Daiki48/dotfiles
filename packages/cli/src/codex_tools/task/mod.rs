//! OSタイマーに依存しない、ホスト内のタスク制御。

#[cfg(test)]
mod autonomy_tests;
mod ci;
mod recovery;
mod resources;
mod store;
mod worker;
#[cfg(test)]
mod workflow_tests;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::worktree;
use store::{Lock, Store, now};

type Result<T> = std::result::Result<T, String>;

#[derive(Parser, Debug)]
#[command(
    name = "codex-task",
    version,
    about = "ホスト内の並列タスク・CI・引継ぎを管理"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 実装依頼を登録する。相談だけでは登録しない。
    Submit {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        objective: String,
        /// 元の利用者依頼と明示境界。親が分割したobjectiveと区別する。
        #[arg(long)]
        authorization: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        depends_on: Vec<String>,
        #[arg(long)]
        resource: Vec<String>,
        #[arg(long, default_value = "high", value_parser = ["low", "medium", "high", "critical"])]
        risk: String,
        /// PRと後続pushのCI起動が既存方針で委任済みのlabel。SHA単位の認可には使わない。
        #[arg(long)]
        ci_label: Vec<String>,
        /// 既存方針で明示されたremote gate。API失敗から推測しない。
        #[arg(long, default_value = "strict", value_parser = ["strict", "github-free-private"])]
        gate_mode: String,
        /// 利用者の終了境界。worker内部のPR作成段階とは別に保存する。
        #[arg(long, default_value = "finish", value_parser = ["pr", "merge", "finish"])]
        stop_after: String,
    },
    List,
    Show {
        #[arg(long)]
        task_id: String,
    },
    Events {
        #[arg(long)]
        task_id: String,
        /// ack済みも含める。既定は未処理だけ。
        #[arg(long)]
        all: bool,
    },
    /// 確認したsequenceまでのイベントを累積ackする。
    Ack {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        through: u64,
    },
    /// 完了taskのack済みイベント・旧worker結果の保持秒数を設定する。
    Retention {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        seconds: u64,
    },
    /// 完了・全イベントack・worker停止を確認して期限超過分を回収する。
    Collect {
        #[arg(long)]
        task_id: String,
    },
    /// 一度だけ照合し、実行可能なworkerを起動する。
    Reconcile,
    /// coordinatorを端末から切り離して起動する。既に稼働中なら再利用する。
    Start,
    /// 共通の照合処理を繰り返す。通常の作業にOSタイマーは不要。
    Run {
        #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..=3600))]
        poll_seconds: u64,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
    /// 判断への回答や追加指示を、同じtaskへ渡して再開する。
    Resume {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        instruction: String,
        #[arg(long, value_parser = ["strict", "github-free-private"])]
        gate_mode: Option<String>,
        #[arg(long)]
        authorization: Option<String>,
    },
    /// 次のworker起動を止める。動作中workerは停止要求を受けて回収する。
    Pause {
        #[arg(long)]
        task_id: String,
    },
    Doctor,
    /// OS設定は変更せず、起動adapter用のコマンドを表示する。
    LaunchCommand,
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        generation: u64,
        #[arg(long)]
        lease_fd: i32,
        #[arg(long)]
        admission_fd: i32,
        #[arg(long)]
        admission_slot: usize,
    },
    #[command(hide = true)]
    Coordinator {
        #[arg(long)]
        lock_fd: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Implementation,
    Review,
    CiAuthorization,
    Integration,
    MainCi,
    Finish,
    Completed,
    AwaitingDecision,
    Blocked,
    Paused,
    Diagnosis,
    Waiting,
    BoundaryReached,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    generation: u64,
    phase: Phase,
    started: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    sequence: u64,
    at: u64,
    kind: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    version: u32,
    id: String,
    host: String,
    repository: PathBuf,
    github: String,
    branch: String,
    objective: String,
    #[serde(default)]
    authorization: String,
    risk: String,
    ci_labels: Vec<String>,
    dependencies: Vec<String>,
    resources: Vec<String>,
    phase: Phase,
    resume_phase: Phase,
    generation: u64,
    run: Option<Run>,
    worktree: Option<PathBuf>,
    pr: Option<u64>,
    head: Option<String>,
    base: Option<String>,
    merge_commit: Option<String>,
    review: Option<worker::Report>,
    review_independent: bool,
    checkpoint: String,
    ci_fingerprint: String,
    ci_state: Option<ci::CiState>,
    // merge後もreceiptに選んだgateを保持する。旧taskはstrictで再観測する。
    #[serde(default)]
    gate_mode: ci::GateMode,
    #[serde(default)]
    requested_gate: ci::GateMode,
    #[serde(default)]
    stop_after: recovery::Boundary,
    #[serde(default)]
    recovery: recovery::State,
    #[serde(default)]
    ci_evidence: Option<ci::Observation>,
    #[serde(default)]
    ci_observation_error_at: Option<u64>,
    #[serde(default)]
    ci_activation_until: u64,
    next_check: u64,
    failures: u32,
    pause_requested: bool,
    created: u64,
    updated: u64,
    events: Vec<Event>,
    event_sequence: u64,
    #[serde(default)]
    acknowledged_sequence: u64,
    #[serde(default = "default_retention")]
    retention_seconds: u64,
    // ack・保持設定とcoordinatorの楽観的更新を照合する。
    #[serde(default)]
    revision: u64,
}

fn default_retention() -> u64 {
    30 * 24 * 60 * 60
}

fn valid_id(id: &str) -> bool {
    worktree::valid_artifact_task_id(id)
}
fn safe_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains('\0')
}
fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn risk_rank(risk: &str) -> usize {
    ["low", "medium", "high", "critical"]
        .iter()
        .position(|candidate| *candidate == risk)
        .unwrap_or(usize::MAX)
}

impl Task {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !valid_id(&self.id)
            || self.host.len() != 64
            || !self.repository.is_absolute()
            || !safe_text(&self.objective, 32 * 1024)
            || !self.authorization.is_empty() && !safe_text(&self.authorization, 32 * 1024)
            || risk_rank(&self.risk) == usize::MAX
            || !valid_ci_labels(&self.ci_labels)
            || !self.github.contains('/')
            || !self.branch.starts_with("feat/")
            || self.branch.contains(['\n', '\r', '\0'])
            || self
                .dependencies
                .iter()
                .any(|id| !valid_id(id) || id == &self.id)
            || self
                .recovery
                .repair_task
                .as_ref()
                .is_some_and(|id| !valid_id(id) || !self.recovery.children.contains(id))
            || self.resources.iter().any(|value| !resource_name(value))
            || self.head.as_deref().is_some_and(|head| !valid_sha(head))
            || self.base.as_deref().is_some_and(|head| !valid_sha(head))
            || self
                .merge_commit
                .as_deref()
                .is_some_and(|head| !valid_sha(head))
            || self.acknowledged_sequence > self.event_sequence
            || self
                .events
                .iter()
                .any(|event| event.sequence == 0 || event.sequence > self.event_sequence)
            || self
                .events
                .windows(2)
                .any(|events| events[0].sequence >= events[1].sequence)
            || self.checkpoint.len() > 32 * 1024
            || self.recovery.children.len() > 32
            || self
                .recovery
                .children
                .iter()
                .any(|id| !valid_id(id) || id == &self.id)
            || self
                .recovery
                .parent
                .as_deref()
                .is_some_and(|id| !valid_id(id) || id == self.id)
            || self
                .recovery
                .assessment
                .as_ref()
                .is_some_and(|assessment| assessment.validate().is_err())
            || self.requested_gate == ci::GateMode::LocalValidation
            || self.requested_gate == ci::GateMode::GithubFreePrivate
                && risk_rank(&self.risk) < risk_rank("high")
            || self
                .review
                .as_ref()
                .is_some_and(|review| review.validate().is_err())
            || (matches!(self.phase, Phase::CiAuthorization | Phase::Integration)
                && !review_ready(self))
            || self
                .run
                .as_ref()
                .is_some_and(|run| run.generation != self.generation)
        {
            return Err("task状態の不変条件が不正です".into());
        }
        Ok(())
    }

    fn event(&mut self, kind: &str, detail: &str) {
        self.event_sequence += 1;
        self.updated = now();
        self.events.push(Event {
            sequence: self.event_sequence,
            at: self.updated,
            kind: kind.into(),
            detail: detail.into(),
        });
    }

    fn acknowledge(&mut self, through: u64) -> Result<()> {
        if through < self.acknowledged_sequence || through > self.event_sequence {
            return Err("ackのsequenceが現在の範囲外です".into());
        }
        self.acknowledged_sequence = through;
        self.revision += 1;
        Ok(())
    }

    fn invalidate_ci(&mut self) {
        self.ci_state = None;
        self.ci_fingerprint.clear();
        self.ci_evidence = None;
    }

    fn delivery_gate(&self) -> ci::GateMode {
        self.gate_mode
    }

    fn stop(&mut self, phase: Phase, message: &str) {
        if !matches!(
            self.phase,
            Phase::Blocked | Phase::AwaitingDecision | Phase::Paused
        ) {
            self.resume_phase = self.phase.clone();
        }
        self.phase = phase;
        self.checkpoint = message.into();
        self.event("stopped", message);
    }
}

fn resource_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

struct Context {
    home: PathBuf,
    codex_home: PathBuf,
    repository: PathBuf,
    github: String,
    store: Store,
}

impl Context {
    fn open() -> Result<Self> {
        let home =
            crate::codex::setup_home_dir().map_err(|_| "検証済みaccount homeを取得できません")?;
        let codex_home = crate::codex::resolve_codex_home(&home)
            .map_err(|_| "CODEX_HOMEを安全に解決できません")?;
        let cwd = std::env::current_dir().map_err(|_| "current directoryを確認できません")?;
        let (repository, github, key) =
            worktree::task_repository(&cwd).map_err(|error| error.to_string())?;
        let store = Store::open(&codex_home, &key)?;
        Ok(Self {
            home,
            codex_home,
            repository,
            github,
            store,
        })
    }

    fn load(&self, id: &str) -> Result<Task> {
        let task = self.store.load(id)?;
        if task.repository != self.repository || task.github != self.github {
            return Err("taskのrepository identityが現在のcheckoutと一致しません".into());
        }
        Ok(task)
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn submit(
    context: &Context,
    id: String,
    objective: String,
    branch: Option<String>,
    dependencies: Vec<String>,
    resources: Vec<String>,
    risk: String,
    ci_labels: Vec<String>,
) -> Result<Value> {
    submit_with_policy(
        context,
        id,
        objective.clone(),
        objective,
        branch,
        dependencies,
        resources,
        risk,
        ci_labels,
        ci::GateMode::Strict,
        recovery::Boundary::Finish,
    )
}

#[allow(clippy::too_many_arguments)]
fn submit_with_policy(
    context: &Context,
    id: String,
    objective: String,
    authorization: String,
    branch: Option<String>,
    dependencies: Vec<String>,
    resources: Vec<String>,
    risk: String,
    ci_labels: Vec<String>,
    requested_gate: ci::GateMode,
    stop_after: recovery::Boundary,
) -> Result<Value> {
    if !valid_id(&id) || !safe_text(&objective, 32 * 1024) {
        return Err("task IDまたは目的が不正です".into());
    }
    let risk = if requested_gate == ci::GateMode::GithubFreePrivate
        && risk_rank(&risk) < risk_rank("high")
    {
        "high".to_string()
    } else {
        risk
    };
    let _lock = context.store.lock()?;
    if context
        .store
        .path(&id)?
        .try_exists()
        .map_err(|_| "既存taskを確認できません")?
    {
        let task = context.load(&id)?;
        if task.objective == objective
            && task.dependencies == dependencies
            && task.resources == resources
            && task.risk == risk
            && task.ci_labels == ci_labels
            && task.requested_gate == requested_gate
            && task.stop_after == stop_after
            && task.authorization == authorization
            && branch.as_ref().is_none_or(|branch| branch == &task.branch)
        {
            return Ok(json!({"task_id":id,"existing":true,"phase":task.phase}));
        }
        return Err(
            "同じIDの別の依頼があります。showで確認し、追加指示はresumeを使ってください".into(),
        );
    }
    for dependency in &dependencies {
        let task = context.load(dependency)?;
        if task.id == id {
            return Err("taskは自分自身に依存できません".into());
        }
    }
    let task = Task {
        version: 1,
        id: id.clone(),
        host: context.store.host.clone(),
        repository: context.repository.clone(),
        github: context.github.clone(),
        branch: branch.unwrap_or_else(|| format!("feat/{id}")),
        objective,
        authorization,
        risk,
        ci_labels,
        dependencies,
        resources,
        phase: Phase::Implementation,
        resume_phase: Phase::Implementation,
        generation: 0,
        run: None,
        worktree: None,
        pr: None,
        head: None,
        base: None,
        merge_commit: None,
        review: None,
        review_independent: false,
        checkpoint: String::new(),
        ci_fingerprint: String::new(),
        ci_state: None,
        gate_mode: ci::GateMode::Strict,
        requested_gate,
        stop_after,
        recovery: recovery::State::default(),
        ci_evidence: None,
        ci_observation_error_at: None,
        ci_activation_until: 0,
        next_check: 0,
        failures: 0,
        pause_requested: false,
        created: now(),
        updated: now(),
        events: Vec::new(),
        event_sequence: 0,
        acknowledged_sequence: 0,
        retention_seconds: default_retention(),
        revision: 0,
    };
    context.store.save(&task)?;
    Ok(json!({"task_id":id,"existing":false,"phase":task.phase}))
}

fn apply_report(context: &Context, task: &mut Task) -> Result<()> {
    apply_report_with(context, task, |context, task| {
        worktree::task_completed(&context.repository, &task.id).map_err(|error| error.to_string())
    })
}

fn apply_report_with(
    context: &Context,
    task: &mut Task,
    completed: impl Fn(&Context, &Task) -> Result<bool>,
) -> Result<()> {
    let Some(run) = task.run.clone() else {
        return Ok(());
    };
    if Lock::try_acquire(&context.store.lease_path(&task.id)?)?.is_none() {
        return Ok(());
    }
    let path = worker::report_path(&context.store, &task.id, run.generation)?;
    let report: worker::Report = match path.try_exists() {
        Ok(true) => match store::read_json(&path) {
            Ok(report) => report,
            Err(reason) => {
                task.run = None;
                task.diagnose(&reason);
                return Ok(());
            }
        },
        Ok(false) => {
            task.run = None;
            task.failures += 1;
            if task.pause_requested {
                task.stop(Phase::Paused, "workerの停止を確認しました");
            } else if run.phase == Phase::Diagnosis {
                task.recovery.attempts += 1;
                task.monitor("診断workerが中断しました。状態変化を監視して再診断します");
            } else {
                task.diagnose("前回workerの停止を生存lockで確認しました。既存worktree・Git・PR・delivery記録を照合し、中断原因と再開方法を診断してください。");
                task.event(
                    "worker_recovery",
                    "旧writerの停止を確認し、保存済み作業から再開します",
                );
            }
            return Ok(());
        }
        Err(_) => return Err("worker報告を検査できません".into()),
    };
    if report.generation != run.generation {
        task.run = None;
        task.diagnose("古いworker報告を拒否しました。旧報告を保持して現在の状態を診断します");
        return Ok(());
    }
    if let Err(reason) = report.validate() {
        task.run = None;
        task.diagnose(&reason);
        return Ok(());
    }
    if risk_rank(&report.risk) > risk_rank(&task.risk) {
        task.risk = report.risk.clone();
    }
    if run.phase != Phase::Implementation && (report.head != task.head || report.pr != task.pr) {
        task.run = None;
        task.diagnose("worker報告のレビュー対象HEADが一致しません");
        task.resume_phase = Phase::Implementation;
        task.recovery.origin = Some(Phase::Implementation);
        return Ok(());
    }
    task.run = None;
    task.checkpoint = report.summary.clone();
    if run.phase == Phase::Implementation && report.pr.is_some() && report.head.is_some() {
        task.pr = report.pr;
        task.head = report.head.clone();
        task.invalidate_ci();
    }
    let review_outdated = run.phase == Phase::Integration && !review_ready(task);
    if task.pause_requested {
        task.stop(Phase::Paused, "workerの停止と回収を確認しました");
        if review_outdated {
            task.review = None;
            task.review_independent = false;
            task.resume_phase = Phase::Review;
        }
        return Ok(());
    }
    if let Some(assessment) = &report.assessment {
        task.record_assessment(assessment);
    }
    if run.phase == Phase::Diagnosis {
        apply_diagnosis(context, task, &report)?;
        task.event("worker_result", &report.outcome);
        return Ok(());
    }
    match report.outcome.as_str() {
        "resource_wait" => {
            task.recovery.origin = Some(run.phase);
            task.monitor("別taskが共有資源を使用しています。生存lockの解放を監視して再開します");
        }
        "ready" => match run.phase {
            Phase::Implementation => {
                if let Some(assessment) = &report.assessment {
                    register_followups(context, task, assessment)?;
                }
                let pr = report
                    .pr
                    .filter(|number| *number > 0)
                    .ok_or("worker報告にPRがありません")?;
                let head = report
                    .head
                    .as_deref()
                    .filter(|head| valid_sha(head))
                    .ok_or("worker報告に固定HEADがありません")?;
                task.invalidate_ci();
                task.gate_mode = task.requested_gate;
                task.pr = Some(pr);
                task.head = Some(head.into());
                task.review = None;
                task.review_independent = false;
                if risk_rank(&task.risk) < risk_rank("high") {
                    task.review = Some(worker::Report {
                        risk: task.risk.clone(),
                        ..report.clone()
                    });
                    task.phase = Phase::CiAuthorization;
                } else {
                    task.phase = Phase::Review;
                }
                task.next_check = 0;
                if task.stop_after == recovery::Boundary::Pr && task.phase == Phase::CiAuthorization
                {
                    task.phase = Phase::BoundaryReached;
                }
            }
            Phase::Review => {
                task.review = Some(worker::Report {
                    risk: task.risk.clone(),
                    ..report.clone()
                });
                task.review_independent = true;
                task.phase = Phase::CiAuthorization;
                if task.stop_after == recovery::Boundary::Pr {
                    task.phase = Phase::BoundaryReached;
                }
            }
            Phase::Integration => {
                if review_ready(task) {
                    task.phase = Phase::MainCi;
                } else {
                    task.review = None;
                    task.review_independent = false;
                    task.phase = Phase::Review;
                    task.checkpoint =
                        "riskが上がりました。新しい深度の独立レビューを行います".into();
                }
                task.next_check = 0;
                if task.stop_after == recovery::Boundary::Merge && task.phase == Phase::MainCi {
                    task.phase = Phase::BoundaryReached;
                }
            }
            Phase::Finish => {
                if !completed(context, task)? {
                    task.diagnose("delivery完了記録とworktree回収を確認できません");
                } else {
                    task.phase = Phase::Completed;
                }
            }
            _ => return Err("worker報告のphaseが不正です".into()),
        },
        "needs_changes" => {
            task.phase = Phase::Implementation;
            task.review = None;
            task.review_independent = false;
            task.invalidate_ci();
            task.failures += 1;
            if task.failures >= 3 {
                task.diagnose("修正が反復しています。回数で終了せず、観測できた進展・原因・実装境界・検証方法を見直してください");
            }
        }
        _ => task.diagnose(&report.summary),
    }
    if review_outdated
        && matches!(
            task.phase,
            Phase::Blocked
                | Phase::AwaitingDecision
                | Phase::Paused
                | Phase::Diagnosis
                | Phase::Waiting
        )
    {
        task.review = None;
        task.review_independent = false;
        task.resume_phase = Phase::Review;
        task.recovery.origin = Some(Phase::Review);
    }
    task.event("worker_result", &report.outcome);
    Ok(())
}

fn apply_diagnosis(context: &Context, task: &mut Task, report: &worker::Report) -> Result<()> {
    let Some(assessment) = &report.assessment else {
        task.recovery.attempts += 1;
        task.monitor(
            "診断に根拠・次の対応・再開条件がありません。既存状態を保持して変化を監視します",
        );
        return Ok(());
    };
    task.record_assessment(assessment);
    task.recovery.attempts += 1;
    if report.outcome == "awaiting_decision" && assessment.decision.is_some() {
        task.stop(Phase::AwaitingDecision, &report.summary);
        task.resume_phase = task
            .recovery
            .origin
            .clone()
            .unwrap_or(Phase::Implementation);
        if matches!(
            task.resume_phase,
            Phase::Integration | Phase::CiAuthorization
        ) && !review_ready(task)
        {
            task.resume_phase = Phase::Review;
        }
        return Ok(());
    }
    if assessment.decision.is_some() {
        task.monitor("意思決定と実行要求が一致しません。書込みは行わず、保存状態を監視します");
        return Ok(());
    }
    register_followups(context, task, assessment)?;
    if task.recovery.attempts > 3
        || report.outcome == "waiting"
        || !assessment.followups.is_empty()
        || matches!(report.outcome.as_str(), "blocked" | "awaiting_decision")
    {
        task.monitor(&report.summary);
        return Ok(());
    }
    let origin = task
        .recovery
        .origin
        .clone()
        .unwrap_or(Phase::Implementation);
    task.checkpoint = format!(
        "{}\n診断の根拠: {}\n次の対応: {}",
        report.summary, assessment.evidence, assessment.next_action
    );
    task.phase = if report.outcome == "needs_changes" && task.merge_commit.is_none() {
        task.review = None;
        task.review_independent = false;
        Phase::Implementation
    } else if origin == Phase::Diagnosis || origin == Phase::Waiting {
        Phase::Implementation
    } else {
        origin
    };
    task.failures = 0;
    task.next_check = 0;
    // 古いreviewを使って統合へ戻さない。
    if matches!(task.phase, Phase::Integration | Phase::CiAuthorization) && !review_ready(task) {
        task.phase = Phase::Review;
    }
    task.event(
        "replanned",
        "元の依頼と既存許可を照合し、診断に基づく対応へ戻します",
    );
    Ok(())
}

fn register_followups(
    context: &Context,
    task: &mut Task,
    assessment: &recovery::Assessment,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    if assessment
        .followups
        .iter()
        .filter(|next| next.purpose == recovery::FollowupPurpose::RepairMainCi)
        .count()
        > 1
    {
        return Err("1つのmain CI失敗には修復専用taskを1つだけ紐付けてください".into());
    }
    if assessment
        .followups
        .iter()
        .any(|next| next.purpose == recovery::FollowupPurpose::RepairMainCi)
        && !(task.recovery.barrier
            && task.merge_commit.is_some()
            && task.ci_state == Some(ci::CiState::Failure)
            && matches!(task.effective_phase(), Phase::MainCi | Phase::Finish))
    {
        return Err("実在するmain CI失敗に対してのみ修復専用taskを登録できます".into());
    }
    if task.recovery.children.len() + assessment.followups.len() > 32 {
        return Err("後続taskの登録数が上限です。既存の計画・依存関係を見直してください".into());
    }
    let mut ancestors = std::collections::BTreeSet::from([task.id.clone()]);
    let mut parent = task.recovery.parent.clone();
    while let Some(id) = parent {
        if !ancestors.insert(id.clone()) || ancestors.len() > 4 {
            return Err(
                "後続taskの依存・分割が深すぎます。元の目的へ戻して計画を見直してください".into(),
            );
        }
        parent = context.load(&id)?.recovery.parent;
    }
    let mut previous = (task.phase == Phase::Implementation).then_some(task.id.clone());
    for next in &assessment.followups {
        // 同じ目的の再報告や中断後の再読込みで同じ後続taskを二重登録しない。
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", task.id, next.objective))
        );
        let id = format!("task-followup-{}", &digest[..20]);
        let repairs_main = next.purpose == recovery::FollowupPurpose::RepairMainCi;
        if repairs_main
            && task
                .recovery
                .repair_task
                .as_ref()
                .is_some_and(|existing| existing != &id)
        {
            return Err(
                "既存の修復専用taskを継続してください。別taskへbarrier例外を付与しません".into(),
            );
        }
        {
            let _lock = context.store.lock()?;
            let current = context.load(&task.id)?;
            if current.pause_requested
                || current.generation != task.generation
                || current.revision != task.revision
            {
                return Err(
                    "親taskの停止・世代・追加指示が変化しました。後続登録を保留して再照合します"
                        .into(),
                );
            }
        }
        let dependencies = if next.after_previous {
            previous.clone().into_iter().collect()
        } else {
            vec![]
        };
        let objective = format!(
            "元の許可済み依頼: {}\n\n今回の部分作業: {}\n元依頼に含まれる根拠: {}\n新規権限・費用・製品判断・不可逆操作を追加しない。",
            task.objective, next.objective, next.authorization
        );
        submit_with_policy(
            context,
            id.clone(),
            objective,
            task.authorization.clone(),
            None,
            dependencies,
            task.resources.clone(),
            task.risk.clone(),
            task.ci_labels.clone(),
            task.requested_gate,
            task.stop_after.clone(),
        )?;
        let _lock = context.store.lock()?;
        let mut child = context.load(&id)?;
        if child
            .recovery
            .parent
            .as_ref()
            .is_some_and(|parent| parent != &task.id)
        {
            return Err("後続taskの所有元が一致しません".into());
        }
        child.recovery.parent = Some(task.id.clone());
        child.recovery.repairs_main = repairs_main;
        context.store.save(&child)?;
        if repairs_main {
            task.recovery.repair_task = Some(id.clone());
        }
        if !task.recovery.children.contains(&id) {
            task.recovery.children.push(id.clone());
            task.event("followup_registered", "元の依頼内の後続taskを登録しました");
        }
        previous = Some(id);
    }
    Ok(())
}

fn waiting_observation(context: &Context, task: &Task) -> String {
    use sha2::{Digest, Sha256};
    let mut value = format!(
        "{:?}|{:?}|{:?}",
        task.ci_fingerprint,
        task.ci_state,
        task.ci_observation_error_at.is_none()
    );
    if let Some(event) = task.events.iter().rev().find(|event| {
        matches!(
            event.kind.as_str(),
            "api_wait" | "ci_authorization_wait" | "repair_wait"
        )
    }) {
        value.push_str(&event.detail);
    }
    for id in &task.recovery.children {
        let child = context.load(id);
        value.push_str(&format!(
            "|{:?}",
            child.map(|child| (child.phase, child.updated))
        ));
    }
    let resource_root = context.codex_home.join("worktrees/.task-resources");
    for resource in &task.resources {
        let key = format!("{:x}", Sha256::digest(resource.as_bytes()));
        let available = if resource_root.is_dir() {
            Lock::try_acquire(&resource_root.join(key)).map(|lease| lease.is_some())
        } else {
            Ok(true)
        };
        value.push_str(&format!("|resource:{available:?}"));
    }
    if let Some(workspace) = &task.worktree {
        value.push_str(&format!("|{:?}", worktree::task_observation(workspace)));
        if let Some(assessment) = &task.recovery.assessment {
            for relative in &assessment.watch_paths {
                value.push_str(&format!("|{:?}", watch_metadata(workspace, relative)));
            }
        }
    }
    value.push_str(&format!(
        "|{:?}",
        host_capacity(context).map(|capacity| capacity > 0)
    ));
    value.push_str(&format!(
        "|{:?}",
        watch_metadata(&context.codex_home, "config.toml")
    ));
    format!("{:x}", Sha256::digest(value))
}

fn watch_metadata(root: &std::path::Path, relative: &str) -> Result<(u64, u128)> {
    if !recovery::safe_relative(relative) {
        return Err("監視pathが不正です".into());
    }
    let mut path = root.to_path_buf();
    for part in std::path::Path::new(relative).components() {
        path.push(part);
        if std::fs::symlink_metadata(&path)
            .map_err(|_| "監視対象は未作成です")?
            .file_type()
            .is_symlink()
        {
            return Err("監視対象のsymlinkは読みません".into());
        }
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "監視対象を確認できません")?;
    Ok((
        metadata.len(),
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
    ))
}

fn monitor_task(context: &Context, task: &mut Task) {
    if task.phase != Phase::Waiting {
        return;
    }
    if task.recovery.observation.is_empty() {
        task.recovery.observation = waiting_observation(context, task);
        return;
    }
    if task.recovery.next_observation > now() {
        return;
    }
    let observation = waiting_observation(context, task);
    if !task.recovery.observation.is_empty() && task.recovery.observation != observation {
        task.recovery.attempts = 0;
        task.diagnose(
            "監視対象の状態が変化しました。保存済みの原因・解消条件を照合して継続してください",
        );
    }
    task.recovery.observation = observation;
    task.recovery.next_observation = now() + 120;
}

fn observe_ci(context: &Context, task: &mut Task) -> Result<()> {
    observe_ci_with(context, task, |context, task| {
        ci::observe(
            &context.repository,
            &context.github,
            task.pr.ok_or("CI照合対象のPRがありません")?,
            matches!(task.effective_phase(), Phase::MainCi | Phase::Finish)
                .then_some(task.delivery_gate()),
            task.requested_gate,
        )
    })
}

fn observe_ci_with(
    context: &Context,
    task: &mut Task,
    observe: impl FnOnce(&Context, &Task) -> Result<ci::Observation>,
) -> Result<()> {
    // PRを保持して実装から外部待機したtaskも、CI変化をRustで取得する。
    // 観測で診断・reviewを飛ばさず、未納品の実装や明示停止は起動しない。
    let implementation_wait = matches!(task.phase, Phase::Diagnosis | Phase::Waiting)
        && task.run.is_none()
        && task.effective_phase() == &Phase::Implementation
        && task.pr.is_some()
        && task.head.as_deref().is_some_and(valid_sha);
    if task.next_check > now()
        || !(implementation_wait
            || matches!(
                task.effective_phase(),
                Phase::Review
                    | Phase::CiAuthorization
                    | Phase::Integration
                    | Phase::MainCi
                    | Phase::Finish
            ))
    {
        return Ok(());
    }
    if task.pr.is_none() {
        return Err("CI照合対象のPRがありません".into());
    }
    if task.recovery.barrier
        && matches!(task.effective_phase(), Phase::MainCi | Phase::Finish)
        && refresh_repair(context, task)?
    {
        return Ok(());
    }
    let result = observe(context, task);
    record_ci_result(task, result)
}

fn refresh_repair(context: &Context, task: &mut Task) -> Result<bool> {
    task.recovery.repair_evidence = None;
    let Some(original_merge) = task.merge_commit.clone() else {
        return Ok(false);
    };
    for id in task.recovery.children.clone() {
        if task.recovery.repair_task.as_ref() != Some(&id) {
            continue;
        }
        let child = context.load(&id)?;
        if child.phase != Phase::Completed
            || !child.recovery.repairs_main
            || child.recovery.parent.as_ref() != Some(&task.id)
        {
            continue;
        }
        let mut verified = child;
        let mut visited = std::collections::BTreeSet::from([task.id.clone()]);
        while let Some(next) = verified.recovery.repair_task.clone() {
            if !visited.insert(verified.id.clone()) || visited.len() > 4 {
                return Err("修復taskの親子関係が循環または上限を超えています".into());
            }
            let successor = context.load(&next)?;
            if successor.phase != Phase::Completed
                || !successor.recovery.repairs_main
                || successor.recovery.parent.as_ref() != Some(&verified.id)
            {
                return Ok(false);
            }
            verified = successor;
        }
        let (Some(pr), Some(head)) = (verified.pr, verified.head.as_deref()) else {
            continue;
        };
        let observation = match ci::observe_repair(
            &context.repository,
            &context.github,
            &original_merge,
            pr,
            head,
            verified.delivery_gate(),
            verified.requested_gate,
        ) {
            Ok(observation) => observation,
            Err(reason) => {
                task.event("repair_wait", &reason);
                task.next_check = now() + 120;
                return Ok(false);
            }
        };
        task.recovery.repair_evidence = Some(recovery::RepairEvidence {
            task_id: id,
            verified_task_id: verified.id,
            original_merge,
            observation,
        });
        task.ci_observation_error_at = None;
        task.next_check = now() + 30;
        if !matches!(task.phase, Phase::Diagnosis | Phase::Waiting) {
            task.phase = Phase::Finish;
        }
        return Ok(true);
    }
    Ok(false)
}

fn repair_ready(task: &Task) -> bool {
    task.recovery
        .repair_evidence
        .as_ref()
        .is_some_and(|repair| {
            task.recovery.children.contains(&repair.task_id)
                && task.recovery.repair_task.as_ref() == Some(&repair.task_id)
                && valid_id(&repair.verified_task_id)
                && task.merge_commit.as_ref() == Some(&repair.original_merge)
                && repair.observation.merged
                && repair.observation.allows_delivery()
                && repair.observation.observed_at <= now()
                && now().saturating_sub(repair.observation.observed_at) <= 120
        })
}

fn record_ci_result(task: &mut Task, result: Result<ci::Observation>) -> Result<()> {
    task.next_check = now() + 30;
    let observation = match result {
        Ok(observation) => observation,
        Err(reason) => {
            task.invalidate_ci();
            task.ci_observation_error_at.get_or_insert(now());
            if !task
                .events
                .last()
                .is_some_and(|event| event.kind == "api_wait" && event.detail == reason)
            {
                task.event("api_wait", &reason);
            }
            task.next_check = now() + 120;
            if !matches!(
                task.phase,
                Phase::Diagnosis | Phase::Waiting | Phase::Review
            ) {
                task.diagnose(&reason);
            }
            return Ok(());
        }
    };
    task.ci_observation_error_at = None;
    let eligible = observation.allows_delivery();
    task.ci_evidence = Some(observation.clone());
    task.ci_state = Some(observation.state);
    if task.ci_fingerprint != observation.fingerprint {
        task.ci_fingerprint = observation.fingerprint;
        task.event("ci", &format!("{:?}", observation.state));
    }
    if matches!(task.effective_phase(), Phase::MainCi | Phase::Finish) {
        if !observation.merged
            || observation.merge_commit.as_ref() != Some(&observation.target)
            || task.head.as_ref() != Some(&observation.head)
            || task
                .merge_commit
                .as_ref()
                .is_some_and(|sha| Some(sha) != observation.merge_commit.as_ref())
        {
            task.invalidate_ci();
            task.diagnose("merge済みであることを確認できません");
            return Ok(());
        }
        task.merge_commit = observation.merge_commit;
    } else {
        if !observation.merged && task.merge_commit.is_none() {
            task.recovery.barrier = false;
        }
        if task.head.as_ref() != Some(&observation.head) {
            stale_head(task);
            return Ok(());
        }
        if observation.merged {
            if matches!(task.effective_phase(), Phase::Integration)
                && review_ready(task)
                && observation.merge_commit.is_some()
            {
                task.merge_commit = observation.merge_commit;
                task.recovery.barrier = true;
                task.recovery.origin = Some(Phase::MainCi);
                task.phase = Phase::MainCi;
                task.invalidate_ci();
                task.next_check = 0;
                task.event(
                    "merge_recovered",
                    "固定headの実mergeを再取得しました。mergeを再送せずmain側gateを確認します",
                );
            } else {
                task.diagnose("想定外の段階でPRがmergeされています。receipt・固定head・所有権を照合し、既存gateを省略せず診断してください");
            }
            return Ok(());
        }
        if task
            .base
            .as_ref()
            .is_some_and(|base| base != &observation.base)
        {
            task.invalidate_ci();
            task.review = None;
            task.review_independent = false;
            task.phase = Phase::Implementation;
            task.checkpoint =
                "baseが更新されました。既存worktreeを安全に更新して検証をやり直してください".into();
            task.base = Some(observation.base);
            return Ok(());
        }
        task.base = Some(observation.base);
        task.gate_mode = observation.gate_mode;
    }
    // 診断・外部監視中も証跡を更新するが、その観測だけで診断を飛ばさない。
    if matches!(
        task.phase,
        Phase::Diagnosis | Phase::Waiting | Phase::Review
    ) {
        return Ok(());
    }
    if task.phase == Phase::CiAuthorization && !task.ci_labels.is_empty() {
        // 初回base観測がreview後になっても、admission failureより認可を先に行う。
        return Ok(());
    }
    match observation.state {
        ci::CiState::Success | ci::CiState::LocalValidation if eligible => {
            if task.phase == Phase::MainCi {
                task.phase = Phase::Finish;
            } else if task.phase == Phase::CiAuthorization {
                task.phase = Phase::Integration;
            }
        }
        ci::CiState::Failure => {
            if task.ci_activation_until > now() && task.phase == Phase::Integration {
                return Ok(());
            }
            if matches!(task.phase, Phase::MainCi | Phase::Finish) {
                task.recovery.barrier = true;
                task.diagnose("merge commitのmain CIが失敗しました。後続mergeを保留し、原因調査・許可内の修復PR・再検証を進めてください");
            } else {
                task.phase = Phase::Implementation;
                task.review = None;
                task.checkpoint =
                    "現在のPR CIが失敗しました。対象SHAの失敗原因を確認して修正してください".into();
                task.failures += 1;
                if task.failures >= 3 {
                    task.diagnose("PR CIの失敗が反復しています。実装修正・CI認可・実行環境のどこが原因かを診断し、対応方法を見直してください");
                }
            }
        }
        ci::CiState::Success
        | ci::CiState::LocalValidation
        | ci::CiState::Pending
        | ci::CiState::Missing
        | ci::CiState::Cancelled
        | ci::CiState::TimedOut
        | ci::CiState::Skipped
        | ci::CiState::Stale => {
            if now().saturating_sub(task.updated) >= 900 {
                task.diagnose("CIの状態が15分以上進んでいません。対象SHA・runner・実行承認・cancel/timeoutを診断し、許可内の復旧か外部監視へ分けてください");
            }
        }
    }
    Ok(())
}

fn stale_head(task: &mut Task) {
    task.invalidate_ci();
    task.review = None;
    task.review_independent = false;
    task.diagnose("PRのHEADがworker報告と異なります。所有権を照合し、実装段階でGit・PRと検証・レビューを更新してください");
    task.resume_phase = Phase::Implementation;
    task.recovery.origin = Some(Phase::Implementation);
}

#[cfg(test)]
fn resume_task(context: &Context, task_id: &str, instruction: String) -> Result<Value> {
    resume_with_options(context, task_id, instruction, None, None)
}

fn resume_with_options(
    context: &Context,
    task_id: &str,
    instruction: String,
    gate_mode: Option<String>,
    authorization: Option<String>,
) -> Result<Value> {
    if !safe_text(&instruction, 32 * 1024) {
        return Err("追加指示が不正です".into());
    }
    let _lock = context.store.lock()?;
    let mut task = context.load(task_id)?;
    if task.run.is_some() || Lock::try_acquire(&context.store.lease_path(task_id)?)?.is_none() {
        return Err("workerが動作中です。停止と回収を確認するまで再割当てしません".into());
    }
    if matches!(task.phase, Phase::Completed | Phase::BoundaryReached) {
        return Err("完了taskへ追加作業は登録できません。別taskとして依頼してください".into());
    }
    if let Some(authorization) = authorization {
        if !safe_text(&authorization, 32 * 1024) {
            return Err("元の利用者依頼が不正です".into());
        }
        task.authorization = authorization;
    }
    if let Some(gate) = gate_mode {
        let gate = match gate.as_str() {
            "strict" => ci::GateMode::Strict,
            "github-free-private" => ci::GateMode::GithubFreePrivate,
            _ => return Err("受付policyが不正です".into()),
        };
        if gate != task.requested_gate {
            if task.merge_commit.is_some() {
                return Err("merge後のreceipt policyは変更できません".into());
            }
            task.requested_gate = gate;
            task.gate_mode = gate;
            if gate == ci::GateMode::GithubFreePrivate && risk_rank(&task.risk) < risk_rank("high")
            {
                task.risk = "high".into();
            }
            task.review = None;
            task.review_independent = false;
            task.invalidate_ci();
            task.ci_activation_until = 0;
            task.resume_phase = Phase::Implementation;
            task.recovery.origin = Some(Phase::Implementation);
            task.phase = Phase::Implementation;
        }
    }
    task.pause_requested = false;
    task.failures = 0;
    task.recovery.attempts = 0;
    task.recovery.assessment = None;
    task.recovery.next_authorization = 0;
    task.recovery.authorization_attempts = 0;
    task.next_check = 0;
    task.checkpoint = instruction;
    if matches!(
        task.phase,
        Phase::Blocked
            | Phase::AwaitingDecision
            | Phase::Paused
            | Phase::Waiting
            | Phase::Diagnosis
    ) {
        task.phase = if matches!(task.phase, Phase::Waiting | Phase::Diagnosis) {
            task.recovery
                .origin
                .clone()
                .unwrap_or_else(|| task.resume_phase.clone())
        } else {
            task.resume_phase.clone()
        };
    }
    task.event("resumed", "追加指示を保存しました");
    context.store.save(&task)?;
    Ok(json!({"task_id":task_id,"phase":task.phase}))
}

fn ready(context: &Context, task: &Task, tasks: &[Task]) -> Result<bool> {
    if task.run.is_some()
        || task.pause_requested
        || !matches!(
            task.phase,
            Phase::Implementation
                | Phase::Review
                | Phase::Integration
                | Phase::Finish
                | Phase::Diagnosis
        )
    {
        return Ok(false);
    }
    if let Some(parent) = &task.recovery.parent
        && context.load(parent)?.pause_requested
    {
        return Ok(false);
    }
    if task.phase == Phase::Diagnosis && task.recovery.attempts > 3 {
        return Ok(false);
    }
    if matches!(
        task.phase,
        Phase::CiAuthorization | Phase::Integration | Phase::Finish
    ) && !review_ready(task)
    {
        return Ok(false);
    }
    for dependency in &task.dependencies {
        if task.phase != Phase::Diagnosis
            && !matches!(
                context.load(dependency)?.phase,
                Phase::Completed | Phase::BoundaryReached
            )
        {
            return Ok(false);
        }
    }
    if task.phase != Phase::Diagnosis
        && tasks.iter().any(|other| {
            other.id != task.id
                && other.run.is_some()
                && other
                    .resources
                    .iter()
                    .any(|resource| task.resources.contains(resource))
        })
    {
        return Ok(false);
    }
    if matches!(task.phase, Phase::Integration | Phase::Finish) {
        if tasks.iter().any(|other| {
            other.id != task.id
                && (integration_barrier(other) && !repair_exempt(task, other, tasks)
                    || other
                        .run
                        .as_ref()
                        .is_some_and(|run| run.phase == Phase::Integration))
        }) {
            return Ok(false);
        }
        // 起動直前の再観測もstateへ保存してから、このgateへ渡す。
        if !(task.phase == Phase::Finish && repair_ready(task))
            && (task.ci_observation_error_at.is_some()
                || task.ci_evidence.as_ref().is_none_or(|observation| {
                    !observation.allows_delivery()
                        || task.delivery_gate() != observation.gate_mode
                        || task.ci_state != Some(observation.state)
                        || task.head.as_ref() != Some(&observation.head)
                        || observation.observed_at > now()
                        || now().saturating_sub(observation.observed_at) > 120
                        || if task.phase == Phase::Finish {
                            !observation.merged
                                || task.merge_commit.as_ref() != Some(&observation.target)
                        } else {
                            observation.merged || task.base.as_ref() != Some(&observation.base)
                        }
                }))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn review_ready(task: &Task) -> bool {
    task.head.is_some()
        && task.pr.is_some()
        && task.review.as_ref().is_some_and(|review| {
            review.outcome == "ready"
                && review.head == task.head
                && review.pr == task.pr
                && risk_rank(&review.risk) >= risk_rank(&task.risk)
                && (risk_rank(&task.risk) < risk_rank("high") || task.review_independent)
        })
}

pub(super) fn valid_ci_labels(labels: &[String]) -> bool {
    labels.len() <= 20
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 100
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/ ".contains(&byte))
                && label.trim() == label
        })
}

fn authorize_ci(context: &Context, task: &mut Task) -> Result<()> {
    authorize_ci_with(task, |task| add_ci_labels(context, task))
}

fn add_ci_labels(context: &Context, task: &Task) -> Result<()> {
    super::delivery::task_add_ci_labels(
        &context.repository,
        &context.github,
        task.pr.ok_or("PRがありません")?,
        task.head.as_deref().ok_or("HEADがありません")?,
        task.base.as_deref().ok_or("baseがありません")?,
        &task.ci_labels,
    )
}

fn authorize_ci_with(
    task: &mut Task,
    mut add_labels: impl FnMut(&Task) -> Result<()>,
) -> Result<()> {
    if task.phase != Phase::CiAuthorization
        || task.pause_requested
        || !review_ready(task)
        || task.base.is_none()
        || task.next_check > now()
    {
        return Ok(());
    }
    if !task.ci_labels.is_empty() {
        let result = add_labels(task);
        if let Err(reason) = result {
            task.event("ci_authorization_wait", &reason);
            task.ci_state = None;
            task.next_check = now() + 120;
            task.recovery.next_authorization = now() + 120;
            task.diagnose(&reason);
            return Ok(());
        }
    }
    task.phase = Phase::Integration;
    task.ci_activation_until = if task.ci_labels.is_empty() {
        0
    } else {
        now() + 300
    };
    task.invalidate_ci();
    task.next_check = 0;
    task.event(
        "ci_authorized",
        "受付時に明示された既存CI起動操作を確認しました。CI待機はRustが担当します",
    );
    Ok(())
}

fn retry_ci_authorization_with(
    task: &mut Task,
    mut add_labels: impl FnMut(&Task) -> Result<()>,
) -> Result<()> {
    if task.phase != Phase::Waiting
        || task.pause_requested
        || !review_ready(task)
        || task.effective_phase() != &Phase::CiAuthorization
        || task.ci_labels.is_empty()
        || task.base.is_none()
        || task.recovery.next_authorization > now()
    {
        return Ok(());
    }
    match add_labels(task) {
        Ok(()) => {
            task.phase = Phase::Integration;
            task.invalidate_ci();
            task.ci_activation_until = now() + 300;
            task.next_check = 0;
            task.recovery.authorization_attempts = 0;
            task.event(
                "ci_authorized",
                "委任済みのlabel操作を再照合して確認しました。新権限は追加せず実CIを監視します",
            );
        }
        Err(reason) => {
            task.recovery.authorization_attempts =
                task.recovery.authorization_attempts.saturating_add(1);
            task.recovery.next_authorization =
                now() + (120u64 << task.recovery.authorization_attempts.min(5)).min(3600);
            if !task.events.last().is_some_and(|event| {
                event.kind == "ci_authorization_wait" && event.detail == reason
            }) {
                task.event("ci_authorization_wait", &reason);
            }
        }
    }
    Ok(())
}

fn reconcile(context: &Context, children: &mut Vec<Child>) -> Result<Value> {
    for child in children.iter_mut() {
        let _ = child.try_wait();
    }
    let mut tasks = {
        let _lock = context.store.lock()?;
        context.store.list()?
    };
    for task in &mut tasks {
        let original_sequence = task.event_sequence;
        let original_revision = task.revision;
        recover_legacy(task);
        // APIとbuildの待機中にstate lockを保持しない。
        if let Err(reason) = apply_report(context, task) {
            task.recovery.attempts += 1;
            if task.recovery.attempts > 3 {
                task.monitor(&reason);
            } else {
                task.diagnose(&reason);
            }
        }
        if task.run.is_none() {
            retry_ci_authorization_with(task, |task| add_ci_labels(context, task))?;
            // コードreview後、CI結果の失敗分類より先に委任済みの認可を行う。
            authorize_ci(context, task)?;
            observe_ci(context, task)?;
            authorize_ci(context, task)?;
            monitor_task(context, task);
            if task.phase == Phase::Diagnosis && task.recovery.attempts > 3 {
                task.monitor("同じ観測で診断が反復しています。別の方法と外部解消条件を記録し、状態変化を監視します");
            }
        }
        let _lock = context.store.lock()?;
        let current = context.load(&task.id)?;
        task.pause_requested |= current.pause_requested;
        if current.generation != task.generation
            || current.event_sequence != original_sequence
            || current.revision != original_revision
        {
            *task = current;
            continue;
        }
        context.store.save(task)?;
    }
    let capacity = host_capacity(context)?;
    tasks.sort_by_key(|task| {
        (
            std::cmp::Reverse(recovery::scheduling_score(task, now())),
            task.created,
            task.id.clone(),
        )
    });
    let mut running = tasks.iter().filter(|task| task.run.is_some()).count();
    let mut launched = Vec::new();
    for index in 0..tasks.len() {
        if running >= capacity {
            break;
        }
        // 他repositoryのbuild/workerも同じhostを使うため、起動直前にも再計測する。
        if resources::Budget::load(&context.store.root)?.capacity(&context.store.root)? == 0 {
            break;
        }
        // 実際の統合起動直前の観測も永続化する。API失敗時に旧成功を残さない。
        if matches!(tasks[index].phase, Phase::Integration | Phase::Finish)
            && tasks[index].run.is_none()
        {
            let original = tasks[index].clone();
            tasks[index].next_check = 0;
            observe_ci(context, &mut tasks[index])?;
            let _lock = context.store.lock()?;
            let current = context.load(&original.id)?;
            if current.event_sequence != original.event_sequence
                || current.revision != original.revision
                || current.generation != original.generation
                || current.pause_requested
            {
                tasks[index] = current;
                continue;
            }
            context.store.save(&tasks[index])?;
        }
        if !ready(context, &tasks[index], &tasks)? {
            continue;
        }
        let lease = match Lock::try_acquire(&context.store.lease_path(&tasks[index].id)?)? {
            Some(lease) => lease,
            None => continue,
        };
        let budget = resources::Budget::load(&context.store.root)?;
        let admission_root = context.codex_home.join("worktrees/.task-admission");
        let Some(admission) = resources::Reservation::acquire(&admission_root, &budget)? else {
            continue;
        };
        let task = &mut tasks[index];
        {
            let _lock = context.store.lock()?;
            let current = context.load(&task.id)?;
            if current.event_sequence != task.event_sequence
                || current.revision != task.revision
                || current.generation != task.generation
            {
                *task = current;
                continue;
            }
        }
        if task.phase != Phase::Diagnosis && task.phase != Phase::Finish {
            match worktree::task_workspace(&context.repository, &task.id, &task.branch) {
                Ok(workspace) => task.worktree = Some(workspace),
                Err(error) => {
                    task.diagnose(&error.to_string());
                    let _lock = context.store.lock()?;
                    let current = context.load(&task.id)?;
                    if current.revision == task.revision
                        && current.event_sequence + 1 == task.event_sequence
                        && current.generation == task.generation
                        && !current.pause_requested
                    {
                        context.store.save(task)?;
                    } else {
                        *task = current;
                    }
                    continue;
                }
            }
        }
        {
            let _lock = context.store.lock()?;
            let current = context.load(&task.id)?;
            if current.event_sequence != task.event_sequence
                || current.revision != task.revision
                || current.generation != task.generation
                || current.pause_requested
            {
                *task = current;
                continue;
            }
            task.generation += 1;
            task.recovery.last_scheduled = now();
            task.run = Some(Run {
                generation: task.generation,
                phase: task.phase.clone(),
                started: now(),
            });
            task.event("worker_start", &format!("{:?}", task.phase));
            context.store.save(task)?;
        }
        match worker::spawn(context, task, &lease, &admission) {
            Ok(child) => {
                children.push(child);
                launched.push(task.id.clone());
                running += 1;
            }
            Err(reason) => {
                let _lock = context.store.lock()?;
                task.run = None;
                task.diagnose(&reason);
                context.store.save(task)?;
            }
        }
    }
    Ok(
        json!({"launched":launched,"running":running,"tasks":tasks.iter().map(|task| json!({"task_id":task.id,"phase":task.phase,"running":task.run.is_some()})).collect::<Vec<_>>()}),
    )
}

fn host_capacity(context: &Context) -> Result<usize> {
    let budget = resources::Budget::load(&context.store.root)?;
    let capacity = budget.capacity(&context.store.root)?;
    if capacity == 0 {
        return Ok(0);
    }
    // 照会用のpermitは直ちに解放する。起動時には別途保持するpermitを原子的に取得する。
    let permit = resources::Reservation::acquire(
        &context.codex_home.join("worktrees/.task-admission"),
        &budget,
    )?;
    Ok(if permit.is_some() { capacity } else { 0 })
}

fn recover_legacy(task: &mut Task) {
    if task.run.is_none()
        && (task.phase == Phase::Blocked
            || task.phase == Phase::AwaitingDecision
                && task
                    .recovery
                    .assessment
                    .as_ref()
                    .is_none_or(|assessment| assessment.decision.is_none()))
    {
        let checkpoint = task.checkpoint.clone();
        task.diagnose(&checkpoint);
    }
}

fn integration_barrier(task: &Task) -> bool {
    task.recovery.barrier && task.phase != Phase::Completed
        || matches!(task.phase, Phase::MainCi | Phase::Finish)
        || matches!(
            task.phase,
            Phase::Blocked | Phase::AwaitingDecision | Phase::Paused
        ) && matches!(
            task.resume_phase,
            Phase::MainCi | Phase::Finish | Phase::Integration
        )
}

fn repair_exempt(task: &Task, barrier: &Task, tasks: &[Task]) -> bool {
    let mut cursor = task;
    let mut visited = std::collections::BTreeSet::new();
    while cursor.recovery.repairs_main && visited.len() < 4 && visited.insert(cursor.id.clone()) {
        let Some(parent) = tasks
            .iter()
            .find(|parent| cursor.recovery.parent.as_ref() == Some(&parent.id))
        else {
            return false;
        };
        if parent.recovery.repair_task.as_ref() != Some(&cursor.id) {
            return false;
        }
        if parent.id == barrier.id {
            return true;
        }
        cursor = parent;
    }
    false
}

fn run_loop(
    context: &Context,
    poll_seconds: u64,
    timeout_seconds: Option<u64>,
    coordinator: &mut Option<Lock>,
) -> Result<Value> {
    let mut children = Vec::new();
    let started = Instant::now();
    loop {
        let result = reconcile(context, &mut children)?;
        {
            // 登録とidle終了を直列化し、終了直前の追加依頼を取りこぼさない。
            let _state = context.store.lock()?;
            let tasks = context.store.list()?;
            if timeout_seconds
                .is_some_and(|seconds| started.elapsed() >= Duration::from_secs(seconds))
                || tasks.iter().all(|task| {
                    task.run.is_none()
                        && matches!(
                            task.phase,
                            Phase::Completed
                                | Phase::Blocked
                                | Phase::AwaitingDecision
                                | Phase::Paused
                                | Phase::BoundaryReached
                        )
                })
            {
                coordinator.take();
                return Ok(result);
            }
        }
        std::thread::sleep(Duration::from_secs(poll_seconds));
    }
}

fn start(context: &Context) -> Result<Value> {
    worker::preflight(context)?;
    let Some(lock) = Lock::try_acquire(&context.store.root.join("coordinator.lock"))? else {
        return Ok(json!({"coordinator":"already_running"}));
    };
    let mut command = std::process::Command::new(
        std::env::current_exe().map_err(|_| "coordinator実行fileを確認できません")?,
    );
    worker::configure_environment(&mut command, context);
    command
        .current_dir(&context.repository)
        .args(["coordinator", "--lock-fd", &lock.fd().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.arg0("codex-task");
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    lock.inherit()?;
    let child = command.spawn().map_err(|_| "coordinatorを起動できません")?;
    Ok(json!({"coordinator":"started","pid":child.id()}))
}

fn execute(cli: Cli) -> Result<Value> {
    let context = Context::open()?;
    match cli.command {
        Commands::Submit {
            task_id,
            objective,
            authorization,
            branch,
            depends_on,
            resource,
            risk,
            ci_label,
            gate_mode,
            stop_after,
        } => {
            let requested_gate = if gate_mode == "github-free-private" {
                ci::GateMode::GithubFreePrivate
            } else {
                ci::GateMode::Strict
            };
            let stop_after = match stop_after.as_str() {
                "pr" => recovery::Boundary::Pr,
                "merge" => recovery::Boundary::Merge,
                _ => recovery::Boundary::Finish,
            };
            submit_with_policy(
                &context,
                task_id,
                objective.clone(),
                authorization.unwrap_or(objective),
                branch,
                depends_on,
                resource,
                risk,
                ci_label,
                requested_gate,
                stop_after,
            )
        }
        Commands::List => {
            let tasks = context.store.list()?;
            let capacity = host_capacity(&context)?;
            Ok(json!(tasks.iter().map(|task| json!({"task_id":task.id,"phase":task.phase,"running":task.run.is_some(),"updated":task.updated,"next_check":task.next_check,"waiting_on":waiting_on(task,&tasks,capacity),"recovery":task.recovery})).collect::<Vec<_>>()))
        }
        Commands::Show { task_id } => Ok(json!(context.load(&task_id)?)),
        Commands::Events { task_id, all } => {
            let task = context.load(&task_id)?;
            Ok(
                json!({"task_id":task_id,"acknowledged_sequence":task.acknowledged_sequence,"event_sequence":task.event_sequence,
                "events":task.events.iter().filter(|event| all || event.sequence > task.acknowledged_sequence).collect::<Vec<_>>()}),
            )
        }
        Commands::Ack { task_id, through } => {
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            task.acknowledge(through)?;
            context.store.save(&task)?;
            Ok(json!({"task_id":task_id,"acknowledged_sequence":through}))
        }
        Commands::Retention { task_id, seconds } => {
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            task.retention_seconds = seconds;
            task.revision += 1;
            context.store.save(&task)?;
            Ok(json!({"task_id":task_id,"retention_seconds":seconds}))
        }
        Commands::Collect { task_id } => {
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            let _lease = Lock::try_acquire(&context.store.lease_path(&task_id)?)?
                .ok_or("workerが動作中です")?;
            let removed = context.store.collect(&mut task, now())?;
            context.store.save(&task)?;
            Ok(
                json!({"task_id":task_id,"removed_worker_generations":removed,"retained_events":task.events.len()}),
            )
        }
        Commands::Pause { task_id } => {
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            task.pause_requested = true;
            if task.run.is_none() {
                task.stop(Phase::Paused, "利用者の指示で停止しました");
            }
            task.event("pause_requested", "次のworker起動を停止します");
            context.store.save(&task)?;
            Ok(json!({"task_id":task_id,"pause_requested":true}))
        }
        Commands::Resume {
            task_id,
            instruction,
            gate_mode,
            authorization,
        } => resume_with_options(&context, &task_id, instruction, gate_mode, authorization),
        Commands::Reconcile | Commands::Run { .. } => {
            worker::preflight(&context)?;
            let mut coordinator = Some(
                Lock::try_acquire(&context.store.root.join("coordinator.lock"))?
                    .ok_or("このrepositoryのcoordinatorは既に稼働中です")?,
            );
            let mut children = Vec::new();
            if matches!(cli.command, Commands::Reconcile) {
                return reconcile(&context, &mut children);
            }
            let Commands::Run {
                poll_seconds,
                timeout_seconds,
            } = cli.command
            else {
                unreachable!()
            };
            run_loop(&context, poll_seconds, timeout_seconds, &mut coordinator)
        }
        Commands::Start => start(&context),
        Commands::Coordinator { lock_fd } => {
            let lock = unsafe {
                Lock::from_inherited(lock_fd, &context.store.root.join("coordinator.lock"))?
            };
            lock.close_on_exec()?;
            worker::preflight(&context)?;
            let mut coordinator = Some(lock);
            let result = run_loop(&context, 15, None, &mut coordinator);
            store::atomic_json(
                &context.store.root.join("coordinator-status.json"),
                &json!({"stopped":true,"at":now(),"error":result.as_ref().err()}),
            )?;
            result
        }
        Commands::Doctor => worker::doctor(&context),
        Commands::LaunchCommand => Ok(
            json!({"executable":context.home.join(".local/bin/codex-task"),"arguments":["run"],"working_directory":context.repository,"codex_home":context.codex_home,"timer_required":false,"note":"OS起動adapterは任意です。WSL停止中は待機し、次回稼働時に照合します。"}),
        ),
        Commands::Worker {
            task_id,
            generation,
            lease_fd,
            admission_fd,
            admission_slot,
        } => worker::execute(
            &context,
            &task_id,
            generation,
            lease_fd,
            admission_fd,
            admission_slot,
        ),
    }
}

fn waiting_on(task: &Task, tasks: &[Task], capacity: usize) -> &'static str {
    if task.run.is_some() {
        return "running";
    }
    match task.phase {
        Phase::AwaitingDecision => return "user-decision",
        Phase::Paused => return "user-paused",
        Phase::BoundaryReached | Phase::Completed => return "complete",
        Phase::Waiting => return "external-observation",
        Phase::CiAuthorization | Phase::MainCi => return "ci",
        _ => (),
    }
    if task.dependencies.iter().any(|id| {
        tasks
            .iter()
            .find(|other| &other.id == id)
            .is_none_or(|other| !matches!(other.phase, Phase::Completed | Phase::BoundaryReached))
    }) {
        return "dependency";
    }
    if task.phase != Phase::Diagnosis
        && tasks.iter().any(|other| {
            other.id != task.id
                && other.run.is_some()
                && other
                    .resources
                    .iter()
                    .any(|resource| task.resources.contains(resource))
        })
    {
        return "shared-resource";
    }
    if capacity == 0 {
        return "host-capacity";
    }
    if task.phase == Phase::Integration
        && task
            .ci_evidence
            .as_ref()
            .is_none_or(|ci| !ci.allows_delivery())
    {
        return "ci";
    }
    if matches!(task.phase, Phase::Integration | Phase::Finish)
        && tasks.iter().any(|other| {
            other.id != task.id && integration_barrier(other) && !repair_exempt(task, other, tasks)
        })
    {
        return "integration-barrier";
    }
    "eligible"
}

pub fn entrypoint<I: IntoIterator<Item = OsString>>(args: I) -> i32 {
    let parsed = Cli::try_parse_from(std::iter::once(OsString::from("codex-task")).chain(args));
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    let structured = cli.json;
    match execute(cli) {
        Ok(value) => {
            let code = if value.get("ready").and_then(Value::as_bool) == Some(false) {
                1
            } else {
                0
            };
            if structured {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            } else {
                print_human(&value);
            }
            code
        }
        Err(error) => {
            eprintln!("codex-task: {error}");
            1
        }
    }
}

fn print_human(value: &Value) {
    if let Some(rows) = value.as_array() {
        for row in rows {
            if let Some(id) = row.get("task_id").and_then(Value::as_str) {
                println!(
                    "{id}\t{}\t{}",
                    row.get("phase")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    if row.get("running").and_then(Value::as_bool).unwrap_or(false) {
                        "実行中"
                    } else {
                        "待機"
                    }
                );
            } else {
                println!("{row}");
            }
        }
    } else if let Some(tasks) = value.get("tasks") {
        print_human(tasks);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }
}

pub(crate) fn invocation_reason(args: &[String]) -> Option<String> {
    let parsed =
        Cli::try_parse_from(std::iter::once("codex-task".to_string()).chain(args.iter().cloned()));
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error)
            if error.exit_code() == 0
                && matches!(args, [flag] if flag == "--help" || flag == "--version") =>
        {
            return None;
        }
        Err(_) => return Some("codex-taskの引数を正規形で指定してください".into()),
    };
    match cli.command {
        Commands::Worker { .. } | Commands::Coordinator { .. } => {
            Some("内部processはtask helperだけが起動できます".into())
        }
        Commands::Submit {
            task_id,
            objective,
            branch,
            depends_on,
            resource,
            ci_label,
            ..
        } => (!valid_id(&task_id)
            || !safe_text(&objective, 32 * 1024)
            || branch.is_some_and(|branch| {
                !branch.starts_with("feat/") || branch.contains(['\n', '\r', '\0'])
            })
            || depends_on.iter().any(|id| !valid_id(id) || id == &task_id)
            || resource.iter().any(|name| !resource_name(name))
            || !valid_ci_labels(&ci_label))
        .then(|| "codex-taskの登録内容が不正です".into()),
        Commands::Show { task_id }
        | Commands::Events { task_id, .. }
        | Commands::Pause { task_id }
        | Commands::Ack { task_id, .. }
        | Commands::Retention { task_id, .. }
        | Commands::Collect { task_id } => {
            (!valid_id(&task_id)).then(|| "task IDが不正です".into())
        }
        Commands::Resume {
            task_id,
            instruction,
            authorization,
            ..
        } => (!valid_id(&task_id) || !safe_text(&instruction, 32 * 1024))
            .then(|| "再開指示が不正です".into())
            .or_else(|| {
                authorization
                    .is_some_and(|text| !safe_text(&text, 32 * 1024))
                    .then(|| "元の利用者依頼が不正です".into())
            }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub(super) struct Fixture(pub(super) Context);
    impl Fixture {
        pub(super) fn new() -> Self {
            let base = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            fs::create_dir_all(&base).unwrap();
            let root = base.join(format!(
                "task-test-{}-{}",
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
        pub(super) fn submit(&self, id: &str) {
            submit(
                &self.0,
                id.into(),
                "許可された実装依頼".into(),
                None,
                vec![],
                vec![],
                "high".into(),
                vec![],
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.home);
        }
    }

    pub(super) fn successful_observation() -> ci::Observation {
        serde_json::from_value(json!({
            "state":"success","head":"a".repeat(40),"base":"b".repeat(40),
            "target":"a".repeat(40),"fingerprint":"observed","merge_commit":null,
            "merged":false,"observed_at":now(),"required":[],
            "snapshot":{"runs":[],"statuses":[]}
        }))
        .unwrap()
    }

    #[test]
    fn unacknowledged_events_survive_the_old_limit_and_ack_is_explicit() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        for _ in 0..300 {
            task.event("test", "未処理");
        }
        fixture.0.store.save(&task).unwrap();
        let mut task = fixture.0.load("task-one").unwrap();
        assert_eq!(task.events.len(), 300);
        assert_eq!(task.events[0].sequence, 1);
        task.acknowledge(128).unwrap();
        assert_eq!(
            task.events
                .iter()
                .filter(|event| event.sequence > task.acknowledged_sequence)
                .count(),
            172
        );
        assert!(task.acknowledge(127).is_err());
        assert!(task.acknowledge(301).is_err());
        // readerのackが入ったsnapshotをcoordinatorは上書きできない。
        assert_eq!(task.revision, 1);
    }

    #[test]
    fn old_state_defaults_to_unacknowledged_and_no_ci_evidence() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let task = fixture.0.load("task-one").unwrap();
        let mut legacy = serde_json::to_value(&task).unwrap();
        for field in [
            "acknowledged_sequence",
            "retention_seconds",
            "revision",
            "ci_evidence",
            "ci_observation_error_at",
            "gate_mode",
        ] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        let loaded: Task = serde_json::from_value(legacy).unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.acknowledged_sequence, 0);
        assert_eq!(loaded.retention_seconds, 30 * 24 * 60 * 60);
        assert!(loaded.ci_evidence.is_none());
        assert_eq!(loaded.delivery_gate(), ci::GateMode::Strict);
    }

    #[test]
    fn state_capacity_errors_preserve_the_previously_saved_events() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.event("test", "引継ぎ");
        fixture.0.store.save(&task).unwrap();
        task.event("test", &"x".repeat(1024 * 1024));
        assert!(fixture.0.store.save(&task).is_err());
        assert_eq!(fixture.0.load("task-one").unwrap().events.len(), 1);
    }

    #[test]
    fn collection_preserves_active_unacknowledged_and_latest_handoff() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.generation = 3;
        task.event("test", "最後の引継ぎ");
        task.checkpoint = "保持する判断".into();
        task.retention_seconds = 0;
        for generation in 1..=2 {
            store::atomic_json(
                &worker::report_path(&fixture.0.store, &task.id, generation).unwrap(),
                &json!({"handoff":generation}),
            )
            .unwrap();
        }
        let future = now() + 10;
        assert!(fixture.0.store.collect(&mut task, future).is_err());
        task.phase = Phase::Completed;
        assert!(fixture.0.store.collect(&mut task, future).is_err());
        task.acknowledge(task.event_sequence).unwrap();
        task.run = Some(Run {
            generation: 3,
            phase: Phase::Finish,
            started: now(),
        });
        assert!(fixture.0.store.collect(&mut task, future).is_err());
        task.run = None;
        task.retention_seconds = 100;
        assert!(
            fixture
                .0
                .store
                .collect(&mut task, future)
                .unwrap()
                .is_empty()
        );
        task.retention_seconds = 0;
        assert_eq!(fixture.0.store.collect(&mut task, future).unwrap(), vec![1]);
        assert!(
            worker::report_path(&fixture.0.store, &task.id, 2)
                .unwrap()
                .exists()
        );
        assert!(task.events.is_empty());
        assert_eq!(task.checkpoint, "保持する判断");
        fixture.0.store.save(&task).unwrap();
        assert_eq!(
            fixture.0.load("task-one").unwrap().checkpoint,
            task.checkpoint
        );
        assert!(
            fixture
                .0
                .store
                .collect(&mut task, future)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn collection_rejects_unknown_files_before_deleting_any_generation() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.phase = Phase::Completed;
        task.generation = 3;
        task.retention_seconds = 0;
        let first = worker::report_path(&fixture.0.store, &task.id, 1).unwrap();
        store::atomic_json(&first, &json!({})).unwrap();
        let second = worker::run_dir(&fixture.0.store, &task.id, 2).unwrap();
        fs::write(second.join("unknown"), "固有data").unwrap();
        assert!(fixture.0.store.collect(&mut task, now() + 10).is_err());
        assert!(first.exists());
    }

    #[test]
    fn ci_evidence_is_persisted_and_invalidated_on_api_error_head_or_base_change() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.head = Some("a".repeat(40));
        task.base = Some("b".repeat(40));
        task.phase = Phase::Review;
        record_ci_result(&mut task, Ok(successful_observation())).unwrap();
        fixture.0.store.save(&task).unwrap();
        let loaded = fixture.0.load("task-one").unwrap();
        assert_eq!(loaded.ci_evidence, task.ci_evidence);
        record_ci_result(&mut task, Err("API unavailable".into())).unwrap();
        assert!(task.ci_state.is_none() && task.ci_evidence.is_none());
        assert!(task.ci_observation_error_at.is_some());
        record_ci_result(&mut task, Ok(successful_observation())).unwrap();
        assert!(task.ci_observation_error_at.is_none());
        let mut changed = successful_observation();
        changed.base = "c".repeat(40);
        record_ci_result(&mut task, Ok(changed)).unwrap();
        assert_eq!(task.phase, Phase::Implementation);
        assert!(task.ci_state.is_none() && task.ci_evidence.is_none());
        task.phase = Phase::Review;
        let mut changed = successful_observation();
        changed.head = "d".repeat(40);
        record_ci_result(&mut task, Ok(changed)).unwrap();
        assert_eq!(task.phase, Phase::Diagnosis);
        assert!(task.ci_state.is_none() && task.ci_evidence.is_none());
    }

    fn local_observation(merged: bool) -> ci::Observation {
        let tree = |sha: &str| json!({"commit":sha,"tree":sha,"workflows":[]});
        let mut value = serde_json::to_value(successful_observation()).unwrap();
        value["state"] = json!("local_validation");
        value["gate_mode"] = json!("local-validation");
        value["workflows"] = json!({
            "base":tree(&"b".repeat(40)),"head":tree(&"a".repeat(40)),"main":null
        });
        if merged {
            value["merged"] = json!(true);
            value["merge_commit"] = json!("c".repeat(40));
            value["target"] = json!("c".repeat(40));
            value["workflows"]["main"] = tree(&"c".repeat(40));
        }
        serde_json::from_value(value).unwrap()
    }

    fn reviewed_task(fixture: &Fixture) -> Task {
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.phase = Phase::CiAuthorization;
        task.pr = Some(12);
        task.head = Some("a".repeat(40));
        task.base = Some("b".repeat(40));
        task.review = Some(worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "固定headの独立review済み".into(),
            pr: task.pr,
            head: task.head.clone(),
            risk: "high".into(),
            assessment: None,
        });
        task.review_independent = true;
        task
    }

    #[test]
    fn delivery_workers_preserve_the_existing_strict_and_local_argument_contract() {
        let fixture = Fixture::new();
        let mut task = reviewed_task(&fixture);
        for phase in [Phase::Integration, Phase::Finish] {
            task.phase = phase;
            task.gate_mode = ci::GateMode::Strict;
            let strict = worker::prompt(&task);
            assert!(strict.contains("--gate-mode を省略"));
            assert!(!strict.contains("--gate-mode strict"));
            assert!(!strict.contains("--gate-mode local-validation"));
            task.gate_mode = ci::GateMode::LocalValidation;
            let local = worker::prompt(&task);
            assert!(
                local.contains(
                    "record-review・deliver・finishにはすべて --gate-mode local-validation"
                )
            );
            assert!(!local.contains("--gate-mode を省略"));
        }
    }

    #[test]
    fn local_gate_connects_review_integration_main_and_finish_without_ci_success() {
        let fixture = Fixture::new();
        let mut task = reviewed_task(&fixture);
        record_ci_result(&mut task, Ok(local_observation(false))).unwrap();
        assert_eq!(task.phase, Phase::Integration);
        assert_eq!(task.ci_state, Some(ci::CiState::LocalValidation));
        assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
        fixture.0.store.save(&task).unwrap();
        let mut task = fixture.0.load(&task.id).unwrap();
        assert_eq!(task.delivery_gate(), ci::GateMode::LocalValidation);
        task.phase = Phase::MainCi;
        record_ci_result(&mut task, Ok(local_observation(true))).unwrap();
        assert_eq!(task.phase, Phase::Finish);
        assert_eq!(task.merge_commit, Some("c".repeat(40)));
        assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
        record_ci_result(&mut task, Err("API unavailable".into())).unwrap();
        assert_eq!(task.delivery_gate(), ci::GateMode::LocalValidation);
        assert!(task.ci_evidence.is_none());
        assert_eq!(task.phase, Phase::Diagnosis);
        assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
        task.phase = Phase::Finish;
        record_ci_result(&mut task, Ok(local_observation(true))).unwrap();
        assert!(ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
    }

    #[test]
    fn local_gate_rejects_missing_review_old_observations_and_mismatched_evidence() {
        let fixture = Fixture::new();
        let mut original = reviewed_task(&fixture);
        record_ci_result(&mut original, Ok(local_observation(false))).unwrap();
        for case in 0..10 {
            let mut task = original.clone();
            match case {
                0 => task.review = None,
                1 => task.review_independent = false,
                2 => task.ci_evidence.as_mut().unwrap().observed_at = now() - 121,
                3 => task.ci_evidence.as_mut().unwrap().observed_at = now() + 30,
                4 => task.ci_evidence.as_mut().unwrap().head = "d".repeat(40),
                5 => task.ci_evidence.as_mut().unwrap().base = "d".repeat(40),
                6 => task.ci_evidence.as_mut().unwrap().workflows = None,
                7 => task.ci_state = Some(ci::CiState::Success),
                8 => task.gate_mode = ci::GateMode::Strict,
                _ => task.ci_evidence = None,
            }
            assert!(
                !ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn local_push_and_base_changes_discard_both_ci_and_independent_review() {
        let fixture = Fixture::new();
        let mut original = reviewed_task(&fixture);
        record_ci_result(&mut original, Ok(local_observation(false))).unwrap();
        for head_changed in [true, false] {
            let mut task = original.clone();
            let mut observation = local_observation(false);
            if head_changed {
                observation.head = "d".repeat(40);
            } else {
                observation.base = "d".repeat(40);
            }
            record_ci_result(&mut task, Ok(observation)).unwrap();
            assert!(task.review.is_none() && !task.review_independent);
            assert!(task.ci_evidence.is_none() && task.ci_state.is_none());
            assert_eq!(
                task.phase,
                if head_changed {
                    Phase::Diagnosis
                } else {
                    Phase::Implementation
                }
            );
            assert!(!review_ready(&task));
        }
    }

    #[test]
    fn local_main_gate_keeps_barrier_for_unmerged_wrong_head_wrong_merge_or_failure() {
        let fixture = Fixture::new();
        let mut original = reviewed_task(&fixture);
        record_ci_result(&mut original, Ok(local_observation(false))).unwrap();
        original.phase = Phase::MainCi;
        for case in 0..4 {
            let mut task = original.clone();
            let mut observation = local_observation(true);
            match case {
                0 => observation.merged = false,
                1 => observation.head = "d".repeat(40),
                2 => observation.merge_commit = Some("d".repeat(40)),
                _ => observation.state = ci::CiState::Failure,
            }
            record_ci_result(&mut task, Ok(observation)).unwrap();
            assert_eq!(task.phase, Phase::Diagnosis);
            assert!(integration_barrier(&task));
        }
    }

    #[test]
    fn duplicate_submission_is_idempotent_and_other_intent_is_not_adopted() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        fixture.submit("task-one");
        assert_eq!(fixture.0.store.list().unwrap().len(), 1);
        assert!(
            submit(
                &fixture.0,
                "task-one".into(),
                "別の作業".into(),
                None,
                vec![],
                vec![],
                "high".into(),
                vec![]
            )
            .is_err()
        );
        assert!(fixture.0.load("task-one").unwrap().worktree.is_none());
    }

    #[test]
    fn a_dependency_must_exist_and_cannot_be_self_or_a_cycle() {
        let fixture = Fixture::new();
        assert!(
            submit(
                &fixture.0,
                "task-one".into(),
                "実装".into(),
                None,
                vec!["task-missing".into()],
                vec![],
                "high".into(),
                vec![]
            )
            .is_err()
        );
        fixture.submit("task-one");
        submit(
            &fixture.0,
            "task-two".into(),
            "実装".into(),
            None,
            vec!["task-one".into()],
            vec![],
            "high".into(),
            vec![],
        )
        .unwrap();
        assert!(
            submit(
                &fixture.0,
                "task-one".into(),
                "実装".into(),
                None,
                vec!["task-two".into()],
                vec![],
                "high".into(),
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn distinct_tasks_can_hold_writer_leases_but_the_same_task_cannot() {
        let fixture = Fixture::new();
        let first = fixture.0.store.lease_path("task-one").unwrap();
        let second = fixture.0.store.lease_path("task-two").unwrap();
        let lease = Lock::try_acquire(&first).unwrap().unwrap();
        let other = Lock::try_acquire(&second).unwrap().unwrap();
        assert!(Lock::try_acquire(&first).unwrap().is_none());
        drop(lease);
        assert!(Lock::try_acquire(&first).unwrap().is_some());
        drop(other);
    }

    #[test]
    fn inherited_writer_lease_survives_its_supervisor() {
        let fixture = Fixture::new();
        let path = fixture.0.store.lease_path("task-one").unwrap();
        let lease = Lock::try_acquire(&path).unwrap().unwrap();
        lease.inherit().unwrap();
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        drop(lease);
        let retained = Lock::try_acquire(&path).unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            retained,
            "coordinator終了後も旧workerがwriter lockを保持する"
        );
        assert!(Lock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn coordinator_restart_does_not_consume_a_report_while_old_worker_is_alive() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.generation = 1;
        task.failures = 2;
        task.head = Some("a".repeat(40));
        task.run = Some(Run {
            generation: 1,
            phase: Phase::Implementation,
            started: now(),
        });
        let report = worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "固定HEADのDraft PRを作成しました".into(),
            pr: Some(12),
            head: Some("b".repeat(40)),
            risk: "medium".into(),
            assessment: None,
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 1).unwrap(),
            &report,
        )
        .unwrap();
        let lease = Lock::try_acquire(&fixture.0.store.lease_path(&task.id).unwrap())
            .unwrap()
            .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Implementation);
        assert!(task.run.is_some());
        drop(lease);
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Review);
        assert_eq!(task.risk, "high");
        assert!(task.run.is_none());
        assert_eq!(task.pr, Some(12));
        assert_eq!(task.failures, 2);
    }

    #[test]
    fn stale_report_and_missing_report_never_relaunch_a_writer() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.generation = 2;
        task.run = Some(Run {
            generation: 2,
            phase: Phase::Implementation,
            started: now(),
        });
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Diagnosis);
        assert!(task.run.is_none());
        assert_eq!(task.failures, 1);
        task.phase = Phase::Implementation;
        task.run = Some(Run {
            generation: 2,
            phase: Phase::Implementation,
            started: now(),
        });
        let report = worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: String::new(),
            pr: Some(1),
            head: Some("b".repeat(40)),
            risk: "low".into(),
            assessment: None,
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 2).unwrap(),
            &report,
        )
        .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Diagnosis);
        assert!(task.run.is_none());
        fixture.0.store.save(&task).unwrap();
        resume_task(
            &fixture.0,
            &task.id,
            "旧報告を保持して新しい実行世代で再開する".into(),
        )
        .unwrap();
        let resumed = fixture.0.load(&task.id).unwrap();
        assert_eq!(resumed.phase, Phase::Implementation);
        assert_eq!(resumed.generation, 2);
    }

    #[test]
    fn failed_main_ci_keeps_the_integration_barrier() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        fixture.submit("task-two");
        let mut first = fixture.0.load("task-one").unwrap();
        first.phase = Phase::MainCi;
        first.stop(Phase::Blocked, "main CI失敗");
        let mut second = fixture.0.load("task-two").unwrap();
        second.phase = Phase::Integration;
        assert!(integration_barrier(&first));
        assert!(!ready(&fixture.0, &second, &[first, second.clone()]).unwrap());
    }

    #[test]
    fn changed_pr_head_resumes_implementation_without_reusing_the_review() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.phase = Phase::Integration;
        task.head = Some("a".repeat(40));
        task.pr = Some(12);
        task.review = Some(worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "旧レビュー".into(),
            pr: task.pr,
            head: task.head.clone(),
            risk: "high".into(),
            assessment: None,
        });
        stale_head(&mut task);
        fixture.0.store.save(&task).unwrap();
        assert_eq!(task.phase, Phase::Diagnosis);
        assert!(task.review.is_none());
        resume_task(
            &fixture.0,
            &task.id,
            "更新されたPRを照合して継続する".into(),
        )
        .unwrap();
        let resumed = fixture.0.load(&task.id).unwrap();
        assert_eq!(resumed.phase, Phase::Implementation);
        assert!(ready(&fixture.0, &resumed, std::slice::from_ref(&resumed)).unwrap());
    }

    #[test]
    fn trusted_low_risk_input_can_use_self_review_but_worker_cannot_lower_high_risk() {
        let fixture = Fixture::new();
        submit(
            &fixture.0,
            "task-low".into(),
            "局所的な変更".into(),
            None,
            vec![],
            vec![],
            "low".into(),
            vec![],
        )
        .unwrap();
        let mut task = fixture.0.load("task-low").unwrap();
        task.generation = 1;
        task.run = Some(Run {
            generation: 1,
            phase: Phase::Implementation,
            started: now(),
        });
        let report = worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "self-review済み".into(),
            pr: Some(1),
            head: Some("b".repeat(40)),
            risk: "low".into(),
            assessment: None,
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 1).unwrap(),
            &report,
        )
        .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::CiAuthorization);
        assert!(task.review.is_some());
    }

    #[test]
    fn ci_authorization_has_no_ai_worker_and_waits_on_api_outage() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.phase = Phase::CiAuthorization;
        task.pr = Some(12);
        task.base = Some("b".repeat(40));
        task.head = Some("a".repeat(40));
        task.ci_state = Some(ci::CiState::Pending);
        assert!(!ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
        task.review = Some(worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "固定HEADのレビュー済み".into(),
            pr: task.pr,
            head: task.head.clone(),
            risk: "high".into(),
            assessment: None,
        });
        task.review_independent = true;
        assert!(!worker::worker_phase(&task.phase));
        for state in [ci::CiState::Pending, ci::CiState::Missing] {
            task.phase = Phase::CiAuthorization;
            task.ci_state = Some(state);
            assert!(!ready(&fixture.0, &task, std::slice::from_ref(&task)).unwrap());
            authorize_ci(&fixture.0, &mut task).unwrap();
            assert_eq!(task.phase, Phase::Integration);
        }
        for state in [
            None,
            Some(ci::CiState::Failure),
            Some(ci::CiState::Cancelled),
        ] {
            task.phase = Phase::CiAuthorization;
            task.ci_state = state;
            authorize_ci(&fixture.0, &mut task).unwrap();
            assert_eq!(task.phase, Phase::Integration);
        }
    }

    #[test]
    fn risk_increase_invalidates_self_review_before_integration_can_continue() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.risk = "low".into();
        task.phase = Phase::Integration;
        task.pr = Some(12);
        task.head = Some("a".repeat(40));
        task.review = Some(worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "局所変更のself-review".into(),
            pr: task.pr,
            head: task.head.clone(),
            risk: "low".into(),
            assessment: None,
        });
        assert!(review_ready(&task));
        task.generation = 2;
        task.run = Some(Run {
            generation: 2,
            phase: Phase::Integration,
            started: now(),
        });
        let report = worker::Report {
            generation: 2,
            risk: "high".into(),
            ..task.review.clone().unwrap()
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 2).unwrap(),
            &report,
        )
        .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Review);
        assert!(task.review.is_none());
        assert!(!review_ready(&task));
        assert_eq!(task.risk, "high");
    }

    #[test]
    fn raised_risk_while_awaiting_decision_resumes_an_independent_review() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let mut task = fixture.0.load("task-one").unwrap();
        task.risk = "low".into();
        task.phase = Phase::Integration;
        task.pr = Some(12);
        task.head = Some("a".repeat(40));
        task.review = Some(worker::Report {
            generation: 1,
            outcome: "ready".into(),
            summary: "self-review".into(),
            pr: task.pr,
            head: task.head.clone(),
            risk: "low".into(),
            assessment: None,
        });
        task.generation = 2;
        task.run = Some(Run {
            generation: 2,
            phase: Phase::Integration,
            started: now(),
        });
        let report = worker::Report {
            generation: 2,
            outcome: "awaiting_decision".into(),
            risk: "high".into(),
            ..task.review.clone().unwrap()
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 2).unwrap(),
            &report,
        )
        .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::Diagnosis);
        assert_eq!(task.resume_phase, Phase::Review);
        fixture.0.store.save(&task).unwrap();
        resume_task(&fixture.0, &task.id, "判断を反映してレビューする".into()).unwrap();
        let mut task = fixture.0.load(&task.id).unwrap();
        assert_eq!(task.phase, Phase::Review);
        task.generation = 3;
        task.run = Some(Run {
            generation: 3,
            phase: Phase::Review,
            started: now(),
        });
        let report = worker::Report {
            generation: 3,
            outcome: "ready".into(),
            risk: "medium".into(),
            ..report
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 3).unwrap(),
            &report,
        )
        .unwrap();
        apply_report(&fixture.0, &mut task).unwrap();
        assert_eq!(task.phase, Phase::CiAuthorization);
        assert!(task.review_independent);
        assert!(review_ready(&task));
        assert_eq!(task.risk, "high");
    }

    #[test]
    fn ci_activation_only_constructs_add_existing_label_requests() {
        let args = super::super::delivery::task_ci_label_args(
            "example/product",
            12,
            &["ci-approved".into()],
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "api",
                "repos/example/product/issues/12/labels",
                "--method",
                "POST",
                "-f",
                "labels[]=ci-approved"
            ]
        );
        for label in ["", "--body=overwrite", "label,other", "label\nsecret"] {
            assert!(
                super::super::delivery::task_ci_label_args("example/product", 12, &[label.into()])
                    .is_err()
            );
        }
        assert!(
            super::super::delivery::task_ci_label_args(
                "example/product/other",
                12,
                &["ci-approved".into()]
            )
            .is_err()
        );
    }

    #[test]
    fn unsafe_and_foreign_state_is_not_accepted() {
        let fixture = Fixture::new();
        fixture.submit("task-one");
        let path = fixture.0.store.path("task-one").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(fixture.0.load("task-one").is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut task = fixture.0.load("task-one").unwrap();
        task.host = "b".repeat(64);
        store::atomic_json(&path, &task).unwrap();
        assert!(fixture.0.load("task-one").is_err());
        assert!(fixture.0.store.path("../task-one").is_err());
        let dangling = fixture.0.store.path("task-dangling").unwrap();
        std::os::unix::fs::symlink("/missing-task-state-fixture", &dangling).unwrap();
        assert!(store::atomic_json(&dangling, &task).is_err());
        assert_eq!(
            fs::read_link(dangling).unwrap(),
            PathBuf::from("/missing-task-state-fixture")
        );
    }

    #[test]
    fn internal_worker_and_unknown_arguments_are_not_public_operations() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert!(
            invocation_reason(&args(&[
                "worker",
                "--task-id",
                "task-one",
                "--generation",
                "1",
                "--lease-fd",
                "3"
            ]))
            .is_some()
        );
        assert!(invocation_reason(&args(&["run", "--force"])).is_some());
        assert!(
            invocation_reason(&args(&[
                "submit",
                "--task-id",
                "task-one",
                "--objective",
                "実装"
            ]))
            .is_none()
        );
        assert!(invocation_reason(&args(&["show", "--task-id", "../task-one"])).is_some());
    }
}
