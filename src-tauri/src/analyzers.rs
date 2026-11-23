//! The language analyzers: which languages have one, how each is started,
//! and running it on a source tree. The core never looks inside a language;
//! an analyzer is a program that prints one JSON document per command (see
//! `analysis`, `runner_protocol` and `analyzers/protocol/`).
//!
//! An analyzer reaches the user one of two ways (`Delivery`):
//! - as a sidecar, a prebuilt executable bundled next to the app (Go: built
//!   by `pnpm analyzers:build`, listed in `bundle.externalBin`);
//! - provisioned: shipped as source (`analyzers/<folder>/`, in the bundle's
//!   resources) and built into an environment of its own the first time a
//!   project of its language needs it (`analyzer_envs`). Python, JS/TS and
//!   Rust are written in their own language, each with its own toolchain, so
//!   the app installs nothing a user's projects do not need, and nothing at
//!   install time.
//!
//! Either way it is started through a `Launcher` (a program, the arguments
//! that come before the command, and environment variables), so whoever runs
//! an analyzer does not care how it was delivered.
//!
//! Finding an analyzer, first match wins:
//! 1. the override variable (`DIFFLENS_ANALYZER_GO=/path/to/analyzer`, and
//!    `DIFFLENS_ANALYZER_PYTHON`, `_JSTS` and `_RUST` for development): a
//!    program run as it is;
//! 2. for a sidecar, next to the application executable, where Tauri puts the
//!    sidecars (also in `tauri dev`, which copies them into the target
//!    folder), then in debug builds `src-tauri/binaries/<name>-<target
//!    triple>`, where `pnpm analyzers:build` writes it;
//! 3. for a provisioned analyzer, its shipped source: in debug builds the
//!    repository's `analyzers/<folder>/` first (so an edit is picked up),
//!    then the bundle's resources; and the environment built from it.
//!
//! Results are cached under a fingerprint of the analyzer (see `Resolved`):
//! the SHA-256 of the program's bytes for a sidecar or an override, the
//! fingerprint of the shipped source for a provisioned analyzer, and both for
//! an override of a provisioned analyzer, whose program is often a wrapper
//! script that stays the same while the source it runs changes.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crate::analysis::{self, Analysis};
use crate::analyzer_envs::{self, AnalyzerEnvs};
use crate::duplication;
use crate::error::{core_text, CoreText};
use crate::fingerprint;
use crate::process::{self, Ending, Limits};
use crate::runner_protocol::{self, ChecksOutput};
use crate::sandbox;

/// The toolchain a provisioned analyzer is built and run with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// `python3` (and `uv` when there is one): a virtual environment.
    Python,
    /// `node` and `npm`: `npm ci` of the shipped lockfile.
    Node,
    /// `cargo`: a release build of the shipped lockfile.
    Cargo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// A prebuilt executable bundled as a sidecar; `executable` is its name
    /// without target triple or extension.
    Sidecar { executable: &'static str },
    /// Shipped as the source folder `analyzers/<folder>/` and built with
    /// `runtime` on first need.
    Provisioned {
        runtime: Runtime,
        folder: &'static str,
    },
}

#[derive(Debug)]
pub struct AnalyzerSpec {
    /// Names the analyzer in cache folders, environments and single flights,
    /// such as "go" or "jsts".
    pub id: &'static str,
    /// The language ids it analyses, as `languages::detect` reports them;
    /// what its documents may report as `language`.
    pub languages: &'static [&'static str],
    /// For messages: "The Go analyzer".
    pub display_name: &'static str,
    /// The analyzer's own name, as it reports it (`analyzer.name`).
    pub name: &'static str,
    pub override_variable: &'static str,
    pub delivery: Delivery,
    /// It also runs the project's tests (`prepare` and `test`, see
    /// `analyzers/protocol/tests.schema.json`).
    pub tests: bool,
    /// It also runs static checkers (`check`).
    pub checks: bool,
    /// Its checkers need the project's dependencies (the project's own tsc
    /// and ESLint, mypy resolving imports): they run on a throwaway copy of
    /// the tree that `prepare` ran on first, instead of the shared tree.
    pub checks_need_prepare: bool,
    /// Its `mutate` drives an engine that `prepare --tools <folder>`
    /// installs (see `analyzers/protocol/mutation.schema.json`), into a
    /// folder of its environment (`AnalyzerLocator::tools_folder`).
    pub mutation_tools: bool,
}

