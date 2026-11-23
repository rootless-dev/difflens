//! Project registry: which repositories are registered and what git says
//! about them right now. Only the registration, the integration branches, the
//! last fetch and pinned bases are stored; everything else is computed when
//! asked.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};
use std::thread;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::branches::{self, Candidate, Distances, Inventory};
#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, escaped, AppError, AppResult};
use crate::git::{AheadBehind, BranchRef, Git, RefKind};
use crate::languages;
use crate::line_count;
use crate::model::{
    Branch, BranchLocation, ChangeSet, CompareSummary, FetchResult, FileVersions, Isolation,
    Project, ProjectDetail, ReviewBase, ReviewBaseKind, ReviewTarget, TerminalLocation,
    WorkingTree, WorkingTreeSummary,
};
use crate::review;
use crate::store::{ProjectRecord, Store, DEFAULT_INTEGRATION_BRANCHES};
use crate::worktrees;

/// Upper bound of repositories inspected at the same time by `list`.
const MAX_PARALLEL_PROJECTS: usize = 8;

/// Upper bound of git processes counting commits for one project at a time.
const MAX_PARALLEL_GIT_CALLS: usize = 8;

pub struct ProjectService {
    store: Store,
    git: Git,
    /// Home directory, written as `~` in display paths.
    home: Option<PathBuf>,
}

/// Branches to review in a project, each with its base, and the bases that
/// exist.
struct Review {
    branches: Vec<Branch>,
    bases: Vec<String>,
}

/// A branch against its base: the base a review compares with, where the
/// branch left it, and the branch head.
struct Comparison {
    base: ReviewBase,
    merge_base_sha: String,
    head_sha: String,
}

impl ProjectService {
    pub fn new(store: Store, git: Git, home: Option<PathBuf>) -> Self {
        Self { store, git, home }
    }

