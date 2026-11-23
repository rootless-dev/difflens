//! The call graph of a review: runs the analyzer of each of the project's
//! languages on both sides and compares the results (see `flow_graph`).
//!
//! The analysis of a commit is cached on disk as JSON under
//! `<cache>/analyses/<project id>/<analyzer id>/<analyzer fingerprint>/<sha>.json`,
//! so the merge base shared by many branches is analysed once, and an
//! analyzer whose code changed starts over (the fingerprint is a hash of the
//! executable, or of the shipped source of a provisioned analyzer, see
//! `analyzers`, so an identical rebuild, as `tauri dev` makes on every start,
//! keeps the cache; a cache file
//! that no longer parses, such as one written before a protocol field became
//! required, counts as missing). An analyzer shipped as source gets its
//! environment the first time a review needs it (`analyzer_envs`), which the
//! request waits for. The working tree is never cached on disk:
//! its last analysis per project and analyzer is kept in memory with a stamp
//! of the files it was made from (`trees::working_tree_stamp`), and reused
//! while the stamp is the same. So a request that changes only the options of
//! the graph never runs an analyzer again.
//! Concurrent requests for the same side and analyzer share one run.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::analysis::{self, Analysis};
use crate::analyzers::{self, AnalyzerLocator, AnalyzerSpec, Launcher, RunSettings};
use crate::build_cache::{self, BuildCaches};
use crate::disk_cache;
use crate::error::{app_error, AppError, AppResult, CoreText};
use crate::flow_graph::{self, GraphRequest, LanguageSides, MAX_NODES};
use crate::git::Git;
use crate::hunks;
use crate::languages;
use crate::line_count;
use crate::model::{FlowOptions, FlowResult, FlowUnavailable, ReviewSide};
use crate::review;
use crate::single_flight::SingleFlight;
use crate::tool_path::ToolPath;
use crate::trees::{self, ScratchTree, TreeStore, KEEP_PER_PROJECT};

/// Cached analyses kept per project and language.
const KEEP_ANALYSES: usize = 50;