/// Add a language analyzer by adding it here, and to `bundle.externalBin`
/// (a sidecar) or `bundle.resources` (a provisioned one).
pub const REGISTRY: &[AnalyzerSpec] = &[
    AnalyzerSpec {
        id: "go",
        languages: &["go"],
        display_name: "Go",
        name: "difflens-analyzer-go",
        override_variable: "DIFFLENS_ANALYZER_GO",
        delivery: Delivery::Sidecar {
            executable: "difflens-analyzer-go",
        },
        tests: true,
        checks: true,
        // go vet fetches the modules it needs itself.
        checks_need_prepare: false,
        // Its own mutator, on go/ast.
        mutation_tools: false,
    },
    AnalyzerSpec {
        id: "python",
        languages: &["python"],
        display_name: "Python",
        name: "difflens-analyzer-python",
        override_variable: "DIFFLENS_ANALYZER_PYTHON",
        delivery: Delivery::Provisioned {
            runtime: Runtime::Python,
            folder: "python",
        },
        tests: true,
        checks: true,
        // mypy and ruff from the project's environment, imports resolved.
        checks_need_prepare: true,
        // Its own mutator, on ast.
        mutation_tools: false,
    },
    AnalyzerSpec {
        id: "jsts",
        languages: &["javascript", "typescript"],
        display_name: "JavaScript/TypeScript",
        name: "difflens-analyzer-jsts",
        override_variable: "DIFFLENS_ANALYZER_JSTS",
        delivery: Delivery::Provisioned {
            runtime: Runtime::Node,
            folder: "jsts",
        },
        tests: true,
        checks: true,
        // The project's tsc and ESLint live in its node_modules.
        checks_need_prepare: true,
        // StrykerJS.
        mutation_tools: true,
    },
    AnalyzerSpec {
        id: "rust",
        languages: &["rust"],
        display_name: "Rust",
        name: "difflens-analyzer-rust",
        override_variable: "DIFFLENS_ANALYZER_RUST",
        delivery: Delivery::Provisioned {
            runtime: Runtime::Cargo,
            folder: "rust",
        },
        tests: true,
        checks: true,
        // clippy copies the tree and fetches the crates itself.
        checks_need_prepare: false,
        // cargo-mutants.
        mutation_tools: true,
    },
];

/// The analyzer of the language id `language`, as `languages::detect`
/// reports it.
pub fn for_language(language: &str) -> Option<&'static AnalyzerSpec> {
    REGISTRY
        .iter()
        .find(|spec| spec.languages.contains(&language))
}

/// The analyzers of `languages`, each once, in the order of the languages.
pub fn for_languages(languages: &[String]) -> Vec<&'static AnalyzerSpec> {
    let mut specs: Vec<&'static AnalyzerSpec> = Vec::new();
    for spec in languages.iter().filter_map(|language| for_language(language)) {
        if !specs.iter().any(|known| known.id == spec.id) {
            specs.push(spec);
        }
    }
    specs
}

impl AnalyzerSpec {
    /// "The Go analyzer", to start a sentence.
    pub fn title(&self) -> CoreText {
        core_text!(
            AnalyzerTitle,
            "The {language} analyzer",
            language = self.display_name
        )
    }

    /// Built into an environment of its own on first need.
    pub fn is_provisioned(&self) -> bool {
        matches!(self.delivery, Delivery::Provisioned { .. })
    }

    pub fn not_installed(&self) -> CoreText {
        match self.delivery {
            Delivery::Sidecar { .. } => core_text!(
                AnalyzerNotInstalled,
                "{analyzer} is not installed; run pnpm analyzers:build.",
                analyzer = self.title()
            ),
            Delivery::Provisioned { folder, .. } => core_text!(
                AnalyzerSourceMissing,
                "{analyzer} is missing from this installation (its source folder analyzers/{folder} was not found).",
                analyzer = self.title(),
                folder = folder
            ),
        }
    }
}

/// How to start an analyzer: `program`, then `leading` before the command
/// and its flags, with `env` added to the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub program: PathBuf,
    pub leading: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl Launcher {
    /// A program that is the analyzer itself.
    pub fn program(program: PathBuf) -> Self {
        Self {
            program,
            leading: Vec::new(),
            env: Vec::new(),
        }
    }

    /// A command that runs the analyzer with `arguments` after the leading
    /// ones.
    pub fn command(&self, arguments: &[OsString]) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.leading)
            .args(arguments)
            .envs(self.env.iter().map(|(name, value)| (name, value)));
        command
    }

    /// As `command`, under the sandbox of `policy` run by `sandbox_exec`.
    /// The environment set on `sandbox-exec` reaches the analyzer.
    pub fn sandboxed(
        &self,
        sandbox_exec: &Path,
        policy: &sandbox::Policy,
        arguments: &[OsString],
    ) -> Command {
        let all: Vec<OsString> = self.leading.iter().chain(arguments).cloned().collect();
        let mut command = sandbox::command(sandbox_exec, policy, &self.program, &all);
        command.envs(self.env.iter().map(|(name, value)| (name, value)));
        command
    }
}