    /// Projects newest first. A project whose folder is gone is still listed,
    /// with `available: false`.
    pub fn list(&self) -> AppResult<Vec<Project>> {
        let records = self.store.list()?;
        let mut projects = Vec::with_capacity(records.len());
        for chunk in records.chunks(MAX_PARALLEL_PROJECTS) {
            let described: Vec<AppResult<Project>> = thread::scope(|scope| {
                let workers: Vec<_> = chunk
                    .iter()
                    .map(|record| scope.spawn(move || self.describe(record)))
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| {
                        worker.join().unwrap_or_else(|_| {
                            Err(app_error!(
                                Internal,
                                ProjectReadFailed,
                                "Reading a project failed unexpectedly."
                            ))
                        })
                    })
                    .collect()
            });
            for project in described {
                projects.push(project?);
            }
        }
        Ok(projects)
    }

    /// Registers the repository that contains `path`. A folder inside a
    /// repository registers the repository root.
    pub fn add_local(&self, path: &str) -> AppResult<Project> {
        let root = self.repository_root(Path::new(path))?;
        self.register(&root)
    }

    /// Clones `url` into `destination`, a folder that must not exist yet, and
    /// registers the result.
    pub fn clone_repository(&self, url: &str, destination: &str) -> AppResult<Project> {
        let url = url.trim();
        if url.is_empty() || url.starts_with('-') {
            return Err(app_error!(
                CloneFailed,
                CloneUrlInvalid,
                "That is not a repository URL."
            ));
        }
        let destination = Path::new(destination);
        if !destination.is_absolute() {
            return Err(app_error!(
                CloneFailed,
                CloneDestinationNotAbsolute,
                "The destination must be an absolute path."
            ));
        }
        // symlink_metadata also sees a dangling symlink, which git would refuse.
        if destination.symlink_metadata().is_ok() {
            return Err(app_error!(
                DestinationExists,
                CloneDestinationExists,
                "{path} already exists.",
                path = destination.display()
            ));
        }

        self.git.clone_repository(url, destination)?;

        let root = self.repository_root(destination)?;
        // The destination may be a project whose folder had been deleted; the
        // clone brings that project back instead of failing as already added.
        if let Some(existing) = self.store.find_by_path(&path_text(&root)?)? {
            return self.describe(&existing);
        }
        self.register(&root)
    }

    /// Forgets the project. Nothing on disk is touched.
    pub fn remove(&self, id: &str) -> AppResult<()> {
        if self.store.delete(id)? {
            Ok(())
        } else {
            Err(unknown_project(id))
        }
    }

    pub fn detail(&self, id: &str) -> AppResult<ProjectDetail> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        if !self.is_repository_root(path)? {
            return Ok(ProjectDetail {
                project: self.project(&record, false, false, 0),
                branches: Vec::new(),
                bases: Vec::new(),
                working_tree: WorkingTree {
                    dirty: false,
                    files_changed: 0,
                },
                worktrees: Vec::new(),
            });
        }

        let review = self.review(path, &record)?;
        let has_remote = self.has_remote(path)?;
        let project = self.project(&record, true, has_remote, count_ahead(&review.branches));
        let working_tree = self.git.working_tree(path)?;
        let worktrees = worktrees::list(&self.git, path, self.home.as_deref())?;
        Ok(ProjectDetail {
            project,
            branches: review.branches,
            bases: review.bases,
            working_tree,
            worktrees,
        })
    }

    /// Where terminals open for `target`, or for the project screen when
    /// `None`; see `worktrees::location`.
    pub fn terminal_location(
        &self,
        id: &str,
        target: Option<&ReviewTarget>,
    ) -> AppResult<TerminalLocation> {
        let path = self.repository(id)?;
        worktrees::location(&self.git, &path, self.home.as_deref(), target)
    }

    /// Puts `.difflens/` in the repository's `info/exclude`, the first step
    /// of creating a worktree; see `worktrees::ensure_excluded`.
    pub fn exclude_difflens_folder(&self, id: &str) -> AppResult<()> {
        let path = self.repository(id)?;
        worktrees::ensure_excluded(&self.git, &path)
    }

    /// Creates the DiffLens worktree of `branch`; see `worktrees::create`.
    pub fn create_worktree(&self, id: &str, branch: &str) -> AppResult<TerminalLocation> {
        let path = self.repository(id)?;
        worktrees::create(&self.git, &path, self.home.as_deref(), branch)
    }

    /// Removes a DiffLens worktree; see `worktrees::remove`.
    pub fn remove_worktree(
        &self,
        id: &str,
        worktree: &str,
        force: bool,
        terminals_in: impl Fn(&Path) -> usize,
    ) -> AppResult<()> {
        let path = self.repository(id)?;
        worktrees::remove(&self.git, &path, Path::new(worktree), force, terminals_in)
    }

    /// Name of a registered project, as its screen shows it.
    pub fn project_name(&self, id: &str) -> AppResult<String> {
        Ok(project_name(&self.record(id)?))
    }

    /// Brings in the branches of every remote. Only remote-tracking branches
    /// change; see `Git::fetch`.
    pub fn fetch(&self, id: &str) -> AppResult<FetchResult> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;

        let remotes = self.git.remotes(path)?;
        if remotes.is_empty() {
            return Err(app_error!(
                NoRemote,
                NoRemote,
                "{project} has no remote to fetch from.",
                project = project_name(&record)
            ));
        }
        let before = self.git.remote_tracking_refs(path)?;
        for remote in &remotes {
            self.git.fetch(path, remote)?;
        }
        let after = self.git.remote_tracking_refs(path)?;

        let fetched_at = now()?;
        self.store.set_last_fetched_at(&record.id, &fetched_at)?;
        Ok(FetchResult {
            fetched_at,
            branches_updated: count_changed_refs(&before, &after),
        })
    }

    /// What `branch` adds on top of `base`, counted from their merge base.
    /// `branch` is a local branch or a branch of origin; `base` must be one of
    /// the project's bases.
    pub fn compare(&self, id: &str, branch: &str, base: &str) -> AppResult<CompareSummary> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;

        let Comparison {
            merge_base_sha,
            head_sha,
            ..
        } = self.comparison(path, &record, branch, base, None)?;
        let commits = self.git.count_commits(path, &merge_base_sha, &head_sha)?;
        let stat = self.git.diff_stat(path, &merge_base_sha, Some(&head_sha))?;
        Ok(CompareSummary {
            branch: branch.to_string(),
            base: base.to_string(),
            head_sha,
            merge_base_sha,
            commits,
            files_changed: stat.files_changed,
            additions: stat.additions,
            deletions: stat.deletions,
        })
    }

    /// Uncommitted changes against HEAD: tracked files, staged or not, plus
    /// untracked files that are not ignored, counted with all their lines as
    /// additions. Nothing is written to the index.
    pub fn compare_working_tree(&self, id: &str) -> AppResult<WorkingTreeSummary> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;

        let head = self.git.head_commit(path)?;
        // Before the first commit everything is compared with nothing.
        let from = match &head {
            Some(sha) => sha.clone(),
            None => self.git.empty_tree(path)?,
        };
        let tracked = self.git.diff_stat(path, &from, None)?;
        let untracked = self.git.untracked_files(path, &[])?;
        let untracked_additions = untracked
            .iter()
            // A file that cannot be read still counts as changed, with no
            // lines, rather than failing the whole summary.
            .map(|file| line_count::added_lines(&path.join(file)).unwrap_or(0))
            .fold(0u32, u32::saturating_add);
        let untracked_files = u32::try_from(untracked.len()).unwrap_or(u32::MAX);
        Ok(WorkingTreeSummary {
            head_sha: head.unwrap_or_default(),
            files_changed: tracked.files_changed.saturating_add(untracked_files),
            additions: tracked.additions.saturating_add(untracked_additions),
            deletions: tracked.deletions,
        })
    }

    /// `changed_files_against` a branch without a pull request.
    #[cfg(test)]
    pub fn changed_files(&self, id: &str, target: ReviewTarget) -> AppResult<ChangeSet> {
        self.changed_files_against(id, target, None)
    }

    /// The files a review target changes, with its two sides frozen: the
    /// merge base with the branch's base and the branch head, or HEAD and
    /// the working tree. A branch compares with `pull_request_base`, the
    /// destination commit of its pull request as the host gives it, when
    /// this repository has that commit; see `review_base`. Never writes the
    /// index.
    pub fn changed_files_against(
        &self,
        id: &str,
        target: ReviewTarget,
        pull_request_base: Option<&str>,
    ) -> AppResult<ChangeSet> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;

        let (old_sha, new_sha, base, files) = match &target {
            ReviewTarget::Branch { branch, base } => {
                let comparison =
                    self.comparison(path, &record, branch, base, pull_request_base)?;
                let files = review::between_commits(
                    &self.git,
                    path,
                    &comparison.merge_base_sha,
                    &comparison.head_sha,
                )?;
                (
                    comparison.merge_base_sha,
                    Some(comparison.head_sha),
                    Some(comparison.base),
                    files,
                )
            }
            ReviewTarget::WorkingTree => {
                // Before the first commit everything is compared with nothing.
                let old_sha = match self.git.head_commit(path)? {
                    Some(head) => head,
                    None => self.git.empty_tree(path)?,
                };
                let files = review::in_working_tree(&self.git, path, &old_sha)?;
                (old_sha, None, None, files)
            }
        };
        Ok(ChangeSet {
            target,
            old_sha,
            new_sha,
            base,
            files,
        })
    }

    /// Both sides of one file of a change set; see `review::file_versions`.
    pub fn file_versions(
        &self,
        id: &str,
        old_sha: &str,
        new_sha: Option<&str>,
        file: &str,
        old_file: Option<&str>,
    ) -> AppResult<FileVersions> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;
        review::file_versions(&self.git, path, old_sha, new_sha, file, old_file)
    }

    /// The storage, for the review state kept per project (`review_state`).
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// Folder of a registered project that is still a repository.
    pub fn repository(&self, id: &str) -> AppResult<PathBuf> {
        let record = self.record(id)?;
        let path = PathBuf::from(&record.path);
        self.ensure_available(&path)?;
        Ok(path)
    }

    /// How the next test run of the project is isolated: the last choice,
    /// the sandbox until there is one.
    pub fn test_isolation(&self, id: &str) -> AppResult<Isolation> {
        let record = self.record(id)?;
        Ok(match self.store.test_isolation(&record.id)?.as_deref() {
            Some("process") => Isolation::Process,
            _ => Isolation::Sandbox,
        })
    }

    pub fn set_test_isolation(&self, id: &str, isolation: Isolation) -> AppResult<()> {
        let record = self.record(id)?;
        let value = match isolation {
            Isolation::Process => "process",
            Isolation::Sandbox => "sandbox",
        };
        self.store.set_test_isolation(&record.id, value)
    }

    /// Pins the base of a branch to review, or with `None` goes back to the
    /// base found from history. Returns the branch as `detail` would.
    pub fn set_branch_base(&self, id: &str, branch: &str, base: Option<&str>) -> AppResult<Branch> {
        let record = self.record(id)?;
        let path = Path::new(&record.path);
        self.ensure_available(path)?;

        let inventory = Inventory::new(self.git.branch_refs(path)?);
        let is_review_branch = inventory
            .review_branches(&record.integration_branches)
            .any(|candidate| candidate.name == branch);
        if !is_review_branch {
            return Err(unknown_branch(branch));
        }
        if let Some(base) = base {
            self.resolve_base(&inventory, &record, base)?;
        }
        self.store.set_branch_base(&record.id, branch, base)?;

        self.review_inventory(path, &record, inventory)?
            .branches
            .into_iter()
            .find(|candidate| candidate.name == branch)
            .ok_or_else(|| unknown_branch(branch))
    }

    fn record(&self, id: &str) -> AppResult<ProjectRecord> {
        self.store.get(id)?.ok_or_else(|| unknown_project(id))
    }

    fn register(&self, root: &Path) -> AppResult<Project> {
        let record = ProjectRecord {
            id: Uuid::new_v4().to_string(),
            path: path_text(root)?,
            integration_branches: DEFAULT_INTEGRATION_BRANCHES
                .iter()
                .map(|name| name.to_string())
                .collect(),
            last_fetched_at: None,
            added_at: now()?,
        };
        // The unique path in the store is what rejects a second registration.
        self.store.insert(&record)?;
        self.describe(&record)
    }

    /// Canonical root of the repository that contains `path`.
    fn repository_root(&self, path: &Path) -> AppResult<PathBuf> {
        let not_found = || {
            app_error!(
                PathNotFound,
                PathNotFound,
                "{path} does not exist.",
                path = path.display()
            )
        };
        if !path.is_absolute() {
            return Err(not_found());
        }
        let canonical = fs::canonicalize(path).map_err(|_| not_found())?;
        let root = self.git.toplevel(&canonical)?.ok_or_else(|| {
            app_error!(
                NotAGitRepository,
                NotAGitRepository,
                "{path} is not a git repository.",
                path = path.display()
            )
        })?;
        Ok(fs::canonicalize(&root).unwrap_or(root))
    }

    fn describe(&self, record: &ProjectRecord) -> AppResult<Project> {
        let path = Path::new(&record.path);
        let available = self.is_repository_root(path)?;
        if !available {
            return Ok(self.project(record, false, false, 0));
        }
        // 0 when it cannot be computed, as the contract asks.
        let branches_ahead = self
            .review(path, record)
            .map(|review| count_ahead(&review.branches))
            .unwrap_or(0);
        let has_remote = self.has_remote(path)?;
        Ok(self.project(record, true, has_remote, branches_ahead))
    }

    fn project(
        &self,
        record: &ProjectRecord,
        available: bool,
        has_remote: bool,
        branches_ahead: u32,
    ) -> Project {
        let path = Path::new(&record.path);
        let languages = if available {
            languages::detect(path)
        } else {
            Vec::new()
        };
        // None when it cannot be read: the list only sorts by it.
        let last_modified_at = if available {
            self.git.last_commit_date(path).ok().flatten()
        } else {
            None
        };
        Project {
            id: record.id.clone(),
            name: project_name(record),
            path: record.path.clone(),
            display_path: display_path(&record.path, self.home.as_deref()),
            analysis_support: languages::analysis_support(&languages),
            languages,
            integration_branches: record.integration_branches.clone(),
            available,
            has_remote,
            branches_ahead,
            last_fetched_at: record.last_fetched_at.clone(),
            added_at: record.added_at.clone(),
            last_modified_at,
        }
    }

    /// True when `path` still is the root of a git working tree. A folder that
    /// lost its `.git` but sits inside another repository does not count.
    fn is_repository_root(&self, path: &Path) -> AppResult<bool> {
        if !path.is_dir() {
            return Ok(false);
        }
        let Some(root) = self.git.toplevel(path)? else {
            return Ok(false);
        };
        Ok(match (fs::canonicalize(&root), fs::canonicalize(path)) {
            (Ok(root), Ok(path)) => root == path,
            _ => false,
        })
    }

    /// True when the repository at `path` has at least one remote, so a fetch
    /// has something to do.
    fn has_remote(&self, path: &Path) -> AppResult<bool> {
        Ok(!self.git.remotes(path)?.is_empty())
    }

    fn ensure_available(&self, path: &Path) -> AppResult<()> {
        if !path.exists() {
            return Err(app_error!(
                PathNotFound,
                PathGone,
                "{path} no longer exists.",
                path = path.display()
            ));
        }
        if !self.is_repository_root(path)? {
            return Err(app_error!(
                NotAGitRepository,
                NoLongerAGitRepository,
                "{path} is no longer a git repository.",
                path = path.display()
            ));
        }
        Ok(())
    }

    /// The base of `branch` (see `review_base`), their merge base, and the
    /// head of `branch`. `branch` is a local branch or a branch of origin;
    /// `base` must be one of the project's bases.
    fn comparison(
        &self,
        path: &Path,
        record: &ProjectRecord,
        branch: &str,
        base: &str,
        pull_request_base: Option<&str>,
    ) -> AppResult<Comparison> {
        let inventory = Inventory::new(self.git.branch_refs(path)?);
        let head_sha = inventory
            .resolve(branch)
            .ok_or_else(|| unknown_branch(branch))?
            .head
            .sha
            .clone();
        let review_base = self.review_base(path, &inventory, record, base, pull_request_base)?;
        let merge_base_sha = self
            .git
            .merge_base(path, &review_base.sha, &head_sha)?
            .ok_or_else(|| {
                app_error!(
                    NoCommonHistory,
                    NoCommonHistory,
                    "{branch} and {base} have no common history to compare from.",
                    branch = branch,
                    base = base
                )
            })?;
        Ok(Comparison {
            base: review_base,
            merge_base_sha,
            head_sha,
        })
    }

    /// The commit a branch review compares with, the one place it is
    /// decided, so every view of the review (and its file count) reads the
    /// same old side:
    ///
    /// 1. the destination commit of the branch's pull request, as the host
    ///    reports it, when this repository has it: what the pull request
    ///    itself compares with;
    /// 2. otherwise the base branch of origin, such as `origin/develop`,
    ///    which a pull request would compare with even when the local
    ///    `develop` is out of date or has commits never pushed;
    /// 3. the local base branch only when origin has none of that name.
    ///
    /// The old side is then the merge base of that commit and the branch.
    fn review_base(
        &self,
        path: &Path,
        inventory: &Inventory,
        record: &ProjectRecord,
        name: &str,
        pull_request_base: Option<&str>,
    ) -> AppResult<ReviewBase> {
        let base_ref = self.resolve_base(inventory, record, name)?;
        if let Some(destination) = pull_request_base {
            if let Some(sha) = self.git.resolve_commit(path, destination)? {
                return Ok(ReviewBase {
                    name: name.to_string(),
                    kind: ReviewBaseKind::PullRequest,
                    ref_name: None,
                    sha,
                });
            }
        }
        Ok(ReviewBase {
            name: name.to_string(),
            kind: match base_ref.kind {
                RefKind::Origin => ReviewBaseKind::RemoteTracking,
                RefKind::Local => ReviewBaseKind::Local,
            },
            ref_name: Some(branches::ref_name(base_ref)),
            sha: base_ref.head.sha.clone(),
        })
    }

    /// The ref of `name` when it is one of the project's bases, origin's
    /// before the local one; see `Inventory::resolve_base`.
    fn resolve_base<'a>(
        &self,
        inventory: &'a Inventory,
        record: &ProjectRecord,
        name: &str,
    ) -> AppResult<&'a BranchRef> {
        let is_base = inventory
            .bases(&record.integration_branches)
            .iter()
            .any(|base| base == name);
        let not_a_base = || {
            app_error!(
                UnknownBranch,
                NotAnIntegrationBranch,
                "\"{branch}\" is not an integration branch of this project.",
                branch = escaped(name)
            )
        };
        if !is_base {
            return Err(not_a_base());
        }
        inventory.resolve_base(name).ok_or_else(not_a_base)
    }

    fn review(&self, path: &Path, record: &ProjectRecord) -> AppResult<Review> {
        let inventory = Inventory::new(self.git.branch_refs(path)?);
        self.review_inventory(path, record, inventory)
    }

    fn review_inventory(
        &self,
        path: &Path,
        record: &ProjectRecord,
        inventory: Inventory,
    ) -> AppResult<Review> {
        let patterns = &record.integration_branches;
        let bases = inventory.bases(patterns);
        let to_review: Vec<&BranchRef> = inventory.review_branches(patterns).collect();
        // Resolved as a review resolves them (`review_base`), so the base,
        // the counts and the review agree.
        let base_refs: Vec<(&str, &BranchRef, String)> = bases
            .iter()
            .filter_map(|name| {
                inventory
                    .resolve_base(name)
                    .map(|base| (name.as_str(), base, branches::ref_name(base)))
            })
            .collect();
        let base_commits: Vec<(&str, &str)> = base_refs
            .iter()
            .map(|(name, base, _)| (*name, base.head.sha.as_str()))
            .collect();
        let distances = if to_review.is_empty() {
            HashMap::new()
        } else {
            self.distances(path, &base_commits)?
        };
        let candidates: Vec<Candidate<'_>> = base_refs
            .iter()
            .filter_map(|(name, base, ref_name)| {
                distances
                    .get(base.head.sha.as_str())
                    .map(|distances| Candidate {
                        name,
                        ref_name,
                        sha: &base.head.sha,
                        distances,
                    })
            })
            .collect();
        let pins = self.store.branch_bases(&record.id)?;
        let branches = to_review
            .into_iter()
            .map(|branch| {
                let pinned = pins.get(&branch.name).map(String::as_str);
                branch_view(branch, pinned, &candidates)
            })
            .collect();
        Ok(Review { branches, bases })
    }

    /// Distances of every branch from each base commit: one git process per
    /// distinct commit (two bases at the same commit cost one), run in
    /// parallel because each is an independent read.
    fn distances<'a>(
        &self,
        path: &Path,
        base_commits: &[(&str, &'a str)],
    ) -> AppResult<HashMap<&'a str, Distances>> {
        let mut commits: Vec<&'a str> = base_commits.iter().map(|(_, commit)| *commit).collect();
        commits.sort_unstable();
        commits.dedup();
        let mut by_commit = HashMap::with_capacity(commits.len());
        for chunk in commits.chunks(MAX_PARALLEL_GIT_CALLS) {
            let measured: Vec<AppResult<Distances>> = thread::scope(|scope| {
                let workers: Vec<_> = chunk
                    .iter()
                    .map(|commit| scope.spawn(move || self.git.ahead_behind_all(path, commit)))
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| {
                        worker.join().unwrap_or_else(|_| {
                            Err(app_error!(
                                Internal,
                                CountingCommitsFailed,
                                "Counting commits failed unexpectedly."
                            ))
                        })
                    })
                    .collect()
            });
            for (commit, distances) in chunk.iter().zip(measured) {
                by_commit.insert(*commit, distances?);
            }
        }
        Ok(by_commit)
    }
}

