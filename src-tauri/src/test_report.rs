//! Turns what a test runner reported into the `TestRun` of one review, at
//! read time, so one run of a commit serves every review whose new side it
//! is. Pure: no git, no disk.
//!
//! - A test's `state` is the state in the review of its test function
//!   (`function`), from the same comparison as the Flow tab; "untouched" when
//!   the runner could not tell the function or the comparison does not list
//!   it.
//! - `exercises`: from the test function, a breadth-first walk of the new
//!   side's relations (calls, registrations, references), each function's
//!   relations in line order; the first new or modified production function
//!   (not test, not generated) reached.
//! - `inChangedPackage`: a changed file of the review holds a function of
//!   the test's container, on the side where the file changed.
//! - Coverage: a line is coverable when a block spans it and covered when a
//!   spanning block ran. Changed lines are the lines the review adds or
//!   changes on the new side; blocks are of production code only, so test
//!   files never count.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, Call};
use crate::error::CoreText;
use crate::flow_graph::{self, GraphRequest, LanguageSides};
use crate::hunks::ChangedLines;
use crate::model::{
    AnalyzerIdentity, FileCoverage, FlowNode, FlowNodeState, FlowOptions, FunctionRef, Isolation,
    LineCoverage, PackageOutcome, PackageOutcomeKind, PartialRun, RaceReport, RunCoverage,
    RunOptions, RunSelection, TestCase, TestFunctionState, TestOutcome, TestRun, TestRunStatus,
    TestSummary,
};
use crate::runner_protocol::{Coverage, CoverageBlock, RunnerPackage, RunnerTest, TestOutput};
use crate::test_failures::{self, Locator};

/// Most output kept for a failed test, a package or a failed run.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Most output kept for one data race.
const MAX_RACE_BYTES: usize = 16 * 1024;

/// One run as it is kept: how it ended and, when the runner reported, its
/// output as printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub isolation: Isolation,
    /// RFC 3339.
    pub started_at: String,
    pub duration_ms: u64,
    pub status: TestRunStatus,
    /// For a failed run: why, at most `MAX_OUTPUT_BYTES` of the runner's own
    /// output. A record kept before messages had reasons holds a plain string,
    /// read back as text without a reason.
    pub message: Option<CoreText>,
    /// For a completed run: the runner's standard output, or for one a
    /// partial run was merged into, the merged output (see `merge`).
    pub output: Option<String>,
    /// What the run asked for; a record kept before had both on.
    #[serde(default)]
    pub options: RunOptions,
    /// The last partial run of this new side, merged into this record when
    /// it completed.
    #[serde(default)]
    pub partial: Option<PartialRun>,
}

/// What the comparison of the two sides of one language says about its
/// functions.
pub struct ReviewFacts<'a> {
    /// New and modified functions, test functions included, generated ones
    /// not, by id.
    changed: HashMap<String, FlowNode>,
    /// Relations of the new side by caller, in line order.
    calls: HashMap<&'a str, Vec<&'a Call>>,
    changed_containers: HashSet<&'a str>,
    /// Files of the new side's functions, to place the paths a failure
    /// prints.
    files: HashSet<&'a str>,
}

impl<'a> ReviewFacts<'a> {
    pub fn new(old: &'a Analysis, new: &'a Analysis, lines: &ChangedLines) -> Self {
        let graph = flow_graph::build(&GraphRequest {
            languages: vec![LanguageSides { old, new }],
            lines,
            old_sha: "",
            new_sha: None,
            analyzed_at: String::new(),
            max_nodes: 0,
            options: FlowOptions {
                include_tests: true,
                include_callers: false,
                include_callees: false,
            },
        });
        let changed = graph
            .nodes
            .into_iter()
            .filter(|node| matches!(node.state, FlowNodeState::New | FlowNodeState::Modified))
            .map(|node| (node.id.clone(), node))
            .collect();

        let mut calls: HashMap<&str, Vec<&Call>> = HashMap::new();
        for call in &new.calls {
            calls.entry(call.caller.as_str()).or_default().push(call);
        }
        for list in calls.values_mut() {
            list.sort_by(|left, right| {
                left.line
                    .cmp(&right.line)
                    .then_with(|| left.callee.cmp(&right.callee))
            });
        }

        let mut changed_containers = HashSet::new();
        for (analysis, paths) in [
            (new, lines.added_paths().collect::<HashSet<_>>()),
            (old, lines.removed_paths().collect::<HashSet<_>>()),
        ] {
            for function in &analysis.functions {
                if paths.contains(function.file.as_str()) {
                    changed_containers.insert(function.container.as_str());
                }
            }
        }
        Self {
            changed,
            calls,
            changed_containers,
            files: new.functions.iter().map(|function| function.file.as_str()).collect(),
        }
    }

    /// State of the function `id` in the review, test or not.
    pub fn state(&self, id: &str) -> TestFunctionState {
        match self.changed.get(id).map(|node| node.state) {
            Some(FlowNodeState::New) => TestFunctionState::New,
            Some(FlowNodeState::Modified) => TestFunctionState::Modified,
            _ => TestFunctionState::Untouched,
        }
    }

