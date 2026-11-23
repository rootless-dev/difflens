//! The command line every DiffLens analyzer implements (see README.md):
//!
//! ```text
//! difflens-analyzer-rust analyze --dir <source tree> [--timeout 120s]
//! difflens-analyzer-rust prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>]
//!     [--tools <folder>]
//! difflens-analyzer-rust test --dir <source tree> [--timeout 10m] [--cache-dir <folder>]
//!     [--offline] [--no-race] [--no-cover] [--selection <file>] [--progress <file>]
//! difflens-analyzer-rust check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]
//! difflens-analyzer-rust dupes --dir <source tree> --changed <file> [--timeout 120s]
//!     [--min-tokens 50] [--min-lines 6]
//! difflens-analyzer-rust mutate --dir <source tree> --changed <file> --coverage <file>
//!     [--timeout 30m] [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200]
//!     [--jobs <n>] [--min-mutant-timeout 10s] [--progress <file>] [--offline]
//! difflens-analyzer-rust --version
//! ```
//!
//! `--cache-dir` is the core's folder for the project, outside the tree and
//! kept between runs: `test`, `check` and `mutate` keep their build output
//! there (see `cache`), `mutate` its results too; `prepare` has nothing to
//! keep and ignores it. `--tools` is where `prepare` installs the mutation
//! engine and `mutate` runs it from (see `engine`).
//!
//! Each command's work is in its module: `analyze`, `checks`, `testrun`
//! (prepare and test), `dupes`, `mutation`. This one parses the command
//! line, keeps the exit codes and prints the single document on stdout.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::log::Logger;
use crate::protocol::{self, Changed, CoverageFile, Prepare, Selection};

pub const EXIT_OK: i32 = 0;
/// The command itself failed; stderr says why.
pub const EXIT_FAILED: i32 = 1;
/// Bad command line, or a selection or changed file that cannot be read.
pub const EXIT_USAGE: i32 = 2;

/// Defaults of `dupes --min-tokens` and `--min-lines`.
const DEFAULT_MIN_TOKENS: usize = 50;
const DEFAULT_MIN_LINES: u32 = 6;
/// Defaults of `mutate --max-mutants` and `--min-mutant-timeout`.
const DEFAULT_MAX_MUTANTS: usize = 200;
const DEFAULT_MIN_MUTANT_TIMEOUT: Duration = Duration::from_secs(10);

fn usage() -> String {
    let name = protocol::NAME;
    format!(
        "usage: {name} analyze --dir <source tree> [--timeout 120s]\n       \
         {name} prepare --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--tools <folder>]\n       \
         {name} test --dir <source tree> [--timeout 10m] [--cache-dir <folder>] [--offline] [--no-race]\n           \
         [--no-cover] [--selection <file>] [--progress <file>]\n       \
         {name} check --dir <source tree> [--timeout 5m] [--cache-dir <folder>]\n       \
         {name} dupes --dir <source tree> --changed <file> [--timeout 120s]\n           \
         [--min-tokens 50] [--min-lines 6]\n       \
         {name} mutate --dir <source tree> --changed <file> --coverage <file> [--timeout 30m]\n           \
         [--cache-dir <folder>] [--tools <folder>] [--max-mutants 200] [--jobs <n>]\n           \
         [--min-mutant-timeout 10s] [--progress <file>] [--offline]\n       \
         {name} --version\n"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Analyze,
    Prepare,
    Test,
    Check,
    Dupes,
    Mutate,
}

impl Command {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "analyze" => Self::Analyze,
            "prepare" => Self::Prepare,
            "test" => Self::Test,
            "check" => Self::Check,
            "dupes" => Self::Dupes,
            "mutate" => Self::Mutate,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Analyze => "analyze",
            Self::Prepare => "prepare",
            Self::Test => "test",
            Self::Check => "check",
            Self::Dupes => "dupes",
            Self::Mutate => "mutate",
        }
    }

    fn default_timeout(self) -> &'static str {
        match self {
            Self::Analyze | Self::Dupes => "120s",
            Self::Prepare | Self::Test => "10m",
            Self::Check => "5m",
            Self::Mutate => "30m",
        }
    }
}

