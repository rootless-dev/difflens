//! Running the toolchain: each child in its own process group, its stdout
//! read line by line as it comes, its stderr kept (the end of it), and the
//! whole group stopped at a deadline, so that nothing a command started (a
//! rustc, a test's own child) outlives it.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// How much of a command's stderr is kept: its end.
pub const MAX_STDERR: usize = 256 << 10;

/// How a command ended.
#[derive(Debug)]
pub struct Finished {
    /// None when it could not be waited for.
    pub status: Option<ExitStatus>,
    /// The end of its stderr, at most MAX_STDERR bytes.
    pub stderr: String,
    /// It was stopped because the deadline passed.
    pub timed_out: bool,
}

impl Finished {
    pub fn success(&self) -> bool {
        !self.timed_out && self.status.is_some_and(|status| status.success())
    }

    /// The exit status as a person reads it: "exit status: 101", "signal: 9".
    pub fn describe(&self) -> String {
        if self.timed_out {
            return "stopped: the time ran out".to_string();
        }
        match self.status {
            Some(status) => status.to_string(),
            None => "it could not be waited for".to_string(),
        }
    }
}

/// Runs `command` until it ends or `deadline` passes, giving each line of
/// its stdout (without the newline) to `on_line`. Fails only when the
/// command cannot be started.
pub fn stream(
    command: Command,
    deadline: Instant,
    on_line: impl FnMut(&str),
) -> std::io::Result<Finished> {
    stream_until(command, None, deadline, on_line)
}

/// `stream`, and at `interrupt` (when given, before `deadline`) the command
/// alone gets SIGTERM, so that a program that runs its children in process
/// groups of their own (cargo-mutants) can stop them and clean up; its
/// group is still killed at `deadline`. Reaching `interrupt` counts as
/// timing out.
pub fn stream_until(
    mut command: Command,
    interrupt: Option<Instant>,
    deadline: Instant,
    mut on_line: impl FnMut(&str),
) -> std::io::Result<Finished> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (done, timed_out) = (Arc::clone(&done), Arc::clone(&timed_out));
        thread::spawn(move || {
            let mut interrupt = interrupt.filter(|interrupt| *interrupt < deadline);
            while !done.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now >= deadline {
                    timed_out.store(true, Ordering::Relaxed);
                    kill_group(pid);
                    return;
                }
                if interrupt.is_some_and(|interrupt| now >= interrupt) {
                    interrupt = None;
                    timed_out.store(true, Ordering::Relaxed);
                    signal(pid, "-TERM");
                }
                thread::sleep(Duration::from_millis(25));
            }
        })
    };
    let stderr = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut tail = Tail::new(MAX_STDERR);
            let mut buffer = [0_u8; 16 * 1024];
            while let Ok(read) = pipe.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                tail.push(&buffer[..read]);
            }
            tail.into_string()
        })
    });
    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while line
                        .last()
                        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
                    {
                        line.pop();
                    }
                    on_line(&String::from_utf8_lossy(&line));
                }
            }
        }
    }
    let status = child.wait().ok();
    done.store(true, Ordering::Relaxed);
    let _ = watchdog.join();
    // Whatever the command left running in its group goes with it.
    kill_group(pid);
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ok(Finished {
        status,
        stderr,
        timed_out: timed_out.load(Ordering::Relaxed),
    })
}

/// Runs `command` and returns its whole stdout with how it ended.
pub fn capture(command: Command, deadline: Instant) -> std::io::Result<(String, Finished)> {
    let mut stdout = String::new();
    let finished = stream(command, deadline, |line| {
        stdout.push_str(line);
        stdout.push('\n');
    })?;
    Ok((stdout, finished))
}

/// Stops every process of the group `pid` leads. There is no libc here:
/// kill(1) does it.
fn kill_group(pid: u32) {
    signal_target(&format!("-{pid}"), "-KILL");
}

/// Sends `signal` ("-TERM") to the process `pid` alone.
fn signal(pid: u32, signal: &str) {
    signal_target(&pid.to_string(), signal);
}

fn signal_target(target: &str, signal: &str) {
    let _ = Command::new("kill")
        .arg(signal)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Kills every process whose command line contains `text` (pkill -f), for
/// what escaped its group: the children cargo-mutants runs in groups of
/// their own, when cargo-mutants itself had to be killed.
pub fn kill_matching(text: &str) {
    // An extended regular expression matching `text` as it is.
    let pattern: String = text
        .chars()
        .flat_map(|c| {
            let special = ".[]()*+?{}|^$\\".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect();
    let _ = Command::new("pkill")
        .arg("-KILL")
        .arg("-f")
        .arg("--")
        .arg(pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether `program` can be found on the PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file()
                && std::fs::metadata(&candidate)
                    .map(|meta| {
                        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0
                    })
                    .unwrap_or(false)
        })
    })
}

/// Keeps the last `max` bytes written to it.
pub struct Tail {
    buffer: Vec<u8>,
    max: usize,
}

impl Tail {
    pub fn new(max: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        // Trimmed only once twice the limit, so that trimming is amortised.
        if self.buffer.len() > 2 * self.max {
            let cut = self.buffer.len() - self.max;
            self.buffer.drain(..cut);
        }
    }

    pub fn push_str(&mut self, text: &str) {
        self.push(text.as_bytes());
    }

    /// At most `max` bytes, the end kept, starting on a character.
    pub fn into_string(self) -> String {
        keep_end(&String::from_utf8_lossy(&self.buffer), self.max)
    }
}

/// The last `max` bytes of `text`, cut on a character boundary.
pub fn keep_end(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

/// The first line of `text` that says something, for one-line messages.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_streamed_and_its_status_kept() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo one; echo two; echo oops >&2; exit 3"]);
        let mut lines = Vec::new();
        let finished = stream(command, Instant::now() + Duration::from_secs(10), |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(finished.stderr, "oops\n");
        assert_eq!(finished.status.unwrap().code(), Some(3));
        assert!(!finished.success() && !finished.timed_out);
    }

    #[test]
    fn a_command_past_its_deadline_is_stopped_with_its_children() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & sleep 30"]);
        let started = Instant::now();
        let finished =
            stream(command, Instant::now() + Duration::from_millis(200), |_| {}).unwrap();
        assert!(finished.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(finished.describe(), "stopped: the time ran out");
    }

    #[test]
    fn a_command_is_interrupted_before_its_deadline() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "trap 'echo stopping; exit 7' TERM; sleep 30 >/dev/null 2>&1 & wait",
        ]);
        let started = Instant::now();
        let mut lines = Vec::new();
        let finished = stream_until(
            command,
            Some(Instant::now() + Duration::from_millis(300)),
            Instant::now() + Duration::from_secs(20),
            |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(lines, ["stopping"]);
        assert_eq!(finished.status.and_then(|status| status.code()), Some(7));
        assert!(finished.timed_out);
    }

    #[test]
    fn the_end_of_long_output_is_kept_on_a_character() {
        let mut tail = Tail::new(4);
        tail.push_str("abcdefgh");
        assert_eq!(tail.into_string(), "efgh");
        // "é" is two bytes: the cut moves past its second.
        assert_eq!(keep_end("abcéfgh", 4), "fgh");
        assert_eq!(keep_end("aé", 1), "");
        assert_eq!(keep_end("short", 10), "short");
        assert_eq!(first_line("\n  \n first \nsecond"), "first");
    }
}