    /// New and modified production functions, in no particular order.
    pub fn production(&self) -> impl Iterator<Item = &FlowNode> {
        self.changed.values().filter(|node| !node.test)
    }

    /// The first new or modified production function reached from `start`.
    pub fn exercises(&self, start: &str) -> Option<FunctionRef> {
        let mut seen: HashSet<&str> = HashSet::from([start]);
        let mut queue: VecDeque<&str> = VecDeque::from([start]);
        while let Some(caller) = queue.pop_front() {
            for call in self.calls.get(caller).map_or(&[][..], Vec::as_slice) {
                let callee = call.callee.as_str();
                if !seen.insert(callee) {
                    continue;
                }
                if let Some(node) = self.changed.get(callee).filter(|node| !node.test) {
                    return Some(function_ref(node));
                }
                queue.push_back(callee);
            }
        }
        None
    }

    pub fn in_changed_package(&self, container: &str) -> bool {
        self.changed_containers.contains(container)
    }
}

pub fn function_ref(node: &FlowNode) -> FunctionRef {
    FunctionRef {
        id: node.id.clone(),
        qualified_name: node.qualified_name.clone(),
    }
}

/// Which lines of which files hold statements, and which of them ran.
pub struct CoverageIndex {
    /// File, then line, then whether a block spanning it ran.
    lines: HashMap<String, HashMap<u32, bool>>,
}

impl CoverageIndex {
    pub fn new(blocks: &[CoverageBlock]) -> Self {
        let mut lines: HashMap<String, HashMap<u32, bool>> = HashMap::new();
        for block in blocks {
            let file = lines.entry(block.file.clone()).or_default();
            for line in block.start_line..=block.end_line {
                *file.entry(line).or_insert(false) |= block.count > 0;
            }
        }
        Self { lines }
    }

    /// Over every coverable line.
    pub fn project(&self) -> LineCoverage {
        let mut total = LineCoverage::default();
        for file in self.lines.values() {
            total.coverable = total.coverable.saturating_add(count(file.len()));
            total.covered = total
                .covered
                .saturating_add(count(file.values().filter(|ran| **ran).count()));
        }
        total
    }

    /// Over the coverable ones among `lines` of `file`.
    pub fn over(&self, file: &str, lines: impl IntoIterator<Item = u32>) -> LineCoverage {
        let mut total = LineCoverage::default();
        let Some(known) = self.lines.get(file) else {
            return total;
        };
        for line in lines {
            if let Some(ran) = known.get(&line) {
                total.coverable += 1;
                total.covered += u32::from(*ran);
            }
        }
        total
    }

    /// Over the coverable lines of `file` from `start` to `end`.
    pub fn within(&self, file: &str, start: u32, end: u32) -> LineCoverage {
        self.over(file, start..=end)
    }

