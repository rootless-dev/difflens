//! The Quality tab: the review's changed production functions with their
//! complexity, the coverage and CRAP score of the last completed test run of
//! the new side, the limits they cross, and the static checkers of the new
//! side. Never executes project code: complexity comes from the Flow
//! analyses, coverage from a run made by the Tests tab, and the checkers
//! only read the code.
//!
//! A function's limits are its complexity, its CRAP score and the coverage
//! of its changed lines (below the target for the review's changed lines);
//! a function the run measured and never executed is untested. The checker
//! findings on lines the review adds are also given placed on the function
//! that holds them (`signals::locate_checks`).
//!
//! Checker results are cached on disk per commit under
//! `<cache>/checks/<project>/<analyzer id>/<analyzer fingerprint>/<sha>.json`,
//! run on the shared extracted tree of the commit (checkers do not write).
//! The checkers of an analyzer that need the project's dependencies
//! (`checks_need_prepare`: the project's own tsc and ESLint, mypy resolving
//! imports) run instead on a throwaway copy under `<cache>/check-scratch/`
//! that `prepare` ran on first, as a plain process like a test run's; when
//! the dependencies cannot be installed they run anyway, a diagnostic says
//! so, and that result is not kept. Checkers that execute project code run
//! in the analyzer's own sandbox. Every check gets the project's build cache
//! (`build_cache`) as `--cache-dir`.
//! The working tree is checked on a scratch copy and its last result kept in
//! memory with the stamp of the files it was made from, like its analysis. A
//! checker run that fails is reported in the diagnostics and not kept.
//!
//! Duplicated code the review introduces comes from each analyzer's `dupes`
//! on the new side, given the lines the review adds and the duplication
//! limits (`duplication`), while the checkers run; production code and test
//! code come apart. It depends on both sides, so it is cached per pair of
//! commits, limits and search rules (`duplication::SEARCH_RULES`) under
//! `<cache>/duplicates/<project>/<analyzer id>/<analyzer fingerprint>/<old>-<new>-<tokens>-<lines>-r<rules>.json`,
//! and the working tree's last search is kept in memory with its stamp, the
//! old side and the limits. A search that fails is a diagnostic, not kept.
//!
//! The report also says where coverage stands (`QualityCoverage`): no run,
//! a run asked not to measure it, or one that measured none, with the
//! analyzer's own words about why, so an empty coverage column is never
//! left unexplained.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::analyzers::{self, DupesRequest};
use crate::disk_cache;
use crate::duplication::{self, ChangedInput, Found};
use crate::error::{app_error, core_text, AppError, AppResult, CoreText};
use crate::flow::{Analyzer, FlowService};
use crate::hunks::ChangedLines;
use crate::model::{
    ChangedState, CoverageState, CrossedLimit, Duplicate, FlowNodeState, FunctionRef, LimitKind,
    LineCoverage, QualityCoverage, QualityFunction, QualityLimits, QualityReport, QualityResult,
    RaceDetectorState, StaticCheck,
};
use crate::runner_protocol::{self, ChecksOutput, TestOutput};
use crate::sandbox;
use crate::signals;
use crate::single_flight::SingleFlight;
use crate::test_report::{CoverageIndex, ReviewFacts, RunRecord};
use crate::test_runs::TestsService;
use crate::trees::{self, ScratchTree};

/// Checker results kept per project and language.
const KEEP_CHECKS: usize = 50;
/// Duplicate code searches kept per project and language.
const KEEP_DUPLICATES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CheckKey {
    project: String,
    analyzer: &'static str,
    fingerprint: String,
    commit: Option<String>,
    stamp: Option<u64>,
}

/// The checkers' report with a note of the core's about it (the
/// dependencies could not be installed), or why there is none.
type CheckResult = Result<(Arc<ChecksOutput>, Option<CoreText>), CoreText>;

/// Under the cache folder: the throwaway copies checkers that need the
/// project's dependencies run on.
const CHECK_SCRATCH: &str = "check-scratch";
/// When `prepare` is stopped before the checkers run anyway.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(600);