/// An analyzer ready to run, and the fingerprint its results are cached
/// under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub launcher: Launcher,
    pub fingerprint: String,
}

/// Where the shipped sources of provisioned analyzers are, and the
/// environments built from them.
#[derive(Clone)]
struct Provisioning {
    /// Folders holding `<folder>/` of each provisioned analyzer, in the order
    /// they are looked in.
    source_roots: Vec<PathBuf>,
    envs: Arc<AnalyzerEnvs>,
}

#[derive(Clone, Default)]
pub struct AnalyzerLocator {
    /// By analyzer id.
    overrides: HashMap<&'static str, PathBuf>,
    sidecar_directory: Option<PathBuf>,
    development_directory: Option<PathBuf>,
    /// None: provisioned analyzers cannot be found (tests).
    provisioning: Option<Provisioning>,
}

impl AnalyzerLocator {
    /// Reads the override variables now. `sidecar_directory` is the folder of
    /// the application executable.
    pub fn from_environment(sidecar_directory: Option<PathBuf>) -> Self {
        let overrides = REGISTRY
            .iter()
            .filter_map(|spec| {
                std::env::var_os(spec.override_variable)
                    .filter(|value| !value.is_empty())
                    .map(|value| (spec.id, PathBuf::from(value)))
            })
            .collect();
        let development_directory =
            cfg!(debug_assertions).then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"));
        Self {
            overrides,
            sidecar_directory,
            development_directory,
            provisioning: None,
        }
    }

    /// Provisioned analyzers get their environments from `envs`, built from
    /// the sources in `resources` (the bundle's `analyzers/` folder), or in
    /// debug builds first from the repository's `analyzers/`.
    pub fn with_provisioning(mut self, envs: Arc<AnalyzerEnvs>, resources: Option<PathBuf>) -> Self {
        let repository = cfg!(debug_assertions)
            .then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("analyzers"));
        let source_roots = repository.into_iter().chain(resources).collect();
        self.provisioning = Some(Provisioning { source_roots, envs });
        self
    }

    /// Only `path` for the analyzer `id`; for tests.
    #[cfg(test)]
    pub fn with_executable(id: &'static str, path: PathBuf) -> Self {
        Self {
            overrides: HashMap::from([(id, path)]),
            ..Self::default()
        }
    }

    /// Provisioned analyzers built by `envs` from the sources under `root`
    /// only; for tests.
    #[cfg(test)]
    pub fn with_sources(envs: Arc<AnalyzerEnvs>, root: PathBuf) -> Self {
        Self {
            provisioning: Some(Provisioning {
                source_roots: vec![root],
                envs,
            }),
            ..Self::default()
        }
    }

    /// The analyzer of `spec`, ready to run, or a message for the user
    /// saying why there is none. A provisioned analyzer without an
    /// environment gets one first, which can take minutes, once; concurrent
    /// callers wait for the same one. `path` is the PATH its toolchain is
    /// looked for in.
    pub fn resolve(
        &self,
        spec: &'static AnalyzerSpec,
        fingerprints: &Fingerprints,
        path: &OsStr,
    ) -> Result<Resolved, CoreText> {
        let unreadable = |error: io::Error| {
            core_text!(
                AnalyzerUnreadable,
                "{analyzer} cannot be read: {detail}",
                analyzer = spec.title(),
                detail = error
            )
        };
        if let Some(program) = self.overrides.get(spec.id) {
            if !program.is_file() {
                return Err(core_text!(
                    AnalyzerOverrideNotAFile,
                    "{variable} points to {path}, which is not a file. {then}",
                    variable = spec.override_variable,
                    path = program.display(),
                    then = spec.not_installed()
                ));
            }
            let mut fingerprint = fingerprints.of(program).map_err(unreadable)?;
            if let Some(source) = self.source(spec) {
                let source = analyzer_envs::source_fingerprint(&source).map_err(unreadable)?;
                fingerprint = fingerprint::of_bytes(format!("{fingerprint}+{source}").as_bytes())
                    .trim_start_matches("sha256:")
                    .to_string();
            }
            return Ok(Resolved {
                launcher: Launcher::program(program.clone()),
                fingerprint,
            });
        }
        match spec.delivery {
            Delivery::Sidecar { executable } => {
                let program = self.sidecar(executable).ok_or_else(|| spec.not_installed())?;
                let fingerprint = fingerprints.of(&program).map_err(unreadable)?;
                Ok(Resolved {
                    launcher: Launcher::program(program),
                    fingerprint,
                })
            }
            Delivery::Provisioned { runtime, .. } => {
                let (Some(provisioning), Some(source)) = (&self.provisioning, self.source(spec))
                else {
                    return Err(spec.not_installed());
                };
                provisioning.envs.ensure(spec, runtime, &source, path)
            }
        }
    }

    /// Where `prepare --tools` installs the mutation engine of `spec`, an
    /// analyzer resolved with `fingerprint`: a folder of its environment
    /// (`AnalyzerEnvs::tools_folder`), so the engine goes when the
    /// environment does; None when it needs no engine, or is not
    /// provisioned here.
    pub fn tools_folder(&self, spec: &AnalyzerSpec, fingerprint: &str) -> Option<PathBuf> {
        if !spec.mutation_tools || !spec.is_provisioned() {
            return None;
        }
        let provisioning = self.provisioning.as_ref()?;
        Some(provisioning.envs.tools_folder(spec, fingerprint))
    }

    /// The bundled executable named `executable`, if there is one.
    fn sidecar(&self, executable: &str) -> Option<PathBuf> {
        let sidecar = self.sidecar_directory.as_ref().map(|directory| {
            directory.join(format!("{executable}{}", std::env::consts::EXE_SUFFIX))
        });
        let development = self.development_directory.as_ref().and_then(|directory| {
            option_env!("TAURI_ENV_TARGET_TRIPLE").map(|triple| {
                directory.join(format!(
                    "{executable}-{triple}{}",
                    std::env::consts::EXE_SUFFIX
                ))
            })
        });
        [sidecar, development]
            .into_iter()
            .flatten()
            .find(|path| path.is_file())
    }

    /// The shipped source folder of a provisioned `spec`, if found.
    fn source(&self, spec: &AnalyzerSpec) -> Option<PathBuf> {
        let Delivery::Provisioned { folder, .. } = spec.delivery else {
            return None;
        };
        self.provisioning.as_ref()?.source_roots.iter().find_map(|root| {
            let source = root.join(folder);
            source.is_dir().then_some(source)
        })
    }
}

