//! The environments of provisioned analyzers (see `analyzers`): an analyzer
//! shipped as source is built, the first time a project of its language
//! needs it, into a folder of its own in the app's data folder, never at
//! install time and never inside the user's project.
//!
//! Layout, under `<app data>/analyzers/`:
//!
//! ```text
//! <analyzer id>/<source fingerprint>/
//!     source/          a copy of the shipped source, what the fingerprint covers
//!     venv/            Python: the virtual environment (its site-packages
//!                      points at source/ with a .pth file)
//!     source/node_modules/  JS/TS: what `npm ci` installed
//!     bin/<name>       Rust: the built binary (the build folder is removed)
//!     tools/           the mutation engine (Rust: cargo-mutants; JS/TS:
//!                      StrykerJS), installed by the analyzer itself
//!                      (`prepare --tools`) the first time a project asks
//!                      for mutation testing, never as part of the build
//!     provision.log    every command run and what it printed
//!     ready            written last: the environment is complete
//! ```
//!
//! The fingerprint is a hash of the shipped source folder, its files in
//! sorted order with their relative paths, leaving out build output
//! (`target/`, `node_modules/`, `__pycache__/`, virtual environments). So
//! another version of an analyzer, or an edit in development, gets a fresh
//! environment, while the same source finds the one already built. Results
//! are cached under the same fingerprint (`analyzers::Resolved`). Once an
//! environment is ready, the older ones of the same analyzer are deleted.
//!
//! Building one runs the language's own toolchain, found on the login
//! shell's PATH (`tool_path`): Python creates a virtual environment with
//! `uv venv` when uv is installed, or `python3 -m venv`, and installs
//! `requirements.txt` when it lists anything; JS/TS runs `npm ci
//! --omit=dev` (the one case that downloads, once); Rust runs `cargo build
//! --release --locked` with its build folder inside the environment. A
//! missing toolchain is a message naming it and how to install it; the rest
//! of the app keeps working.
//!
//! Concurrent requests for the same environment share one build. A folder
//! without its `ready` marker is a build that did not finish (the app quit,
//! a step failed): it is deleted and built again on the next need, and its
//! log is kept until then. Builds stop when the app exits (`stop`). Each
//! build emits its progress as `analyzer-provision` events, so the views
//! waiting for it can say what is happening.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::analyzers::{AnalyzerSpec, Launcher, Resolved, Runtime};
use crate::error::{core_text, CoreText};
use crate::model::{AnalyzerProvision, ProvisionStatus, ProvisionStep};
use crate::process::{self, Ending, Limits};
use crate::single_flight::SingleFlight;

/// The Tauri event every change of a build is emitted as.
pub const EVENT: &str = "analyzer-provision";

const READY_MARKER: &str = "ready";
const LOG_FILE: &str = "provision.log";
const SOURCE_FOLDER: &str = "source";
const TOOLS_FOLDER: &str = "tools";
/// Names of the source folder never fingerprinted nor copied: build output
/// and caches a developer's own runs leave in the repository.
const SKIPPED_NAMES: &[&str] = &[
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".git",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".DS_Store",
];
/// Output kept from one command, for the log.
const MAX_STEP_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const STEP_STDERR_TAIL_BYTES: usize = 256 * 1024;
/// How much of a failed step's output a message shows.
const MAX_DETAIL_CHARS: usize = 2000;

/// Receives the progress of builds.
pub trait ProvisionEvents: Send + Sync {
    fn emit(&self, event: &AnalyzerProvision);
}

/// Emits builds' progress to the window.
pub struct TauriProvisionEvents(pub tauri::AppHandle);