/// The flags of one command, as given.
#[derive(Debug, Default)]
pub struct Options {
    pub dir: PathBuf,
    pub timeout: Duration,
    pub offline: bool,
    pub no_race: bool,
    pub no_cover: bool,
    pub selection: Option<PathBuf>,
    pub progress: Option<PathBuf>,
    /// Absolute; None when not given or given empty.
    pub cache_dir: Option<PathBuf>,
    pub changed: Option<PathBuf>,
    pub min_tokens: usize,
    pub min_lines: u32,
    pub coverage: Option<PathBuf>,
    /// Absolute; None when not given or given empty.
    pub tools: Option<PathBuf>,
    pub max_mutants: usize,
    /// None: the default, half the CPUs.
    pub jobs: Option<usize>,
    pub min_mutant_timeout: Duration,
}

/// A folder flag's value: absolute, None when empty.
fn folder(command: &str, flag: &str, value: &str) -> Result<Option<PathBuf>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    std::path::absolute(value)
        .map(Some)
        .map_err(|error| format!("{command}: --{flag}: {error}"))
}

/// A `--min-tokens` or `--min-lines` value: a whole number, at least 1.
fn parse_minimum<T: std::str::FromStr + PartialOrd + From<u8>>(
    command: &str,
    flag: &str,
    text: &str,
) -> Result<T, String> {
    text.parse::<T>()
        .ok()
        .filter(|value| *value >= T::from(1))
        .ok_or_else(|| {
            format!("{command}: --{flag} must be a whole number of at least 1, not {text:?}")
        })
}

/// Parses Go-style flags: `--name value`, `--name=value`, one or two dashes.
fn parse_options(command: Command, args: &[String]) -> Result<Options, String> {
    let name = command.name();
    let mut options = Options {
        min_tokens: DEFAULT_MIN_TOKENS,
        min_lines: DEFAULT_MIN_LINES,
        max_mutants: DEFAULT_MAX_MUTANTS,
        min_mutant_timeout: DEFAULT_MIN_MUTANT_TIMEOUT,
        ..Options::default()
    };
    let mut dir = String::new();
    let mut timeout = command.default_timeout().to_string();
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        let Some(flag) = argument
            .strip_prefix("--")
            .or_else(|| argument.strip_prefix('-'))
        else {
            return Err(format!("{name}: unexpected argument {argument:?}"));
        };
        let (flag, inline) = match flag.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (flag, None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| arguments.next().cloned())
                .ok_or_else(|| format!("{name}: flag needs an argument: -{flag}"))
        };
        match (flag, command) {
            ("dir", _) => dir = value()?,
            ("timeout", _) => timeout = value()?,
            ("selection", Command::Test) => options.selection = Some(PathBuf::from(value()?)),
            ("progress", Command::Test | Command::Mutate) => {
                options.progress = Some(PathBuf::from(value()?));
            }
            ("offline", Command::Test | Command::Mutate) => options.offline = true,
            ("no-race", Command::Test) => options.no_race = true,
            ("no-cover", Command::Test) => options.no_cover = true,
            ("cache-dir", Command::Prepare | Command::Test | Command::Check | Command::Mutate) => {
                options.cache_dir = folder(name, flag, &value()?)?;
            }
            ("tools", Command::Prepare | Command::Mutate) => {
                options.tools = folder(name, flag, &value()?)?;
            }
            ("changed", Command::Dupes | Command::Mutate) => {
                options.changed = Some(PathBuf::from(value()?));
            }
            ("min-tokens", Command::Dupes) => {
                options.min_tokens = parse_minimum(name, flag, &value()?)?;
            }
            ("min-lines", Command::Dupes) => {
                options.min_lines = parse_minimum(name, flag, &value()?)?;
            }
            ("coverage", Command::Mutate) => options.coverage = Some(PathBuf::from(value()?)),
            ("max-mutants", Command::Mutate) => {
                options.max_mutants = parse_minimum(name, flag, &value()?)?;
            }
            ("jobs", Command::Mutate) => {
                options.jobs = Some(parse_minimum(name, flag, &value()?)?);
            }
            ("min-mutant-timeout", Command::Mutate) => {
                let text = value()?;
                options.min_mutant_timeout = parse_duration(&text)
                    .filter(|duration| !duration.is_zero())
                    .ok_or_else(|| {
                        format!("{name}: --{flag} must be a positive duration, not {text:?}")
                    })?;
            }
            _ => return Err(format!("{name}: flag provided but not defined: -{flag}")),
        }
    }
    if matches!(command, Command::Dupes | Command::Mutate) && options.changed.is_none() {
        return Err(format!("{name}: --changed is required"));
    }
    if command == Command::Mutate && options.coverage.is_none() {
        return Err(format!("{name}: --coverage is required"));
    }
    options.timeout =
        parse_duration(&timeout).ok_or_else(|| format!("{name}: invalid duration {timeout:?}"))?;
    if dir.is_empty() || options.timeout.is_zero() {
        return Err(format!(
            "{name}: --dir is required, --timeout must be positive and no positional arguments are accepted"
        ));
    }
    options.dir = PathBuf::from(dir);
    Ok(options)
}

