#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::fs;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
#[cfg(target_os = "linux")]
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

pub(crate) const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const READER_GRACE: Duration = Duration::from_millis(250);
#[cfg(unix)]
const TERMINATE_GRACE: Duration = Duration::from_millis(100);
#[cfg(target_os = "linux")]
const DESCENDANT_REAP_GRACE: Duration = Duration::from_millis(500);
#[cfg(target_os = "linux")]
const UNSHARE_BINARY: &str = "/usr/bin/unshare";
#[cfg(target_os = "linux")]
const TRUE_BINARY: &str = "/usr/bin/true";
#[cfg(target_os = "linux")]
const NAMESPACE_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(target_os = "linux")]
const ENVIRONMENT_CLEARED_MARKER: &str = "CODEX_PROCESS_ENVIRONMENT_CLEARED";

#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum ProcessError {
    #[error("failed to start {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to capture {stream} from {program}: {source}")]
    Capture {
        program: String,
        stream: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{program} exceeded the {stream} capture limit of {limit} bytes")]
    CaptureLimit {
        program: String,
        stream: &'static str,
        limit: usize,
    },
    #[error("{program} exceeded its deadline of {timeout:?}")]
    Timeout { program: String, timeout: Duration },
    #[error("failed to wait for {program}: {source}")]
    Wait {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("capture worker for {stream} terminated unexpectedly")]
    ReaderStopped { stream: &'static str },
    #[error("capture worker for {stream} did not stop before its hard deadline")]
    ReaderDeadline { stream: &'static str },
    #[error("{program} did not expose its configured {stream} pipe")]
    MissingPipe {
        program: String,
        stream: &'static str,
    },
    #[error("{program} completed without an exit status")]
    MissingStatus { program: String },
    #[error("failed to configure process descendant isolation for {program}: {source}")]
    Isolation {
        program: String,
        #[source]
        source: io::Error,
    },
}

enum ReadOutcome {
    Data(Vec<u8>),
    Limit,
    Error(io::Error),
}

struct ReadMessage {
    stream: &'static str,
    outcome: ReadOutcome,
}

pub(crate) fn clear_environment(command: &mut Command) {
    command.env_clear();
    #[cfg(target_os = "linux")]
    command.env(ENVIRONMENT_CLEARED_MARKER, "1");
}

#[cfg(target_os = "linux")]
fn linux_pid_namespace_command(command: &Command) -> io::Result<Command> {
    let unshare =
        super::trust::trusted_system_binary(UNSHARE_BINARY, "Unshare").map_err(io::Error::other)?;
    let program = command.get_program().to_os_string();
    let arguments = command.get_args().map(OsString::from).collect::<Vec<_>>();
    let environment = command
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.map(OsString::from)))
        .collect::<Vec<_>>();
    let environment_was_cleared = environment.iter().any(|(key, value)| {
        key == ENVIRONMENT_CLEARED_MARKER && value.as_deref() == Some(std::ffi::OsStr::new("1"))
    });

    let mut isolated = Command::new(unshare);
    if environment_was_cleared {
        isolated.env_clear();
    }
    for (key, value) in environment {
        if key == ENVIRONMENT_CLEARED_MARKER {
            continue;
        }
        if let Some(value) = value {
            isolated.env(key, value);
        } else {
            isolated.env_remove(key);
        }
    }
    if let Some(directory) = command.get_current_dir() {
        isolated.current_dir(directory);
    }
    isolated.args([
        "--user",
        "--map-current-user",
        "--pid",
        "--fork",
        "--kill-child=KILL",
        "--",
    ]);
    isolated.arg(program).args(arguments).stdin(Stdio::null());
    Ok(isolated)
}

#[cfg(target_os = "linux")]
fn linux_pid_namespace_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let Ok(unshare) = super::trust::trusted_system_binary(UNSHARE_BINARY, "Unshare") else {
            return false;
        };
        let Ok(true_binary) = super::trust::trusted_system_binary(TRUE_BINARY, "True") else {
            return false;
        };
        let mut probe = Command::new(unshare);
        probe
            .env_clear()
            .args([
                "--user",
                "--map-current-user",
                "--pid",
                "--fork",
                "--kill-child=KILL",
                "--",
            ])
            .arg(true_binary)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = probe.spawn() else {
            return false;
        };
        let deadline = Instant::now() + NAMESPACE_PROBE_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    let _ = child.kill();
                    reap_probe_until(&mut child, Instant::now() + NAMESPACE_PROBE_TIMEOUT);
                    return false;
                }
                Err(_) => {
                    let _ = child.kill();
                    reap_probe_until(&mut child, Instant::now() + NAMESPACE_PROBE_TIMEOUT);
                    return false;
                }
            }
        }
    })
}