/// Identifies one build of an executable without running it: the SHA-256 of
/// its bytes, in hex. Analyses, checks and test runs are cached under it, so
/// an analyzer whose code changed makes its old results unused, while an
/// identical rebuild keeps them: `tauri dev` rebuilds the analyzer on every
/// start (`beforeDevCommand`), and Go writes the same bytes with a new
/// modification time.
pub fn fingerprint(executable: &Path) -> io::Result<String> {
    let digest = fingerprint::of_reader(File::open(executable)?)?;
    Ok(digest
        .strip_prefix("sha256:")
        .unwrap_or(&digest)
        .to_string())
}

/// The fingerprints of executables already hashed, so each build is read
/// once: a hash is reused while the file keeps its size and modification
/// time.
#[derive(Debug, Default)]
pub struct Fingerprints {
    known: Mutex<HashMap<PathBuf, KnownFingerprint>>,
}

#[derive(Debug, Clone)]
struct KnownFingerprint {
    length: u64,
    modified: SystemTime,
    fingerprint: String,
}

impl Fingerprints {
    pub fn of(&self, executable: &Path) -> io::Result<String> {
        let metadata = executable.metadata()?;
        let (length, modified) = (metadata.len(), metadata.modified()?);
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kept) = known.get(executable) {
            if kept.length == length && kept.modified == modified {
                return Ok(kept.fingerprint.clone());
            }
        }
        let fingerprint = fingerprint(executable)?;
        known.insert(
            executable.to_path_buf(),
            KnownFingerprint {
                length,
                modified,
                fingerprint: fingerprint.clone(),
            },
        );
        Ok(fingerprint)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunSettings {
    /// Passed to the analyzer as `--timeout`.
    pub analyzer_timeout: Duration,
    /// When the analyzer is stopped if it has not finished; a little longer
    /// than `analyzer_timeout`, so it can report its own timeout.
    pub hard_timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            analyzer_timeout: Duration::from_secs(120),
            hard_timeout: Duration::from_secs(150),
            max_output_bytes: 200 * 1024 * 1024,
        }
    }
}

const STDERR_TAIL_BYTES: usize = 16 * 1024;
/// How much of the analyzer's error output a message shows.
const MAX_ERROR_DETAIL_CHARS: usize = 1500;