/// A Go duration such as "120s" or "1m30s", the form `--timeout` takes.
pub fn parse_duration(text: &str) -> Option<Duration> {
    if text.is_empty() {
        return None;
    }
    let mut rest = text;
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number: f64 = rest[..number_end].parse().ok()?;
        rest = &rest[number_end..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit) && !(*unit == "m" && rest.starts_with("ms")))?;
        rest = &rest[unit.len()..];
        seconds += number * scale;
    }
    Some(Duration::from_secs_f64(seconds))
}

/// Why a command failed: its command line (exit 2) or its work (exit 1).
enum Failure {
    Usage(String),
    Failed(String),
}

/// The source tree to read; a missing one fails the command (exit 1).
fn tree(dir: &Path) -> Result<PathBuf, Failure> {
    if !dir.is_dir() {
        return Err(Failure::Failed(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    dir.canonicalize()
        .map_err(|error| Failure::Failed(error.to_string()))
}

/// Functions and calls of the tree (analysis.schema.json). Must not write
/// inside --dir.
fn analyze<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let started = Instant::now();
    let analysis = crate::analyze::analyze(&dir, started + options.timeout)
        .map_err(|error| Failure::Failed(error.to_string()))?;
    log.info(
        "analysis done",
        &[
            ("functions", json!(analysis.functions.len())),
            ("calls", json!(analysis.calls.len())),
            ("elapsed", json!(format!("{:?}", started.elapsed()))),
        ],
    );
    to_value(&analysis)
}

/// Fetches what the tests need, so the test step can run offline; never
/// runs project code. --cache-dir is accepted and not needed: the test
/// step writes there only, and the core opens it itself. With --tools, it
/// installs the mutation engine there when it is missing.
fn prepare<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let started = Instant::now();
    let writable =
        crate::testrun::prepare(&dir, started + options.timeout).map_err(Failure::Failed)?;
    if let Some(tools) = &options.tools {
        crate::engine::install(tools, started + options.timeout).map_err(Failure::Failed)?;
    }
    log.info(
        "prepare done",
        &[("elapsed", json!(format!("{:?}", started.elapsed())))],
    );
    to_value(&Prepare {
        protocol: protocol::PROTOCOL,
        writable,
    })
}

/// Runs the project's tests once (tests.schema.json). Failing tests are
/// results, not a failure.
fn run_tests<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let selection = read_selection(options.selection.as_deref())?;
    let started = Instant::now();
    let partial = selection.is_some();
    let run = crate::testrun::run(
        &dir,
        &crate::testrun::Options {
            timeout: options.timeout,
            offline: options.offline,
            race: !options.no_race,
            cover: !options.no_cover,
            selection,
            progress: options.progress.clone(),
            cache_dir: options.cache_dir.clone(),
        },
    )
    .map_err(Failure::Failed)?;
    log.info(
        "test run done",
        &[
            ("tests", json!(run.tests.len())),
            ("packages", json!(run.packages.len())),
            ("partial", json!(partial)),
            ("elapsed", json!(format!("{:?}", started.elapsed()))),
        ],
    );
    to_value(&run)
}