struct WorkingChecks {
    fingerprint: String,
    stamp: u64,
    output: Arc<ChecksOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DupesKey {
    project: String,
    analyzer: &'static str,
    fingerprint: String,
    old: String,
    commit: Option<String>,
    stamp: Option<u64>,
    tokens: u32,
    lines: u32,
}

type DupesResult = Result<Arc<Found>, CoreText>;

struct WorkingDupes {
    fingerprint: String,
    stamp: u64,
    old: String,
    found: Arc<Found>,
}

pub struct QualityService {
    flow: Arc<FlowService>,
    tests: Arc<TestsService>,
    cache: PathBuf,
    limits: QualityLimits,
    flights: SingleFlight<CheckKey, AppResult<CheckResult>>,
    /// By project id and language.
    working: Mutex<HashMap<(String, &'static str), WorkingChecks>>,
    dupes_flights: SingleFlight<DupesKey, AppResult<DupesResult>>,
    /// By project id and language.
    working_dupes: Mutex<HashMap<(String, &'static str), WorkingDupes>>,
}

impl QualityService {
    /// Copies left by an earlier run of the app are deleted.
    pub fn new(flow: Arc<FlowService>, tests: Arc<TestsService>, cache: PathBuf) -> Self {
        let _ = fs::remove_dir_all(cache.join(CHECK_SCRATCH));
        Self {
            flow,
            tests,
            cache,
            limits: QualityLimits::default(),
            flights: SingleFlight::new(),
            working: Mutex::new(HashMap::new()),
            dupes_flights: SingleFlight::new(),
            working_dupes: Mutex::new(HashMap::new()),
        }
    }

    /// The quality report of the review from `old_sha` to `new_sha` (the
    /// working tree when `None`). Fails with `unknown_branch` when a sha is
    /// not a commit of `repo`.
    pub fn quality(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> AppResult<QualityResult> {
        let sides = match self.flow.sides(project_id, repo, old_sha, new_sha)? {
            Ok(sides) => sides,
            Err(unavailable) => return Ok(QualityResult::Unavailable(unavailable)),
        };
        let run = sides
            .analyzers
            .iter()
            .find(|analyzer| analyzer.spec.tests)
            .and_then(|analyzer| {
                let (record, output) =
                    self.tests
                        .last_completed(project_id, repo, analyzer, new_sha)?;
                Some((analyzer.spec.id, record, output))
            });
        let coverage = run.as_ref().and_then(|(language, _, output)| {
            let blocks = &output.coverage.as_ref()?.blocks;
            Some((*language, CoverageIndex::new(blocks)))
        });
        let facts: Vec<ReviewFacts<'_>> = sides
            .old
            .iter()
            .zip(&sides.new)
            .map(|(old, new)| ReviewFacts::new(old, new, &sides.lines))
            .collect();

        // The duplicate code search reads the same tree as the checkers, and
        // neither waits for the other.
        let (checked, searched) = thread::scope(|scope| {
            let searched = scope.spawn(|| {
                sides
                    .analyzers
                    .iter()
                    .map(|analyzer| {
                        self.duplicates(project_id, repo, old_sha, new_sha, analyzer, &sides.lines)
                    })
                    .collect::<Vec<_>>()
            });
            let checked: Vec<_> = sides
                .analyzers
                .iter()
                .filter(|analyzer| analyzer.spec.checks)
                .map(|analyzer| self.checks(project_id, repo, new_sha, analyzer))
                .collect();
            (checked, searched.join().unwrap_or_else(|_| vec![Err(dupes_abandoned())]))
        });

        let mut checks = Vec::new();
        let mut diagnostics: Vec<CoreText> = sides.left_out.clone();
        for result in checked {
            match result? {
                Ok((output, note)) => {
                    checks.extend(output.checks.iter().cloned());
                    diagnostics.extend(note);
                    diagnostics.extend(
                        output
                            .diagnostics
                            .iter()
                            .map(|diagnostic| CoreText::raw(diagnostic.as_str())),
                    );
                }
                Err(message) => diagnostics.push(message),
            }
        }
        let mut duplicates = Vec::new();
        let mut test_duplicates = Vec::new();
        for result in searched {
            match result? {
                Ok(found) => {
                    duplicates.extend(found.duplicates.iter().cloned());
                    test_duplicates.extend(found.test_duplicates.iter().cloned());
                    diagnostics.extend(
                        found
                            .diagnostics
                            .iter()
                            .map(|diagnostic| CoreText::raw(diagnostic.as_str())),
                    );
                }
                Err(message) => diagnostics.push(message),
            }
        }

        let languages = sides
            .analyzers
            .iter()
            .zip(&facts)
            .map(|(analyzer, facts)| LanguageInput {
                facts,
                coverage: coverage
                    .as_ref()
                    .filter(|(id, _)| *id == analyzer.spec.id)
                    .map(|(_, index)| index),
            })
            .collect();
        let mut report = assemble(QualityInput {
            language: sides
                .new
                .iter()
                .map(|analysis| analysis.language.as_str())
                .collect::<Vec<_>>()
                .join("+"),
            old_sha,
            new_sha,
            limits: self.limits,
            languages,
            lines: &sides.lines,
            run: run.as_ref().map(|(_, record, output)| (record, output)),
            checks,
            duplicates,
            test_duplicates,
            diagnostics,
        });
        // The working tree is read where it is, as the Diff tab reads it; a
        // commit from its shared extracted tree, there already for the
        // checkers and the duplicate code search.
        match new_sha {
            Some(_) => self
                .flow
                .with_tree(project_id, repo, new_sha, |tree| with_function_code(&mut report, tree))?,
            None => with_function_code(&mut report, repo),
        }
        Ok(QualityResult::Report {
            report: Box::new(report),
        })
    }

    /// The duplicated code the review from `old_sha` to `new_sha` introduces,
    /// as one analyzer finds it on the new side, or a message saying why
    /// there is none.
    fn duplicates(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        analyzer: &Analyzer,
        lines: &ChangedLines,
    ) -> AppResult<DupesResult> {
        let (tokens, min_lines) = (self.limits.duplication_tokens, self.limits.duplication_lines);
        let cached = new_sha.map(|sha| {
            self.cache
                .join("duplicates")
                .join(project_id)
                .join(analyzer.spec.id)
                .join(&analyzer.fingerprint)
                .join(format!(
                    "{old_sha}-{sha}-{tokens}-{min_lines}-r{rules}.json",
                    rules = duplication::SEARCH_RULES
                ))
        });
        let stamp = match new_sha {
            Some(_) => None,
            None => Some(trees::working_tree_stamp(self.flow.git(), repo)?),
        };
        let remembered = || {
            let found = match stamp {
                Some(stamp) => self.remembered_dupes(project_id, analyzer, stamp, old_sha),
                None => cached.as_deref().and_then(read_cached_dupes),
            };
            found.filter(|found| found.min_tokens == tokens && found.min_lines == min_lines)
        };
        if let Some(found) = remembered() {
            return Ok(Ok(found));
        }
        let key = DupesKey {
            project: project_id.to_string(),
            analyzer: analyzer.spec.id,
            fingerprint: analyzer.fingerprint.clone(),
            old: old_sha.to_string(),
            commit: new_sha.map(str::to_string),
            stamp,
            tokens,
            lines: min_lines,
        };
        self.dupes_flights.run(key, Err(dupes_abandoned()), || {
            if let Some(found) = remembered() {
                return Ok(Ok(found));
            }
            let searched = self.flow.with_tree(project_id, repo, new_sha, |tree| {
                let input = ChangedInput::write(&self.cache.join("scratch"), lines)?;
                let output = analyzers::run_dupes(
                    analyzer.spec,
                    &analyzer.launcher,
                    tree,
                    self.flow.tool_path(),
                    self.flow.settings(),
                    DupesRequest {
                        changed: input.path(),
                        min_tokens: tokens,
                        min_lines,
                    },
                )?;
                Ok::<_, CoreText>(duplication::with_code(output, tree))
            })?;
            let found = match searched {
                Ok(found) => Arc::new(found),
                Err(message) => return Ok(Err(message)),
            };
            if let Some(file) = &cached {
                if let Ok(bytes) = serde_json::to_vec(&*found) {
                    // A cache that cannot be written only costs a later rerun.
                    let _ = disk_cache::write(file, &bytes, KEEP_DUPLICATES);
                }
            }
            if let Some(stamp) = stamp {
                self.working_dupes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        (project_id.to_string(), analyzer.spec.id),
                        WorkingDupes {
                            fingerprint: analyzer.fingerprint.clone(),
                            stamp,
                            old: old_sha.to_string(),
                            found: Arc::clone(&found),
                        },
                    );
            }
            Ok(Ok(found))
        })
    }

    fn remembered_dupes(
        &self,
        project_id: &str,
        analyzer: &Analyzer,
        stamp: u64,
        old_sha: &str,
    ) -> Option<Arc<Found>> {
        self.working_dupes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(project_id.to_string(), analyzer.spec.id))
            .filter(|kept| {
                kept.stamp == stamp && kept.fingerprint == analyzer.fingerprint && kept.old == old_sha
            })
            .map(|kept| Arc::clone(&kept.found))
    }

    /// The checker results of the new side for one analyzer, or a message
    /// saying why there are none.
    fn checks(
        &self,
        project_id: &str,
        repo: &Path,
        new_sha: Option<&str>,
        analyzer: &Analyzer,
    ) -> AppResult<CheckResult> {
        let cached = new_sha.map(|sha| {
            self.cache
                .join("checks")
                .join(project_id)
                .join(analyzer.spec.id)
                .join(&analyzer.fingerprint)
                .join(format!("{sha}.json"))
        });
        let stamp = match new_sha {
            Some(_) => None,
            None => Some(trees::working_tree_stamp(self.flow.git(), repo)?),
        };
        let remembered = || match stamp {
            Some(stamp) => self.remembered_working(project_id, analyzer, stamp),
            None => cached
                .as_deref()
                .and_then(|file| read_cached(file, analyzer)),
        };
        if let Some(output) = remembered() {
            return Ok(Ok((output, None)));
        }
        let key = CheckKey {
            project: project_id.to_string(),
            analyzer: analyzer.spec.id,
            fingerprint: analyzer.fingerprint.clone(),
            commit: new_sha.map(str::to_string),
            stamp,
        };
        let abandoned = Err(app_error!(
            Internal,
            StaticChecksAbandoned,
            "The static checks stopped unexpectedly."
        ));
        self.flights.run(key, abandoned, || {
            if let Some(output) = remembered() {
                return Ok(Ok((output, None)));
            }
            // Held while the checkers run, so it is not emptied under them.
            let build_cache = self
                .flow
                .build_caches()
                .lease(project_id, analyzer.spec.id)?;
            let cache_directory = sandbox::canonical(build_cache.path());
            let (checked, note) = if analyzer.spec.checks_need_prepare {
                self.checks_prepared(repo, new_sha, analyzer, &cache_directory)?
            } else {
                let checked = self.flow.with_tree(project_id, repo, new_sha, |tree| {
                    analyzers::run_checks(
                        analyzer.spec,
                        &analyzer.launcher,
                        tree,
                        &cache_directory,
                        self.flow.tool_path(),
                        self.flow.settings(),
                    )
                })?;
                (checked, None)
            };
            let output = match checked {
                Ok(output) => Arc::new(output),
                Err(message) => return Ok(Err(message)),
            };
            // Without the dependencies the result is not kept: the network
            // may be back next time.
            if note.is_some() {
                return Ok(Ok((output, note)));
            }
            if let Some(file) = &cached {
                if let Ok(bytes) = serde_json::to_vec(&*output) {
                    // A cache that cannot be written only costs a later rerun.
                    let _ = disk_cache::write(file, &bytes, KEEP_CHECKS);
                }
            }
            if let Some(stamp) = stamp {
                self.working
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        (project_id.to_string(), analyzer.spec.id),
                        WorkingChecks {
                            fingerprint: analyzer.fingerprint.clone(),
                            stamp,
                            output: Arc::clone(&output),
                        },
                    );
            }
            Ok(Ok((output, None)))
        })
    }

    /// Runs the checkers of `analyzer` on a throwaway copy of the new side
    /// that `prepare` ran on first, so the project's dependencies are there,
    /// with a note when they could not be installed.
    fn checks_prepared(
        &self,
        repo: &Path,
        new_sha: Option<&str>,
        analyzer: &Analyzer,
        cache_directory: &Path,
    ) -> AppResult<(Result<ChecksOutput, CoreText>, Option<CoreText>)> {
        let scratch = ScratchTree::create(&self.cache.join(CHECK_SCRATCH))?;
        let tree = sandbox::canonical(scratch.path()).join("tree");
        fs::create_dir(&tree).map_err(|error| {
            app_error!(
                Internal,
                FileWriteFailed,
                "Could not write {path}: {detail}",
                path = tree.display(),
                detail = error
            )
        })?;
        let git = self.flow.git();
        match new_sha {
            Some(sha) => git.archive(repo, sha, |archive| trees::extract_tar(archive, &tree))?,
            None => trees::copy_working_tree(git, repo, &tree)?,
        }
        let path = self.flow.tool_path();
        let prepared = analyzers::run_prepare(
            analyzer.spec,
            &analyzer.launcher,
            &tree,
            cache_directory,
            path,
            PREPARE_TIMEOUT,
        );
        let checked = analyzers::run_checks(
            analyzer.spec,
            &analyzer.launcher,
            &tree,
            cache_directory,
            path,
            self.flow.settings(),
        );
        Ok(match (checked, prepared) {
            (Ok(output), Err(why)) => (Ok(output), Some(dependencies_missing(why))),
            (checked, _) => (checked, None),
        })
    }