    /// Changed lines of the review, overall and per file with coverable lines.
    pub fn changed(&self, lines: &ChangedLines) -> (LineCoverage, Vec<FileCoverage>) {
        let mut paths: Vec<&str> = lines.added_paths().collect();
        paths.sort_unstable();
        let mut total = LineCoverage::default();
        let mut files = Vec::new();
        for path in paths {
            let coverage = self.over(path, lines.added_lines(path));
            if coverage.coverable == 0 {
                continue;
            }
            total.covered += coverage.covered;
            total.coverable += coverage.coverable;
            files.push(FileCoverage {
                path: path.to_string(),
                covered: coverage.covered,
                coverable: coverage.coverable,
            });
        }
        (total, files)
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Everything a `TestRun` is derived from.
pub struct RunInput<'a> {
    pub language: &'a str,
    /// Used when the runner did not report (a failed, stopped run).
    pub runner: AnalyzerIdentity,
    pub old_sha: &'a str,
    pub new_sha: Option<&'a str>,
    pub record: &'a RunRecord,
    /// The parsed `record.output`, for a completed run.
    pub output: Option<&'a TestOutput>,
    /// `None` when the sides could not be analysed: every test is then
    /// untouched, exercises nothing and is outside the changed packages.
    pub facts: Option<&'a ReviewFacts<'a>>,
    pub lines: &'a ChangedLines,
    /// Added after the runner's own.
    pub diagnostics: Vec<CoreText>,
}

pub fn derive(input: RunInput<'_>) -> TestRun {
    let record = input.record;
    let mut run = TestRun {
        language: input.language.to_string(),
        runner: input.runner,
        old_sha: input.old_sha.to_string(),
        new_sha: input.new_sha.map(str::to_string),
        isolation: record.isolation,
        started_at: record.started_at.clone(),
        duration_ms: record.duration_ms,
        status: record.status,
        message: record.message.clone().map(|mut message| {
            if message.reason.is_none() {
                message.message = keep_end(&message.message, MAX_OUTPUT_BYTES);
            }
            message
        }),
        tests: Vec::new(),
        summary: TestSummary::default(),
        packages: Vec::new(),
        coverage: None,
        race_detector: false,
        races: Vec::new(),
        diagnostics: Vec::new(),
        options: record.options,
        partial: record.partial.clone(),
    };
    if let Some(output) = input.output {
        let locator = locator(output, input.facts);
        run.runner = output.runner.clone();
        run.tests = tests(output, input.facts, &locator, input.language);
        run.summary = summary(&run.tests);
        run.packages = output
            .packages
            .iter()
            .map(|package| {
                let output = package
                    .output
                    .as_deref()
                    .map(|text| keep_end(text, MAX_OUTPUT_BYTES));
                let failure = output
                    .as_deref()
                    .filter(|_| {
                        matches!(package.outcome, PackageOutcomeKind::BuildFailed | PackageOutcomeKind::Fail)
                    })
                    .map(|text| test_failures::package_failure(text, package.outcome, &locator));
                PackageOutcome {
                    container: package.container.clone(),
                    outcome: package.outcome,
                    duration_ms: package.duration_ms,
                    output,
                    failure,
                    command: package.command.clone(),
                }
            })
            .collect();
        run.coverage = output.coverage.as_ref().map(|coverage| {
            let index = CoverageIndex::new(&coverage.blocks);
            let (changed_lines, files) = index.changed(input.lines);
            RunCoverage {
                changed_lines,
                files,
                project: index.project(),
            }
        });
        run.race_detector = output.race;
        run.races = output
            .races
            .iter()
            .map(|race| RaceReport {
                container: race.container.clone(),
                test: race.test.clone(),
                output: keep_end(&race.output, MAX_RACE_BYTES),
            })
            .collect();
        run.diagnostics = output
            .diagnostics
            .iter()
            .map(|diagnostic| CoreText::raw(diagnostic.as_str()))
            .collect();
    }
    run.diagnostics.extend(input.diagnostics);
    run
}

/// The project's files a run knows of: the new side's functions, the tests'
/// own files and the files coverage measured.
fn locator(output: &TestOutput, facts: Option<&ReviewFacts<'_>>) -> Locator {
    let mut files: HashSet<String> = facts
        .map(|facts| facts.files.iter().map(|file| file.to_string()).collect())
        .unwrap_or_default();
    files.extend(output.tests.iter().filter_map(|test| test.file.clone()));
    if let Some(coverage) = &output.coverage {
        files.extend(coverage.blocks.iter().map(|block| block.file.clone()));
    }
    Locator::new(files)
}

/// Whether a race report belongs to `test` or one of its subtests.
fn race_of(race: &crate::runner_protocol::RunnerRace, test: &RunnerTest) -> bool {
    race.container == test.container
        && race.test.as_deref().is_some_and(|name| {
            name == test.name
                || name
                    .strip_prefix(test.name.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

fn tests(
    output: &TestOutput,
    facts: Option<&ReviewFacts<'_>>,
    locator: &Locator,
    language: &str,
) -> Vec<TestCase> {
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut tests: Vec<TestCase> = output
        .tests
        .iter()
        .map(|test| {
            let base = format!("{}::{}", test.container, test.name);
            let uses = ids.entry(base.clone()).or_insert(0);
            *uses += 1;
            let id = if *uses == 1 {
                base
            } else {
                format!("{base}#{uses}")
            };
            let function = test.function.as_deref();
            let state = match (facts, function) {
                (Some(facts), Some(function)) => facts.state(function),
                _ => TestFunctionState::Untouched,
            };
            let exercises = facts
                .zip(function)
                .and_then(|(facts, function)| facts.exercises(function));
            let output_text = match test.outcome {
                TestOutcome::Fail => test
                    .output
                    .as_deref()
                    .map(|text| keep_end(text, MAX_OUTPUT_BYTES)),
                TestOutcome::Pass | TestOutcome::Skip => None,
            };
            let failure = (test.outcome == TestOutcome::Fail).then(|| {
                let races: Vec<&str> = output
                    .races
                    .iter()
                    .filter(|race| race_of(race, test))
                    .map(|race| race.output.as_str())
                    .collect();
                test_failures::test_failure(
                    output_text.as_deref().unwrap_or(""),
                    &races,
                    test.file.as_deref(),
                    locator,
                )
            });
            let levels = test.levels();
            TestCase {
                id,
                container: test.container.clone(),
                package: test.package().to_string(),
                name: test.name.clone(),
                hierarchy: levels.iter().map(|level| level.to_string()).collect(),
                title: title(&levels, language),
                command: test.command.clone(),
                function_id: test.function.clone(),
                state,
                outcome: test.outcome,
                duration_ms: test.duration_ms,
                output: output_text,
                file: test.file.clone(),
                line: test.line,
                exercises,
                in_changed_package: facts
                    .is_some_and(|facts| facts.in_changed_package(&test.container)),
                failure,
            }
        })
        .collect();
    tests.sort_by(|left, right| {
        (left.outcome != TestOutcome::Fail)
            .cmp(&(right.outcome != TestOutcome::Fail))
            .then_with(|| left.container.cmp(&right.container))
            .then_with(|| left.name.cmp(&right.name))
    });
    tests
}

fn summary(tests: &[TestCase]) -> TestSummary {
    let mut summary = TestSummary::default();
    for test in tests {
        match test.outcome {
            TestOutcome::Pass => summary.passed += 1,
            TestOutcome::Fail => summary.failed += 1,
            TestOutcome::Skip => summary.skipped += 1,
        }
        if test.state == TestFunctionState::New {
            summary.added += 1;
        }
    }
    summary
}

/// Whether `selection` ran the test whose name nests as `levels` (of the
/// package `package`): its whole package, its whole top-level test, the
/// parent of the subtests it ran, or one of those subtests or theirs.
/// Selected subtests are named relative to their test, levels joined by "/".
pub fn selected(selection: &RunSelection, package: &str, levels: &[&str]) -> bool {
    let Some(selected) = selection.packages.iter().find(|p| p.package == package) else {
        return false;
    };
    let Some(tests) = &selected.tests else {
        return true;
    };
    let Some((top, rest)) = levels.split_first() else {
        return false;
    };
    tests.iter().filter(|test| test.name == *top).any(|test| {
        match (&test.subtests, rest.first()) {
            (None, _) | (Some(_), None) => true,
            (Some(subtests), Some(first)) => subtests.iter().any(|sub| {
                sub == first || sub.strip_prefix(*first).is_some_and(|deeper| deeper.starts_with('/'))
            }),
        }
    })
}

fn selects_whole(selection: &RunSelection, package: &str) -> bool {
    selection
        .packages
        .iter()
        .any(|p| p.package == package && p.tests.is_none())
}

/// The last complete run `base` with the result of a partial run of the
/// same new side over it: the tests the selection ran (and any the partial
/// run reported) replaced, every other test as it was. A package run whole
/// takes the partial run's outcome; one run in part is worked out again
/// from its tests. Races go with their tests. Coverage is the union of both
/// runs' blocks when they ran the same code (`same_code`: the same commit,
/// or the same working tree), since the tests that did not run again still
/// cover what they covered; otherwise, or when the partial run did not
/// measure it, the base's is kept. A partial run never brings coverage to a
/// base without it, which would read as the coverage of the whole suite.
pub fn merge(base: &TestOutput, partial: &TestOutput, selection: &RunSelection, same_code: bool) -> TestOutput {
    let reported: HashSet<(&str, &str)> = partial
        .tests
        .iter()
        .map(|test| (test.package(), test.name.as_str()))
        .collect();
    let replaced = |test: &RunnerTest| {
        selected(selection, test.package(), &test.levels()) || reported.contains(&(test.package(), test.name.as_str()))
    };
    let removed: HashSet<(&str, &str)> = base
        .tests
        .iter()
        .filter(|test| replaced(test))
        .map(|test| (test.container.as_str(), test.name.as_str()))
        .collect();
    let mut tests: Vec<RunnerTest> = base.tests.iter().filter(|test| !replaced(test)).cloned().collect();
    tests.extend(partial.tests.iter().cloned());

    let mut packages: Vec<RunnerPackage> = Vec::new();
    for package in &base.packages {
        match partial.packages.iter().find(|p| p.container == package.container) {
            Some(ran) => packages.push(merged_package(package, ran, selection, &tests)),
            None => packages.push(package.clone()),
        }
    }
    for ran in &partial.packages {
        if !base.packages.iter().any(|p| p.container == ran.container) {
            packages.push(ran.clone());
        }
    }

    let mut races: Vec<_> = base
        .races
        .iter()
        .filter(|race| match &race.test {
            Some(name) => !removed.contains(&(race.container.as_str(), name.as_str())),
            None => !selects_whole(selection, &race.container),
        })
        .cloned()
        .collect();
    races.extend(partial.races.iter().cloned());

    let coverage = match (&base.coverage, &partial.coverage) {
        (Some(base), Some(ran)) if same_code => Some(union(base, ran)),
        (base, _) => base.clone(),
    };
    TestOutput {
        protocol: base.protocol,
        language: base.language.clone(),
        runner: partial.runner.clone(),
        race: base.race,
        tests,
        packages,
        coverage,
        races,
        diagnostics: base.diagnostics.clone(),
    }
}

fn merged_package(
    base: &RunnerPackage,
    ran: &RunnerPackage,
    selection: &RunSelection,
    tests: &[RunnerTest],
) -> RunnerPackage {
    if selects_whole(selection, &base.container) || ran.outcome == PackageOutcomeKind::BuildFailed {
        return ran.clone();
    }
    let own: Vec<&RunnerTest> = tests.iter().filter(|test| test.package() == base.container).collect();
    let (outcome, output) = if ran.outcome == PackageOutcomeKind::Fail && ran.output.is_some() {
        (PackageOutcomeKind::Fail, ran.output.clone())
    } else if own.iter().any(|test| test.outcome == TestOutcome::Fail) {
        (PackageOutcomeKind::Fail, None)
    } else if own.is_empty() {
        (PackageOutcomeKind::NoTests, None)
    } else {
        (PackageOutcomeKind::Pass, None)
    };
    RunnerPackage {
        container: base.container.clone(),
        outcome,
        // The time of the package as a whole, which only a whole run says.
        duration_ms: base.duration_ms,
        output,
        command: ran.command.clone().or_else(|| base.command.clone()),
    }
}

/// Blocks of both, a block run by either counted as run.
fn union(base: &Coverage, ran: &Coverage) -> Coverage {
    let mut blocks = base.blocks.clone();
    let index: HashMap<(String, u32, u32), usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| ((block.file.clone(), block.start_line, block.end_line), i))
        .collect();
    for block in &ran.blocks {
        match index.get(&(block.file.clone(), block.start_line, block.end_line)) {
            Some(&i) => blocks[i].count = blocks[i].count.max(block.count),
            None => blocks.push(block.clone()),
        }
    }
    Coverage { blocks }
}

/// What a partial run covered, for the header of the Tests tab.
pub fn partial_run(record: &RunRecord, selection: &RunSelection, output: Option<&TestOutput>) -> PartialRun {
    PartialRun {
        started_at: record.started_at.clone(),
        duration_ms: record.duration_ms,
        status: record.status,
        isolation: record.isolation,
        packages: u32::try_from(selection.packages.len()).unwrap_or(u32::MAX),
        tests: output.map_or(0, |output| {
            count(output.tests.iter().filter(|test| test.is_top_level()).count())
        }),
    }
}

/// The last level of a subtest's name, with "_" read as spaces for Go (whose
/// subtest names have spaces turned into "_"); a top-level test's whole
/// name.
pub fn title(levels: &[&str], language: &str) -> String {
    match levels {
        [] => String::new(),
        [only] => only.to_string(),
        [.., last] if language == "go" => last.replace('_', " "),
        [.., last] => last.to_string(),
    }
}

/// At most `max` bytes of `text`, its end, cut on a character boundary.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{analysis, call, function, in_test, reference};
    use crate::model::PackageOutcomeKind;
    use crate::runner_protocol::fixtures::{block, failing, identity, output, test};

    fn record(status: TestRunStatus) -> RunRecord {
        RunRecord {
            isolation: Isolation::Sandbox,
            started_at: "2026-11-17T10:00:00Z".to_string(),
            duration_ms: 1500,
            status,
            message: None,
            output: None,
            options: RunOptions::default(),
            partial: None,
        }
    }

    /// Old side: p.A (a.go 1-10), test p.TestA. New side: p.A modified at
    /// line 3, p.B new (b.go), p.TestA unchanged, p.TestB new (b_test.go)
    /// calling a helper that calls p.A then p.B; p.Gen generated and new.
    fn sides() -> (Analysis, Analysis, ChangedLines) {
        let old = analysis(
            vec![
                function("p.A", "a.go", 1, 10),
                in_test(function("p.TestA", "a_test.go", 1, 5)),
            ],
            vec![call("p.TestA", "p.A", 2)],
        );
        let mut generated = function("p.Gen", "gen.go", 1, 3);
        generated.generated = true;
        let new = analysis(
            vec![
                function("p.A", "a.go", 1, 10),
                function("p.B", "b.go", 1, 5),
                generated,
                in_test(function("p.TestA", "a_test.go", 1, 5)),
                in_test(function("p.TestB", "b_test.go", 1, 9)),
                in_test(function("p.helper", "b_test.go", 11, 15)),
                in_test(function("p.TestGen", "gen_test.go", 1, 3)),
            ],
            vec![
                call("p.TestA", "p.A", 2),
                call("p.TestB", "p.helper", 2),
                call("p.TestB", "p.Gen", 3),
                call("p.helper", "p.B", 14),
                call("p.helper", "p.A", 12),
                reference("p.TestGen", "p.Gen", 2),
            ],
        );
        let lines = ChangedLines::default()
            .with_added("a.go", 3, 3)
            .with_removed("a.go", 3, 3)
            .with_added("b.go", 1, 5)
            .with_added("gen.go", 1, 3)
            .with_added("b_test.go", 1, 15)
            .with_added("gen_test.go", 1, 3);
        (old, new, lines)
    }

    fn input<'a>(
        record: &'a RunRecord,
        output: Option<&'a TestOutput>,
        facts: Option<&'a ReviewFacts<'a>>,
        lines: &'a ChangedLines,
    ) -> RunInput<'a> {
        RunInput {
            language: "go",
            runner: AnalyzerIdentity {
                name: "fallback".to_string(),
                version: "0".to_string(),
            },
            old_sha: "old",
            new_sha: Some("new"),
            record,
            output,
            facts,
            lines,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn states_exercises_and_packages_come_from_the_review() {
        let (old, new, lines) = sides();
        let facts = ReviewFacts::new(&old, &new, &lines);
        let mut other = test("TestOther", Some("q.TestOther"));
        other.container = "q".to_string();
        let output = output(
            vec![
                test("TestA", Some("p.TestA")),
                test("TestB/gateway_declines", Some("p.TestB")),
                test("TestB", Some("p.TestB")),
                failing(test("TestGen", Some("p.TestGen")), "boom"),
                test("TestUnknown", None),
                other,
            ],
            None,
        );
        let record = record(TestRunStatus::Completed);

        let run = derive(input(&record, Some(&output), Some(&facts), &lines));

        let rows: Vec<_> = run
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.state,
                    t.exercises.as_ref().map(|f| f.id.as_str()),
                    t.in_changed_package,
                )
            })
            .collect();
        use TestFunctionState::{Modified, New, Untouched};
        assert_eq!(
            rows,
            vec![
                // Failed first; the generated function is not production.
                ("TestGen", New, None, true),
                ("TestA", Untouched, Some("p.A"), true),
                // Breadth first through the helper, its calls by line.
                ("TestB", New, Some("p.A"), true),
                ("TestB/gateway_declines", New, Some("p.A"), true),
                ("TestUnknown", Untouched, None, true),
                ("TestOther", Untouched, None, false),
            ]
        );
        assert_eq!(run.tests[0].output.as_deref(), Some("boom"));
        assert_eq!(run.tests[0].id, "pkg::TestGen");
        assert_eq!(run.tests[3].title, "gateway declines");
        assert_eq!(run.tests[2].title, "TestB");
        assert_eq!(
            run.summary,
            TestSummary {
                passed: 5,
                failed: 1,
                skipped: 0,
                added: 3
            }
        );
        assert_eq!(run.runner, identity());
        assert!(run.race_detector);
        assert_eq!(run.packages[0].outcome, PackageOutcomeKind::Pass);
        assert_eq!(run.status, TestRunStatus::Completed);
        assert!(run.coverage.is_none());
        let modified = facts.state("p.A");
        assert_eq!(modified, Modified);
    }

