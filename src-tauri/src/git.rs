//! Access to git through the `git` command line, so cloning and fetching use
//! the SSH keys and configuration the user already has.
//!
//! Every call passes an argv list (never a shell string). Only these commands
//! write: `clone_repository`, into a new destination; `fetch`, which updates
//! remote-tracking branches and nothing else; and `add_worktree` and
//! `remove_worktree`, which add or remove a worktree at a path the caller
//! chose (`worktrees` keeps it under the project's `.difflens/worktrees/`),
//! creating a local branch only for `add_tracking_worktree`. None of them
//! checks anything out in the project folder. Values that come from the
//! user never reach git as options: branch names are looked up in the list of
//! refs git reported, and only the resulting commit ids are passed back;
//! commit ids are checked to be object ids; file paths are literal pathspecs
//! after `--`; the clone URL and destination follow `--`.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, core_text, escaped, AppError, AppResult, CoreText, Reason};
use crate::model::{Commit, Upstream, WorkingTree};
use crate::process::{start_process_group, stop_process_group};

/// Variables that would point git at a repository other than the one passed
/// with `-C`.
const SCRUBBED_ENVIRONMENT: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

pub const LOCAL_BRANCHES: &str = "refs/heads/";
pub const ORIGIN_BRANCHES: &str = "refs/remotes/origin/";
const REMOTE_BRANCHES: &str = "refs/remotes/";

/// One line per ref, fields separated by NUL. The subject comes last because
/// it is the only free-text field.
const BRANCH_FORMAT: &str = "--format=%(refname)%00%(symref)%00%(objectname)%00%(HEAD)%00\
     %(authordate:iso-strict)%00%(upstream:short)%00%(upstream:track,nobracket)%00\
     %(contents:subject)";
const BRANCH_FIELDS: usize = 8;

/// How much of git's error output is kept in a message shown to the user.
const MAX_ERROR_DETAIL_CHARS: usize = 600;

/// Longest a fetch may run before it is stopped, so a stuck network or a
/// prompt nobody can answer does not hold a worker forever.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Time allowed to collect the error output once git has exited.
const OUTPUT_GRACE: Duration = Duration::from_secs(5);

const ADD_WORKTREE: (Reason, &str) = (Reason::GitAddWorktreeFailed, "create the worktree");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `refs/heads/<name>`
    Local,
    /// `refs/remotes/origin/<name>`
    Origin,
}

/// A local branch or a branch of origin, as `for-each-ref` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    pub kind: RefKind,
    /// Name without `refs/heads/` or `refs/remotes/origin/`.
    pub name: String,
    pub full_ref: String,
    pub is_current: bool,
    pub head: Commit,
    /// For a local branch whose upstream still exists.
    pub upstream: Option<Upstream>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStat {
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

/// Mode of a gitlink, the entry a submodule leaves in its parent's tree.
pub const GITLINK_MODE: u32 = 0o160000;

/// Status letter of one `git diff --raw` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Copied,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
    /// A path with a merge conflict, in the working tree.
    Unmerged,
}

/// One side of a changed file as the tree records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    /// All zeros on the working tree side: the file is read from disk.
    pub oid: String,
}

/// One file of `git diff --raw --numstat`. Paths are the bytes git wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub kind: ChangeKind,
    /// Path on the new side; the old one for a deletion.
    pub path: Vec<u8>,
    /// Source of a rename or a copy.
    pub source: Option<Vec<u8>>,
    /// `None` when the file does not exist on that side.
    pub old: Option<TreeEntry>,
    pub new: Option<TreeEntry>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

/// An object read with `read_blobs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blob {
    Missing,
    /// Larger than the limit asked for; only its size was read.
    TooLarge(u64),
    Content(Vec<u8>),
}

/// One worktree of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// The local branch checked out there, without `refs/heads/`; `None`
    /// for a detached HEAD or a bare repository.
    pub branch: Option<String>,
    pub bare: bool,
    /// Its folder is gone; git keeps its record until it is pruned.
    pub prunable: bool,
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}

#[derive(Debug, Clone)]
pub struct Git {
    program: OsString,
    environment: Vec<(OsString, OsString)>,
    fetch_timeout: Duration,
}

