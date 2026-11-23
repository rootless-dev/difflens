//! Running a child process with a hard time limit and bounded output, and
//! stopping it together with everything it started.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Time allowed to collect the output once the process has exited or was
/// stopped.
const OUTPUT_GRACE: Duration = Duration::from_secs(5);
const READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    /// More standard output than this stops the process.
    pub max_stdout_bytes: usize,
    /// Only the end of the error output is kept.
    pub stderr_tail_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Exited(ExitStatus),
    /// Stopped after `Limits::timeout`.
    TimedOut,
    /// Stopped after printing more than `Limits::max_stdout_bytes`.
    OutputTooLarge,
    /// Stopped because the caller raised its cancel flag.
    Cancelled,
}

#[derive(Debug)]
pub struct Captured {
    pub ending: Ending,
    pub stdout: Vec<u8>,
    pub stderr_tail: Vec<u8>,
}

/// Runs `command` with stdin closed, capturing both outputs, in a process
/// group of its own so that a stop also reaches its children. Fails only when
/// it cannot be started or waited for, or when it exited but something it left
/// behind keeps its output open.
pub fn run_captured(command: Command, limits: &Limits) -> io::Result<Captured> {
    run_captured_until(command, limits, &AtomicBool::new(false))
}

/// `run_captured`, also stopping the process group as soon as `cancel` is
/// raised, which ends it as `Ending::Cancelled`.
pub fn run_captured_until(
    command: Command,
    limits: &Limits,
    cancel: &AtomicBool,
) -> io::Result<Captured> {
    run_captured_with_input(command, None, limits, cancel)
}

/// `run_captured_until`, writing `input` to the process's standard input
/// (then closing it) when there is one; stdin is closed from the start
/// otherwise. A process that stops reading early does not fail the run.
pub fn run_captured_with_input(
    mut command: Command,
    input: Option<Vec<u8>>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> io::Result<Captured> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    start_process_group(&mut command);
    let mut child = command.spawn()?;
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        // Written on a thread of its own: a large input would otherwise
        // block here while the process waits for its output to be read.
        thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&input);
        });
    }

    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().map(|stdout| {
        let overflow = Arc::clone(&overflow);
        let limit = limits.max_stdout_bytes;
        read_in_background(move || read_bounded(stdout, limit, &overflow))
    });
    let stderr = child.stderr.take().map(|stderr| {
        let tail = limits.stderr_tail_bytes;
        read_in_background(move || read_tail(stderr, tail))
    });

    let deadline = Instant::now() + limits.timeout;
    let ending = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ending::Exited(status),
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                stop_process_group(&mut child);
                break Ending::Cancelled;
            }
            Ok(None) if overflow.load(Ordering::Relaxed) => {
                stop_process_group(&mut child);
                break Ending::OutputTooLarge;
            }
            Ok(None) if Instant::now() >= deadline => {
                stop_process_group(&mut child);
                break Ending::TimedOut;
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                stop_process_group(&mut child);
                return Err(error);
            }
        }
    };

    let collect = |receiver: Option<mpsc::Receiver<Vec<u8>>>| {
        receiver.map(|receiver| receiver.recv_timeout(OUTPUT_GRACE).ok())
    };
    let stdout = collect(stdout);
    let stderr_tail = collect(stderr).flatten().unwrap_or_default();
    let stdout = match (ending, stdout) {
        (Ending::Exited(_), Some(None)) => {
            return Err(io::Error::other(
                "the process exited but something it started kept its output open",
            ))
        }
        (_, stdout) => stdout.flatten().unwrap_or_default(),
    };
    let ending = if overflow.load(Ordering::Relaxed) {
        Ending::OutputTooLarge
    } else {
        ending
    };
    Ok(Captured {
        ending,
        stdout,
        stderr_tail,
    })
}

fn read_in_background(read: impl FnOnce() -> Vec<u8> + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(read());
    });
    receiver
}

/// Reads until end of file, or until more than `limit` bytes arrived, which
/// raises `overflow`. A read error ends the output early.
fn read_bounded(mut source: impl Read, limit: usize, overflow: &AtomicBool) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    loop {
        match source.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if buffer.len() + read > limit {
                    overflow.store(true, Ordering::Relaxed);
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
        }
    }
    buffer
}

/// Reads until end of file, keeping the last `keep` bytes.
fn read_tail(mut source: impl Read, keep: usize) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    loop {
        match source.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > keep.saturating_mul(2) {
                    let excess = buffer.len() - keep;
                    buffer.drain(..excess);
                }
            }
        }
    }
    if buffer.len() > keep {
        let excess = buffer.len() - keep;
        buffer.drain(..excess);
    }
    buffer
}

/// Makes the child the leader of a new process group, so everything it
/// starts can be stopped with it.
#[cfg(unix)]
pub fn start_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
pub fn start_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub fn stop_process_group(child: &mut Child) {
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) only sends a signal. The child has not been reaped
        // yet, so its id still names it and the process group it leads
        // (see start_process_group); no unrelated process can be reached.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
pub fn stop_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn limits(timeout_ms: u64) -> Limits {
        Limits {
            timeout: Duration::from_millis(timeout_ms),
            max_stdout_bytes: 1024,
            stderr_tail_bytes: 8,
        }
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn captures_output_and_keeps_the_end_of_stderr() {
        let captured = run_captured(
            shell("printf hello; printf 'first line, last line' >&2; exit 3"),
            &limits(5000),
        )
        .unwrap();
        assert!(matches!(captured.ending, Ending::Exited(status) if status.code() == Some(3)));
        assert_eq!(captured.stdout, b"hello");
        assert_eq!(captured.stderr_tail, b"ast line");
    }

    #[test]
    fn stops_a_process_that_prints_too_much() {
        let captured = run_captured(shell("yes"), &limits(10_000)).unwrap();
        assert_eq!(captured.ending, Ending::OutputTooLarge);
    }

    #[test]
    fn stops_a_process_that_runs_too_long() {
        let started = Instant::now();
        let captured = run_captured(shell("sleep 30"), &limits(300)).unwrap();
        assert_eq!(captured.ending, Ending::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn stops_a_process_when_cancelled() {
        let cancel = Arc::new(AtomicBool::new(false));
        let raise = {
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                cancel.store(true, Ordering::Relaxed);
            })
        };
        let started = Instant::now();
        let captured =
            run_captured_until(shell("sleep 30 & wait"), &limits(30_000), &cancel).unwrap();
        raise.join().unwrap();
        assert_eq!(captured.ending, Ending::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn feeds_the_input_to_stdin_and_closes_it() {
        let input = "x".repeat(200_000);
        let captured = run_captured_with_input(
            shell("wc -c"),
            Some(input.into_bytes()),
            &Limits {
                timeout: Duration::from_secs(10),
                max_stdout_bytes: 1024,
                stderr_tail_bytes: 64,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(matches!(captured.ending, Ending::Exited(status) if status.success()));
        assert_eq!(String::from_utf8_lossy(&captured.stdout).trim(), "200000");
    }

    #[test]
    fn a_missing_program_cannot_be_started() {
        let error =
            run_captured(Command::new("difflens-test-no-such-program"), &limits(1000)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
