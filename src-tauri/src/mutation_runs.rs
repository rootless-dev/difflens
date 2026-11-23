//! Running mutation testing on the lines a review adds, only when the user
//! asks (never as part of an analysis), and keeping the last run.
//!
//! The engine is the language analyzer that runs the tests (`mutate`, see
//! `analyzers/protocol/mutation.schema.json`, and `mutation`). A run goes
//! like a test run (`test_runs`): a fresh copy of the new side in a run
//! folder of its own, `prepare` on it as a plain process, then `mutate`
//! isolated as the project's tests are (the macOS sandbox with the network
//! off, or a plain process), with the project's build cache as
//! `--cache-dir`, held for the whole run. For an analyzer that drives an
//! engine (Rust: cargo-mutants; JS/TS: StrykerJS), `prepare` also gets
//! `--tools <folder>`, a folder of the analyzer's environment where it
//! installs the engine the first time (`AnalyzerLocator::tools_folder`).
//!
//! Which lines are tried: those the review adds that the last completed
//! test run with coverage of the new side executed (lines no test executes
//! are already shown as untested). Without such a run there is nothing to
//! tell them by, and the state says so (`MutationUnavailable::NeedsCoverage`).
//! When none of the added lines ran, no analyzer is started.
//!
//! The analyzer reuses the results of functions whose code and tests did not
//! change (it keeps them in `--cache-dir`), so running again only tests what
//! changed. The core keeps the last completed run of each review, under
//! `<cache>/mutation-runs/<project>/<analyzer id>/<runner fingerprint>/`, as
//! `<old sha>..<new sha>.json`, or `<old sha>..working-tree.json` with the
//! stamp of the files it ran on (`trees::working_tree_stamp`), found again
//! only while the working tree still has that stamp.
//!
//! One run per new side at a time, shared by concurrent callers asking the
//! same; another one meanwhile fails with `mutation_run_busy`. While it goes
//! on, the analyzer keeps a progress file in the run folder (`progress`); a
//! cancel stops it with all it started.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::analysis::Function;
use crate::disk_cache;
use crate::duplication;
use crate::error::{app_error, core_text, AppError, AppResult, CoreText};
use crate::flow::{self, Analyzer, FlowService};
use crate::hunks::ChangedLines;
use crate::model::{
    AnalyzerIdentity, Isolation, Mutant, MutationProgress, MutationRun, MutationState,
    MutationSummary, MutationUnavailable, TestRunStatus,
};
use crate::mutation::{self, MAX_MUTANTS};
use crate::process::{Ending, Limits};
use crate::review;
use crate::runner_protocol::Coverage;
use crate::single_flight::SingleFlight;
use crate::test_report::MAX_OUTPUT_BYTES;
use crate::test_runs::{self, disk_error, text_of, Outcome, PrepareRequest, TestsService};
use crate::trees;

/// Runs kept per project and analyzer build.
const KEEP_RUN_FILES: usize = 50;
const WORKING_TREE: &str = "working-tree";
/// In the run folder, outside the copy.
const CHANGED_FILE: &str = "changed.json";
const COVERAGE_FILE: &str = "coverage.json";
const PROGRESS_FILE: &str = "mutation-progress.json";

#[derive(Debug, Clone)]
pub struct MutationSettings {
    /// Passed to the analyzer as `--timeout`: it reports what it tested by
    /// then.
    pub runner_timeout: Duration,
    /// When `mutate` is stopped: a little after `runner_timeout`.
    pub hard_timeout: Duration,
    pub max_mutants: u32,
    /// Mutants tested at the same time (`--jobs`).
    pub jobs: usize,
    pub max_output_bytes: usize,
}

impl Default for MutationSettings {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(2, usize::from);
        Self {
            runner_timeout: Duration::from_secs(30 * 60),
            hard_timeout: Duration::from_secs(31 * 60),
            max_mutants: MAX_MUTANTS,
            jobs: (cpus / 2).clamp(1, 8),
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

/// One new side of one project: what runs are shared and cancelled by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunKey {
    project: String,
    /// `None` for the working tree.
    commit: Option<String>,
}

impl RunKey {
    fn new(project_id: &str, new_sha: Option<&str>) -> Self {
        Self {
            project: project_id.to_string(),
            commit: new_sha.map(str::to_string),
        }
    }
}

/// What a run is asked: the old side chooses the lines.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    old_sha: String,
    isolation: Isolation,
}

struct ActiveRun {
    cancel: Arc<AtomicBool>,
    request: Request,
    /// Once its run folder is made.
    progress: Arc<Mutex<Option<PathBuf>>>,
}

type ActiveRuns = Mutex<HashMap<RunKey, ActiveRun>>;

