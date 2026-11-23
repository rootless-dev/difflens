//! Terminal sessions of the embedded terminal: one pseudo-terminal per
//! session, running the user's login shell, or Claude Code through it, in a
//! folder `worktrees::location` chose.
//!
//! Sessions live here, in the core, not in a view: a terminal view that
//! unmounts (another review tab, another review, the project screen) and
//! mounts again attaches to its session and is sent the session's scrollback
//! first, a bounded ring of what it printed, then what it prints next. They
//! end when closed or when the app quits (`close_all`); a process that exits
//! on its own leaves its session listed, ended, until it is closed.
//!
//! Each session has three threads: one reads the output into the scrollback
//! and the sink, one writes input in the order it was sent, and one watches
//! the process for its exit and for the command in the foreground, which the
//! tab shows and which makes closing ask first. Closing hangs up the shell's
//! process group and the foreground one, then kills them after a short grace.
//!
//! Output reaches the frontend through a `TerminalSink`: raw bytes, and
//! `TerminalEvent`s for the rest. Attaching swaps the sink under the same
//! lock that appends output, so a new view gets every byte exactly once.
//!
//! What a Claude Code session runs is read from Preferences as it starts
//! (`claude_command`).

mod claude_command;

pub use claude_command::{
    claude_command_setting, save_custom_claude_command, set_claude_command_mode,
    ClaudeCommandMode, ClaudeCommandSetting,
};

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use uuid::Uuid;

use crate::error::{app_error, escaped, AppError, AppResult};
use crate::model::{ReviewTarget, TerminalEvent, TerminalKind, TerminalSession};
use crate::projects::now;
use crate::store::Store;

/// What a session keeps of its output for a view that attaches later.
pub const SCROLLBACK_BYTES: usize = 1024 * 1024;
/// How often the process is checked for its exit and its foreground command.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Time the last output of an exited process has to arrive before its exit
/// is reported after it.
const EXIT_DRAIN: Duration = Duration::from_secs(1);
/// Time a hung-up process has to exit before it is killed.
const HANGUP_GRACE: Duration = Duration::from_millis(500);
/// Time a killed process has to be reaped.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);
const READ_CHUNK_BYTES: usize = 16 * 1024;
/// A replay starts at a line, when the scrollback lost its beginning and a
/// line ends this close to the start.
const REPLAY_LINE_SEARCH_BYTES: usize = 4096;
const MAX_COMMAND_CHARS: usize = 200;
const CLAUDE_NAME: &str = "Claude Code";
const DEFAULT_SHELL: &str = "/bin/zsh";
const INITIAL_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};

/// Variables of the app's own environment that would mislead what runs in a
/// session: git ones pointing at another repository, the terminal the app
/// may have been started from, and Claude Code's marker, with which `claude`
/// refuses to start when DiffLens itself was launched from a Claude Code
/// session.
const SCRUBBED_ENVIRONMENT: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
];

/// Where a session's output goes: the channel of the view attached to it.
pub trait TerminalSink: Send + Sync {
    fn output(&self, bytes: &[u8]);
    fn event(&self, event: &TerminalEvent);
}

/// How sessions are started.
pub struct Launcher {
    shell: PathBuf,
    shell_arguments: Vec<OsString>,
    /// The command a Claude Code session `exec`s through the shell while
    /// Preferences holds no custom one.
    claude: String,
    environment: Vec<(OsString, OsString)>,
}

impl Launcher {
    /// The user's login shell (`$SHELL`, `/bin/zsh` without one) as a login
    /// shell, so the PATH is the one of their terminal: an app started from
    /// the Dock does not inherit it. Interactive, because its terminal is a
    /// pseudo-terminal.
    pub fn from_environment() -> Self {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SHELL));
        Self {
            shell,
            shell_arguments: vec![OsString::from("-l")],
            claude: "claude".to_string(),
            environment: Vec::new(),
        }
    }

    /// `shell` without login arguments, `claude` instead of the real default
    /// command and `environment` on top of this process's.
    #[cfg(test)]
    pub fn for_tests(shell: &str, claude: &str, environment: &[(&str, &str)]) -> Self {
        Self {
            shell: PathBuf::from(shell),
            shell_arguments: vec![OsString::from("-i")],
            claude: claude.to_string(),
            environment: environment
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect(),
        }
    }

    /// "zsh" for `/bin/zsh`: what a shell session's tab is called.
    fn shell_name(&self) -> String {
        self.shell
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "shell".to_string())
    }

    /// The shell, or for Claude Code `<shell> -l -i -c 'exec claude'`, with
    /// `custom` instead of `claude` when the user saved a command of their
    /// own: no prompt and no argument of DiffLens's is ever added to it. When
    /// it is not found, the shell prints why and the session ends.
    fn command(&self, kind: TerminalKind, cwd: &Path, custom: Option<&str>) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.shell);
        command.args(&self.shell_arguments);
        if kind == TerminalKind::Claude {
            command.args(["-i", "-c"]);
            command.arg(format!("exec {}", custom.unwrap_or(&self.claude)));
        }
        command.cwd(cwd);
        for variable in SCRUBBED_ENVIRONMENT {
            command.env_remove(variable);
        }
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("PWD", cwd);
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        command
    }
}