/// Clippy on the tree (checks.schema.json), in a sandboxed copy.
fn check<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let started = Instant::now();
    let run = crate::checks::check(
        &dir,
        started + options.timeout,
        options.cache_dir.as_deref(),
    )
    .map_err(|error| Failure::Failed(error.to_string()))?;
    log.info(
        "checks done",
        &[
            (
                "status",
                json!(run.checks.first().map(|check| check.status.clone())),
            ),
            ("elapsed", json!(format!("{:?}", started.elapsed()))),
        ],
    );
    to_value(&run)
}

/// Duplicated code the review adds (dupes.schema.json). Reads the code only.
fn dupes<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let changed = read_changed("dupes", options.changed.as_deref().unwrap_or(Path::new("")))?;
    let started = Instant::now();
    let run = crate::dupes::dupes(
        &dir,
        &changed,
        options.min_tokens,
        options.min_lines,
        started + options.timeout,
    )
    .map_err(|error| Failure::Failed(error.to_string()))?;
    log.info(
        "dupes done",
        &[
            ("duplicates", json!(run.duplicates.len())),
            ("diagnostics", json!(run.diagnostics.len())),
            ("elapsed", json!(format!("{:?}", started.elapsed()))),
        ],
    );
    to_value(&run)
}

/// Mutation testing of the lines the review adds that the tests run
/// (mutation.schema.json). Surviving mutants are results, not a failure.
fn mutate<W: Write>(options: &Options, log: &mut Logger<W>) -> Result<Value, Failure> {
    let dir = tree(&options.dir)?;
    let changed = read_changed(
        "mutate",
        options.changed.as_deref().unwrap_or(Path::new("")),
    )?;
    let coverage = read_coverage(options.coverage.as_deref().unwrap_or(Path::new("")))?;
    let started = Instant::now();
    let run = crate::mutation::mutate(
        &dir,
        &changed,
        &coverage,
        &crate::mutation::Options {
            timeout: options.timeout,
            offline: options.offline,
            cache_dir: options.cache_dir.clone(),
            tools: options.tools.clone(),
            max_mutants: options.max_mutants,
            jobs: options.jobs.unwrap_or_else(crate::mutation::default_jobs),
            min_mutant_timeout: options.min_mutant_timeout,
            progress: options.progress.clone(),
        },
    )
    .map_err(Failure::Failed)?;
    log.info(
        "mutate done",
        &[
            ("candidates", json!(run.summary.candidates)),
            ("mutants", json!(run.mutants.len())),
            ("survived", json!(run.summary.survived)),
            ("cached", json!(run.summary.cached)),
            ("diagnostics", json!(run.diagnostics.len())),
            ("elapsed", json!(format!("{:?}", started.elapsed()))),
        ],
    );
    to_value(&run)
}

/// The `--coverage` file of `mutate` (mutation.schema.json
/// `$defs/coverage`).
fn read_coverage(file: &Path) -> Result<CoverageFile, Failure> {
    let invalid =
        |why: String| Failure::Usage(format!("mutate: cannot read the coverage file: {why}"));
    let text = std::fs::read_to_string(file).map_err(|error| invalid(error.to_string()))?;
    let coverage: CoverageFile =
        serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?;
    if coverage.protocol != protocol::PROTOCOL {
        return Err(invalid(format!(
            "protocol {} is not {}",
            coverage.protocol,
            protocol::PROTOCOL
        )));
    }
    if coverage
        .blocks
        .iter()
        .any(|block| block.start_line == 0 || block.end_line == 0)
    {
        return Err(invalid("lines start at 1".to_string()));
    }
    Ok(coverage)
}