    #[test]
    fn coverage_counts_changed_lines_per_file_and_the_project() {
        let (_, _, lines) = sides();
        let lines = lines.with_added("untouched-by-tests.go", 1, 2);
        let output = output(
            Vec::new(),
            Some(vec![
                // a.go: line 3 changed and covered; 4-6 not changed.
                block("a.go", 2, 4, 1),
                block("a.go", 5, 6, 0),
                // b.go: 1-5 added; 2-3 covered, 4 coverable but not run,
                // 3 also spanned by a block that did not run.
                block("b.go", 2, 3, 2),
                block("b.go", 3, 4, 0),
                // Not changed by the review.
                block("c.go", 1, 10, 1),
            ]),
        );
        let record = record(TestRunStatus::Completed);

        let run = derive(input(&record, Some(&output), None, &lines));

        let coverage = run.coverage.unwrap();
        assert_eq!(
            coverage.changed_lines,
            LineCoverage {
                covered: 3,
                coverable: 4
            }
        );
        assert_eq!(
            coverage.files,
            vec![
                FileCoverage {
                    path: "a.go".to_string(),
                    covered: 1,
                    coverable: 1
                },
                FileCoverage {
                    path: "b.go".to_string(),
                    covered: 2,
                    coverable: 3
                },
            ]
        );
        assert_eq!(
            coverage.project,
            LineCoverage {
                covered: 15,
                coverable: 18
            }
        );
        assert!(run.tests.is_empty());
    }