/// What a new session is, besides its process.
pub struct OpenRequest {
    pub kind: TerminalKind,
    /// An existing folder, canonical.
    pub cwd: PathBuf,
    pub project_id: String,
    pub project_name: String,
    pub target: Option<ReviewTarget>,
    pub branch: Option<String>,
}

pub struct Terminals {
    launcher: Launcher,
    scrollback_bytes: usize,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    sequence: AtomicU64,
}

impl Terminals {
    pub fn new(launcher: Launcher, scrollback_bytes: usize) -> Self {
        Self {
            launcher,
            scrollback_bytes,
            sessions: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }

    /// Starts a session in `request.cwd`, its output going to `sink` until a
    /// view attaches. A Claude Code session runs the command `settings`
    /// holds now.
    pub fn open(
        &self,
        request: OpenRequest,
        settings: &Store,
        sink: Arc<dyn TerminalSink>,
    ) -> AppResult<TerminalSession> {
        if !request.cwd.is_dir() {
            return Err(app_error!(
                PathNotFound,
                PathGone,
                "{path} no longer exists.",
                path = request.cwd.display()
            ));
        }
        let custom = match request.kind {
            TerminalKind::Claude => claude_command::session_claude_command(settings)?,
            TerminalKind::Shell => None,
        };
        let command = self
            .launcher
            .command(request.kind, &request.cwd, custom.as_deref());
        let pair = native_pty_system()
            .openpty(INITIAL_SIZE)
            .map_err(start_failed)?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(start_failed)?;
        // The reader only sees the end of the output once every copy of the
        // other side is closed, this one included.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(start_failed)?;
        let writer = pair.master.take_writer().map_err(start_failed)?;
        let opened_at = now()?;
        let (input, inputs) = mpsc::channel::<Vec<u8>>();

        let mut sessions = lock(&self.sessions);
        let name = self.unique_name(&sessions, request.kind, &request.cwd);
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            info: TerminalSession {
                id: id.clone(),
                kind: request.kind,
                name,
                cwd: request.cwd.to_string_lossy().into_owned(),
                project_id: request.project_id,
                project_name: request.project_name,
                target: request.target,
                branch: request.branch,
                opened_at,
                foreground: None,
                ended: false,
                exit_code: None,
            },
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            pid: child.process_id(),
            state: Arc::new(Mutex::new(State {
                scrollback: Scrollback::new(self.scrollback_bytes),
                sink,
                foreground: None,
                exit: None,
            })),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            input,
            stopped: AtomicBool::new(false),
        });
        sessions.insert(id, Arc::clone(&session));
        drop(sessions);