impl Git {
    pub fn new() -> Self {
        Self {
            program: OsString::from("git"),
            environment: Vec::new(),
            fetch_timeout: FETCH_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn with_program(mut self, program: &str) -> Self {
        self.program = OsString::from(program);
        self
    }

    #[cfg(test)]
    pub fn with_environment(mut self, key: &str, value: &str) -> Self {
        self.environment
            .push((OsString::from(key), OsString::from(value)));
        self
    }

    #[cfg(test)]
    pub fn with_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// Root of the working tree that contains `path`, or `None` when `path`
    /// is not inside a git working tree.
    pub fn toplevel(&self, path: &Path) -> AppResult<Option<PathBuf>> {
        let output = self.run(Some(path), ["rev-parse", "--show-toplevel"])?;
        if !output.success {
            return Ok(None);
        }
        let Ok(text) = String::from_utf8(output.stdout) else {
            return Ok(None);
        };
        let root = text.trim_end_matches(['\n', '\r']);
        Ok((!root.is_empty()).then(|| PathBuf::from(root)))
    }

    /// Local branches and branches of origin, most recently committed first,
    /// in one call. Upstream counts come from the same call.
    pub fn branch_refs(&self, repo: &Path) -> AppResult<Vec<BranchRef>> {
        let stdout = self.run_checked(
            repo,
            [
                "for-each-ref",
                "--sort=-committerdate",
                BRANCH_FORMAT,
                LOCAL_BRANCHES,
                ORIGIN_BRANCHES,
            ],
            (Reason::GitListBranchesFailed, "list branches"),
        )?;
        let mut refs = Vec::new();
        for line in String::from_utf8_lossy(&stdout).split('\n') {
            if let Some(branch) = parse_branch_line(line)? {
                refs.push(branch);
            }
        }
        Ok(refs)
    }

    /// Committer date (RFC 3339) of the newest commit on a local branch or a
    /// branch of origin, or `None` before the first commit.
    pub fn last_commit_date(&self, repo: &Path) -> AppResult<Option<String>> {
        let stdout = self.run_checked(
            repo,
            [
                "for-each-ref",
                "--sort=-committerdate",
                "--count=1",
                "--format=%(committerdate:iso-strict)",
                LOCAL_BRANCHES,
                ORIGIN_BRANCHES,
            ],
            (Reason::GitListBranchesFailed, "list branches"),
        )?;
        let text = String::from_utf8_lossy(&stdout);
        let date = text.trim();
        Ok((!date.is_empty()).then(|| date.to_string()))
    }

    /// Commits ahead of and behind `base` for every local branch and every
    /// branch of origin, keyed by full ref name. One process for all refs:
    /// git walks the history once (`%(ahead-behind)`, git 2.41 or newer).
    pub fn ahead_behind_all(
        &self,
        repo: &Path,
        base_commit: &str,
    ) -> AppResult<HashMap<String, AheadBehind>> {
        if !is_object_id(base_commit) {
            return Err(app_error!(
                Internal,
                NotACommitId,
                "\"{commit}\" is not a commit id.",
                commit = escaped(base_commit)
            ));
        }
        let format = format!("--format=%(refname)%00%(ahead-behind:{base_commit})");
        let output = self.run(
            Some(repo),
            [
                "for-each-ref",
                format.as_str(),
                LOCAL_BRANCHES,
                ORIGIN_BRANCHES,
            ],
        )?;
        if !output.success {
            if output.stderr.contains("unknown field name") {
                return Err(app_error!(
                    Internal,
                    GitTooOld,
                    "DiffLens needs git 2.41 or newer to compare branches with their base."
                ));
            }
            return Err(app_error!(
                Internal,
                GitCountCommitsFailed,
                "git could not count commits: {detail}",
                detail = git_detail(&output.stderr)
            ));
        }
        let mut distances = HashMap::new();
        for line in String::from_utf8_lossy(&output.stdout).split('\n') {
            if line.is_empty() {
                continue;
            }
            let (full_ref, counts) = line
                .split_once('\0')
                .ok_or_else(|| unexpected_output("for-each-ref ahead-behind"))?;
            let mut numbers = counts.split(' ').map(str::parse::<u32>);
            let (Some(Ok(ahead)), Some(Ok(behind)), None) =
                (numbers.next(), numbers.next(), numbers.next())
            else {
                return Err(unexpected_output("for-each-ref ahead-behind"));
            };
            distances.insert(full_ref.to_string(), AheadBehind { ahead, behind });
        }
        Ok(distances)
    }

    pub fn working_tree(&self, repo: &Path) -> AppResult<WorkingTree> {
        let stdout = self.run_checked(
            repo,
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            (
                Reason::GitWorkingTreeStatusFailed,
                "read the working tree status",
            ),
        )?;
        let files_changed = count_status_entries(&stdout);
        Ok(WorkingTree {
            dirty: files_changed > 0,
            files_changed,
        })
    }

    /// Commit checked out, or `None` before the first commit.
    pub fn head_commit(&self, repo: &Path) -> AppResult<Option<String>> {
        let output = self.run(
            Some(repo),
            ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        )?;
        Ok(first_line(&output).map(str::to_string))
    }

    /// Id of the empty tree in this repository's hash format. Nothing is
    /// written: without `-w` hash-object only computes the id.
    pub fn empty_tree(&self, repo: &Path) -> AppResult<String> {
        // stdin is /dev/null, so --stdin hashes an empty tree.
        let output = self.run(Some(repo), ["hash-object", "-t", "tree", "--stdin"])?;
        first_line(&output)
            .map(str::to_string)
            .ok_or_else(|| unexpected_output("hash-object"))
    }

    /// Best common ancestor of two commits, or `None` for unrelated histories.
    pub fn merge_base(&self, repo: &Path, left: &str, right: &str) -> AppResult<Option<String>> {
        let output = self.run(Some(repo), ["merge-base", left, right])?;
        Ok(first_line(&output).map(str::to_string))
    }

    /// Whether `ancestor` is `descendant` or one of its ancestors.
    pub fn is_ancestor(&self, repo: &Path, ancestor: &str, descendant: &str) -> AppResult<bool> {
        let output = self.run(
            Some(repo),
            ["merge-base", "--is-ancestor", ancestor, descendant],
        )?;
        // Exit status 1 means "not an ancestor"; anything else failed.
        match output.success {
            true => Ok(true),
            false if output.stderr.trim().is_empty() => Ok(false),
            false => Err(app_error!(
                Internal,
                GitCompareCommitsFailed,
                "git could not compare commits: {detail}",
                detail = git_detail(&output.stderr)
            )),
        }
    }

    /// The full id of the commit an abbreviated id names, as hosts such as
    /// Bitbucket abbreviate them; `None` when it names no commit here, or is
    /// not a hexadecimal id (so it can never be taken for an option or a
    /// ref name).
    pub fn resolve_commit(&self, repo: &Path, abbreviated: &str) -> AppResult<Option<String>> {
        let hex = abbreviated.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !hex || !(7..=64).contains(&abbreviated.len()) {
            return Ok(None);
        }
        let output = self.run(
            Some(repo),
            [
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{abbreviated}^{{commit}}"),
            ],
        )?;
        Ok(first_line(&output).map(str::to_string))
    }

    /// URL of the remote `name`, as git would use it (with `insteadOf`
    /// rewrites applied); `None` when there is no such remote.
    pub fn remote_url(&self, repo: &Path, name: &str) -> AppResult<Option<String>> {
        if !self.remotes(repo)?.iter().any(|remote| remote == name) {
            return Ok(None);
        }
        let stdout = self.run_checked(
            repo,
            ["remote", "get-url", "--", name],
            (Reason::GitReadRemoteFailed, "read a remote"),
        )?;
        Ok(String::from_utf8_lossy(&stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string))
    }

    /// Number of commits reachable from `to` and not from `from`.
    pub fn count_commits(&self, repo: &Path, from: &str, to: &str) -> AppResult<u32> {
        let range = format!("{from}..{to}");
        let stdout =
            self.run_checked(
                repo,
                ["rev-list", "--count", &range, "--"],
                (Reason::GitCountCommitsFailed, "count commits"),
            )?;
        String::from_utf8_lossy(&stdout)
            .trim()
            .parse()
            .map_err(|_| unexpected_output("rev-list --count"))
    }

    /// Files and lines changed from `from` to `to`, or to the working tree
    /// (staged and unstaged changes of tracked files) when `to` is `None`. A
    /// rename counts as one file; a binary file counts as changed with no
    /// lines.
    pub fn diff_stat(&self, repo: &Path, from: &str, to: Option<&str>) -> AppResult<DiffStat> {
        let mut arguments = vec![
            "diff",
            "--numstat",
            "-z",
            // Set explicitly so the numbers do not depend on the user's
            // diff.renames, and so no configured external program runs.
            "--find-renames",
            "--no-ext-diff",
            "--no-textconv",
            from,
        ];
        arguments.extend(to);
        arguments.push("--");
        let stdout = self.run_checked(
            repo,
            arguments,
            (Reason::GitCompareCommitsFailed, "compare commits"),
        )?;
        parse_numstat(&stdout)
    }

    /// Every changed file from `from` to `to`, or to the working tree when
    /// `to` is `None`, limited to `paths` (taken literally) when any is
    /// given. One process: the `--raw` records give status, modes and
    /// objects, the `--numstat` records that follow give the line counts.
    /// Renames are found with the same options as `diff_stat`, so the counts
    /// add up to its totals.
    pub fn diff_files(
        &self,
        repo: &Path,
        from: &str,
        to: Option<&str>,
        paths: &[&str],
    ) -> AppResult<Vec<FileChange>> {
        let arguments = raw_diff_arguments(from, to, &["--numstat"], paths)?;
        let stdout = self.run_checked(
            repo,
            arguments,
            (Reason::GitListChangedFilesFailed, "list changed files"),
        )?;
        parse_diff_files(&stdout)
    }

    /// The renames from `from` to `to` (or to the working tree), as
    /// `diff_files` finds them over the whole diff, without line counts.
    pub fn renames(&self, repo: &Path, from: &str, to: Option<&str>) -> AppResult<Vec<FileChange>> {
        let arguments = raw_diff_arguments(from, to, &["--diff-filter=R"], &[])?;
        let stdout = self.run_checked(
            repo,
            arguments,
            (Reason::GitListRenamedFilesFailed, "list renamed files"),
        )?;
        let mut fields = stdout.split(|byte| *byte == 0).peekable();
        let renames = parse_raw(&mut fields)?;
        if fields.any(|field| !field.is_empty()) {
            return Err(unexpected_output("diff --raw"));
        }
        Ok(renames)
    }

    /// Whether each of `candidates` is a commit of this repository. Anything
    /// that is not a full object id is not, without asking git.
    pub fn are_commits(&self, repo: &Path, candidates: &[&str]) -> AppResult<Vec<bool>> {
        let ids: Vec<&str> = candidates
            .iter()
            .copied()
            .filter(|candidate| is_object_id(candidate))
            .collect();
        let mut found = Vec::new();
        if !ids.is_empty() {
            let input: String = ids.iter().map(|id| format!("{id}^{{commit}}\n")).collect();
            let output = self.run_with_input(
                repo,
                ["cat-file", "--batch-check=%(objecttype)"],
                input.into_bytes(),
            )?;
            if !output.success {
                return Err(app_error!(
                    Internal,
                    GitLookUpCommitsFailed,
                    "git could not look up commits: {detail}",
                    detail = git_detail(&output.stderr)
                ));
            }
            // One line per query: the type, or the query followed by
            // "missing" when it does not name a commit.
            found = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line == "commit")
                .collect();
            if found.len() != ids.len() {
                return Err(unexpected_output("cat-file --batch-check"));
            }
        }
        let mut found = found.into_iter();
        Ok(candidates
            .iter()
            .map(|candidate| is_object_id(candidate) && found.next().unwrap_or(false))
            .collect())
    }

    /// Whether each of `directories` (relative, "/"-separated, "" for the
    /// root) is a directory of commit `commit`, in one git process. A path
    /// git could not be asked about (a line break in it) is not.
    pub fn are_directories(
        &self,
        repo: &Path,
        commit: &str,
        directories: &[&str],
    ) -> AppResult<Vec<bool>> {
        if !is_object_id(commit) {
            return Err(app_error!(
                Internal,
                NotACommitId,
                "\"{commit}\" is not a commit id.",
                commit = escaped(commit)
            ));
        }
        let asked: Vec<&str> = directories
            .iter()
            .copied()
            .filter(|directory| !directory.contains(['\n', '\r']))
            .collect();
        let mut found = Vec::new();
        if !asked.is_empty() {
            let input: String = asked
                .iter()
                .map(|directory| format!("{commit}:{directory}\n"))
                .collect();
            let output = self.run_with_input(
                repo,
                ["cat-file", "--batch-check=%(objecttype)"],
                input.into_bytes(),
            )?;
            if !output.success {
                return Err(app_error!(
                    Internal,
                    GitLookUpDirectoriesFailed,
                    "git could not look up directories: {detail}",
                    detail = git_detail(&output.stderr)
                ));
            }
            found = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line == "tree")
                .collect();
            if found.len() != asked.len() {
                return Err(unexpected_output("cat-file --batch-check"));
            }
        }
        let mut found = found.into_iter();
        Ok(directories
            .iter()
            .map(|directory| !directory.contains(['\n', '\r']) && found.next().unwrap_or(false))
            .collect())
    }