/// The `--changed` file: the lines the review adds, per file
/// (dupes.schema.json `$defs/changed`).
fn read_changed(command: &str, file: &Path) -> Result<Changed, Failure> {
    let invalid =
        |why: String| Failure::Usage(format!("{command}: cannot read the changed file: {why}"));
    let text = std::fs::read_to_string(file).map_err(|error| invalid(error.to_string()))?;
    let changed: Changed =
        serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))?;
    if changed.protocol != protocol::PROTOCOL {
        return Err(invalid(format!(
            "protocol {} is not {}",
            changed.protocol,
            protocol::PROTOCOL
        )));
    }
    for file in &changed.files {
        if file.added.iter().flatten().any(|line| *line == 0) {
            return Err(invalid(format!("lines of {} start at 1", file.path)));
        }
    }
    Ok(changed)
}

/// The `--selection` file; none selects everything.
fn read_selection(file: Option<&Path>) -> Result<Option<Selection>, Failure> {
    let Some(file) = file else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(file)
        .map_err(|error| Failure::Usage(format!("test: cannot read the selection: {error}")))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| Failure::Usage(format!("test: cannot read the selection: {error}")))
}

fn to_value(document: &impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(document).map_err(|error| Failure::Failed(error.to_string()))
}

/// Runs `args` (without the program name) and returns the exit code.
pub fn run(args: &[String], stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    if let Some(first) = args.first() {
        if ["--version", "-version", "version"].contains(&first.as_str()) {
            let _ = writeln!(stdout, "{} {}", protocol::NAME, protocol::VERSION);
            return EXIT_OK;
        }
    }
    let Some(command) = args.first().and_then(|name| Command::parse(name)) else {
        let _ = write!(stderr, "{}", usage());
        return EXIT_USAGE;
    };
    let options = match parse_options(command, &args[1..]) {
        Ok(options) => options,
        Err(message) => {
            let _ = writeln!(stderr, "{message}");
            return EXIT_USAGE;
        }
    };
    let level = std::env::var("DIFFLENS_LOG_LEVEL").unwrap_or_default();
    let mut log = Logger::new(&mut *stderr, protocol::NAME, &level);
    let result = match command {
        Command::Analyze => analyze(&options, &mut log),
        Command::Prepare => prepare(&options, &mut log),
        Command::Test => run_tests(&options, &mut log),
        Command::Check => check(&options, &mut log),
        Command::Dupes => dupes(&options, &mut log),
        Command::Mutate => mutate(&options, &mut log),
    };
    match result {
        Ok(document) => {
            if writeln!(stdout, "{document}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return EXIT_FAILED;
            }
            EXIT_OK
        }
        Err(Failure::Usage(message)) => {
            let _ = writeln!(stderr, "{message}");
            EXIT_USAGE
        }
        Err(Failure::Failed(message)) => {
            log.error(
                &format!("{} failed", command.name()),
                &[
                    ("dir", json!(options.dir.display().to_string())),
                    ("error", json!(message)),
                ],
            );
            EXIT_FAILED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = run(&args, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    #[test]
    fn durations_read_as_go_writes_them() {
        assert_eq!(parse_duration("120s"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("1.5h"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("10x"), None);
    }

    #[test]
    fn every_command_prints_one_document_of_its_kind() {
        let (folder, tree) = crate::testing::TempDir::with_fixture("failing");
        let dir = tree.to_str().unwrap();
        let cache_dir = folder.0.join("cache");
        let cache_dir = cache_dir.to_str().unwrap();
        for (command, key) in [
            ("analyze", "functions"),
            ("check", "checks"),
            ("prepare", "writable"),
            ("test", "tests"),
        ] {
            let mut args = vec![command, "--dir", dir, "--timeout", "60s"];
            if command != "analyze" {
                args.extend(["--cache-dir", cache_dir]);
            }
            let (code, stdout, _) = run_with(&args);
            assert_eq!(code, EXIT_OK, "{command}");
            let document: Value = serde_json::from_str(stdout.trim()).unwrap();
            assert_eq!(document["protocol"], 1, "{command}");
            assert!(document.get(key).is_some(), "{command}: {stdout}");
        }
    }

    #[test]
    fn the_cache_folder_is_taken_by_prepare_test_and_check_only() {
        let args =
            |list: &[&str]| -> Vec<String> { list.iter().map(|arg| arg.to_string()).collect() };
        for command in [Command::Prepare, Command::Test, Command::Check] {
            for given in [
                args(&["--dir", "/t", "--cache-dir", "/c/rust"]),
                args(&["-dir=/t", "-cache-dir=/c/rust"]),
            ] {
                let options = parse_options(command, &given).unwrap();
                assert_eq!(
                    options.cache_dir.as_deref(),
                    Some(Path::new("/c/rust")),
                    "{command:?}"
                );
            }
            let relative =
                parse_options(command, &args(&["--dir", "/t", "--cache-dir", "c"])).unwrap();
            assert_eq!(
                relative.cache_dir,
                Some(std::env::current_dir().unwrap().join("c"))
            );
            let empty = parse_options(command, &args(&["--dir", "/t", "--cache-dir="])).unwrap();
            assert_eq!(empty.cache_dir, None);
            assert!(parse_options(command, &args(&["--dir", "/t", "--cache-dir"])).is_err());
        }
        assert_eq!(
            parse_options(Command::Test, &args(&["--dir", "/t"]))
                .unwrap()
                .cache_dir,
            None
        );
        assert!(parse_options(
            Command::Analyze,
            &args(&["--dir", "/t", "--cache-dir", "/c"])
        )
        .is_err());
    }

    #[test]
    fn a_bad_command_line_exits_2_and_a_missing_tree_exits_1() {
        assert_eq!(run_with(&[]).0, EXIT_USAGE);
        assert_eq!(run_with(&["bogus"]).0, EXIT_USAGE);
        assert_eq!(run_with(&["analyze"]).0, EXIT_USAGE);
        assert_eq!(
            run_with(&["analyze", "--dir", "/tmp", "--offline"]).0,
            EXIT_USAGE
        );
        assert_eq!(
            run_with(&["analyze", "--dir", "/tmp", "extra"]).0,
            EXIT_USAGE
        );
        assert_eq!(
            run_with(&["analyze", "--dir=/definitely/not/here"]).0,
            EXIT_FAILED
        );
        let (code, stdout, _) = run_with(&["--version"]);
        assert_eq!(
            (code, stdout.as_str()),
            (EXIT_OK, "difflens-analyzer-rust 0.0.0\n")
        );
    }

    #[test]
    fn dupes_needs_a_valid_changed_file_and_minimums_of_at_least_1() {
        let tree = crate::testing::testdata("dupes");
        let dir = tree.to_str().unwrap();
        let changed = tree.join("changed.json");
        let changed = changed.to_str().unwrap();
        let folder = crate::testing::TempDir::new();
        let file = |name: &str, text: &str| {
            let path = folder.0.join(name);
            std::fs::write(&path, text).unwrap();
            path.to_str().unwrap().to_string()
        };
        let unknown = file("unknown.json", r#"{"protocol":1,"files":[],"extra":1}"#);
        let protocol = file("protocol.json", r#"{"protocol":2,"files":[]}"#);
        let zero = file(
            "zero.json",
            r#"{"protocol":1,"files":[{"path":"a.rs","added":[[0,3]]}]}"#,
        );
        let triple = file(
            "triple.json",
            r#"{"protocol":1,"files":[{"path":"a","added":[[1,2,3]]}]}"#,
        );
        let missing = folder.0.join("missing.json");
        let missing = missing.to_str().unwrap();
        let with_changed = |changed: &str, extra: &[&str]| {
            let mut args = vec!["dupes".to_string(), "--dir".to_string(), dir.to_string()];
            args.extend(["--changed".to_string(), changed.to_string()]);
            args.extend(extra.iter().map(|arg| arg.to_string()));
            args
        };
        let to_strings = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect();
        for (args, why) in [
            (to_strings(&["dupes", "--dir", dir]), "no --changed"),
            (with_changed(missing, &[]), "missing file"),
            (with_changed(&unknown, &[]), "unknown field"),
            (with_changed(&protocol, &[]), "protocol 2"),
            (with_changed(&zero, &[]), "line 0"),
            (with_changed(&triple, &[]), "three bounds"),
            (with_changed(changed, &["--min-tokens", "0"]), "no tokens"),
            (with_changed(changed, &["--min-lines=x"]), "not a number"),
            (
                to_strings(&["analyze", "--dir", dir, "--changed", changed]),
                "a flag of dupes only",
            ),
        ] {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let (code, stdout, stderr) = run_with(&args);
            assert_eq!(code, EXIT_USAGE, "{why}: {stderr}");
            assert!(stdout.is_empty(), "{why}");
        }
        assert_eq!(
            run_with(&["dupes", "--dir=/definitely/not/here", "--changed", changed]).0,
            EXIT_FAILED
        );
    }

    #[test]
    fn the_changed_file_may_carry_an_exclude_that_mutate_ignores() {
        let folder = crate::testing::TempDir::new();
        let file = |name: &str, text: &str| {
            let path = folder.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        };
        let with_exclude = file(
            "exclude.json",
            r#"{"protocol":1,"exclude":{"dirs":["locales"],"extensions":[".po"]},"files":[{"path":"src/lib.rs","added":[[1,2]]}]}"#,
        );
        for command in ["mutate", "dupes"] {
            let Ok(changed) = read_changed(command, &with_exclude) else {
                panic!("{command} rejects exclude");
            };
            assert_eq!(changed.files.len(), 1, "{command}");
            assert!(changed.exclude.is_some(), "{command}");
        }
        let without = file("plain.json", r#"{"protocol":1,"files":[]}"#);
        let Ok(changed) = read_changed("mutate", &without) else {
            panic!("mutate rejects a file without exclude");
        };
        assert!(changed.exclude.is_none());
        for (name, text) in [
            (
                "half.json",
                r#"{"protocol":1,"exclude":{"dirs":[]},"files":[]}"#,
            ),
            (
                "extra.json",
                r#"{"protocol":1,"exclude":{"dirs":[],"extensions":[],"names":[]},"files":[]}"#,
            ),
        ] {
            assert!(read_changed("mutate", &file(name, text)).is_err(), "{name}");
        }
    }

    #[test]
    fn mutate_reads_its_flags_and_needs_valid_files_and_the_engine() {
        let tree = crate::testing::testdata("mutation");
        let dir = tree.to_str().unwrap();
        let changed = tree.join("changed.json");
        let changed = changed.to_str().unwrap();
        let coverage = tree.join("coverage.json");
        let coverage = coverage.to_str().unwrap();
        let args =
            |list: &[&str]| -> Vec<String> { list.iter().map(|arg| arg.to_string()).collect() };
        let base = ["--dir", dir, "--changed", changed, "--coverage", coverage];
        let options = parse_options(Command::Mutate, &args(&base)).unwrap();
        assert_eq!(
            (
                options.max_mutants,
                options.jobs,
                options.min_mutant_timeout,
                options.timeout,
                options.offline,
                options.tools.clone(),
            ),
            (
                200,
                None,
                Duration::from_secs(10),
                Duration::from_secs(1800),
                false,
                None
            )
        );
        let mut given = base.to_vec();
        given.extend([
            "--max-mutants=3",
            "--jobs",
            "2",
            "--min-mutant-timeout",
            "1m",
            "--tools",
            "/t/tools",
            "--cache-dir",
            "/c",
            "--progress",
            "/p.json",
            "--offline",
        ]);
        let options = parse_options(Command::Mutate, &args(&given)).unwrap();
        assert_eq!(
            (
                options.max_mutants,
                options.jobs,
                options.min_mutant_timeout,
                options.tools.as_deref(),
                options.cache_dir.as_deref(),
                options.offline,
            ),
            (
                3,
                Some(2),
                Duration::from_secs(60),
                Some(Path::new("/t/tools")),
                Some(Path::new("/c")),
                true
            )
        );
        let folder = crate::testing::TempDir::new();
        let file = |name: &str, text: &str| {
            let path = folder.0.join(name);
            std::fs::write(&path, text).unwrap();
            path.to_str().unwrap().to_string()
        };
        let unknown = file("unknown.json", r#"{"protocol":1,"blocks":[],"extra":1}"#);
        let protocol = file("protocol.json", r#"{"protocol":2,"blocks":[]}"#);
        let zero = file(
            "zero.json",
            r#"{"protocol":1,"blocks":[{"file":"a.rs","startLine":0,"endLine":1,"statements":1,"count":1}]}"#,
        );
        let tools = folder.0.join("tools");
        let tools = tools.to_str().unwrap();
        for (extra, why) in [
            (vec!["--coverage", unknown.as_str()], "unknown field"),
            (vec!["--coverage", protocol.as_str()], "protocol 2"),
            (vec!["--coverage", zero.as_str()], "line 0"),
            (vec!["--max-mutants", "0"], "no mutants"),
            (vec!["--jobs", "0"], "no jobs"),
            (vec!["--min-mutant-timeout", "0s"], "no time"),
            (vec!["--min-mutant-timeout", "10"], "not a duration"),
            (vec!["--min-tokens", "3"], "a flag of dupes only"),
        ] {
            let mut list = vec!["mutate"];
            list.extend(base);
            list.extend(extra);
            list.extend(["--tools", tools]);
            let (code, stdout, stderr) = run_with(&list);
            assert_eq!(code, EXIT_USAGE, "{why}: {stderr}");
            assert!(stdout.is_empty(), "{why}");
        }
        for missing in ["--changed", "--coverage"] {
            let list: Vec<&str> = ["mutate"]
                .into_iter()
                .chain(
                    base.chunks(2)
                        .filter(|pair| pair[0] != missing)
                        .flatten()
                        .copied(),
                )
                .collect();
            assert_eq!(run_with(&list).0, EXIT_USAGE, "{missing}");
        }
        // No engine: the command fails and says how to get one.
        let mut list = vec!["mutate"];
        list.extend(base);
        let (code, stdout, stderr) = run_with(&list);
        assert_eq!(code, EXIT_FAILED);
        assert!(
            stdout.is_empty() && stderr.contains("mutate needs --tools"),
            "{stderr}"
        );
        list.extend(["--tools", tools]);
        let (code, _, stderr) = run_with(&list);
        assert_eq!(code, EXIT_FAILED);
        assert!(stderr.contains("run `prepare --tools"), "{stderr}");
        assert!(parse_options(Command::Test, &args(&["--dir", "/t", "--tools", "/x"])).is_err());
    }

    #[test]
    fn dupes_prints_one_document_and_its_minimums_change_it() {
        let tree = crate::testing::testdata("dupes");
        let dir = tree.to_str().unwrap();
        let changed = tree.join("changed.json");
        let changed = changed.to_str().unwrap();
        let groups = |extra: &[&str]| {
            let mut args = vec!["dupes", "--dir", dir, "--changed", changed];
            args.extend(["--timeout", "60s"]);
            args.extend(extra);
            let (code, stdout, stderr) = run_with(&args);
            assert_eq!(code, EXIT_OK, "{stderr}");
            assert!(stdout.ends_with('\n') && stdout.trim().lines().count() == 1);
            let document: Value = serde_json::from_str(stdout.trim()).unwrap();
            let schema = crate::testing::schema("dupes.schema.json");
            assert_eq!(
                crate::testing::validate(&document, &schema, &schema),
                Vec::<String>::new()
            );
            (
                document["minTokens"].clone(),
                document["minLines"].clone(),
                document["duplicates"].as_array().unwrap().len(),
            )
        };
        assert_eq!(groups(&[]), (json!(50), json!(6), 2));
        assert_eq!(
            groups(&["--min-tokens", "20", "--min-lines", "3"]),
            (json!(20), json!(3), 3)
        );
        assert_eq!(groups(&["-min-lines=14"]), (json!(50), json!(14), 0));
    }
}