        let reader_done = Arc::new(AtomicBool::new(false));
        {
            let state = Arc::clone(&session.state);
            let done = Arc::clone(&reader_done);
            thread::spawn(move || read_output(reader, &state, &done));
        }
        thread::spawn(move || write_input(writer, &inputs));
        let described = session.describe();
        thread::spawn(move || watch(&session, &reader_done));
        Ok(described)
    }

    /// Sends `sink` everything the session prints from now on, and with
    /// `replay` first its scrollback and its exit or foreground command when
    /// it has one. Without, it is a window that reloaded and only follows
    /// the session until a view attaches.
    pub fn attach(
        &self,
        id: &str,
        sink: Arc<dyn TerminalSink>,
        replay: bool,
    ) -> AppResult<TerminalSession> {
        let session = self.get(id)?;
        {
            let mut state = lock(&session.state);
            state.sink = sink;
            if !replay {
                return Ok(Session::describe_with(&session.info, &state));
            }
            let scrollback = state.scrollback.replay();
            if !scrollback.is_empty() {
                state.sink.output(&scrollback);
            }
            if let Some(code) = state.exit {
                state.sink.event(&TerminalEvent::Exited { code });
            } else if state.foreground.is_some() {
                let command = state.foreground.clone();
                state.sink.event(&TerminalEvent::Foreground { command });
            }
        }
        Ok(session.describe())
    }

    /// Queues `bytes` as typed input; written in the order sent. Input to an
    /// ended session is dropped.
    pub fn write(&self, id: &str, bytes: Vec<u8>) -> AppResult<()> {
        let session = self.get(id)?;
        let _ = session.input.send(bytes);
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let session = self.get(id)?;
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        };
        let resized = lock(&session.master).resize(size);
        resized.map_err(|error| {
            app_error!(
                Internal,
                TerminalResizeFailed,
                "Could not resize the terminal: {detail}",
                detail = error.to_string()
            )
        })
    }

    /// Ends the session and its processes. Closing one already closed does
    /// nothing.
    pub fn close(&self, id: &str) -> AppResult<()> {
        let removed = lock(&self.sessions).remove(id);
        if let Some(session) = removed {
            stop(&[session]);
        }
        Ok(())
    }

    /// Ends every session, as the app quits.
    pub fn close_all(&self) {
        let sessions: Vec<Arc<Session>> = lock(&self.sessions)
            .drain()
            .map(|(_, session)| session)
            .collect();
        stop(&sessions);
    }

    /// Every session, oldest first.
    pub fn list(&self) -> Vec<TerminalSession> {
        let mut sessions: Vec<Arc<Session>> = lock(&self.sessions).values().cloned().collect();
        sessions.sort_by_key(|session| session.sequence);
        sessions.iter().map(|session| session.describe()).collect()
    }

    /// Sessions open in `folder`, ended ones included: their tabs are.
    pub fn count_in(&self, folder: &Path) -> usize {
        lock(&self.sessions)
            .values()
            .filter(|session| Path::new(&session.info.cwd) == folder)
            .count()
    }

    fn get(&self, id: &str) -> AppResult<Arc<Session>> {
        lock(&self.sessions).get(id).cloned().ok_or_else(|| {
            app_error!(
                UnknownTerminal,
                UnknownTerminal,
                "There is no terminal with id \"{id}\".",
                id = escaped(id)
            )
        })
    }

    /// "zsh", then "zsh 2", "zsh 3"...: the first number no session of that
    /// kind in that folder has.
    fn unique_name(
        &self,
        sessions: &HashMap<String, Arc<Session>>,
        kind: TerminalKind,
        cwd: &Path,
    ) -> String {
        let base = match kind {
            TerminalKind::Shell => self.launcher.shell_name(),
            TerminalKind::Claude => CLAUDE_NAME.to_string(),
        };
        let taken: Vec<&str> = sessions
            .values()
            .filter(|session| session.info.kind == kind && Path::new(&session.info.cwd) == cwd)
            .map(|session| session.info.name.as_str())
            .collect();
        (1..)
            .map(|number| match number {
                1 => base.clone(),
                _ => format!("{base} {number}"),
            })
            .find(|name| !taken.contains(&name.as_str()))
            .unwrap_or(base)
    }
}

struct Session {
    /// What does not change; `describe` adds what does.
    info: TerminalSession,
    sequence: u64,
    pid: Option<u32>,
    state: Arc<Mutex<State>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    input: mpsc::Sender<Vec<u8>>,
    /// Raised by `close`: the watcher stops and nothing more is reported.
    stopped: AtomicBool,
}

struct State {
    scrollback: Scrollback,
    sink: Arc<dyn TerminalSink>,
    foreground: Option<String>,
    /// `Some` once the process exited, with its code when it had one.
    exit: Option<Option<i32>>,
}

impl Session {
    fn describe(&self) -> TerminalSession {
        Self::describe_with(&self.info, &lock(&self.state))
    }

    fn describe_with(info: &TerminalSession, state: &State) -> TerminalSession {
        TerminalSession {
            foreground: state.foreground.clone(),
            ended: state.exit.is_some(),
            exit_code: state.exit.flatten(),
            ..info.clone()
        }
    }

