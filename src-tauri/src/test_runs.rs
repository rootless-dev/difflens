//! Running a project's tests on the new side of a review, and keeping the
//! last run.
//!
//! The runner is the language analyzer itself (`prepare` and `test`, see
//! `analyzers/protocol/tests.schema.json`). Every run gets a throwaway run
//! folder under `<cache>/test-scratch/`, holding a fresh copy of the new side
//! (`git archive` of the commit, or a copy of the working tree) in `tree/`
//! and the TMPDIR of the run in `tmp/`; tests may write there, and it is
//! deleted when the run ends. The shared trees analyzers read are never
//! used, since tests write.
//!
//! First `prepare` runs as a plain process (it may download dependencies and
//! executes no project code), then `test`, isolated as asked: a plain child
//! process in its own process group, or under the macOS sandbox (see
//! `sandbox`) with the network off (`--offline`) and writes only to the run
//! folder, the folders `prepare` reported and the project's build cache.
//! Both steps get that build cache (`build_cache`, one folder per project
//! and analyzer kept between runs) as `--cache-dir`, held for the whole run.
//! A run is stopped, with all it started, after the time limit or when
//! cancelled.
//!
//! Concurrent runs for the same new side share one run; the old side does
//! not matter to a run, only to what is derived from it (`test_report`).
//! Only one run of a new side goes on at a time: asking for a different one
//! meanwhile fails with `test_run_busy`.
//!
//! A run asks for the race detector and coverage or not (`RunOptions`), and
//! may run only a selection (`RunSelection`), given to the runner in a file
//! of the run folder. A partial run that completes is merged into the last
//! completed run (`test_report::merge`): only the tests it ran are
//! replaced, and the merged run is kept in its place, saying when the
//! partial run was (`partial`). One that does not complete leaves the kept
//! runs as they were and is only noted on the last one. While a run goes
//! on, the runner keeps a progress file in the run folder up to date, read
//! by `progress`.
//!
//! The last run of a commit is kept on disk as the runner printed it, under
//! `<cache>/test-runs/<project>/<analyzer id>/<runner fingerprint>/<sha>.json`,
//! and the last completed one beside it (`<sha>.completed.json`), so the
//! Quality tab still finds coverage after a cancelled run. The runner
//! fingerprint is a hash of the executable, or of the shipped source of a
//! provisioned analyzer (see `analyzers`), so a restart that rebuilt the
//! same analyzer finds them again. Both steps start the analyzer through its
//! launcher; under the sandbox, the launcher's program (a sidecar, a Python
//! or Node runtime, a built binary) and its environment are read and run
//! like any other file, as the profile only limits writes. A working tree
//! run is kept in memory, the last one per project, and on disk beside the
//! commits' runs (`working-tree.json`, `working-tree.completed.json`) with
//! the stamp of the files it ran on (`trees::working_tree_stamp`): after a
//! restart it is found again only while the working tree still has that
//! stamp.
//!
//! What `state` derives from the last run for the sides of a review (the
//! run read against the review, with the new side's analysis it was read
//! with) is kept in memory for the few last reviews, keyed by both sides,
//! the runner build, the stamp of a working tree and how many runs were
//! kept since the app started, so reading it again (on each return to a
//! review, or for the explanations of its failures) derives nothing until
//! a run is kept or the files change.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::analysis::Analysis;
use crate::analyzers::{self, AnalyzerSpec};
use crate::disk_cache;
use crate::error::{app_error, core_text, AppError, AppResult, CoreText};
use crate::flow::{self, Analyzer, FlowService};
use crate::languages;
use crate::model::{
    AnalyzerIdentity, FlowUnavailable, Isolation, ReviewSide, RunOptions, RunSelection, TestProgress,
    TestRun, TestRunStatus, TestsState, TestsUnavailable,
};
use crate::process::{self, Ending, Limits};
use crate::review;
use crate::runner_protocol::{self, TestOutput};
use crate::sandbox;
use crate::single_flight::SingleFlight;
use crate::test_report::{self, ReviewFacts, RunInput, RunRecord, MAX_OUTPUT_BYTES};
use crate::trees::{self, ScratchTree};

/// Runs kept per project and language: the last and the last completed of
/// each commit.
const KEEP_RUN_FILES: usize = 100;
const LAST_SUFFIX: &str = ".json";
const COMPLETED_SUFFIX: &str = ".completed.json";
/// The name the working tree's runs are kept under, in place of a sha.
const WORKING_TREE: &str = "working-tree";
/// In the run folder: what a partial run runs, and how far a run got.
const SELECTION_FILE: &str = "selection.json";
const PROGRESS_FILE: &str = "progress.json";
/// Derived runs kept in memory: a review's, and a few the user went back
/// and forth between. Each holds the new side's analysis.
const KEEP_DERIVED: usize = 3;

/// What a derived run was derived from: the sides of the review, the runner
/// build, the files of a working tree and the runs kept so far. The same key
/// derives the same run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivedKey {
    project: String,
    old_sha: String,
    new_sha: Option<String>,
    fingerprint: String,
    /// The working tree's stamp; None for a commit.
    stamp: Option<u64>,
    /// How many times runs were kept, when it was derived.
    generation: u64,
}

#[cfg(test)]
impl DerivedKey {
    /// The key of a commit run of `project_id` derived after `generation`
    /// runs were kept.
    pub fn for_tests(project_id: &str, generation: u64) -> Self {
        Self {
            project: project_id.to_string(),
            old_sha: "a".repeat(40),
            new_sha: Some("b".repeat(40)),
            fingerprint: "runner".to_string(),
            stamp: None,
            generation,
        }
    }
}

/// The last run of a review's new side as `state` shows it, with the new
/// side's analysis in the run's language it was read with.
#[derive(Debug, Clone)]
pub struct DerivedRun {
    /// None when the sides could not be analysed: such a run is derived
    /// again each time, since the analyses may work by then.
    pub key: Option<DerivedKey>,
    pub run: Arc<TestRun>,
    pub new_side: Option<Arc<Analysis>>,
}

/// What a run is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub isolation: Isolation,
    pub options: RunOptions,
    /// None runs every test.
    pub selection: Option<RunSelection>,
}

impl RunRequest {
    /// Every test, with the race detector and coverage.
    pub fn whole(isolation: Isolation) -> Self {
        Self {
            isolation,
            options: RunOptions::default(),
            selection: None,
        }
    }
}

/// A working tree run as kept on disk, with the stamp of the files it ran on.
#[derive(Debug, Serialize, Deserialize)]
struct KeptWorkingRun {
    stamp: u64,
    record: RunRecord,
}

#[derive(Debug, Clone)]
pub struct TestSettings {
    /// When `prepare` is stopped.
    pub prepare_timeout: Duration,
    /// Passed to the runner as `--timeout`.
    pub runner_timeout: Duration,
    /// When `test` is stopped: a little after `runner_timeout`, so the
    /// runner can report its own timeout.
    pub hard_timeout: Duration,
    pub max_output_bytes: usize,
    /// The `sandbox-exec` to run sandboxed tests with.
    pub sandbox_exec: PathBuf,
    /// Where each run's TMPDIR is made: a short path, see `attempt`.
    pub temporary_root: PathBuf,
}

impl Default for TestSettings {
    fn default() -> Self {
        Self {
            prepare_timeout: Duration::from_secs(600),
            runner_timeout: Duration::from_secs(600),
            hard_timeout: Duration::from_secs(630),
            max_output_bytes: 200 * 1024 * 1024,
            sandbox_exec: PathBuf::from(sandbox::SANDBOX_EXEC),
            temporary_root: short_temporary_root(),
        }
    }
}

/// `/private/tmp` on macOS, where the user's own temporary folder is already
/// about 50 bytes long; the system temporary folder elsewhere.
fn short_temporary_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    }
}

/// One new side of one project: what runs are shared, cancelled and kept by.
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

/// The last working tree runs of one project, with the stamps of the files
/// they ran on.
struct WorkingRuns {
    fingerprint: String,
    last: (RunRecord, Option<u64>),
    completed: Option<(RunRecord, Option<u64>)>,
}

/// A run going on: its cancel flag, what it was asked, and its progress
/// file once its run folder is made.
struct ActiveRun {
    cancel: Arc<AtomicBool>,
    request: RunRequest,
    progress: Arc<Mutex<Option<PathBuf>>>,
}

type ActiveRuns = Mutex<HashMap<RunKey, ActiveRun>>;

pub struct TestsService {
    flow: Arc<FlowService>,
    cache: PathBuf,
    settings: TestSettings,
    /// The record of a run, with what it was asked.
    flights: SingleFlight<RunKey, AppResult<(RunRequest, RunRecord)>>,
    /// Runs going on, with their cancel flags.
    active: ActiveRuns,
    /// By project id.
    working: Mutex<HashMap<String, WorkingRuns>>,
    /// Bumped each time a run is kept; part of `DerivedKey`.
    generation: AtomicU64,
    /// The last derived runs, the latest last.
    derived: Mutex<VecDeque<DerivedRun>>,
    /// Derivations going on, so concurrent reads of one review share one.
    derivations: SingleFlight<DerivedKey, AppResult<Option<DerivedRun>>>,
}

/// How the steps of a run ended.
pub(crate) struct Outcome {
    pub(crate) status: TestRunStatus,
    pub(crate) message: Option<CoreText>,
    pub(crate) output: Option<String>,
}

/// What `prepared_copy` copies and prepares: the new side of a project
/// (`commit` None for the working tree), for `analyzer`, and the folder its
/// mutation engine is installed into, when it needs one.
pub(crate) struct PrepareRequest<'a> {
    pub(crate) project_id: &'a str,
    pub(crate) commit: Option<&'a str>,
    pub(crate) repo: &'a Path,
    pub(crate) analyzer: &'a Analyzer,
    pub(crate) tools: Option<&'a Path>,
}

/// A fresh copy of a new side in a run folder of its own, that `prepare`
/// ran on: where a step of the runner (`test`, `mutate`) runs. Dropping it
/// gives the build cache back and deletes the folders.
pub(crate) struct PreparedCopy<'a> {
    /// Holds `tree`; what the sandbox lets the step write in.
    pub(crate) run_directory: PathBuf,
    /// The copy of the new side, the step's `--dir`.
    pub(crate) tree: PathBuf,
    /// The step's TMPDIR.
    pub(crate) temporary: PathBuf,
    /// The project's build cache, the step's `--cache-dir`.
    pub(crate) cache_directory: PathBuf,
    /// The folders `prepare` reported, and the build cache.
    pub(crate) writable: Vec<PathBuf>,
    // Dropped in this order: the lease, then the folders.
    _build_cache: crate::build_cache::Lease<'a>,
    _temporary_scratch: ScratchTree,
    _scratch: ScratchTree,
}