/// Runs the analyzer of `spec` on `tree` with `path` as PATH. The error is a
/// message for the user.
pub fn run(
    spec: &AnalyzerSpec,
    launcher: &Launcher,
    tree: &Path,
    path: &OsStr,
    settings: &RunSettings,
) -> Result<Analysis, CoreText> {
    let stdout = invoke(spec, "analyze", launcher, tree, path, settings, &[])?;
    analysis::parse(&stdout, spec.languages, spec.title())
}

/// Runs the static checkers of `spec` on `tree` (`check`, see
/// `analyzers/protocol/checks.schema.json`), with `cache_directory`, the
/// project's build cache, as `--cache-dir`. They read the code and never
/// write in `tree`.
pub fn run_checks(
    spec: &AnalyzerSpec,
    launcher: &Launcher,
    tree: &Path,
    cache_directory: &Path,
    path: &OsStr,
    settings: &RunSettings,
) -> Result<ChecksOutput, CoreText> {
    let extra = ["--cache-dir".into(), cache_directory.into()];
    let stdout = invoke(spec, "check", launcher, tree, path, settings, &extra)?;
    runner_protocol::parse_checks(&stdout, spec.languages, spec.title())
}

/// `prepare --dir <tree> --cache-dir <cache_directory>`, then `--tools
/// <folder>` when the mutation engine is to be installed there too.
pub fn prepare_arguments(tree: &Path, cache_directory: &Path, tools: Option<&Path>) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "prepare".into(),
        "--dir".into(),
        tree.into(),
        "--cache-dir".into(),
        cache_directory.into(),
    ];
    if let Some(tools) = tools {
        arguments.push("--tools".into());
        arguments.push(tools.into());
    }
    arguments
}

/// Runs `prepare` of `spec` on `tree`, a throwaway copy, as a plain process
/// (it may use the network and runs no project code), stopped after
/// `timeout`. The error is a message for the user.
pub fn run_prepare(
    spec: &AnalyzerSpec,
    launcher: &Launcher,
    tree: &Path,
    cache_directory: &Path,
    path: &OsStr,
    timeout: Duration,
) -> Result<(), CoreText> {
    let title = spec.title();
    let mut command = launcher.command(&prepare_arguments(tree, cache_directory, None));
    command.current_dir(tree).env("PATH", path);
    let limits = Limits {
        timeout,
        max_stdout_bytes: 1024 * 1024,
        stderr_tail_bytes: STDERR_TAIL_BYTES,
    };
    let captured = process::run_captured(command, &limits).map_err(|error| not_started(spec, &error))?;
    match captured.ending {
        Ending::Exited(status) if status.success() => {
            runner_protocol::parse_prepare(&captured.stdout, title).map(|_| ())
        }
        Ending::Exited(status) => Err(core_text!(
            PrepareFailed,
            "{analyzer} could not prepare the tests ({status}):\n{detail}",
            analyzer = title,
            status = status.to_string(),
            detail = detail_text(&captured.stderr_tail)
        )),
        Ending::TimedOut => Err(core_text!(
            AnalyzerTimedOut,
            "{analyzer} did not finish within {seconds} seconds and was stopped.",
            analyzer = title,
            seconds = timeout.as_secs()
        )),
        Ending::OutputTooLarge => Err(printed_too_much(&title, 1)),
        Ending::Cancelled => Err(core_text!(
            AnalyzerStopped,
            "{analyzer} was stopped.",
            analyzer = title
        )),
    }
}

/// What `dupes` is asked: the file listing the lines the review adds
/// (`dupes.schema.json`, `$defs/changed`), outside the tree, and the size a
/// copy must reach to count.
#[derive(Debug, Clone, Copy)]
pub struct DupesRequest<'a> {
    pub changed: &'a Path,
    pub min_tokens: u32,
    pub min_lines: u32,
}

/// Runs the duplicate code search of `spec` on `tree` (`dupes`, see
/// `analyzers/protocol/dupes.schema.json`). It reads the code and never
/// writes in `tree`.
pub fn run_dupes(
    spec: &AnalyzerSpec,
    launcher: &Launcher,
    tree: &Path,
    path: &OsStr,
    settings: &RunSettings,
    request: DupesRequest<'_>,
) -> Result<duplication::DupesOutput, CoreText> {
    let extra: [OsString; 6] = [
        "--changed".into(),
        request.changed.into(),
        "--min-tokens".into(),
        request.min_tokens.to_string().into(),
        "--min-lines".into(),
        request.min_lines.to_string().into(),
    ];
    let stdout = invoke(spec, "dupes", launcher, tree, path, settings, &extra)?;
    duplication::parse(&stdout, spec.languages, spec.title())
}