    #[test]
    fn a_run_that_did_not_report_has_only_its_ending() {
        let (_, _, lines) = sides();
        let mut failed = record(TestRunStatus::Failed);
        failed.message = Some(format!("{}the end", "x".repeat(MAX_OUTPUT_BYTES)).into());
        let mut input = input(&failed, None, None, &lines);
        input.diagnostics = vec!["new: the analysis failed".into()];

        let run = derive(input);

        assert_eq!(run.status, TestRunStatus::Failed);
        assert_eq!(run.runner.name, "fallback");
        let message = run.message.unwrap();
        assert_eq!(message.len(), MAX_OUTPUT_BYTES);
        assert!(message.ends_with("the end"));
        assert!(run.tests.is_empty() && run.packages.is_empty() && run.coverage.is_none());
        assert!(!run.race_detector);
        assert_eq!(run.diagnostics, vec!["new: the analysis failed"]);
        assert_eq!(
            (run.isolation, run.duration_ms, run.started_at.as_str()),
            (Isolation::Sandbox, 1500, "2026-11-17T10:00:00Z")
        );
    }

    /// A test to select: its name and, for only some, its subtests.
    type Pick<'a> = (&'a str, Option<Vec<&'a str>>);

    fn selection(packages: Vec<(&str, Option<Vec<Pick<'_>>>)>) -> RunSelection {
        use crate::model::{SelectedPackage, SelectedTest};
        RunSelection {
            packages: packages
                .into_iter()
                .map(|(package, tests)| SelectedPackage {
                    package: package.to_string(),
                    tests: tests.map(|tests| {
                        tests
                            .into_iter()
                            .map(|(name, subtests)| SelectedTest {
                                name: name.to_string(),
                                subtests: subtests.map(|subs| subs.into_iter().map(str::to_string).collect()),
                            })
                            .collect()
                    }),
                })
                .collect(),
        }
    }

    fn in_package(mut test: RunnerTest, package: &str) -> RunnerTest {
        test.container = package.to_string();
        test.package = Some(package.to_string());
        test
    }

    #[test]
    fn a_selection_says_which_tests_it_ran() {
        let only_some = selection(vec![("pkg", Some(vec![("TestA", Some(vec!["x", "y/deep"])), ("TestB", None)]))]);
        for (name, ran) in [
            ("TestA", true),
            ("TestA/x", true),
            ("TestA/x/inner", true),
            ("TestA/y", true),
            ("TestA/z", false),
            ("TestB", true),
            ("TestB/any", true),
            ("TestC", false),
            ("TestAB", false),
        ] {
            let levels: Vec<&str> = name.split('/').collect();
            assert_eq!(selected(&only_some, "pkg", &levels), ran, "{name}");
        }
        assert!(!selected(&only_some, "other", &["TestA"]));
        let whole = selection(vec![("pkg", None)]);
        assert!(selected(&whole, "pkg", &["TestAnything", "sub"]));
        // A "/" inside a level is not a subtest: a Rust doc test or a pytest
        // case selected by its whole name.
        let doc = selection(vec![("pkg", Some(vec![("src/lib.rs - add (line 5)", None)]))]);
        assert!(selected(&doc, "pkg", &["src/lib.rs - add (line 5)"]));
        assert!(!selected(&doc, "pkg", &["src/lib.rs - other (line 9)"]));
    }

    #[test]
    fn merging_replaces_only_what_ran_and_works_packages_out_again() {
        let mut base = output(
            vec![
                failing(test("TestA", None), "boom"),
                failing(test("TestA/x", None), "boom"),
                test("TestA/y", None),
                test("TestB", None),
                in_package(test("TestQ", None), "q"),
                in_package(test("TestR", None), "r"),
            ],
            Some(vec![block("a.go", 1, 2, 1), block("b.go", 1, 1, 0)]),
        );
        base.packages = vec![
            RunnerPackage { container: "pkg".to_string(), outcome: PackageOutcomeKind::Fail, duration_ms: 50, output: None, command: None },
            RunnerPackage { container: "q".to_string(), outcome: PackageOutcomeKind::Pass, duration_ms: 9, output: None, command: None },
            RunnerPackage {
                container: "broken".to_string(),
                outcome: PackageOutcomeKind::BuildFailed,
                duration_ms: 0,
                output: Some("x.go:1:1: oops".to_string()),
                command: Some("go test ./broken".to_string()),
            },
            RunnerPackage { container: "r".to_string(), outcome: PackageOutcomeKind::Pass, duration_ms: 4, output: None, command: None },
        ];
        base.races = vec![
            crate::runner_protocol::RunnerRace { container: "pkg".to_string(), test: Some("TestA/x".to_string()), output: "old".to_string() },
            crate::runner_protocol::RunnerRace { container: "q".to_string(), test: None, output: "q race".to_string() },
            crate::runner_protocol::RunnerRace { container: "r".to_string(), test: None, output: "r race".to_string() },
        ];
        let mut partial = output(
            vec![
                test("TestA/x", None),
                test("TestA", None),
                in_package(test("TestQ2", None), "q"),
                in_package(test("TestFixed", None), "broken"),
            ],
            Some(vec![block("b.go", 1, 1, 2), block("c.go", 1, 1, 1)]),
        );
        partial.packages = vec![
            RunnerPackage { container: "pkg".to_string(), outcome: PackageOutcomeKind::Pass, duration_ms: 3, output: None, command: Some("go test ./pkg".to_string()) },
            RunnerPackage { container: "q".to_string(), outcome: PackageOutcomeKind::Pass, duration_ms: 12, output: None, command: None },
            RunnerPackage { container: "broken".to_string(), outcome: PackageOutcomeKind::Pass, duration_ms: 2, output: None, command: None },
        ];
        // TestA without TestA/y, the whole of q and of broken.
        let ran = selection(vec![
            ("pkg", Some(vec![("TestA", Some(vec!["x"]))])),
            ("q", None),
            ("broken", None),
        ]);

        let merged = merge(&base, &partial, &ran, true);

        let mut names: Vec<_> = merged
            .tests
            .iter()
            .map(|t| format!("{}:{}:{:?}", t.package(), t.name, t.outcome))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "broken:TestFixed:Pass",
                "pkg:TestA/x:Pass",
                "pkg:TestA/y:Pass",
                "pkg:TestA:Pass",
                "pkg:TestB:Pass",
                "q:TestQ2:Pass",
                "r:TestR:Pass",
            ],
            "TestQ is gone with its package's whole run; r was not run"
        );
        let packages: Vec<_> = merged
            .packages
            .iter()
            .map(|p| (p.container.as_str(), p.outcome, p.duration_ms, p.output.is_some()))
            .collect();
        assert_eq!(
            packages,
            vec![
                // Run in part: worked out from its tests, its whole time kept.
                ("pkg", PackageOutcomeKind::Pass, 50, false),
                ("q", PackageOutcomeKind::Pass, 12, false),
                ("broken", PackageOutcomeKind::Pass, 2, false),
                ("r", PackageOutcomeKind::Pass, 4, false),
            ]
        );
        let commands: Vec<_> = merged.packages.iter().map(|p| p.command.as_deref()).collect();
        assert_eq!(
            commands,
            vec![Some("go test ./pkg"), None, None, None],
            "the command of the run that ran it last"
        );
        let races: Vec<_> = merged.races.iter().map(|r| r.output.as_str()).collect();
        assert_eq!(races, vec!["r race"]);
        let blocks: Vec<_> = merged
            .coverage
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .map(|b| (b.file.as_str(), b.count))
            .collect();
        assert_eq!(blocks, vec![("a.go", 1), ("b.go", 2), ("c.go", 1)]);
        assert_eq!(merged.diagnostics, base.diagnostics);