    /// Contents of the blobs `oids`, in order. A blob above `max_bytes` is
    /// not read, only measured. Two processes at most: one for the sizes, one
    /// for the contents.
    pub fn read_blobs(&self, repo: &Path, oids: &[&str], max_bytes: u64) -> AppResult<Vec<Blob>> {
        if let Some(bad) = oids.iter().find(|oid| !is_object_id(oid)) {
            return Err(app_error!(
                Internal,
                NotAnObjectId,
                "\"{id}\" is not an object id.",
                id = escaped(bad)
            ));
        }
        self.read_objects(repo, oids, max_bytes, false)
    }

    /// Contents of the files `paths` as commit `commit` has them, in order;
    /// a path the commit does not have (or that is not a file there) reads
    /// as missing. As `read_blobs`, a file above `max_bytes` is only measured.
    pub fn files_at(
        &self,
        repo: &Path,
        commit: &str,
        paths: &[&str],
        max_bytes: u64,
    ) -> AppResult<Vec<Blob>> {
        if !is_object_id(commit) {
            return Err(app_error!(
                Internal,
                NotACommitId,
                "\"{commit}\" is not a commit id.",
                commit = escaped(commit)
            ));
        }
        // A name of the batch is one line, so a path cannot hold a newline.
        let names: Vec<String> = paths
            .iter()
            .map(|path| {
                if path.is_empty() || path.contains(['\n', '\r']) {
                    String::new()
                } else {
                    format!("{commit}:{path}")
                }
            })
            .collect();
        let wanted: Vec<&str> = names
            .iter()
            .filter(|name| !name.is_empty())
            .map(String::as_str)
            .collect();
        let mut read = self.read_objects(repo, &wanted, max_bytes, true)?.into_iter();
        names
            .iter()
            .map(|name| {
                if name.is_empty() {
                    Ok(Blob::Missing)
                } else {
                    read.next().ok_or_else(|| unexpected_output("cat-file --batch"))
                }
            })
            .collect()
    }