    /// The process group in the foreground of the terminal, when it is not
    /// the shell's own: a command the shell runs.
    fn foreground_group(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            let group = lock(&self.master).process_group_leader()?;
            let shell = self.pid.and_then(|pid| i32::try_from(pid).ok());
            (Some(group) != shell && group > 0).then_some(group)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn running(&self) -> bool {
        matches!(lock(&self.child).try_wait(), Ok(None))
    }

    fn report_foreground(&self, command: Option<String>) {
        let mut state = lock(&self.state);
        let stopped = self.stopped.load(Ordering::Relaxed);
        if state.exit.is_some() || stopped || state.foreground == command {
            return;
        }
        state.foreground.clone_from(&command);
        state.sink.event(&TerminalEvent::Foreground { command });
    }

    fn report_exit(&self, code: Option<i32>) {
        let mut state = lock(&self.state);
        if state.exit.is_some() || self.stopped.load(Ordering::Relaxed) {
            return;
        }
        state.exit = Some(code);
        state.foreground = None;
        state.sink.event(&TerminalEvent::Exited { code });
    }
}

/// The last `capacity` bytes a session printed.
struct Scrollback {
    bytes: VecDeque<u8>,
    capacity: usize,
    /// Its beginning was dropped to stay within the capacity.
    trimmed: bool,
}

impl Scrollback {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity,
            trimmed: false,
        }
    }

    fn push(&mut self, data: &[u8]) {
        self.bytes.extend(data);
        if self.bytes.len() > self.capacity {
            let excess = self.bytes.len() - self.capacity;
            self.bytes.drain(..excess);
            self.trimmed = true;
        }
    }

    /// What a view that attaches is sent. Once the beginning was dropped,
    /// it starts after the first line end near the start, so it does not
    /// begin in the middle of a line or of an escape sequence.
    fn replay(&self) -> Vec<u8> {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        if self.trimmed {
            if let Some(end) = bytes
                .iter()
                .take(REPLAY_LINE_SEARCH_BYTES)
                .position(|byte| *byte == b'\n')
            {
                return bytes[end + 1..].to_vec();
            }
        }
        bytes
    }
}

fn read_output(mut reader: Box<dyn Read + Send>, state: &Mutex<State>, done: &AtomicBool) {
    let mut buffer = vec![0; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let mut state = lock(state);
                state.scrollback.push(&buffer[..read]);
                state.sink.output(&buffer[..read]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // EIO once the process side is closed.
            Err(_) => break,
        }
    }
    done.store(true, Ordering::Release);
}

fn write_input(mut writer: Box<dyn Write + Send>, inputs: &mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = inputs.recv() {
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
            break;
        }
    }
}

/// Reports the process's exit, after its last output, and the command in the
/// foreground. A group has to be in the foreground for two checks in a row
/// to be reported, so the commands a prompt runs (a git status for the
/// prompt line) do not flash in the tab.
fn watch(session: &Session, reader_done: &AtomicBool) {
    let mut seen: Option<i32> = None;
    let mut reported: Option<i32> = None;
    while !session.stopped.load(Ordering::Relaxed) {
        let status = lock(&session.child).try_wait();
        match status {
            Ok(Some(status)) => {
                let deadline = Instant::now() + EXIT_DRAIN;
                while !reader_done.load(Ordering::Acquire) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                session.report_exit(i32::try_from(status.exit_code()).ok());
                return;
            }
            Err(_) => {
                session.report_exit(None);
                return;
            }
            Ok(None) => {}
        }
        let group = session.foreground_group();
        if group == seen && group != reported {
            reported = group;
            session.report_foreground(group.map(command_line));
        }
        seen = group;
        thread::sleep(POLL_INTERVAL);
    }
}

/// The command line of the process leading `group`, its program without its
/// folder: "go test ./...". Empty when it cannot be read (its leader already
/// exited).
fn command_line(group: i32) -> String {
    let mut command = std::process::Command::new("ps");
    command.args(["-o", "args=", "-p", &group.to_string()]);
    let captured = crate::process::run_captured(
        command,
        &crate::process::Limits {
            timeout: Duration::from_secs(2),
            max_stdout_bytes: 64 * 1024,
            stderr_tail_bytes: 0,
        },
    );
    let Ok(captured) = captured else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&captured.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    let (program, rest) = line.split_once(' ').unwrap_or((line, ""));
    let program = program.rsplit('/').next().unwrap_or(program);
    let command = if rest.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", rest.trim())
    };
    command.chars().take(MAX_COMMAND_CHARS).collect()
}