/// Runs `<analyzer> <command> --dir <tree> --timeout <seconds>s`, then
/// `extra`, and returns what it printed when it succeeded.
fn invoke(
    spec: &AnalyzerSpec,
    subcommand: &str,
    launcher: &Launcher,
    tree: &Path,
    path: &OsStr,
    settings: &RunSettings,
    extra: &[OsString],
) -> Result<Vec<u8>, CoreText> {
    let title = spec.title();
    let mut arguments: Vec<OsString> = vec![
        subcommand.into(),
        "--dir".into(),
        tree.into(),
        "--timeout".into(),
        format!("{}s", settings.analyzer_timeout.as_secs().max(1)).into(),
    ];
    arguments.extend_from_slice(extra);
    let mut command = launcher.command(&arguments);
    command.current_dir(tree).env("PATH", path);
    let limits = Limits {
        timeout: settings.hard_timeout,
        max_stdout_bytes: settings.max_output_bytes,
        stderr_tail_bytes: STDERR_TAIL_BYTES,
    };
    let captured = match process::run_captured(command, &limits) {
        Ok(captured) => captured,
        Err(error) => return Err(not_started(spec, &error)),
    };
    match captured.ending {
        Ending::TimedOut => Err(core_text!(
            AnalyzerTimedOut,
            "{analyzer} did not finish within {seconds} seconds and was stopped.",
            analyzer = title,
            seconds = settings.hard_timeout.as_secs()
        )),
        Ending::OutputTooLarge => Err(printed_too_much(
            &title,
            settings.max_output_bytes / (1024 * 1024),
        )),
        Ending::Cancelled => Err(core_text!(
            AnalyzerStopped,
            "{analyzer} was stopped.",
            analyzer = title
        )),
        Ending::Exited(status) if !status.success() => Err(core_text!(
            AnalyzerFailed,
            "{analyzer} failed ({status}): {detail}",
            analyzer = title,
            status = status.to_string(),
            detail = detail_text(&captured.stderr_tail)
        )),
        Ending::Exited(_) => Ok(captured.stdout),
    }
}

/// Why the analyzer of `spec` could not be started: a sidecar that is gone
/// is not installed; anything else is said as it is.
pub fn not_started(spec: &AnalyzerSpec, error: &io::Error) -> CoreText {
    if error.kind() == io::ErrorKind::NotFound && !spec.is_provisioned() {
        spec.not_installed()
    } else {
        could_not_run(&spec.title(), error)
    }
}

/// "{analyzer} could not be run: {error}".
pub fn could_not_run(analyzer: &CoreText, error: &io::Error) -> CoreText {
    core_text!(
        AnalyzerRunFailed,
        "{analyzer} could not be run: {detail}",
        analyzer = analyzer.clone(),
        detail = error
    )
}

/// "{analyzer} printed more than {megabytes} MB and was stopped."
pub fn printed_too_much(analyzer: &CoreText, megabytes: usize) -> CoreText {
    core_text!(
        AnalyzerOutputTooLarge,
        "{analyzer} printed more than {megabytes} MB and was stopped.",
        analyzer = analyzer.clone(),
        megabytes = megabytes
    )
}

/// The end of the error output, on one line, or the core's words when it is
/// empty.
fn detail_text(stderr: &[u8]) -> CoreText {
    if String::from_utf8_lossy(stderr).trim().is_empty() {
        core_text!(AnalyzerNoDetails, "it reported no details.")
    } else {
        CoreText::raw(error_detail(stderr))
    }
}