    /// The messages of the commits reachable from `to` and not from `from`,
    /// newest first, at most `max` of them, each trimmed.
    pub fn commit_messages(
        &self,
        repo: &Path,
        from: &str,
        to: &str,
        max: usize,
    ) -> AppResult<Vec<String>> {
        for commit in [from, to] {
            if !is_object_id(commit) {
                return Err(app_error!(
                    Internal,
                    NotACommitId,
                    "\"{commit}\" is not a commit id.",
                    commit = escaped(commit)
                ));
            }
        }
        let range = format!("{from}..{to}");
        let count = format!("--max-count={max}");
        let stdout = self.run_checked(
            repo,
            ["log", "--no-color", "--format=%B%x00", &count, &range, "--"],
            (Reason::GitReadCommitMessagesFailed, "read commit messages"),
        )?;
        Ok(String::from_utf8_lossy(&stdout)
            .split('\0')
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Objects by name (an object id, or `commit:path` when `by_path`, where
    /// what is not a file reads as missing), as `read_blobs` reads them.
    fn read_objects(
        &self,
        repo: &Path,
        oids: &[&str],
        max_bytes: u64,
        by_path: bool,
    ) -> AppResult<Vec<Blob>> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let queries = |ids: &[&str]| -> Vec<u8> {
            ids.iter()
                .flat_map(|id| format!("{id}\n").into_bytes())
                .collect()
        };
        let output = self.run_with_input(
            repo,
            ["cat-file", "--batch-check=%(objecttype) %(objectsize)"],
            queries(oids),
        )?;
        if !output.success {
            return Err(app_error!(
                Internal,
                GitReadSizesFailed,
                "git could not read file sizes: {detail}",
                detail = git_detail(&output.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() != oids.len() {
            return Err(unexpected_output("cat-file --batch-check"));
        }
        // Small blobs hold an empty placeholder until their content is read.
        let mut blobs = Vec::with_capacity(oids.len());
        let mut wanted = Vec::new();
        for (oid, line) in oids.iter().zip(lines) {
            if line.ends_with(" missing") {
                blobs.push(Blob::Missing);
                continue;
            }
            // A named path can be a folder or a submodule there.
            if by_path && matches!(line.split_once(' '), Some(("tree" | "commit", _))) {
                blobs.push(Blob::Missing);
                continue;
            }
            let size = match line.split_once(' ') {
                Some(("blob", size)) => size.parse::<u64>().ok(),
                _ => None,
            }
            .ok_or_else(|| unexpected_output("cat-file --batch-check"))?;
            if size > max_bytes {
                blobs.push(Blob::TooLarge(size));
            } else {
                wanted.push(*oid);
                blobs.push(Blob::Content(Vec::new()));
            }
        }
        if wanted.is_empty() {
            return Ok(blobs);
        }

        let output = self.run_with_input(repo, ["cat-file", "--batch"], queries(&wanted))?;
        if !output.success {
            return Err(app_error!(
                Internal,
                GitReadFilesFailed,
                "git could not read files: {detail}",
                detail = git_detail(&output.stderr)
            ));
        }
        let mut contents = parse_batch(&output.stdout)?.into_iter();
        for blob in blobs.iter_mut() {
            if matches!(blob, Blob::Content(_)) {
                *blob = match contents.next() {
                    Some(Some(content)) => Blob::Content(content),
                    Some(None) => Blob::Missing,
                    None => return Err(unexpected_output("cat-file --batch")),
                };
            }
        }
        Ok(blobs)
    }

    /// Files git does not track and does not ignore, relative to the root,
    /// limited to `paths` (taken literally) when any is given. An untracked
    /// nested repository is one entry ending in `/`.
    pub fn untracked_files(&self, repo: &Path, paths: &[&str]) -> AppResult<Vec<PathBuf>> {
        let mut arguments = vec![
            "ls-files".to_string(),
            "--others".to_string(),
            "--exclude-standard".to_string(),
            "-z".to_string(),
            "--".to_string(),
        ];
        arguments.extend(literal_pathspecs(paths));
        let stdout = self.run_checked(
            repo,
            arguments,
            (Reason::GitListUntrackedFilesFailed, "list untracked files"),
        )?;
        Ok(stdout
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(path_from_bytes)
            .collect())
    }

    /// Tracked files and untracked files that are not ignored, relative to
    /// the root, each once. Tracked files deleted from disk are still listed.
    pub fn listed_files(&self, repo: &Path) -> AppResult<Vec<PathBuf>> {
        let stdout = self.run_checked(
            repo,
            [
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "--deduplicate",
                "-z",
            ],
            (
                Reason::GitListWorkingTreeFilesFailed,
                "list the files of the working tree",
            ),
        )?;
        Ok(stdout
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(path_from_bytes)
            .collect())
    }

    /// Patch from `from` to `to`, or to the working tree (tracked files),
    /// without context lines, so every hunk covers only changed lines. Paths
    /// are prefixed with `a/` and `b/` whatever the user's configuration.
    pub fn diff_patch(&self, repo: &Path, from: &str, to: Option<&str>) -> AppResult<Vec<u8>> {
        for commit in std::iter::once(from).chain(to) {
            if !is_object_id(commit) {
                return Err(app_error!(
                    Internal,
                    NotAnObjectId,
                    "\"{id}\" is not an object id.",
                    id = escaped(commit)
                ));
            }
        }
        let mut arguments = vec![
            "-c",
            "core.quotePath=false",
            "diff",
            "-U0",
            "--no-color",
            // See diff_stat.
            "--find-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            from,
        ];
        arguments.extend(to);
        arguments.push("--");
        self.run_checked(
            repo,
            arguments,
            (Reason::GitCompareSidesFailed, "compare the two sides"),
        )
    }

    /// Streams the files of `commit` as a tar archive into `consume`, then
    /// waits for git. Nothing is written to the repository.
    pub fn archive(
        &self,
        repo: &Path,
        commit: &str,
        consume: impl FnOnce(&mut dyn Read) -> AppResult<()>,
    ) -> AppResult<()> {
        if !is_object_id(commit) {
            return Err(app_error!(
                Internal,
                NotAnObjectId,
                "\"{id}\" is not an object id.",
                id = escaped(commit)
            ));
        }
        let mut command = self.command(Some(repo), ["archive", "--format=tar", commit]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(spawn_error)?;
        let stderr = child.stderr.take();
        let stderr_reader = thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stderr) = stderr {
                // A read error only loses error details.
                let _ = stderr.read_to_end(&mut buffer);
            }
            buffer
        });

        let consumed = match child.stdout.take() {
            Some(mut stdout) => consume(&mut stdout).and_then(|()| {
                // The tar reader stops at the end marker; git may still be
                // writing padding and must not block on a full pipe.
                io::copy(&mut stdout, &mut io::sink())
                    .map(drop)
                    .map_err(|error| {
                        app_error!(
                            Internal,
                            GitArchiveReadFailed,
                            "Could not read git archive: {detail}",
                            detail = error
                        )
                    })
            }),
            None => Err(app_error!(
                Internal,
                GitArchiveEmpty,
                "git archive gave no output."
            )),
        };
        if consumed.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().map_err(wait_error)?;
        let stderr = stderr_reader.join().unwrap_or_default();
        // When reading failed git was stopped here; its own error, if it
        // reported one, is the better explanation.
        if !status.success() && (consumed.is_ok() || !stderr.is_empty()) {
            return Err(app_error!(
                Internal,
                GitReadTreeFailed,
                "git could not read the files of {commit}: {detail}",
                commit = commit,
                detail = git_detail(&String::from_utf8_lossy(&stderr))
            ));
        }
        consumed
    }

    pub fn remotes(&self, repo: &Path) -> AppResult<Vec<String>> {
        let stdout = self.run_checked(
            repo,
            ["remote"],
            (Reason::GitListRemotesFailed, "list remotes"),
        )?;
        Ok(String::from_utf8_lossy(&stdout)
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Commit of every remote-tracking branch, by full ref name. Symbolic
    /// refs such as `origin/HEAD` are left out.
    pub fn remote_tracking_refs(&self, repo: &Path) -> AppResult<BTreeMap<String, String>> {
        let stdout = self.run_checked(
            repo,
            [
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(symref)",
                REMOTE_BRANCHES,
            ],
            (Reason::GitListRemoteBranchesFailed, "list remote branches"),
        )?;
        let mut refs = BTreeMap::new();
        for line in String::from_utf8_lossy(&stdout).split('\n') {
            let mut fields = line.split('\0');
            if let (Some(name), Some(sha), Some("")) = (fields.next(), fields.next(), fields.next())
            {
                refs.insert(name.to_string(), sha.to_string());
            }
        }
        Ok(refs)
    }

    /// Fetches the branches of `remote` into `refs/remotes/<remote>/`,
    /// removing the ones the remote deleted. The refspec is given explicitly
    /// and `--refmap=` drops the configured ones, so no other ref can change
    /// whatever the repository's fetch configuration says: local branches,
    /// tags, HEAD and the working tree are left alone.
    pub fn fetch(&self, repo: &Path, remote: &str) -> AppResult<()> {
        let refspec = format!("+refs/heads/*:refs/remotes/{remote}/*");
        let arguments = [
            // The ext transport runs arbitrary commands taken from the URL.
            "-c",
            "protocol.ext.allow=never",
            "fetch",
            "--prune",
            // Tags would land in refs/tags, which is the user's namespace,
            // not a remote-tracking one.
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            // Otherwise git may leave a detached maintenance process behind.
            "--no-auto-maintenance",
            "--refmap=",
            "--quiet",
            "--",
            remote,
            refspec.as_str(),
        ];
        let failed = |detail: CoreText| {
            app_error!(
                FetchFailed,
                FetchFailed,
                "Could not fetch {remote}: {detail}",
                remote = remote,
                detail = detail
            )
        };
        match self.run_with_timeout(repo, arguments, self.fetch_timeout)? {
            Some(output) if output.success => Ok(()),
            Some(output) => Err(failed(git_detail(&output.stderr))),
            None => Err(failed(core_text!(
                FetchNoAnswer,
                "no answer after {seconds} seconds.",
                seconds = self.fetch_timeout.as_secs()
            ))),
        }
    }

    /// Clones `url` into `destination`. Authentication is whatever the user's
    /// git already does; nothing is stored here.
    pub fn clone_repository(&self, url: &str, destination: &Path) -> AppResult<()> {
        let arguments: [&OsStr; 6] = [
            // The ext transport runs arbitrary commands taken from the URL.
            OsStr::new("-c"),
            OsStr::new("protocol.ext.allow=never"),
            OsStr::new("clone"),
            OsStr::new("--"),
            OsStr::new(url),
            destination.as_os_str(),
        ];
        let output = self.run(None, arguments)?;
        if output.success {
            Ok(())
        } else {
            Err(app_error!(
                CloneFailed,
                CloneFailed,
                "Could not clone {url}: {detail}",
                url = url,
                detail = git_detail(&output.stderr)
            ))
        }
    }

    /// Every worktree of the repository, the main one first, as
    /// `git worktree list --porcelain -z` reports them.
    pub fn worktrees(&self, repo: &Path) -> AppResult<Vec<WorktreeEntry>> {
        let stdout = self.run_checked(
            repo,
            ["worktree", "list", "--porcelain", "-z"],
            (Reason::GitListWorktreesFailed, "list worktrees"),
        )?;
        Ok(parse_worktrees(&stdout))
    }

    /// The folder shared by every worktree of the repository (`.git` of the
    /// main one), where `info/exclude` lives. Absolute.
    pub fn common_dir(&self, repo: &Path) -> AppResult<PathBuf> {
        let stdout = self.run_checked(
            repo,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
            (Reason::GitReadCommonDirFailed, "find the repository folder"),
        )?;
        let text = String::from_utf8_lossy(&stdout);
        let path = text.trim_end_matches(['\n', '\r']);
        if path.is_empty() {
            return Err(unexpected_output("rev-parse --git-common-dir"));
        }
        Ok(PathBuf::from(path))
    }

    /// Checks out the existing local `branch` in a new worktree at `path`.
    /// `branch` must be a name git reported (see the module comment).
    pub fn add_worktree(&self, repo: &Path, path: &Path, branch: &str) -> AppResult<()> {
        let arguments: [&OsStr; 6] = [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(branch),
        ];
        self.run_worktree_change(repo, arguments, ADD_WORKTREE)
    }

    /// Creates the local `branch` tracking `upstream` (such as
    /// `origin/feature/x`) and checks it out in a new worktree at `path`.
    pub fn add_tracking_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        upstream: &str,
    ) -> AppResult<()> {
        let arguments: [&OsStr; 9] = [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--quiet"),
            OsStr::new("--track"),
            OsStr::new("-b"),
            OsStr::new(branch),
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(upstream),
        ];
        self.run_worktree_change(repo, arguments, ADD_WORKTREE)
    }

    /// Removes the worktree at `path`; with `force`, also when it has
    /// uncommitted changes, which are lost. The branch is left alone.
    pub fn remove_worktree(&self, repo: &Path, path: &Path, force: bool) -> AppResult<()> {
        let mut arguments: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("remove")];
        if force {
            arguments.push(OsStr::new("--force"));
        }
        arguments.extend([OsStr::new("--"), path.as_os_str()]);
        self.run_worktree_change(
            repo,
            arguments,
            (Reason::GitRemoveWorktreeFailed, "remove the worktree"),
        )
    }

    fn run_worktree_change<I, S>(
        &self,
        repo: &Path,
        arguments: I,
        action: (Reason, &str),
    ) -> AppResult<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_checked(repo, arguments, action).map(drop)
    }

    /// `action` is the reason of the failure, worded "git could not
    /// {action}: {detail}", and its English action.
    fn run_checked<I, S>(
        &self,
        repo: &Path,
        arguments: I,
        action: (Reason, &str),
    ) -> AppResult<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run(Some(repo), arguments)?;
        if output.success {
            Ok(output.stdout)
        } else {
            let (reason, action) = action;
            let detail = git_detail(&output.stderr);
            Err(AppError::internal(
                reason,
                format!("git could not {action}: {detail}"),
            )
            .arg("detail", detail)
            .verified())
        }
    }

    fn run<I, S>(&self, repo: Option<&Path>, arguments: I) -> AppResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self
            .command(repo, arguments)
            .output()
            .map_err(spawn_error)?;
        Ok(Output {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Runs git with `input` on its standard input. The input is written from
    /// another thread, so git can never block on a full output pipe while the
    /// input is still being written.
    fn run_with_input<I, S>(&self, repo: &Path, arguments: I, input: Vec<u8>) -> AppResult<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(Some(repo), arguments);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(spawn_error)?;
        let writer = child.stdin.take().map(|mut stdin| {
            thread::spawn(move || {
                // A failed write shows up as a failure or a short answer of
                // git itself. Dropping stdin here closes it.
                let _ = stdin.write_all(&input);
            })
        });
        let output = child.wait_with_output().map_err(wait_error)?;
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        Ok(Output {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Runs git with its standard output discarded. Returns `None` when it
    /// did not finish within `timeout`; it is then stopped together with
    /// every process it started (ssh, credential or remote helpers).
    fn run_with_timeout<I, S>(
        &self,
        repo: &Path,
        arguments: I,
        timeout: Duration,
    ) -> AppResult<Option<Output>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(Some(repo), arguments);
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        start_process_group(&mut command);
        let mut child = command.spawn().map_err(spawn_error)?;

        let (sender, receiver) = mpsc::channel();
        let stderr = child.stderr.take();
        thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stderr) = stderr {
                // A read error only loses error details.
                let _ = stderr.read_to_end(&mut buffer);
            }
            let _ = sender.send(buffer);
        });

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    stop_process_group(&mut child);
                    return Ok(None);
                }
                Err(error) => {
                    stop_process_group(&mut child);
                    return Err(wait_error(error));
                }
            }
        };
        let stderr = receiver.recv_timeout(OUTPUT_GRACE).unwrap_or_default();
        Ok(Some(Output {
            success: status.success(),
            stdout: Vec::new(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }))
    }