/// One side of a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source<'a> {
    Commit(&'a str),
    WorkingTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FlightKey {
    project: String,
    analyzer: &'static str,
    fingerprint: String,
    /// The commit, or `None` for the working tree.
    commit: Option<String>,
    /// The working tree's stamp, `None` for a commit.
    stamp: Option<u64>,
}

/// Why one side has no analysis.
#[derive(Debug, Clone)]
enum SideError {
    /// The analyzer is missing or failed; shown as `analysis_failed`.
    Failed(CoreText),
    /// git or the disk failed.
    Error(AppError),
}

impl From<AppError> for SideError {
    fn from(error: AppError) -> Self {
        Self::Error(error)
    }
}

type SideResult = Result<Arc<Analysis>, SideError>;

/// The last working tree analysis of one project and analyzer.
struct WorkingAnalysis {
    fingerprint: String,
    stamp: u64,
    analysis: Arc<Analysis>,
}

/// An analyzer ready to run.
#[derive(Debug, Clone)]
pub struct Analyzer {
    pub spec: &'static AnalyzerSpec,
    pub launcher: Launcher,
    /// What its results are cached under (see `analyzers`).
    pub fingerprint: String,
}

/// Both sides of a review, analysed by every analyzer of the project, and
/// the lines the review changed.
pub struct ReviewSides {
    /// In the order of `old` and `new`.
    pub analyzers: Vec<Analyzer>,
    pub old: Vec<Arc<Analysis>>,
    pub new: Vec<Arc<Analysis>>,
    pub lines: hunks::ChangedLines,
    /// Why the analyzers of some of the project's languages were left out:
    /// a provisioned analyzer that could not be prepared, while another
    /// language's analyzer could be used.
    pub left_out: Vec<CoreText>,
}

pub struct FlowService {
    git: Git,
    cache: PathBuf,
    locator: AnalyzerLocator,
    tool_path: ToolPath,
    settings: RunSettings,
    trees: TreeStore,
    fingerprints: analyzers::Fingerprints,
    flights: SingleFlight<FlightKey, SideResult>,
    /// By project id and analyzer id.
    working: Mutex<HashMap<(String, &'static str), WorkingAnalysis>>,
    /// The folders analyzers keep builds in between runs (`--cache-dir`).
    build_caches: BuildCaches,
}

impl FlowService {
    /// Everything is kept under `cache`. Leftovers of an earlier run (scratch
    /// copies, unfinished extractions) are deleted.
    pub fn new(
        git: Git,
        cache: PathBuf,
        locator: AnalyzerLocator,
        tool_path: ToolPath,
        settings: RunSettings,
    ) -> Self {
        let trees = TreeStore::new(cache.join("trees"), KEEP_PER_PROJECT);
        trees.remove_leftovers();
        let _ = fs::remove_dir_all(cache.join("scratch"));
        let build_caches = BuildCaches::new(cache.join("build-cache"), build_cache::MAX_BYTES);
        Self {
            git,
            cache,
            locator,
            tool_path,
            settings,
            trees,
            fingerprints: analyzers::Fingerprints::default(),
            flights: SingleFlight::new(),
            working: Mutex::new(HashMap::new()),
            build_caches,
        }
    }

    /// Keeps the analyzers' build caches under `root` rather than the cache
    /// folder (the app keeps them with its data).
    pub fn with_build_caches(mut self, root: PathBuf) -> Self {
        self.build_caches = BuildCaches::new(root, build_cache::MAX_BYTES);
        self
    }

    /// The analyzers' build caches, one folder per project and analyzer.
    pub fn build_caches(&self) -> &BuildCaches {
        &self.build_caches
    }

    /// Looks up the PATH analyzers need now rather than on the first review.
    pub fn warm_up(self: &Arc<Self>) {
        let service = Arc::clone(self);
        thread::spawn(move || {
            service.tool_path.get();
        });
    }

    /// The flow of the review from `old_sha` to `new_sha` (the working tree
    /// when `None`) of the repository `repo` registered as `project_id`,
    /// built with `options`. Fails with `unknown_branch` when a sha is not a
    /// commit of `repo`.
    pub fn flow(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
        options: FlowOptions,
    ) -> AppResult<FlowResult> {
        let sides = match self.sides(project_id, repo, old_sha, new_sha)? {
            Ok(sides) => sides,
            Err(unavailable) => return Ok(FlowResult::Unavailable(unavailable)),
        };
        let mut graph = flow_graph::build(&GraphRequest {
            languages: sides
                .old
                .iter()
                .zip(&sides.new)
                .map(|(old, new)| LanguageSides { old, new })
                .collect(),
            lines: &sides.lines,
            old_sha,
            new_sha,
            analyzed_at: now()?,
            max_nodes: MAX_NODES,
            options,
        });
        graph.diagnostics.extend(sides.left_out.iter().cloned());
        Ok(FlowResult::Graph {
            graph: Box::new(graph),
        })
    }

    /// The analyses of both sides of the review and its changed lines, as
    /// `flow` compares them (same caches, same single flights), or why there
    /// are none. Fails with `unknown_branch` when a sha is not a commit of
    /// `repo`.
    pub fn sides(
        &self,
        project_id: &str,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> AppResult<Result<ReviewSides, FlowUnavailable>> {
        review::check_commits(&self.git, repo, old_sha, new_sha)?;
        let detected = languages::detect(repo);
        let specs = analyzers::for_languages(&detected);
        if specs.is_empty() {
            return Ok(Err(FlowUnavailable::NoAnalyzer {
                languages: detected,
            }));
        }
        let mut found = Vec::with_capacity(specs.len());
        // A language whose analyzer could not be prepared (its toolchain is
        // missing, its build failed) is left out while another language's
        // analyzer works, as the rest of the app keeps working.
        let mut unprepared = Vec::new();
        for spec in specs {
            match self.analyzer(spec) {
                Ok(analyzer) => found.push(analyzer),
                Err(message) if spec.is_provisioned() => unprepared.push((spec, message)),
                Err(message) => return Ok(Err(failed(ReviewSide::Old, message))),
            }
        }
        if found.is_empty() {
            if let Some((spec, message)) = unprepared.into_iter().next() {
                return Ok(Err(FlowUnavailable::AnalyzerUnavailable {
                    analyzer: spec.display_name.to_string(),
                    message,
                }));
            }
            return Ok(Err(FlowUnavailable::NoAnalyzer { languages: detected }));
        }
        let left_out = unprepared.into_iter().map(|(_, message)| message).collect();

        let new_source = new_sha.map_or(Source::WorkingTree, Source::Commit);
        let (old, new) = thread::scope(|scope| {
            let old = scope.spawn(|| self.side(project_id, repo, Source::Commit(old_sha), &found));
            let new = self.side(project_id, repo, new_source, &found);
            let old = old.join().unwrap_or_else(|_| {
                Err(SideError::Error(app_error!(
                    Internal,
                    AnalysisAbandoned,
                    "The analysis stopped unexpectedly."
                )))
            });
            (old, new)
        });
        let old = match old {
            Ok(analyses) => analyses,
            Err(SideError::Failed(message)) => return Ok(Err(failed(ReviewSide::Old, message))),
            Err(SideError::Error(error)) => return Err(error),
        };
        let new = match new {
            Ok(analyses) => analyses,
            Err(SideError::Failed(message)) => return Ok(Err(failed(ReviewSide::New, message))),
            Err(SideError::Error(error)) => return Err(error),
        };
        let lines = self.changed_lines(repo, old_sha, new_sha)?;
        Ok(Ok(ReviewSides {
            analyzers: found,
            old,
            new,
            lines,
            left_out,
        }))
    }

    /// The analyzer of `spec` ready to run, with its fingerprint, or a
    /// message for the user saying why there is none. A provisioned analyzer
    /// without an environment gets one first (minutes, once).
    pub fn analyzer(&self, spec: &'static AnalyzerSpec) -> Result<Analyzer, CoreText> {
        self.locator
            .resolve(spec, &self.fingerprints, self.tool_path.get())
            .map(|resolved| Analyzer {
                spec,
                launcher: resolved.launcher,
                fingerprint: resolved.fingerprint,
            })
    }

    /// Where `analyzer` installs its mutation engine, when it needs one (see
    /// `AnalyzerLocator::tools_folder`).
    pub fn tools_folder(&self, analyzer: &Analyzer) -> Option<PathBuf> {
        self.locator.tools_folder(analyzer.spec, &analyzer.fingerprint)
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The PATH analyzers and runners get.
    pub fn tool_path(&self) -> &std::ffi::OsStr {
        self.tool_path.get()
    }

    pub fn settings(&self) -> &RunSettings {
        &self.settings
    }

    /// Runs `work` on a tree of the files of one side that nothing may write
    /// in: the shared extraction of `sha`, or a scratch copy of the working
    /// tree (`sha` `None`) deleted afterwards. Never the project's folder.
    pub fn with_tree<T>(
        &self,
        project_id: &str,
        repo: &Path,
        sha: Option<&str>,
        work: impl FnOnce(&Path) -> T,
    ) -> AppResult<T> {
        match sha {
            Some(sha) => {
                let tree = self.trees.commit_tree(&self.git, repo, project_id, sha)?;
                Ok(work(tree.path()))
            }
            None => {
                let scratch = ScratchTree::create(&self.cache.join("scratch"))?;
                trees::copy_working_tree(&self.git, repo, scratch.path())?;
                Ok(work(scratch.path()))
            }
        }
    }

    /// The analyses of one side, one per analyzer, in order.
    fn side(
        &self,
        project_id: &str,
        repo: &Path,
        source: Source<'_>,
        analyzers: &[Analyzer],
    ) -> Result<Vec<Arc<Analysis>>, SideError> {
        analyzers
            .iter()
            .map(|analyzer| self.analysis(project_id, repo, source, analyzer))
            .collect()
    }

    fn analysis(
        &self,
        project_id: &str,
        repo: &Path,
        source: Source<'_>,
        analyzer: &Analyzer,
    ) -> SideResult {
        let cached = match source {
            Source::Commit(sha) => Some(self.cache_file(project_id, analyzer, sha)),
            Source::WorkingTree => None,
        };
        let stamp = match source {
            Source::Commit(_) => None,
            Source::WorkingTree => Some(trees::working_tree_stamp(&self.git, repo)?),
        };
        let remembered = || match stamp {
            Some(stamp) => self.remembered_working(project_id, analyzer, stamp),
            None => cached
                .as_deref()
                .and_then(|file| read_cached(file, analyzer)),
        };
        if let Some(analysis) = remembered() {
            return Ok(analysis);
        }
        let key = FlightKey {
            project: project_id.to_string(),
            analyzer: analyzer.spec.id,
            fingerprint: analyzer.fingerprint.clone(),
            commit: match source {
                Source::Commit(sha) => Some(sha.to_string()),
                Source::WorkingTree => None,
            },
            stamp,
        };
        let abandoned = Err(SideError::Error(app_error!(
            Internal,
            AnalysisAbandoned,
            "The analysis stopped unexpectedly."
        )));
        self.flights.run(key, abandoned, || {
            // The flight that just landed may have kept it.
            if let Some(analysis) = remembered() {
                return Ok(analysis);
            }
            let sha = match source {
                Source::Commit(sha) => Some(sha),
                Source::WorkingTree => None,
            };
            let analysis =
                self.with_tree(project_id, repo, sha, |tree| self.run(analyzer, tree))??;
            if let Some(file) = &cached {
                // A cache that cannot be written only costs a later rerun.
                let _ = write_cached(file, &analysis);
            }
            let analysis = Arc::new(analysis);
            if let Some(stamp) = stamp {
                self.remember_working(project_id, analyzer, stamp, &analysis);
            }
            Ok(analysis)
        })
    }

    /// The working tree analysis kept for this analyzer build and stamp.
    fn remembered_working(
        &self,
        project_id: &str,
        analyzer: &Analyzer,
        stamp: u64,
    ) -> Option<Arc<Analysis>> {
        let working = self.working.lock().unwrap_or_else(PoisonError::into_inner);
        working
            .get(&(project_id.to_string(), analyzer.spec.id))
            .filter(|kept| kept.stamp == stamp && kept.fingerprint == analyzer.fingerprint)
            .map(|kept| Arc::clone(&kept.analysis))
    }

    /// Keeps `analysis` as the last working tree analysis, replacing the
    /// previous one of this project and language.
    fn remember_working(
        &self,
        project_id: &str,
        analyzer: &Analyzer,
        stamp: u64,
        analysis: &Arc<Analysis>,
    ) {
        let mut working = self.working.lock().unwrap_or_else(PoisonError::into_inner);
        working.insert(
            (project_id.to_string(), analyzer.spec.id),
            WorkingAnalysis {
                fingerprint: analyzer.fingerprint.clone(),
                stamp,
                analysis: Arc::clone(analysis),
            },
        );
    }

    fn run(&self, analyzer: &Analyzer, tree: &Path) -> Result<Analysis, SideError> {
        analyzers::run(
            analyzer.spec,
            &analyzer.launcher,
            tree,
            self.tool_path.get(),
            &self.settings,
        )
        .map_err(SideError::Failed)
    }

    fn cache_file(&self, project_id: &str, analyzer: &Analyzer, sha: &str) -> PathBuf {
        self.cache
            .join("analyses")
            .join(project_id)
            .join(analyzer.spec.id)
            .join(&analyzer.fingerprint)
            .join(format!("{sha}.json"))
    }

    /// Lines added and removed by the review. On the working tree side,
    /// untracked files count as added in full, as the review lists them.
    pub fn changed_lines(
        &self,
        repo: &Path,
        old_sha: &str,
        new_sha: Option<&str>,
    ) -> AppResult<hunks::ChangedLines> {
        let patch = self.git.diff_patch(repo, old_sha, new_sha)?;
        let mut lines = hunks::parse_patch(&patch)?;
        if new_sha.is_none() {
            for untracked in self.git.untracked_files(repo, &[])? {
                let Ok(added) = line_count::added_file(&repo.join(&untracked)) else {
                    continue;
                };
                if !added.binary {
                    let path = untracked.to_string_lossy();
                    lines.add_whole_file(path.trim_end_matches('/'), added.lines);
                }
            }
        }
        Ok(lines)
    }
}

fn failed(side: ReviewSide, message: CoreText) -> FlowUnavailable {
    FlowUnavailable::AnalysisFailed { side, message }
}

/// A cached analysis, or `None` when there is none or it cannot be used, in
/// which case it is deleted.
fn read_cached(file: &Path, analyzer: &Analyzer) -> Option<Arc<Analysis>> {
    let bytes = fs::read(file).ok()?;
    match analysis::parse(&bytes, analyzer.spec.languages, analyzer.spec.title()) {
        Ok(analysis) => {
            disk_cache::touch(file);
            Some(Arc::new(analysis))
        }
        Err(_) => {
            let _ = fs::remove_file(file);
            None
        }
    }
}

/// Writes atomically, then forgets older analyzer builds of this language
/// and all but the most recently used analyses.
fn write_cached(file: &Path, analysis: &Analysis) -> io::Result<()> {
    let bytes = serde_json::to_vec(analysis).map_err(io::Error::other)?;
    disk_cache::write(file, &bytes, KEEP_ANALYSES)
}

pub fn now() -> AppResult<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| {
            app_error!(
                Internal,
                TimeFormatFailed,
                "Could not format the current time: {detail}",
                detail = error
            )
        })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{analysis, call, entry, function};
    use crate::error::ErrorCode;
    use crate::model::{FlowGraph, FlowNodeState};
    use crate::test_support::{isolated_git, TestRepo};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant, SystemTime};
    use tempfile::TempDir;

    const PROJECT: &str = "project-1";
    const TOOL_PATH: &str = "/difflens/test/tools:/usr/bin:/bin";

    /// A stand-in analyzer: prints `fixture/analysis.json` of the tree it is
    /// given, or misbehaves as other files in `fixture/` ask, and records
    /// each run, its PATH and its folder next to itself.
    fn fake_analyzer(tools: &Path) -> PathBuf {
        let t = tools.display();
        let script = format!(
            r#"#!/bin/sh
echo run >> '{t}/runs'
if [ "$1" != analyze ] || [ "$2" != --dir ] || [ "$4" != --timeout ]; then
  echo "unexpected arguments: $*" >&2; exit 64
fi
dir="$3"
printf '%s' "$PATH" > '{t}/path-seen'
pwd -P >> '{t}/cwd-seen'
if [ -f "$dir/fixture/slow" ]; then sleep 1; fi
if [ -f "$dir/fixture/hang" ]; then sleep 30 & echo $! > '{t}/child.pid'; wait; fi
if [ -f "$dir/fixture/fail" ]; then
  echo 'compiling...' >&2; echo 'go: package example.com/p is broken' >&2; exit 2
fi
if [ -f "$dir/fixture/garbage" ]; then echo 'this is not json'; exit 0; fi
if [ -f "$dir/fixture/protocol2" ]; then echo '{{"protocol":2}}'; exit 0; fi
cat "$dir/fixture/analysis.json"
"#
        );
        let path = tools.join("fake analyzer");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    struct Harness {
        repo: TestRepo,
        tools: TempDir,
        cache: TempDir,
        service: FlowService,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_settings(settings(Duration::from_secs(30)))
        }

        fn with_settings(settings: RunSettings) -> Self {
            let tools = TempDir::new().unwrap();
            let analyzer = fake_analyzer(tools.path());
            Self::with_locator(
                AnalyzerLocator::with_executable("go", analyzer),
                settings,
                tools,
            )
        }

        fn with_locator(locator: AnalyzerLocator, settings: RunSettings, tools: TempDir) -> Self {
            let cache = TempDir::new().unwrap();
            let service = FlowService::new(
                isolated_git(),
                cache.path().to_path_buf(),
                locator,
                ToolPath::fixed(TOOL_PATH),
                settings,
            );
            let repo = TestRepo::init();
            Self {
                repo,
                tools,
                cache,
                service,
            }
        }

        fn runs(&self) -> usize {
            fs::read_to_string(self.tools.path().join("runs"))
                .map(|text| text.lines().count())
                .unwrap_or(0)
        }

        /// Commits `files` plus the analysis the fake analyzer prints for
        /// this commit, returns its sha.
        fn commit(&self, files: &[(&str, &str)], analysis: &Analysis) -> String {
            self.write(files, analysis);
            self.repo.commit_all("commit");
            self.repo.git(&["rev-parse", "HEAD"])
        }

        fn write(&self, files: &[(&str, &str)], analysis: &Analysis) {
            for (path, contents) in files {
                self.repo.write(path, contents);
            }
            self.repo.write(
                "fixture/analysis.json",
                serde_json::to_string(analysis).unwrap(),
            );
        }

        fn flow(&self, old: &str, new: Option<&str>) -> FlowResult {
            self.flow_with(old, new, FlowOptions::default())
        }

        fn flow_with(&self, old: &str, new: Option<&str>, options: FlowOptions) -> FlowResult {
            self.service
                .flow(PROJECT, self.repo.path(), old, new, options)
                .unwrap()
        }

        fn graph(&self, old: &str, new: Option<&str>) -> FlowGraph {
            self.graph_with(old, new, FlowOptions::default())
        }

        fn graph_with(&self, old: &str, new: Option<&str>, options: FlowOptions) -> FlowGraph {
            match self.flow_with(old, new, options) {
                FlowResult::Graph { graph } => *graph,
                other => panic!("expected a graph, got {other:?}"),
            }
        }
    }

    fn settings(hard_timeout: Duration) -> RunSettings {
        RunSettings {
            analyzer_timeout: Duration::from_secs(120),
            hard_timeout,
            max_output_bytes: 10 * 1024 * 1024,
        }
    }

    fn numbered_lines(count: u32) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    fn failure(result: FlowResult) -> (ReviewSide, String) {
        match result {
            FlowResult::Unavailable(FlowUnavailable::AnalysisFailed { side, message }) => {
                (side, message.message)
            }
            other => panic!("expected a failed analysis, got {other:?}"),
        }
    }

    /// Every file under `root` with its contents and modification time.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap().flatten() {
                let metadata = fs::symlink_metadata(entry.path()).unwrap();
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else {
                    let contents = fs::read(entry.path()).unwrap_or_default();
                    files.insert(entry.path(), (contents, metadata.modified().unwrap()));
                }
            }
        }
        files
    }

    fn old_side() -> Analysis {
        analysis(
            vec![
                function("p.X", "x.go", 1, 10),
                entry(function("p.Route", "x.go", 12, 15), "GET /x"),
            ],
            vec![call("p.Route", "p.X", 13)],
        )
    }

    fn new_side() -> Analysis {
        analysis(
            vec![
                function("p.X", "x.go", 1, 10),
                entry(function("p.Route", "x.go", 12, 15), "GET /x"),
                function("p.N", "n.go", 1, 3),
            ],
            vec![call("p.Route", "p.X", 13), call("p.X", "p.N", 4)],
        )
    }

    /// A Go project with two commits: x.go line 3 edited, n.go added.
    fn two_commits(harness: &Harness) -> (String, String) {
        let old = harness.commit(
            &[
                ("go.mod", "module example.com/p\n"),
                ("x.go", &numbered_lines(15)),
            ],
            &old_side(),
        );
        let new = harness.commit(
            &[
                (
                    "x.go",
                    &numbered_lines(15).replace("line 3\n", "line three\n"),
                ),
                ("n.go", "a\nb\nc\n"),
            ],
            &new_side(),
        );
        (old, new)
    }

    #[test]
    fn builds_the_graph_of_two_commits_and_caches_both_sides() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let git_before = snapshot(&harness.repo.path().join(".git"));

        let graph = harness.graph(&old, Some(&new));

        let state = |id: &str| {
            let node = graph.nodes.iter().find(|node| node.id == id).unwrap();
            (node.state, node.additions, node.deletions)
        };
        assert_eq!(state("p.X"), (FlowNodeState::Modified, 1, 1));
        assert_eq!(state("p.N"), (FlowNodeState::New, 3, 0));
        assert_eq!(graph.nodes.len(), 2, "the unchanged Route only on request");
        assert_eq!(graph.hidden.callers, 1);
        let edges: Vec<_> = graph
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.added))
            .collect();
        assert_eq!(edges, vec![("p.X", "p.N", true)]);
        assert_eq!(
            (graph.old_sha.as_str(), graph.new_sha.as_deref()),
            (old.as_str(), Some(new.as_str()))
        );
        assert_eq!(graph.language, "go");
        assert_eq!(harness.runs(), 2);

        // Cached: no more runs, same graph.
        let again = harness.graph(&old, Some(&new));
        assert_eq!(harness.runs(), 2);
        assert_eq!(again.nodes, graph.nodes);

        // The analyzer ran in an extracted tree with the tool PATH, and the
        // repository was not touched.
        assert_eq!(
            fs::read_to_string(harness.tools.path().join("path-seen")).unwrap(),
            TOOL_PATH
        );
        let cwd = fs::read_to_string(harness.tools.path().join("cwd-seen")).unwrap();
        let trees = fs::canonicalize(harness.cache.path().join("trees").join(PROJECT)).unwrap();
        assert!(
            cwd.lines()
                .all(|line| line.starts_with(trees.to_str().unwrap())),
            "{cwd}"
        );
        assert_eq!(snapshot(&harness.repo.path().join(".git")), git_before);
        let cached: Vec<_> = fs::read_dir(
            harness
                .cache
                .path()
                .join("analyses")
                .join(PROJECT)
                .join("go"),
        )
        .unwrap()
        .flatten()
        .flat_map(|fingerprint| fs::read_dir(fingerprint.path()).unwrap().flatten())
        .map(|file| file.file_name().to_string_lossy().into_owned())
        .collect();
        assert_eq!(cached.len(), 2, "{cached:?}");
    }

    #[test]
    fn the_working_tree_is_analysed_on_a_copy_and_never_cached() {
        let harness = Harness::new();
        let old = harness.commit(
            &[
                ("go.mod", "module example.com/p\n"),
                ("x.go", &numbered_lines(15)),
            ],
            &old_side(),
        );
        let mut working = new_side();
        working
            .functions
            .push(function("p.U", "untracked dir/u.go", 1, 2));
        harness.write(
            &[
                (
                    "x.go",
                    &numbered_lines(15).replace("line 3\n", "line three\n"),
                ),
                ("n.go", "a\nb\nc\n"),
                ("untracked dir/u.go", "u\nv\n"),
            ],
            &working,
        );
        let status_before = harness.repo.git(&["status", "--porcelain"]);
        let git_before = snapshot(&harness.repo.path().join(".git"));

        let graph = harness.graph(&old, None);

        let node = |id: &str| graph.nodes.iter().find(|node| node.id == id).unwrap();
        assert_eq!(node("p.U").additions, 2, "untracked files count in full");
        assert_eq!(node("p.N").state, FlowNodeState::New);
        assert_eq!(node("p.X").state, FlowNodeState::Modified);
        assert_eq!(graph.new_sha, None);
        assert_eq!(harness.runs(), 2);
        let cwd = fs::read_to_string(harness.tools.path().join("cwd-seen")).unwrap();
        let scratch = fs::canonicalize(harness.cache.path())
            .unwrap()
            .join("scratch");
        assert!(
            cwd.lines()
                .any(|line| line.starts_with(scratch.to_str().unwrap())),
            "{cwd}"
        );
        assert_eq!(
            fs::read_dir(&scratch).unwrap().count(),
            0,
            "the copy is deleted after use"
        );

        harness.graph(&old, None);
        assert_eq!(harness.runs(), 2, "kept in memory while no file changes");
        // Compared first: the test's own `git status` may refresh the index.
        assert_eq!(snapshot(&harness.repo.path().join(".git")), git_before);
        assert_eq!(harness.repo.git(&["status", "--porcelain"]), status_before);
        let cached: Vec<_> = fs::read_dir(
            harness
                .cache
                .path()
                .join("analyses")
                .join(PROJECT)
                .join("go"),
        )
        .unwrap()
        .flatten()
        .flat_map(|fingerprint| fs::read_dir(fingerprint.path()).unwrap().flatten())
        .map(|file| file.file_name().to_string_lossy().into_owned())
        .collect();
        assert_eq!(
            cached,
            vec![format!("{old}.json")],
            "only the commit is on disk"
        );

        harness.repo.write("n.go", "a\nb\nc\nd\n");
        harness.graph(&old, None);
        assert_eq!(
            harness.runs(),
            3,
            "an edited file runs the working tree again"
        );
    }

    #[test]
    fn changing_only_the_options_runs_no_analyzer() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let all = FlowOptions {
            include_tests: true,
            include_callers: true,
            include_callees: true,
        };

        harness.graph(&old, Some(&new));
        assert_eq!(harness.runs(), 2);
        let graph = harness.graph_with(&old, Some(&new), all);
        assert_eq!(harness.runs(), 2, "both commits are cached");
        assert_eq!(graph.options, all);
        assert_eq!(graph.entry_points, vec!["p.Route"], "the caller is drawn");

        harness.write(
            &[(
                "x.go",
                &numbered_lines(15).replace("line 5\n", "line five\n"),
            )],
            &new_side(),
        );
        harness.graph(&old, None);
        assert_eq!(harness.runs(), 3);
        let graph = harness.graph_with(&old, None, all);
        assert_eq!(harness.runs(), 3, "the working tree analysis is reused");
        assert_eq!(graph.options, all);
    }

    #[test]
    fn a_restart_that_rebuilt_the_same_analyzer_reuses_the_cache() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.graph(&old, Some(&new));
        assert_eq!(harness.runs(), 2);

        // `tauri dev` rebuilds the analyzer on every start: the same bytes,
        // a new modification time.
        let analyzer = harness.tools.path().join("fake analyzer");
        fs::write(&analyzer, fs::read(&analyzer).unwrap()).unwrap();
        let later = SystemTime::now() + Duration::from_secs(3600);
        fs::File::options().write(true).open(&analyzer).unwrap().set_modified(later).unwrap();
        let restarted = FlowService::new(
            isolated_git(),
            harness.cache.path().to_path_buf(),
            AnalyzerLocator::with_executable("go", analyzer.clone()),
            ToolPath::fixed(TOOL_PATH),
            settings(Duration::from_secs(30)),
        );
        let graph = restarted.flow(PROJECT, harness.repo.path(), &old, Some(&new), FlowOptions::default());
        assert!(matches!(graph, Ok(FlowResult::Graph { .. })), "{graph:?}");
        assert_eq!(harness.runs(), 2, "both commits come from the cache");

        // Another analyzer build analyses again.
        let mut script = fs::read_to_string(&analyzer).unwrap();
        script.push_str("# another build\n");
        fs::write(&analyzer, script).unwrap();
        restarted
            .flow(PROJECT, harness.repo.path(), &old, Some(&new), FlowOptions::default())
            .unwrap();
        assert_eq!(harness.runs(), 4);
    }

    /// The cache file of the old side of `two_commits`, once written.
    fn cached_old_side(harness: &Harness, old: &str) -> PathBuf {
        fs::read_dir(
            harness
                .cache
                .path()
                .join("analyses")
                .join(PROJECT)
                .join("go"),
        )
        .unwrap()
        .flatten()
        .map(|fingerprint| fingerprint.path().join(format!("{old}.json")))
        .find(|file| file.exists())
        .unwrap()
    }

    #[test]
    fn a_cached_analysis_without_the_test_flag_is_analysed_again() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.graph(&old, Some(&new));
        assert_eq!(harness.runs(), 2);
        let file = cached_old_side(&harness, &old);
        // What analyzer 0.1.0 left in the cache.
        let mut stale: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        for function in stale["functions"].as_array_mut().unwrap() {
            function.as_object_mut().unwrap().remove("test");
        }
        fs::write(&file, stale.to_string()).unwrap();

        let graph = harness.graph(&old, Some(&new));

        assert_eq!(harness.runs(), 3, "the stale file is a miss");
        assert!(!graph.nodes.is_empty());
        let fresh: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(fresh["functions"][0]["test"], false, "written again");
    }

    #[test]
    fn a_cached_analysis_without_the_relation_of_calls_is_analysed_again() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.graph(&old, Some(&new));
        assert_eq!(harness.runs(), 2);
        let file = cached_old_side(&harness, &old);
        // What analyzers before the relation left in the cache.
        let mut stale: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        for call in stale["calls"].as_array_mut().unwrap() {
            call.as_object_mut().unwrap().remove("relation");
        }
        fs::write(&file, stale.to_string()).unwrap();

        let graph = harness.graph(&old, Some(&new));

        assert_eq!(harness.runs(), 3, "the stale file is a miss");
        assert!(!graph.nodes.is_empty());
        let fresh: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(fresh["calls"][0]["relation"], "call", "written again");
    }

    #[test]
    fn concurrent_requests_for_the_same_sides_share_one_run() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        harness.repo.write("fixture/slow", "");
        harness.repo.commit_all("slow analyzer");
        let new = harness.repo.git(&["rev-parse", "HEAD"]);
        let barrier = std::sync::Barrier::new(2);
        let (service, repo) = (&harness.service, harness.repo.path());

        let results: Vec<FlowResult> = thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        service
                            .flow(PROJECT, repo, &old, Some(&new), FlowOptions::default())
                            .unwrap()
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        });

        assert_eq!(harness.runs(), 2, "one run per side");
        let nodes = |result: &FlowResult| match result {
            FlowResult::Graph { graph } => graph.nodes.clone(),
            other => panic!("expected a graph, got {other:?}"),
        };
        assert_eq!(nodes(&results[0]), nodes(&results[1]));
        assert!(!nodes(&results[0]).is_empty());
    }

    #[test]
    fn a_failing_analyzer_is_reported_with_its_error_output_and_not_cached() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        harness.repo.write("fixture/fail", "");
        harness.repo.commit_all("broken");
        let broken = harness.repo.git(&["rev-parse", "HEAD"]);

        let (side, message) = failure(harness.flow(&old, Some(&broken)));
        assert_eq!(side, ReviewSide::New);
        assert!(message.starts_with("The Go analyzer failed"), "{message}");
        assert!(
            message.ends_with("go: package example.com/p is broken"),
            "{message}"
        );
        let runs = harness.runs();

        failure(harness.flow(&old, Some(&broken)));
        assert_eq!(
            harness.runs(),
            runs + 1,
            "the failure is retried, the old side is cached"
        );
    }

    #[test]
    fn bad_output_and_another_protocol_are_reported() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        harness.repo.write("fixture/garbage", "");
        harness.repo.commit_all("garbage");
        let garbage = harness.repo.git(&["rev-parse", "HEAD"]);
        let (side, message) = failure(harness.flow(&garbage, Some(&old)));
        assert_eq!(side, ReviewSide::Old);
        assert!(message.contains("invalid analysis"), "{message}");

        harness.repo.git(&["rm", "--quiet", "fixture/garbage"]);
        harness.repo.write("fixture/protocol2", "");
        harness.repo.commit_all("future");
        let future = harness.repo.git(&["rev-parse", "HEAD"]);
        let (_, message) = failure(harness.flow(&old, Some(&future)));
        assert!(message.contains("speaks protocol 2"), "{message}");
    }

    #[test]
    fn a_hanging_analyzer_is_stopped_with_everything_it_started() {
        let harness = Harness::with_settings(settings(Duration::from_millis(1500)));
        let (old, new) = two_commits(&harness);
        // The old side is analysed first, without the short limit, by a
        // service sharing the cache: under the limit it would race the
        // hanging side, and a slow start of its analyzer would fail the old
        // side instead. Only the hanging analyzer runs below.
        let unhurried = FlowService::new(
            isolated_git(),
            harness.cache.path().to_path_buf(),
            AnalyzerLocator::with_executable("go", harness.tools.path().join("fake analyzer")),
            ToolPath::fixed(TOOL_PATH),
            settings(Duration::from_secs(30)),
        );
        let warmed = unhurried
            .flow(PROJECT, harness.repo.path(), &old, Some(&new), FlowOptions::default())
            .unwrap();
        assert!(matches!(warmed, FlowResult::Graph { .. }), "{warmed:?}");
        harness.repo.write("fixture/hang", "");
        harness.repo.commit_all("hang");
        let hanging = harness.repo.git(&["rev-parse", "HEAD"]);
        let runs = harness.runs();

        let started = Instant::now();
        let (side, message) = failure(harness.flow(&old, Some(&hanging)));

        assert_eq!(harness.runs(), runs + 1, "the old side is read from the cache");
        assert_eq!(side, ReviewSide::New);
        assert!(
            message.contains("did not finish within 1 seconds"),
            "{message}"
        );
        assert!(started.elapsed() < Duration::from_secs(15));
        let pid = fs::read_to_string(harness.tools.path().join("child.pid")).unwrap();
        let alive = || {
            Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the analyzer's child {pid} is still running");
    }

    #[test]
    fn a_missing_analyzer_asks_to_build_it() {
        let tools = TempDir::new().unwrap();
        let harness = Harness::with_locator(
            AnalyzerLocator::default(),
            settings(Duration::from_secs(5)),
            tools,
        );
        let (old, new) = two_commits(&harness);

        let (side, message) = failure(harness.flow(&old, Some(&new)));

        assert_eq!(side, ReviewSide::Old);
        assert_eq!(
            message,
            "The Go analyzer is not installed; run pnpm analyzers:build."
        );
    }

    #[test]
    fn a_project_without_a_known_language_has_no_analyzer() {
        let harness = Harness::new();
        let old = harness.commit(&[("README.md", "notes\n")], &old_side());
        let new = harness.commit(&[("notes.txt", "more\n")], &old_side());

        let result = harness.flow(&old, Some(&new));

        assert_eq!(
            result,
            FlowResult::Unavailable(FlowUnavailable::NoAnalyzer { languages: vec![] })
        );
        assert_eq!(harness.runs(), 0);
    }

    #[test]
    fn a_language_whose_analyzer_cannot_be_prepared_is_left_out_while_another_works() {
        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        harness.repo.write("tools/Cargo.toml", "[package]\n");

        let graph = harness.graph(&old, Some(&new));

        assert_eq!(graph.language, "go");
        let left_out = graph.diagnostics.last().unwrap();
        assert_eq!(*left_out, analyzers::for_language("rust").unwrap().not_installed());
        assert_eq!(left_out.reason.map(|reason| reason.key()), Some("analyzer_source_missing"));
    }

    #[test]
    fn a_provisioned_analyzer_that_cannot_be_prepared_says_so() {
        let tools = TempDir::new().unwrap();
        let harness = Harness::with_locator(
            AnalyzerLocator::default(),
            settings(Duration::from_secs(5)),
            tools,
        );
        let old = harness.commit(&[("Cargo.toml", "[package]\n")], &old_side());
        let new = harness.commit(&[("src/main.rs", "fn main() {}\n")], &old_side());

        let result = harness.flow(&old, Some(&new));

        assert_eq!(
            result,
            FlowResult::Unavailable(FlowUnavailable::AnalyzerUnavailable {
                analyzer: "Rust".to_string(),
                message: analyzers::for_language("rust").unwrap().not_installed(),
            })
        );
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["reason"], "analyzer_unavailable");
        assert_eq!(json["analyzer"], "Rust");
        assert_eq!(json["message"]["reason"], "analyzer_source_missing");
    }

    #[test]
    fn shas_must_be_commits_of_the_project() {
        let harness = Harness::new();
        let (old, _) = two_commits(&harness);
        let tree = harness.repo.git(&["rev-parse", "HEAD^{tree}"]);
        for (old, new) in [
            (old.as_str(), Some(tree.as_str())),
            ("0123456789abcdef0123456789abcdef01234567", None),
            ("HEAD", None),
        ] {
            let error = harness
                .service
                .flow(
                    PROJECT,
                    harness.repo.path(),
                    old,
                    new,
                    FlowOptions::default(),
                )
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::UnknownBranch, "{old} {new:?}");
        }
        assert_eq!(harness.runs(), 0);
    }

    #[test]
    fn results_serialise_to_the_contract_shapes() {
        let failed = failure_json(FlowResult::Unavailable(failed(
            ReviewSide::New,
            "boom".into(),
        )));
        assert_eq!(
            failed,
            serde_json::json!({ "kind": "unavailable", "reason": "analysis_failed", "side": "new", "message": "boom" })
        );
        let none = failure_json(FlowResult::Unavailable(FlowUnavailable::NoAnalyzer {
            languages: vec!["rust".to_string()],
        }));
        assert_eq!(
            none,
            serde_json::json!({ "kind": "unavailable", "reason": "no_analyzer", "languages": ["rust"] })
        );

        let harness = Harness::new();
        let (old, new) = two_commits(&harness);
        let graph = failure_json(harness.flow(&old, Some(&new)));
        assert_eq!(graph["kind"], "graph");
        let node = &graph["graph"]["nodes"][0];
        for field in [
            "id",
            "name",
            "qualifiedName",
            "container",
            "state",
            "file",
            "startLine",
            "endLine",
            "additions",
            "deletions",
            "complexity",
            "previousComplexity",
            "generated",
            "test",
            "entryPoints",
            "calls",
            "externalFunctions",
        ] {
            assert!(node.get(field).is_some(), "node lacks {field}: {node}");
        }
        let edge = &graph["graph"]["edges"][0];
        for field in [
            "from",
            "to",
            "position",
            "line",
            "kind",
            "resolution",
            "added",
            "removed",
            "relation",
            "evidence",
        ] {
            assert!(edge.get(field).is_some(), "edge lacks {field}: {edge}");
        }
        assert_eq!(
            (&edge["relation"], &edge["evidence"]),
            (&serde_json::json!("call"), &serde_json::Value::Null)
        );
        for field in [
            "language",
            "analyzer",
            "oldSha",
            "newSha",
            "nodes",
            "edges",
            "options",
            "hidden",
            "entryPoints",
            "truncated",
            "diagnostics",
            "analyzedAt",
        ] {
            assert!(graph["graph"].get(field).is_some(), "graph lacks {field}");
        }
        assert_eq!(
            graph["graph"]["options"],
            serde_json::json!({ "includeTests": false, "includeCallers": false, "includeCallees": false })
        );
        assert_eq!(
            graph["graph"]["hidden"],
            serde_json::json!({ "changedTests": 0, "callers": 1, "callees": 0, "externalPackages": 0 })
        );
        let x = graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "p.X")
            .unwrap();
        assert_eq!(
            x["calls"][0],
            serde_json::json!({
                "calleeId": "p.N", "qualifiedName": "pkg.N", "line": 4, "kind": "always",
                "external": false, "shown": true, "added": true, "removed": false,
                "relation": "call", "evidence": null
            })
        );
        let options: FlowOptions = serde_json::from_value(serde_json::json!({
            "includeTests": true, "includeCallers": true, "includeCallees": false
        }))
        .unwrap();
        assert!(options.include_tests && options.include_callers && !options.include_callees);
    }

    fn failure_json(result: FlowResult) -> serde_json::Value {
        serde_json::to_value(result).unwrap()
    }

    /// Builds the real Go analyzer from `analyzers/go` and runs it on a tiny
    /// Go repository. Needs Go on the PATH.
    #[test]
    #[ignore = "builds and runs the real Go analyzer"]
    fn the_go_analyzer_runs_end_to_end() {
        let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("../analyzers/go");
        let tools = TempDir::new().unwrap();
        let executable = tools.path().join("difflens-analyzer-go");
        let built = Command::new("go")
            .args(["build", "-o"])
            .arg(&executable)
            .arg("./cmd/difflens-analyzer-go")
            .current_dir(&sources)
            .status()
            .unwrap();
        assert!(built.success(), "go build failed");

        let cache = TempDir::new().unwrap();
        let service = FlowService::new(
            isolated_git(),
            cache.path().to_path_buf(),
            AnalyzerLocator::with_executable("go", executable),
            ToolPath::fixed(std::env::var_os("PATH").unwrap_or_default()),
            RunSettings::default(),
        );
        let repo = TestRepo::init();
        repo.write("go.mod", "module example.com/demo\n\ngo 1.22\n");
        repo.write(
            "main.go",
            "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(greet(\"a\"))\n}\n\nfunc greet(name string) string {\n\treturn \"hi \" + name\n}\n",
        );
        repo.commit_all("first");
        let old = repo.git(&["rev-parse", "HEAD"]);
        repo.write(
            "main.go",
            "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(greet(\"a\"))\n}\n\nfunc greet(name string) string {\n\tif name == \"\" {\n\t\treturn shout()\n\t}\n\treturn \"hi \" + name\n}\n\nfunc shout() string {\n\treturn \"HI\"\n}\n",
        );
        repo.commit_all("second");
        let new = repo.git(&["rev-parse", "HEAD"]);

        let result = service
            .flow(
                PROJECT,
                repo.path(),
                &old,
                Some(&new),
                FlowOptions::default(),
            )
            .unwrap();

        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        let FlowResult::Graph { graph } = result else {
            panic!("expected a graph");
        };
        let states: BTreeMap<&str, FlowNodeState> = graph
            .nodes
            .iter()
            .map(|node| (node.name.as_str(), node.state))
            .collect();
        assert_eq!(states.get("shout"), Some(&FlowNodeState::New), "{states:?}");
        assert_eq!(
            states.get("greet"),
            Some(&FlowNodeState::Modified),
            "{states:?}"
        );
        assert!(
            graph.edges.iter().any(|edge| edge.added),
            "{:?}",
            graph.edges
        );
    }
}