impl ProvisionEvents for TauriProvisionEvents {
    fn emit(&self, event: &AnalyzerProvision) {
        use tauri::Emitter;
        // A window that is gone has nothing to show it in.
        let _ = self.0.emit(EVENT, event);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProvisionSettings {
    /// Creating a virtual environment, reading its paths.
    pub environment_timeout: Duration,
    /// Installing dependencies (downloads).
    pub install_timeout: Duration,
    /// Compiling a Rust analyzer from scratch.
    pub compile_timeout: Duration,
}

impl Default for ProvisionSettings {
    fn default() -> Self {
        Self {
            environment_timeout: Duration::from_secs(120),
            install_timeout: Duration::from_secs(15 * 60),
            compile_timeout: Duration::from_secs(30 * 60),
        }
    }
}

type FlightKey = (&'static str, String);

pub struct AnalyzerEnvs {
    root: PathBuf,
    events: Arc<dyn ProvisionEvents>,
    settings: ProvisionSettings,
    flights: SingleFlight<FlightKey, Result<Resolved, CoreText>>,
    /// The builds going on, by analyzer id and fingerprint, as last emitted.
    active: Mutex<HashMap<FlightKey, AnalyzerProvision>>,
    /// Raised when the app exits: running steps are stopped and no build
    /// starts.
    stopping: AtomicBool,
}

impl AnalyzerEnvs {
    /// Environments are kept under `root` (`<app data>/analyzers`).
    pub fn new(root: PathBuf, events: Arc<dyn ProvisionEvents>, settings: ProvisionSettings) -> Self {
        Self {
            root,
            events,
            settings,
            flights: SingleFlight::new(),
            active: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        }
    }

    /// The analyzer of `spec` built with `runtime` from `source`, building
    /// its environment first when it has none. `path` is the PATH the
    /// toolchain is looked for in and run with.
    pub fn ensure(
        &self,
        spec: &'static AnalyzerSpec,
        runtime: Runtime,
        source: &Path,
        path: &OsStr,
    ) -> Result<Resolved, CoreText> {
        if self.stopping.load(Ordering::Relaxed) {
            return Err(stopped(spec));
        }
        let fingerprint = source_fingerprint(source).map_err(|error| disk_failed(spec, source, &error))?;
        let directory = self.root.join(spec.id).join(&fingerprint);
        if let Some(resolved) = ready(spec, runtime, &directory, &fingerprint, path)? {
            return Ok(resolved);
        }
        let key = (spec.id, fingerprint.clone());
        let abandoned = Err(core_text!(
            AnalyzerPrepareAbandoned,
            "{analyzer} could not be prepared: the preparation stopped unexpectedly.",
            analyzer = spec.title()
        ));
        self.flights.run(key.clone(), abandoned, || {
            // The build that just landed may have made it.
            if let Some(resolved) = ready(spec, runtime, &directory, &fingerprint, path)? {
                return Ok(resolved);
            }
            let progress = Progress::start(self, spec, key);
            let built = self.build(spec, runtime, source, &directory, &fingerprint, path, &progress);
            match &built {
                Ok(_) => {
                    self.remove_older(spec, &fingerprint);
                    progress.finish(ProvisionStatus::Ready, None);
                }
                Err(message) => progress.finish(ProvisionStatus::Failed, Some(message.clone())),
            }
            built
        })
    }

    /// The folder of the environment of `spec` built as `fingerprint` where
    /// the analyzer installs its mutation engine (`prepare --tools`, see
    /// `analyzers/protocol/mutation.schema.json`), the first time a project
    /// asks for mutation testing: never at build time, never globally.
    pub fn tools_folder(&self, spec: &AnalyzerSpec, fingerprint: &str) -> PathBuf {
        self.root.join(spec.id).join(fingerprint).join(TOOLS_FOLDER)
    }

    /// The builds going on now, so a view opened during one can show it.
    pub fn running(&self) -> Vec<AnalyzerProvision> {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        let mut running: Vec<_> = active.values().cloned().collect();
        running.sort_by(|a, b| a.analyzer.cmp(&b.analyzer));
        running
    }

    /// Stops the builds going on and refuses new ones; for the app's exit.
    /// Waits a little for the builds to stop their commands, which run in
    /// process groups of their own and would outlive the app. A stopped
    /// build leaves an unfinished folder, built again next time.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && !self
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        spec: &'static AnalyzerSpec,
        runtime: Runtime,
        source: &Path,
        directory: &Path,
        fingerprint: &str,
        path: &OsStr,
        progress: &Progress<'_>,
    ) -> Result<Resolved, CoreText> {
        let tools = Toolchain::find(runtime, path).map_err(|program| runtime_missing(spec, runtime, program))?;
        progress.step(ProvisionStep::CreatingEnvironment);
        let disk = |path: &Path| {
            let path = path.to_path_buf();
            move |error: io::Error| disk_failed(spec, &path, &error)
        };
        if directory.exists() {
            fs::remove_dir_all(directory).map_err(disk(directory))?;
        }
        fs::create_dir_all(directory).map_err(disk(directory))?;
        let copy = directory.join(SOURCE_FOLDER);
        copy_source(source, &copy).map_err(disk(&copy))?;
        let mut step = Steps {
            envs: self,
            spec,
            path,
            log: Log::create(&directory.join(LOG_FILE)).map_err(disk(directory))?,
        };

        match runtime {
            Runtime::Python => {
                let venv = directory.join("venv");
                let python3 = tools.program.as_os_str();
                let create = match &tools.helper {
                    Some(uv) => command(
                        uv,
                        &[
                            "venv".into(),
                            "--no-config".into(),
                            "--python".into(),
                            python3.into(),
                            venv.clone().into(),
                        ],
                    ),
                    None => command(&tools.program, &["-m".into(), "venv".into(), venv.clone().into()]),
                };
                step.run(create, directory, self.settings.environment_timeout)?;
                let python = venv_python(&venv);
                let purelib = step.run(
                    command(
                        &python,
                        &[
                            "-I".into(),
                            "-c".into(),
                            "import sysconfig; print(sysconfig.get_path('purelib'))".into(),
                        ],
                    ),
                    directory,
                    self.settings.environment_timeout,
                )?;
                let site_packages = PathBuf::from(String::from_utf8_lossy(&purelib).trim());
                let pth = site_packages.join("difflens_analyzer.pth");
                fs::write(&pth, format!("{}\n", copy.display())).map_err(disk(&pth))?;
                let requirements = copy.join("requirements.txt");
                if lists_requirements(&requirements) {
                    progress.step(ProvisionStep::InstallingDependencies);
                    let install = match &tools.helper {
                        Some(uv) => command(
                            uv,
                            &[
                                "pip".into(),
                                "install".into(),
                                "--no-config".into(),
                                "--python".into(),
                                python.clone().into(),
                                "-r".into(),
                                requirements.into(),
                            ],
                        ),
                        None => command(
                            &python,
                            &[
                                "-I".into(),
                                "-m".into(),
                                "pip".into(),
                                "install".into(),
                                "--disable-pip-version-check".into(),
                                "--no-input".into(),
                                "-r".into(),
                                requirements.into(),
                            ],
                        ),
                    };
                    step.run(install, directory, self.settings.install_timeout)?;
                }
            }
            Runtime::Node => {
                progress.step(ProvisionStep::InstallingDependencies);
                let npm = tools.helper.as_ref().unwrap_or(&tools.program);
                let arguments = [
                    "ci",
                    "--omit=dev",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--loglevel=error",
                ]
                .map(OsString::from);
                let mut install = command(npm, &arguments);
                install.env("npm_config_update_notifier", "false");
                step.run(install, &copy, self.settings.install_timeout)?;
            }
            Runtime::Cargo => {
                progress.step(ProvisionStep::Compiling);
                let target = directory.join("target");
                let mut build = command(
                    &tools.program,
                    &[
                        "build".into(),
                        "--release".into(),
                        "--locked".into(),
                        "--manifest-path".into(),
                        copy.join("Cargo.toml").into(),
                    ],
                );
                build.env("CARGO_TARGET_DIR", &target);
                step.run(build, directory, self.settings.compile_timeout)?;
                let built = target.join("release").join(executable_name(spec.name));
                let bin = directory.join("bin");
                fs::create_dir_all(&bin).map_err(disk(&bin))?;
                fs::copy(&built, bin.join(executable_name(spec.name))).map_err(disk(&built))?;
                // Hundreds of megabytes of intermediate files nothing reads again.
                let _ = fs::remove_dir_all(&target);
            }
        }

        let resolved = ready_launcher(spec, runtime, directory, path)?.map(|launcher| Resolved {
            launcher,
            fingerprint: fingerprint.to_string(),
        });
        let Some(resolved) = resolved else {
            return Err(disk_failed(
                spec,
                directory,
                &io::Error::other("the environment has no program to start the analyzer with"),
            ));
        };
        let marker = directory.join(READY_MARKER);
        fs::write(&marker, format!("{fingerprint}\n")).map_err(disk(&marker))?;
        Ok(resolved)
    }

    /// Deletes the other environments of `spec`, except those being built.
    fn remove_older(&self, spec: &AnalyzerSpec, keep: &str) {
        let building: Vec<String> = {
            let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
            active
                .keys()
                .filter(|(id, _)| *id == spec.id)
                .map(|(_, fingerprint)| fingerprint.clone())
                .collect()
        };
        let Ok(entries) = fs::read_dir(self.root.join(spec.id)) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == keep || building.contains(&name) {
                continue;
            }
            let path = entry.path();
            // An environment that cannot be deleted only takes space.
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }

    fn publish(&self, key: &FlightKey, event: AnalyzerProvision, keep: bool) {
        {
            let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
            if keep {
                active.insert(key.clone(), event.clone());
            } else {
                active.remove(key);
            }
        }
        self.events.emit(&event);
    }
}

/// The environment in `directory`, when it is complete and its program is
/// still there (a Python upgrade can leave a virtual environment without
/// its interpreter).
fn ready(
    spec: &AnalyzerSpec,
    runtime: Runtime,
    directory: &Path,
    fingerprint: &str,
    path: &OsStr,
) -> Result<Option<Resolved>, CoreText> {
    if !directory.join(READY_MARKER).is_file() {
        return Ok(None);
    }
    Ok(ready_launcher(spec, runtime, directory, path)?.map(|launcher| Resolved {
        launcher,
        fingerprint: fingerprint.to_string(),
    }))
}

/// How to start the analyzer built in `directory`, or None when its program
/// is missing. Node is looked up on `path` each time, as it is not part of
/// the environment.
fn ready_launcher(
    spec: &AnalyzerSpec,
    runtime: Runtime,
    directory: &Path,
    path: &OsStr,
) -> Result<Option<Launcher>, CoreText> {
    let launcher = match runtime {
        Runtime::Python => Launcher {
            program: venv_python(&directory.join("venv")),
            // Isolated (-I): the project's folder, the user's site-packages
            // and PYTHON* variables stay out of the analyzer's imports. No
            // bytecode (-B): the environment is read-only in the sandbox.
            leading: ["-I", "-B", "-X", "utf8", "-m", "difflens_analyzer"]
                .map(OsString::from)
                .to_vec(),
            env: Vec::new(),
        },
        Runtime::Node => {
            let node = find_program("node", path).ok_or_else(|| runtime_missing(spec, runtime, "node"))?;
            let main = directory.join(SOURCE_FOLDER).join("src").join("main.js");
            if !main.is_file() {
                return Ok(None);
            }
            Launcher {
                program: node,
                leading: vec![main.into_os_string()],
                env: Vec::new(),
            }
        }
        Runtime::Cargo => Launcher::program(directory.join("bin").join(executable_name(spec.name))),
    };
    Ok(launcher.program.is_file().then_some(launcher))
}

/// The programs a runtime builds with, found on PATH.
struct Toolchain {
    /// python3, node or cargo.
    program: PathBuf,
    /// uv for Python when installed; npm for Node.
    helper: Option<PathBuf>,
}

impl Toolchain {
    /// The toolchain of `runtime`, or the name of the program missing.
    fn find(runtime: Runtime, path: &OsStr) -> Result<Self, &'static str> {
        let required = |name: &'static str| find_program(name, path).ok_or(name);
        Ok(match runtime {
            Runtime::Python => Self {
                program: required("python3")?,
                helper: find_program("uv", path),
            },
            Runtime::Node => Self {
                program: required("node")?,
                helper: Some(required("npm")?),
            },
            Runtime::Cargo => Self {
                program: required("cargo")?,
                helper: None,
            },
        })
    }
}

/// The first executable file named `name` in the folders of `path`.
pub fn find_program(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(executable_name(name)))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn executable_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

fn command(program: &Path, arguments: &[OsString]) -> Command {
    let mut command = Command::new(program);
    command.args(arguments);
    command
}

/// A requirements file with at least one line that is not blank or a
/// comment.
fn lists_requirements(file: &Path) -> bool {
    fs::read_to_string(file).is_ok_and(|text| {
        text.lines()
            .map(str::trim)
            .any(|line| !line.is_empty() && !line.starts_with('#'))
    })
}

/// Runs the commands of one build, writing each and what it printed to the
/// build's log.
struct Steps<'a> {
    envs: &'a AnalyzerEnvs,
    spec: &'static AnalyzerSpec,
    path: &'a OsStr,
    log: Log,
}