    fn command<I, S>(&self, repo: Option<&Path>, arguments: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.program);
        command.stdin(Stdio::null());
        for variable in SCRUBBED_ENVIRONMENT {
            command.env_remove(variable);
        }
        command
            // There is no terminal to answer a credential prompt.
            .env("GIT_TERMINAL_PROMPT", "0")
            // Same for Git Credential Manager, which would open a window.
            .env("GCM_INTERACTIVE", "never")
            // Keeps read commands such as status from refreshing the index
            // file of the user's repository.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C");
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        hide_console_window(&mut command);
        if let Some(repo) = repo {
            command.arg("-C").arg(repo);
        }
        // `git diff` refreshes the index when a file's stat changed but its
        // content did not, and it ignores GIT_OPTIONAL_LOCKS when doing so.
        command.args(["-c", "diff.autoRefreshIndex=false"]);
        command.args(arguments);
        command
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

fn spawn_error(error: io::Error) -> AppError {
    if error.kind() == io::ErrorKind::NotFound {
        app_error!(
            GitNotFound,
            GitNotFound,
            "git was not found. Install git and make sure it is on the PATH."
        )
    } else {
        app_error!(
            Internal,
            GitRunFailed,
            "Could not run git: {detail}",
            detail = error
        )
    }
}

fn wait_error(error: io::Error) -> AppError {
    app_error!(
        Internal,
        GitWaitFailed,
        "Could not wait for git: {detail}",
        detail = error
    )
}

fn unexpected_output(command: &str) -> AppError {
    app_error!(
        Internal,
        GitUnexpectedOutput,
        "Unexpected output from git {command}.",
        command = command
    )
}

/// A full SHA-1 or SHA-256 object id.
fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// git writes paths as UTF-8 on other systems.
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn first_line(output: &Output) -> Option<&str> {
    if !output.success {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then_some(line)
}

/// What git printed on failure, as the detail of an error; worded by the
/// core when git printed nothing.
fn git_detail(stderr: &str) -> CoreText {
    if stderr.trim().is_empty() {
        core_text!(GitNoDetails, "git reported no details.")
    } else {
        CoreText::raw(error_detail(stderr))
    }
}

fn error_detail(stderr: &str) -> String {
    let detail = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        return "git reported no details.".to_string();
    }
    let length = detail.chars().count();
    if length <= MAX_ERROR_DETAIL_CHARS {
        detail
    } else {
        // The end of the output carries the actual failure.
        detail
            .chars()
            .skip(length - MAX_ERROR_DETAIL_CHARS)
            .collect()
    }
}

/// Parses one line of `BRANCH_FORMAT`. Symbolic refs (`origin/HEAD`) and
/// empty lines give `None`.
fn parse_branch_line(line: &str) -> AppResult<Option<BranchRef>> {
    if line.is_empty() {
        return Ok(None);
    }
    let fields: Vec<&str> = line.splitn(BRANCH_FIELDS, '\0').collect();
    let [full_ref, symref, sha, head_marker, authored_at, upstream, track, subject] =
        fields.as_slice()
    else {
        return Err(unexpected_output("for-each-ref"));
    };
    if !symref.is_empty() {
        return Ok(None);
    }
    let (kind, name) = if let Some(name) = full_ref.strip_prefix(LOCAL_BRANCHES) {
        (RefKind::Local, name)
    } else if let Some(name) = full_ref.strip_prefix(ORIGIN_BRANCHES) {
        (RefKind::Origin, name)
    } else {
        return Err(unexpected_output("for-each-ref"));
    };
    let upstream = match kind {
        RefKind::Local => parse_upstream(upstream, track)?,
        RefKind::Origin => None,
    };
    Ok(Some(BranchRef {
        kind,
        name: name.to_string(),
        full_ref: full_ref.to_string(),
        is_current: *head_marker == "*",
        head: Commit {
            sha: sha.to_string(),
            subject: subject.to_string(),
            authored_at: authored_at.to_string(),
        },
        upstream,
    }))
}

/// `%(upstream:track,nobracket)` is empty when in sync, `gone` when the
/// remote-tracking branch no longer exists, otherwise `ahead N`, `behind N`
/// or `ahead N, behind M`.
fn parse_upstream(name: &str, track: &str) -> AppResult<Option<Upstream>> {
    if name.is_empty() || track == "gone" {
        return Ok(None);
    }
    let mut upstream = Upstream {
        name: name.to_string(),
        ahead: 0,
        behind: 0,
    };
    for part in track.split(", ").filter(|part| !part.is_empty()) {
        let (label, number) = part
            .split_once(' ')
            .ok_or_else(|| unexpected_output("for-each-ref upstream:track"))?;
        let number: u32 = number
            .parse()
            .map_err(|_| unexpected_output("for-each-ref upstream:track"))?;
        match label {
            "ahead" => upstream.ahead = number,
            "behind" => upstream.behind = number,
            _ => return Err(unexpected_output("for-each-ref upstream:track")),
        }
    }
    Ok(Some(upstream))
}

/// Parses `git worktree list --porcelain -z`: one NUL-terminated attribute
/// per field (`worktree <path>`, `branch <ref>`, `detached`, `bare`,
/// `prunable <reason>`...), and an empty field after each worktree.
fn parse_worktrees(stdout: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            entries.extend(current.take());
            continue;
        }
        let (name, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(space) => (&field[..space], &field[space + 1..]),
            None => (field, &[][..]),
        };
        match name {
            b"worktree" => {
                entries.extend(current.take());
                current = Some(WorktreeEntry {
                    path: path_from_bytes(value),
                    branch: None,
                    bare: false,
                    prunable: false,
                });
            }
            b"branch" => {
                if let Some(entry) = current.as_mut() {
                    let reference = String::from_utf8_lossy(value);
                    entry.branch = reference
                        .strip_prefix(LOCAL_BRANCHES)
                        .map(str::to_string);
                }
            }
            b"bare" => {
                if let Some(entry) = current.as_mut() {
                    entry.bare = true;
                }
            }
            b"prunable" => {
                if let Some(entry) = current.as_mut() {
                    entry.prunable = true;
                }
            }
            _ => {}
        }
    }
    entries.extend(current);
    entries
}

/// Counts the entries of `git status --porcelain=v1 -z`. Each entry is
/// `XY <path>` terminated by NUL; a rename or copy is followed by one more
/// NUL-terminated field with the original path, which is not an entry.
fn count_status_entries(stdout: &[u8]) -> u32 {
    let mut fields = stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut entries = 0u32;
    while let Some(entry) = fields.next() {
        entries = entries.saturating_add(1);
        let has_original_path = entry
            .iter()
            .take(2)
            .any(|status| matches!(status, b'R' | b'C'));
        if has_original_path {
            fields.next();
        }
    }
    entries
}

/// Parses `git diff --numstat -z`. A record is `<added>\t<deleted>\t<path>`
/// terminated by NUL; for a rename the path is empty and two more
/// NUL-terminated fields follow with the old and the new path. Binary files
/// report `-` for both counts.
fn parse_numstat(stdout: &[u8]) -> AppResult<DiffStat> {
    let mut stat = DiffStat::default();
    let mut fields = stdout.split(|byte| *byte == 0);
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut columns = record.splitn(3, |byte| *byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            return Err(unexpected_output("diff --numstat"));
        };
        stat.files_changed = stat.files_changed.saturating_add(1);
        stat.additions = stat.additions.saturating_add(parse_line_count(added)?);
        stat.deletions = stat.deletions.saturating_add(parse_line_count(deleted)?);
        if path.is_empty() {
            fields.next();
            fields.next();
        }
    }
    Ok(stat)
}