impl Outcome {
    /// Text from elsewhere (the runner's own output) keeps at most its last
    /// `MAX_OUTPUT_BYTES`; what the core worded fills in output already
    /// capped to that.
    pub(crate) fn failed(message: impl Into<CoreText>) -> Self {
        let mut message = message.into();
        if message.reason.is_none() {
            message.message = test_report::keep_end(&message.message, MAX_OUTPUT_BYTES);
        }
        Self {
            status: TestRunStatus::Failed,
            message: Some(message),
            output: None,
        }
    }

    pub(crate) fn stopped(status: TestRunStatus) -> Self {
        Self {
            status,
            message: None,
            output: None,
        }
    }
}

impl TestsService {
    /// Everything is kept under `cache`. Run folders left by an earlier run
    /// of the app are deleted.
    pub fn new(flow: Arc<FlowService>, cache: PathBuf, settings: TestSettings) -> Self {
        let _ = fs::remove_dir_all(cache.join("test-scratch"));
        Self {
            flow,
            cache,
            settings,
            flights: SingleFlight::new(),
            active: Mutex::new(HashMap::new()),
            working: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            derived: Mutex::new(VecDeque::new()),
            derivations: SingleFlight::new(),
        }
    }

    /// The last run for these sides as `state` shows it, with the new
    /// side's analysis it was read with; None when the project has no test
    /// runner or the new side no run. Runs nothing but analyzers, and
    /// derives nothing when the same was derived since the last run was
    /// kept.
    pub fn last_run(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> AppResult<Option<DerivedRun>> {
        review::check_commits(self.flow.git(), repo, old_sha, new_sha)?;
        match runner_spec(repo) {
            Ok(spec) => self.derived_run(project_id, repo, old_sha, new_sha, spec),
            Err(_) => Ok(None),
        }
    }

    /// The last run for these sides, whether one is going on, and
    /// `isolation`, the project's choice, as given. Runs nothing but
    /// analyzers.
    pub fn state(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        isolation: Isolation,
    ) -> AppResult<TestsState> {
        review::check_commits(self.flow.git(), repo, old_sha, new_sha)?;
        let running_request = self.running_request(project_id, new_sha);
        let running = running_request.is_some();
        let running_selection = running_request.and_then(|request| request.selection);
        let spec = match runner_spec(repo) {
            Ok(spec) => spec,
            Err(languages) => {
                return Ok(TestsState {
                    run: None,
                    running,
                    running_selection,
                    isolation,
                    unavailable: Some(TestsUnavailable::NoRunner { languages }),
                })
            }
        };
        let run = self
            .derived_run(project_id, repo, old_sha, new_sha, spec)?
            .map(|derived| (*derived.run).clone());
        Ok(TestsState {
            run,
            running,
            running_selection,
            isolation,
            unavailable: None,
        })
    }

    /// Runs every test of the new side with the race detector and coverage,
    /// as `run_with`.
    pub fn run(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        isolation: Isolation,
    ) -> AppResult<TestRun> {
        self.run_with(project_id, repo, old_sha, new_sha, &RunRequest::whole(isolation))
    }

    /// Runs the tests of the new side as asked, or joins the same run going
    /// on for it, and returns the run as `state` would. Fails with
    /// `internal` when the project has no test runner, or when another run
    /// of this new side goes on (`test_run_busy`).
    pub fn run_with(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        request: &RunRequest,
    ) -> AppResult<TestRun> {
        review::check_commits(self.flow.git(), repo, old_sha, new_sha)?;
        let spec = runner_spec(repo).map_err(|languages| {
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
        let key = RunKey::new(project_id, new_sha);
        if self
            .running_request(project_id, new_sha)
            .is_some_and(|running| running != *request)
        {
            return Err(busy());
        }
        let abandoned = Err(app_error!(
            Internal,
            TestRunAbandoned,
            "The test run stopped unexpectedly."
        ));
        let (ran, record) = self.flights.run(key.clone(), abandoned, || {
            self.execute(&key, repo, spec, request)
                .map(|record| (request.clone(), record))
        })?;
        // Joined a run asked otherwise, started between the check and here.
        if ran != *request {
            return Err(busy());
        }
        let output = parse_output(&record, spec);
        self.derive(
            project_id,
            repo,
            old_sha,
            new_sha,
            spec,
            &record,
            output.as_ref(),
        )
    }

    /// Stops the run going on for this new side, if any.
    pub fn cancel(&self, project_id: &str, new_sha: Option<&str>) {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = active.get(&RunKey::new(project_id, new_sha)) {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub fn is_running(&self, project_id: &str, new_sha: Option<&str>) -> bool {
        self.running_request(project_id, new_sha).is_some()
    }

    /// What the run going on for this new side was asked, if one goes on.
    fn running_request(&self, project_id: &str, new_sha: Option<&str>) -> Option<RunRequest> {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&RunKey::new(project_id, new_sha))
            .map(|run| run.request.clone())
    }

    /// How far the run going on for this new side got, as its runner last
    /// wrote it; None when none runs or it has not said yet.
    pub fn progress(&self, project_id: &str, new_sha: Option<&str>) -> Option<TestProgress> {
        let file = {
            let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
            let run = active.get(&RunKey::new(project_id, new_sha))?;
            let file = run.progress.lock().unwrap_or_else(PoisonError::into_inner).clone();
            file?
        };
        runner_protocol::parse_progress(&fs::read(file).ok()?)
    }

    /// The last completed run of this runner build on the new side, with
    /// what the runner reported.
    pub fn last_completed(
        &self,
        project_id: &str,
        repo: &Path,
        analyzer: &Analyzer,
        new_sha: Option<&str>,
    ) -> Option<(RunRecord, TestOutput)> {
        let (record, output) = self.last(project_id, repo, analyzer, new_sha, true)?;
        Some((record, output?))
    }

    fn execute(
        &self,
        key: &RunKey,
        repo: &Path,
        spec: &'static AnalyzerSpec,
        request: &RunRequest,
    ) -> AppResult<RunRecord> {
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
        let started_at = flow::now()?;
        let clock = Instant::now();
        // Taken before the copy: files edited during the run make the run
        // older than the working tree. Without one the run is kept in memory
        // only.
        let stamp = match &key.commit {
            Some(_) => None,
            None => trees::working_tree_stamp(self.flow.git(), repo).ok(),
        };
        let analyzer = self.flow.analyzer(spec);
        let ending = match &analyzer {
            Ok(analyzer) => self.attempt(key, repo, analyzer, request, &cancel, &progress)?,
            Err(message) => Outcome::failed(message.clone()),
        };
        let record = RunRecord {
            isolation: request.isolation,
            started_at,
            duration_ms: u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX),
            status: ending.status,
            message: ending.message,
            output: ending.output,
            options: request.options,
            partial: None,
        };
        match &analyzer {
            Ok(analyzer) => Ok(match &request.selection {
                None => {
                    self.keep(key, analyzer, &record, stamp);
                    record
                }
                Some(selection) => self.keep_partial(key, repo, analyzer, record, selection, stamp),
            }),
            Err(_) => Ok(record),
        }
    }

    /// Keeps a partial run: merged into the last completed run when it
    /// completed, or noted on the last run when it did not. Returns the run
    /// to show.
    fn keep_partial(
        &self,
        key: &RunKey,
        repo: &Path,
        analyzer: &Analyzer,
        record: RunRecord,
        selection: &RunSelection,
        stamp: Option<u64>,
    ) -> RunRecord {
        let ran = parse_output(&record, analyzer.spec);
        let partial = test_report::partial_run(&record, selection, ran.as_ref());
        let new_sha = key.commit.as_deref();
        if record.status != TestRunStatus::Completed {
            let Some((mut last, last_stamp)) = self.kept(&key.project, repo, analyzer, new_sha, false) else {
                return RunRecord {
                    partial: Some(partial),
                    ..record
                };
            };
            last.partial = Some(partial);
            last.options = record.options;
            // Kept again as the last only: the last completed is as it was.
            self.write_kept(key, analyzer, &last, last_stamp, false);
            return last;
        }
        let base = self
            .kept(&key.project, repo, analyzer, new_sha, true)
            .and_then(|(base, base_stamp)| Some((parse_output(&base, analyzer.spec)?, base, base_stamp)));
        let merged = match (base, ran) {
            (Some((base_output, base, base_stamp)), Some(ran)) => {
                let same_code = key.commit.is_some() || (stamp.is_some() && base_stamp == stamp);
                let merged = test_report::merge(&base_output, &ran, selection, same_code);
                RunRecord {
                    output: serde_json::to_string(&merged).ok().or(base.output.clone()),
                    options: record.options,
                    partial: Some(partial),
                    ..base
                }
            }
            // Nothing to merge into: the partial run is all there is.
            _ => RunRecord {
                partial: Some(partial),
                ..record
            },
        };
        self.keep(key, analyzer, &merged, stamp);
        merged
    }

    /// Copies the new side into a fresh run folder, prepares and runs the
    /// tests there, and deletes the folder.
    fn attempt(
        &self,
        key: &RunKey,
        repo: &Path,
        analyzer: &Analyzer,
        request: &RunRequest,
        cancel: &AtomicBool,
        progress: &Mutex<Option<PathBuf>>,
    ) -> AppResult<Outcome> {
        let copy = PrepareRequest {
            project_id: &key.project,
            commit: key.commit.as_deref(),
            repo,
            analyzer,
            tools: None,
        };
        let on_folder = |run_directory: &Path| {
            *progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(run_directory.join(PROGRESS_FILE));
        };
        let copy = match self.prepared_copy(&copy, cancel, on_folder)? {
            Ok(copy) => copy,
            Err(ending) => return Ok(ending),
        };
        let mut arguments: Vec<OsString> = vec![
            "test".into(),
            "--dir".into(),
            copy.tree.clone().into(),
            "--timeout".into(),
            format!("{}s", self.settings.runner_timeout.as_secs().max(1)).into(),
            "--cache-dir".into(),
            copy.cache_directory.clone().into(),
            "--progress".into(),
            copy.run_directory.join(PROGRESS_FILE).into(),
        ];
        if !request.options.race {
            arguments.push("--no-race".into());
        }
        if !request.options.coverage {
            arguments.push("--no-cover".into());
        }
        if let Some(selection) = &request.selection {
            let file = copy.run_directory.join(SELECTION_FILE);
            let bytes = serde_json::to_vec(selection).unwrap_or_default();
            fs::write(&file, bytes).map_err(|error| disk_error(&file, &error))?;
            arguments.push("--selection".into());
            arguments.push(file.into());
        }
        let command = match self.step_command(analyzer, &copy, request.isolation, arguments) {
            Ok(command) => command,
            Err(ending) => return Ok(ending),
        };
        Ok(self.test(analyzer, command, &copy.tree, &copy.temporary, cancel))
    }

    /// Copies the new side into a fresh run folder and runs `prepare` there
    /// (with `--tools` when asked), as the first steps of a test run;
    /// `on_folder` learns the run folder as soon as it is made. The copy, or
    /// how the run ends (cancelled meanwhile, `prepare` failed). The folders
    /// are deleted and the build cache given back when the copy is dropped.
    pub(crate) fn prepared_copy(
        &self,
        request: &PrepareRequest<'_>,
        cancel: &AtomicBool,
        on_folder: impl FnOnce(&Path),
    ) -> AppResult<Result<PreparedCopy<'_>, Outcome>> {
        let scratch = ScratchTree::create(&self.cache.join("test-scratch"))?;
        // The sandbox compares resolved paths.
        let run_directory = sandbox::canonical(scratch.path());
        let tree = run_directory.join("tree");
        fs::create_dir(&tree).map_err(|error| disk_error(&tree, &error))?;
        on_folder(&run_directory);
        // The run's TMPDIR is a short folder of its own, not one inside the
        // run folder: tests put Unix sockets under it (Go's t.TempDir()), and
        // a socket path longer than 104 bytes cannot be bound on macOS. The
        // cache path alone is about 100.
        let temporary_scratch = ScratchTree::create_short(&self.settings.temporary_root, "dl-")?;
        let temporary = sandbox::canonical(temporary_scratch.path());
        let git = self.flow.git();
        match request.commit {
            Some(sha) => git.archive(request.repo, sha, |archive| trees::extract_tar(archive, &tree))?,
            None => trees::copy_working_tree(git, request.repo, &tree)?,
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Err(Outcome::stopped(TestRunStatus::Cancelled)));
        }

        // Held for the whole run, so it is not emptied under the build.
        let build_cache = self
            .flow
            .build_caches()
            .lease(request.project_id, request.analyzer.spec.id)?;
        let cache_directory = sandbox::canonical(build_cache.path());
        let mut writable =
            match self.prepare(request.analyzer, &tree, &cache_directory, request.tools, &temporary, cancel) {
                Ok(writable) => writable,
                Err(ending) => return Ok(Err(ending)),
            };
        writable.push(cache_directory.clone());
        Ok(Ok(PreparedCopy {
            run_directory,
            tree,
            temporary,
            cache_directory,
            writable,
            _build_cache: build_cache,
            _temporary_scratch: temporary_scratch,
            _scratch: scratch,
        }))
    }

    /// The command running the analyzer with `arguments` in `copy`, isolated
    /// as asked: under the sandbox it gets `--offline`, and writes only to
    /// the run folder, its TMPDIR and the folders `prepare` reported.
    pub(crate) fn step_command(
        &self,
        analyzer: &Analyzer,
        copy: &PreparedCopy<'_>,
        isolation: Isolation,
        mut arguments: Vec<OsString>,
    ) -> Result<Command, Outcome> {
        match isolation {
            Isolation::Process => Ok(analyzer.launcher.command(&arguments)),
            Isolation::Sandbox => {
                if !cfg!(target_os = "macos") {
                    return Err(Outcome::failed(core_text!(
                        SandboxUnavailable,
                        "The sandbox is only available on macOS; run the tests as a process."
                    )));
                }
                arguments.push("--offline".into());
                let policy = sandbox::Policy {
                    run_directory: copy.run_directory.clone(),
                    temporary: copy.temporary.clone(),
                    writable: copy.writable.clone(),
                };
                Ok(analyzer
                    .launcher
                    .sandboxed(&self.settings.sandbox_exec, &policy, &arguments))
            }
        }
    }

    /// Runs `prepare` (with `--tools` when given); the folders it reported,
    /// or how the run ends.
    fn prepare(
        &self,
        analyzer: &Analyzer,
        tree: &Path,
        cache_directory: &Path,
        tools: Option<&Path>,
        temporary: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<PathBuf>, Outcome> {
        let title = analyzer.spec.title();
        let command = analyzer
            .launcher
            .command(&analyzers::prepare_arguments(tree, cache_directory, tools));
        let limits = Limits {
            timeout: self.settings.prepare_timeout,
            max_stdout_bytes: 1024 * 1024,
            stderr_tail_bytes: MAX_OUTPUT_BYTES,
        };
        let captured = self.spawn(command, tree, temporary, &limits, cancel, analyzer.spec)?;
        match captured.ending {
            Ending::Exited(status) if status.success() => {
                runner_protocol::parse_prepare(&captured.stdout, &title)
                    .map(|prepared| prepared.writable.into_iter().map(PathBuf::from).collect())
                    .map_err(Outcome::failed)
            }
            Ending::Exited(status) => Err(Outcome::failed(core_text!(
                PrepareFailed,
                "{analyzer} could not prepare the tests ({status}):\n{detail}",
                analyzer = title,
                status = status.to_string(),
                detail = match text_of(&captured.stderr_tail) {
                    Some(stderr) => CoreText::raw(stderr),
                    None => core_text!(NoDetailsReported, "It reported no details."),
                }
            ))),
            Ending::OutputTooLarge => Err(Outcome::failed(core_text!(
                PreparePrintedTooMuch,
                "{analyzer} printed too much while preparing the tests.",
                analyzer = title
            ))),
            Ending::TimedOut => Err(Outcome::stopped(TestRunStatus::TimedOut)),
            Ending::Cancelled => Err(Outcome::stopped(TestRunStatus::Cancelled)),
        }
    }

    fn test(
        &self,
        analyzer: &Analyzer,
        command: Command,
        tree: &Path,
        temporary: &Path,
        cancel: &AtomicBool,
    ) -> Outcome {
        let title = analyzer.spec.title();
        let limits = Limits {
            timeout: self.settings.hard_timeout,
            max_stdout_bytes: self.settings.max_output_bytes,
            stderr_tail_bytes: MAX_OUTPUT_BYTES,
        };
        let captured = match self.spawn(command, tree, temporary, &limits, cancel, analyzer.spec) {
            Ok(captured) => captured,
            Err(ending) => return ending,
        };
        match captured.ending {
            Ending::Exited(status) if status.success() => {
                match runner_protocol::parse_tests(&captured.stdout, analyzer.spec.languages, &title)
                {
                    Ok(_) => Outcome {
                        status: TestRunStatus::Completed,
                        message: None,
                        output: String::from_utf8(captured.stdout).ok(),
                    },
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
            Ending::OutputTooLarge => Outcome::failed(analyzers::printed_too_much(
                &title,
                self.settings.max_output_bytes / (1024 * 1024),
            )),
            Ending::TimedOut => Outcome::stopped(TestRunStatus::TimedOut),
            Ending::Cancelled => Outcome::stopped(TestRunStatus::Cancelled),
        }
    }

    /// Runs one step in the copy, with the tool PATH and the run's TMPDIR.
    pub(crate) fn spawn(
        &self,
        mut command: Command,
        tree: &Path,
        temporary: &Path,
        limits: &Limits,
        cancel: &AtomicBool,
        spec: &AnalyzerSpec,
    ) -> Result<process::Captured, Outcome> {
        command
            .current_dir(tree)
            .env("PATH", self.flow.tool_path())
            .env("TMPDIR", temporary);
        let program = PathBuf::from(command.get_program());
        process::run_captured_until(command, limits, cancel).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound && program == self.settings.sandbox_exec {
                Outcome::failed(core_text!(
                    ProgramMissing,
                    "{path} is missing.",
                    path = program.display()
                ))
            } else {
                Outcome::failed(analyzers::not_started(spec, &error))
            }
        })
    }

    /// Keeps `record` as the last run of its new side, and as the last
    /// completed one when it is; a working tree run with `stamp`, the stamp
    /// of the files it ran on. A run that cannot be kept is only lost.
    fn keep(&self, key: &RunKey, analyzer: &Analyzer, record: &RunRecord, stamp: Option<u64>) {
        let completed = record.status == TestRunStatus::Completed;
        self.write_kept(key, analyzer, record, stamp, completed);
    }

    /// Keeps `record` as the last run, and as the last completed one too
    /// when `completed`.
    fn write_kept(
        &self,
        key: &RunKey,
        analyzer: &Analyzer,
        record: &RunRecord,
        stamp: Option<u64>,
        completed: bool,
    ) {
        let write = |name: &str, bytes: &[u8]| {
            let folder = self.run_folder(&key.project, analyzer);
            let _ = disk_cache::write(
                &folder.join(format!("{name}{LAST_SUFFIX}")),
                bytes,
                KEEP_RUN_FILES,
            );
            if completed {
                let _ = disk_cache::write(
                    &folder.join(format!("{name}{COMPLETED_SUFFIX}")),
                    bytes,
                    KEEP_RUN_FILES,
                );
            }
        };
        match &key.commit {
            Some(sha) => {
                if let Ok(bytes) = serde_json::to_vec(record) {
                    write(sha, &bytes);
                }
            }
            None => {
                if let Some(stamp) = stamp {
                    let kept = KeptWorkingRun {
                        stamp,
                        record: record.clone(),
                    };
                    if let Ok(bytes) = serde_json::to_vec(&kept) {
                        write(WORKING_TREE, &bytes);
                    }
                }
                let mut working = self.working.lock().unwrap_or_else(PoisonError::into_inner);
                let previous = working
                    .remove(&key.project)
                    .filter(|kept| kept.fingerprint == analyzer.fingerprint)
                    .and_then(|kept| kept.completed);
                working.insert(
                    key.project.clone(),
                    WorkingRuns {
                        fingerprint: analyzer.fingerprint.clone(),
                        last: (record.clone(), stamp),
                        completed: if completed {
                            Some((record.clone(), stamp))
                        } else {
                            previous
                        },
                    },
                );
            }
        }
        // After the run is in place: see `derived_run`.
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The last run (or the last completed one) of this runner build on the
    /// new side, with its parsed output when it has one. A kept run whose
    /// output this version cannot read counts as missing. For the working
    /// tree, the one this run of the app made, or else the one kept on disk
    /// while the files of `repo` still have the stamp it ran on.
    fn last(
        &self,
        project_id: &str,
        repo: &Path,
        analyzer: &Analyzer,
        new_sha: Option<&str>,
        completed: bool,
    ) -> Option<(RunRecord, Option<TestOutput>)> {
        let (record, _) = self.kept(project_id, repo, analyzer, new_sha, completed)?;
        let output = parse_output(&record, analyzer.spec);
        if record.status == TestRunStatus::Completed && output.is_none() {
            return None;
        }
        Some((record, output))
    }

    /// The kept record `last` reads, with the stamp of the files a working
    /// tree run ran on (None for a commit's, or one kept in memory only).
    fn kept(
        &self,
        project_id: &str,
        repo: &Path,
        analyzer: &Analyzer,
        new_sha: Option<&str>,
        completed: bool,
    ) -> Option<(RunRecord, Option<u64>)> {
        let suffix = if completed {
            COMPLETED_SUFFIX
        } else {
            LAST_SUFFIX
        };
        match new_sha {
            Some(sha) => {
                let file = self
                    .run_folder(project_id, analyzer)
                    .join(format!("{sha}{suffix}"));
                let record: RunRecord = serde_json::from_slice(&fs::read(&file).ok()?).ok()?;
                disk_cache::touch(&file);
                Some((record, None))
            }
            None => {
                let remembered = {
                    let working = self.working.lock().unwrap_or_else(PoisonError::into_inner);
                    working
                        .get(project_id)
                        .filter(|kept| kept.fingerprint == analyzer.fingerprint)
                        .and_then(|kept| {
                            if completed {
                                kept.completed.clone()
                            } else {
                                Some(kept.last.clone())
                            }
                        })
                };
                match remembered {
                    Some(remembered) => Some(remembered),
                    None => {
                        let file = self
                            .run_folder(project_id, analyzer)
                            .join(format!("{WORKING_TREE}{suffix}"));
                        let kept: KeptWorkingRun =
                            serde_json::from_slice(&fs::read(&file).ok()?).ok()?;
                        if kept.stamp != trees::working_tree_stamp(self.flow.git(), repo).ok()? {
                            return None;
                        }
                        disk_cache::touch(&file);
                        Some((kept.record, Some(kept.stamp)))
                    }
                }
            }
        }
    }

    fn run_folder(&self, project_id: &str, analyzer: &Analyzer) -> PathBuf {
        self.cache
            .join("test-runs")
            .join(project_id)
            .join(analyzer.spec.id)
            .join(&analyzer.fingerprint)
    }

    /// The last run of this runner on the new side, derived for the review
    /// from `old_sha` to `new_sha`, or as derived before for the same key.
    fn derived_run(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        spec: &'static AnalyzerSpec,
    ) -> AppResult<Option<DerivedRun>> {
        let Ok(analyzer) = self.flow.analyzer(spec) else {
            return Ok(None);
        };
        // Read before the run: a run kept meanwhile bumps it after it is
        // written, so what is derived from an older run is never kept under
        // a newer generation.
        let generation = self.generation.load(Ordering::SeqCst);
        let stamp = match new_sha {
            Some(_) => None,
            None => match trees::working_tree_stamp(self.flow.git(), repo) {
                Ok(stamp) => Some(stamp),
                Err(_) => return self.derive_last(project_id, repo, old_sha, new_sha, &analyzer, None),
            },
        };
        let key = DerivedKey {
            project: project_id.to_string(),
            old_sha: old_sha.to_string(),
            new_sha: new_sha.map(str::to_string),
            fingerprint: analyzer.fingerprint.clone(),
            stamp,
            generation,
        };
        if let Some(found) = self.remembered_run(&key) {
            return Ok(Some(found));
        }
        let abandoned = Err(app_error!(
            Internal,
            AnalysisAbandoned,
            "The analysis stopped unexpectedly."
        ));
        self.derivations.run(key.clone(), abandoned, || {
            // The derivation that just landed may have kept it.
            if let Some(found) = self.remembered_run(&key) {
                return Ok(Some(found));
            }
            let derived =
                self.derive_last(project_id, repo, old_sha, new_sha, &analyzer, Some(&key))?;
            if let Some(derived) = derived.as_ref().filter(|derived| derived.key.is_some()) {
                let mut kept = self.derived.lock().unwrap_or_else(PoisonError::into_inner);
                kept.retain(|other| other.key != derived.key);
                kept.push_back(derived.clone());
                while kept.len() > KEEP_DERIVED {
                    kept.pop_front();
                }
            }
            Ok(derived)
        })
    }

    fn remembered_run(&self, key: &DerivedKey) -> Option<DerivedRun> {
        let kept = self.derived.lock().unwrap_or_else(PoisonError::into_inner);
        kept.iter()
            .rev()
            .find(|derived| derived.key.as_ref() == Some(key))
            .cloned()
    }

    /// Derives the last run of `analyzer` on the new side, under `key` when
    /// the sides could be analysed.
    fn derive_last(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        analyzer: &Analyzer,
        key: Option<&DerivedKey>,
    ) -> AppResult<Option<DerivedRun>> {
        let Some((record, output)) = self.last(project_id, repo, analyzer, new_sha, false) else {
            return Ok(None);
        };
        let (run, new_side) = self.derive_with_side(
            project_id,
            repo,
            old_sha,
            new_sha,
            analyzer.spec,
            &record,
            output.as_ref(),
        )?;
        Ok(Some(DerivedRun {
            key: new_side.is_some().then(|| key.cloned()).flatten(),
            run: Arc::new(run),
            new_side,
        }))
    }

    /// The `TestRun` of `record` in the review from `old_sha` to `new_sha`.
    #[allow(clippy::too_many_arguments)]
    fn derive(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        spec: &'static AnalyzerSpec,
        record: &RunRecord,
        output: Option<&TestOutput>,
    ) -> AppResult<TestRun> {
        self.derive_with_side(project_id, repo, old_sha, new_sha, spec, record, output)
            .map(|(run, _)| run)
    }

    /// As `derive`, with the new side's analysis in the run's language; None
    /// when the sides could not be analysed.
    #[allow(clippy::too_many_arguments)]
    fn derive_with_side(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        spec: &'static AnalyzerSpec,
        record: &RunRecord,
        output: Option<&TestOutput>,
    ) -> AppResult<(TestRun, Option<Arc<Analysis>>)> {
        let sides = self.flow.sides(project_id, repo, old_sha, new_sha)?;
        let mut diagnostics = Vec::new();
        let separate_lines;
        let (lines, pair) = match &sides {
            Ok(sides) => {
                let pair = sides
                    .analyzers
                    .iter()
                    .position(|analyzer| analyzer.spec.id == spec.id)
                    .map(|index| (&*sides.old[index], &*sides.new[index]));
                (&sides.lines, pair)
            }
            Err(unavailable) => {
                diagnostics.push(unknown_functions(unavailable));
                separate_lines = self.flow.changed_lines(repo, old_sha, new_sha)?;
                (&separate_lines, None)
            }
        };
        let facts = pair.map(|(old, new)| ReviewFacts::new(old, new, lines));
        let runner = pair.map_or_else(
            || AnalyzerIdentity {
                name: spec.name.to_string(),
                version: String::new(),
            },
            |(_, new)| new.analyzer.clone(),
        );
        // What the runner or the analysis reported, as an analyzer may cover
        // several languages.
        let language = output
            .map(|output| output.language.as_str())
            .or(pair.map(|(_, new)| new.language.as_str()))
            .unwrap_or(spec.languages[0]);
        let run = test_report::derive(RunInput {
            language,
            runner,
            old_sha,
            new_sha,
            record,
            output,
            facts: facts.as_ref(),
            lines,
            diagnostics,
        });
        let new_side = match &sides {
            Ok(sides) => sides
                .analyzers
                .iter()
                .position(|analyzer| analyzer.spec.id == spec.id)
                .map(|index| Arc::clone(&sides.new[index])),
            Err(_) => None,
        };
        Ok((run, new_side))
    }
}

/// The first of the project's languages whose analyzer runs tests, or the
/// languages found when none does.
pub(crate) fn runner_spec(repo: &Path) -> Result<&'static AnalyzerSpec, Vec<String>> {
    let detected = languages::detect(repo);
    analyzers::for_languages(&detected)
        .into_iter()
        .find(|spec| spec.tests)
        .ok_or(detected)
}

fn parse_output(record: &RunRecord, spec: &AnalyzerSpec) -> Option<TestOutput> {
    let text = record.output.as_deref()?;
    runner_protocol::parse_tests(text.as_bytes(), spec.languages, spec.title()).ok()
}

/// Why the tests' functions are unknown, as a diagnostic.
fn unknown_functions(unavailable: &FlowUnavailable) -> CoreText {
    match unavailable {
        FlowUnavailable::NoAnalyzer { .. } => core_text!(
            NoAnalyzerForTests,
            "No analyzer could tell which functions the review changed."
        ),
        FlowUnavailable::AnalysisFailed {
            side: ReviewSide::Old,
            message,
        } => core_text!(
            OldSideUnanalysed,
            "The old side could not be analysed, so test states are unknown: {detail}",
            detail = message.clone()
        ),
        FlowUnavailable::AnalysisFailed {
            side: ReviewSide::New,
            message,
        }
        // Neither side could be analysed: the analyzer is not there.
        | FlowUnavailable::AnalyzerUnavailable { message, .. } => core_text!(
            NewSideUnanalysed,
            "The new side could not be analysed, so test states are unknown: {detail}",
            detail = message.clone()
        ),
    }
}

/// The text of `bytes` trimmed, when there is any.
pub(crate) fn text_of(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn busy() -> AppError {
    app_error!(
        Internal,
        TestRunBusy,
        "Another test run of this review is going on. Wait for it to end."
    )
}

pub(crate) fn disk_error(path: &Path, error: &io::Error) -> AppError {
    app_error!(
        Internal,
        FileWriteFailed,
        "Could not write {path}: {detail}",
        path = path.display(),
        detail = error
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
pub mod fixtures {
    //! A stand-in analyzer that also runs tests and checks, and a stand-in
    //! `sandbox-exec`.

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn executable(path: PathBuf, script: String) -> PathBuf {
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Prints `fixture/analysis.json`, `fixture/tests.json` or
    /// `fixture/checks.json` of the tree it is given, records what it was
    /// asked next to itself (`check` also whether `prepare` ran on its tree,
    /// in `check-tree`), and misbehaves while a flag file is there
    /// (`slow`, `hang`, `test-fail`, `prepare-fail`, `garbage`,
    /// `check-fail`), so one commit can be run several ways. `test` writes
    /// a fixed progress file when given `--progress`, records the file given
    /// to `--selection` as `selection`, and then prints
    /// `fixture/tests-partial.json` when the tree has one. `mutate` keeps the
    /// files given to `--changed` and `--coverage` (`mutate-changed`,
    /// `mutate-coverage`), writes a fixed progress file, waits while
    /// `mutate-hang` is there, fails with `mutate-fail`, and prints
    /// `fixture/mutation.json`.
    pub fn fake_runner(tools: &Path) -> PathBuf {
        let t = tools.display();
        let script = format!(
            r#"#!/bin/sh
t='{t}'
command="$1"; dir="$3"
echo "$command" >> "$t/calls"
case "$command" in
  analyze) cat "$dir/fixture/analysis.json" ;;
  prepare)
    printf '%s\n' "$*" > "$t/prepare-args"
    if [ -f "$t/prepare-fail" ]; then echo 'go: download failed' >&2; exit 1; fi
    mkdir -p "$dir/installed-by-prepare"
    printf '{{"protocol":1,"writable":["%s/gocache","relative"]}}' "$t" ;;
  test)
    printf '%s\n' "$*" > "$t/test-args"
    progress=''; selection=''
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --progress) progress="$2"; shift ;;
        --selection) selection="$2"; shift ;;
      esac
      shift
    done
    if [ -n "$progress" ]; then
      printf '{{"protocol":1,"packagesDone":1,"packagesTotal":2,"tests":3,"failed":1,"current":"pkg"}}' > "$progress"
    fi
    if [ -n "$selection" ]; then cat "$selection" > "$t/selection"; fi
    pwd -P > "$t/test-cwd"
    printf '%s' "$TMPDIR" > "$t/test-tmpdir"
    echo written > "$dir/written-by-tests"
    if [ -f "$t/slow" ]; then sleep 1; fi
    if [ -f "$t/hang" ]; then sleep 30 & echo $! > "$t/child.pid"; wait; fi
    if [ -f "$t/test-fail" ]; then echo 'compiling' >&2; echo 'cannot run tests' >&2; exit 3; fi
    if [ -f "$t/garbage" ]; then echo 'not json'; exit 0; fi
    if [ -n "$selection" ] && [ -f "$dir/fixture/tests-partial.json" ]; then
      cat "$dir/fixture/tests-partial.json"
    else
      cat "$dir/fixture/tests.json"
    fi ;;
  check)
    printf '%s\n' "$*" > "$t/check-args"
    if [ -d "$dir/installed-by-prepare" ]; then echo prepared > "$t/check-tree"; else echo bare > "$t/check-tree"; fi
    if [ -f "$t/check-fail" ]; then echo 'vet crashed' >&2; exit 2; fi
    cat "$dir/fixture/checks.json" ;;
  dupes)
    printf '%s\n' "$*" > "$t/dupes-args"
    while [ "$#" -gt 0 ]; do
      case "$1" in --changed) cp "$2" "$t/dupes-changed" ;; esac
      shift
    done
    if [ -f "$t/dupes-fail" ]; then echo 'tokenizer crashed' >&2; exit 1; fi
    if [ -f "$dir/fixture/dupes.json" ]; then
      cat "$dir/fixture/dupes.json"
    else
      printf '{{"protocol":1,"language":"go","runner":{{"name":"difflens-analyzer-go","version":"0.0.1"}},"minTokens":50,"minLines":6,"duplicates":[],"diagnostics":[]}}'
    fi ;;
  mutate)
    printf '%s\n' "$*" > "$t/mutate-args"
    progress=''
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --changed) cp "$2" "$t/mutate-changed"; shift ;;
        --coverage) cp "$2" "$t/mutate-coverage"; shift ;;
        --progress) progress="$2"; shift ;;
      esac
      shift
    done
    if [ -n "$progress" ]; then
      printf '{{"protocol":1,"phase":"mutating","total":3,"done":1,"killed":1,"survived":0,"timeout":0,"error":0,"current":"a.go:3"}}' > "$progress"
    fi
    if [ -f "$t/mutate-hang" ]; then sleep 30 & echo $! > "$t/child.pid"; wait; fi
    if [ -f "$t/mutate-fail" ]; then echo 'mutator crashed' >&2; exit 1; fi
    cat "$dir/fixture/mutation.json" ;;
  *) echo "unexpected: $*" >&2; exit 64 ;;