/// Hangs up every session's shell and foreground groups, waits a little for
/// them to exit, then kills what is left and reaps the shells.
fn stop(sessions: &[Arc<Session>]) {
    let groups: Vec<Vec<i32>> = sessions
        .iter()
        .map(|session| {
            session.stopped.store(true, Ordering::Relaxed);
            let mut groups: Vec<i32> = session
                .pid
                .and_then(|pid| i32::try_from(pid).ok())
                .into_iter()
                .collect();
            groups.extend(session.foreground_group());
            groups
        })
        .collect();
    for (session, groups) in sessions.iter().zip(&groups) {
        signal(session, groups, Signal::HangUp);
    }
    let deadline = Instant::now() + HANGUP_GRACE;
    while sessions.iter().any(|session| session.running()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    for (session, groups) in sessions.iter().zip(&groups) {
        signal(session, groups, Signal::Kill);
        let deadline = Instant::now() + REAP_TIMEOUT;
        while session.running() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(Clone, Copy)]
enum Signal {
    HangUp,
    Kill,
}

#[cfg(unix)]
fn signal(_session: &Session, groups: &[i32], signal: Signal) {
    let number = match signal {
        Signal::HangUp => libc::SIGHUP,
        Signal::Kill => libc::SIGKILL,
    };
    for group in groups {
        // SAFETY: kill(2) only sends a signal. Each group is the session's
        // shell's own (it leads the terminal's session) or the one in the
        // terminal's foreground, both read before the hang-up. A group id is
        // not reused while the group has members; one that emptied during
        // the grace could only name another group if the system reused its
        // id within that half second.
        unsafe {
            libc::kill(-group, number);
        }
    }
}

#[cfg(not(unix))]
fn signal(session: &Session, _groups: &[i32], _signal: Signal) {
    let _ = lock(&session.child).kill();
}

fn start_failed(error: impl std::fmt::Display) -> AppError {
    app_error!(
        Internal,
        TerminalStartFailed,
        "Could not start the terminal: {detail}",
        detail = error.to_string()
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use tempfile::TempDir;

    const NO_CLAUDE: &str = "difflens-test-no-such-claude";
    const WAIT: Duration = Duration::from_secs(10);

    /// What a sink was sent, as a view would see it.
    #[derive(Default)]
    struct Recorder {
        output: Mutex<Vec<u8>>,
        events: Mutex<Vec<TerminalEvent>>,
    }

    impl TerminalSink for Recorder {
        fn output(&self, bytes: &[u8]) {
            lock(&self.output).extend_from_slice(bytes);
        }

        fn event(&self, event: &TerminalEvent) {
            lock(&self.events).push(event.clone());
        }
    }

    impl Recorder {
        fn text(&self) -> String {
            String::from_utf8_lossy(&lock(&self.output)).into_owned()
        }

        fn events(&self) -> Vec<TerminalEvent> {
            lock(&self.events).clone()
        }
    }

    /// `/bin/sh` in a folder of its own, its home that folder too, so no
    /// startup file of the machine running the tests is read. The settings
    /// start empty, so Claude Code sessions run `NO_CLAUDE`.
    struct Harness {
        terminals: Terminals,
        settings: Store,
        folder: TempDir,
    }

    fn harness(scrollback_bytes: usize) -> Harness {
        let folder = TempDir::new().unwrap();
        let home = folder.path().canonicalize().unwrap();
        let home = home.to_str().unwrap().to_string();
        let launcher = Launcher::for_tests(
            "/bin/sh",
            NO_CLAUDE,
            &[
                ("HOME", &home),
                ("ENV", ""),
                ("PS1", "$ "),
                ("BASH_SILENCE_DEPRECATION_WARNING", "1"),
            ],
        );
        Harness {
            terminals: Terminals::new(launcher, scrollback_bytes),
            settings: Store::open_in_memory().unwrap(),
            folder,
        }
    }

    impl Harness {
        fn cwd(&self) -> PathBuf {
            self.folder.path().canonicalize().unwrap()
        }

        fn open(&self, kind: TerminalKind) -> (TerminalSession, Arc<Recorder>) {
            let recorder = Arc::new(Recorder::default());
            let session = self
                .terminals
                .open(
                    OpenRequest {
                        kind,
                        cwd: self.cwd(),
                        project_id: "p-1".to_string(),
                        project_name: "demo".to_string(),
                        target: None,
                        branch: Some("main".to_string()),
                    },
                    &self.settings,
                    Arc::clone(&recorder) as Arc<dyn TerminalSink>,
                )
                .unwrap();
            (session, recorder)
        }

        fn type_line(&self, id: &str, line: &str) {
            self.terminals.write(id, format!("{line}\n").into_bytes()).unwrap();
        }

        fn session(&self, id: &str) -> Arc<Session> {
            self.terminals.get(id).unwrap()
        }
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn the_launcher_runs_a_login_shell_and_claude_through_it_without_arguments() {
        let launcher = Launcher {
            shell: PathBuf::from("/bin/zsh"),
            shell_arguments: vec![OsString::from("-l")],
            claude: "claude".to_string(),
            environment: Vec::new(),
        };
        let cwd = Path::new("/tmp");
        assert_eq!(launcher.shell_name(), "zsh");
        let shell = launcher.command(TerminalKind::Shell, cwd, None);
        assert_eq!(shell.get_argv(), &[OsString::from("/bin/zsh"), OsString::from("-l")]);
        assert_eq!(shell.get_env("TERM"), Some(std::ffi::OsStr::new("xterm-256color")));
        assert_eq!(shell.get_env("COLORTERM"), Some(std::ffi::OsStr::new("truecolor")));
        assert_eq!(shell.get_env("GIT_DIR"), None);
        assert_eq!(shell.get_env("CLAUDECODE"), None);
        assert_eq!(shell.get_cwd().map(PathBuf::from), Some(PathBuf::from("/tmp")));
        let claude = launcher.command(TerminalKind::Claude, cwd, None);
        assert_eq!(argv(&claude), ["/bin/zsh", "-l", "-i", "-c", "exec claude"]);
    }

    #[test]
    fn a_custom_command_runs_through_the_login_shell_like_claude() {
        let launcher = Launcher {
            shell: PathBuf::from("/bin/zsh"),
            shell_arguments: vec![OsString::from("-l")],
            claude: "claude".to_string(),
            environment: Vec::new(),
        };
        let cwd = Path::new("/tmp");
        let custom = launcher.command(TerminalKind::Claude, cwd, Some("claude --model opus"));
        assert_eq!(argv(&custom), ["/bin/zsh", "-l", "-i", "-c", "exec claude --model opus"]);
        assert_eq!(custom.get_env("CLAUDECODE"), None);
        assert_eq!(custom.get_env("CLAUDE_CODE_ENTRYPOINT"), None);
        assert_eq!(custom.get_env("TERM"), Some(std::ffi::OsStr::new("xterm-256color")));
        assert_eq!(custom.get_cwd().map(PathBuf::from), Some(PathBuf::from("/tmp")));
        let shell = launcher.command(TerminalKind::Shell, cwd, Some("claude --model opus"));
        assert_eq!(argv(&shell), ["/bin/zsh", "-l"], "a shell session runs no command");
    }

    fn argv(command: &CommandBuilder) -> Vec<&str> {
        command.get_argv().iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn a_shell_session_echoes_input_runs_it_and_streams_the_output() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Shell);
        assert_eq!(session.name, "sh");
        assert_eq!(session.cwd, harness.cwd().to_str().unwrap());
        assert!(!session.ended);

        harness.type_line(&session.id, "echo hello-$((40+2)); pwd");

        wait_until("the command's output", || output.text().contains("hello-42"));
        let text = output.text();
        assert!(text.contains("echo hello-$((40+2))"), "the input is echoed: {text:?}");
        wait_until("pwd", || output.text().contains(harness.cwd().to_str().unwrap()));
        harness.terminals.close(&session.id).unwrap();
        assert!(harness.terminals.list().is_empty());
    }

    #[test]
    fn a_resize_reaches_the_terminal() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Shell);

        harness.terminals.resize(&session.id, 100, 30).unwrap();
        harness.type_line(&session.id, "stty size");

        wait_until("stty's answer", || output.text().contains("30 100"));
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn the_foreground_command_is_reported_and_closing_kills_it() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Shell);
        let shell = harness.session(&session.id).pid.unwrap();

        harness.type_line(&session.id, "sleep 300");

        let reported = || {
            output
                .events()
                .contains(&TerminalEvent::Foreground { command: Some("sleep 300".to_string()) })
        };
        wait_until("the foreground command", reported);
        assert_eq!(harness.terminals.list()[0].foreground.as_deref(), Some("sleep 300"));
        let sleep = harness.session(&session.id).foreground_group().unwrap();
        assert_ne!(i64::from(sleep), i64::from(shell));
        assert!(alive(sleep));

        harness.terminals.close(&session.id).unwrap();

        wait_until("sleep to be gone", || !alive(sleep));
        wait_until("the shell to be gone", || !alive(i32::try_from(shell).unwrap()));
        assert_eq!(harness.terminals.count_in(&harness.cwd()), 0);
    }

    #[test]
    fn an_idle_shell_reports_no_foreground_command() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Shell);
        harness.type_line(&session.id, "true; echo ready");
        wait_until("the prompt", || output.text().contains("ready"));
        thread::sleep(POLL_INTERVAL * 3);

        assert_eq!(harness.session(&session.id).foreground_group(), None);
        assert_eq!(harness.terminals.list()[0].foreground, None);
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn a_session_whose_process_exits_stays_listed_as_ended() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Shell);

        harness.type_line(&session.id, "echo bye; exit 3");

        wait_until("the exit", || output.events().contains(&TerminalEvent::Exited { code: Some(3) }));
        assert!(output.text().contains("bye"));
        let listed = &harness.terminals.list()[0];
        assert!(listed.ended);
        assert_eq!(listed.exit_code, Some(3));
        harness.terminals.write(&session.id, b"ignored\n".to_vec()).unwrap();
        harness.terminals.close(&session.id).unwrap();
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn a_claude_session_without_claude_shows_the_shell_error_and_ends() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, output) = harness.open(TerminalKind::Claude);
        assert_eq!(session.name, "Claude Code");

        wait_for_exit(&output);
        assert!(output.text().contains(NO_CLAUDE), "{:?}", output.text());
        assert!(output.events().contains(&TerminalEvent::Exited { code: Some(127) }));
        harness.terminals.close(&session.id).unwrap();
    }

    fn wait_for_exit(output: &Recorder) {
        wait_until("the session to end", || {
            output.events().iter().any(|event| matches!(event, TerminalEvent::Exited { .. }))
        });
    }

    #[test]
    fn a_claude_session_runs_the_custom_command_saved_when_it_starts() {
        let harness = harness(SCROLLBACK_BYTES);
        // The shell works out the number, so it ran the command.
        save_custom_claude_command(&harness.settings, "echo fake-claude-$((6*7)) --model opus").unwrap();

        let (session, output) = harness.open(TerminalKind::Claude);
        assert_eq!(session.name, "Claude Code");
        wait_for_exit(&output);
        assert!(output.text().contains("fake-claude-42 --model opus"), "{:?}", output.text());
        assert!(!output.text().contains(NO_CLAUDE));
        assert!(output.events().contains(&TerminalEvent::Exited { code: Some(0) }));

        // Back to the default: the next session runs it, the custom command kept.
        set_claude_command_mode(&harness.settings, ClaudeCommandMode::Default).unwrap();
        let (_, default) = harness.open(TerminalKind::Claude);
        wait_for_exit(&default);
        assert!(default.text().contains(NO_CLAUDE), "{:?}", default.text());
        assert!(!default.text().contains("fake-claude-42"));
        harness.terminals.close_all();
    }

    #[test]
    fn a_claude_session_ignores_a_stored_command_it_cannot_run() {
        let harness = harness(SCROLLBACK_BYTES);
        save_custom_claude_command(&harness.settings, "echo first").unwrap();
        harness
            .settings
            .set_setting(claude_command::CUSTOM_SETTING, "echo first\necho injected")
            .unwrap();

        let (_, output) = harness.open(TerminalKind::Claude);
        wait_for_exit(&output);
        let text = output.text();
        assert!(text.contains(NO_CLAUDE), "the default runs: {text:?}");
        assert!(!text.contains("injected"), "{text:?}");
        assert!(!text.contains("first"), "{text:?}");
        harness.terminals.close_all();
    }

    #[test]
    fn a_shell_session_runs_the_shell_whatever_command_is_saved() {
        let harness = harness(SCROLLBACK_BYTES);
        save_custom_claude_command(&harness.settings, "echo fake-claude-$((6*7))").unwrap();
        let (session, output) = harness.open(TerminalKind::Shell);

        harness.type_line(&session.id, "echo shell-$((40+2))");

        wait_until("the shell's output", || output.text().contains("shell-42"));
        assert!(!output.text().contains("fake-claude-42"));
        assert!(!harness.terminals.list()[0].ended);
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn a_view_that_attaches_gets_the_scrollback_once_then_the_rest() {
        let harness = harness(SCROLLBACK_BYTES);
        let (session, first) = harness.open(TerminalKind::Shell);
        harness.type_line(&session.id, "echo before-attach");
        // The output line, then the next prompt.
        wait_until("the first output", || first.text().contains("\nbefore-attach\r\n$ "));
        thread::sleep(Duration::from_millis(200));

        let second = Arc::new(Recorder::default());
        harness
            .terminals
            .attach(&session.id, Arc::clone(&second) as Arc<dyn TerminalSink>, true)
            .unwrap();
        let sent_before = first.text();
        assert_eq!(second.text(), sent_before, "the replay is everything printed so far");

        harness.type_line(&session.id, "echo after-attach");
        wait_until("the output after attaching", || second.text().contains("\nafter-attach\r\n"));
        assert_eq!(first.text(), sent_before, "the old view gets nothing more");
        assert!(second.text().starts_with(&sent_before));
        assert_eq!(
            second.text().matches("before-attach").count(),
            sent_before.matches("before-attach").count(),
            "replayed once"
        );

        // A window that reloaded follows the session without its past.
        thread::sleep(Duration::from_millis(200));
        let reloaded = Arc::new(Recorder::default());
        harness
            .terminals
            .attach(&session.id, Arc::clone(&reloaded) as Arc<dyn TerminalSink>, false)
            .unwrap();
        assert_eq!(reloaded.text(), "");
        harness.type_line(&session.id, "echo after-reload");
        wait_until("the output after the reload", || reloaded.text().contains("\nafter-reload\r\n"));
        assert!(!reloaded.text().contains("before-attach"));
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn the_scrollback_is_bounded_and_replays_from_a_line() {
        let harness = harness(4096);
        let (session, output) = harness.open(TerminalKind::Shell);

        // The shell works out the last line, so the echo of the typed command
        // cannot pass for it; the prompt after it means nothing more comes.
        harness.type_line(
            &session.id,
            "i=0; while [ $i -lt 400 ]; do echo line-$i-of-output; i=$((i+1)); done; echo done-$((40+2))",
        );
        wait_until("the loop", || output.text().contains("\ndone-42\r\n$ "));

        let late = Arc::new(Recorder::default());
        harness
            .terminals
            .attach(&session.id, Arc::clone(&late) as Arc<dyn TerminalSink>, true)
            .unwrap();
        let replayed = late.text();
        assert!(replayed.len() <= 4096);
        assert!(replayed.contains("line-399-of-output"));
        assert!(!replayed.contains("line-0-of-output"));
        assert!(replayed.starts_with("line-"), "starts at a line: {:?}", &replayed[..40]);
        harness.terminals.close(&session.id).unwrap();
    }

    #[test]
    fn names_are_unique_per_folder_and_kind() {
        let harness = harness(SCROLLBACK_BYTES);
        let (first, _) = harness.open(TerminalKind::Shell);
        let (second, _) = harness.open(TerminalKind::Shell);
        let (third, _) = harness.open(TerminalKind::Shell);
        assert_eq!([&*first.name, &*second.name, &*third.name], ["sh", "sh 2", "sh 3"]);

        harness.terminals.close(&second.id).unwrap();
        let (again, _) = harness.open(TerminalKind::Shell);
        assert_eq!(again.name, "sh 2");
        let names: Vec<String> = harness.terminals.list().into_iter().map(|session| session.name).collect();
        assert_eq!(names, ["sh", "sh 3", "sh 2"], "oldest first");
        assert_eq!(harness.terminals.count_in(&harness.cwd()), 3);
        harness.terminals.close_all();
    }

    #[test]
    fn quitting_ends_every_session_and_its_processes() {
        let harness = harness(SCROLLBACK_BYTES);
        let mut groups = Vec::new();
        for _ in 0..2 {
            let (session, output) = harness.open(TerminalKind::Shell);
            harness.type_line(&session.id, "sleep 300");
            wait_until("sleep", || !output.events().is_empty());
            let shell = i32::try_from(harness.session(&session.id).pid.unwrap()).unwrap();
            groups.push(shell);
            groups.extend(harness.session(&session.id).foreground_group());
        }
        assert_eq!(groups.len(), 4);

        harness.terminals.close_all();

        assert!(harness.terminals.list().is_empty());
        for group in groups {
            wait_until("every process to be gone", || !alive(group));
        }
    }

    #[test]
    fn unknown_sessions_and_missing_folders_are_reported() {
        let harness = harness(SCROLLBACK_BYTES);
        let unknown = harness.terminals.write("nope", Vec::new()).unwrap_err();
        assert_eq!(unknown.code, ErrorCode::UnknownTerminal);
        assert_eq!(
            harness.terminals.attach("nope", Arc::new(Recorder::default()), true).unwrap_err().code,
            ErrorCode::UnknownTerminal
        );
        let gone = harness
            .terminals
            .open(
                OpenRequest {
                    kind: TerminalKind::Shell,
                    cwd: harness.cwd().join("missing"),
                    project_id: "p-1".to_string(),
                    project_name: "demo".to_string(),
                    target: None,
                    branch: None,
                },
                &harness.settings,
                Arc::new(Recorder::default()),
            )
            .unwrap_err();
        assert_eq!(gone.code, ErrorCode::PathNotFound);
    }

    #[test]
    fn the_scrollback_keeps_the_end() {
        let mut scrollback = Scrollback::new(8);
        scrollback.push(b"abc");
        assert_eq!(scrollback.replay(), b"abc");
        scrollback.push(b"def\nghijkl");
        assert_eq!(scrollback.bytes.len(), 8);
        assert_eq!(scrollback.replay(), b"ghijkl", "starts after the line end");
    }
}