/// Arguments of `git diff --raw -z` from `from` to `to`, or to the working
/// tree, with `extra` options and limited to `paths`. Both sides must be
/// object ids, so nothing from outside can be read as an option.
fn raw_diff_arguments(
    from: &str,
    to: Option<&str>,
    extra: &[&str],
    paths: &[&str],
) -> AppResult<Vec<String>> {
    for commit in std::iter::once(from).chain(to) {
        if !is_object_id(commit) {
            return Err(app_error!(
                Internal,
                NotAnObjectId,
                "\"{id}\" is not an object id.",
                id = escaped(commit)
            ));
        }
    }
    let mut arguments: Vec<String> = [
        "diff",
        "--raw",
        "-z",
        "--no-abbrev",
        // See diff_stat.
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
    ]
    .iter()
    .chain(extra)
    .map(|argument| argument.to_string())
    .collect();
    arguments.push(from.to_string());
    arguments.extend(to.map(str::to_string));
    arguments.push("--".to_string());
    arguments.extend(literal_pathspecs(paths));
    Ok(arguments)
}

/// Pathspecs that match `paths` exactly as written: no glob, no magic, and a
/// leading `-` or `:` is part of the name.
fn literal_pathspecs<'a>(paths: &'a [&str]) -> impl Iterator<Item = String> + 'a {
    paths.iter().map(|path| format!(":(literal){path}"))
}

/// Parses `git diff --raw --numstat -z`. The raw records come first, one per
/// file: `:<old mode> <new mode> <old oid> <new oid> <status>` then the path,
/// or for a rename or copy the source and the destination, each field ending
/// in NUL. The numstat records follow (see `parse_numstat`). A file the raw
/// part lists but numstat does not only had its stat information change
/// (`diff.autoRefreshIndex` is off), so it is left out, as numstat leaves it
/// out of the totals.
fn parse_diff_files(stdout: &[u8]) -> AppResult<Vec<FileChange>> {
    let bad = || unexpected_output("diff --raw --numstat");
    let mut fields = stdout.split(|byte| *byte == 0).peekable();
    let changes = parse_raw(&mut fields)?;

    let mut counts: HashMap<Vec<u8>, (u32, u32, bool)> = HashMap::new();
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut columns = record.splitn(3, |byte| *byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            return Err(bad());
        };
        let path = if path.is_empty() {
            // A rename: the source, then the destination.
            fields.next().ok_or_else(bad)?;
            fields.next().ok_or_else(bad)?
        } else {
            path
        };
        let binary = added == b"-";
        counts.insert(
            path.to_vec(),
            (parse_line_count(added)?, parse_line_count(deleted)?, binary),
        );
    }

    Ok(changes
        .into_iter()
        .filter_map(|mut change| {
            let (additions, deletions, binary) = counts.remove(&change.path)?;
            change.additions = additions;
            change.deletions = deletions;
            change.binary = binary;
            Some(change)
        })
        .collect())
}