    fn remembered_working(
        &self,
        project_id: &str,
        analyzer: &Analyzer,
        stamp: u64,
    ) -> Option<Arc<ChecksOutput>> {
        self.working
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(project_id.to_string(), analyzer.spec.id))
            .filter(|kept| kept.stamp == stamp && kept.fingerprint == analyzer.fingerprint)
            .map(|kept| Arc::clone(&kept.output))
    }
}

/// Why the checkers ran without the project's dependencies.
fn dependencies_missing(why: CoreText) -> CoreText {
    core_text!(
        ChecksWithoutDependencies,
        "The project's dependencies could not be installed for the static checks, so the checkers that need them may be unavailable or report more: {detail}",
        detail = why
    )
}

/// A cached checker result, or `None` when there is none or it cannot be
/// used, in which case it is deleted.
fn read_cached(file: &Path, analyzer: &Analyzer) -> Option<Arc<ChecksOutput>> {
    let bytes = fs::read(file).ok()?;
    match runner_protocol::parse_checks(&bytes, analyzer.spec.languages, analyzer.spec.title()) {
        Ok(output) => {
            disk_cache::touch(file);
            Some(Arc::new(output))
        }
        Err(_) => {
            let _ = fs::remove_file(file);
            None
        }
    }
}

/// A cached duplicate code search, or `None` when there is none or it cannot
/// be used, in which case it is deleted.
fn read_cached_dupes(file: &Path) -> Option<Arc<Found>> {
    let bytes = fs::read(file).ok()?;
    match serde_json::from_slice::<Found>(&bytes) {
        Ok(found) => {
            disk_cache::touch(file);
            Some(Arc::new(found))
        }
        Err(_) => {
            let _ = fs::remove_file(file);
            None
        }
    }
}

fn dupes_abandoned() -> AppError {
    app_error!(
        Internal,
        DuplicateSearchAbandoned,
        "The duplicate code search stopped unexpectedly."
    )
}

/// One language of the review.
pub struct LanguageInput<'a> {
    pub facts: &'a ReviewFacts<'a>,
    /// Coverage of the run used, when it measured this language.
    pub coverage: Option<&'a CoverageIndex>,
}

pub struct QualityInput<'a> {
    pub language: String,
    pub old_sha: &'a str,
    pub new_sha: Option<&'a str>,
    pub limits: QualityLimits,
    pub languages: Vec<LanguageInput<'a>>,
    pub lines: &'a ChangedLines,
    /// The last completed test run of the new side.
    pub run: Option<(&'a RunRecord, &'a TestOutput)>,
    pub checks: Vec<StaticCheck>,
    /// The duplicate code searches of the new side, not yet placed on
    /// functions: production code's, and test code's.
    pub duplicates: Vec<Duplicate>,
    pub test_duplicates: Vec<Duplicate>,
    pub diagnostics: Vec<CoreText>,
}

/// Where the coverage of `run` stands. The reasons are the run's
/// diagnostics that speak of coverage, as every analyzer words why it
/// measured none ("coverage was not measured: ...", "coverage of <unit> was
/// not measured: ...").
pub fn coverage_state(run: Option<(&RunRecord, &TestOutput)>) -> QualityCoverage {
    let state = |state| QualityCoverage {
        state,
        reasons: Vec::new(),
    };
    match run {
        None => state(CoverageState::NoRun),
        Some((record, _)) if !record.options.coverage => state(CoverageState::Off),
        Some((_, output)) if output.coverage.is_some() => state(CoverageState::Measured),
        Some((_, output)) => QualityCoverage {
            state: CoverageState::NotMeasured,
            reasons: output
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.to_ascii_lowercase().contains("coverage"))
                .cloned()
                .collect(),
        },
    }
}

/// Lines of a function's code kept in the report, at most: a function that
/// crosses a limit is handed to a coding agent with its code, and one longer
/// than this is better read in the editor anyway.
pub const MAX_FUNCTION_LINES: usize = 400;

/// Fills the code of every function of `report` that crosses a limit, read
/// from `tree`, the new side. A file that cannot be read leaves it empty.
pub fn with_function_code(report: &mut QualityReport, tree: &Path) {
    let crossing: std::collections::HashSet<&str> = report
        .crossed
        .iter()
        .filter_map(|limit| limit.function.as_ref().map(|function| function.id.as_str()))
        .collect();
    let mut files: HashMap<String, Option<Vec<String>>> = HashMap::new();
    for function in report.functions.iter_mut() {
        if !crossing.contains(function.id.as_str()) {
            continue;
        }
        let lines = files
            .entry(function.file.clone())
            .or_insert_with(|| duplication::read_lines(tree, &function.file));
        if let Some(lines) = lines {
            let skip = (function.start_line as usize).saturating_sub(1);
            let take = ((function.end_line.saturating_sub(function.start_line)) as usize + 1)
                .min(MAX_FUNCTION_LINES);
            function.code = lines.iter().skip(skip).take(take).cloned().collect();
        }
    }
}

/// CRAP of a function: complexity² × (1 − coverage)³ + complexity. A
/// function without coverable lines leaves nothing uncovered.
pub fn crap(complexity: u32, coverage: LineCoverage) -> f64 {
    let complexity = f64::from(complexity);
    let uncovered = 1.0 - coverage.ratio().unwrap_or(1.0);
    complexity * complexity * uncovered.powi(3) + complexity
}