fn branch_view(branch: &BranchRef, pinned: Option<&str>, candidates: &[Candidate<'_>]) -> Branch {
    let chosen = branches::choose_base(&branch.full_ref, pinned, candidates);
    let distance = chosen.as_ref().map_or(
        AheadBehind {
            ahead: 0,
            behind: 0,
        },
        |chosen| chosen.distance,
    );
    Branch {
        name: branch.name.clone(),
        location: match branch.kind {
            RefKind::Local => BranchLocation::Local,
            RefKind::Origin => BranchLocation::RemoteOnly,
        },
        is_current: branch.is_current,
        head: branch.head.clone(),
        upstream: branch.upstream.clone(),
        card: branches::card(&branch.name),
        base: chosen.map(|chosen| chosen.base),
        ahead: distance.ahead,
        behind: distance.behind,
    }
}

fn count_ahead(branches: &[Branch]) -> u32 {
    let ahead = branches.iter().filter(|branch| branch.ahead > 0).count();
    u32::try_from(ahead).unwrap_or(u32::MAX)
}

/// Refs that moved, appeared or disappeared between two snapshots.
fn count_changed_refs(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> u32 {
    let moved_or_gone = before
        .iter()
        .filter(|(name, commit)| after.get(*name) != Some(*commit))
        .count();
    let appeared = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .count();
    u32::try_from(moved_or_gone + appeared).unwrap_or(u32::MAX)
}

/// `path` with the home directory written as `~`, only when the home
/// directory is a whole leading part of it: `/home/ann2` is not under
/// `/home/ann`.
pub(crate) fn display_path(path: &str, home: Option<&Path>) -> String {
    let home = home.filter(|home| {
        home.is_absolute()
            && home
                .components()
                .any(|component| matches!(component, Component::Normal(_)))
    });
    let Some(home) = home else {
        return path.to_string();
    };
    match Path::new(path).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

fn project_name(record: &ProjectRecord) -> String {
    Path::new(&record.path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| record.path.clone())
}

fn unknown_project(id: &str) -> AppError {
    app_error!(
        UnknownProject,
        UnknownProject,
        "There is no project with id \"{id}\".",
        id = escaped(id)
    )
}

pub(crate) fn unknown_branch(name: &str) -> AppError {
    app_error!(
        UnknownBranch,
        UnknownBranch,
        "There is no branch named \"{branch}\".",
        branch = escaped(name)
    )
}

pub(crate) fn path_text(path: &Path) -> AppResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        app_error!(
            Internal,
            PathNotUnicode,
            "{path} is not valid Unicode and cannot be registered.",
            path = path.display()
        )
    })
}