impl Steps<'_> {
    /// Runs `command` in `folder` and returns what it printed on stdout.
    fn run(&mut self, mut command: Command, folder: &Path, timeout: Duration) -> Result<Vec<u8>, CoreText> {
        let label = describe(&command);
        command
            .current_dir(folder)
            .env("PATH", self.path)
            .env_remove("PYTHONHOME")
            .env_remove("PYTHONPATH")
            .env_remove("VIRTUAL_ENV");
        self.log.line(&format!("$ {label}"));
        let limits = Limits {
            timeout,
            max_stdout_bytes: MAX_STEP_OUTPUT_BYTES,
            stderr_tail_bytes: STEP_STDERR_TAIL_BYTES,
        };
        let captured = match process::run_captured_until(command, &limits, &self.envs.stopping) {
            Ok(captured) => captured,
            Err(error) => {
                self.log.line(&format!("could not start: {error}"));
                return Err(self.failed(&label, &error.to_string(), String::new()));
            }
        };
        self.log.bytes(&captured.stdout);
        self.log.bytes(&captured.stderr_tail);
        let output = [captured.stderr_tail.as_slice(), captured.stdout.as_slice()].concat();
        match captured.ending {
            Ending::Exited(status) if status.success() => {
                self.log.line("ok");
                Ok(captured.stdout)
            }
            Ending::Exited(status) => {
                self.log.line(&format!("failed: {status}"));
                Err(self.failed(&label, &status.to_string(), tail(&output)))
            }
            Ending::OutputTooLarge => {
                self.log.line("stopped: printed too much");
                Err(self.failed(&label, "printed too much", tail(&output)))
            }
            Ending::TimedOut => {
                self.log.line("stopped: timed out");
                Err(core_text!(
                    AnalyzerPrepareTimedOut,
                    "{analyzer} could not be prepared: {command} did not finish within {minutes} minutes. Its log is at {log}.",
                    analyzer = self.spec.title(),
                    command = label,
                    minutes = timeout.as_secs().div_ceil(60),
                    log = self.log.path.display()
                ))
            }
            Ending::Cancelled => {
                self.log.line("stopped: the app is closing");
                Err(stopped(self.spec))
            }
        }
    }

    fn failed(&self, label: &str, status: &str, detail: String) -> CoreText {
        core_text!(
            AnalyzerPrepareFailed,
            "{analyzer} could not be prepared: {command} failed ({status}). Its log is at {log}.\n{detail}",
            analyzer = self.spec.title(),
            command = label,
            status = status,
            log = self.log.path.display(),
            detail = detail
        )
    }
}

/// "uv venv --python /usr/bin/python3 …", for the log and messages.
fn describe(command: &Command) -> String {
    let program = Path::new(command.get_program())
        .file_name()
        .unwrap_or(command.get_program())
        .to_string_lossy()
        .into_owned();
    std::iter::once(program)
        .chain(command.get_args().map(|argument| argument.to_string_lossy().into_owned()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The end of a step's output, its lines kept.
fn tail(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let text = text.trim();
    let length = text.chars().count();
    if length <= MAX_DETAIL_CHARS {
        text.to_string()
    } else {
        text.chars().skip(length - MAX_DETAIL_CHARS).collect()
    }
}

/// The build's log file. A log that cannot be written only loses lines.
struct Log {
    path: PathBuf,
    file: File,
}

impl Log {
    fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            file: OpenOptions::new().create(true).append(true).open(path)?,
        })
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.file, "{text}");
    }

    fn bytes(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.file.write_all(bytes);
            if !bytes.ends_with(b"\n") {
                let _ = self.file.write_all(b"\n");
            }
        }
    }
}

/// Lists a build as going on until it finishes, emitting each change.
struct Progress<'a> {
    envs: &'a AnalyzerEnvs,
    spec: &'static AnalyzerSpec,
    key: FlightKey,
    finished: bool,
}