pub fn assemble(input: QualityInput<'_>) -> QualityReport {
    let limits = input.limits;
    let mut functions: Vec<QualityFunction> = Vec::new();
    for language in &input.languages {
        for node in language.facts.production() {
            let (Some(file), Some(start_line), Some(end_line)) =
                (node.file.as_ref(), node.start_line, node.end_line)
            else {
                continue;
            };
            let state = match node.state {
                FlowNodeState::New => ChangedState::New,
                _ => ChangedState::Modified,
            };
            let changed_lines = language.coverage.map(|index| {
                let lines = input
                    .lines
                    .added_lines(file)
                    .filter(|line| (start_line..=end_line).contains(line));
                index.over(file, lines)
            });
            let within = language
                .coverage
                .map(|index| index.within(file, start_line, end_line));
            let crap = within
                .zip(node.complexity)
                .map(|(within, complexity)| crap(complexity, within));
            functions.push(QualityFunction {
                id: node.id.clone(),
                qualified_name: node.qualified_name.clone(),
                name: node.name.clone(),
                container: node.container.clone(),
                file: file.clone(),
                start_line,
                end_line,
                untested: within.is_some_and(|within| within.coverable > 0 && within.covered == 0),
                state,
                complexity: node.complexity,
                previous_complexity: match state {
                    ChangedState::New => None,
                    ChangedState::Modified => node.previous_complexity,
                },
                changed_lines,
                crap,
                code: Vec::new(),
            });
        }
    }
    functions.sort_by(|left, right| {
        right
            .complexity
            .cmp(&left.complexity)
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut crossed = Vec::new();
    for function in &functions {
        let reference = || {
            Some(FunctionRef {
                id: function.id.clone(),
                qualified_name: function.qualified_name.clone(),
            })
        };
        if let Some(complexity) = function.complexity.filter(|c| *c > limits.complexity) {
            crossed.push(CrossedLimit {
                kind: LimitKind::Complexity,
                function: reference(),
                value: f64::from(complexity),
                limit: f64::from(limits.complexity),
            });
        }
        if let Some(crap) = function.crap.filter(|crap| *crap > limits.crap) {
            crossed.push(CrossedLimit {
                kind: LimitKind::Crap,
                function: reference(),
                value: crap,
                limit: limits.crap,
            });
        }
        // The function's own changed lines, below the target for the review's.
        if let Some(ratio) = function
            .changed_lines
            .and_then(LineCoverage::ratio)
            .filter(|ratio| *ratio < limits.changed_lines_coverage)
        {
            crossed.push(CrossedLimit {
                kind: LimitKind::ChangedLinesCoverage,
                function: reference(),
                value: ratio,
                limit: limits.changed_lines_coverage,
            });
        }
    }
    let changed_ratio = input
        .languages
        .iter()
        .filter_map(|language| language.coverage)
        .map(|index| index.changed(input.lines).0)
        .reduce(|total, more| LineCoverage {
            covered: total.covered + more.covered,
            coverable: total.coverable + more.coverable,
        })
        .and_then(LineCoverage::ratio);
    if let Some(ratio) = changed_ratio.filter(|ratio| *ratio < limits.changed_lines_coverage) {
        crossed.push(CrossedLimit {
            kind: LimitKind::ChangedLinesCoverage,
            function: None,
            value: ratio,
            limit: limits.changed_lines_coverage,
        });
    }

    let race_detector = match input.run {
        Some((_, output)) if output.race => {
            if output.races.is_empty() {
                RaceDetectorState::Clean
            } else {
                RaceDetectorState::Races
            }
        }
        _ => RaceDetectorState::NotRun,
    };
    let check_findings = signals::locate_checks(&input.checks, &functions, input.lines);
    let mut duplicates = input.duplicates;
    duplication::locate(&mut duplicates, &functions);
    duplication::sort(&mut duplicates);
    // Test code holds no function of the report.
    let mut test_duplicates = input.test_duplicates;
    duplication::sort(&mut test_duplicates);
    let coverage = coverage_state(input.run);
    QualityReport {
        language: input.language,
        old_sha: input.old_sha.to_string(),
        new_sha: input.new_sha.map(str::to_string),
        limits,
        functions,
        crossed,
        checks: input.checks,
        check_findings,
        duplicates,
        test_duplicates,
        race_detector,
        test_run_at: input.run.map(|(record, _)| record.started_at.clone()),
        coverage,
        diagnostics: input.diagnostics,
    }
}

#[cfg(test)]
mod assembly_tests {
    use super::*;
    use crate::analysis::fixtures::{analysis, call, function, in_test};
    use crate::analysis::Analysis;
    use crate::model::{Isolation, TestRunStatus};
    use crate::runner_protocol::fixtures::{block, output};
    use crate::runner_protocol::{CoverageBlock, RunnerRace};

    fn with_complexity(mut function: crate::analysis::Function, complexity: u32) -> crate::analysis::Function {
        function.complexity = Some(complexity);
        function
    }

    /// p.A modified (complexity 4 to 12), p.B new (2), p.Z new without a
    /// complexity, a new test, a new generated function, an untouched one.
    fn sides() -> (Analysis, Analysis, ChangedLines) {
        let old = analysis(
            vec![
                with_complexity(function("p.A", "a.go", 1, 10), 4),
                function("p.C", "c.go", 1, 3),
            ],
            Vec::new(),
        );
        let mut generated = function("p.Gen", "gen.go", 1, 3);
        generated.generated = true;
        let new = analysis(
            vec![
                with_complexity(function("p.A", "a.go", 1, 10), 12),
                with_complexity(function("p.B", "b.go", 1, 5), 2),
                function("p.Z", "z.go", 1, 2),
                in_test(with_complexity(function("p.TestB", "b_test.go", 1, 4), 1)),
                generated,
                function("p.C", "c.go", 1, 3),
            ],
            vec![call("p.TestB", "p.B", 2)],
        );
        let lines = ChangedLines::default()
            .with_added("a.go", 3, 4)
            .with_removed("a.go", 3, 3)
            .with_added("b.go", 1, 5)
            .with_added("z.go", 1, 2)
            .with_added("b_test.go", 1, 4)
            .with_added("gen.go", 1, 3);
        (old, new, lines)
    }

    fn record() -> RunRecord {
        RunRecord {
            isolation: Isolation::Sandbox,
            started_at: "2026-11-17T10:00:00Z".to_string(),
            duration_ms: 10,
            status: TestRunStatus::Completed,
            message: None,
            output: None,
            options: Default::default(),
            partial: None,
        }
    }

    fn report(blocks: Option<Vec<CoverageBlock>>, races: Vec<RunnerRace>, race: bool) -> QualityReport {
        let (old, new, lines) = sides();
        let facts = ReviewFacts::new(&old, &new, &lines);
        let mut run_output = output(Vec::new(), blocks);
        run_output.races = races;
        run_output.race = race;
        let index = run_output
            .coverage
            .as_ref()
            .map(|coverage| CoverageIndex::new(&coverage.blocks));
        let record = record();
        assemble(QualityInput {
            language: "go".to_string(),
            old_sha: "old",
            new_sha: Some("new"),
            limits: QualityLimits::default(),
            languages: vec![LanguageInput {
                facts: &facts,
                coverage: index.as_ref(),
            }],
            lines: &lines,
            run: Some((&record, &run_output)),
            checks: Vec::new(),
            duplicates: Vec::new(),
            test_duplicates: Vec::new(),
            diagnostics: vec!["note".into()],
        })
    }

    #[test]
    fn functions_carry_complexity_coverage_and_crap_and_cross_limits() {
        let report = report(
            Some(vec![
                // p.A: 2-8 coverable, 2-3 ran; changed lines 3 (ran), 4 (not).
                block("a.go", 2, 3, 1),
                block("a.go", 4, 8, 0),
                // p.B: every line ran.
                block("b.go", 1, 5, 1),
            ]),
            Vec::new(),
            true,
        );

        let names: Vec<_> = report.functions.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(names, vec!["p.A", "p.B", "p.Z"], "by complexity, unknown last");
        let a = &report.functions[0];
        assert_eq!(a.state, ChangedState::Modified);
        assert_eq!((a.complexity, a.previous_complexity), (Some(12), Some(4)));
        assert_eq!(a.changed_lines, Some(LineCoverage { covered: 1, coverable: 2 }));
        let expected = 144.0 * (5.0f64 / 7.0).powi(3) + 12.0;
        assert!((a.crap.unwrap() - expected).abs() < 1e-9, "{:?}", a.crap);
        let b = &report.functions[1];
        assert_eq!((b.state, b.previous_complexity), (ChangedState::New, None));
        assert_eq!(b.crap, Some(2.0));
        assert_eq!(b.changed_lines, Some(LineCoverage { covered: 5, coverable: 5 }));
        let z = &report.functions[2];
        assert_eq!((z.complexity, z.crap), (None, None));
        assert_eq!(z.changed_lines, Some(LineCoverage { covered: 0, coverable: 0 }));

        let crossed: Vec<_> = report
            .crossed
            .iter()
            .map(|c| (c.kind, c.function.as_ref().map(|f| f.id.as_str()), c.limit))
            .collect();
        assert_eq!(
            crossed,
            vec![
                (LimitKind::Complexity, Some("p.A"), 10.0),
                (LimitKind::Crap, Some("p.A"), 30.0),
                // 1 of p.A's 2 changed lines; 6 of the review's 7, above 0.8.
                (LimitKind::ChangedLinesCoverage, Some("p.A"), 0.8),
            ],
        );
        assert_eq!(report.crossed[0].value, 12.0);
        assert_eq!(report.crossed[2].value, 0.5);
        assert_eq!((a.start_line, a.end_line), (1, 10));
        assert!(
            report.functions.iter().all(|f| !f.untested),
            "p.A and p.B ran, p.Z has nothing to run"
        );
        assert_eq!(report.race_detector, RaceDetectorState::Clean);
        assert_eq!(report.test_run_at.as_deref(), Some("2026-11-17T10:00:00Z"));
        assert_eq!(report.diagnostics, vec!["note"]);
        assert_eq!(report.limits, QualityLimits::default());
    }

    #[test]
    fn low_changed_line_coverage_is_crossed_last_and_races_are_reported() {
        let report = report(
            Some(vec![block("a.go", 2, 8, 0), block("b.go", 1, 5, 0)]),
            vec![RunnerRace {
                container: "pkg".to_string(),
                test: None,
                output: "race".to_string(),
            }],
            true,
        );
        let last = report.crossed.last().unwrap();
        assert_eq!(last.kind, LimitKind::ChangedLinesCoverage);
        assert_eq!((last.function.as_ref(), last.value, last.limit), (None, 0.0, 0.8));
        assert_eq!(report.race_detector, RaceDetectorState::Races);
        // p.B uncovered: 2² × 1 + 2.
        assert_eq!(report.functions[1].crap, Some(6.0));
        let untested: Vec<_> = report
            .functions
            .iter()
            .map(|f| (f.id.as_str(), f.untested))
            .collect();
        assert_eq!(untested, vec![("p.A", true), ("p.B", true), ("p.Z", false)]);
        let coverage: Vec<_> = report
            .crossed
            .iter()
            .filter(|c| c.kind == LimitKind::ChangedLinesCoverage)
            .map(|c| (c.function.as_ref().map(|f| f.id.as_str()), c.value))
            .collect();
        assert_eq!(
            coverage,
            vec![(Some("p.A"), 0.0), (Some("p.B"), 0.0), (None, 0.0)],
            "each function's changed lines, then the review's"
        );
    }

    #[test]
    fn checks_on_added_lines_are_placed_on_their_functions() {
        let (old, new, lines) = sides();
        let facts = ReviewFacts::new(&old, &new, &lines);
        let finding = |file: &str, line: u32, rule: &str| crate::model::CheckFinding {
            file: file.to_string(),
            line,
            message: "message".to_string(),
            rule: Some(rule.to_string()),
            severity: crate::model::CheckSeverity::Warning,
        };
        let report = assemble(QualityInput {
            language: "go".to_string(),
            old_sha: "old",
            new_sha: Some("new"),
            limits: QualityLimits::default(),
            languages: vec![LanguageInput {
                facts: &facts,
                coverage: None,
            }],
            lines: &lines,
            run: None,
            checks: vec![StaticCheck {
                tool: "staticcheck".to_string(),
                status: crate::model::CheckStatus::Findings,
                message: None,
                // a.go:3 is added in p.A, a.go:9 is not; b.go:2 is in p.B.
                findings: vec![finding("a.go", 3, "SA4006"), finding("a.go", 9, "SA4006"), finding("b.go", 2, "ST1005")],
            }],
            duplicates: Vec::new(),
            test_duplicates: Vec::new(),
            diagnostics: Vec::new(),
        });
        let located: Vec<_> = report
            .check_findings
            .iter()
            .map(|c| (c.file.as_str(), c.line, c.class, c.function.as_ref().map(|f| f.id.as_str())))
            .collect();
        assert_eq!(
            located,
            vec![
                ("a.go", 3, crate::model::CheckClass::Correctness, Some("p.A")),
                ("b.go", 2, crate::model::CheckClass::Style, Some("p.B")),
            ]
        );
        assert_eq!(report.checks[0].findings.len(), 3, "the Quality tab keeps them all");
        assert!(report.functions.iter().all(|f| !f.untested), "nothing is known without a run");
    }

    #[test]
    fn duplicates_are_sorted_and_placed_on_changed_production_functions() {
        use crate::duplication::fixtures::{block, duplicate};
        use crate::model::DuplicateKind;
        let (old, new, lines) = sides();
        let facts = ReviewFacts::new(&old, &new, &lines);
        let report = assemble(QualityInput {
            language: "go".to_string(),
            old_sha: "old",
            new_sha: Some("new"),
            limits: QualityLimits::default(),
            languages: vec![LanguageInput {
                facts: &facts,
                coverage: None,
            }],
            lines: &lines,
            run: None,
            checks: Vec::new(),
            duplicates: vec![
                duplicate(DuplicateKind::CopiesExisting, block("z.go", 1, 2), vec![block("c.go", 1, 3)]),
                duplicate(DuplicateKind::CopiesExisting, block("b.go", 1, 5), vec![block("c.go", 1, 3)]),
            ],
            test_duplicates: vec![
                duplicate(DuplicateKind::RepeatedInChange, block("z_test.go", 1, 4), vec![block("b_test.go", 6, 9)]),
                duplicate(DuplicateKind::RepeatedInChange, block("b_test.go", 1, 4), vec![block("b_test.go", 6, 9)]),
            ],
            diagnostics: Vec::new(),
        });
        let placed: Vec<_> = report
            .duplicates
            .iter()
            .map(|d| (d.block.file.as_str(), d.function.as_ref().map(|f| f.id.as_str())))
            .collect();
        assert_eq!(placed, vec![("b.go", Some("p.B")), ("z.go", Some("p.Z"))]);
        let tests: Vec<_> = report
            .test_duplicates
            .iter()
            .map(|d| (d.block.file.as_str(), d.function.as_ref().map(|f| f.id.as_str())))
            .collect();
        // Test code holds no function of the report.
        assert_eq!(tests, vec![("b_test.go", None), ("z_test.go", None)]);
    }

    #[test]
    fn the_report_says_where_coverage_stands() {
        let measured = report(Some(vec![block("a.go", 2, 8, 1)]), Vec::new(), true);
        assert_eq!(measured.coverage, QualityCoverage { state: CoverageState::Measured, reasons: Vec::new() });

        // The run that left every function's coverage and CRAP empty: it was
        // asked for coverage, and the analyzer measured none.
        let mut run_output = output(Vec::new(), None);
        run_output.diagnostics = vec![
            "coverage was not measured: coverage.py is not installed in the project's environment (/envs/acme/bin/python)".to_string(),
            "the package tests.slow was skipped".to_string(),
        ];
        let record = record();
        assert_eq!(
            coverage_state(Some((&record, &run_output))),
            QualityCoverage {
                state: CoverageState::NotMeasured,
                reasons: vec![
                    "coverage was not measured: coverage.py is not installed in the project's environment (/envs/acme/bin/python)".to_string()
                ],
            }
        );

        let mut off = record.clone();
        off.options.coverage = false;
        assert_eq!(coverage_state(Some((&off, &run_output))).state, CoverageState::Off);
        assert_eq!(coverage_state(None).state, CoverageState::NoRun);
    }

    #[test]
    fn without_coverage_or_a_run_there_is_no_coverage_crap_or_race_detector() {
        let report = report(None, Vec::new(), false);
        assert!(report
            .functions
            .iter()
            .all(|f| f.changed_lines.is_none() && f.crap.is_none()));
        assert_eq!(report.crossed.len(), 1, "only the complexity of p.A");
        assert_eq!(report.race_detector, RaceDetectorState::NotRun);

        let (old, new, lines) = sides();
        let facts = ReviewFacts::new(&old, &new, &lines);
        let report = assemble(QualityInput {
            language: "go".to_string(),
            old_sha: "old",
            new_sha: None,
            limits: QualityLimits::default(),
            languages: vec![LanguageInput {
                facts: &facts,
                coverage: None,
            }],
            lines: &lines,
            run: None,
            checks: Vec::new(),
            duplicates: Vec::new(),
            test_duplicates: Vec::new(),
            diagnostics: Vec::new(),
        });
        assert_eq!(report.race_detector, RaceDetectorState::NotRun);
        assert_eq!(report.test_run_at, None);
        assert_eq!(report.new_sha, None);
        assert_eq!(report.coverage.state, CoverageState::NoRun);
    }

    #[test]
    fn crap_follows_the_formula() {
        let none = LineCoverage { covered: 0, coverable: 4 };
        let half = LineCoverage { covered: 2, coverable: 4 };
        assert_eq!(crap(5, none), 30.0);
        assert_eq!(crap(4, half), 16.0 * 0.125 + 4.0);
        assert_eq!(crap(3, LineCoverage::default()), 3.0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{analysis, call, function, in_test};
    use crate::analysis::Analysis;
    use crate::analyzers::{AnalyzerLocator, RunSettings};
    use crate::model::{CheckStatus, FlowUnavailable, Isolation, ReviewSide};
    use crate::runner_protocol::fixtures::{block, output, test};
    use crate::test_runs::fixtures::{calls, fake_runner, fake_sandbox_exec};
    use crate::test_runs::TestSettings;
    use crate::test_support::{isolated_git, TestRepo};
    use crate::tool_path::ToolPath;
    use std::time::Duration;
    use tempfile::TempDir;

    const PROJECT: &str = "project-1";

    struct Harness {
        repo: TestRepo,
        tools: TempDir,
        cache: TempDir,
        tests: Arc<TestsService>,
        service: QualityService,
    }

    impl Harness {
        fn new() -> Self {
            let tools = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let (tests, service) = services(tools.path(), cache.path());
            Self {
                repo: TestRepo::init(),
                tools,
                cache,
                tests,
                service,
            }
        }

        fn quality(&self, service: &QualityService, old: &str, new: Option<&str>) -> QualityResult {
            service.quality(PROJECT, self.repo.path(), old, new).unwrap()
        }

        fn report(&self, old: &str, new: Option<&str>) -> QualityReport {
            match self.quality(&self.service, old, new) {
                QualityResult::Report { report } => *report,
                other => panic!("expected a report, got {other:?}"),
            }
        }

        fn write(&self, files: &[(&str, &str)], analysis: &Analysis) {
            for (path, contents) in files {
                self.repo.write(path, contents);
            }
            self.repo.write(
                "fixture/analysis.json",
                serde_json::to_string(analysis).unwrap(),
            );
            let tests = output(
                vec![test("TestB", Some("p.TestB"))],
                Some(vec![block("b.go", 1, 3, 1), block("a.go", 1, 10, 0)]),
            );
            self.repo
                .write("fixture/tests.json", serde_json::to_string(&tests).unwrap());
            self.repo.write(
                "fixture/checks.json",
                serde_json::json!({
                    "protocol": 1, "language": "go",
                    "runner": { "name": "difflens-analyzer-go", "version": "0.0.1" },
                    "checks": [
                        { "tool": "go vet", "status": "findings", "message": null,
                          "findings": [{ "file": "b.go", "line": 2, "message": "unreachable code", "rule": "unreachable" }] },
                        { "tool": "staticcheck", "status": "unavailable", "message": "not installed", "findings": [] }
                    ],
                    "diagnostics": ["vet: one package skipped"]
                })
                .to_string(),
            );
        }

        fn commit(&self, files: &[(&str, &str)], analysis: &Analysis) -> String {
            self.write(files, analysis);
            self.repo.commit_all("commit");
            self.repo.git(&["rev-parse", "HEAD"])
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

    fn services(tools: &Path, cache: &Path) -> (Arc<TestsService>, QualityService) {
        services_for("go", tools, cache)
    }

    /// With the fake analyzer standing in for the analyzer `id`.
    fn services_for(id: &'static str, tools: &Path, cache: &Path) -> (Arc<TestsService>, QualityService) {
        let runner = tools.join("fake analyzer");
        if !runner.exists() {
            fake_runner(tools);
        }
        let flow = Arc::new(FlowService::new(
            isolated_git(),
            cache.to_path_buf(),
            AnalyzerLocator::with_executable(id, runner),
            ToolPath::fixed("/usr/bin:/bin"),
            RunSettings::default(),
        ));
        let tests = Arc::new(TestsService::new(
            Arc::clone(&flow),
            cache.to_path_buf(),
            TestSettings {
                prepare_timeout: Duration::from_secs(30),
                runner_timeout: Duration::from_secs(600),
                hard_timeout: Duration::from_secs(30),
                max_output_bytes: 10 * 1024 * 1024,
                sandbox_exec: fake_sandbox_exec(tools),
                temporary_root: tools.to_path_buf(),
            },
        ));
        let quality = QualityService::new(flow, Arc::clone(&tests), cache.to_path_buf());
        (tests, quality)
    }

    fn numbered_lines(count: u32) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    fn with_complexity(mut function: crate::analysis::Function, complexity: u32) -> crate::analysis::Function {
        function.complexity = Some(complexity);
        function
    }

    /// a.go edited (p.A, complexity 3 to 11), b.go and its test added.
    fn two_commits(harness: &Harness) -> (String, String) {
        let old = harness.commit(
            &[("go.mod", "module example.com/p\n"), ("a.go", &numbered_lines(10))],
            &analysis(vec![with_complexity(function("p.A", "a.go", 1, 10), 3)], Vec::new()),
        );
        let new = harness.commit(
            &[
                ("a.go", &numbered_lines(10).replace("line 3\n", "line three\n")),
                ("b.go", &numbered_lines(3)),
                ("b_test.go", &numbered_lines(3)),
            ],
            &analysis(
                vec![
                    with_complexity(function("p.A", "a.go", 1, 10), 11),
                    with_complexity(function("p.B", "b.go", 1, 3), 1),
                    in_test(function("p.TestB", "b_test.go", 1, 3)),
                ],
                vec![call("p.TestB", "p.B", 2)],
            ),
        );
        (old, new)
    }

    #[test]
    fn reports_functions_and_checks_without_running_tests_and_caches_checks() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);

        let report = harness.report(&old, Some(&new));

        let ids: Vec<_> = report.functions.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["p.A", "p.B"], "the test is not production code");
        // p.A crosses the complexity limit: its code comes with it, from the
        // new side; p.B crosses none.
        assert_eq!(report.functions[0].code.len(), 10);
        assert_eq!(report.functions[0].code[2], "line three");
        assert!(report.functions[1].code.is_empty());
        assert!(report.functions.iter().all(|f| f.crap.is_none() && f.changed_lines.is_none()));
        assert_eq!(report.race_detector, RaceDetectorState::NotRun);
        assert_eq!(report.test_run_at, None);
        assert_eq!(report.crossed.len(), 1);
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.checks[0].tool, "go vet");
        assert_eq!(report.checks[0].status, CheckStatus::Findings);
        assert_eq!(report.checks[1].status, CheckStatus::Unavailable);
        assert_eq!(report.diagnostics, vec!["vet: one package skipped"]);
        assert_eq!(report.check_findings.len(), 1);
        assert_eq!(report.check_findings[0].tool, "go vet");
        assert_eq!(
            report.check_findings[0].function.as_ref().map(|f| f.id.as_str()),
            Some("p.B"),
            "b.go:2, a line the review adds in p.B"
        );
        assert_eq!(calls(harness.tools.path(), "check"), 1);
        assert_eq!(calls(harness.tools.path(), "test"), 0, "never runs project code");
        assert_eq!(calls(harness.tools.path(), "prepare"), 0);

        harness.report(&old, Some(&new));
        assert_eq!(calls(harness.tools.path(), "check"), 1, "cached");
        let (_, restarted) = services(harness.tools.path(), harness.cache.path());
        harness.quality(&restarted, &old, Some(&new));
        assert_eq!(calls(harness.tools.path(), "check"), 1, "cached on disk");
        let cached = harness.cache.path().join("checks").join(PROJECT).join("go");
        assert_eq!(fs::read_dir(cached).unwrap().count(), 1);
    }

    /// c.go added on top of `old`, its lines 1 to 8 a copy of a.go's, as the
    /// fake's dupes.json says.
    fn copy_commit(harness: &Harness, old_lines: u32) -> String {
        harness.repo.write(
            "fixture/dupes.json",
            serde_json::json!({
                "protocol": 1, "language": "go",
                "runner": { "name": "difflens-analyzer-go", "version": "0.0.1" },
                "minTokens": 50, "minLines": 6,
                "duplicates": [
                    { "kind": "copies_existing", "scope": "production",
                      "block": { "file": "c.go", "startLine": 1, "endLine": 8 },
                      "others": [{ "file": "a.go", "startLine": 1, "endLine": old_lines }],
                      "tokens": 72, "lines": 8 },
                    { "kind": "copies_existing", "scope": "test",
                      "block": { "file": "c_test.go", "startLine": 1, "endLine": 6 },
                      "others": [{ "file": "b_test.go", "startLine": 1, "endLine": 3 }],
                      "tokens": 60, "lines": 6 }
                ],
                "diagnostics": ["dupes: one file skipped"]
            })
            .to_string(),
        );
        harness.commit(
            &[("c.go", &numbered_lines(8)), ("c_test.go", &numbered_lines(6))],
            &analysis(
                vec![
                    with_complexity(function("p.A", "a.go", 1, 10), 11),
                    with_complexity(function("p.C", "c.go", 1, 8), 1),
                ],
                Vec::new(),
            ),
        )
    }

    #[test]
    fn duplicates_come_from_dupes_given_the_added_lines_and_are_cached() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        let new = copy_commit(&harness, 4);

        let report = harness.report(&old, Some(&new));

        assert_eq!(report.duplicates.len(), 1);
        let duplicate = &report.duplicates[0];
        assert_eq!(duplicate.kind, crate::model::DuplicateKind::CopiesExisting);
        assert_eq!((duplicate.block.file.as_str(), duplicate.block.start_line, duplicate.block.end_line), ("c.go", 1, 8));
        assert_eq!(duplicate.block.code.len(), 8);
        assert_eq!(duplicate.block.code[7], "line 8");
        assert_eq!(
            duplicate.others[0].code,
            vec!["line 1", "line 2", "line three", "line 4"],
            "read from the new side"
        );
        assert_eq!((duplicate.tokens, duplicate.lines), (72, 8));
        assert_eq!(
            duplicate.function.as_ref().map(|f| f.id.as_str()),
            Some("p.C"),
            "on the changed function holding it"
        );
        assert!(report.diagnostics.iter().any(|d| d == "dupes: one file skipped"), "{:?}", report.diagnostics);
        // The tests' own copy is apart, never among the production ones.
        assert_eq!(report.test_duplicates.len(), 1);
        assert_eq!(report.test_duplicates[0].block.file, "c_test.go");
        assert_eq!(report.test_duplicates[0].block.code.len(), 6);
        assert_eq!(report.test_duplicates[0].function, None);

        let arguments = fs::read_to_string(harness.tools.path().join("dupes-args")).unwrap();
        assert!(arguments.contains("--min-tokens 50 --min-lines 6"), "{arguments}");
        assert!(arguments.contains("--changed "), "{arguments}");
        let changed: serde_json::Value =
            serde_json::from_slice(&fs::read(harness.tools.path().join("dupes-changed")).unwrap()).unwrap();
        let files = changed["files"].as_array().unwrap();
        assert!(
            files.contains(&serde_json::json!({ "path": "c.go", "added": [[1, 8]] })),
            "{changed}"
        );
        assert!(
            files.iter().any(|file| file["path"] == "a.go"),
            "from old, so a.go's edit is in too: {changed}"
        );
        assert_eq!(
            changed["exclude"]["dirs"],
            serde_json::json!(duplication::TRANSLATION_DIRS),
            "the translation catalogs to leave out: {changed}"
        );
        assert_eq!(
            fs::read_dir(harness.cache.path().join("scratch")).map_or(0, |dir| dir.count()),
            0,
            "the changed file is deleted"
        );
        assert_eq!(calls(harness.tools.path(), "dupes"), 1);

        harness.report(&old, Some(&new));
        assert_eq!(calls(harness.tools.path(), "dupes"), 1, "cached");
        let (_, restarted) = services(harness.tools.path(), harness.cache.path());
        harness.quality(&restarted, &old, Some(&new));
        assert_eq!(calls(harness.tools.path(), "dupes"), 1, "cached on disk");
        let kept = harness.cache.path().join("duplicates").join(PROJECT).join("go");
        assert_eq!(fs::read_dir(kept).unwrap().count(), 1);

        // Another old side adds other lines: searched again.
        let middle = harness.repo.git(&["rev-parse", "HEAD~1"]);
        harness.report(&middle, Some(&new));
        assert_eq!(calls(harness.tools.path(), "dupes"), 2);
    }

    #[test]
    fn a_search_kept_by_older_rules_is_never_served() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        let new = copy_commit(&harness, 4);
        harness.report(&old, Some(&new));
        let folder = fs::read_dir(harness.cache.path().join("duplicates").join(PROJECT).join("go"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let kept: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(kept.len(), 1);
        let name = kept[0].file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with(&format!("-r{}.json", duplication::SEARCH_RULES)), "{name}");

        // What a search kept before tests were told apart looked like, where
        // the current rules would read it: test copies among production ones.
        let unscoped = serde_json::json!({
            "minTokens": 50, "minLines": 6,
            "duplicates": [{
                "kind": "copies_existing",
                "block": { "file": "c_test.go", "startLine": 1, "endLine": 6, "code": [] },
                "others": [{ "file": "c.go", "startLine": 1, "endLine": 6, "code": [] }],
                "tokens": 60, "lines": 6, "function": null
            }],
            "diagnostics": []
        });
        fs::write(&kept[0], unscoped.to_string()).unwrap();
        let (_, restarted) = services(harness.tools.path(), harness.cache.path());
        let report = match harness.quality(&restarted, &old, Some(&new)) {
            QualityResult::Report { report } => *report,
            other => panic!("expected a report, got {other:?}"),
        };
        assert_eq!(calls(harness.tools.path(), "dupes"), 2, "searched again");
        assert!(report.duplicates.iter().all(|duplicate| duplicate.block.file != "c_test.go"));
        assert_eq!(report.test_duplicates.len(), 1);
    }

    #[test]
    fn a_failing_duplicate_search_is_a_diagnostic_and_not_kept() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.flag("dupes-fail", true);

        let report = harness.report(&old, Some(&new));

        assert!(report.duplicates.is_empty());
        assert_eq!(report.checks.len(), 2, "the checkers still ran");
        assert!(
            report.diagnostics.iter().any(|d| d.contains("tokenizer crashed")),
            "{:?}",
            report.diagnostics
        );
        harness.flag("dupes-fail", false);
        let report = harness.report(&old, Some(&new));
        assert_eq!(calls(harness.tools.path(), "dupes"), 2, "the failure was not kept");
        assert!(report.diagnostics.iter().all(|d| !d.contains("tokenizer crashed")));
    }

    #[test]
    fn the_working_tree_search_is_kept_in_memory_for_its_old_side() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.repo.write("d.go", numbered_lines(6));

        harness.report(&new, None);
        harness.report(&new, None);
        assert_eq!(calls(harness.tools.path(), "dupes"), 1, "kept while no file changes");
        assert!(!harness.cache.path().join("duplicates").exists(), "not on disk");
        let changed = fs::read_to_string(harness.tools.path().join("dupes-changed")).unwrap();
        assert!(changed.contains(r#"{"path":"d.go","added":[[1,6]]}"#), "untracked counts as added: {changed}");

        harness.report(&old, None);
        assert_eq!(calls(harness.tools.path(), "dupes"), 2, "another old side");
        harness.repo.write("d.go", numbered_lines(7));
        harness.report(&old, None);
        assert_eq!(calls(harness.tools.path(), "dupes"), 3, "a file changed");
    }

    #[test]
    fn coverage_crap_and_races_come_from_the_last_completed_run() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let run = harness
            .tests
            .run(PROJECT, harness.repo.path(), &old, Some(&new), Isolation::Process)
            .unwrap();
        // A later failed run does not hide the completed one.
        harness.flag("test-fail", true);
        harness
            .tests
            .run(PROJECT, harness.repo.path(), &old, Some(&new), Isolation::Process)
            .unwrap();

        let report = harness.report(&old, Some(&new));

        assert_eq!(report.test_run_at, Some(run.started_at));
        assert_eq!(report.race_detector, RaceDetectorState::Clean);
        let a = &report.functions[0];
        assert_eq!(a.changed_lines, Some(LineCoverage { covered: 0, coverable: 1 }));
        assert_eq!(a.crap, Some(121.0 + 11.0));
        let b = &report.functions[1];
        assert_eq!(b.changed_lines, Some(LineCoverage { covered: 3, coverable: 3 }));
        assert_eq!(b.crap, Some(1.0));
        let crossed: Vec<_> = report.crossed.iter().map(|c| (c.kind, c.value)).collect();
        assert_eq!(
            crossed,
            vec![
                (LimitKind::Complexity, 11.0),
                (LimitKind::Crap, 132.0),
                // p.A's one changed line did not run.
                (LimitKind::ChangedLinesCoverage, 0.0),
                // 3 of the 4 changed coverable lines.
                (LimitKind::ChangedLinesCoverage, 0.75),
            ]
        );
        assert!(a.untested, "none of p.A's lines ran");
        assert!(!b.untested);
    }

    #[test]
    fn the_working_tree_is_checked_on_a_copy_kept_in_memory() {
        let harness = Harness::new();
        let (_, new) = two_commits(&harness);
        harness.write(&[("b.go", "changed\n")], &analysis(Vec::new(), Vec::new()));

        harness.report(&new, None);
        harness.report(&new, None);

        assert_eq!(calls(harness.tools.path(), "check"), 1, "kept while no file changes");
        assert_eq!(
            fs::read_dir(harness.cache.path().join("scratch")).unwrap().count(),
            0
        );
        assert!(!harness.cache.path().join("checks").exists(), "not on disk");
        harness.repo.write("b.go", "changed again\n");
        harness.report(&new, None);
        assert_eq!(calls(harness.tools.path(), "check"), 2);
    }

    #[test]
    fn a_failing_checker_is_a_diagnostic_and_not_kept() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.flag("check-fail", true);

        let report = harness.report(&old, Some(&new));

        assert!(report.checks.is_empty());
        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0].contains("vet crashed"), "{:?}", report.diagnostics);
        harness.flag("check-fail", false);
        let report = harness.report(&old, Some(&new));
        assert_eq!(report.checks.len(), 2);
        assert_eq!(calls(harness.tools.path(), "check"), 2);
    }

    #[test]
    fn checkers_that_need_the_dependencies_run_on_a_prepared_copy_with_the_build_cache() {
        let tools = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let (_, service) = services_for("python", tools.path(), cache.path());
        let repo = TestRepo::init();
        let mut analysis = analysis(vec![function("app.main", "app.py", 1, 3)], Vec::new());
        analysis.language = "python".to_string();
        analysis.analyzer.name = "difflens-analyzer-python".to_string();
        let checks = serde_json::json!({
            "protocol": 1, "language": "python",
            "runner": { "name": "difflens-analyzer-python", "version": "0.0.0" },
            "checks": [
                { "tool": "mypy", "status": "findings", "message": null,
                  "findings": [{ "file": "app.py", "line": 2, "message": "Incompatible types", "rule": "assignment", "severity": "error" }] }
            ],
            "diagnostics": []
        });
        // The duplicate code search runs beside the checkers; it finds nothing.
        let dupes = serde_json::json!({
            "protocol": 1, "language": "python",
            "runner": { "name": "difflens-analyzer-python", "version": "0.0.0" },
            "minTokens": 50, "minLines": 6, "duplicates": [], "diagnostics": []
        });
        let commit = |text: &str| {
            repo.write("pyproject.toml", "[project]\nname = \"app\"\n");
            repo.write("app.py", text);
            repo.write("fixture/analysis.json", serde_json::to_string(&analysis).unwrap());
            repo.write("fixture/checks.json", checks.to_string());
            repo.write("fixture/dupes.json", dupes.to_string());
            repo.commit_all("commit");
            repo.git(&["rev-parse", "HEAD"])
        };
        let old = commit("one\n");
        let new = commit("one\ntwo\nthree\n");
        let read = |name: &str| fs::read_to_string(tools.path().join(name)).unwrap_or_default();
        let report = |new: &str| match service.quality(PROJECT, repo.path(), &old, Some(new)).unwrap() {
            QualityResult::Report { report } => *report,
            other => panic!("expected a report, got {other:?}"),
        };

        let first = report(&new);

        assert_eq!((calls(tools.path(), "prepare"), calls(tools.path(), "check")), (1, 1));
        assert_eq!(read("check-tree").trim(), "prepared", "checked where prepare installed");
        let build_cache = sandbox::canonical(&cache.path().join("build-cache").join(PROJECT).join("python"));
        assert!(read("check-args").trim().ends_with(&format!("--cache-dir {}", build_cache.display())), "{}", read("check-args"));
        assert!(read("prepare-args").contains(&format!("--cache-dir {}", build_cache.display())));
        assert_eq!(fs::read_dir(cache.path().join(CHECK_SCRATCH)).unwrap().count(), 0, "the copy is deleted");
        assert_eq!(first.checks[0].findings[0].severity, crate::model::CheckSeverity::Error);
        assert_eq!(first.check_findings[0].severity, crate::model::CheckSeverity::Error);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        report(&new);
        assert_eq!(calls(tools.path(), "check"), 1, "kept per commit");

        // Without the dependencies the checkers still run, and say so; that
        // result is not kept.
        let other = commit("one\ntwo\nfour\n");
        fs::write(tools.path().join("prepare-fail"), "").unwrap();
        let without = report(&other);
        assert_eq!(read("check-tree").trim(), "bare");
        assert_eq!(without.checks.len(), 1);
        assert!(
            without.diagnostics.iter().any(|d| d.contains("dependencies could not be installed")
                && d.contains("go: download failed")),
            "{:?}",
            without.diagnostics
        );
        report(&other);
        assert_eq!(calls(tools.path(), "check"), 3, "checked again");
    }

    #[test]
    fn unavailable_reviews_reuse_the_flow_reasons() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        harness.repo.write("fixture/analysis.json", "broken");
        harness.repo.commit_all("broken");
        let broken = harness.repo.git(&["rev-parse", "HEAD"]);
        match harness.quality(&harness.service, &old, Some(&broken)) {
            QualityResult::Unavailable(FlowUnavailable::AnalysisFailed { side, .. }) => {
                assert_eq!(side, ReviewSide::New)
            }
            other => panic!("expected a failed analysis, got {other:?}"),
        }

        let unknown = Harness::new();
        unknown.repo.write("README.md", "notes\n");
        unknown.repo.commit_all("notes");
        let sha = unknown.repo.git(&["rev-parse", "HEAD"]);
        assert_eq!(
            unknown.quality(&unknown.service, &sha, Some(&sha)),
            QualityResult::Unavailable(FlowUnavailable::NoAnalyzer { languages: vec![] })
        );
    }

    #[test]
    fn quality_serialises_to_the_contract_shape() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness
            .tests
            .run(PROJECT, harness.repo.path(), &old, Some(&new), Isolation::Process)
            .unwrap();

        let json = serde_json::to_value(harness.quality(&harness.service, &old, Some(&new))).unwrap();

        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let sorted = |names: &[&str]| {
            let mut names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
            names.sort();
            names
        };
        assert_eq!(keys(&json), sorted(&["kind", "report"]));
        assert_eq!(json["kind"], "report");
        let report = &json["report"];
        assert_eq!(
            keys(report),
            sorted(&[
                "language", "oldSha", "newSha", "limits", "functions", "crossed", "checks",
                "checkFindings", "duplicates", "testDuplicates", "raceDetector", "testRunAt",
                "coverage", "diagnostics",
            ])
        );
        assert_eq!(report["duplicates"], serde_json::json!([]));
        assert_eq!(report["testDuplicates"], serde_json::json!([]));
        assert_eq!(report["coverage"], serde_json::json!({ "state": "measured", "reasons": [] }));
        assert_eq!(
            serde_json::to_value(CoverageState::NotMeasured).unwrap(),
            "not_measured"
        );
        assert_eq!(serde_json::to_value(CoverageState::NoRun).unwrap(), "no_run");
        let duplicate = serde_json::to_value(crate::duplication::fixtures::duplicate(
            crate::model::DuplicateKind::RepeatedInChange,
            crate::duplication::fixtures::block("a.go", 1, 6),
            vec![crate::duplication::fixtures::block("b.go", 3, 8)],
        ))
        .unwrap();
        assert_eq!(
            duplicate,
            serde_json::json!({
                "kind": "repeated_in_change",
                "block": { "file": "a.go", "startLine": 1, "endLine": 6, "code": [] },
                "others": [{ "file": "b.go", "startLine": 3, "endLine": 8, "code": [] }],
                "tokens": 60, "lines": 6, "function": null
            })
        );
        assert_eq!(
            keys(&report["checkFindings"][0]),
            sorted(&["tool", "rule", "file", "line", "message", "severity", "class", "function"])
        );
        assert_eq!(report["checkFindings"][0]["class"], "correctness");
        assert_eq!(report["checkFindings"][0]["severity"], "warning", "the fixture has none");
        assert_eq!(
            report["limits"],
            serde_json::json!({
                "complexity": 10, "crap": 30.0, "changedLinesCoverage": 0.8,
                "duplicationTokens": 50, "duplicationLines": 6
            })
        );
        assert_eq!(
            keys(&report["functions"][0]),
            sorted(&[
                "id", "qualifiedName", "name", "container", "file", "startLine", "endLine",
                "state", "complexity", "previousComplexity", "changedLines", "crap", "untested",
                "code",
            ])
        );
        assert_eq!(report["functions"][0]["state"], "modified");
        assert_eq!(keys(&report["functions"][0]["changedLines"]), sorted(&["covered", "coverable"]));
        assert_eq!(keys(&report["crossed"][0]), sorted(&["kind", "function", "value", "limit"]));
        assert_eq!(report["crossed"][0]["kind"], "complexity");
        assert_eq!(keys(&report["crossed"][0]["function"]), sorted(&["id", "qualifiedName"]));
        assert_eq!(keys(&report["checks"][0]), sorted(&["tool", "status", "message", "findings"]));
        assert_eq!(
            keys(&report["checks"][0]["findings"][0]),
            sorted(&["file", "line", "message", "rule", "severity"])
        );
        assert_eq!(report["raceDetector"], "clean");
        let crossed = serde_json::to_value(CrossedLimit {
            kind: LimitKind::ChangedLinesCoverage,
            function: None,
            value: 0.5,
            limit: 0.8,
        })
        .unwrap();
        assert_eq!(
            crossed,
            serde_json::json!({ "kind": "changed_lines_coverage", "function": null, "value": 0.5, "limit": 0.8 })
        );
        assert_eq!(serde_json::to_value(RaceDetectorState::NotRun).unwrap(), "not_run");
        let unavailable = serde_json::to_value(QualityResult::Unavailable(FlowUnavailable::NoAnalyzer {
            languages: Vec::new(),
        }))
        .unwrap();
        assert_eq!(
            unavailable,
            serde_json::json!({ "kind": "unavailable", "reason": "no_analyzer", "languages": [] })
        );
    }
}
