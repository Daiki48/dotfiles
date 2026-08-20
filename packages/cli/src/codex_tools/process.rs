use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

pub(crate) const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const READER_GRACE: Duration = Duration::from_millis(250);
#[cfg(unix)]
const TERMINATE_GRACE: Duration = Duration::from_millis(100);

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

pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<Output, ProcessError> {
    run_with_limit(command, timeout, MAX_CAPTURE_BYTES)
}

pub(crate) fn run_with_limit(
    command: &mut Command,
    timeout: Duration,
    capture_limit: usize,
) -> Result<Output, ProcessError> {
    let program = command.get_program().to_string_lossy().into_owned();
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
    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        (stdout, stderr) => {
            let missing = if stdout.is_none() { "stdout" } else { "stderr" };
            terminate_and_reap(&mut child, false, &program)?;
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
            terminate_and_reap(&mut child, false, &program)?;
            return Err(ProcessError::Capture {
                program,
                stream: "stdout",
                source,
            });
        }
        if let Err(source) = make_nonblocking(&stderr) {
            terminate_and_reap(&mut child, false, &program)?;
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

    while result_error.is_none() && (status.is_none() || stdout.is_none() || stderr.is_none()) {
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
        if let Err(error) = terminate_and_reap(&mut child, status.is_some(), &program) {
            result_error.get_or_insert(error);
        }
    } else if status.is_none() {
        match child.wait() {
            Ok(child_status) => status = Some(child_status),
            Err(source) => {
                cancel_readers.store(true, Ordering::Release);
                result_error = Some(ProcessError::Wait {
                    program: program.clone(),
                    source,
                });
            }
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
    program: &str,
) -> Result<(), ProcessError> {
    #[cfg(unix)]
    {
        let process_group = child.id() as i32;
        let mut cleanup_error = None;
        // SAFETY: kill is called with a validated child process-group id and no pointers.
        unsafe {
            if libc::kill(-process_group, libc::SIGTERM) == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    cleanup_error = Some(error);
                }
            }
        }
        let grace_deadline = Instant::now() + TERMINATE_GRACE;
        while !already_reaped && Instant::now() < grace_deadline {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(error) => {
                    cleanup_error.get_or_insert(error);
                    break;
                }
            }
        }
        // Killing the group also closes pipes inherited by descendants after the leader exits.
        unsafe {
            if libc::kill(-process_group, libc::SIGKILL) == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    cleanup_error.get_or_insert(error);
                }
            }
        }
        if !already_reaped && let Err(error) = child.wait() {
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
    use super::{ProcessError, run_with_limit};
    use std::process::Command;
    use std::time::{Duration, Instant};

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
        let started = Instant::now();
        let error = run_with_limit(&mut command, Duration::from_secs(5), 1024)
            .expect_err("reject unbounded output");
        assert!(matches!(error, ProcessError::CaptureLimit { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
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
        let error = run_with_limit(&mut command, Duration::from_millis(50), 1024)
            .expect_err("escaped pipe holder must hit the bounded deadline");
        assert!(matches!(error, ProcessError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