/// Parses the `--raw -z` records at the front of `fields`, leaving whatever
/// follows them. Line counts are left at zero.
fn parse_raw<'a>(
    fields: &mut std::iter::Peekable<impl Iterator<Item = &'a [u8]>>,
) -> AppResult<Vec<FileChange>> {
    let bad = || unexpected_output("diff --raw");
    let mut changes = Vec::new();
    while let Some(header) = fields.next_if(|field| field.first() == Some(&b':')) {
        let header = std::str::from_utf8(&header[1..]).map_err(|_| bad())?;
        let columns: Vec<&str> = header.split(' ').collect();
        let [old_mode, new_mode, old_oid, new_oid, status] = columns.as_slice() else {
            return Err(bad());
        };
        let kind = match status.as_bytes().first() {
            Some(b'A') => ChangeKind::Added,
            Some(b'C') => ChangeKind::Copied,
            Some(b'D') => ChangeKind::Deleted,
            Some(b'M') => ChangeKind::Modified,
            Some(b'R') => ChangeKind::Renamed,
            Some(b'T') => ChangeKind::TypeChanged,
            Some(b'U') => ChangeKind::Unmerged,
            _ => return Err(bad()),
        };
        let mut next_path = || match fields.next() {
            Some(path) if !path.is_empty() => Ok(path.to_vec()),
            _ => Err(bad()),
        };
        let first = next_path()?;
        let (path, source) = match kind {
            ChangeKind::Renamed | ChangeKind::Copied => (next_path()?, Some(first)),
            _ => (first, None),
        };
        let side = |mode: &str, oid: &str| -> AppResult<Option<TreeEntry>> {
            let mode = u32::from_str_radix(mode, 8).map_err(|_| bad())?;
            Ok((mode != 0).then(|| TreeEntry {
                mode,
                oid: oid.to_string(),
            }))
        };
        changes.push(FileChange {
            kind,
            path,
            source,
            old: side(old_mode, old_oid)?,
            new: side(new_mode, new_oid)?,
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    Ok(changes)
}

/// Parses `git cat-file --batch`: for each object a header line
/// `<oid> <type> <size>` followed by exactly that many bytes and a newline, or
/// `<query> missing`.
fn parse_batch(stdout: &[u8]) -> AppResult<Vec<Option<Vec<u8>>>> {
    let bad = || unexpected_output("cat-file --batch");
    let mut objects = Vec::new();
    let mut rest = stdout;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(bad)?;
        let header = std::str::from_utf8(&rest[..end]).map_err(|_| bad())?;
        rest = &rest[end + 1..];
        if header.ends_with(" missing") {
            objects.push(None);
            continue;
        }
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(bad)?;
        let content = rest.get(..size).ok_or_else(bad)?;
        if rest.get(size) != Some(&b'\n') {
            return Err(bad());
        }
        objects.push(Some(content.to_vec()));
        rest = &rest[size + 1..];
    }
    Ok(objects)
}

fn parse_line_count(field: &[u8]) -> AppResult<u32> {
    if field == b"-" {
        return Ok(0);
    }
    std::str::from_utf8(field)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| unexpected_output("diff --numstat"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{isolated_git, run_git, TestRepo};

    #[test]
    fn worktree_records_are_parsed_with_branch_detached_bare_and_prunable() {
        let stdout = b"worktree /repo\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0\
            worktree /repo/.difflens/worktrees/feature/x y\0HEAD 2222222222222222222222222222222222222222\0detached\0\0\
            worktree /gone\0HEAD 3333333333333333333333333333333333333333\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0\
            worktree /bare\0bare\0\0";
        let entries = parse_worktrees(stdout);
        let summary: Vec<(&str, Option<&str>, bool, bool)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.path.to_str().unwrap(),
                    entry.branch.as_deref(),
                    entry.bare,
                    entry.prunable,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("/repo", Some("main"), false, false),
                ("/repo/.difflens/worktrees/feature/x y", None, false, false),
                ("/gone", Some("old"), false, true),
                ("/bare", None, true, false),
            ]
        );
    }

    #[test]
    fn toplevel_is_none_outside_a_repository() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(isolated_git().toplevel(dir.path()).unwrap(), None);
    }

    #[test]
    fn toplevel_finds_the_root_from_a_subfolder() {
        let repo = TestRepo::init();
        repo.commit_file("deep/inside/file.txt", "x\n", "add file");
        let root = isolated_git()
            .toplevel(&repo.path().join("deep/inside"))
            .unwrap();
        assert_eq!(root.as_deref(), Some(repo.path()));
    }

    #[test]
    fn a_missing_git_binary_is_reported_as_git_not_found() {
        let dir = tempfile::TempDir::new().unwrap();
        let git = Git::new().with_program("difflens-test-no-such-git");
        let error = git.toplevel(dir.path()).unwrap_err();
        assert_eq!(error.code, ErrorCode::GitNotFound);
    }

    #[test]
    fn a_repository_without_commits_has_no_branches_and_no_head() {
        let repo = TestRepo::init();
        let git = isolated_git();
        assert!(git.branch_refs(repo.path()).unwrap().is_empty());
        assert_eq!(git.head_commit(repo.path()).unwrap(), None);
        assert_eq!(
            git.empty_tree(repo.path()).unwrap(),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
    }

    #[test]
    fn branch_refs_report_head_commit_and_current_marker() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first: with \"quotes\" and\ttab");
        repo.git(&["checkout", "--quiet", "-b", "feature/ünï-côde#1"]);
        repo.commit_file("b.txt", "b\n", "second");

        let branches = isolated_git().branch_refs(repo.path()).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["feature/ünï-côde#1", "main"]);
        assert!(branches.iter().all(|b| b.kind == RefKind::Local));
        assert!(branches[0].is_current);
        assert!(!branches[1].is_current);
        assert_eq!(branches[0].full_ref, "refs/heads/feature/ünï-côde#1");
        assert_eq!(branches[0].head.subject, "second");
        assert_eq!(branches[1].head.subject, "first: with \"quotes\" and\ttab");
        assert_eq!(branches[0].head.sha, repo.git(&["rev-parse", "HEAD"]));
        assert_eq!(branches[1].head.authored_at, "2026-01-01T02:00:01+02:00");
        assert_eq!(branches[0].upstream, None);
    }

    #[test]
    fn branch_refs_include_origin_without_its_head_and_track_upstreams() {
        let source = TestRepo::init();
        source.commit_file("a.txt", "a\n", "first");
        source.git(&["branch", "other"]);
        let clone = TestRepo::clone_from(source.path());
        clone.commit_file("b.txt", "b\n", "local work");
        clone.git(&["branch", "--quiet", "--track", "behind", "origin/other"]);
        // As if someone pushed the local commit to origin/other.
        clone.git(&["update-ref", "refs/remotes/origin/other", "HEAD"]);
        // A second remote is not origin and is ignored.
        clone.git(&["update-ref", "refs/remotes/fork/main", "HEAD"]);

        let refs = isolated_git().branch_refs(clone.path()).unwrap();

        let summary: Vec<(&str, RefKind)> = refs
            .iter()
            .map(|branch| (branch.full_ref.as_str(), branch.kind))
            .collect();
        assert_eq!(summary.len(), 4, "{summary:?}");
        let main = refs
            .iter()
            .find(|b| b.full_ref == "refs/heads/main")
            .unwrap();
        assert_eq!(
            main.upstream,
            Some(Upstream {
                name: "origin/main".to_string(),
                ahead: 1,
                behind: 0
            })
        );
        let origin_main = refs
            .iter()
            .find(|b| b.full_ref == "refs/remotes/origin/main")
            .unwrap();
        assert_eq!(origin_main.kind, RefKind::Origin);
        assert_eq!(origin_main.name, "main");
        assert_eq!(origin_main.upstream, None);
        let behind = refs
            .iter()
            .find(|b| b.full_ref == "refs/heads/behind")
            .unwrap();
        assert_eq!(
            behind.upstream,
            Some(Upstream {
                name: "origin/other".to_string(),
                ahead: 0,
                behind: 1
            })
        );
        assert!(refs
            .iter()
            .any(|b| b.full_ref == "refs/remotes/origin/other"));
    }

    #[test]
    fn upstream_tracking_text_is_parsed() {
        let parsed = |track| parse_upstream("origin/x", track).unwrap();
        assert_eq!(parsed("").map(|u| (u.ahead, u.behind)), Some((0, 0)));
        assert_eq!(parsed("ahead 3").map(|u| (u.ahead, u.behind)), Some((3, 0)));
        assert_eq!(
            parsed("behind 2").map(|u| (u.ahead, u.behind)),
            Some((0, 2))
        );
        assert_eq!(
            parsed("ahead 3, behind 2").map(|u| (u.ahead, u.behind)),
            Some((3, 2))
        );
        assert_eq!(parsed("gone"), None);
        assert_eq!(parse_upstream("", "").unwrap(), None);
        assert!(parse_upstream("origin/x", "sideways 1").is_err());
    }

    #[test]
    fn ahead_behind_all_counts_every_ref_against_one_commit() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        repo.git(&["checkout", "--quiet", "-b", "feature"]);
        repo.commit_file("b.txt", "b\n", "feature 1");
        repo.commit_file("c.txt", "c\n", "feature 2");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit_file("d.txt", "d\n", "main moves on");
        let main = repo.git(&["rev-parse", "main"]);

        let distances = isolated_git().ahead_behind_all(repo.path(), &main).unwrap();

        assert_eq!(distances.len(), 2);
        assert_eq!(
            distances["refs/heads/feature"],
            AheadBehind {
                ahead: 2,
                behind: 1
            }
        );
        assert_eq!(
            distances["refs/heads/main"],
            AheadBehind {
                ahead: 0,
                behind: 0
            }
        );
        assert!(isolated_git()
            .ahead_behind_all(repo.path(), "main)")
            .is_err());
    }

    #[test]
    fn working_tree_counts_staged_unstaged_untracked_and_renamed_files() {
        let repo = TestRepo::init();
        repo.commit_file("tracked.txt", "one\n", "first");
        repo.commit_file("to rename.txt", "content that stays the same\n", "second");
        let git = isolated_git();
        assert_eq!(
            git.working_tree(repo.path()).unwrap(),
            WorkingTree {
                dirty: false,
                files_changed: 0
            }
        );

        repo.write("tracked.txt", "changed\n");
        repo.write("new folder/untracked a.txt", "a\n");
        repo.write("new folder/untracked b.txt", "b\n");
        repo.write("staged.txt", "staged\n");
        repo.git(&["add", "staged.txt"]);
        repo.git(&["mv", "to rename.txt", "renamed file.txt"]);

        assert_eq!(
            git.working_tree(repo.path()).unwrap(),
            WorkingTree {
                dirty: true,
                files_changed: 5
            }
        );
    }

    #[test]
    fn reading_the_status_does_not_touch_the_index() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        // A changed mtime with the same content makes a plain `git status`
        // rewrite the index.
        repo.write("a.txt", "a\n");
        let index = repo.path().join(".git/index");
        let before = std::fs::read(&index).unwrap();

        let git = isolated_git();
        git.working_tree(repo.path()).unwrap();
        let head = git.head_commit(repo.path()).unwrap().unwrap();
        git.diff_stat(repo.path(), &head, None).unwrap();
        git.untracked_files(repo.path(), &[]).unwrap();

        assert_eq!(std::fs::read(&index).unwrap(), before);
    }

    #[test]
    fn untracked_files_respect_gitignore() {
        let repo = TestRepo::init();
        repo.commit_file(".gitignore", "*.log\nbuild/\n", "ignore");
        repo.write("new file.txt", "x\n");
        repo.write("dir/nested.txt", "y\n");
        repo.write("debug.log", "ignored\n");
        repo.write("build/out.bin", "ignored\n");

        let mut files = isolated_git().untracked_files(repo.path(), &[]).unwrap();
        files.sort();

        assert_eq!(
            files,
            vec![
                PathBuf::from("dir/nested.txt"),
                PathBuf::from("new file.txt")
            ]
        );
    }

    #[test]
    fn remotes_and_remote_tracking_refs_are_listed() {
        let source = TestRepo::init();
        source.commit_file("a.txt", "a\n", "first");
        let clone = TestRepo::clone_from(source.path());
        let git = isolated_git();

        assert_eq!(git.remotes(clone.path()).unwrap(), vec!["origin"]);
        let refs = git.remote_tracking_refs(clone.path()).unwrap();
        assert_eq!(
            refs.keys().collect::<Vec<_>>(),
            vec!["refs/remotes/origin/main"]
        );
        assert!(git.remotes(source.path()).unwrap().is_empty());
    }

    #[test]
    fn remote_urls_abbreviated_commits_and_ancestry_are_read() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        let first = repo.git(&["rev-parse", "HEAD"]);
        repo.commit_file("a.txt", "b\n", "second");
        let second = repo.git(&["rev-parse", "HEAD"]);
        let git = isolated_git();

        assert_eq!(git.remote_url(repo.path(), "origin").unwrap(), None);
        repo.git(&[
            "remote",
            "add",
            "origin",
            "git@bitbucket.org:acme/payments-api.git",
        ]);
        assert_eq!(
            git.remote_url(repo.path(), "origin").unwrap().as_deref(),
            Some("git@bitbucket.org:acme/payments-api.git")
        );

        assert_eq!(
            git.resolve_commit(repo.path(), &second[..12]).unwrap(),
            Some(second.clone())
        );
        for not_a_commit in ["0000000000ab", "HEAD", "--all", "abc"] {
            assert_eq!(
                git.resolve_commit(repo.path(), not_a_commit).unwrap(),
                None,
                "{not_a_commit}"
            );
        }

        assert!(git.is_ancestor(repo.path(), &first, &second).unwrap());
        assert!(git.is_ancestor(repo.path(), &second, &second).unwrap());
        assert!(!git.is_ancestor(repo.path(), &second, &first).unwrap());
    }

    #[test]
    fn a_fetch_that_hangs_is_stopped_with_everything_it_started() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        repo.git(&["remote", "add", "origin", "ssh://example.invalid/repo.git"]);
        let pid_file = repo.parent().join("ssh.pid");
        // Stands in for an ssh that never answers. git appends the host and
        // the remote command, which the trailing `:` swallows.
        let ssh = format!("echo $$ > '{}'; exec sleep 30; :", pid_file.display());
        let git = isolated_git()
            .with_environment("GIT_SSH_COMMAND", &ssh)
            .with_fetch_timeout(Duration::from_millis(1500));

        let started = Instant::now();
        let error = git.fetch(repo.path(), "origin").unwrap_err();

        assert_eq!(error.code, ErrorCode::FetchFailed);
        assert!(error.message.contains("no answer"), "{}", error.message);
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim();
        let alive = || {
            Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let stopped_by = Instant::now() + Duration::from_secs(5);
        while alive() && Instant::now() < stopped_by {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the fake ssh process {pid} is still running");
    }

    #[test]
    fn a_failed_fetch_reports_git_details() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        let missing = repo.parent().join("no such remote");
        run_git(
            repo.path(),
            &["remote", "add", "origin", missing.to_str().unwrap()],
        );

        let error = isolated_git().fetch(repo.path(), "origin").unwrap_err();

        assert_eq!(error.code, ErrorCode::FetchFailed);
        assert!(error.message.starts_with("Could not fetch origin: "));
    }

    #[test]
    fn numstat_parsing_handles_text_binary_and_renames() {
        let output = b"3\t1\tsrc/a file.rs\0-\t-\timage.png\x000\t0\t\0old name.txt\0new name.txt\x002\t0\tb.rs\0";
        assert_eq!(
            parse_numstat(output).unwrap(),
            DiffStat {
                files_changed: 4,
                additions: 5,
                deletions: 1
            }
        );
        assert_eq!(parse_numstat(b"").unwrap(), DiffStat::default());
        assert!(parse_numstat(b"garbage\0").is_err());
    }

    #[test]
    fn raw_and_numstat_records_are_joined_by_path() {
        let zero = "0".repeat(40);
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let output = format!(
            ":100644 100644 {a} {zero} M\0src/a file.rs\0\
             :000000 100644 {zero} {b} A\0:starts with colon\0\
             :100644 100644 {a} {b} R087\0old\tname\0new name\0\
             :100644 120000 {a} {zero} T\0link\0\
             :100644 100644 {a} {zero} M\0only stat changed\0\
             :100644 000000 {a} {zero} D\0-gone\0\
             3\t1\tsrc/a file.rs\0-\t-\t:starts with colon\0\
             1\t1\t\0old\tname\0new name\x001\t1\tlink\x000\t2\t-gone\0"
        );

        let changes = parse_diff_files(output.as_bytes()).unwrap();

        type Summary<'a> = (ChangeKind, &'a [u8], Option<&'a [u8]>, u32, u32, bool);
        let summary: Vec<Summary<'_>> = changes
            .iter()
            .map(|c| {
                (
                    c.kind,
                    c.path.as_slice(),
                    c.source.as_deref(),
                    c.additions,
                    c.deletions,
                    c.binary,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    ChangeKind::Modified,
                    &b"src/a file.rs"[..],
                    None,
                    3,
                    1,
                    false
                ),
                (ChangeKind::Added, b":starts with colon", None, 0, 0, true),
                (
                    ChangeKind::Renamed,
                    b"new name",
                    Some(&b"old\tname"[..]),
                    1,
                    1,
                    false
                ),
                (ChangeKind::TypeChanged, b"link", None, 1, 1, false),
                (ChangeKind::Deleted, b"-gone", None, 0, 2, false),
            ]
        );
        assert_eq!(changes[1].old, None);
        assert_eq!(
            changes[3].new,
            Some(TreeEntry {
                mode: 0o120000,
                oid: zero.clone()
            })
        );
        assert_eq!(changes[4].new, None);
        assert!(parse_diff_files(b"").unwrap().is_empty());
        assert!(parse_diff_files(b":100644 100644 x y Z\0path\0").is_err());
        assert!(parse_diff_files(b":100644 100644 x y R100\0only one\0").is_err());
    }

    #[test]
    fn batch_output_is_split_by_the_sizes_it_announces() {
        let output = b"aaa blob 4\nx\ny\n\nbbb missing\nccc blob 0\n\n";
        assert_eq!(
            parse_batch(output).unwrap(),
            vec![Some(b"x\ny\n".to_vec()), None, Some(Vec::new())]
        );
        assert!(parse_batch(b"aaa blob 9\nshort\n").is_err());
        assert!(parse_batch(b"aaa blob 1\nxy").is_err());
    }

    #[test]
    fn commits_and_blobs_are_looked_up_in_one_batch() {
        let repo = TestRepo::init();
        repo.write("small.txt", "small\n");
        repo.write("large.txt", "large file\n");
        repo.write("empty.txt", "");
        repo.commit_all("first");
        let git = isolated_git();
        let head = repo.git(&["rev-parse", "HEAD"]);
        let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
        let blob = |path: &str| repo.git(&["rev-parse", &format!("HEAD:{path}")]);
        let unknown = "0123456789abcdef0123456789abcdef01234567";

        assert_eq!(
            git.are_commits(repo.path(), &[&head, &tree, unknown, "HEAD", "", &head])
                .unwrap(),
            vec![true, false, false, false, false, true]
        );
        assert!(git.are_commits(repo.path(), &[]).unwrap().is_empty());

        let (small, large, empty) = (blob("small.txt"), blob("large.txt"), blob("empty.txt"));
        assert_eq!(
            git.read_blobs(repo.path(), &[&large, unknown, &small, &empty], 8)
                .unwrap(),
            vec![
                Blob::TooLarge(11),
                Blob::Missing,
                Blob::Content(b"small\n".to_vec()),
                Blob::Content(Vec::new()),
            ]
        );
        assert!(git.read_blobs(repo.path(), &["HEAD:small.txt"], 8).is_err());
        assert!(git.read_blobs(repo.path(), &[&tree], 8).is_err());
    }

    #[test]
    fn diff_paths_are_literal_and_sides_must_be_object_ids() {
        let repo = TestRepo::init();
        repo.commit_file("a.txt", "a\n", "first");
        let first = repo.git(&["rev-parse", "HEAD"]);
        repo.write("*.txt", "glob\n");
        repo.write("-n", "dash\n");
        repo.write("a.txt", "b\n");
        repo.commit_all("second");
        let second = repo.git(&["rev-parse", "HEAD"]);
        let git = isolated_git();

        let paths = |pathspecs: &[&str]| -> Vec<Vec<u8>> {
            git.diff_files(repo.path(), &first, Some(&second), pathspecs)
                .unwrap()
                .into_iter()
                .map(|change| change.path)
                .collect()
        };
        assert_eq!(paths(&["*.txt"]), vec![b"*.txt".to_vec()]);
        assert_eq!(paths(&["-n"]), vec![b"-n".to_vec()]);
        assert_eq!(paths(&[]).len(), 3);
        assert!(git.diff_files(repo.path(), "HEAD", None, &[]).is_err());
        assert!(git
            .diff_files(repo.path(), &first, Some("--output=x"), &[])
            .is_err());
        assert!(git
            .renames(repo.path(), &first, Some(&second))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn status_parsing_skips_the_original_path_of_a_rename() {
        let output = b" M a.txt\0R  new.txt\0old.txt\0?? R starts with r.txt\0";
        assert_eq!(count_status_entries(output), 3);
        assert_eq!(count_status_entries(b""), 0);
    }

    #[test]
    fn reads_files_at_a_commit_and_the_messages_of_a_range() {
        let repo = TestRepo::init();
        repo.commit_file("a.go", "package a\n", "first");
        let first = repo.git(&["rev-parse", "HEAD"]);
        repo.commit_file("dir/b.go", "package b\n", "Add b\n\nBecause the card asks for it.");
        repo.commit_file("a.go", "package a // two\n", "Touch a");
        let head = repo.git(&["rev-parse", "HEAD"]);
        let git = isolated_git();

        let files = git
            .files_at(repo.path(), &head, &["a.go", "dir", "gone.go", "bad\nname"], 1024)
            .unwrap();
        assert_eq!(files[0], Blob::Content(b"package a // two\n".to_vec()));
        assert_eq!(files[1..], [Blob::Missing, Blob::Missing, Blob::Missing]);
        assert!(git.files_at(repo.path(), "HEAD", &["a.go"], 1024).is_err());

        let messages = git.commit_messages(repo.path(), &first, &head, 10).unwrap();
        assert_eq!(
            messages,
            ["Touch a", "Add b\n\nBecause the card asks for it."]
        );
        assert_eq!(
            git.commit_messages(repo.path(), &first, &head, 1).unwrap(),
            ["Touch a"]
        );
        assert!(git.commit_messages(repo.path(), "HEAD~1", &head, 1).is_err());
    }

    #[test]
    fn error_detail_keeps_the_end_of_long_output() {
        let long = format!("{}\nfatal: the real reason", "noise ".repeat(500));
        let detail = error_detail(&long);
        assert_eq!(detail.chars().count(), MAX_ERROR_DETAIL_CHARS);
        assert!(detail.ends_with("fatal: the real reason"));
        assert_eq!(error_detail(" \n"), "git reported no details.");
    }
}