pub(crate) fn now() -> AppResult<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AnalysisSupport, BaseSource, Upstream};
    use crate::test_support::{isolated_git, run_git, TestRepo};

    fn service() -> ProjectService {
        ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None)
    }

    fn text(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    fn code<T: std::fmt::Debug>(result: AppResult<T>) -> ErrorCode {
        result.unwrap_err().code
    }

    fn defaults() -> Vec<String> {
        DEFAULT_INTEGRATION_BRANCHES
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    /// A repository with one commit on `main` and a go.mod.
    fn go_repo() -> TestRepo {
        let repo = TestRepo::init();
        repo.commit_file("go.mod", "module example.com/demo\n", "initial commit");
        repo
    }

    /// The shape of the project that showed the problem: features cut from
    /// develop, hotfixes from master.
    ///
    /// master:  m1 ------------ m2 -- h1   (hotfix/OPS-12)
    /// develop:   \-- d1 -- d2 -- f1         (feature/ABC-178-login)
    /// stale:   m1
    fn acme_api() -> TestRepo {
        let repo = TestRepo::init_with_branch("master");
        repo.commit_file("go.mod", "module example.com/acme-api\n", "m1");
        repo.git(&["branch", "stale"]);
        repo.git(&["checkout", "--quiet", "-b", "develop"]);
        repo.commit_file("d1.go", "package acmeapi\n", "d1");
        repo.commit_file("d2.go", "package acmeapi\n", "d2");
        repo.git(&["checkout", "--quiet", "-b", "feature/ABC-178-login"]);
        repo.commit_file("login.go", "package acmeapi\n", "f1");
        repo.git(&["checkout", "--quiet", "master"]);
        repo.commit_file("m2.go", "package acmeapi\n", "m2");
        repo.git(&["checkout", "--quiet", "-b", "hotfix/OPS-12"]);
        repo.commit_file("fix.go", "package acmeapi\n", "h1");
        repo.git(&["checkout", "--quiet", "master"]);
        repo
    }

    type Row = (String, Option<(String, BaseSource)>, u32, u32);

    fn rows<'a>(branches: impl IntoIterator<Item = &'a Branch>) -> Vec<Row> {
        branches
            .into_iter()
            .map(|branch| {
                (
                    branch.name.clone(),
                    branch
                        .base
                        .as_ref()
                        .map(|base| (base.name.clone(), base.source)),
                    branch.ahead,
                    branch.behind,
                )
            })
            .collect()
    }

    fn row(name: &str, base: &str, source: BaseSource, ahead: u32, behind: u32) -> Row {
        (
            name.to_string(),
            Some((base.to_string(), source)),
            ahead,
            behind,
        )
    }

    fn find<'a>(detail: &'a ProjectDetail, name: &str) -> &'a Branch {
        detail
            .branches
            .iter()
            .find(|branch| branch.name == name)
            .unwrap_or_else(|| panic!("no branch {name}"))
    }

    #[test]
    fn adds_a_local_repository() {
        let repo = go_repo();
        let service = service();

        let project = service.add_local(text(repo.path())).unwrap();

        assert!(Uuid::parse_str(&project.id).is_ok());
        assert_eq!(project.name, "repo with space");
        assert_eq!(project.path, text(repo.path()));
        assert_eq!(project.display_path, text(repo.path()));
        assert_eq!(project.languages, vec!["go"]);
        assert_eq!(project.analysis_support, AnalysisSupport::Full);
        assert_eq!(project.integration_branches, defaults());
        assert!(project.available);
        assert!(!project.has_remote);
        assert_eq!(project.branches_ahead, 0);
        assert_eq!(project.last_fetched_at, None);
        assert!(OffsetDateTime::parse(&project.added_at, &Rfc3339).is_ok());
        let modified = project.last_modified_at.as_deref().unwrap();
        assert!(OffsetDateTime::parse(modified, &Rfc3339).is_ok());
        assert_eq!(service.list().unwrap(), vec![project]);
    }

    #[test]
    fn display_path_replaces_the_home_folder_on_a_component_boundary() {
        let home = Path::new("/Users/ann");
        assert_eq!(display_path("/Users/ann/code/x", Some(home)), "~/code/x");
        assert_eq!(display_path("/Users/ann", Some(home)), "~");
        assert_eq!(
            display_path("/Users/ann/code/x", Some(Path::new("/Users/ann/"))),
            "~/code/x"
        );
        assert_eq!(display_path("/Users/anna/x", Some(home)), "/Users/anna/x");
        assert_eq!(display_path("/opt/x", Some(home)), "/opt/x");
        assert_eq!(display_path("/Users/ann/x", None), "/Users/ann/x");
        assert_eq!(display_path("/x", Some(Path::new("/"))), "/x");
        assert_eq!(display_path("/x", Some(Path::new("relative"))), "/x");
    }

    #[test]
    fn projects_show_their_path_relative_to_home() {
        let repo = go_repo();
        let service = ProjectService::new(
            Store::open_in_memory().unwrap(),
            isolated_git(),
            Some(repo.parent().to_path_buf()),
        );

        let project = service.add_local(text(repo.path())).unwrap();

        assert_eq!(project.display_path, "~/repo with space");
        assert_eq!(project.path, text(repo.path()));
    }

    #[test]
    fn adding_a_subfolder_registers_the_repository_root() {
        let repo = go_repo();
        repo.commit_file("pkg/inner/file.go", "package inner\n", "add package");
        let service = service();

        let project = service
            .add_local(text(&repo.path().join("pkg/inner")))
            .unwrap();

        assert_eq!(project.path, text(repo.path()));
    }

    #[test]
    fn a_repository_without_a_known_language_is_diff_only() {
        let repo = TestRepo::init();
        repo.commit_file("notes.txt", "hello\n", "initial commit");

        let project = service().add_local(text(repo.path())).unwrap();

        assert!(project.languages.is_empty());
        assert_eq!(project.analysis_support, AnalysisSupport::DiffOnly);
    }

    #[test]
    fn adding_reports_missing_paths_non_repositories_and_duplicates() {
        let repo = go_repo();
        let service = service();
        let plain_folder = repo.parent().join("plain");
        fs::create_dir(&plain_folder).unwrap();

        assert_eq!(
            code(service.add_local(text(&repo.parent().join("missing")))),
            ErrorCode::PathNotFound
        );
        assert_eq!(code(service.add_local("")), ErrorCode::PathNotFound);
        assert_eq!(
            code(service.add_local("relative/path")),
            ErrorCode::PathNotFound
        );
        assert_eq!(
            code(service.add_local(text(&plain_folder))),
            ErrorCode::NotAGitRepository
        );
        assert_eq!(
            code(service.add_local(text(&repo.path().join("go.mod")))),
            ErrorCode::NotAGitRepository
        );

        service.add_local(text(repo.path())).unwrap();
        assert_eq!(
            code(service.add_local(text(repo.path()))),
            ErrorCode::AlreadyAdded
        );
        assert_eq!(service.list().unwrap().len(), 1);
    }

    #[test]
    fn a_missing_git_binary_is_reported_as_git_not_found() {
        let repo = go_repo();
        let service = ProjectService::new(
            Store::open_in_memory().unwrap(),
            Git::new().with_program("difflens-test-no-such-git"),
            None,
        );

        assert_eq!(
            code(service.add_local(text(repo.path()))),
            ErrorCode::GitNotFound
        );
        assert_eq!(
            code(service.clone_repository(text(repo.path()), text(&repo.parent().join("clone")))),
            ErrorCode::GitNotFound
        );
    }

    #[test]
    fn clones_and_registers_a_repository() {
        let source = TestRepo::init_with_branch("trunk");
        source.commit_file("go.mod", "module example.com/demo\n", "first");
        let destination = source.parent().join("nested/my clone");
        let service = service();

        let project = service
            .clone_repository(text(source.path()), text(&destination))
            .unwrap();

        assert_eq!(project.name, "my clone");
        assert_eq!(project.path, text(&destination));
        assert_eq!(project.integration_branches, defaults());
        assert_eq!(project.languages, vec!["go"]);
        assert!(project.available);
        // The clone has origin.
        assert!(project.has_remote);
        assert!(destination.join("go.mod").is_file());
        assert_eq!(service.list().unwrap(), vec![project]);
    }

    #[test]
    fn clone_reports_an_existing_destination_and_a_failed_clone() {
        let source = go_repo();
        let service = service();
        let existing = source.parent().join("existing");
        fs::create_dir(&existing).unwrap();
        let destination = source.parent().join("clone");

        assert_eq!(
            code(service.clone_repository(text(source.path()), text(&existing))),
            ErrorCode::DestinationExists
        );
        assert_eq!(
            code(service.clone_repository(
                text(&source.parent().join("no such repository")),
                text(&destination)
            )),
            ErrorCode::CloneFailed
        );
        assert_eq!(
            code(service.clone_repository(text(source.path()), "relative/clone")),
            ErrorCode::CloneFailed
        );
        assert!(!destination.exists());
        assert!(service.list().unwrap().is_empty());
    }

    #[test]
    fn clone_rejects_urls_that_git_could_execute_or_read_as_options() {
        let source = go_repo();
        let service = service();
        let destination = source.parent().join("clone");
        let marker = source.parent().join("executed");
        let ext_url = format!("ext::sh -c touch% {}", text(&marker));
        let option_url = format!("--upload-pack=touch {}", text(&marker));

        for url in [ext_url.as_str(), option_url.as_str(), "-v", "  "] {
            assert_eq!(
                code(service.clone_repository(url, text(&destination))),
                ErrorCode::CloneFailed,
                "url {url:?}"
            );
        }
        assert!(!marker.exists());
        assert!(!destination.exists());
    }

    #[test]
    fn cloning_into_the_folder_of_a_vanished_project_restores_it() {
        let source = go_repo();
        let destination = source.parent().join("clone");
        let service = service();
        let first = service
            .clone_repository(text(source.path()), text(&destination))
            .unwrap();
        assert!(first.has_remote);
        fs::remove_dir_all(&destination).unwrap();

        // An unavailable project has no remote, even one that had origin.
        let vanished = &service.list().unwrap()[0];
        assert!(!vanished.available);
        assert!(!vanished.has_remote);
        assert!(!service.detail(&first.id).unwrap().project.has_remote);

        let second = service
            .clone_repository(text(source.path()), text(&destination))
            .unwrap();

        assert_eq!(second.id, first.id);
        assert!(second.available);
        assert!(second.has_remote);
        assert_eq!(service.list().unwrap().len(), 1);
    }

    #[test]
    fn lists_newest_first() {
        let first = go_repo();
        let second = go_repo();
        let service = service();
        let first_id = service.add_local(text(first.path())).unwrap().id;
        let second_id = service.add_local(text(second.path())).unwrap().id;

        let ids: Vec<String> = service.list().unwrap().into_iter().map(|p| p.id).collect();

        assert_eq!(ids, vec![second_id, first_id]);
    }

    #[test]
    fn lists_more_projects_than_one_parallel_batch_in_order() {
        let repo = go_repo();
        let service = service();
        let mut expected = Vec::new();
        for index in 0..=MAX_PARALLEL_PROJECTS {
            let copy = repo.parent().join(format!("copy {index}"));
            run_git(
                repo.parent(),
                &["clone", "--quiet", text(repo.path()), text(&copy)],
            );
            expected.push(service.add_local(text(&copy)).unwrap().id);
        }
        expected.reverse();

        let ids: Vec<String> = service.list().unwrap().into_iter().map(|p| p.id).collect();

        assert_eq!(ids, expected);
    }

    #[test]
    fn a_removed_folder_is_listed_as_unavailable() {
        let kept = go_repo();
        let removed = go_repo();
        let service = service();
        let kept_id = service.add_local(text(kept.path())).unwrap().id;
        let removed_project = service.add_local(text(removed.path())).unwrap();
        fs::remove_dir_all(removed.path()).unwrap();

        let projects = service.list().unwrap();

        assert_eq!(projects.len(), 2);
        let gone = &projects[0];
        assert_eq!(gone.id, removed_project.id);
        assert!(!gone.available);
        assert_eq!(gone.name, "repo with space");
        assert_eq!(gone.integration_branches, defaults());
        assert!(gone.languages.is_empty());
        assert_eq!(gone.analysis_support, AnalysisSupport::DiffOnly);
        assert_eq!(gone.branches_ahead, 0);
        assert_eq!(gone.last_modified_at, None);
        assert_eq!(projects[1].id, kept_id);
        assert!(projects[1].available);
        assert!(projects[1].last_modified_at.is_some());

        let detail = service.detail(&gone.id).unwrap();
        assert!(!detail.project.available);
        assert!(detail.branches.is_empty());
        assert!(detail.bases.is_empty());
        assert!(!detail.working_tree.dirty);
        for result in [
            service.compare(&gone.id, "main", "main").map(|_| ()),
            service.compare_working_tree(&gone.id).map(|_| ()),
            service.fetch(&gone.id).map(|_| ()),
            service.set_branch_base(&gone.id, "x", None).map(|_| ()),
        ] {
            assert_eq!(code(result), ErrorCode::PathNotFound);
        }
    }

    #[test]
    fn the_last_modification_is_the_newest_commit_of_any_local_or_origin_branch() {
        let repo = TestRepo::init();
        repo.commit_file("go.mod", "module example.com/demo\n", "m1");
        repo.git(&["checkout", "--quiet", "-b", "feature/a"]);
        repo.commit_file("a.go", "package demo\n", "a1");
        repo.git(&["checkout", "--quiet", "-b", "feature/b"]);
        repo.commit_file("b.go", "package demo\n", "b1");
        // b1, the newest commit, is left only on a branch of origin.
        repo.git(&["update-ref", "refs/remotes/origin/feature/b", "feature/b"]);
        repo.git(&["checkout", "--quiet", "main"]);
        repo.git(&["branch", "-D", "feature/b"]);
        let service = service();

        let project = service.add_local(text(repo.path())).unwrap();

        let b1 = repo.git(&["log", "-1", "--format=%cI", "refs/remotes/origin/feature/b"]);
        let a1 = repo.git(&["log", "-1", "--format=%cI", "feature/a"]);
        assert_ne!(b1, a1);
        assert_eq!(project.last_modified_at.as_deref(), Some(b1.as_str()));
        assert_eq!(service.list().unwrap()[0].last_modified_at, Some(b1.clone()));
        assert_eq!(
            service.detail(&project.id).unwrap().project.last_modified_at,
            Some(b1)
        );
    }

    #[test]
    fn a_repository_without_commits_has_no_last_modification() {
        let repo = TestRepo::init();
        let service = service();

        let project = service.add_local(text(repo.path())).unwrap();

        assert!(project.available);
        assert_eq!(project.last_modified_at, None);
        assert_eq!(service.list().unwrap()[0].last_modified_at, None);
    }

    #[test]
    fn a_folder_that_is_no_longer_a_repository_is_unavailable() {
        let repo = go_repo();
        let service = service();
        let project = service.add_local(text(repo.path())).unwrap();
        fs::remove_dir_all(repo.path().join(".git")).unwrap();

        let listed = service.list().unwrap();

        assert!(!listed[0].available);
        assert_eq!(
            code(service.compare(&project.id, "main", "main")),
            ErrorCode::NotAGitRepository
        );
    }

    #[test]
    fn a_folder_inside_another_repository_is_not_mistaken_for_the_project() {
        let outer = go_repo();
        let inner = outer.path().join("inner");
        fs::create_dir(&inner).unwrap();
        run_git(&inner, &["init", "--quiet", "--initial-branch", "main"]);
        let service = service();
        service.add_local(text(&inner)).unwrap();
        fs::remove_dir_all(inner.join(".git")).unwrap();

        assert!(!service.list().unwrap()[0].available);
    }

    #[test]
    fn remove_forgets_the_project_and_keeps_the_folder() {
        let repo = go_repo();
        let service = service();
        let project = service.add_local(text(repo.path())).unwrap();

        service.remove(&project.id).unwrap();

        assert!(service.list().unwrap().is_empty());
        assert!(repo.path().join("go.mod").is_file());
        assert_eq!(code(service.remove(&project.id)), ErrorCode::UnknownProject);
        assert_eq!(code(service.detail(&project.id)), ErrorCode::UnknownProject);
        assert_eq!(
            code(service.compare(&project.id, "main", "main")),
            ErrorCode::UnknownProject
        );
        assert_eq!(code(service.fetch(&project.id)), ErrorCode::UnknownProject);
        assert_eq!(
            code(service.compare_working_tree(&project.id)),
            ErrorCode::UnknownProject
        );
        assert_eq!(
            code(service.set_branch_base(&project.id, "main", None)),
            ErrorCode::UnknownProject
        );
        // Removing frees the path to be added again.
        service.add_local(text(repo.path())).unwrap();
    }

    #[test]
    fn the_test_isolation_is_the_sandbox_until_another_is_chosen() {
        let repo = go_repo();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        assert_eq!(service.test_isolation(&id).unwrap(), Isolation::Sandbox);
        service.set_test_isolation(&id, Isolation::Process).unwrap();
        assert_eq!(service.test_isolation(&id).unwrap(), Isolation::Process);
        service.set_test_isolation(&id, Isolation::Sandbox).unwrap();
        assert_eq!(service.test_isolation(&id).unwrap(), Isolation::Sandbox);
        assert_eq!(
            code(service.test_isolation("missing")),
            ErrorCode::UnknownProject
        );
        assert_eq!(
            code(service.set_test_isolation("missing", Isolation::Process)),
            ErrorCode::UnknownProject
        );
    }

    #[test]
    fn each_branch_is_compared_with_the_closest_integration_branch() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let detail = service.detail(&id).unwrap();

        assert_eq!(detail.bases, vec!["master", "develop"]);
        assert_eq!(
            rows(&detail.branches),
            vec![
                row("hotfix/OPS-12", "master", BaseSource::History, 1, 0),
                row(
                    "feature/ABC-178-login",
                    "develop",
                    BaseSource::History,
                    1,
                    0
                ),
                row("stale", "master", BaseSource::History, 0, 1),
            ]
        );
        let feature = find(&detail, "feature/ABC-178-login");
        assert_eq!(feature.card.as_deref(), Some("ABC-178"));
        assert_eq!(feature.location, BranchLocation::Local);
        assert_eq!(feature.upstream, None);
        assert!(!feature.is_current);
        assert_eq!(feature.head.subject, "f1");
        assert_eq!(
            feature.head.sha,
            repo.git(&["rev-parse", "feature/ABC-178-login"])
        );
        assert_eq!(
            find(&detail, "hotfix/OPS-12").card.as_deref(),
            Some("OPS-12")
        );
        assert_eq!(find(&detail, "stale").card, None);
        assert_eq!(detail.project.branches_ahead, 2);
        assert_eq!(service.list().unwrap()[0].branches_ahead, 2);
        assert_eq!(
            detail.working_tree,
            WorkingTree {
                dirty: false,
                files_changed: 0
            }
        );
    }

    #[test]
    fn the_base_carries_its_commit_and_follows_it_when_the_base_moves() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let feature = "feature/ABC-178-login";
        let base_sha = |detail: &ProjectDetail, name: &str| {
            find(detail, name).base.as_ref().unwrap().sha.clone()
        };

        let before = service.detail(&id).unwrap();

        let develop = repo.git(&["rev-parse", "develop"]);
        assert_eq!(base_sha(&before, feature), develop);
        assert_eq!(
            base_sha(&before, "hotfix/OPS-12"),
            repo.git(&["rev-parse", "master"])
        );

        repo.git(&["checkout", "--quiet", "develop"]);
        repo.commit_file("d3.go", "package acmeapi\n", "d3");
        let after = service.detail(&id).unwrap();

        let moved = repo.git(&["rev-parse", "develop"]);
        assert_ne!(moved, develop);
        assert_eq!(base_sha(&after, feature), moved);
        // The branch itself did not move.
        assert_eq!(
            find(&after, feature).head.sha,
            find(&before, feature).head.sha
        );
    }

    #[test]
    fn equal_distances_are_settled_by_the_order_of_the_integration_branches() {
        let repo = TestRepo::init_with_branch("develop");
        repo.commit_file("a.txt", "a\n", "first");
        repo.git(&["branch", "master"]);
        repo.git(&["branch", "twin"]);
        repo.git(&["checkout", "--quiet", "-b", "topic"]);
        repo.commit_file("b.txt", "b\n", "topic");
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let detail = service.detail(&id).unwrap();

        // The defaults list master before develop.
        assert_eq!(detail.bases, vec!["master", "develop"]);
        assert_eq!(
            rows(&detail.branches),
            vec![
                row("topic", "master", BaseSource::History, 1, 0),
                row("twin", "master", BaseSource::History, 0, 0),
            ]
        );
        assert!(detail.branches[0].is_current);
        assert_eq!(detail.project.branches_ahead, 1);
    }

    #[test]
    fn glob_patterns_make_release_branches_bases() {
        let repo = acme_api();
        repo.git(&["checkout", "--quiet", "-b", "release/2.4"]);
        repo.commit_file("release.txt", "2.4\n", "release 2.4");
        repo.git(&["checkout", "--quiet", "-b", "fix/release-typo"]);
        repo.commit_file("typo.txt", "fixed\n", "typo");
        repo.git(&["branch", "release/rc/2.5", "master"]);
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let detail = service.detail(&id).unwrap();

        assert_eq!(detail.bases, vec!["master", "develop", "release/2.4"]);
        let names: Vec<&str> = detail.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(!names.contains(&"release/2.4"));
        // `*` does not cross a slash, as in git's own patterns.
        assert!(names.contains(&"release/rc/2.5"));
        assert_eq!(
            rows([find(&detail, "fix/release-typo")]),
            vec![row(
                "fix/release-typo",
                "release/2.4",
                BaseSource::History,
                1,
                0
            )]
        );
    }

    #[test]
    fn a_repository_without_integration_branches_has_no_bases() {
        let repo = TestRepo::init_with_branch("trunk");
        repo.commit_file("a.txt", "a\n", "first");
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let detail = service.detail(&id).unwrap();

        assert!(detail.bases.is_empty());
        assert_eq!(detail.branches.len(), 1);
        assert_eq!(detail.branches[0].base, None);
        assert_eq!(
            (detail.branches[0].ahead, detail.branches[0].behind),
            (0, 0)
        );
        assert_eq!(detail.project.branches_ahead, 0);
    }

    #[test]
    fn the_user_can_pin_a_base_and_go_back_to_history() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let feature = "feature/ABC-178-login";

        let pinned = service
            .set_branch_base(&id, feature, Some("master"))
            .unwrap();

        assert_eq!(
            rows([&pinned]),
            vec![row(feature, "master", BaseSource::User, 3, 1)]
        );
        let detail = service.detail(&id).unwrap();
        assert_eq!(find(&detail, feature), &pinned);
        // Other branches keep the base found from history.
        assert_eq!(
            find(&detail, "hotfix/OPS-12")
                .base
                .as_ref()
                .map(|b| b.source),
            Some(BaseSource::History)
        );

        let reset = service.set_branch_base(&id, feature, None).unwrap();

        assert_eq!(
            rows([&reset]),
            vec![row(feature, "develop", BaseSource::History, 1, 0)]
        );
    }

    #[test]
    fn a_pinned_base_that_disappears_falls_back_to_history_and_returns_with_it() {
        let repo = acme_api();
        repo.git(&["branch", "release/1.0", "master"]);
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let feature = "feature/ABC-178-login";
        service
            .set_branch_base(&id, feature, Some("release/1.0"))
            .unwrap();

        repo.git(&["branch", "--quiet", "-D", "release/1.0"]);
        let without = service.detail(&id).unwrap();

        assert_eq!(
            rows([find(&without, feature)]),
            vec![row(feature, "develop", BaseSource::History, 1, 0)]
        );

        repo.git(&["branch", "release/1.0", "master"]);
        let with = service.detail(&id).unwrap();

        assert_eq!(
            rows([find(&with, feature)]),
            vec![row(feature, "release/1.0", BaseSource::User, 3, 1)]
        );
    }

    #[test]
    fn pinning_accepts_only_branches_to_review_and_existing_bases() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let feature = "feature/ABC-178-login";

        for (branch, base) in [
            ("missing", Some("master")),
            ("develop", Some("master")),
            ("develop", None),
            (feature, Some("main")),
            (feature, Some("stale")),
            (feature, Some("origin/master")),
            (feature, Some("--all")),
        ] {
            assert_eq!(
                code(service.set_branch_base(&id, branch, base)),
                ErrorCode::UnknownBranch,
                "branch {branch:?} base {base:?}"
            );
        }
        assert!(service.store.branch_bases(&id).unwrap().is_empty());
    }

    #[test]
    fn remote_only_branches_are_listed_and_bases_may_live_on_origin() {
        let source = acme_api();
        // The clone only has master locally; develop exists as origin/develop.
        let clone = TestRepo::clone_from(source.path());
        let service = service();
        let id = service.add_local(text(clone.path())).unwrap().id;

        let detail = service.detail(&id).unwrap();

        assert_eq!(detail.bases, vec!["master", "develop"]);
        assert_eq!(
            rows(&detail.branches),
            vec![
                row("hotfix/OPS-12", "master", BaseSource::History, 1, 0),
                row(
                    "feature/ABC-178-login",
                    "develop",
                    BaseSource::History,
                    1,
                    0
                ),
                row("stale", "master", BaseSource::History, 0, 1),
            ]
        );
        assert!(detail
            .branches
            .iter()
            .all(|b| b.location == BranchLocation::RemoteOnly
                && b.upstream.is_none()
                && !b.is_current));
        assert_eq!(detail.project.branches_ahead, 2);
        // develop has no local branch, so its commit is origin's.
        assert_eq!(
            find(&detail, "feature/ABC-178-login")
                .base
                .as_ref()
                .map(|base| base.sha.clone()),
            Some(clone.git(&["rev-parse", "origin/develop"]))
        );

        let summary = service
            .compare(&id, "feature/ABC-178-login", "develop")
            .unwrap();
        assert_eq!((summary.commits, summary.files_changed), (1, 1));
        assert_eq!(
            summary.head_sha,
            clone.git(&["rev-parse", "origin/feature/ABC-178-login"])
        );

        // A local branch hides the remote one of the same name.
        clone.git(&["branch", "--quiet", "stale", "origin/stale"]);
        let detail = service.detail(&id).unwrap();
        let stale: Vec<&Branch> = detail
            .branches
            .iter()
            .filter(|b| b.name == "stale")
            .collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].location, BranchLocation::Local);
        assert_eq!(
            stale[0].upstream,
            Some(Upstream {
                name: "origin/stale".to_string(),
                ahead: 0,
                behind: 0
            })
        );
    }

    fn branch_target(branch: &str, base: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: branch.to_string(),
            base: base.to_string(),
        }
    }

    fn paths(change_set: &ChangeSet) -> Vec<&str> {
        change_set
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    }

    #[test]
    fn a_base_with_commits_never_pushed_is_compared_on_origin() {
        let source = acme_api();
        let clone = TestRepo::clone_from(source.path());
        clone.git(&["checkout", "--quiet", "develop"]);
        clone.commit_file("d3.go", "package acmeapi\n", "d3, not pushed");
        clone.git(&["checkout", "--quiet", "-b", "feature/new"]);
        clone.commit_file("new.go", "package acmeapi\n", "n1");
        let service = service();
        let id = service.add_local(text(clone.path())).unwrap().id;
        let origin_develop = clone.git(&["rev-parse", "origin/develop"]);

        let detail = service.detail(&id).unwrap();

        assert_eq!(detail.bases, vec!["master", "develop"]);
        // A pull request into develop would carry d3 too: it is not on
        // origin.
        assert_eq!(
            rows([find(&detail, "feature/new")]),
            vec![row("feature/new", "develop", BaseSource::History, 2, 0)]
        );
        let base = find(&detail, "feature/new").base.as_ref().unwrap();
        assert_eq!(
            (base.ref_name.as_str(), base.sha.as_str()),
            ("origin/develop", origin_develop.as_str())
        );
        let summary = service.compare(&id, "feature/new", "develop").unwrap();
        assert_eq!(summary.merge_base_sha, origin_develop);
        assert_eq!((summary.commits, summary.files_changed), (2, 2));
        let change_set = service
            .changed_files(&id, branch_target("feature/new", "develop"))
            .unwrap();
        assert_eq!(change_set.old_sha, origin_develop);
        assert_eq!(paths(&change_set), vec!["d3.go", "new.go"]);
    }

    /// A clone whose local develop was made long ago, after which origin's
    /// develop gained `stale_files` files and `feature/ABC-123` was pushed
    /// on top of it, changing two files; the clone then fetched, so it has
    /// the feature only as `origin/feature/ABC-123`.
    fn stale_local_develop(stale_files: usize) -> (TestRepo, TestRepo) {
        let source = acme_api();
        let clone = TestRepo::clone_from(source.path());
        clone.git(&["branch", "--quiet", "develop", "origin/develop"]);
        source.git(&["checkout", "--quiet", "develop"]);
        for n in 0..stale_files {
            source.commit_file(&format!("later/{n}.go"), "package later\n", "later on develop");
        }
        source.git(&["checkout", "--quiet", "-b", "feature/ABC-123"]);
        source.write("login.go", "package acmeapi\n\n// changed\n");
        source.commit_file("auth.go", "package acmeapi\n", "the change");
        source.git(&["checkout", "--quiet", "master"]);
        clone.git(&["fetch", "--quiet", "origin"]);
        (source, clone)
    }

    #[test]
    fn a_stale_local_base_is_passed_over_for_origins() {
        let (_source, clone) = stale_local_develop(22);
        let service = service();
        let id = service.add_local(text(clone.path())).unwrap().id;
        let feature = "feature/ABC-123";
        let origin_develop = clone.git(&["rev-parse", "origin/develop"]);
        // What the review showed before: everything origin's develop gained
        // since the local one was made.
        let stale_fork = clone.git(&["merge-base", "develop", "origin/feature/ABC-123"]);
        let stale_diff = clone.git(&[
            "diff",
            "--name-only",
            &stale_fork,
            "origin/feature/ABC-123",
        ]);
        assert_eq!(stale_diff.lines().count(), 24);

        let change_set = service
            .changed_files(&id, branch_target(feature, "develop"))
            .unwrap();

        assert_eq!(
            change_set.base,
            Some(ReviewBase {
                name: "develop".to_string(),
                kind: ReviewBaseKind::RemoteTracking,
                ref_name: Some("origin/develop".to_string()),
                sha: origin_develop.clone(),
            })
        );
        assert_eq!(change_set.old_sha, origin_develop);
        assert_eq!(paths(&change_set), vec!["auth.go", "login.go"]);
        // The branch row counts the same, from the same base.
        let summary = service.compare(&id, feature, "develop").unwrap();
        assert_eq!((summary.commits, summary.files_changed), (1, 2));
        assert_eq!(summary.merge_base_sha, change_set.old_sha);
        let detail = service.detail(&id).unwrap();
        let listed = find(&detail, feature);
        assert_eq!(listed.location, BranchLocation::RemoteOnly);
        assert_eq!((listed.ahead, listed.behind), (1, 0));
        let base = listed.base.as_ref().unwrap();
        assert_eq!(
            (base.name.as_str(), base.ref_name.as_str(), base.sha.as_str()),
            ("develop", "origin/develop", origin_develop.as_str())
        );
    }

    #[test]
    fn a_pull_requests_destination_commit_is_the_base_when_it_is_here() {
        let source = acme_api();
        let clone = TestRepo::clone_from(source.path());
        let service = service();
        let id = service.add_local(text(clone.path())).unwrap().id;
        let feature = "feature/ABC-178-login";
        // The host's destination commit: d1, one before origin's develop.
        let d1 = clone.git(&["rev-parse", "origin/develop~1"]);

        let change_set = service
            .changed_files_against(&id, branch_target(feature, "develop"), Some(&d1[..12]))
            .unwrap();

        assert_eq!(
            change_set.base,
            Some(ReviewBase {
                name: "develop".to_string(),
                kind: ReviewBaseKind::PullRequest,
                ref_name: None,
                sha: d1.clone(),
            })
        );
        assert_eq!(change_set.old_sha, d1);
        assert_eq!(paths(&change_set), vec!["d2.go", "login.go"]);
        // Without it, origin's develop.
        let without = service
            .changed_files(&id, branch_target(feature, "develop"))
            .unwrap();
        assert_eq!(paths(&without), vec!["login.go"]);
    }

    #[test]
    fn a_destination_commit_that_is_not_here_falls_back_to_origins_base() {
        let source = acme_api();
        let clone = TestRepo::clone_from(source.path());
        let service = service();
        let id = service.add_local(text(clone.path())).unwrap().id;
        let feature = "feature/ABC-178-login";
        let origin_develop = clone.git(&["rev-parse", "origin/develop"]);

        for missing in ["0123456789ab", "--all", "origin/master", ""] {
            let change_set = service
                .changed_files_against(&id, branch_target(feature, "develop"), Some(missing))
                .unwrap();
            let base = change_set.base.as_ref().unwrap();
            assert_eq!(
                (base.kind, base.sha.as_str()),
                (ReviewBaseKind::RemoteTracking, origin_develop.as_str()),
                "{missing:?}"
            );
            assert_eq!(paths(&change_set), vec!["login.go"]);
        }
        // The base must still be one of the project's.
        assert_eq!(
            code(service.changed_files_against(
                &id,
                branch_target(feature, "stale"),
                Some(&origin_develop)
            )),
            ErrorCode::UnknownBranch
        );
    }

    #[test]
    fn without_a_remote_the_local_base_is_used() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let develop = repo.git(&["rev-parse", "develop"]);

        let change_set = service
            .changed_files(&id, branch_target("feature/ABC-178-login", "develop"))
            .unwrap();

        assert_eq!(
            change_set.base,
            Some(ReviewBase {
                name: "develop".to_string(),
                kind: ReviewBaseKind::Local,
                ref_name: Some("develop".to_string()),
                sha: develop.clone(),
            })
        );
        assert_eq!(change_set.old_sha, develop);
        assert_eq!(paths(&change_set), vec!["login.go"]);
        let detail = service.detail(&id).unwrap();
        let base = find(&detail, "feature/ABC-178-login").base.as_ref().unwrap();
        assert_eq!(base.ref_name, "develop");
        let working_tree = service
            .changed_files(&id, ReviewTarget::WorkingTree)
            .unwrap();
        assert_eq!(working_tree.base, None);
    }

    #[test]
    fn compare_counts_from_the_merge_base() {
        let repo = TestRepo::init();
        repo.write("a.txt", "1\n2\n3\n");
        repo.write("old name.txt", "line\n".repeat(10));
        repo.commit_all("initial commit");
        let fork_point = repo.git(&["rev-parse", "HEAD"]);

        repo.git(&["checkout", "--quiet", "-b", "feature/review-me"]);
        repo.write("a.txt", "1\n2 changed\n3\n4\n");
        repo.write("dir with space/new file.txt", "x\ny\nz\n");
        repo.commit_all("change and add");
        repo.write("image.bin", [0u8, 159, 146, 150, 0, 1, 2]);
        repo.git(&["mv", "old name.txt", "new name.txt"]);
        repo.commit_all("binary and rename");
        let head = repo.git(&["rev-parse", "HEAD"]);

        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit_file("main only.txt", "m\nm\n", "main moves on");

        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let summary = service.compare(&id, "feature/review-me", "main").unwrap();
        assert_eq!(
            summary,
            CompareSummary {
                branch: "feature/review-me".to_string(),
                base: "main".to_string(),
                head_sha: head,
                merge_base_sha: fork_point,
                commits: 2,
                files_changed: 4,
                additions: 5,
                deletions: 1,
            }
        );

        let same = service.compare(&id, "main", "main").unwrap();
        assert_eq!(
            (
                same.commits,
                same.files_changed,
                same.additions,
                same.deletions
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(same.head_sha, same.merge_base_sha);
    }

    #[test]
    fn compare_reports_unknown_branches_and_never_treats_input_as_options() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;
        let marker = repo.parent().join("written-by-git");
        let output_option = format!("--output={}", text(&marker));
        let feature = "feature/ABC-178-login";

        for bad in [
            "missing",
            "",
            "master~1",
            "HEAD",
            "--all",
            "origin/master",
            output_option.as_str(),
        ] {
            assert_eq!(
                code(service.compare(&id, bad, "master")),
                ErrorCode::UnknownBranch,
                "branch {bad:?}"
            );
            assert_eq!(
                code(service.compare(&id, feature, bad)),
                ErrorCode::UnknownBranch,
                "base {bad:?}"
            );
        }
        // An existing branch that is not an integration branch is not a base.
        assert_eq!(
            code(service.compare(&id, feature, "stale")),
            ErrorCode::UnknownBranch
        );
        assert!(!marker.exists());
    }

    #[test]
    fn compare_of_unrelated_histories_is_no_common_history() {
        let repo = go_repo();
        repo.git(&["checkout", "--quiet", "--orphan", "island"]);
        repo.commit_file("island.txt", "alone\n", "unrelated root");
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        assert_eq!(
            code(service.compare(&id, "island", "main")),
            ErrorCode::NoCommonHistory
        );
        // The branch is still listed, with all its commits ahead.
        let detail = service.detail(&id).unwrap();
        assert_eq!(
            rows(&detail.branches),
            vec![row("island", "main", BaseSource::History, 1, 1)]
        );
    }

    #[test]
    fn working_tree_counts_tracked_untracked_and_binary_files_but_not_ignored_ones() {
        let repo = TestRepo::init();
        repo.write("a.txt", "1\n2\n3\n");
        repo.write("gone.txt", "x\ny\n");
        repo.write("image.bin", [0u8, 1, 2, 3]);
        repo.write(".gitignore", "*.log\n");
        repo.commit_all("initial commit");
        let head = repo.git(&["rev-parse", "HEAD"]);

        repo.write("a.txt", "1\nchanged\n3\n4\n"); // +2 -1
        fs::remove_file(repo.path().join("gone.txt")).unwrap(); // -2
        repo.write("image.bin", [0u8, 9, 9]); // binary, 0 lines
        repo.write("staged.txt", "s\n"); // +1
        repo.git(&["add", "staged.txt"]);
        repo.write("new dir/untracked.txt", "u1\nu2\nu3"); // +3
        repo.write("blob.dat", [7u8, 0, 7, b'\n']); // binary, 0 lines
        repo.write("debug.log", "ignored\nignored\n");
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let summary = service.compare_working_tree(&id).unwrap();

        assert_eq!(
            summary,
            WorkingTreeSummary {
                head_sha: head,
                files_changed: 6,
                additions: 6,
                deletions: 3,
            }
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            repo.git(&["status", "--porcelain", "--untracked-files=all"])
                .lines()
                .filter(|line| line.starts_with("??"))
                .count(),
            2
        );
    }

    #[test]
    fn working_tree_of_a_clean_or_unborn_repository() {
        let clean = go_repo();
        let unborn = TestRepo::init();
        unborn.write("first.txt", "a\nb\n");
        let service = service();
        let clean_id = service.add_local(text(clean.path())).unwrap().id;
        let unborn_id = service.add_local(text(unborn.path())).unwrap().id;

        let summary = service.compare_working_tree(&clean_id).unwrap();
        assert_eq!(
            (summary.files_changed, summary.additions, summary.deletions),
            (0, 0, 0)
        );
        assert_eq!(summary.head_sha, clean.git(&["rev-parse", "HEAD"]));

        let summary = service.compare_working_tree(&unborn_id).unwrap();
        assert_eq!(
            summary,
            WorkingTreeSummary {
                head_sha: String::new(),
                files_changed: 1,
                additions: 2,
                deletions: 0,
            }
        );
    }

    /// A bare `origin`, a local clone `work` to review in, and a second clone
    /// `agent` that pushes from elsewhere.
    struct Remote {
        /// Holds the temporary folder `origin` lives in.
        _source: TestRepo,
        origin: PathBuf,
        work: TestRepo,
        agent: TestRepo,
    }

    fn remote_setup() -> Remote {
        let source = acme_api();
        source.git(&["branch", "old", "master"]);
        let origin = source.parent().join("origin.git");
        run_git(
            source.parent(),
            &[
                "clone",
                "--quiet",
                "--bare",
                text(source.path()),
                text(&origin),
            ],
        );
        let work = TestRepo::clone_from(&origin);
        work.git(&["checkout", "--quiet", "feature/ABC-178-login"]);
        let agent = TestRepo::clone_from(&origin);
        agent.git(&["checkout", "--quiet", "feature/ABC-178-login"]);
        Remote {
            _source: source,
            origin,
            work,
            agent,
        }
    }

    #[test]
    fn fetch_brings_in_remote_work_and_reports_what_moved() {
        let remote = remote_setup();
        let (work, agent) = (&remote.work, &remote.agent);
        agent.commit_file("agent1.go", "package acmeapi\n", "agent 1");
        agent.commit_file("agent2.go", "package acmeapi\n", "agent 2");
        agent.git(&["push", "--quiet", "origin", "feature/ABC-178-login"]);
        agent.git(&[
            "checkout",
            "--quiet",
            "-b",
            "feature/ABC-200",
            "origin/develop",
        ]);
        agent.commit_file("new.go", "package acmeapi\n", "new work");
        agent.git(&["push", "--quiet", "origin", "feature/ABC-200"]);
        agent.git(&["push", "--quiet", "origin", "--delete", "old"]);
        agent.git(&["tag", "v1.0"]);
        agent.git(&["push", "--quiet", "origin", "v1.0"]);
        let service = service();
        let id = service.add_local(text(work.path())).unwrap().id;
        let before = service.detail(&id).unwrap();
        assert!(before.branches.iter().any(|b| b.name == "old"));
        assert_eq!(
            find(&before, "feature/ABC-178-login").upstream,
            Some(Upstream {
                name: "origin/feature/ABC-178-login".to_string(),
                ahead: 0,
                behind: 0
            })
        );

        let result = service.fetch(&id).unwrap();

        // feature/ABC-178-login moved, feature/ABC-200 appeared, old went away.
        assert_eq!(result.branches_updated, 3);
        assert!(OffsetDateTime::parse(&result.fetched_at, &Rfc3339).is_ok());
        let after = service.detail(&id).unwrap();
        assert_eq!(
            after.project.last_fetched_at.as_deref(),
            Some(result.fetched_at.as_str())
        );
        assert!(!after.branches.iter().any(|b| b.name == "old"));
        let pushed = find(&after, "feature/ABC-200");
        assert_eq!(pushed.location, BranchLocation::RemoteOnly);
        assert_eq!(
            rows([pushed]),
            vec![row(
                "feature/ABC-200",
                "develop",
                BaseSource::History,
                1,
                0
            )]
        );
        let local = find(&after, "feature/ABC-178-login");
        assert_eq!(
            local.upstream,
            Some(Upstream {
                name: "origin/feature/ABC-178-login".to_string(),
                ahead: 0,
                behind: 2
            })
        );
        // The local branch itself did not move.
        assert_eq!((local.ahead, local.behind), (1, 0));
        assert_eq!(work.git(&["tag", "--list"]), "");

        // Nothing new: nothing updated.
        assert_eq!(service.fetch(&id).unwrap().branches_updated, 0);
    }

    #[test]
    fn fetch_never_touches_local_branches_head_index_or_working_tree() {
        let remote = remote_setup();
        let (work, agent) = (&remote.work, &remote.agent);
        // Local work that diverges from what the agent pushes.
        work.commit_file("local.go", "package acmeapi\n", "local, not pushed");
        work.write("login.go", "package acmeapi // edited\n");
        work.write("scratch.txt", "untracked\n");
        work.write("staged.txt", "staged\n");
        work.git(&["add", "staged.txt"]);
        agent.commit_file("agent.go", "package acmeapi\n", "agent");
        agent.git(&["push", "--quiet", "origin", "feature/ABC-178-login"]);
        agent.git(&["push", "--quiet", "origin", "HEAD:develop"]);
        // A mirror-style refspec in the config must not make fetch write
        // into local branches.
        work.git(&[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/heads/mirror/*",
        ]);
        let service = service();
        let id = service.add_local(text(work.path())).unwrap().id;
        let before = local_state(work);

        let result = service.fetch(&id).unwrap();

        assert_eq!(result.branches_updated, 2);
        assert_eq!(local_state(work), before);
        assert_eq!(
            work.git(&["rev-parse", "origin/feature/ABC-178-login"]),
            agent.git(&["rev-parse", "HEAD"])
        );
    }

    /// Everything a fetch must leave alone: local branches, HEAD, tags, the
    /// index and every file outside `.git`.
    fn local_state(repo: &TestRepo) -> (String, String, String, Vec<u8>, Vec<FileStamp>) {
        let branches = repo.git(&[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ]);
        let head = repo.git(&["symbolic-ref", "HEAD"]);
        let status = repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let files = snapshot(repo.path())
            .into_iter()
            .filter(|(path, ..)| !path.starts_with(repo.path().join(".git")))
            .collect();
        (branches, head, status, index, files)
    }

    #[test]
    fn fetch_without_a_remote_is_no_remote() {
        let repo = acme_api();
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        assert_eq!(code(service.fetch(&id)), ErrorCode::NoRemote);
        assert_eq!(service.detail(&id).unwrap().project.last_fetched_at, None);
    }

    #[test]
    fn a_failed_fetch_keeps_the_last_fetch_time() {
        let remote = remote_setup();
        let service = service();
        let id = service.add_local(text(remote.work.path())).unwrap().id;
        let first = service.fetch(&id).unwrap();
        fs::remove_dir_all(&remote.origin).unwrap();

        assert_eq!(code(service.fetch(&id)), ErrorCode::FetchFailed);
        assert_eq!(
            service.detail(&id).unwrap().project.last_fetched_at,
            Some(first.fetched_at)
        );
    }

    #[test]
    fn changed_refs_are_counted_once_each() {
        let refs = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(name, sha)| (name.to_string(), sha.to_string()))
                .collect()
        };
        let before = refs(&[("a", "1"), ("b", "2"), ("gone", "3")]);
        let after = refs(&[("a", "1"), ("b", "9"), ("new", "4")]);
        assert_eq!(count_changed_refs(&before, &after), 3);
        assert_eq!(count_changed_refs(&before, &before), 0);
    }

    #[test]
    fn nothing_is_written_to_the_registered_repository() {
        let repo = acme_api();
        repo.write("dirty.txt", "x\n");
        let before = snapshot(repo.path());
        let service = service();

        let id = service.add_local(text(repo.path())).unwrap().id;
        service.list().unwrap();
        service.detail(&id).unwrap();
        service
            .compare(&id, "feature/ABC-178-login", "develop")
            .unwrap();
        service.compare_working_tree(&id).unwrap();
        service
            .set_branch_base(&id, "feature/ABC-178-login", Some("master"))
            .unwrap();

        let after = snapshot(repo.path());
        let touched: Vec<_> = after
            .iter()
            .filter(|file| !before.contains(file))
            .chain(before.iter().filter(|file| !after.contains(file)))
            .collect();
        assert!(
            touched.is_empty(),
            "files created, changed or removed: {touched:?}"
        );
    }

    type FileStamp = (PathBuf, u64, std::time::SystemTime);

    /// Every file under `root` with its size and modification time.
    fn snapshot(root: &Path) -> Vec<FileStamp> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else {
                    files.push((entry.path(), metadata.len(), metadata.modified().unwrap()));
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn responses_use_the_field_names_of_the_contract() {
        let remote = remote_setup();
        let service = service();
        let id = service.add_local(text(remote.work.path())).unwrap().id;
        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort_unstable();
            keys
        };

        let detail = serde_json::to_value(service.detail(&id).unwrap()).unwrap();
        assert_eq!(
            keys(&detail),
            vec!["bases", "branches", "project", "workingTree", "worktrees"]
        );
        assert_eq!(
            keys(&detail["worktrees"][0]),
            vec!["branch", "changedFiles", "createdAt", "folder"]
        );
        assert_eq!(
            keys(&detail["worktrees"][0]["folder"]),
            vec!["displayPath", "kind", "path", "relativePath"]
        );
        assert_eq!(detail["worktrees"][0]["folder"]["kind"], "project");
        let location =
            serde_json::to_value(service.terminal_location(&id, None).unwrap()).unwrap();
        assert_eq!(keys(&location), vec!["branch", "folder", "plan"]);
        assert_eq!(
            keys(&detail["project"]),
            vec![
                "addedAt",
                "analysisSupport",
                "available",
                "branchesAhead",
                "displayPath",
                "hasRemote",
                "id",
                "integrationBranches",
                "languages",
                "lastFetchedAt",
                "lastModifiedAt",
                "name",
                "path"
            ]
        );
        assert_eq!(detail["project"]["analysisSupport"], "full");
        assert_eq!(detail["project"]["lastFetchedAt"], serde_json::Value::Null);
        let local = detail["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "feature/ABC-178-login")
            .unwrap();
        assert_eq!(
            keys(local),
            vec![
                "ahead",
                "base",
                "behind",
                "card",
                "head",
                "isCurrent",
                "location",
                "name",
                "upstream"
            ]
        );
        assert_eq!(local["location"], "local");
        assert_eq!(detail["project"]["hasRemote"], true);
        assert_eq!(
            local["base"],
            serde_json::json!({
                "name": "develop",
                "source": "history",
                "refName": "origin/develop",
                "sha": remote.work.git(&["rev-parse", "origin/develop"])
            })
        );
        assert_eq!(keys(&local["upstream"]), vec!["ahead", "behind", "name"]);
        assert_eq!(keys(&local["head"]), vec!["authoredAt", "sha", "subject"]);
        let remote_only = detail["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "old")
            .unwrap();
        assert_eq!(remote_only["location"], "remote_only");
        assert_eq!(remote_only["upstream"], serde_json::Value::Null);
        assert_eq!(remote_only["card"], serde_json::Value::Null);
        assert_eq!(
            detail["workingTree"],
            serde_json::json!({ "dirty": false, "filesChanged": 0 })
        );

        let summary = serde_json::to_value(
            service
                .compare(&id, "feature/ABC-178-login", "develop")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            keys(&summary),
            vec![
                "additions",
                "base",
                "branch",
                "commits",
                "deletions",
                "filesChanged",
                "headSha",
                "mergeBaseSha"
            ]
        );
        let working = serde_json::to_value(service.compare_working_tree(&id).unwrap()).unwrap();
        assert_eq!(
            keys(&working),
            vec!["additions", "deletions", "filesChanged", "headSha"]
        );
        let fetched = serde_json::to_value(service.fetch(&id).unwrap()).unwrap();
        assert_eq!(keys(&fetched), vec!["branchesUpdated", "fetchedAt"]);
    }

    /// Run with `cargo test fifty_branches -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement"]
    fn fifty_branches_timing() {
        let repo = TestRepo::init_with_branch("master");
        for index in 0..20 {
            repo.commit_file("master.txt", format!("{index}\n"), "master");
        }
        repo.git(&["checkout", "--quiet", "-b", "develop"]);
        for index in 0..30 {
            repo.commit_file("develop.txt", format!("{index}\n"), "develop");
        }
        repo.git(&["branch", "release/1.0"]);
        repo.commit_file("develop.txt", "release 1.1\n", "develop");
        repo.git(&["branch", "release/1.1"]);
        for index in 0..25 {
            repo.git(&[
                "checkout",
                "--quiet",
                "-b",
                &format!("feature/ABC-{index}"),
                "develop",
            ]);
            repo.commit_file(&format!("f{index}.txt"), "1\n", "feature 1");
            repo.commit_file(&format!("f{index}.txt"), "1\n2\n", "feature 2");
        }
        for index in 0..20 {
            repo.git(&[
                "checkout",
                "--quiet",
                "-b",
                &format!("hotfix/OPS-{index}"),
                "master",
            ]);
            repo.commit_file(&format!("h{index}.txt"), "1\n", "hotfix");
        }
        repo.git(&["checkout", "--quiet", "master"]);
        let origin = repo.parent().join("origin.git");
        run_git(
            repo.parent(),
            &[
                "clone",
                "--quiet",
                "--bare",
                text(repo.path()),
                text(&origin),
            ],
        );
        run_git(repo.path(), &["remote", "add", "origin", text(&origin)]);
        run_git(repo.path(), &["fetch", "--quiet", "origin"]);
        let service = service();
        let id = service.add_local(text(repo.path())).unwrap().id;

        let mut timings = Vec::new();
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let detail = service.detail(&id).unwrap();
            timings.push(started.elapsed());
            assert_eq!(detail.branches.len(), 45);
            assert_eq!(detail.bases.len(), 4);
            assert_eq!(detail.project.branches_ahead, 45);
        }
        let started = std::time::Instant::now();
        service.list().unwrap();
        let list = started.elapsed();
        eprintln!("detail with 45 branches to review, 4 bases, 49 local + 49 origin refs: {timings:?}; list: {list:?}");
    }
}