esac
"#
        );
        executable(tools.join("fake analyzer"), script)
    }

    /// Records its arguments in `sandbox-args`, then runs what follows `--`.
    pub fn fake_sandbox_exec(tools: &Path) -> PathBuf {
        let t = tools.display();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{t}/sandbox-args'
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
exec "$@"
"#
        );
        executable(tools.join("fake sandbox-exec"), script)
    }

    /// How many times the fake was asked `command`.
    pub fn calls(tools: &Path, command: &str) -> usize {
        fs::read_to_string(tools.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == command)
            .count()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fixtures::{calls, fake_runner, fake_sandbox_exec};
    use super::*;
    use crate::analysis::fixtures::{analysis, call, function, in_test};
    use crate::analysis::Analysis;
    use crate::analyzers::{AnalyzerLocator, RunSettings};
    use crate::error::ErrorCode;
    use crate::model::{TestFunctionState, TestOutcome};
    use crate::runner_protocol::fixtures::{block, failing, output, test};
    use crate::test_support::{isolated_git, TestRepo};
    use crate::tool_path::ToolPath;
    use std::process::Stdio;
    use std::thread;
    use tempfile::TempDir;

    const PROJECT: &str = "project-1";

    struct Harness {
        repo: TestRepo,
        tools: TempDir,
        cache: TempDir,
        flow: Arc<FlowService>,
        service: TestsService,
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

    impl Harness {
        fn new() -> Self {
            let tools = TempDir::new().unwrap();
            let settings = settings(tools.path());
            Self::with(tools, settings)
        }

        fn with(tools: TempDir, settings: TestSettings) -> Self {
            let runner = fake_runner(tools.path());
            let cache = TempDir::new().unwrap();
            let flow = Arc::new(FlowService::new(
                isolated_git(),
                cache.path().to_path_buf(),
                AnalyzerLocator::with_executable("go", runner),
                ToolPath::fixed("/usr/bin:/bin"),
                RunSettings::default(),
            ));
            let service =
                TestsService::new(Arc::clone(&flow), cache.path().to_path_buf(), settings);
            Self {
                repo: TestRepo::init(),
                tools,
                cache,
                flow,
                service,
            }
        }

        /// The same caches and runner, as after a restart of the app.
        fn restarted(&self) -> TestsService {
            let flow = Arc::new(FlowService::new(
                isolated_git(),
                self.cache.path().to_path_buf(),
                AnalyzerLocator::with_executable("go", self.tools.path().join("fake analyzer")),
                ToolPath::fixed("/usr/bin:/bin"),
                RunSettings::default(),
            ));
            TestsService::new(flow, self.cache.path().to_path_buf(), settings(self.tools.path()))
        }

        fn flag(&self, name: &str, on: bool) {
            let path = self.tools.path().join(name);
            if on {
                fs::write(path, "").unwrap();
            } else {
                let _ = fs::remove_file(path);
            }
        }

        fn write(&self, files: &[(&str, &str)], analysis: &Analysis, tests: &TestOutput) {
            for (path, contents) in files {
                self.repo.write(path, contents);
            }
            self.repo.write(
                "fixture/analysis.json",
                serde_json::to_string(analysis).unwrap(),
            );
            self.repo
                .write("fixture/tests.json", serde_json::to_string(tests).unwrap());
        }

        fn commit(&self, files: &[(&str, &str)], analysis: &Analysis, tests: &TestOutput) -> String {
            self.write(files, analysis, tests);
            self.repo.commit_all("commit");
            self.repo.git(&["rev-parse", "HEAD"])
        }

        fn run(&self, old: &str, new: Option<&str>, isolation: Isolation) -> TestRun {
            self.service
                .run(PROJECT, self.repo.path(), old, new, isolation)
                .unwrap()
        }

        fn state(&self, service: &TestsService, old: &str, new: Option<&str>) -> TestsState {
            service
                .state(PROJECT, self.repo.path(), old, new, Isolation::Sandbox)
                .unwrap()
        }

        fn read_tool(&self, name: &str) -> String {
            fs::read_to_string(self.tools.path().join(name)).unwrap_or_default()
        }
    }

    fn numbered_lines(count: u32) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    fn old_side() -> Analysis {
        analysis(vec![function("p.A", "a.go", 1, 10)], Vec::new())
    }

    /// p.A changed at line 3, a new test p.TestA calling it.
    fn new_side() -> Analysis {
        analysis(
            vec![
                function("p.A", "a.go", 1, 10),
                in_test(function("p.TestA", "a_test.go", 1, 5)),
            ],
            vec![call("p.TestA", "p.A", 2)],
        )
    }

    fn reported() -> TestOutput {
        let mut sub = test("TestA/declines_politely", Some("p.TestA"));
        sub.outcome = TestOutcome::Skip;
        output(
            vec![test("TestA", Some("p.TestA")), sub],
            Some(vec![block("a.go", 2, 4, 1), block("a.go", 5, 6, 0)]),
        )
    }

    /// A Go project with two commits: a.go line 3 edited, a test added.
    fn two_commits(harness: &Harness) -> (String, String) {
        let old = harness.commit(
            &[("go.mod", "module example.com/p\n"), ("a.go", &numbered_lines(10))],
            &old_side(),
            &reported(),
        );
        let new = harness.commit(
            &[
                ("a.go", &numbered_lines(10).replace("line 3\n", "line three\n")),
                ("a_test.go", &numbered_lines(5)),
            ],
            &new_side(),
            &reported(),
        );
        (old, new)
    }

    #[test]
    fn runs_on_a_fresh_copy_and_derives_the_run_from_the_review() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let status_before = harness.repo.git(&["status", "--porcelain"]);

        let run = harness.run(&old, Some(&new), Isolation::Process);

        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        assert_eq!(run.isolation, Isolation::Process);
        assert_eq!((run.old_sha.as_str(), run.new_sha.as_deref()), (old.as_str(), Some(new.as_str())));
        let rows: Vec<_> = run
            .tests
            .iter()
            .map(|t| (t.title.as_str(), t.state, t.exercises.as_ref().map(|f| f.id.as_str())))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("TestA", TestFunctionState::New, Some("p.A")),
                ("declines politely", TestFunctionState::New, Some("p.A")),
            ]
        );
        assert_eq!((run.summary.passed, run.summary.skipped, run.summary.added), (1, 1, 2));
        let coverage = run.coverage.as_ref().unwrap();
        assert_eq!((coverage.changed_lines.covered, coverage.changed_lines.coverable), (1, 1));
        assert_eq!((coverage.project.covered, coverage.project.coverable), (3, 5));

        // prepare, then test, both in a fresh copy inside a run folder of
        // the cache, with a short TMPDIR of its own; no --offline as a
        // process.
        let scratch = fs::canonicalize(harness.cache.path()).unwrap().join("test-scratch");
        let cwd = PathBuf::from(harness.read_tool("test-cwd").trim());
        assert!(cwd.starts_with(&scratch), "{cwd:?}");
        assert_eq!(cwd.file_name().unwrap(), "tree");
        let temporary_root = fs::canonicalize(harness.tools.path().join("tmp-root")).unwrap();
        let tmpdir = PathBuf::from(harness.read_tool("test-tmpdir"));
        assert_eq!(tmpdir.parent(), Some(temporary_root.as_path()));
        let name = tmpdir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("dl-") && name.len() == 11, "{name}");
        assert_eq!(fs::read_dir(&temporary_root).unwrap().count(), 0, "TMPDIR is deleted");
        let arguments = harness.read_tool("test-args");
        // The progress file is in the run folder; no option is turned off;
        // the build cache is the project's, kept outside the run folder.
        let build_cache = sandbox::canonical(&harness.cache.path().join("build-cache").join(PROJECT).join("go"));
        assert!(build_cache.is_dir(), "kept after the run");
        assert_eq!(
            arguments.trim(),
            format!(
                "test --dir {} --timeout 600s --cache-dir {} --progress {}",
                cwd.display(),
                build_cache.display(),
                cwd.parent().unwrap().join("progress.json").display()
            )
        );
        assert_eq!(
            harness.read_tool("prepare-args").trim(),
            format!("prepare --dir {} --cache-dir {}", cwd.display(), build_cache.display())
        );
        assert!(!harness.tools.path().join("sandbox-args").exists());
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0, "the copy is deleted");
        // What the tests wrote is nowhere else: not in the shared trees, not
        // in the repository.
        let trees = harness.cache.path().join("trees");
        let leaked = std::process::Command::new("find")
            .arg(&trees)
            .args(["-name", "written-by-tests"])
            .output()
            .unwrap();
        assert!(leaked.stdout.is_empty(), "{}", String::from_utf8_lossy(&leaked.stdout));
        assert!(!harness.repo.path().join("written-by-tests").exists());
        assert_eq!(harness.repo.git(&["status", "--porcelain"]), status_before);

        let state = harness.state(&harness.service, &old, Some(&new));
        assert_eq!(state.run, Some(run));
        assert!(!state.running);
        assert_eq!(state.unavailable, None);
    }

    #[test]
    fn the_sandbox_wraps_only_the_test_step_and_opens_the_prepared_folders() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);

        let run = harness.run(&old, Some(&new), Isolation::Sandbox);

        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        assert_eq!(run.isolation, Isolation::Sandbox);
        let recorded = harness.read_tool("sandbox-args");
        let lines: Vec<&str> = recorded.lines().collect();
        assert_eq!(lines[0], "-p");
        assert!(recorded.contains("(deny network*)"), "{recorded}");
        let cwd = PathBuf::from(harness.read_tool("test-cwd").trim());
        let run_folder = cwd.parent().unwrap();
        let gocache = fs::canonicalize(harness.tools.path()).unwrap().join("gocache");
        assert!(lines.contains(&format!("RUN_DIRECTORY={}", run_folder.display()).as_str()), "{recorded}");
        assert!(lines.contains(&format!("WRITABLE_0={}", gocache.display()).as_str()), "{recorded}");
        let tmpdir = harness.read_tool("test-tmpdir");
        assert!(lines.contains(&format!("TEMPORARY={tmpdir}").as_str()), "{recorded}");
        // Relative folders are dropped; the project's build cache is opened.
        let build_cache = sandbox::canonical(&harness.cache.path().join("build-cache").join(PROJECT).join("go"));
        assert!(lines.contains(&format!("WRITABLE_1={}", build_cache.display()).as_str()), "{recorded}");
        assert!(!recorded.contains("WRITABLE_2"), "{recorded}");
        let after: Vec<&str> = lines.iter().skip_while(|line| **line != "--").copied().collect();
        assert_eq!(after[2], "test");
        assert_eq!(*after.last().unwrap(), "--offline");
        assert_eq!(recorded.matches("\n--\n").count(), 1, "prepare is not sandboxed");
    }

    #[test]
    fn the_last_run_of_a_commit_survives_a_restart_and_is_derived_per_review() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let run = harness.run(&old, Some(&new), Isolation::Process);
        let tests_run = calls(harness.tools.path(), "test");

        let restarted = harness.restarted();
        let state = harness.state(&restarted, &old, Some(&new));
        assert_eq!(state.run.as_ref(), Some(&run));

        // Another review with the same new side: the same run, its tests
        // compared with that review's old side.
        let same = harness.state(&restarted, &new, Some(&new)).run.unwrap();
        assert_eq!(same.started_at, run.started_at);
        assert_eq!(same.old_sha, new);
        assert!(same.tests.iter().all(|t| t.state == TestFunctionState::Untouched));
        assert!(same.tests.iter().all(|t| t.exercises.is_none()));
        assert_eq!(calls(harness.tools.path(), "test"), tests_run, "nothing ran again");

        // Another commit has no run.
        assert_eq!(harness.state(&restarted, &old, Some(&old)).run, None);
    }

    /// Writes the runner again with the same bytes and a later modification
    /// time, as `tauri dev` rebuilds the analyzer on every start.
    fn rebuild_identically(runner: &Path) {
        let bytes = fs::read(runner).unwrap();
        fs::write(runner, bytes).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(3600);
        fs::File::options().write(true).open(runner).unwrap().set_modified(later).unwrap();
    }

    #[test]
    fn the_last_run_survives_a_restart_that_rebuilt_the_same_runner() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let run = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);

        rebuild_identically(&harness.tools.path().join("fake analyzer"));
        let restarted = harness.restarted();

        assert_eq!(harness.state(&restarted, &old, Some(&new)).run, Some(run));
        let analyzer = restarted.flow.analyzer(analyzers::for_language("go").unwrap()).unwrap();
        assert!(restarted
            .last_completed(PROJECT, harness.repo.path(), &analyzer, Some(&new))
            .is_some());
    }

    #[test]
    fn a_runner_with_other_code_starts_over() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.run(&old, Some(&new), Isolation::Process);

        let runner = harness.tools.path().join("fake analyzer");
        let mut script = fs::read_to_string(&runner).unwrap();
        script.push_str("# another build\n");
        fs::write(&runner, script).unwrap();

        assert_eq!(harness.state(&harness.restarted(), &old, Some(&new)).run, None);
    }

    #[test]
    fn a_working_tree_run_survives_a_restart_until_its_files_change() {
        let harness = Harness::new();
        let (_, new) = two_commits(&harness);
        harness.write(
            &[("a.go", &numbered_lines(10).replace("line 4\n", "line four\n"))],
            &new_side(),
            &reported(),
        );

        let run = harness.run(&new, None, Isolation::Process);

        assert_eq!(run.status, TestRunStatus::Completed, "{:?}", run.message);
        assert_eq!(run.new_sha, None);
        assert_eq!(harness.state(&harness.service, &new, None).run, Some(run.clone()));
        rebuild_identically(&harness.tools.path().join("fake analyzer"));
        let restarted = harness.restarted();
        assert_eq!(harness.state(&restarted, &new, None).run, Some(run.clone()));
        let analyzer = restarted.flow.analyzer(analyzers::for_language("go").unwrap()).unwrap();
        assert!(restarted
            .last_completed(PROJECT, harness.repo.path(), &analyzer, None)
            .is_some());

        // Once a file changed, the run is no longer the working tree's after
        // a restart; this run of the app still shows its last run.
        harness.repo.write("a.go", numbered_lines(11));
        assert_eq!(harness.state(&harness.restarted(), &new, None).run, None);
        assert_eq!(harness.state(&harness.service, &new, None).run.map(|r| r.started_at), Some(run.started_at));
    }

    #[test]
    fn a_derived_run_is_read_again_until_a_run_is_kept_or_the_files_change() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.run(&old, Some(&new), Isolation::Process);
        let last = || {
            harness
                .service
                .last_run(PROJECT, harness.repo.path(), &old, Some(&new))
                .unwrap()
                .unwrap()
        };

        // Read twice (the Tests tab, then the explanations of its failures):
        // derived once, with the new side's analysis it was read with.
        let first = last();
        let again = last();
        assert!(Arc::ptr_eq(&first.run, &again.run));
        assert_eq!(first.key, again.key);
        let functions: Vec<&str> = first
            .new_side
            .as_ref()
            .unwrap()
            .functions
            .iter()
            .map(|function| function.id.as_str())
            .collect();
        assert_eq!(functions, vec!["p.A", "p.TestA"]);
        assert_eq!(harness.state(&harness.service, &old, Some(&new)).run.as_ref(), Some(&*first.run));
        // Another review of the same new side is derived on its own.
        let other = harness
            .service
            .last_run(PROJECT, harness.repo.path(), &new, Some(&new))
            .unwrap()
            .unwrap();
        assert_ne!(other.key, first.key);
        assert_eq!(other.run.old_sha, new);

        // A run kept since: derived again, the new run.
        let rerun = harness.run(&old, Some(&new), Isolation::Sandbox);
        let after = last();
        assert!(!Arc::ptr_eq(&first.run, &after.run));
        assert_ne!(after.key, first.key);
        assert_eq!(*after.run, rerun);
        assert_eq!(after.run.isolation, Isolation::Sandbox);

        // The working tree: derived again once its files change.
        let head = new.clone();
        harness.run(&head, None, Isolation::Process);
        let working = || {
            harness
                .service
                .last_run(PROJECT, harness.repo.path(), &head, None)
                .unwrap()
                .unwrap()
        };
        let before = working();
        assert!(Arc::ptr_eq(&before.run, &working().run));
        harness.repo.write("a.go", numbered_lines(11));
        let edited = working();
        assert!(!Arc::ptr_eq(&before.run, &edited.run));
        assert_ne!(before.key, edited.key);
    }

    #[test]
    fn concurrent_runs_for_the_same_side_share_one_and_show_as_running() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.flag("slow", true);
        let (service, repo) = (&harness.service, harness.repo.path());

        let runs: Vec<TestRun> = thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        service
                            .run(PROJECT, repo, &old, Some(&new), Isolation::Process)
                            .unwrap()
                    })
                })
                .collect();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !harness.service.is_running(PROJECT, Some(&new)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(harness.state(&harness.service, &old, Some(&new)).running);
            assert!(!harness.service.is_running(PROJECT, Some(&old)));
            workers.into_iter().map(|worker| worker.join().unwrap()).collect()
        });

        assert_eq!(calls(harness.tools.path(), "test"), 1);
        assert_eq!(runs[0], runs[1]);
        assert!(!harness.state(&harness.service, &old, Some(&new)).running);
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    fn wait_until_dead(pid: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        !alive(pid)
    }

    #[test]
    fn cancelling_stops_the_run_with_everything_it_started() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let completed = harness.run(&old, Some(&new), Isolation::Process);
        harness.flag("hang", true);
        let (service, repo) = (&harness.service, harness.repo.path());

        let started = Instant::now();
        let run = thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .run(PROJECT, repo, &old, Some(&new), Isolation::Process)
                    .unwrap()
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while !harness.tools.path().join("child.pid").exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            // Another side is not affected; a side without a run is a no-op.
            harness.service.cancel(PROJECT, Some(&old));
            harness.service.cancel(PROJECT, None);
            assert!(harness.service.is_running(PROJECT, Some(&new)));
            harness.service.cancel(PROJECT, Some(&new));
            worker.join().unwrap()
        });

        assert_eq!(run.status, TestRunStatus::Cancelled);
        assert!(run.tests.is_empty() && run.message.is_none());
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(wait_until_dead(&harness.read_tool("child.pid")));
        assert!(!harness.service.is_running(PROJECT, Some(&new)));
        // The last run is the cancelled one; the last completed is kept.
        let state = harness.state(&harness.service, &old, Some(&new));
        assert_eq!(state.run.unwrap().status, TestRunStatus::Cancelled);
        let analyzer = harness.flow.analyzer(analyzers::for_language("go").unwrap()).unwrap();
        let (record, _) = harness
            .service
            .last_completed(PROJECT, harness.repo.path(), &analyzer, Some(&new))
            .unwrap();
        assert_eq!(record.started_at, completed.started_at);
    }

    #[test]
    fn a_run_past_the_time_limit_times_out() {
        let tools = TempDir::new().unwrap();
        let mut settings = settings(tools.path());
        settings.hard_timeout = Duration::from_millis(1500);
        let harness = Harness::with(tools, settings);
        let (old, new) = two_commits(&harness);
        harness.flag("hang", true);

        let run = harness.run(&old, Some(&new), Isolation::Process);

        assert_eq!(run.status, TestRunStatus::TimedOut);
        assert!(run.duration_ms >= 1500, "{}", run.duration_ms);
        assert!(wait_until_dead(&harness.read_tool("child.pid")));
    }

    #[test]
    fn a_runner_or_prepare_that_fails_makes_a_failed_run() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);

        harness.flag("test-fail", true);
        let run = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(run.status, TestRunStatus::Failed);
        assert_eq!(run.message.as_deref(), Some("compiling\ncannot run tests"));
        assert!(run.tests.is_empty() && run.coverage.is_none());
        assert_eq!(run.runner.name, "difflens-analyzer-go", "from the analysis");
        harness.flag("test-fail", false);

        harness.flag("garbage", true);
        let run = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(run.status, TestRunStatus::Failed);
        assert!(run.message.unwrap().contains("invalid test report"));
        harness.flag("garbage", false);

        harness.flag("prepare-fail", true);
        let tests_before = calls(harness.tools.path(), "test");
        let run = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(run.status, TestRunStatus::Failed);
        let message = run.message.unwrap();
        assert!(message.contains("could not prepare the tests"), "{message}");
        assert!(message.ends_with("go: download failed"), "{message}");
        assert_eq!(calls(harness.tools.path(), "test"), tests_before);
    }

    #[test]
    fn a_missing_runner_makes_a_failed_run_and_no_kept_run() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.run(&old, Some(&new), Isolation::Process);
        fs::remove_file(harness.tools.path().join("fake analyzer")).unwrap();

        // The analyses are cached; only the runner is gone.
        let run = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(run.status, TestRunStatus::Failed);
        let message = run.message.unwrap();
        assert!(
            message.ends_with("The Go analyzer is not installed; run pnpm analyzers:build."),
            "{message}"
        );
        let state = harness.state(&harness.service, &old, Some(&new));
        assert_eq!(state.run, None, "runs are kept per runner build");
        assert_eq!(state.unavailable, None);
    }

    #[test]
    fn a_project_without_a_test_runner_is_unavailable() {
        let harness = Harness::new();
        harness.repo.write("README.md", "notes\n");
        harness.repo.commit_all("notes");
        let sha = harness.repo.git(&["rev-parse", "HEAD"]);

        let state = harness.state(&harness.service, &sha, Some(&sha));
        assert_eq!(
            state.unavailable,
            Some(TestsUnavailable::NoRunner { languages: vec![] })
        );
        assert_eq!((state.run, state.running), (None, false));
        let error = harness
            .service
            .run(PROJECT, harness.repo.path(), &sha, Some(&sha), Isolation::Sandbox)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("no test runner"), "{}", error.message);
        let error = harness
            .service
            .run(PROJECT, harness.repo.path(), "HEAD", None, Isolation::Sandbox)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnknownBranch);
        assert_eq!(calls(harness.tools.path(), "test"), 0);
    }

    #[test]
    fn a_failed_analysis_leaves_tests_untouched_with_a_diagnostic() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        // A commit whose analysis is not valid JSON.
        harness.repo.write("fixture/analysis.json", "broken");
        harness.repo.commit_all("broken analysis");
        let broken = harness.repo.git(&["rev-parse", "HEAD"]);
        let _ = new;

        let run = harness.run(&old, Some(&broken), Isolation::Process);

        assert_eq!(run.status, TestRunStatus::Completed);
        assert!(run.tests.iter().all(|t| t.state == TestFunctionState::Untouched));
        assert!(run.tests.iter().all(|t| !t.in_changed_package));
        assert_eq!(run.diagnostics.len(), 1);
        assert!(
            run.diagnostics[0].starts_with("The new side could not be analysed"),
            "{:?}",
            run.diagnostics
        );
        assert!(run.coverage.is_some());
    }

    fn select_test_a() -> RunSelection {
        RunSelection {
            packages: vec![crate::model::SelectedPackage {
                package: "pkg".to_string(),
                tests: Some(vec![crate::model::SelectedTest {
                    name: "TestA".to_string(),
                    subtests: None,
                }]),
            }],
        }
    }

    fn partial(isolation: Isolation) -> RunRequest {
        RunRequest {
            isolation,
            options: RunOptions::default(),
            selection: Some(select_test_a()),
        }
    }

    /// Two commits whose new side reports TestA, its subtest and TestB in a
    /// whole run, and TestA and its subtest, now failing, in a partial run.
    fn commits_with_a_partial_report(harness: &Harness) -> (String, String) {
        let (old, _) = two_commits(harness);
        let mut b = test("TestB", Some("p.TestB"));
        b.duration_ms = 7;
        let whole = output(
            vec![test("TestA", Some("p.TestA")), test("TestA/declines_politely", Some("p.TestA")), b],
            Some(vec![block("a.go", 2, 4, 1), block("a.go", 5, 6, 0)]),
        );
        let partial = output(
            vec![
                failing(test("TestA/declines_politely", Some("p.TestA")), "    a_test.go:3: boom\n"),
                failing(test("TestA", Some("p.TestA")), "    a_test.go:3: boom\n"),
            ],
            Some(vec![block("a.go", 2, 4, 0), block("a.go", 5, 6, 3)]),
        );
        harness.repo.write("fixture/tests-partial.json", serde_json::to_string(&partial).unwrap());
        harness.repo.write("fixture/tests.json", serde_json::to_string(&whole).unwrap());
        harness.repo.commit_all("partial report");
        (old, harness.repo.git(&["rev-parse", "HEAD"]))
    }

    fn outcomes(run: &TestRun) -> Vec<(String, TestOutcome)> {
        let mut rows: Vec<_> = run.tests.iter().map(|t| (t.name.clone(), t.outcome)).collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    #[test]
    fn a_partial_run_is_merged_into_the_last_complete_run_and_kept() {
        let harness = Harness::new();
        let (old, new) = commits_with_a_partial_report(&harness);
        let whole = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(whole.partial, None);
        assert_eq!(whole.coverage.as_ref().unwrap().project.covered, 3);

        let run = harness
            .service
            .run_with(PROJECT, harness.repo.path(), &old, Some(&new), &partial(Isolation::Process))
            .unwrap();

        // The selection reached the runner as a file.
        let sent: RunSelection = serde_json::from_str(&harness.read_tool("selection")).unwrap();
        assert_eq!(sent, select_test_a());
        assert!(harness.read_tool("test-args").contains("--selection "));
        // Only TestA and its subtest were replaced; TestB kept its result.
        assert_eq!(
            outcomes(&run),
            vec![
                ("TestA".to_string(), TestOutcome::Fail),
                ("TestA/declines_politely".to_string(), TestOutcome::Fail),
                ("TestB".to_string(), TestOutcome::Pass),
            ]
        );
        assert_eq!(run.tests.iter().find(|t| t.name == "TestB").unwrap().duration_ms, 7);
        assert_eq!(run.packages[0].outcome, crate::model::PackageOutcomeKind::Fail);
        // The header says when the whole run was and what the partial one covered.
        assert_eq!(run.started_at, whole.started_at);
        let partial_run = run.partial.clone().unwrap();
        assert_eq!((partial_run.status, partial_run.packages, partial_run.tests), (TestRunStatus::Completed, 1, 1));
        // Coverage of the same commit is the union: a.go 5-6 ran now, 2-4 before.
        assert_eq!(run.coverage.as_ref().unwrap().project, crate::model::LineCoverage { covered: 5, coverable: 5 });
        assert!(run.tests.iter().find(|t| t.name == "TestA").unwrap().failure.is_some());

        // Kept as the last and the last completed run, across a restart.
        let restarted = harness.restarted();
        assert_eq!(harness.state(&restarted, &old, Some(&new)).run, Some(run.clone()));
        let analyzer = restarted.flow.analyzer(analyzers::for_language("go").unwrap()).unwrap();
        let (_, kept) = restarted
            .last_completed(PROJECT, harness.repo.path(), &analyzer, Some(&new))
            .unwrap();
        assert_eq!(kept.tests.len(), 3);

        // A whole run replaces everything again.
        let again = harness.run(&old, Some(&new), Isolation::Process);
        assert_eq!(again.partial, None);
        assert_eq!(outcomes(&again)[0].1, TestOutcome::Pass);
    }

    #[test]
    fn a_partial_run_without_a_complete_run_is_all_there_is() {
        let harness = Harness::new();
        let (old, new) = commits_with_a_partial_report(&harness);

        let run = harness
            .service
            .run_with(PROJECT, harness.repo.path(), &old, Some(&new), &partial(Isolation::Process))
            .unwrap();

        assert_eq!(run.status, TestRunStatus::Completed);
        assert_eq!(run.tests.len(), 2);
        assert_eq!(run.partial.unwrap().tests, 1);
    }

    #[test]
    fn a_cancelled_partial_run_leaves_the_last_results_as_they_were() {
        let harness = Harness::new();
        let (old, new) = commits_with_a_partial_report(&harness);
        let whole = harness.run(&old, Some(&new), Isolation::Process);
        harness.flag("hang", true);
        let (service, repo) = (&harness.service, harness.repo.path());

        let run = thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .run_with(PROJECT, repo, &old, Some(&new), &partial(Isolation::Process))
                    .unwrap()
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while !harness.tools.path().join("child.pid").exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            harness.service.cancel(PROJECT, Some(&new));
            worker.join().unwrap()
        });

        assert!(wait_until_dead(&harness.read_tool("child.pid")));
        assert_eq!(run.status, TestRunStatus::Completed, "the last results are shown");
        assert_eq!(outcomes(&run), outcomes(&whole));
        assert_eq!(run.partial.as_ref().unwrap().status, TestRunStatus::Cancelled);
        let state = harness.state(&harness.service, &old, Some(&new));
        assert_eq!(state.run.unwrap().partial.unwrap().status, TestRunStatus::Cancelled);
        let analyzer = harness.flow.analyzer(analyzers::for_language("go").unwrap()).unwrap();
        let (record, _) = harness
            .service
            .last_completed(PROJECT, harness.repo.path(), &analyzer, Some(&new))
            .unwrap();
        assert_eq!((record.started_at, record.partial), (whole.started_at, None));
    }

    #[test]
    fn a_working_tree_partial_run_merges_and_survives_a_restart() {
        let harness = Harness::new();
        let (_, new) = commits_with_a_partial_report(&harness);
        harness.repo.write("a.go", numbered_lines(12));
        harness.run(&new, None, Isolation::Process);

        let run = harness
            .service
            .run_with(PROJECT, harness.repo.path(), &new, None, &partial(Isolation::Process))
            .unwrap();

        assert_eq!(outcomes(&run)[2], ("TestB".to_string(), TestOutcome::Pass));
        assert_eq!(outcomes(&run)[0].1, TestOutcome::Fail);
        // The same files: the coverage is merged too.
        assert_eq!(run.coverage.as_ref().unwrap().project.covered, 5);
        assert_eq!(harness.state(&harness.restarted(), &new, None).run, Some(run));
    }

    #[test]
    fn another_run_while_one_goes_on_is_refused_and_progress_is_read() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.flag("slow", true);
        let (service, repo) = (&harness.service, harness.repo.path());

        thread::scope(|scope| {
            let worker = scope.spawn(|| {
                service
                    .run(PROJECT, repo, &old, Some(&new), Isolation::Process)
                    .unwrap()
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut progress = None;
            while progress.is_none() && Instant::now() < deadline {
                progress = harness.service.progress(PROJECT, Some(&new));
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                progress,
                Some(TestProgress {
                    packages_done: 1,
                    packages_total: 2,
                    tests: 3,
                    failed: 1,
                    current: Some("pkg".to_string()),
                })
            );
            assert_eq!(harness.service.progress(PROJECT, Some(&old)), None);
            let state = harness.state(&harness.service, &old, Some(&new));
            assert!(state.running && state.running_selection.is_none());

            let error = harness
                .service
                .run_with(PROJECT, repo, &old, Some(&new), &partial(Isolation::Process))
                .unwrap_err();
            assert_eq!(error.reason, Some(crate::error::Reason::TestRunBusy));
            worker.join().unwrap();
        });
        assert_eq!(calls(harness.tools.path(), "test"), 1);
        assert_eq!(harness.service.progress(PROJECT, Some(&new)), None, "gone with the run");
    }

    #[test]
    fn options_turn_the_race_detector_and_coverage_off() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let request = RunRequest {
            isolation: Isolation::Process,
            options: RunOptions {
                race: false,
                coverage: false,
            },
            selection: None,
        };

        let run = harness
            .service
            .run_with(PROJECT, harness.repo.path(), &old, Some(&new), &request)
            .unwrap();

        let arguments = harness.read_tool("test-args");
        assert!(arguments.trim().ends_with("--no-race --no-cover"), "{arguments}");
        assert_eq!(run.options, request.options);
        assert_eq!(harness.state(&harness.restarted(), &old, Some(&new)).run.unwrap().options, request.options);
    }

    /// Keys of every object in `value`, by JSON pointer of the first object
    /// found at each path (arrays by their first item).
    fn shape(value: &serde_json::Value, path: &str, found: &mut Vec<(String, Vec<String>)>) {
        match value {
            serde_json::Value::Object(object) => {
                let mut keys: Vec<String> = object.keys().cloned().collect();
                keys.sort();
                found.push((path.to_string(), keys));
                for (key, inner) in object {
                    shape(inner, &format!("{path}/{key}"), found);
                }
            }
            serde_json::Value::Array(items) => {
                if let Some(first) = items.first() {
                    shape(first, &format!("{path}/0"), found);
                }
            }
            _ => {}
        }
    }

    fn sorted(keys: &[&str]) -> Vec<String> {
        let mut keys: Vec<String> = keys.iter().map(|key| key.to_string()).collect();
        keys.sort();
        keys
    }

    #[test]
    fn tests_state_serialises_to_the_contract_shape() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let mut reported = reported();
        reported.races.push(crate::runner_protocol::RunnerRace {
            container: "pkg".to_string(),
            test: Some("TestA".to_string()),
            output: "WARNING: DATA RACE".to_string(),
        });
        reported.diagnostics.push("slow".to_string());
        harness.repo.write("fixture/tests.json", serde_json::to_string(&reported).unwrap());
        harness.repo.commit_all("races");
        let new_with_races = harness.repo.git(&["rev-parse", "HEAD"]);
        let _ = new;
        harness.run(&old, Some(&new_with_races), Isolation::Sandbox);

        let state = harness.state(&harness.service, &old, Some(&new_with_races));
        let json = serde_json::to_value(&state).unwrap();
        let mut found = Vec::new();
        shape(&json, "", &mut found);
        let expected: Vec<(&str, Vec<String>)> = vec![
            ("", sorted(&["run", "running", "runningSelection", "isolation", "unavailable"])),
            (
                "/run",
                sorted(&[
                    "language", "runner", "oldSha", "newSha", "isolation", "startedAt",
                    "durationMs", "status", "message", "tests", "summary", "packages",
                    "coverage", "raceDetector", "races", "diagnostics", "options", "partial",
                ]),
            ),
            ("/run/runner", sorted(&["name", "version"])),
            (
                "/run/tests/0",
                sorted(&[
                    "id", "container", "package", "name", "hierarchy", "title", "command",
                    "functionId", "state", "outcome", "durationMs", "output", "file", "line",
                    "exercises", "inChangedPackage", "failure",
                ]),
            ),
            ("/run/tests/0/exercises", sorted(&["id", "qualifiedName"])),
            ("/run/summary", sorted(&["passed", "failed", "skipped", "added"])),
            (
                "/run/packages/0",
                sorted(&["container", "outcome", "durationMs", "output", "failure", "command"]),
            ),
            ("/run/options", sorted(&["race", "coverage"])),
            ("/run/coverage", sorted(&["changedLines", "files", "project"])),
            ("/run/coverage/changedLines", sorted(&["covered", "coverable"])),
            ("/run/coverage/files/0", sorted(&["path", "covered", "coverable"])),
            ("/run/coverage/project", sorted(&["covered", "coverable"])),
            ("/run/races/0", sorted(&["container", "test", "output"])),
        ];
        for (path, keys) in &expected {
            let actual = found
                .iter()
                .find(|(found_path, _)| found_path == path)
                .unwrap_or_else(|| panic!("no object at {path}: {json}"));
            assert_eq!(&actual.1, keys, "{path}");
        }
        assert_eq!(found.len(), expected.len(), "{found:?}");
        assert_eq!(json["isolation"], "sandbox");
        assert_eq!(json["run"]["status"], "completed");
        assert_eq!(json["run"]["tests"][0]["state"], "new");
        assert_eq!(json["run"]["tests"][0]["outcome"], "pass");
        assert_eq!(json["run"]["packages"][0]["outcome"], "pass");
        assert_eq!(json["unavailable"], serde_json::Value::Null);
        let unavailable = serde_json::to_value(TestsUnavailable::NoRunner {
            languages: vec!["rust".to_string()],
        })
        .unwrap();
        assert_eq!(unavailable, serde_json::json!({ "reason": "no_runner", "languages": ["rust"] }));
        for (status, text) in [
            (TestRunStatus::TimedOut, "timed_out"),
            (TestRunStatus::Cancelled, "cancelled"),
            (TestRunStatus::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), text);
        }
        let isolation: Isolation = serde_json::from_value(serde_json::json!("process")).unwrap();
        assert_eq!(isolation, Isolation::Process);
        assert_eq!(
            serde_json::to_value(crate::model::PackageOutcomeKind::BuildFailed).unwrap(),
            "build_failed"
        );
    }

    /// Builds the real Go analyzer from `analyzers/go` and runs the tests of
    /// a tiny Go repository in the sandbox and as a process, then its
    /// quality report. Needs Go on the PATH and macOS.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "builds the real Go analyzer and runs go test under sandbox-exec"]
    fn the_go_runner_runs_end_to_end_in_the_sandbox_and_as_a_process() {
        use crate::model::{QualityResult, RaceDetectorState};
        use crate::quality::QualityService;

        let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("../analyzers/go");
        let tools = TempDir::new().unwrap();
        let executable = tools.path().join("difflens-analyzer-go");
        let built = std::process::Command::new("go")
            .args(["build", "-o"])
            .arg(&executable)
            .arg("./cmd/difflens-analyzer-go")
            .current_dir(&sources)
            .status()
            .unwrap();
        assert!(built.success(), "go build failed");

        let cache = TempDir::new().unwrap();
        let flow = Arc::new(FlowService::new(
            isolated_git(),
            cache.path().to_path_buf(),
            AnalyzerLocator::with_executable("go", executable),
            ToolPath::fixed(std::env::var_os("PATH").unwrap_or_default()),
            RunSettings::default(),
        ));
        let tests = Arc::new(TestsService::new(
            Arc::clone(&flow),
            cache.path().to_path_buf(),
            TestSettings::default(),
        ));
        let quality = QualityService::new(Arc::clone(&flow), Arc::clone(&tests), cache.path().to_path_buf());
        let outside = TempDir::new().unwrap();
        let outside_path = sandbox::canonical(outside.path());
        let repo = TestRepo::init();
        repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
        repo.write("demo.go", "package demo\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n");
        repo.commit_all("first");
        let old = repo.git(&["rev-parse", "HEAD"]);
        repo.write(
            "demo.go",
            "package demo\n\nfunc Add(a, b int) int {\n\tif a > 100 {\n\t\treturn a\n\t}\n\treturn a + b\n}\n",
        );
        repo.write(
            "demo_test.go",
            crate::sandbox::fixtures::go_probe_tests(&outside_path),
        );
        repo.commit_all("second");
        let new = repo.git(&["rev-parse", "HEAD"]);

        let sandboxed = tests
            .run(PROJECT, repo.path(), &old, Some(&new), Isolation::Sandbox)
            .unwrap();
        println!("{}", serde_json::to_string_pretty(&sandboxed).unwrap());
        assert_eq!(sandboxed.status, TestRunStatus::Completed, "{:?}", sandboxed.message);
        assert!(!sandboxed.tests.is_empty());
        assert!(
            sandboxed.tests.iter().all(|t| t.outcome == TestOutcome::Pass),
            "the walls held and everything else worked"
        );
        let add = sandboxed.tests.iter().find(|t| t.name == "TestAdd").unwrap();
        assert_eq!(add.state, TestFunctionState::New);
        assert!(
            add.exercises.as_ref().is_some_and(|f| f.qualified_name.ends_with("Add")),
            "{add:?}"
        );
        assert!(add.in_changed_package);
        let coverage = sandboxed.coverage.as_ref().unwrap();
        assert!(coverage.changed_lines.coverable > 0, "{coverage:?}");
        assert!(sandboxed.race_detector, "{:?}", sandboxed.diagnostics);
        assert!(!outside_path.join("escaped").exists());

        let plain = tests
            .run(PROJECT, repo.path(), &old, Some(&new), Isolation::Process)
            .unwrap();
        assert_eq!(plain.status, TestRunStatus::Completed, "{:?}", plain.message);
        let walls = plain.tests.iter().find(|t| t.name == "TestWalls").unwrap();
        assert_eq!(walls.outcome, TestOutcome::Fail, "a plain process can write anywhere");
        assert!(outside_path.join("escaped").exists());

        let QualityResult::Report { report } = quality.quality(PROJECT, repo.path(), &old, Some(&new)).unwrap()
        else {
            panic!("expected a report");
        };
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert_eq!(report.test_run_at.as_deref(), Some(plain.started_at.as_str()));
        assert_eq!(report.race_detector, RaceDetectorState::Clean);
        assert_eq!(report.checks.first().map(|c| c.tool.as_str()), Some("go vet"));
        let add = report.functions.iter().find(|f| f.name == "Add").unwrap();
        assert_eq!((add.complexity, add.previous_complexity), (Some(2), Some(1)));
        assert!(add.crap.is_some() && add.changed_lines.is_some());
    }
}
