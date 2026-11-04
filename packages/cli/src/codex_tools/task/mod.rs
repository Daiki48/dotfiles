//! OSタイマーに依存しない、ホスト内のタスク制御。

mod ci;
mod resources;
mod store;
mod worker;

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
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        depends_on: Vec<String>,
        #[arg(long)]
        resource: Vec<String>,
    },
    List,
    Show {
        #[arg(long)]
        task_id: String,
    },
    Events {
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
    Integration,
    MainCi,
    Finish,
    Completed,
    AwaitingDecision,
    Blocked,
    Paused,
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
    checkpoint: String,
    ci_fingerprint: String,
    next_check: u64,
    failures: u32,
    pause_requested: bool,
    created: u64,
    updated: u64,
    events: Vec<Event>,
    event_sequence: u64,
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

impl Task {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !valid_id(&self.id)
            || self.host.len() != 64
            || !self.repository.is_absolute()
            || !safe_text(&self.objective, 32 * 1024)
            || !self.github.contains('/')
            || !self.branch.starts_with("feat/")
            || self.branch.contains(['\n', '\r', '\0'])
            || self
                .dependencies
                .iter()
                .any(|id| !valid_id(id) || id == &self.id)
            || self.resources.iter().any(|value| !resource_name(value))
            || self.head.as_deref().is_some_and(|head| !valid_sha(head))
            || self.base.as_deref().is_some_and(|head| !valid_sha(head))
            || self
                .merge_commit
                .as_deref()
                .is_some_and(|head| !valid_sha(head))
            || self.events.len() > 128
            || self.checkpoint.len() > 32 * 1024
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
        if self.events.len() == 128 {
            self.events.remove(0);
        }
        self.events.push(Event {
            sequence: self.event_sequence,
            at: self.updated,
            kind: kind.into(),
            detail: detail.into(),
        });
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

fn submit(
    context: &Context,
    id: String,
    objective: String,
    branch: Option<String>,
    dependencies: Vec<String>,
    resources: Vec<String>,
) -> Result<Value> {
    if !valid_id(&id) || !safe_text(&objective, 32 * 1024) {
        return Err("task IDまたは目的が不正です".into());
    }
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
        checkpoint: String::new(),
        ci_fingerprint: String::new(),
        next_check: 0,
        failures: 0,
        pause_requested: false,
        created: now(),
        updated: now(),
        events: Vec::new(),
        event_sequence: 0,
    };
    context.store.save(&task)?;
    Ok(json!({"task_id":id,"existing":false,"phase":task.phase}))
}

fn apply_report(context: &Context, task: &mut Task) -> Result<()> {
    let Some(run) = task.run.clone() else {
        return Ok(());
    };
    if Lock::try_acquire(&context.store.lease_path(&task.id)?)?.is_none() {
        return Ok(());
    }
    let path = worker::report_path(&context.store, &task.id, run.generation)?;
    let report: worker::Report = match path.try_exists() {
        Ok(true) => store::read_json(&path)?,
        Ok(false) => {
            task.run = None;
            task.failures += 1;
            if task.pause_requested {
                task.stop(Phase::Paused, "workerの停止を確認しました");
            } else if task.failures >= 3 {
                task.stop(
                    Phase::Blocked,
                    "worker中断が繰り返されました。作業を保持し、原因の確認を待ちます",
                );
            } else {
                task.checkpoint = "前回workerの停止を生存lockで確認しました。既存worktree・Git・PR・delivery記録を照合して同じ段階を再開してください。完了済み操作を重複しないでください。".into();
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
        return Err("古いworker報告を拒否しました".into());
    }
    report.validate()?;
    if run.phase != Phase::Implementation && report.head != task.head {
        return Err("worker報告のレビュー対象HEADが一致しません".into());
    }
    task.run = None;
    task.checkpoint = report.summary.clone();
    if task.pause_requested {
        task.stop(Phase::Paused, "workerの停止と回収を確認しました");
        return Ok(());
    }
    match report.outcome.as_str() {
        "resource_wait" => (),
        "ready" => match run.phase {
            Phase::Implementation => {
                let pr = report
                    .pr
                    .filter(|number| *number > 0)
                    .ok_or("worker報告にPRがありません")?;
                let head = report
                    .head
                    .as_deref()
                    .filter(|head| valid_sha(head))
                    .ok_or("worker報告に固定HEADがありません")?;
                task.pr = Some(pr);
                task.head = Some(head.into());
                task.review = None;
                if matches!(report.risk.as_str(), "low" | "medium") {
                    task.review = Some(report.clone());
                    task.phase = Phase::Integration;
                } else {
                    task.phase = Phase::Review;
                }
                task.next_check = 0;
            }
            Phase::Review => {
                task.review = Some(report.clone());
                task.phase = Phase::Integration;
            }
            Phase::Integration => {
                task.phase = Phase::MainCi;
                task.next_check = 0;
            }
            Phase::Finish => {
                if !worktree::task_completed(&context.repository, &task.id)
                    .map_err(|error| error.to_string())?
                {
                    task.stop(
                        Phase::Blocked,
                        "delivery完了記録とworktree回収を確認できません",
                    );
                } else {
                    task.phase = Phase::Completed;
                }
            }
            _ => return Err("worker報告のphaseが不正です".into()),
        },
        "needs_changes" => {
            task.phase = Phase::Implementation;
            task.review = None;
            task.failures += 1;
        }
        "awaiting_decision" => task.stop(Phase::AwaitingDecision, &report.summary),
        _ => task.stop(Phase::Blocked, &report.summary),
    }
    task.event("worker_result", &report.outcome);
    Ok(())
}

fn observe_ci(context: &Context, task: &mut Task) -> Result<()> {
    if task.next_check > now()
        || !matches!(
            task.phase,
            Phase::Review | Phase::Integration | Phase::MainCi
        )
    {
        return Ok(());
    }
    let Some(pr) = task.pr else {
        return Err("CI照合対象のPRがありません".into());
    };
    let result = ci::observe(
        &context.repository,
        &context.github,
        pr,
        if task.phase == Phase::MainCi {
            task.merge_commit.as_deref()
        } else {
            None
        },
    );
    task.next_check = now() + 30;
    let observation = match result {
        Ok(observation) => observation,
        Err(reason) => {
            task.event("api_wait", &reason);
            task.next_check = now() + 120;
            return Ok(());
        }
    };
    if task.ci_fingerprint != observation.fingerprint {
        task.ci_fingerprint = observation.fingerprint;
        task.event("ci", &format!("{:?}", observation.state));
    }
    if task.phase == Phase::MainCi {
        if !observation.merged {
            task.stop(Phase::Blocked, "merge済みであることを確認できません");
            return Ok(());
        }
        if task.merge_commit.is_none() {
            task.merge_commit = observation.merge_commit;
            task.next_check = 0;
            return Ok(());
        }
    } else {
        if task.head.as_ref() != Some(&observation.head) {
            task.review = None;
            task.stop(
                Phase::Blocked,
                "PRのHEADがworker報告と異なります。作業を照合してください",
            );
            return Ok(());
        }
        if task
            .base
            .as_ref()
            .is_some_and(|base| base != &observation.base)
        {
            task.review = None;
            task.phase = Phase::Implementation;
            task.checkpoint =
                "baseが更新されました。既存worktreeを安全に更新して検証をやり直してください".into();
        }
        task.base = Some(observation.base);
    }
    match observation.state {
        ci::CiState::Success => {
            if task.phase == Phase::MainCi {
                task.phase = Phase::Finish;
            }
        }
        ci::CiState::Failure => {
            if task.phase == Phase::MainCi {
                task.stop(
                    Phase::Blocked,
                    "merge commitのmain CIが失敗しました。後続統合を保留します",
                );
            } else {
                task.phase = Phase::Implementation;
                task.review = None;
                task.checkpoint =
                    "現在のPR CIが失敗しました。対象SHAの失敗原因を確認して修正してください".into();
                task.failures += 1;
            }
        }
        ci::CiState::Pending
        | ci::CiState::Missing
        | ci::CiState::Cancelled
        | ci::CiState::TimedOut
        | ci::CiState::Skipped
        | ci::CiState::Stale => (),
    }
    Ok(())
}

fn ready(context: &Context, task: &Task, tasks: &[Task]) -> Result<bool> {
    if task.run.is_some()
        || task.pause_requested
        || !matches!(
            task.phase,
            Phase::Implementation | Phase::Review | Phase::Integration | Phase::Finish
        )
    {
        return Ok(false);
    }
    if task.failures >= 3 {
        return Ok(false);
    }
    for dependency in &task.dependencies {
        if context.load(dependency)?.phase != Phase::Completed {
            return Ok(false);
        }
    }
    if tasks.iter().any(|other| {
        other.id != task.id
            && other.run.is_some()
            && other
                .resources
                .iter()
                .any(|resource| task.resources.contains(resource))
    }) {
        return Ok(false);
    }
    if matches!(task.phase, Phase::Integration | Phase::Finish) {
        if tasks.iter().any(|other| {
            other.id != task.id
                && (integration_barrier(other)
                    || other
                        .run
                        .as_ref()
                        .is_some_and(|run| run.phase == Phase::Integration))
        }) {
            return Ok(false);
        }
        if task.phase == Phase::Integration {
            // 最新のCI成功は、実際に起動する直前に取り直す。
            let observation = ci::observe(
                &context.repository,
                &context.github,
                task.pr.ok_or("PRがありません")?,
                None,
            )?;
            if observation.state != ci::CiState::Success
                || task.head.as_ref() != Some(&observation.head)
                || task.base.as_ref() != Some(&observation.base)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
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
        // APIとbuildの待機中にstate lockを保持しない。
        apply_report(context, task)?;
        if task.failures >= 3
            && !matches!(
                task.phase,
                Phase::Blocked | Phase::Paused | Phase::AwaitingDecision
            )
        {
            task.stop(
                Phase::Blocked,
                "同じtaskの修正・中断が繰り返されました。作業を保持して原因を確認します",
            );
        }
        if task.run.is_none() {
            observe_ci(context, task)?;
        }
        let _lock = context.store.lock()?;
        let current = context.load(&task.id)?;
        task.pause_requested |= current.pause_requested;
        if current.generation != task.generation || current.event_sequence != original_sequence {
            *task = current;
            continue;
        }
        context.store.save(task)?;
    }
    let capacity = resources::Budget::load(&context.store.root)?.capacity(&context.store.root)?;
    let mut running = tasks.iter().filter(|task| task.run.is_some()).count();
    let mut launched = Vec::new();
    for index in 0..tasks.len() {
        if running >= capacity {
            break;
        }
        if !ready(context, &tasks[index], &tasks)? {
            continue;
        }
        let lease = match Lock::try_acquire(&context.store.lease_path(&tasks[index].id)?)? {
            Some(lease) => lease,
            None => continue,
        };
        let task = &mut tasks[index];
        {
            let _lock = context.store.lock()?;
            let current = context.load(&task.id)?;
            if current.event_sequence != task.event_sequence
                || current.generation != task.generation
            {
                *task = current;
                continue;
            }
        }
        if task.worktree.is_none() {
            task.worktree = Some(
                worktree::task_workspace(&context.repository, &task.id, &task.branch)
                    .map_err(|error| error.to_string())?,
            );
        }
        {
            let _lock = context.store.lock()?;
            let current = context.load(&task.id)?;
            if current.event_sequence != task.event_sequence
                || current.generation != task.generation
                || current.pause_requested
            {
                *task = current;
                continue;
            }
            task.generation += 1;
            task.run = Some(Run {
                generation: task.generation,
                phase: task.phase.clone(),
                started: now(),
            });
            task.event("worker_start", &format!("{:?}", task.phase));
            context.store.save(task)?;
        }
        let child = worker::spawn(context, task, &lease)?;
        children.push(child);
        launched.push(task.id.clone());
        running += 1;
    }
    Ok(
        json!({"launched":launched,"running":running,"tasks":tasks.iter().map(|task| json!({"task_id":task.id,"phase":task.phase,"running":task.run.is_some()})).collect::<Vec<_>>()}),
    )
}

fn integration_barrier(task: &Task) -> bool {
    matches!(task.phase, Phase::MainCi | Phase::Finish)
        || matches!(
            task.phase,
            Phase::Blocked | Phase::AwaitingDecision | Phase::Paused
        ) && matches!(
            task.resume_phase,
            Phase::MainCi | Phase::Finish | Phase::Integration
        )
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
        Commands::Submit { task_id, objective, branch, depends_on, resource } => submit(&context, task_id, objective, branch, depends_on, resource),
        Commands::List => Ok(json!(context.store.list()?.iter().map(|task| json!({"task_id":task.id,"phase":task.phase,"running":task.run.is_some(),"updated":task.updated})).collect::<Vec<_>>())),
        Commands::Show { task_id } => Ok(json!(context.load(&task_id)?)),
        Commands::Events { task_id } => Ok(json!(context.load(&task_id)?.events)),
        Commands::Pause { task_id } => {
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            task.pause_requested = true;
            if task.run.is_none() { task.stop(Phase::Paused, "利用者の指示で停止しました"); }
            task.event("pause_requested", "次のworker起動を停止します");
            context.store.save(&task)?;
            Ok(json!({"task_id":task_id,"pause_requested":true}))
        }
        Commands::Resume { task_id, instruction } => {
            if !safe_text(&instruction, 32 * 1024) { return Err("追加指示が不正です".into()); }
            let _lock = context.store.lock()?;
            let mut task = context.load(&task_id)?;
            if task.run.is_some() || Lock::try_acquire(&context.store.lease_path(&task_id)?)?.is_none() { return Err("workerが動作中です。停止と回収を確認するまで再割当てしません".into()); }
            if task.phase == Phase::Completed { return Err("完了taskへ追加作業は登録できません。別taskとして依頼してください".into()); }
            task.pause_requested = false; task.failures = 0; task.checkpoint = instruction;
            if matches!(task.phase, Phase::Blocked | Phase::AwaitingDecision | Phase::Paused) { task.phase = task.resume_phase.clone(); }
            task.event("resumed", "追加指示を保存しました");
            context.store.save(&task)?;
            Ok(json!({"task_id":task_id,"phase":task.phase}))
        }
        Commands::Reconcile | Commands::Run { .. } => {
            worker::preflight(&context)?;
            let mut coordinator = Some(Lock::try_acquire(&context.store.root.join("coordinator.lock"))?.ok_or("このrepositoryのcoordinatorは既に稼働中です")?);
            let mut children = Vec::new();
            if matches!(cli.command, Commands::Reconcile) { return reconcile(&context, &mut children); }
            let Commands::Run { poll_seconds, timeout_seconds } = cli.command else { unreachable!() };
            run_loop(&context, poll_seconds, timeout_seconds, &mut coordinator)
        }
        Commands::Start => start(&context),
        Commands::Coordinator { lock_fd } => {
            let lock = unsafe { Lock::from_inherited(lock_fd, &context.store.root.join("coordinator.lock"))? };
            lock.close_on_exec()?;
            worker::preflight(&context)?;
            let mut coordinator = Some(lock);
            let result = run_loop(&context,15,None,&mut coordinator);
            store::atomic_json(&context.store.root.join("coordinator-status.json"), &json!({"stopped":true,"at":now(),"error":result.as_ref().err()}))?;
            result
        }
        Commands::Doctor => worker::doctor(&context),
        Commands::LaunchCommand => Ok(json!({"executable":context.home.join(".local/bin/codex-task"),"arguments":["run"],"working_directory":context.repository,"codex_home":context.codex_home,"timer_required":false,"note":"OS起動adapterは任意です。WSL停止中は待機し、次回稼働時に照合します。"})),
        Commands::Worker { task_id, generation, lease_fd } => worker::execute(&context, &task_id, generation, lease_fd),
    }
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
        } => (!valid_id(&task_id)
            || !safe_text(&objective, 32 * 1024)
            || branch.is_some_and(|branch| {
                !branch.starts_with("feat/") || branch.contains(['\n', '\r', '\0'])
            })
            || depends_on.iter().any(|id| !valid_id(id) || id == &task_id)
            || resource.iter().any(|name| !resource_name(name)))
        .then(|| "codex-taskの登録内容が不正です".into()),
        Commands::Show { task_id } | Commands::Events { task_id } | Commands::Pause { task_id } => {
            (!valid_id(&task_id)).then(|| "task IDが不正です".into())
        }
        Commands::Resume {
            task_id,
            instruction,
        } => (!valid_id(&task_id) || !safe_text(&instruction, 32 * 1024))
            .then(|| "再開指示が不正です".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(Context);
    impl Fixture {
        fn new() -> Self {
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
        fn submit(&self, id: &str) {
            submit(
                &self.0,
                id.into(),
                "許可された実装依頼".into(),
                None,
                vec![],
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
        )
        .unwrap();
        assert!(
            submit(
                &fixture.0,
                "task-one".into(),
                "実装".into(),
                None,
                vec!["task-two".into()],
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
        assert_eq!(task.phase, Phase::Integration);
        assert!(task.run.is_none());
        assert_eq!(task.pr, Some(12));
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
        assert_eq!(task.phase, Phase::Implementation);
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
        };
        store::atomic_json(
            &worker::report_path(&fixture.0.store, &task.id, 2).unwrap(),
            &report,
        )
        .unwrap();
        assert!(apply_report(&fixture.0, &mut task).is_err());
        assert!(task.run.is_some());
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