impl<'a> Progress<'a> {
    fn start(envs: &'a AnalyzerEnvs, spec: &'static AnalyzerSpec, key: FlightKey) -> Self {
        let progress = Self {
            envs,
            spec,
            key,
            finished: false,
        };
        progress.emit(ProvisionStatus::Preparing, None, None);
        progress
    }

    fn step(&self, step: ProvisionStep) {
        self.emit(ProvisionStatus::Preparing, Some(step), None);
    }

    fn finish(mut self, status: ProvisionStatus, message: Option<CoreText>) {
        self.finished = true;
        self.emit(status, None, message);
    }

    fn emit(&self, status: ProvisionStatus, step: Option<ProvisionStep>, message: Option<CoreText>) {
        let event = AnalyzerProvision {
            analyzer: self.spec.id.to_string(),
            language: self.spec.display_name.to_string(),
            status,
            step,
            message,
        };
        self.envs
            .publish(&self.key, event, status == ProvisionStatus::Preparing);
    }
}

impl Drop for Progress<'_> {
    fn drop(&mut self) {
        // Only when the build unwound before `finish`.
        if !self.finished {
            self.envs
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.key);
        }
    }
}

/// The files of the source folder `root` to fingerprint and copy: relative
/// paths with "/" separators, sorted, build output and symbolic links left
/// out.
fn source_files(root: &Path) -> io::Result<Vec<String>> {
    fn walk(root: &Path, folder: &Path, files: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIPPED_NAMES.contains(&name.as_ref()) || name.ends_with(".pyc") {
                continue;
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                walk(root, &path, files)?;
            } else if kind.is_file() {
                let relative = path.strip_prefix(root).map_err(io::Error::other)?;
                let parts: Vec<_> = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                files.push(parts.join("/"));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

/// A hash of the source folder `root`, in hex: each file's relative path,
/// length and bytes, in sorted order, build output left out. The same
/// source gives the same fingerprint wherever it is.
pub fn source_fingerprint(root: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    for relative in source_files(root)? {
        let file = root.join(&relative);
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(file.metadata()?.len().to_be_bytes());
        let mut reader = File::open(&file)?;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Copies the files `source_fingerprint` covers from `root` into `target`,
/// keeping their permissions.
fn copy_source(root: &Path, target: &Path) -> io::Result<()> {
    for relative in source_files(root)? {
        let destination = target.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(root.join(&relative), destination)?;
    }
    Ok(())
}

fn stopped(spec: &AnalyzerSpec) -> CoreText {
    core_text!(
        AnalyzerPrepareStopped,
        "{analyzer} was not prepared: DiffLens is closing.",
        analyzer = spec.title()
    )
}

fn disk_failed(spec: &AnalyzerSpec, path: &Path, error: &io::Error) -> CoreText {
    core_text!(
        AnalyzerPrepareDiskFailed,
        "{analyzer} could not be prepared: {path}: {detail}",
        analyzer = spec.title(),
        path = path.display(),
        detail = error
    )
}

/// "The Rust analyzer needs cargo, which was not found…", with how to
/// install it.
fn runtime_missing(spec: &AnalyzerSpec, runtime: Runtime, program: &str) -> CoreText {
    let hint = match runtime {
        Runtime::Python => core_text!(
            RuntimePythonHint,
            "Install Python 3 (python.org, or brew install python), then try again."
        ),
        Runtime::Node => core_text!(
            RuntimeNodeHint,
            "Install Node.js with npm (nodejs.org, or brew install node), then try again."
        ),
        Runtime::Cargo => core_text!(
            RuntimeCargoHint,
            "Install Rust with rustup (rustup.rs), then try again."
        ),
    };
    core_text!(
        AnalyzerRuntimeMissing,
        "{analyzer} needs {program}, which was not found on the PATH of your login shell. {hint}",
        analyzer = spec.title(),
        program = program,
        hint = hint
    )
}

#[cfg(test)]
pub mod fixtures {
    //! A data folder for environments, and folders written from a list.

    use std::path::Path;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{AnalyzerEnvs, ProvisionEvents, ProvisionSettings};
    use crate::model::AnalyzerProvision;

    /// Keeps every event emitted.
    #[derive(Default)]
    pub struct RecordedEvents(Mutex<Vec<AnalyzerProvision>>);

    impl RecordedEvents {
        pub fn all(&self) -> Vec<AnalyzerProvision> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    impl ProvisionEvents for RecordedEvents {
        fn emit(&self, event: &AnalyzerProvision) {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event.clone());
        }
    }

    /// Environments kept under `data`, with the events they emit.
    pub fn envs_in(data: &Path) -> (Arc<AnalyzerEnvs>, Arc<RecordedEvents>) {
        let events = Arc::new(RecordedEvents::default());
        let envs = AnalyzerEnvs::new(data.to_path_buf(), events.clone(), ProvisionSettings::default());
        (Arc::new(envs), events)
    }

    /// Writes `files` (relative path, contents) under `root`.
    pub fn write_tree(root: &Path, files: &[(&str, &str)]) {
        for (relative, contents) in files {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fixtures::{envs_in, write_tree, RecordedEvents};
    use super::*;
    use crate::analyzers::{self, for_language, AnalyzerLocator, Fingerprints};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Instant;
    use tempfile::TempDir;

    fn spec(id: &str) -> &'static AnalyzerSpec {
        analyzers::REGISTRY.iter().find(|spec| spec.id == id).unwrap()
    }

    /// Writes an executable shell script `name` in `tools`. It finds the
    /// system's commands itself: the PATH it gets holds only `tools`, so no
    /// real toolchain of the machine is found instead of a fake one.
    fn tool(tools: &Path, name: &str, body: &str) {
        let path = tools.join(name);
        fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// How many lines of `tools/calls` start with `prefix`.
    fn calls(tools: &Path, prefix: &str) -> usize {
        fs::read_to_string(tools.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .count()
    }

    /// A stand-in cargo: records the build, waits while `slow` or `hang` is
    /// there, fails while `fail` is, and writes the binary into
    /// CARGO_TARGET_DIR.
    fn fake_cargo(tools: &Path) {
        let t = tools.display();
        tool(
            tools,
            "cargo",
            &format!(
                r#"echo "cargo $*" >> '{t}/calls'
if [ -f '{t}/slow' ]; then sleep 1; fi
if [ -f '{t}/hang' ]; then sleep 30 & wait; fi
if [ -f '{t}/fail' ]; then echo '   Compiling difflens-analyzer-rust' >&2; echo 'error[E0425]: cannot find value `x`' >&2; exit 101; fi
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\necho built\n' > "$CARGO_TARGET_DIR/release/difflens-analyzer-rust"
chmod +x "$CARGO_TARGET_DIR/release/difflens-analyzer-rust""#
            ),
        );
    }

    /// A stand-in python3 whose `-m venv <dir>` makes a virtual environment
    /// whose python prints its site-packages and records pip.
    fn fake_python(tools: &Path) {
        let t = tools.display();
        tool(
            tools,
            "python3",
            &format!(
                r#"echo "python3 $*" >> '{t}/calls'
[ "$1" = -m ] && [ "$2" = venv ] || exit 64
mkdir -p "$3/bin" "$3/lib/site-packages"
cat > "$3/bin/python" <<'EOF'
#!/bin/sh
PATH=/usr/bin:/bin
venv="$(cd "$(dirname "$0")/.." && pwd)"
echo "venv-python $*" >> '{t}/calls'
case "$*" in
  *sysconfig*) echo "$venv/lib/site-packages" ;;
esac
EOF
chmod +x "$3/bin/python""#
            ),
        );
    }

    /// A stand-in uv: `venv … <dir>` as fake_python makes one; `pip install`
    /// is recorded.
    fn fake_uv(tools: &Path) {
        let t = tools.display();
        tool(
            tools,
            "uv",
            &format!(
                r#"echo "uv $*" >> '{t}/calls'
if [ "$1" = venv ]; then
  for last; do :; done
  '{t}/python3' -m venv "$last"
fi"#
            ),
        );
    }

    fn fake_node(tools: &Path) {
        let t = tools.display();
        tool(tools, "node", &format!(r#"echo "node $*" >> '{t}/calls'"#));
        tool(
            tools,
            "npm",
            &format!(r#"echo "npm $* in $(pwd)" >> '{t}/calls'; mkdir -p node_modules/typescript"#),
        );
    }

    struct Setup {
        tools: TempDir,
        data: TempDir,
        sources: TempDir,
        envs: Arc<AnalyzerEnvs>,
        events: Arc<RecordedEvents>,
    }

    impl Setup {
        fn new() -> Self {
            let tools = TempDir::new().unwrap();
            let data = TempDir::new().unwrap();
            let sources = TempDir::new().unwrap();
            write_tree(
                &sources.path().join("rust"),
                &[("Cargo.toml", "[package]\nname = \"difflens-analyzer-rust\"\n"), ("Cargo.lock", ""), ("src/main.rs", "fn main() {}\n")],
            );
            write_tree(
                &sources.path().join("python"),
                &[("requirements.txt", "# Nothing yet.\n"), ("difflens_analyzer/__main__.py", "print('x')\n")],
            );
            write_tree(
                &sources.path().join("jsts"),
                &[("package.json", "{}\n"), ("package-lock.json", "{}\n"), ("src/main.js", "\n")],
            );
            let (envs, events) = envs_in(data.path());
            Self {
                tools,
                data,
                sources,
                envs,
                events,
            }
        }

        fn path(&self) -> OsString {
            self.tools.path().as_os_str().to_os_string()
        }

        fn ensure(&self, id: &str) -> Result<Resolved, CoreText> {
            let spec = spec(id);
            let analyzers::Delivery::Provisioned { runtime, folder } = spec.delivery else {
                panic!("{id} is not provisioned");
            };
            self.envs.ensure(spec, runtime, &self.sources.path().join(folder), &self.path())
        }

        fn environments(&self, id: &str) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(self.data.path().join(id))
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }

        fn flag(&self, name: &str, on: bool) {
            let path = self.tools.path().join(name);
            if on {
                fs::write(path, "").unwrap();
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }

    #[test]
    fn the_fingerprint_covers_the_source_only_and_does_not_depend_on_where_it_is() {
        let one = TempDir::new().unwrap();
        let two = TempDir::new().unwrap();
        write_tree(one.path(), &[("a.py", "a"), ("pkg/b.py", "b"), ("requirements.txt", "")]);
        write_tree(two.path(), &[("requirements.txt", ""), ("pkg/b.py", "b"), ("a.py", "a")]);
        let first = source_fingerprint(one.path()).unwrap();
        assert_eq!(source_fingerprint(two.path()).unwrap(), first);
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");

        write_tree(
            two.path(),
            &[
                ("target/release/x", "built"),
                ("node_modules/typescript/index.js", "dep"),
                ("pkg/__pycache__/b.cpython-314.pyc", "bytecode"),
                ("pkg/c.pyc", "bytecode"),
                (".venv/bin/python", "venv"),
                (".DS_Store", "finder"),
            ],
        );
        assert_eq!(source_fingerprint(two.path()).unwrap(), first, "build output is left out");

        write_tree(two.path(), &[("pkg/b.py", "B")]);
        assert_ne!(source_fingerprint(two.path()).unwrap(), first, "contents count");
        write_tree(two.path(), &[("pkg/b.py", "b")]);
        fs::rename(two.path().join("a.py"), two.path().join("z.py")).unwrap();
        assert_ne!(source_fingerprint(two.path()).unwrap(), first, "names count");
        // The same bytes split differently between two files.
        let split = TempDir::new().unwrap();
        write_tree(split.path(), &[("a.py", "ab"), ("pkg/b.py", ""), ("requirements.txt", "")]);
        assert_ne!(source_fingerprint(split.path()).unwrap(), first);
    }

    #[test]
    fn the_fingerprint_of_a_known_tree_is_pinned() {
        // SHA-256 of "a.txt", a NUL, the length 2 as a big-endian u64 and
        // "hi": prepared environments are keyed by it, so it must not change
        // with the hashing crate.
        let root = TempDir::new().unwrap();
        write_tree(root.path(), &[("a.txt", "hi")]);
        assert_eq!(
            source_fingerprint(root.path()).unwrap(),
            "78a424e84024d4146fdace4d20f276f4bd45a8ff2cb3b584d373a2eb70c201ce"
        );
    }

    #[test]
    fn the_copy_leaves_out_what_the_fingerprint_does() {
        let source = TempDir::new().unwrap();
        write_tree(
            source.path(),
            &[("src/main.js", "main"), ("package.json", "{}"), ("node_modules/x/index.js", "dep")],
        );
        let target = TempDir::new().unwrap();
        copy_source(source.path(), &target.path().join("copy")).unwrap();
        assert_eq!(source_files(&target.path().join("copy")).unwrap(), ["package.json", "src/main.js"]);
        assert!(!target.path().join("copy/node_modules").exists());
        assert_eq!(
            source_fingerprint(&target.path().join("copy")).unwrap(),
            source_fingerprint(source.path()).unwrap()
        );
    }

    #[test]
    fn a_rust_analyzer_is_built_once_with_its_progress_and_found_again() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());

        let resolved = setup.ensure("rust").unwrap();

        let fingerprint = source_fingerprint(&setup.sources.path().join("rust")).unwrap();
        let directory = setup.data.path().join("rust").join(&fingerprint);
        assert_eq!(resolved.fingerprint, fingerprint);
        assert_eq!(resolved.launcher, Launcher::program(directory.join("bin/difflens-analyzer-rust")));
        assert!(directory.join(READY_MARKER).is_file());
        assert!(!directory.join("target").exists(), "the build folder is removed");
        assert!(directory.join("source/src/main.rs").is_file());
        let log = fs::read_to_string(directory.join(LOG_FILE)).unwrap();
        assert!(log.contains("$ cargo build --release --locked --manifest-path"), "{log}");
        let events: Vec<_> = setup.events.all().into_iter().map(|event| (event.status, event.step)).collect();
        assert_eq!(
            events,
            [
                (ProvisionStatus::Preparing, None),
                (ProvisionStatus::Preparing, Some(ProvisionStep::CreatingEnvironment)),
                (ProvisionStatus::Preparing, Some(ProvisionStep::Compiling)),
                (ProvisionStatus::Ready, None),
            ]
        );
        assert!(setup.envs.running().is_empty());
        assert_eq!(setup.events.all()[0].language, "Rust");

        assert_eq!(setup.ensure("rust").unwrap(), resolved);
        assert_eq!(calls(setup.tools.path(), "cargo build"), 1);
        assert_eq!(setup.events.all().len(), 4, "a ready environment emits nothing");
    }

    #[test]
    fn concurrent_requests_share_one_build_and_show_it_as_running() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        setup.flag("slow", true);
        let barrier = Barrier::new(3);

        let results: Vec<_> = thread::scope(|scope| {
            let workers: Vec<_> = (0..3)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        setup.ensure("rust")
                    })
                })
                .collect();
            // While it builds, it is listed.
            let started = Instant::now();
            while setup.envs.running().is_empty() && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(10));
            }
            let running = setup.envs.running();
            assert_eq!(running.len(), 1);
            assert_eq!((running[0].analyzer.as_str(), running[0].status), ("rust", ProvisionStatus::Preparing));
            workers.into_iter().map(|worker| worker.join().unwrap()).collect()
        });

        assert!(results.iter().all(|result| result.as_ref().unwrap() == results[0].as_ref().unwrap()));
        assert_eq!(calls(setup.tools.path(), "cargo build"), 1);
    }

    #[test]
    fn a_half_made_environment_is_removed_and_built_again() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        let fingerprint = source_fingerprint(&setup.sources.path().join("rust")).unwrap();
        let directory = setup.data.path().join("rust").join(&fingerprint);
        // What a build stopped halfway leaves: no marker.
        write_tree(&directory, &[("bin/difflens-analyzer-rust", "half"), ("leftover", "x")]);

        setup.ensure("rust").unwrap();

        assert!(!directory.join("leftover").exists());
        assert!(directory.join(READY_MARKER).is_file());
        assert_eq!(calls(setup.tools.path(), "cargo build"), 1);
    }

    #[test]
    fn a_ready_environment_whose_program_is_gone_is_built_again() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        let resolved = setup.ensure("rust").unwrap();
        fs::remove_file(&resolved.launcher.program).unwrap();

        assert_eq!(setup.ensure("rust").unwrap(), resolved);
        assert_eq!(calls(setup.tools.path(), "cargo build"), 2);
    }

    #[test]
    fn older_environments_of_the_analyzer_are_deleted_once_a_new_one_is_ready() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        write_tree(&setup.data.path().join("rust/0ld"), &[(READY_MARKER, ""), ("bin/difflens-analyzer-rust", "")]);
        write_tree(&setup.data.path().join("python/0ther"), &[(READY_MARKER, "")]);

        let resolved = setup.ensure("rust").unwrap();

        assert_eq!(setup.environments("rust"), [resolved.fingerprint]);
        assert_eq!(setup.environments("python"), ["0ther"], "other analyzers are left alone");
    }

    #[test]
    fn a_missing_toolchain_is_named_with_how_to_install_it_and_builds_nothing() {
        let setup = Setup::new();
        let error = setup.ensure("rust").unwrap_err();
        assert_eq!(
            error,
            "The Rust analyzer needs cargo, which was not found on the PATH of your login shell. Install Rust with rustup (rustup.rs), then try again."
        );
        assert_eq!(error.reason.map(|reason| reason.key()), Some("analyzer_runtime_missing"));
        let error = setup.ensure("python").unwrap_err();
        assert!(error.starts_with("The Python analyzer needs python3"), "{error}");
        fake_node(setup.tools.path());
        fs::remove_file(setup.tools.path().join("npm")).unwrap();
        let error = setup.ensure("jsts").unwrap_err();
        assert!(error.starts_with("The JavaScript/TypeScript analyzer needs npm"), "{error}");

        assert!(setup.environments("rust").is_empty());
        let last = setup.events.all().pop().unwrap();
        assert_eq!(last.status, ProvisionStatus::Failed);
        assert!(last.message.unwrap().contains("needs npm"));
        assert!(setup.envs.running().is_empty());
    }

    #[test]
    fn a_failed_build_says_why_keeps_its_log_and_is_tried_again() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        setup.flag("fail", true);

        let error = setup.ensure("rust").unwrap_err();

        assert!(error.starts_with("The Rust analyzer could not be prepared: cargo build --release --locked"), "{error}");
        assert!(error.contains("error[E0425]: cannot find value `x`"), "{error}");
        assert!(error.contains("provision.log"), "{error}");
        let fingerprint = source_fingerprint(&setup.sources.path().join("rust")).unwrap();
        let directory = setup.data.path().join("rust").join(&fingerprint);
        assert!(!directory.join(READY_MARKER).exists());
        assert!(fs::read_to_string(directory.join(LOG_FILE)).unwrap().contains("E0425"));
        assert_eq!(setup.events.all().pop().unwrap().status, ProvisionStatus::Failed);

        setup.flag("fail", false);
        setup.ensure("rust").unwrap();
        assert_eq!(calls(setup.tools.path(), "cargo build"), 2);
    }

    #[test]
    fn stopping_ends_a_build_at_once_and_refuses_new_ones() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        setup.flag("hang", true);
        let started = Instant::now();

        let error = thread::scope(|scope| {
            let build = scope.spawn(|| setup.ensure("rust"));
            while setup.envs.running().is_empty() && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(200));
            setup.envs.stop();
            build.join().unwrap().unwrap_err()
        });

        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error, "The Rust analyzer was not prepared: DiffLens is closing.");
        setup.flag("hang", false);
        assert_eq!(setup.ensure("rust").unwrap_err(), error);
    }

    #[test]
    fn python_uses_uv_when_installed_and_runs_isolated_from_the_project() {
        let setup = Setup::new();
        fake_python(setup.tools.path());
        fake_uv(setup.tools.path());

        let resolved = setup.ensure("python").unwrap();

        let directory = setup.data.path().join("python").join(&resolved.fingerprint);
        assert_eq!(resolved.launcher.program, directory.join("venv/bin/python"));
        assert_eq!(resolved.launcher.leading, ["-I", "-B", "-X", "utf8", "-m", "difflens_analyzer"]);
        let pth = fs::read_to_string(directory.join("venv/lib/site-packages/difflens_analyzer.pth")).unwrap();
        assert_eq!(pth.trim(), directory.join("source").display().to_string());
        assert_eq!(calls(setup.tools.path(), "uv venv --no-config --python"), 1);
        assert_eq!(calls(setup.tools.path(), "uv pip"), 0, "an empty requirements file installs nothing");
        let steps: Vec<_> = setup.events.all().into_iter().filter_map(|event| event.step).collect();
        assert_eq!(steps, [ProvisionStep::CreatingEnvironment]);
    }

    #[test]
    fn python_without_uv_uses_venv_and_pip_for_its_requirements() {
        let setup = Setup::new();
        fake_python(setup.tools.path());
        write_tree(&setup.sources.path().join("python"), &[("requirements.txt", "# pinned\nlibcst==1.5.0\n")]);

        setup.ensure("python").unwrap();

        assert_eq!(calls(setup.tools.path(), "python3 -m venv"), 1);
        assert_eq!(calls(setup.tools.path(), "venv-python -I -m pip install"), 1);
        let steps: Vec<_> = setup.events.all().into_iter().filter_map(|event| event.step).collect();
        assert_eq!(steps, [ProvisionStep::CreatingEnvironment, ProvisionStep::InstallingDependencies]);
    }

    #[test]
    fn jsts_installs_its_runtime_dependencies_and_runs_on_node() {
        let setup = Setup::new();
        fake_node(setup.tools.path());

        let resolved = setup.ensure("jsts").unwrap();

        let directory = setup.data.path().join("jsts").join(&resolved.fingerprint);
        assert_eq!(resolved.launcher.program, setup.tools.path().join("node"));
        assert_eq!(resolved.launcher.leading, [directory.join("source/src/main.js").into_os_string()]);
        assert_eq!(calls(setup.tools.path(), "npm ci --omit=dev --ignore-scripts"), 1);
        let npm = fs::read_to_string(setup.tools.path().join("calls")).unwrap();
        assert!(npm.contains(&format!("in {}", physical(&directory.join("source")))), "{npm}");
        assert!(directory.join("source/node_modules/typescript").is_dir());

        // Node is not part of the environment: without it, nothing runs.
        fs::remove_file(setup.tools.path().join("node")).unwrap();
        let error = setup.ensure("jsts").unwrap_err();
        assert!(error.contains("needs node"), "{error}");
    }

    /// `path` as `pwd` prints it (macOS temporary folders are under a link).
    fn physical(path: &Path) -> String {
        crate::sandbox::canonical(path).display().to_string()
    }

    #[test]
    fn a_locator_resolves_a_provisioned_analyzer_through_its_environment() {
        let setup = Setup::new();
        fake_cargo(setup.tools.path());
        let locator = AnalyzerLocator::with_sources(Arc::clone(&setup.envs), setup.sources.path().to_path_buf());

        let resolved = locator
            .resolve(for_language("rust").unwrap(), &Fingerprints::default(), &setup.path())
            .unwrap();

        assert_eq!(resolved.fingerprint, source_fingerprint(&setup.sources.path().join("rust")).unwrap());
        assert!(resolved.launcher.program.ends_with("bin/difflens-analyzer-rust"));
    }

    #[test]
    fn finding_a_program_needs_an_executable_file() {
        let tools = TempDir::new().unwrap();
        fs::write(tools.path().join("plain"), "").unwrap();
        tool(tools.path(), "runnable", "true");
        fs::create_dir(tools.path().join("folder")).unwrap();
        let path = OsString::from(format!("relative:{}", tools.path().display()));
        assert_eq!(find_program("runnable", &path), Some(tools.path().join("runnable")));
        assert_eq!(find_program("plain", &path), None);
        assert_eq!(find_program("folder", &path), None);
        assert_eq!(find_program("missing", &path), None);
    }

    /// Builds the environment of the shipped analyzer `id` (the repository's
    /// `analyzers/<folder>/`) for real, with the toolchain of the login
    /// shell, then runs each of its commands: analyze and check as the flow
    /// does, prepare as a plain process and test under the macOS sandbox as
    /// a test run does. Each command must print a document the core accepts;
    /// what the documents say is the analyzer's own business, so these hold
    /// for the stubs and for the analyzers that replace them.
    fn provision_and_run_for_real(id: &str, tree_files: &[(&str, &str)]) {
        use crate::analysis;
        use crate::runner_protocol;
        use crate::tool_path::ToolPath;

        let data = TempDir::new().unwrap();
        let (envs, events) = envs_in(data.path());
        let spec = spec(id);
        let analyzers::Delivery::Provisioned { runtime, folder } = spec.delivery else {
            panic!("{id} is not provisioned");
        };
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../analyzers").join(folder);
        let tool_path = ToolPath::from_login_shell();
        let path = tool_path.get();
        let started = Instant::now();
        let resolved = envs
            .ensure(spec, runtime, &source, path)
            .unwrap_or_else(|error| panic!("{error}"));
        println!("{id}: provisioned in {:?} as {:?}", started.elapsed(), resolved.launcher);
        assert_eq!(events.all().last().unwrap().status, ProvisionStatus::Ready);
        assert_eq!(envs.ensure(spec, runtime, &source, path).unwrap(), resolved, "found again");

        let tree = TempDir::new().unwrap();
        write_tree(tree.path(), tree_files);
        let settings = analyzers::RunSettings::default();
        let analysis = analyzers::run(spec, &resolved.launcher, tree.path(), path, &settings)
            .unwrap_or_else(|error| panic!("{error}"));
        println!("{id}: analysis {analysis:?}");
        // On the bare tree, before prepare, as the Quality tab searches it.
        let mut added = crate::hunks::ChangedLines::default();
        for (file, contents) in tree_files {
            added.add_whole_file(file, contents.lines().count() as u32);
        }
        let inputs = TempDir::new().unwrap();
        let changed = crate::duplication::ChangedInput::write(inputs.path(), &added)
            .unwrap_or_else(|error| panic!("{error}"));
        let request = analyzers::DupesRequest {
            changed: changed.path(),
            min_tokens: 50,
            min_lines: 6,
        };
        let dupes = analyzers::run_dupes(spec, &resolved.launcher, tree.path(), path, &settings, request)
            .unwrap_or_else(|error| panic!("{error}"));
        println!("{id}: duplicates {dupes:?}");
        // Bytes of the analysis the core would cache.
        let again = serde_json::to_vec(&analysis).unwrap();
        analysis::parse(&again, spec.languages, "again").unwrap();

        // The project's build cache, as `build_cache` gives it.
        let cache = TempDir::new().unwrap();
        let cache_directory = crate::sandbox::canonical(cache.path());
        let prepare = resolved
            .launcher
            .command(&analyzers::prepare_arguments(tree.path(), &cache_directory, None))
            .current_dir(tree.path())
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(prepare.status.success(), "{}", String::from_utf8_lossy(&prepare.stderr));
        runner_protocol::parse_prepare(&prepare.stdout, "x").unwrap();
        // On the prepared tree, as the Quality tab checks one that needs the
        // dependencies (the others do not mind).
        let checks = analyzers::run_checks(spec, &resolved.launcher, tree.path(), &cache_directory, path, &settings)
            .unwrap_or_else(|error| panic!("{error}"));
        println!("{id}: checks {checks:?}");

        if cfg!(target_os = "macos") {
            let run = TempDir::new().unwrap();
            let run_directory = crate::sandbox::canonical(run.path());
            let copy = run_directory.join("tree");
            write_tree(&copy, tree_files);
            fs::create_dir_all(&copy).unwrap();
            let progress = run_directory.join("progress.json");
            let policy = crate::sandbox::Policy {
                run_directory: run_directory.clone(),
                temporary: run_directory.join("tmp"),
                writable: vec![cache_directory.clone()],
            };
            fs::create_dir_all(&policy.temporary).unwrap();
            let arguments: Vec<OsString> = vec![
                "test".into(),
                "--dir".into(),
                copy.clone().into(),
                "--timeout".into(),
                "60s".into(),
                "--cache-dir".into(),
                cache_directory.clone().into(),
                "--progress".into(),
                progress.clone().into(),
                "--offline".into(),
            ];
            let output = resolved
                .launcher
                .sandboxed(Path::new(crate::sandbox::SANDBOX_EXEC), &policy, &arguments)
                .current_dir(&copy)
                .env("PATH", path)
                .env("TMPDIR", &policy.temporary)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let tests = runner_protocol::parse_tests(&output.stdout, spec.languages, "x")
                .unwrap_or_else(|error| panic!("{error}"));
            println!("{id}: tests {tests:?}");
            // It may be missing until the first packages are listed.
            if progress.exists() {
                runner_protocol::parse_progress(&fs::read(&progress).unwrap()).unwrap();
            }

            // Mutation testing as `mutation_runs` runs it: prepare installs
            // the engine into the environment's folder (once), then mutate
            // runs under the sandbox on the lines the tests covered.
            if let Some(tools) = spec.mutation_tools.then(|| envs.tools_folder(spec, &resolved.fingerprint)) {
                let started = Instant::now();
                let prepared = resolved
                    .launcher
                    .command(&analyzers::prepare_arguments(&copy, &cache_directory, Some(&tools)))
                    .current_dir(&copy)
                    .env("PATH", path)
                    .output()
                    .unwrap();
                assert!(prepared.status.success(), "{}", String::from_utf8_lossy(&prepared.stderr));
                println!("{id}: mutation engine ready in {:?}", started.elapsed());
            }
            // Without the language's coverage tool on this machine, every line
            // counts as covered, so mutate still has lines to try.
            let coverage = tests.coverage.clone().unwrap_or_else(|| runner_protocol::Coverage {
                blocks: tree_files
                    .iter()
                    .map(|(file, contents)| runner_protocol::CoverageBlock {
                        file: file.to_string(),
                        start_line: 1,
                        end_line: (contents.lines().count() as u32).max(1),
                        statements: 1,
                        count: 1,
                    })
                    .collect(),
            });
            println!("{id}: {} coverage blocks", coverage.blocks.len());
            let changed_file = run_directory.join("changed.json");
            fs::write(&changed_file, crate::duplication::changed_document(&added)).unwrap();
            let coverage_file = run_directory.join("coverage.json");
            fs::write(&coverage_file, crate::mutation::coverage_document(&coverage)).unwrap();
            let mutation_progress = run_directory.join("mutation-progress.json");
            let mut arguments: Vec<OsString> = vec![
                "mutate".into(),
                "--dir".into(),
                copy.clone().into(),
                "--timeout".into(),
                "600s".into(),
                "--changed".into(),
                changed_file.into(),
                "--coverage".into(),
                coverage_file.into(),
                "--cache-dir".into(),
                cache_directory.clone().into(),
                "--max-mutants".into(),
                "20".into(),
                "--jobs".into(),
                "2".into(),
                "--progress".into(),
                mutation_progress.clone().into(),
                "--offline".into(),
            ];
            if spec.mutation_tools {
                arguments.push("--tools".into());
                arguments.push(envs.tools_folder(spec, &resolved.fingerprint).into());
            }
            let started = Instant::now();
            let output = resolved
                .launcher
                .sandboxed(Path::new(crate::sandbox::SANDBOX_EXEC), &policy, &arguments)
                .current_dir(&copy)
                .env("PATH", path)
                .env("TMPDIR", &policy.temporary)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let mutation = crate::mutation::parse(&output.stdout, spec.languages, "x")
                .unwrap_or_else(|error| panic!("{error}"));
            println!(
                "{id}: mutation in {:?}: {:?} {:?}",
                started.elapsed(),
                crate::mutation::summary(&mutation),
                mutation.mutants
            );
            if mutation_progress.exists() {
                crate::mutation::parse_progress(&fs::read(&mutation_progress).unwrap()).unwrap();
            }
        }
    }

    #[test]
    #[ignore = "creates a real virtual environment with this machine's python3 (uv when installed) from analyzers/python"]
    fn the_python_stub_is_provisioned_and_answers_every_command() {
        provision_and_run_for_real(
            "python",
            &[
                ("app.py", "def is_adult(age):\n    return age >= 18\n"),
                (
                    "test_app.py",
                    "import unittest\n\nfrom app import is_adult\n\n\nclass IsAdult(unittest.TestCase):\n    def test_adult(self):\n        self.assertTrue(is_adult(30))\n\n    def test_child(self):\n        self.assertFalse(is_adult(5))\n",
                ),
            ],
        );
    }

    #[test]
    #[ignore = "downloads typescript from the npm registry (npm ci) into a temporary environment"]
    fn the_jsts_stub_is_provisioned_and_answers_every_command() {
        provision_and_run_for_real(
            "jsts",
            &[
                (
                    "package.json",
                    "{ \"name\": \"stub\", \"private\": true, \"type\": \"module\", \"scripts\": { \"test\": \"node --test\" } }\n",
                ),
                ("tsconfig.json", "{}\n"),
                ("src/index.ts", "export const one = 1;\n"),
                ("src/limits.js", "export function isAdult(age) {\n  return age >= 18;\n}\n"),
                (
                    "test/limits.test.js",
                    "import test from \"node:test\";\nimport assert from \"node:assert/strict\";\nimport { isAdult } from \"../src/limits.js\";\n\ntest(\"adults and children\", () => {\n  assert.equal(isAdult(30), true);\n  assert.equal(isAdult(5), false);\n});\n",
                ),
            ],
        );
    }

    #[test]
    #[ignore = "downloads crates and compiles analyzers/rust with this machine's cargo (a minute or more)"]
    fn the_rust_stub_is_provisioned_and_answers_every_command() {
        provision_and_run_for_real(
            "rust",
            &[
                ("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
                (
                    "src/lib.rs",
                    "pub fn one() -> u8 { 1 }\n\npub fn is_adult(age: u8) -> bool {\n    age >= 18\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adults_and_children() {\n        assert!(is_adult(30));\n        assert!(!is_adult(5));\n    }\n}\n",
                ),
            ],
        );
    }

    #[test]
    fn requirements_count_only_when_they_list_something() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("requirements.txt");
        assert!(!lists_requirements(&file));
        fs::write(&file, "\n# none yet\n   \n").unwrap();
        assert!(!lists_requirements(&file));
        fs::write(&file, "# pinned\nlibcst==1.5.0\n").unwrap();
        assert!(lists_requirements(&file));
    }
}