#[cfg(target_os = "linux")]
fn reap_probe_until(child: &mut std::process::Child, deadline: Instant) {
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => return,
        }
    }
}

#[cfg(target_os = "linux")]
fn prepare_linux_command(
    command: &mut Command,
    namespace_available: bool,
) -> io::Result<Option<Command>> {
    if namespace_available {
        linux_pid_namespace_command(command).map(Some)
    } else {
        // This marker exists only to preserve env_clear while constructing the
        // unshare wrapper. Do not expose it to a directly executed target.
        command.env_remove(ENVIRONMENT_CLEARED_MARKER);
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct ProcStat {
    parent: i32,
    start_time: u64,
}

#[cfg(target_os = "linux")]
fn process_disappeared(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || matches!(error.raw_os_error(), Some(code) if code == libc::ENOENT || code == libc::ESRCH)
}

#[cfg(target_os = "linux")]
fn proc_snapshot() -> io::Result<HashMap<i32, ProcStat>> {
    let mut result = HashMap::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let contents = match fs::read_to_string(entry.path().join("stat")) {
            Ok(contents) => contents,
            Err(error) if process_disappeared(&error) => continue,
            Err(error) => return Err(error),
        };
        let Some(comm_end) = contents.rfind(") ") else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("/proc/{pid}/stat has no command terminator"),
            ));
        };
        let fields = contents[comm_end + 2..]
            .split_whitespace()
            .collect::<Vec<_>>();
        // After the comm field, field 3 is state and field 22 is starttime.
        let (Some(parent), Some(start_time)) = (fields.get(1), fields.get(19)) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("/proc/{pid}/stat is truncated"),
            ));
        };
        let (Ok(parent), Ok(start_time)) = (parent.parse(), start_time.parse()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("/proc/{pid}/stat contains invalid numeric fields"),
            ));
        };
        result.insert(pid, ProcStat { parent, start_time });
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
fn process_run_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(target_os = "linux")]
fn signal_observed_process(pid: i32, start_time: u64, signal: libc::c_int) -> Option<io::Error> {
    let snapshot = match proc_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return Some(error),
    };
    let observed = snapshot.get(&pid)?;
    if start_time != 0 && observed.start_time != start_time {
        return None;
    }
    // SAFETY: pidfd_open returns a stable kernel reference to the observed PID.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if descriptor == -1 {
        let error = io::Error::last_os_error();
        return (!matches!(error.raw_os_error(), Some(libc::ESRCH))).then_some(error);
    }
    let same_process = match proc_snapshot() {
        Ok(snapshot) => snapshot
            .get(&pid)
            .is_some_and(|stat| start_time == 0 || stat.start_time == start_time),
        Err(error) => {
            // SAFETY: descriptor was returned by pidfd_open above.
            unsafe { libc::close(descriptor) };
            return Some(error);
        }
    };
    let result = if same_process {
        // SAFETY: descriptor is a live pidfd; siginfo is intentionally null.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                descriptor,
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        }
    } else {
        0
    };
    // SAFETY: descriptor was returned by pidfd_open above.
    unsafe { libc::close(descriptor) };
    if result == -1 {
        let error = io::Error::last_os_error();
        (!matches!(error.raw_os_error(), Some(libc::ESRCH))).then_some(error)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn enable_subreaper() -> io::Result<()> {
    // SAFETY: prctl changes only this process's child-reaping policy and takes
    // no pointers for PR_SET_CHILD_SUBREAPER.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
struct DescendantTracker {
    runner_pid: i32,
    root_pid: Option<i32>,
    marker: String,
    baseline: HashMap<i32, u64>,
    tracked: HashMap<i32, u64>,
}

#[cfg(not(target_os = "linux"))]
struct DescendantTracker;

impl DescendantTracker {
    #[cfg(target_os = "linux")]
    fn new() -> io::Result<Self> {
        static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);
        let runner_pid = std::process::id() as i32;
        let snapshot = proc_snapshot()?;
        let baseline = snapshot
            .iter()
            .filter(|(_, stat)| stat.parent == runner_pid)
            .map(|(pid, stat)| (*pid, stat.start_time))
            .collect();
        Ok(Self {
            runner_pid,
            root_pid: None,
            marker: format!(
                "codex-{}-{}",
                runner_pid,
                NEXT_MARKER.fetch_add(1, Ordering::Relaxed)
            ),
            baseline,
            tracked: HashMap::new(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    #[cfg(target_os = "linux")]
    fn set_root(&mut self, root_pid: u32) -> io::Result<()> {
        let root_pid = root_pid as i32;
        self.root_pid = Some(root_pid);
        let snapshot = proc_snapshot()?;
        let start_time = snapshot
            .get(&root_pid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "root PID disappeared"))?
            .start_time;
        self.tracked.insert(root_pid, start_time);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn set_root(&mut self, _root_pid: u32) -> io::Result<()> {
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn mark_command(&self, command: &mut Command) {
        command.env("CODEX_PROCESS_RUN_ID", &self.marker);
    }

    #[cfg(not(target_os = "linux"))]
    fn mark_command(&self, _command: &mut Command) {}

    #[cfg(target_os = "linux")]
    fn carries_marker(&self, pid: i32) -> io::Result<bool> {
        let expected = format!("CODEX_PROCESS_RUN_ID={}", self.marker);
        let environment = match fs::read(format!("/proc/{pid}/environ")) {
            Ok(environment) => environment,
            Err(error) if process_disappeared(&error) => return Ok(false),
            #[cfg(test)]
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                // Parallel fixtures may create an unrelated child in a user
                // namespace whose environ is unreadable. Test fallback treats
                // an unprovable marker as absent; release never uses fallback.
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        Ok(environment
            .split(|byte| *byte == 0)
            .any(|entry| entry == expected.as_bytes()))
    }

    #[cfg(target_os = "linux")]
    fn refresh(&mut self) -> io::Result<()> {
        self.refresh_from(proc_snapshot())
    }

    #[cfg(target_os = "linux")]
    fn refresh_from(&mut self, snapshot: io::Result<HashMap<i32, ProcStat>>) -> io::Result<()> {
        let snapshot = snapshot?;
        let root_pid = self.root_pid;
        self.tracked.retain(|pid, start_time| {
            if Some(*pid) == root_pid && !snapshot.contains_key(pid) {
                return true;
            }
            snapshot
                .get(pid)
                .is_some_and(|stat| *start_time == 0 || stat.start_time == *start_time)
        });
        let mut changed = true;
        while changed {
            changed = false;
            for (pid, stat) in &snapshot {
                if *pid == self.runner_pid
                    || Some(*pid) == root_pid
                    || self.baseline.contains_key(pid)
                    || self.tracked.contains_key(pid)
                {
                    continue;
                }
                // A per-run marker is required for newly adopted children so
                // another thread's unrelated child can never be claimed.
                let adopted = stat.parent == self.runner_pid && self.carries_marker(*pid)?;
                let descendant = self.tracked.contains_key(&stat.parent);
                if adopted || descendant {
                    self.tracked.insert(*pid, stat.start_time);
                    changed = true;
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn refresh(&mut self) -> io::Result<()> {
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn signal_known(&mut self, signal: libc::c_int) -> Option<io::Error> {
        if let Err(error) = self.refresh() {
            return Some(error);
        }
        let mut first_error = None;
        for (pid, start_time) in &self.tracked {
            if Some(*pid) == self.root_pid {
                continue;
            }
            if let Some(error) = signal_observed_process(*pid, *start_time, signal) {
                first_error.get_or_insert(error);
            }
        }
        first_error
    }

    #[cfg(target_os = "linux")]
    fn signal_root(&self, signal: libc::c_int) -> Option<io::Error> {
        let pid = self.root_pid?;
        let start_time = self.tracked.get(&pid).copied().unwrap_or(0);
        signal_observed_process(pid, start_time, signal)
    }

    #[cfg(not(target_os = "linux"))]
    fn signal_known(&mut self, _signal: i32) -> Option<io::Error> {
        None
    }

    #[cfg(not(target_os = "linux"))]
    fn signal_root(&self, _signal: i32) -> Option<io::Error> {
        None
    }

    #[cfg(target_os = "linux")]
    fn reap_known(&mut self) -> Option<io::Error> {
        let mut first_error = None;
        let root_pid = self.root_pid;
        let pids = self
            .tracked
            .keys()
            .copied()
            .filter(|pid| Some(*pid) != root_pid)
            .collect::<Vec<_>>();
        for pid in pids {
            let mut status = 0;
            // SAFETY: waitpid is called only for PIDs observed as descendants;
            // WNOHANG avoids blocking the runner's deadline cleanup.
            let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if result == pid {
                self.tracked.remove(&pid);
            } else if result == -1 {
                let error = io::Error::last_os_error();
                // ECHILD means the process has not been reparented to the
                // subreaper yet; keep tracking it until its parent exits.
                if !matches!(error.raw_os_error(), Some(code) if code == libc::ECHILD || code == libc::ESRCH)
                {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error
    }

    #[cfg(not(target_os = "linux"))]
    fn reap_known(&mut self) -> Option<io::Error> {
        None
    }

    #[cfg(target_os = "linux")]
    fn reap_until_gone(&mut self) -> Option<io::Error> {
        let deadline = Instant::now() + DESCENDANT_REAP_GRACE;
        let mut first_error = None;
        loop {
            if let Err(error) = self.refresh() {
                first_error.get_or_insert(error);
                break;
            }
            if let Some(error) = self.reap_known() {
                first_error.get_or_insert(error);
            }
            if let Err(error) = self.refresh() {
                first_error.get_or_insert(error);
                break;
            }
            let has_live = self.tracked.keys().any(|pid| Some(*pid) != self.root_pid);
            if !has_live {
                break;
            }
            if let Some(error) = self.signal_known(libc::SIGKILL) {
                first_error.get_or_insert(error);
            }
            if Instant::now() >= deadline {
                first_error.get_or_insert_with(|| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "descendant process did not terminate before cleanup deadline",
                    )
                });
                break;
            }
            thread::sleep(POLL_INTERVAL);
        }
        first_error
    }

    #[cfg(not(target_os = "linux"))]
    fn reap_until_gone(&mut self) -> Option<io::Error> {
        None
    }
}

pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<Output, ProcessError> {
    run_with_limit(command, timeout, MAX_CAPTURE_BYTES)
}

pub(crate) fn run_with_limit(
    command: &mut Command,
    timeout: Duration,
    capture_limit: usize,
) -> Result<Output, ProcessError> {
    #[cfg(target_os = "linux")]
    let namespace_available = linux_pid_namespace_available();
    #[cfg(not(target_os = "linux"))]
    let namespace_available = false;
    #[cfg(all(target_os = "linux", not(test)))]
    if !namespace_available {
        return Err(ProcessError::Isolation {
            program: command.get_program().to_string_lossy().into_owned(),
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "trusted user/PID namespace isolation is unavailable",
            ),
        });
    }
    #[cfg(all(not(target_os = "linux"), not(test)))]
    return Err(ProcessError::Isolation {
        program: command.get_program().to_string_lossy().into_owned(),
        source: io::Error::new(
            io::ErrorKind::Unsupported,
            "process-tree isolation is supported only on Linux",
        ),
    });
    run_with_limit_config(command, timeout, capture_limit, namespace_available)
}

fn run_with_limit_config(
    command: &mut Command,
    timeout: Duration,
    capture_limit: usize,
    namespace_available: bool,
) -> Result<Output, ProcessError> {
    #[cfg(not(target_os = "linux"))]
    let _ = namespace_available;
    let program = command.get_program().to_string_lossy().into_owned();
    #[cfg(target_os = "linux")]
    let _run_guard = process_run_lock()
        .lock()
        .map_err(|_| ProcessError::Isolation {
            program: program.clone(),
            source: io::Error::other("process runner lock poisoned"),
        })?;
    #[cfg(target_os = "linux")]
    enable_subreaper().map_err(|source| ProcessError::Isolation {
        program: program.clone(),
        source,
    })?;
    let mut descendants = DescendantTracker::new().map_err(|source| ProcessError::Isolation {
        program: program.clone(),
        source,
    })?;
    descendants.mark_command(command);
    #[cfg(target_os = "linux")]
    let mut isolated_command =
        prepare_linux_command(command, namespace_available).map_err(|source| {
            ProcessError::Isolation {
                program: program.clone(),
                source,
            }
        })?;
    #[cfg(target_os = "linux")]
    let command = match isolated_command.as_mut() {
        Some(isolated) => isolated,
        None => command,
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid(0, 0) mutates only the child process state between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = command.spawn().map_err(|source| ProcessError::Spawn {
        program: program.clone(),
        source,
    })?;
    if let Err(source) = descendants.set_root(child.id()) {
        terminate_and_reap(&mut child, false, &mut descendants, &program)?;
        return Err(ProcessError::Isolation { program, source });
    }
    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        (stdout, stderr) => {
            let missing = if stdout.is_none() { "stdout" } else { "stderr" };
            terminate_and_reap(&mut child, false, &mut descendants, &program)?;
            drop(stdout);
            drop(stderr);
            return Err(ProcessError::MissingPipe {
                program,
                stream: missing,
            });
        }
    };
    #[cfg(unix)]
    {
        if let Err(source) = make_nonblocking(&stdout) {
            terminate_and_reap(&mut child, false, &mut descendants, &program)?;
            return Err(ProcessError::Capture {
                program,
                stream: "stdout",
                source,
            });
        }
        if let Err(source) = make_nonblocking(&stderr) {
            terminate_and_reap(&mut child, false, &mut descendants, &program)?;
            return Err(ProcessError::Capture {
                program,
                stream: "stderr",
                source,
            });
        }
    }
    let (sender, receiver) = mpsc::channel();
    let cancel_readers = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_reader(
        stdout,
        "stdout",
        capture_limit,
        Arc::clone(&cancel_readers),
        sender.clone(),
    );
    let stderr_reader = spawn_reader(
        stderr,
        "stderr",
        capture_limit,
        Arc::clone(&cancel_readers),
        sender,
    );

    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    let mut result_error = None;
    let mut completed_cleanup = false;

    while result_error.is_none() && (status.is_none() || stdout.is_none() || stderr.is_none()) {
        if let Err(source) = descendants.refresh() {
            result_error = Some(ProcessError::Isolation {
                program: program.clone(),
                source,
            });
            break;
        }
        drain_messages(
            &receiver,
            &program,
            capture_limit,
            &mut stdout,
            &mut stderr,
            &mut result_error,
        );
        if result_error.is_some() {
            break;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(child_status) => status = child_status,
                Err(source) => {
                    result_error = Some(ProcessError::Wait {
                        program: program.clone(),
                        source,
                    });
                    break;
                }
            }
        }
        if status.is_some() && !completed_cleanup {
            completed_cleanup = true;
            if let Err(error) = terminate_and_reap(&mut child, true, &mut descendants, &program) {
                result_error = Some(error);
                break;
            }
        }
        if status.is_some() && stdout.is_some() && stderr.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            result_error = Some(ProcessError::Timeout {
                program: program.clone(),
                timeout,
            });
            break;
        }
        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }

    if result_error.is_some() {
        cancel_readers.store(true, Ordering::Release);
        if let Err(error) =
            terminate_and_reap(&mut child, status.is_some(), &mut descendants, &program)
        {
            result_error.get_or_insert(error);
        }
    }

    let reader_deadline = Instant::now() + READER_GRACE;
    if let Err(error) = join_reader(stdout_reader, "stdout", reader_deadline) {
        result_error.get_or_insert(error);
    }
    if let Err(error) = join_reader(stderr_reader, "stderr", reader_deadline) {
        result_error.get_or_insert(error);
    }
    drain_messages(
        &receiver,
        &program,
        capture_limit,
        &mut stdout,
        &mut stderr,
        &mut result_error,
    );

    if let Some(error) = result_error {
        return Err(error);
    }
    Ok(Output {
        status: status.ok_or_else(|| ProcessError::MissingStatus {
            program: program.clone(),
        })?,
        stdout: stdout.ok_or(ProcessError::ReaderStopped { stream: "stdout" })?,
        stderr: stderr.ok_or(ProcessError::ReaderStopped { stream: "stderr" })?,
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    stream: &'static str,
    limit: usize,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::Sender<ReadMessage>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let outcome = read_bounded(reader, limit, &cancelled);
        let _ = sender.send(ReadMessage { stream, outcome });
    })
}

fn read_bounded(mut reader: impl Read, limit: usize, cancelled: &AtomicBool) -> ReadOutcome {
    let mut captured = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 8192];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return ReadOutcome::Data(captured);
        }
        match reader.read(&mut buffer) {
            Ok(0) => return ReadOutcome::Data(captured),
            Ok(read) if captured.len().saturating_add(read) <= limit => {
                captured.extend_from_slice(&buffer[..read]);
            }
            Ok(_) => return ReadOutcome::Limit,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return ReadOutcome::Error(error),
        }
    }
}

#[cfg(unix)]
fn make_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let descriptor = pipe.as_raw_fd();
    // SAFETY: fcntl is called for an owned, live pipe descriptor and no pointers.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the same live descriptor is updated only with O_NONBLOCK.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn join_reader(
    handle: thread::JoinHandle<()>,
    stream: &'static str,
    deadline: Instant,
) -> Result<(), ProcessError> {
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
    if !handle.is_finished() {
        return Err(ProcessError::ReaderDeadline { stream });
    }
    handle
        .join()
        .map_err(|_| ProcessError::ReaderStopped { stream })
}

fn drain_messages(
    receiver: &Receiver<ReadMessage>,
    program: &str,
    limit: usize,
    stdout: &mut Option<Vec<u8>>,
    stderr: &mut Option<Vec<u8>>,
    result_error: &mut Option<ProcessError>,
) {
    loop {
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        };
        match message.outcome {
            ReadOutcome::Data(data) if message.stream == "stdout" => *stdout = Some(data),
            ReadOutcome::Data(data) => *stderr = Some(data),
            ReadOutcome::Limit => {
                result_error.get_or_insert_with(|| ProcessError::CaptureLimit {
                    program: program.to_owned(),
                    stream: message.stream,
                    limit,
                });
            }
            ReadOutcome::Error(source) => {
                result_error.get_or_insert_with(|| ProcessError::Capture {
                    program: program.to_owned(),
                    stream: message.stream,
                    source,
                });
            }
        }
    }
}

fn terminate_and_reap(
    child: &mut std::process::Child,
    already_reaped: bool,
    descendants: &mut DescendantTracker,
    program: &str,
) -> Result<(), ProcessError> {
    #[cfg(unix)]
    {
        let process_group = child.id() as i32;
        let mut cleanup_error = None;
        let mut root_reaped = already_reaped;
        if let Err(error) = descendants.refresh() {
            cleanup_error.get_or_insert(error);
        }
        if !root_reaped {
            // The group id cannot be reused while its observed leader remains
            // unreaped. Root signaling via pidfd also covers a root that called
            // setsid and left its configured process group.
            unsafe {
                if libc::kill(-process_group, libc::SIGTERM) == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        cleanup_error.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = descendants.signal_root(libc::SIGTERM) {
                cleanup_error.get_or_insert(error);
            }
        }
        if let Some(error) = descendants.signal_known(libc::SIGTERM) {
            cleanup_error.get_or_insert(error);
        }
        let grace_deadline = Instant::now() + TERMINATE_GRACE;
        while !root_reaped && Instant::now() < grace_deadline {
            match child.try_wait() {
                Ok(Some(_)) => root_reaped = true,
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(error) => {
                    cleanup_error.get_or_insert(error);
                    break;
                }
            }
            if let Err(error) = descendants.refresh() {
                cleanup_error.get_or_insert(error);
                break;
            }
            if let Some(error) = descendants.reap_known() {
                cleanup_error.get_or_insert(error);
            }
        }
        if !root_reaped {
            unsafe {
                if libc::kill(-process_group, libc::SIGKILL) == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        cleanup_error.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = descendants.signal_root(libc::SIGKILL) {
                cleanup_error.get_or_insert(error);
            }
        }
        if let Some(error) = descendants.signal_known(libc::SIGKILL) {
            cleanup_error.get_or_insert(error);
        }
        let hard_deadline = Instant::now() + DESCENDANT_REAP_GRACE;
        while !root_reaped {
            match child.try_wait() {
                Ok(Some(_)) => root_reaped = true,
                Ok(None) if Instant::now() < hard_deadline => thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    cleanup_error.get_or_insert_with(|| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "root process did not terminate before cleanup deadline",
                        )
                    });
                    break;
                }
                Err(error) => {
                    cleanup_error.get_or_insert(error);
                    break;
                }
            }
        }
        if let Some(error) = descendants.reap_until_gone() {
            cleanup_error.get_or_insert(error);
        }
        if let Some(source) = cleanup_error {
            return Err(ProcessError::Wait {
                program: program.to_owned(),
                source,
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let mut cleanup_error = None;
        if !already_reaped {
            if let Err(error) = child.kill() {
                cleanup_error = Some(error);
            }
            if let Err(error) = child.wait() {
                cleanup_error.get_or_insert(error);
            }
        }
        if let Some(source) = cleanup_error {
            return Err(ProcessError::Wait {
                program: program.to_owned(),
                source,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::{
        DescendantTracker, ENVIRONMENT_CLEARED_MARKER, prepare_linux_command, reap_probe_until,
        run_with_limit_config,
    };
    use super::{ProcessError, run_with_limit};
    use std::fs;
    use std::io;
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_unavailable_uses_the_subreaper_fallback() {
        let mut command = Command::new("sh");
        super::clear_environment(&mut command);
        command.arg("-c").arg("exit 0");
        assert!(
            prepare_linux_command(&mut command, false)
                .expect("prepare fallback command")
                .is_none()
        );
        assert!(
            command
                .get_envs()
                .all(|(key, _)| key != ENVIRONMENT_CLEARED_MARKER)
        );

        let mut escaped = Command::new("sh");
        escaped.args(["-c", "setsid sh -c 'sleep 2' & exit 0"]);
        let started = Instant::now();
        let output = run_with_limit_config(&mut escaped, Duration::from_millis(500), 1024, false)
            .expect("fallback must stop and reap the escaped pipe holder");
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_stops_a_target_that_attempts_to_leave_its_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec setsid sleep 30"]);
        let started = Instant::now();
        let output = run_with_limit_config(&mut command, Duration::from_millis(50), 1024, false)
            .expect("setsid helper and its detached child must be reaped");
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_cleans_an_escaped_descendant_that_closed_its_pipes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let pid_file = std::env::temp_dir().join(format!(
            "codex-process-closed-pipe-{}-{nonce}.pid",
            std::process::id()
        ));
        let pid_file_text = pid_file.to_str().expect("pid file path");
        let script = format!(
            "setsid sh -c 'echo $$ > {pid_file_text}; exec >/dev/null 2>&1; sleep 30' & while [ ! -s {pid_file_text} ]; do sleep 0.01; done; exit 0"
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        let output = run_with_limit_config(&mut command, Duration::from_millis(500), 1024, false)
            .expect("normal completion must clean detached descendants");
        assert!(output.status.success());

        let pid = fs::read_to_string(&pid_file)
            .expect("escaped descendant pid file")
            .trim()
            .parse::<libc::pid_t>()
            .expect("escaped descendant pid");
        // SAFETY: kill(pid, 0) only probes process existence.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        fs::remove_file(pid_file).expect("remove pid file");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_does_not_claim_an_unrelated_child_spawned_during_the_run() {
        let unrelated = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn concurrent unrelated child")
        });
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let error = run_with_limit_config(&mut command, Duration::from_millis(75), 1024, false)
            .expect_err("enforce deadline");
        assert!(matches!(error, ProcessError::Timeout { .. }));

        let mut unrelated = unrelated.join().expect("join unrelated spawner");
        assert!(
            unrelated
                .try_wait()
                .expect("inspect unrelated child")
                .is_none(),
            "cleanup signaled a concurrently-created unrelated child"
        );
        unrelated.kill().expect("stop unrelated child");
        unrelated.wait().expect("reap unrelated child");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_snapshot_failure_is_not_treated_as_success() {
        let mut tracker = DescendantTracker::new().expect("create tracker");
        let failure = tracker.refresh_from(Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "fixture",
        )));
        assert_eq!(
            failure.expect_err("snapshot failure must propagate").kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn short_lived_roots_are_registered_before_cleanup() {
        for _ in 0..100 {
            let mut command = Command::new("/usr/bin/true");
            let output = run_with_limit_config(&mut command, Duration::from_secs(1), 1024, false)
                .expect("short-lived root must retain a trackable PID");
            assert!(output.status.success());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_probe_reap_has_a_hard_deadline() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn probe fixture");
        child.kill().expect("kill probe fixture");
        let started = Instant::now();
        reap_probe_until(&mut child, Instant::now() + Duration::from_millis(500));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(child.try_wait().expect("inspect probe fixture").is_some());
    }

    #[test]
    fn captures_stdout_and_stderr_without_deadlock() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf out; printf err >&2; exit 7"]);
        let output = run_with_limit(&mut command, Duration::from_secs(1), 1024)
            .expect("run bounded command");
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
    }

    #[test]
    fn stops_a_process_group_at_the_capture_limit() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes x"]);
        // process_run_lockの待機時間は責務外なので、壁時計の上限は検証しない。
        let error = run_with_limit(&mut command, Duration::from_secs(5), 1024)
            .expect_err("reject unbounded output");
        assert!(matches!(error, ProcessError::CaptureLimit { .. }));
    }

    #[test]
    fn stops_and_reaps_a_process_group_at_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();
        let error = run_with_limit(&mut command, Duration::from_millis(50), 1024)
            .expect_err("enforce deadline");
        assert!(matches!(error, ProcessError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn escaped_descendant_cannot_hold_capture_workers_past_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "setsid sh -c 'sleep 2' & exit 0"]);
        let started = Instant::now();
        let output = run_with_limit(&mut command, Duration::from_millis(500), 1024)
            .expect("PID namespace teardown must stop the escaped pipe holder");
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn escaped_descendant_is_killed_and_reaped_after_deadline() {
        if !super::linux_pid_namespace_available() {
            // A target can erase the test fallback's ownership marker. The
            // release binary rejects this host instead of claiming isolation.
            return;
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let pid_file = std::env::temp_dir().join(format!(
            "codex-process-descendant-{}-{nonce}.pid",
            std::process::id()
        ));
        let pid_file = pid_file.to_str().expect("pid file path");
        let script = format!(
            "setsid sh -c 'grep \"^NSpid:\" /proc/self/status | cut -f2 > {pid_file}; exec env -u CODEX_PROCESS_RUN_ID sleep 30' & while [ ! -s {pid_file} ]; do sleep 0.01; done; exit 0"
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        match run_with_limit(&mut command, Duration::from_millis(500), 1024) {
            Ok(output) => assert!(output.status.success()),
            Err(ProcessError::Timeout { .. }) => {}
            Err(error) => panic!("unexpected namespace cleanup error: {error}"),
        }

        let pid = fs::read_to_string(pid_file)
            .expect("escaped descendant pid file")
            .trim()
            .parse::<libc::pid_t>()
            .expect("escaped descendant pid");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // SAFETY: kill(pid, 0) only probes whether the recorded test
            // process still exists and sends no signal.
            let result = unsafe { libc::kill(pid, 0) };
            if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "escaped descendant {pid} survived timeout cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_file(pid_file);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descendant_cleanup_does_not_signal_unrelated_children() {
        let mut unrelated = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn unrelated child");
        let mut command = Command::new("sh");
        command.args(["-c", "setsid sh -c 'sleep 30' & wait"]);
        let error = run_with_limit(&mut command, Duration::from_millis(50), 1024)
            .expect_err("enforce deadline");
        assert!(matches!(error, ProcessError::Timeout { .. }));
        assert!(
            unrelated
                .try_wait()
                .expect("inspect unrelated child")
                .is_none(),
            "cleanup signaled a child outside this process run"
        );
        unrelated.kill().expect("stop unrelated test child");
        unrelated.wait().expect("reap unrelated test child");
    }
}