        // Other code, or no coverage measured now: the base's is kept.
        assert_eq!(merge(&base, &partial, &ran, false).coverage, base.coverage);
        let mut unmeasured = partial.clone();
        unmeasured.coverage = None;
        assert_eq!(merge(&base, &unmeasured, &ran, true).coverage, base.coverage);
        // A base without coverage never gets the partial run's.
        let mut uncovered = base.clone();
        uncovered.coverage = None;
        assert_eq!(merge(&uncovered, &partial, &ran, true).coverage, None);

        // A failed test still there keeps its package failed; a package that
        // failed outside its tests says so.
        let mut failing_again = partial.clone();
        failing_again.tests[1].outcome = TestOutcome::Fail;
        let merged = merge(&base, &failing_again, &ran, true);
        assert_eq!(merged.packages[0].outcome, PackageOutcomeKind::Fail);
        let mut setup = partial.clone();
        setup.packages[0].outcome = PackageOutcomeKind::Fail;
        setup.packages[0].output = Some("panic: setup".to_string());
        let merged = merge(&base, &setup, &ran, true);
        assert_eq!(merged.packages[0].output.as_deref(), Some("panic: setup"));
    }

    #[test]
    fn failed_tests_and_packages_say_why() {
        let mut output = output(
            vec![
                failing(test("TestA", None), "=== RUN   TestA\n    a_test.go:4: wanted 2\n--- FAIL: TestA (0.00s)\n"),
                test("TestB", None),
            ],
            None,
        );
        output.packages.push(RunnerPackage {
            container: "broken".to_string(),
            outcome: PackageOutcomeKind::BuildFailed,
            duration_ms: 0,
            output: Some("broken/x.go:3:7: undefined: y\n".to_string()),
            command: Some("go test ./broken".to_string()),
        });
        let record = record(TestRunStatus::Completed);
        let lines = ChangedLines::default();

        let run = derive(input(&record, Some(&output), None, &lines));

        let a = run.tests.iter().find(|t| t.name == "TestA").unwrap();
        let json = serde_json::to_value(a.failure.as_ref().unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "assertion", "message": "wanted 2", "expected": null, "actual": null,
                "at": { "file": "a_test.go", "line": 4 }, "test": "TestA"
            })
        );
        assert_eq!(a.package, "pkg");
        assert_eq!(run.tests.iter().find(|t| t.name == "TestB").unwrap().failure, None);
        assert_eq!(run.packages[0].failure, None);
        let build = serde_json::to_value(run.packages[1].failure.as_ref().unwrap()).unwrap();
        assert_eq!(
            build,
            serde_json::json!({ "kind": "build", "errors": [{
                "file": "broken/x.go", "line": 3, "column": 7, "message": "undefined: y", "detail": []
            }] })
        );
        assert_eq!(run.packages[1].command.as_deref(), Some("go test ./broken"));
        assert_eq!((run.options, run.partial), (RunOptions::default(), None));
    }

    #[test]
    fn tests_nest_as_the_runner_says_and_carry_its_commands() {
        let mut doc = test("src/lib.rs - add (line 5)", None);
        doc.hierarchy = Some(vec!["src/lib.rs - add (line 5)".to_string()]);
        doc.command = Some("cargo test -p shapes --doc -- 'add'".to_string());
        let mut nested = test("math/divide/refuses_zero", None);
        nested.hierarchy = Some(vec!["math".to_string(), "divide".to_string(), "refuses_zero".to_string()]);
        let output = output(vec![doc, nested, test("TestA/sub_case", None)], None);
        let record = record(TestRunStatus::Completed);
        let lines = ChangedLines::default();

        let mut run_input = input(&record, Some(&output), None, &lines);
        run_input.language = "typescript";
        let run = derive(run_input);

        let rows: Vec<_> = run
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.hierarchy.len(), t.title.as_str(), t.command.as_deref()))
            .collect();
        assert_eq!(
            rows,
            vec![
                // Kept from before hierarchies: split at "/".
                ("TestA/sub_case", 2, "sub_case", None),
                ("math/divide/refuses_zero", 3, "refuses_zero", None),
                ("src/lib.rs - add (line 5)", 1, "src/lib.rs - add (line 5)", Some("cargo test -p shapes --doc -- 'add'")),
            ]
        );
        let selection = selection(vec![("pkg", Some(vec![("src/lib.rs - add (line 5)", None)]))]);
        let partial = partial_run(&record, &selection, Some(&output));
        assert_eq!(partial.tests, 1, "only the doc test is top-level");
    }

    #[test]
    fn titles_ids_and_cut_output() {
        assert_eq!(title(&["TestService_Request"], "go"), "TestService_Request");
        assert_eq!(title(&["TestX", "a_b", "c_d_e"], "go"), "c d e");
        // Only Go turns spaces into "_"; a title elsewhere is as written.
        assert_eq!(title(&["math", "handles snake_case"], "typescript"), "handles snake_case");
        assert_eq!(title(&["src/lib.rs - add (line 5)"], "rust"), "src/lib.rs - add (line 5)");
        assert_eq!(keep_end("héllo", 4), "llo");
        assert_eq!(keep_end("short", 10), "short");

        let output = output(vec![test("TestA", None), test("TestA", None)], None);
        let record = record(TestRunStatus::Completed);
        let lines = ChangedLines::default();
        let run = derive(input(&record, Some(&output), None, &lines));
        let ids: Vec<_> = run.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["pkg::TestA", "pkg::TestA#2"]);
    }
}