/// A working tree run as kept on disk, with the stamp of the files it ran on.
#[derive(Debug, Serialize, Deserialize)]
struct KeptWorkingRun {
    stamp: u64,
    run: MutationRun,
}

pub struct MutationService {
    flow: Arc<FlowService>,
    tests: Arc<TestsService>,
    cache: PathBuf,
    settings: MutationSettings,
    flights: SingleFlight<RunKey, AppResult<(Request, MutationRun)>>,
    active: ActiveRuns,
}

impl MutationService {
    /// Runs are kept under `cache`; the copies are made by `tests`, as its
    /// own runs' are.
    pub fn new(
        flow: Arc<FlowService>,
        tests: Arc<TestsService>,
        cache: PathBuf,
        settings: MutationSettings,
    ) -> Self {
        Self {
            flow,
            tests,
            cache,
            settings,
            flights: SingleFlight::new(),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// The last completed run of these sides, whether one goes on, and why
    /// mutation testing cannot run now (no test runner, no test run with
    /// coverage). Runs nothing but analyzers.
    pub fn state(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> AppResult<MutationState> {
        review::check_commits(self.flow.git(), repo, old_sha, new_sha)?;
        let running = self.running_request(project_id, new_sha).is_some();
        let mut state = MutationState {
            run: None,
            running,
            unavailable: None,
            max_mutants: self.settings.max_mutants,
        };
        let spec = match test_runs::runner_spec(repo) {
            Ok(spec) => spec,
            Err(languages) => {
                state.unavailable = Some(MutationUnavailable::NoRunner { languages });
                return Ok(state);
            }
        };
        let Ok(analyzer) = self.flow.analyzer(spec) else {
            return Ok(state);
        };
        state.run = self.kept(project_id, repo, &analyzer, old_sha, new_sha);
        if self.coverage(project_id, repo, &analyzer, new_sha).is_none() {
            state.unavailable = Some(MutationUnavailable::NeedsCoverage);
        }
        Ok(state)
    }

    /// Runs mutation testing on the lines these sides add, isolated as
    /// `isolation` says, or joins the same run going on, and returns it.
    /// Fails with `no_test_runner` when the project has none, with
    /// `mutation_needs_coverage` without a completed test run with coverage
    /// of the new side, and with `mutation_run_busy` while another run of
    /// this new side goes on.
    pub fn run(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        isolation: Isolation,
    ) -> AppResult<MutationRun> {
        review::check_commits(self.flow.git(), repo, old_sha, new_sha)?;
        let spec = test_runs::runner_spec(repo).map_err(|languages| {
            if languages.is_empty() {
                app_error!(
                    Internal,
                    NoTestRunnerNoLanguages,
                    "This project has no test runner (languages: none found)."
                )
            } else {
                app_error!(
                    Internal,
                    NoTestRunner,
                    "This project has no test runner (languages: {languages}).",
                    languages = languages.join(", ")
                )
            }
        })?;
        let request = Request {
            old_sha: old_sha.to_string(),
            isolation,
        };
        if self
            .running_request(project_id, new_sha)
            .is_some_and(|running| running != request)
        {
            return Err(busy());
        }
        let key = RunKey::new(project_id, new_sha);
        let abandoned = Err(app_error!(
            Internal,
            MutationRunAbandoned,
            "The mutation testing run stopped unexpectedly."
        ));
        let (ran, run) = self.flights.run(key.clone(), abandoned, || {
            self.execute(&key, repo, spec, &request)
                .map(|run| (request.clone(), run))
        })?;
        if ran != request {
            return Err(busy());
        }
        Ok(run)
    }

    /// Stops the run going on for this new side, if any.
    pub fn cancel(&self, project_id: &str, new_sha: Option<&str>) {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = active.get(&RunKey::new(project_id, new_sha)) {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// How far the run going on for this new side got, as the analyzer last
    /// wrote it; None when none runs or it has not said yet.
    pub fn progress(&self, project_id: &str, new_sha: Option<&str>) -> Option<MutationProgress> {
        let file = {
            let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
            let run = active.get(&RunKey::new(project_id, new_sha))?;
            let file = run.progress.lock().unwrap_or_else(PoisonError::into_inner).clone();
            file?
        };
        mutation::parse_progress(&fs::read(file).ok()?)
    }

    fn running_request(&self, project_id: &str, new_sha: Option<&str>) -> Option<Request> {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&RunKey::new(project_id, new_sha))
            .map(|run| run.request.clone())
    }

    /// The coverage of the last completed test run of `analyzer` on the new
    /// side that measured it, with when that run started.
    fn coverage(
        &self,
        project_id: &str,
        repo: &Path,
        analyzer: &Analyzer,
        new_sha: Option<&str>,
    ) -> Option<(Coverage, String)> {
        let (record, output) = self.tests.last_completed(project_id, repo, analyzer, new_sha)?;
        if !record.options.coverage {
            return None;
        }
        let coverage = output.coverage?;
        (!coverage.blocks.is_empty()).then_some((coverage, record.started_at))
    }

    fn execute(
        &self,
        key: &RunKey,
        repo: &Path,
        spec: &'static crate::analyzers::AnalyzerSpec,
        request: &Request,
    ) -> AppResult<MutationRun> {
        let started_at = flow::now()?;
        let clock = Instant::now();
        let new_sha = key.commit.as_deref();
        let analyzer = self.flow.analyzer(spec);
        let mut run = MutationRun {
            language: spec.languages[0].to_string(),
            runner: AnalyzerIdentity {
                name: spec.name.to_string(),
                version: String::new(),
            },
            engine: None,
            old_sha: request.old_sha.clone(),
            new_sha: new_sha.map(str::to_string),
            isolation: request.isolation,
            started_at,
            duration_ms: 0,
            status: TestRunStatus::Failed,
            message: None,
            coverage_run_at: None,
            mutants: Vec::new(),
            summary: MutationSummary {
                max_mutants: self.settings.max_mutants,
                ..MutationSummary::default()
            },
            diagnostics: Vec::new(),
        };
        let analyzer = match analyzer {
            Ok(analyzer) => analyzer,
            Err(message) => {
                run.message = Some(message);
                return Ok(run);
            }
        };
        let Some((coverage, coverage_run_at)) = self.coverage(&key.project, repo, &analyzer, new_sha) else {
            return Err(app_error!(
                Internal,
                MutationNeedsCoverage,
                "Mutation testing needs a completed test run with coverage of this code first."
            ));
        };
        run.coverage_run_at = Some(coverage_run_at);
        // Taken before the copy: files edited during the run make the run
        // older than the working tree.
        let stamp = match new_sha {
            Some(_) => None,
            None => trees::working_tree_stamp(self.flow.git(), repo).ok(),
        };
        let lines = self.flow.changed_lines(repo, &request.old_sha, new_sha)?;
        if mutation::candidate_lines(&lines, &coverage) == 0 {
            run.status = TestRunStatus::Completed;
            run.diagnostics.push(core_text!(
                MutationNothingCovered,
                "The tests execute none of the lines this review adds, so there is nothing to mutate."
            ));
        } else {
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(Mutex::new(None));
            let _registration = Registration::new(
                &self.active,
                key,
                ActiveRun {
                    cancel: Arc::clone(&cancel),
                    request: request.clone(),
                    progress: Arc::clone(&progress),
                },
            );
            let attempt = Attempt {
                key,
                repo,
                analyzer: &analyzer,
                request,
                lines: &lines,
                coverage: &coverage,
                cancel: &cancel,
                progress: &progress,
            };
            match self.attempt(&attempt)? {
                Ok(finished) => {
                    run.status = TestRunStatus::Completed;
                    run.runner = finished.runner;
                    run.language = finished.language;
                    run.engine = Some(finished.engine);
                    run.summary = finished.summary;
                    run.diagnostics = finished.diagnostics.into_iter().map(CoreText::raw).collect();
                    run.mutants = finished.mutants;
                }
                Err(ending) => {
                    run.status = ending.status;
                    run.message = ending.message;
                }
            }
        }
        run.duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        if run.status == TestRunStatus::Completed {
            self.keep(key, &analyzer, &run, stamp);
        }
        Ok(run)
    }

    /// Copies and prepares the new side, runs `mutate` there and reads what
    /// it printed, with the mutants' code read from the copy before it is
    /// deleted; or how the run ended.
    fn attempt(&self, attempt: &Attempt<'_>) -> AppResult<Result<Finished, Outcome>> {
        let analyzer = attempt.analyzer;
        let tools = self.flow.tools_folder(analyzer);
        let prepare = PrepareRequest {
            project_id: &attempt.key.project,
            commit: attempt.key.commit.as_deref(),
            repo: attempt.repo,
            analyzer,
            tools: tools.as_deref(),
        };
        let progress = attempt.progress;
        let on_folder = |run_directory: &Path| {
            *progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(run_directory.join(PROGRESS_FILE));
        };
        let copy = match self.tests.prepared_copy(&prepare, attempt.cancel, on_folder)? {
            Ok(copy) => copy,
            Err(ending) => return Ok(Err(ending)),
        };
        let changed = copy.run_directory.join(CHANGED_FILE);
        fs::write(&changed, duplication::changed_document(attempt.lines)).map_err(|error| disk_error(&changed, &error))?;
        let coverage = copy.run_directory.join(COVERAGE_FILE);
        fs::write(&coverage, mutation::coverage_document(attempt.coverage))
            .map_err(|error| disk_error(&coverage, &error))?;
        let mut arguments: Vec<OsString> = vec![
            "mutate".into(),
            "--dir".into(),
            copy.tree.clone().into(),
            "--timeout".into(),
            format!("{}s", self.settings.runner_timeout.as_secs().max(1)).into(),
            "--changed".into(),
            changed.into(),
            "--coverage".into(),
            coverage.into(),
            "--cache-dir".into(),
            copy.cache_directory.clone().into(),
            "--max-mutants".into(),
            self.settings.max_mutants.to_string().into(),
            "--jobs".into(),
            self.settings.jobs.to_string().into(),
            "--progress".into(),
            copy.run_directory.join(PROGRESS_FILE).into(),
        ];
        if let Some(tools) = &tools {
            arguments.push("--tools".into());
            arguments.push(tools.clone().into());
        }
        let command = match self
            .tests
            .step_command(analyzer, &copy, attempt.request.isolation, arguments)
        {
            Ok(command) => command,
            Err(ending) => return Ok(Err(ending)),
        };
        let limits = Limits {
            timeout: self.settings.hard_timeout,
            max_stdout_bytes: self.settings.max_output_bytes,
            stderr_tail_bytes: MAX_OUTPUT_BYTES,
        };
        let captured = match self.tests.spawn(
            command,
            &copy.tree,
            &copy.temporary,
            &limits,
            attempt.cancel,
            analyzer.spec,
        ) {
            Ok(captured) => captured,
            Err(ending) => return Ok(Err(ending)),
        };
        let title = analyzer.spec.title();
        let ending = match captured.ending {
            Ending::Exited(status) if status.success() => {
                match mutation::parse(&captured.stdout, analyzer.spec.languages, &title) {
                    Ok(output) => {
                        let functions = self.functions(attempt);
                        let summary = mutation::summary(&output);
                        let finished = Finished {
                            language: output.language.clone(),
                            runner: output.runner.clone(),
                            engine: output.engine.clone(),
                            summary,
                            diagnostics: output.diagnostics.clone(),
                            // Read now: the copy goes when this returns.
                            mutants: mutation::mutants(output, &copy.tree, &functions),
                        };
                        return Ok(Ok(finished));
                    }
                    Err(message) => Outcome::failed(message),
                }
            }
            Ending::Exited(status) => Outcome::failed(match text_of(&captured.stderr_tail) {
                Some(stderr) => CoreText::raw(stderr),
                None => core_text!(
                    RunnerFailedQuietly,
                    "{analyzer} failed ({status}) and reported no details.",
                    analyzer = title,
                    status = status.to_string()
                ),
            }),
            Ending::OutputTooLarge => Outcome::failed(crate::analyzers::printed_too_much(
                &title,
                self.settings.max_output_bytes / (1024 * 1024),
            )),
            Ending::TimedOut => Outcome::stopped(TestRunStatus::TimedOut),
            Ending::Cancelled => Outcome::stopped(TestRunStatus::Cancelled),
        };
        Ok(Err(ending))
    }

    /// The functions of the new side the analyzer of the run reported, to
    /// place the mutants on; none when the sides cannot be analysed.
    fn functions(&self, attempt: &Attempt<'_>) -> Vec<Function> {
        let new_sha = attempt.key.commit.as_deref();
        let Ok(Ok(sides)) = self
            .flow
            .sides(&attempt.key.project, attempt.repo, &attempt.request.old_sha, new_sha)
        else {
            return Vec::new();
        };
        sides
            .analyzers
            .iter()
            .position(|analyzer| analyzer.spec.id == attempt.analyzer.spec.id)
            .map(|index| sides.new[index].functions.clone())
            .unwrap_or_default()
    }

    fn run_file(&self, project_id: &str, analyzer: &Analyzer, old_sha: &str, new_sha: Option<&str>) -> PathBuf {
        self.cache
            .join("mutation-runs")
            .join(project_id)
            .join(analyzer.spec.id)
            .join(&analyzer.fingerprint)
            .join(format!("{old_sha}..{}.json", new_sha.unwrap_or(WORKING_TREE)))
    }

    /// Keeps a completed run as the last of its sides; a working tree run
    /// with `stamp`, without which it is not kept. One that cannot be kept
    /// is only lost.
    fn keep(&self, key: &RunKey, analyzer: &Analyzer, run: &MutationRun, stamp: Option<u64>) {
        let file = self.run_file(&key.project, analyzer, &run.old_sha, key.commit.as_deref());
        let bytes = match (&key.commit, stamp) {
            (Some(_), _) => serde_json::to_vec(run),
            (None, Some(stamp)) => serde_json::to_vec(&KeptWorkingRun {
                stamp,
                run: run.clone(),
            }),
            (None, None) => return,
        };
        if let Ok(bytes) = bytes {
            let _ = disk_cache::write(&file, &bytes, KEEP_RUN_FILES);
        }
    }

    /// The last completed run kept for these sides; for the working tree,
    /// only while its files still have the stamp it ran on.
    fn kept(
        &self,
        project_id: &str,
        repo: &Path,
        analyzer: &Analyzer,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> Option<MutationRun> {
        let file = self.run_file(project_id, analyzer, old_sha, new_sha);
        let bytes = fs::read(&file).ok()?;
        let run = match new_sha {
            Some(_) => serde_json::from_slice(&bytes).ok()?,
            None => {
                let kept: KeptWorkingRun = serde_json::from_slice(&bytes).ok()?;
                if kept.stamp != trees::working_tree_stamp(self.flow.git(), repo).ok()? {
                    return None;
                }
                kept.run
            }
        };
        disk_cache::touch(&file);
        Some(run)
    }
}

/// What one attempt runs on.
struct Attempt<'a> {
    key: &'a RunKey,
    repo: &'a Path,
    analyzer: &'a Analyzer,
    request: &'a Request,
    lines: &'a ChangedLines,
    coverage: &'a Coverage,
    cancel: &'a AtomicBool,
    progress: &'a Mutex<Option<PathBuf>>,
}

/// What a completed `mutate` reported, with the mutants' code and functions
/// read while the copy was there.
struct Finished {
    language: String,
    runner: AnalyzerIdentity,
    engine: AnalyzerIdentity,
    summary: MutationSummary,
    diagnostics: Vec<String>,
    mutants: Vec<Mutant>,
}

fn busy() -> AppError {
    app_error!(
        Internal,
        MutationRunBusy,
        "Another mutation testing run of this code is going on. Wait for it to end."
    )
}

/// Lists a run as going on, with its cancel flag, until dropped.
struct Registration<'a> {
    active: &'a ActiveRuns,
    key: RunKey,
}

impl<'a> Registration<'a> {
    fn new(active: &'a ActiveRuns, key: &RunKey, run: ActiveRun) -> Self {
        active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), run);
        Self {
            active,
            key: key.clone(),
        }
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{analysis, call, function, in_test};
    use crate::analysis::Analysis;
    use crate::analyzers::{AnalyzerLocator, RunSettings};
    use crate::error::Reason;
    use crate::model::{MutantStatus, MutationPhase};
    use crate::mutation::fixtures::{document, mutant};
    use crate::runner_protocol::fixtures::{block, output, test};
    use crate::runner_protocol::TestOutput;
    use crate::test_runs::fixtures::{calls, fake_runner, fake_sandbox_exec};
    use crate::test_runs::TestSettings;
    use crate::test_support::{isolated_git, TestRepo};
    use crate::tool_path::ToolPath;
    use serde_json::json;
    use std::process::Stdio;
    use std::thread;
    use tempfile::TempDir;

    const PROJECT: &str = "project-1";

    struct Harness {
        repo: TestRepo,
        tools: TempDir,
        cache: TempDir,
        tests: Arc<TestsService>,
        service: MutationService,
    }

    fn settings(tools: &Path) -> TestSettings {
        let temporary_root = tools.join("tmp-root");
        fs::create_dir_all(&temporary_root).unwrap();
        TestSettings {
            prepare_timeout: Duration::from_secs(30),
            runner_timeout: Duration::from_secs(600),
            hard_timeout: Duration::from_secs(30),
            max_output_bytes: 10 * 1024 * 1024,
            sandbox_exec: fake_sandbox_exec(tools),
            temporary_root,
        }
    }

    fn mutation_settings() -> MutationSettings {
        MutationSettings {
            runner_timeout: Duration::from_secs(120),
            hard_timeout: Duration::from_secs(30),
            max_mutants: MAX_MUTANTS,
            jobs: 3,
            max_output_bytes: 10 * 1024 * 1024,
        }
    }

    impl Harness {
        fn new() -> Self {
            let tools = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let services = Self::services(tools.path(), cache.path());
            Self {
                repo: TestRepo::init(),
                tools,
                cache,
                tests: services.0,
                service: services.1,
            }
        }

        /// Services over `cache` with the fake runner in `tools`; called
        /// again, the app after a restart.
        fn services(tools: &Path, cache: &Path) -> (Arc<TestsService>, MutationService) {
            let runner = tools.join("fake analyzer");
            let runner = if runner.exists() { runner } else { fake_runner(tools) };
            let flow = Arc::new(FlowService::new(
                isolated_git(),
                cache.to_path_buf(),
                AnalyzerLocator::with_executable("go", runner),
                ToolPath::fixed("/usr/bin:/bin"),
                RunSettings::default(),
            ));
            let tests = Arc::new(TestsService::new(Arc::clone(&flow), cache.to_path_buf(), settings(tools)));
            let service = MutationService::new(flow, Arc::clone(&tests), cache.to_path_buf(), mutation_settings());
            (tests, service)
        }

        fn flag(&self, name: &str, on: bool) {
            let path = self.tools.path().join(name);
            if on {
                fs::write(path, "").unwrap();
            } else {
                let _ = fs::remove_file(path);
            }
        }

        fn read_tool(&self, name: &str) -> String {
            fs::read_to_string(self.tools.path().join(name)).unwrap_or_default()
        }

        fn commit(&self, files: &[(&str, &str)], analysis: &Analysis, tests: &TestOutput, mutation: &serde_json::Value) -> String {
            for (path, contents) in files {
                self.repo.write(path, contents);
            }
            self.repo.write("fixture/analysis.json", serde_json::to_string(analysis).unwrap());
            self.repo.write("fixture/tests.json", serde_json::to_string(tests).unwrap());
            self.repo.write("fixture/mutation.json", mutation.to_string());
            self.repo.commit_all("commit");
            self.repo.git(&["rev-parse", "HEAD"])
        }

        fn run_tests(&self, old: &str, new: Option<&str>) {
            let run = self
                .tests
                .run(PROJECT, self.repo.path(), old, new, Isolation::Process)
                .unwrap();
            assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        }

        fn run(&self, old: &str, new: Option<&str>, isolation: Isolation) -> AppResult<MutationRun> {
            self.service.run(PROJECT, self.repo.path(), old, new, isolation)
        }

        fn state(&self, old: &str, new: Option<&str>) -> MutationState {
            self.service.state(PROJECT, self.repo.path(), old, new).unwrap()
        }
    }

    const BANK: &str = "package bank\n\nfunc Withdraw(saldo, valor int) bool {\n\tif saldo >= valor {\n\t\treturn true\n\t}\n\treturn false\n}\n";

    fn new_side() -> Analysis {
        analysis(
            vec![
                function("bank.Withdraw", "bank.go", 3, 8),
                in_test(function("bank.TestWithdraw", "bank_test.go", 1, 5)),
            ],
            vec![call("bank.TestWithdraw", "bank.Withdraw", 2)],
        )
    }

    /// Line 4 ran; line 6 never did.
    fn tests_with_coverage(count: u64) -> TestOutput {
        output(
            vec![test("TestWithdraw", Some("bank.TestWithdraw"))],
            Some(vec![block("bank.go", 4, 5, count), block("bank.go", 6, 7, 0)]),
        )
    }

    fn report() -> serde_json::Value {
        document(json!([
            mutant("m-survivor", "bank.go", 4, 11, ">=", ">", "survived"),
            mutant("m-killed", "bank.go", 5, 10, "true", "false", "killed"),
        ]))
    }

    /// The old side without `Withdraw`'s condition; the new one changes
    /// lines 4 to 7 and adds a test.
    fn two_commits(harness: &Harness, count: u64) -> (String, String) {
        let old_bank = "package bank\n\nfunc Withdraw(saldo, valor int) bool {\n\treturn false\n}\n";
        let old = harness.commit(
            &[("go.mod", "module example.com/bank\n"), ("bank.go", old_bank)],
            &analysis(vec![function("bank.Withdraw", "bank.go", 3, 5)], Vec::new()),
            &tests_with_coverage(count),
            &report(),
        );
        let new = harness.commit(
            &[("bank.go", BANK), ("bank_test.go", "package bank\n")],
            &new_side(),
            &tests_with_coverage(count),
            &report(),
        );
        (old, new)
    }

    fn reason(error: &AppError) -> Option<Reason> {
        error.reason
    }

    #[test]
    fn mutation_testing_needs_a_test_run_with_coverage_first() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness, 1);

        let state = harness.state(&old, Some(&new));
        assert_eq!(state.unavailable, Some(MutationUnavailable::NeedsCoverage));
        assert_eq!((state.run.is_none(), state.running, state.max_mutants), (true, false, MAX_MUTANTS));
        let error = harness.run(&old, Some(&new), Isolation::Process).unwrap_err();
        assert_eq!(reason(&error), Some(Reason::MutationNeedsCoverage));
        assert_eq!(calls(harness.tools.path(), "mutate"), 0);

        // A run without coverage does not count either.
        let without = harness
            .tests
            .run_with(
                PROJECT,
                harness.repo.path(),
                &old,
                Some(&new),
                &crate::test_runs::RunRequest {
                    isolation: Isolation::Process,
                    options: crate::model::RunOptions { race: false, coverage: false },
                    selection: None,
                },
            )
            .unwrap();
        assert_eq!(without.status, TestRunStatus::Completed);
        assert_eq!(harness.state(&old, Some(&new)).unavailable, Some(MutationUnavailable::NeedsCoverage));

        harness.run_tests(&old, Some(&new));
        assert_eq!(harness.state(&old, Some(&new)).unavailable, None);
    }