/// The end of the error output, on one line.
pub fn error_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let detail = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        return "it reported no details.".to_string();
    }
    let length = detail.chars().count();
    if length <= MAX_ERROR_DETAIL_CHARS {
        detail
    } else {
        detail
            .chars()
            .skip(length - MAX_ERROR_DETAIL_CHARS)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer_envs::fixtures::{envs_in, write_tree};

    const NO_PATH: &str = "/difflens/test/nothing-here";

    fn go() -> &'static AnalyzerSpec {
        for_language("go").unwrap()
    }

    #[test]
    fn a_mutation_engine_goes_into_its_analyzers_environment_and_prepare_installs_it() {
        let data = tempfile::TempDir::new().unwrap();
        let (envs, _) = envs_in(data.path());
        let locator = AnalyzerLocator::with_sources(envs, data.path().join("sources"));
        let rust = for_language("rust").unwrap();
        assert_eq!(
            locator.tools_folder(rust, "abc"),
            Some(data.path().join("rust").join("abc").join("tools"))
        );
        assert!(locator.tools_folder(for_language("typescript").unwrap(), "abc").is_some());
        // Their own mutators: nothing to install.
        assert_eq!(locator.tools_folder(go(), "abc"), None);
        assert_eq!(locator.tools_folder(for_language("python").unwrap(), "abc"), None);
        assert_eq!(AnalyzerLocator::default().tools_folder(rust, "abc"), None, "not provisioned here");

        let plain = prepare_arguments(Path::new("/t"), Path::new("/c"), None);
        assert_eq!(plain, ["prepare", "--dir", "/t", "--cache-dir", "/c"].map(OsString::from));
        let tools = prepare_arguments(Path::new("/t"), Path::new("/c"), Some(Path::new("/e/tools")));
        assert_eq!(&tools[5..], ["--tools", "/e/tools"].map(OsString::from));
    }

    #[test]
    fn every_detected_language_maps_to_its_analyzer() {
        let cases = [
            ("go", "go"),
            ("python", "python"),
            ("javascript", "jsts"),
            ("typescript", "jsts"),
            ("rust", "rust"),
        ];
        for (language, id) in cases {
            assert_eq!(for_language(language).map(|spec| spec.id), Some(id), "{language}");
        }
        assert!(for_language("haskell").is_none());
        assert_eq!(go().delivery, Delivery::Sidecar { executable: "difflens-analyzer-go" });
        assert!(!go().is_provisioned());
        for id in ["python", "jsts", "rust"] {
            let spec = REGISTRY.iter().find(|spec| spec.id == id).unwrap();
            assert!(spec.is_provisioned(), "{id}");
            assert_eq!(spec.override_variable, format!("DIFFLENS_ANALYZER_{}", id.to_uppercase()));
            assert_eq!(spec.name, format!("difflens-analyzer-{id}"));
        }
    }

    #[test]
    fn languages_sharing_an_analyzer_get_it_once() {
        let languages = ["typescript", "go", "javascript", "haskell"].map(String::from);
        let ids: Vec<_> = for_languages(&languages).iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["jsts", "go"]);
    }

    #[test]
    fn locating_prefers_the_override_then_the_sidecar() {
        let dir = tempfile::TempDir::new().unwrap();
        let sidecar = dir.path().join(format!(
            "difflens-analyzer-go{}",
            std::env::consts::EXE_SUFFIX
        ));
        let custom = dir.path().join("custom analyzer");
        let locator = AnalyzerLocator {
            sidecar_directory: Some(dir.path().to_path_buf()),
            ..AnalyzerLocator::default()
        };
        let fingerprints = Fingerprints::default();
        let resolve = |locator: &AnalyzerLocator| {
            locator.resolve(go(), &fingerprints, OsStr::new(NO_PATH))
        };

        let error = resolve(&locator).unwrap_err();
        assert_eq!(
            error,
            "The Go analyzer is not installed; run pnpm analyzers:build."
        );

        std::fs::write(&sidecar, "sidecar").unwrap();
        let found = resolve(&locator).unwrap();
        assert_eq!(found.launcher, Launcher::program(sidecar.clone()));
        assert_eq!(found.fingerprint, fingerprint(&sidecar).unwrap());

        let overridden = AnalyzerLocator {
            overrides: HashMap::from([("go", custom.clone())]),
            ..locator.clone()
        };
        let error = resolve(&overridden).unwrap_err();
        assert!(error.contains("DIFFLENS_ANALYZER_GO points to"), "{error}");
        std::fs::write(&custom, "custom").unwrap();
        assert_eq!(resolve(&overridden).unwrap().launcher, Launcher::program(custom));
    }

    #[test]
    fn a_provisioned_analyzer_without_its_source_is_missing_from_the_installation() {
        let python = for_language("python").unwrap();
        let error = AnalyzerLocator::default()
            .resolve(python, &Fingerprints::default(), OsStr::new(NO_PATH))
            .unwrap_err();
        assert_eq!(
            error,
            "The Python analyzer is missing from this installation (its source folder analyzers/python was not found)."
        );

        let data = tempfile::TempDir::new().unwrap();
        let (envs, _) = envs_in(data.path());
        let locator = AnalyzerLocator::with_sources(envs, data.path().join("no sources here"));
        let error = locator
            .resolve(python, &Fingerprints::default(), OsStr::new(NO_PATH))
            .unwrap_err();
        assert!(error.contains("is missing from this installation"), "{error}");
    }

    #[test]
    fn an_override_of_a_provisioned_analyzer_runs_as_it_is_and_follows_its_source() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("analyzers");
        write_tree(&sources.join("rust"), &[("Cargo.toml", "[package]\n")]);
        let wrapper = dir.path().join("run-rust-analyzer");
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        let (envs, _) = envs_in(&dir.path().join("data"));
        let locator = AnalyzerLocator {
            overrides: HashMap::from([("rust", wrapper.clone())]),
            ..AnalyzerLocator::with_sources(envs, sources.clone())
        };
        let rust = for_language("rust").unwrap();
        let fingerprints = Fingerprints::default();

        // No toolchain is needed: the override is a ready program.
        let first = locator.resolve(rust, &fingerprints, OsStr::new(NO_PATH)).unwrap();
        assert_eq!(first.launcher, Launcher::program(wrapper.clone()));
        assert_ne!(first.fingerprint, fingerprint(&wrapper).unwrap());
        assert!(first.fingerprint.chars().all(|c| c.is_ascii_hexdigit()), "{}", first.fingerprint);

        std::fs::write(sources.join("rust").join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let edited = locator.resolve(rust, &fingerprints, OsStr::new(NO_PATH)).unwrap();
        assert_ne!(edited.fingerprint, first.fingerprint);
    }

    #[test]
    fn a_launcher_puts_its_leading_arguments_and_environment_first() {
        let launcher = Launcher {
            program: PathBuf::from("/env/venv/bin/python"),
            leading: vec!["-I".into(), "-m".into(), "difflens_analyzer".into()],
            env: vec![("PYTHONUTF8".into(), "1".into())],
        };
        let command = launcher.command(&["analyze".into(), "--dir".into(), "/tree".into()]);
        assert_eq!(command.get_program(), "/env/venv/bin/python");
        let arguments: Vec<_> = command.get_args().collect();
        assert_eq!(arguments, ["-I", "-m", "difflens_analyzer", "analyze", "--dir", "/tree"]);
        let env: Vec<_> = command.get_envs().collect();
        assert_eq!(env, [(OsStr::new("PYTHONUTF8"), Some(OsStr::new("1")))]);

        let policy = sandbox::Policy {
            run_directory: PathBuf::from("/nonexistent-run"),
            temporary: PathBuf::from("/nonexistent-tmp"),
            writable: Vec::new(),
        };
        let command = launcher.sandboxed(Path::new(sandbox::SANDBOX_EXEC), &policy, &["test".into()]);
        assert_eq!(command.get_program(), sandbox::SANDBOX_EXEC);
        let arguments: Vec<_> = command.get_args().collect();
        let after = arguments.iter().position(|argument| *argument == "--").unwrap();
        assert_eq!(
            &arguments[after..],
            ["--", "/env/venv/bin/python", "-I", "-m", "difflens_analyzer", "test"]
        );
        assert_eq!(command.get_envs().count(), 1);
    }

    #[test]
    fn the_fingerprint_changes_when_the_executable_does() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("analyzer");
        std::fs::write(&path, "one").unwrap();
        let first = fingerprint(&path).unwrap();
        std::fs::write(&path, "longer").unwrap();
        assert_ne!(fingerprint(&path).unwrap(), first);
        // Same size, other bytes.
        std::fs::write(&path, "lunger").unwrap();
        let other = fingerprint(&path).unwrap();
        assert_ne!(other, first);
        assert!(fingerprint(&dir.path().join("missing")).is_err());
        // A folder name on every system.
        assert!(other.chars().all(|c| c.is_ascii_hexdigit()), "{other}");
    }

    #[test]
    fn an_identical_rebuild_keeps_the_fingerprint() {
        // `tauri dev` rebuilds the analyzer on every start: the same bytes,
        // a new modification time.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("analyzer");
        std::fs::write(&path, "build").unwrap();
        let fingerprints = Fingerprints::default();
        let first = fingerprints.of(&path).unwrap();

        std::fs::write(&path, "build").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();

        assert_eq!(fingerprints.of(&path).unwrap(), first);
        assert_eq!(Fingerprints::default().of(&path).unwrap(), first);
        assert_eq!(fingerprint(&path).unwrap(), first);
    }

    #[test]
    fn a_rebuild_with_other_code_changes_the_fingerprint_even_when_remembered() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("analyzer");
        std::fs::write(&path, "build one").unwrap();
        let fingerprints = Fingerprints::default();
        let first = fingerprints.of(&path).unwrap();

        std::fs::write(&path, "build two").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();

        assert_ne!(fingerprints.of(&path).unwrap(), first);
        assert!(fingerprints.of(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn error_detail_keeps_the_end() {
        let long = format!("{}\nreal reason", "noise ".repeat(400));
        assert!(error_detail(long.as_bytes()).ends_with("real reason"));
        assert_eq!(error_detail(b"\n"), "it reported no details.");
    }
}