    #[test]
    fn a_project_without_a_test_runner_says_so() {
        let harness = Harness::new();
        harness.repo.write("notes.txt", "hello\n");
        harness.repo.commit_all("notes");
        let sha = harness.repo.git(&["rev-parse", "HEAD"]);
        let state = harness.state(&sha, Some(&sha));
        assert!(matches!(state.unavailable, Some(MutationUnavailable::NoRunner { .. })), "{state:?}");
        let error = harness.run(&sha, Some(&sha), Isolation::Process).unwrap_err();
        assert_eq!(reason(&error), Some(Reason::NoTestRunnerNoLanguages));
    }

    #[test]
    fn runs_mutate_on_a_prepared_copy_under_the_sandbox_and_keeps_the_run() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness, 1);
        harness.run_tests(&old, Some(&new));
        let status_before = harness.repo.git(&["status", "--porcelain"]);

        let run = harness.run(&old, Some(&new), Isolation::Sandbox).unwrap();

        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        assert_eq!((run.old_sha.as_str(), run.new_sha.as_deref()), (old.as_str(), Some(new.as_str())));
        assert_eq!(run.isolation, Isolation::Sandbox);
        assert_eq!(run.engine.as_ref().unwrap().name, "difflens-analyzer-go mutator");
        assert_eq!(run.runner.name, "difflens-analyzer-go");
        assert!(run.coverage_run_at.is_some());
        assert_eq!((run.summary.killed, run.summary.survived, run.summary.candidates), (1, 1, 2));
        assert_eq!(run.diagnostics, vec![CoreText::raw("one package skipped")]);
        let survivor = &run.mutants[0];
        assert_eq!((survivor.id.as_str(), survivor.status), ("m-survivor", MutantStatus::Survived));
        assert_eq!(survivor.before, vec!["\tif saldo >= valor {"]);
        assert_eq!(survivor.after, vec!["\tif saldo > valor {"]);
        assert_eq!(survivor.function.as_ref().unwrap().id, "bank.Withdraw");
        assert_eq!(run.mutants[1].killed_by.as_deref(), Some("TestWithdraw"));

        // prepare first, on the copy; then mutate, offline, in the sandbox.
        let prepare = harness.read_tool("prepare-args");
        assert!(prepare.contains("--cache-dir") && !prepare.contains("--tools"), "Go needs no engine: {prepare}");
        let arguments = harness.read_tool("mutate-args");
        for expected in ["--changed", "--coverage", "--cache-dir", "--max-mutants 200", "--jobs 3", "--progress", "--timeout 120s", "--offline"] {
            assert!(arguments.contains(expected), "{expected}: {arguments}");
        }
        assert!(!arguments.contains("--tools"), "{arguments}");
        let tree = arguments.split_whitespace().nth(2).unwrap();
        assert!(!tree.starts_with(harness.repo.path().to_str().unwrap()), "a copy, not the project");
        assert!(harness.read_tool("sandbox-args").contains("mutate"));
        let changed: serde_json::Value = serde_json::from_str(&harness.read_tool("mutate-changed")).unwrap();
        assert_eq!(changed["files"][0], json!({ "path": "bank.go", "added": [[4, 6]] }));
        assert_eq!(changed["files"][1], json!({ "path": "bank_test.go", "added": [[1, 1]] }));
        let coverage: serde_json::Value = serde_json::from_str(&harness.read_tool("mutate-coverage")).unwrap();
        assert_eq!(coverage["blocks"][0]["startLine"], 4);
        assert_eq!(harness.repo.git(&["status", "--porcelain"]), status_before, "the project is untouched");

        // Kept, also across a restart, and only for these sides.
        let state = harness.state(&old, Some(&new));
        assert_eq!(state.run.as_ref(), Some(&run));
        let (_, restarted) = Harness::services(harness.tools.path(), harness.cache.path());
        let state = restarted.state(PROJECT, harness.repo.path(), &old, Some(&new)).unwrap();
        assert_eq!(state.run, Some(run));
        assert!(harness.state(&new, Some(&new)).run.is_none());
    }

    #[test]
    fn nothing_runs_when_the_tests_execute_no_added_line() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness, 0);
        harness.run_tests(&old, Some(&new));

        let run = harness.run(&old, Some(&new), Isolation::Process).unwrap();

        assert_eq!(run.status, TestRunStatus::Completed);
        assert!(run.mutants.is_empty());
        assert_eq!(run.diagnostics[0].reason, Some(Reason::MutationNothingCovered));
        assert_eq!(calls(harness.tools.path(), "mutate"), 0);
        assert_eq!(calls(harness.tools.path(), "prepare"), 1, "only the test run's");
    }

    #[test]
    fn a_failed_mutate_says_why_and_keeps_the_last_completed_run() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness, 1);
        harness.run_tests(&old, Some(&new));
        let completed = harness.run(&old, Some(&new), Isolation::Process).unwrap();
        harness.flag("mutate-fail", true);

        let failed = harness.run(&old, Some(&new), Isolation::Process).unwrap();

        assert_eq!(failed.status, TestRunStatus::Failed);
        assert!(failed.message.as_ref().unwrap().contains("mutator crashed"), "{:?}", failed.message);
        assert_eq!(harness.state(&old, Some(&new)).run, Some(completed));
    }

    #[test]
    fn an_invalid_report_is_a_failed_run() {
        let harness = Harness::new();
        let mut broken = report();
        broken["mutants"][0]["line"] = json!(0);
        let old = harness.commit(&[("go.mod", "module example.com/bank\n"), ("bank.go", "package bank\n")], &new_side(), &tests_with_coverage(1), &broken);
        let new = harness.commit(&[("bank.go", BANK)], &new_side(), &tests_with_coverage(1), &broken);
        harness.run_tests(&old, Some(&new));

        let run = harness.run(&old, Some(&new), Isolation::Process).unwrap();

        assert_eq!(run.status, TestRunStatus::Failed);
        assert_eq!(run.message.as_ref().unwrap().reason, Some(Reason::InvalidMutationReport));
        assert!(harness.state(&old, Some(&new)).run.is_none());
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn a_run_shows_its_progress_is_shared_and_cancelled_with_all_it_started() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness, 1);
        harness.run_tests(&old, Some(&new));
        harness.flag("mutate-hang", true);
        let (service, repo) = (&harness.service, harness.repo.path());

        let started = Instant::now();
        let run = thread::scope(|scope| {
            let worker = scope.spawn(|| service.run(PROJECT, repo, &old, Some(&new), Isolation::Process));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !harness.tools.path().join("child.pid").exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            let progress = service.progress(PROJECT, Some(&new)).expect("progress while it runs");
            assert_eq!((progress.phase, progress.total, progress.done), (MutationPhase::Mutating, 3, 1));
            assert_eq!(progress.current.as_deref(), Some("a.go:3"));
            assert!(service.progress(PROJECT, Some(&old)).is_none());
            assert!(harness.state(&old, Some(&new)).running);
            // Another run of this new side is busy.
            let busy = service.run(PROJECT, repo, &new, Some(&new), Isolation::Process).unwrap_err();
            assert_eq!(reason(&busy), Some(Reason::MutationRunBusy));
            // Another side is not affected.
            service.cancel(PROJECT, Some(&old));
            service.cancel(PROJECT, None);
            assert!(harness.state(&old, Some(&new)).running);
            service.cancel(PROJECT, Some(&new));
            worker.join().unwrap()
        });

        let run = run.unwrap();
        assert_eq!(run.status, TestRunStatus::Cancelled);
        assert!(run.mutants.is_empty());
        assert!(started.elapsed() < Duration::from_secs(20));
        let pid = harness.read_tool("child.pid");
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(&pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(&pid), "the analyzer's children are stopped");
        let state = harness.state(&old, Some(&new));
        assert!(!state.running && state.run.is_none(), "a cancelled run is not kept");
        assert!(service_is_idle(&harness));
    }

    fn service_is_idle(harness: &Harness) -> bool {
        harness.service.progress(PROJECT, None).is_none()
            && harness.service.active.lock().unwrap().is_empty()
    }

    #[test]
    fn a_working_tree_run_is_kept_while_its_files_stay_the_same() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness, 1);
        harness.repo.write("bank.go", BANK.replace("return false", "return !true"));
        harness.run_tests(&old, None);

        let run = harness.run(&old, None, Isolation::Process).unwrap();
        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        assert_eq!(harness.state(&old, None).run, Some(run));

        harness.repo.write("bank.go", BANK);
        assert!(harness.state(&old, None).run.is_none(), "the files changed since");
    }
}
