//! Where a terminal opens, and the worktrees DiffLens creates so a branch
//! that is checked out nowhere has a folder to open in.
//!
//! A branch already checked out in some worktree of the repository (the
//! project folder included) opens there. Otherwise DiffLens can create a
//! worktree for it, only when asked, at `<project>/.difflens/worktrees/<branch>`
//! (the slashes of the branch become folders). Before the first one,
//! `.difflens/` is added to the repository's `info/exclude`, never to a
//! `.gitignore`, so nothing of it shows in the project folder's `git status`
//! or reaches a commit. A branch that only exists on origin gets a local
//! branch of the same name that tracks it.
//!
//! Nothing here checks anything out in the project folder or changes its
//! files; removing a worktree never deletes its branch.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{app_error, escaped, AppError, AppResult};
use crate::git::{Git, RefKind, WorktreeEntry};
use crate::model::{Folder, FolderKind, ReviewTarget, TerminalLocation, Worktree, WorktreePlan};
use crate::projects::{display_path, path_text, unknown_branch};

/// The folder of the project DiffLens owns, as `info/exclude` names it.
pub const DIFFLENS_FOLDER: &str = ".difflens";
const WORKTREES_FOLDER: &str = "worktrees";
const EXCLUDE_LINE: &str = ".difflens/";
const REMOTE: &str = "origin";

/// Where DiffLens worktrees go in `project`.
pub fn worktrees_root(project: &Path) -> PathBuf {
    project.join(DIFFLENS_FOLDER).join(WORKTREES_FOLDER)
}

/// Where terminals open: for the project screen (`None`) and a review of the
/// uncommitted changes, the project folder; for a branch, the worktree it is
/// checked out in, or a plan for creating one when there is none.
pub fn location(
    git: &Git,
    project: &Path,
    home: Option<&Path>,
    target: Option<&ReviewTarget>,
) -> AppResult<TerminalLocation> {
    let entries = checkouts(git, project)?;
    match target {
        None | Some(ReviewTarget::WorkingTree) => {
            let branch = entries
                .iter()
                .find(|entry| entry.path == project)
                .and_then(|entry| entry.branch.clone());
            Ok(TerminalLocation {
                folder: Some(folder(project, project, home)?),
                branch,
                plan: None,
            })
        }
        Some(ReviewTarget::Branch { branch, .. }) => {
            branch_location(git, project, home, &entries, branch)
        }
    }
}

/// Adds `.difflens/` to the repository's `info/exclude`, unless a line
/// already excludes it. The file (and its folder) is created when missing;
/// what it already says is kept. `info/exclude` lives in the folder shared
/// by every worktree, so it is found through `git rev-parse
/// --git-common-dir`, which also works when the project is itself a linked
/// worktree.
pub fn ensure_excluded(git: &Git, project: &Path) -> AppResult<()> {
    let info = git.common_dir(project)?.join("info");
    let exclude = info.join("exclude");
    let existing = match fs::read(&exclude) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(read_failed(&exclude, &error)),
    };
    let already = String::from_utf8_lossy(&existing).lines().any(|line| {
        matches!(
            line.trim(),
            ".difflens" | ".difflens/" | "/.difflens" | "/.difflens/"
        )
    });
    if already {
        return Ok(());
    }
    fs::create_dir_all(&info).map_err(|error| write_failed(&info, &error))?;
    let mut addition = String::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        addition.push('\n');
    }
    addition.push_str(EXCLUDE_LINE);
    addition.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .and_then(|mut file| file.write_all(addition.as_bytes()))
        .map_err(|error| write_failed(&exclude, &error))
}

/// Creates the DiffLens worktree of `branch` and returns where its terminals
/// open. A local branch is checked out as it is; a branch only on origin
/// gets a local branch tracking it. A branch already checked out somewhere
/// is left alone and its folder returned.
pub fn create(
    git: &Git,
    project: &Path,
    home: Option<&Path>,
    branch: &str,
) -> AppResult<TerminalLocation> {
    let entries = checkouts(git, project)?;
    if find_checkout(&entries, branch).is_some() {
        return branch_location(git, project, home, &entries, branch);
    }
    let source = branch_source(git, project, branch)?;
    let path = worktree_path(project, branch)?;
    // symlink_metadata also sees a dangling symlink, which git would refuse.
    if path.symlink_metadata().is_ok() {
        return Err(app_error!(
            DestinationExists,
            WorktreeFolderTaken,
            "{path} already exists, so the worktree cannot be created there.",
            path = path.display()
        ));
    }
    ensure_excluded(git, project)?;
    match source {
        Source::Local => git.add_worktree(project, &path, branch)?,
        Source::Remote(upstream) => git.add_tracking_worktree(project, &path, branch, &upstream)?,
    }
    let entries = checkouts(git, project)?;
    branch_location(git, project, home, &entries, branch)
}

/// Every worktree of the repository, the project folder first. DiffLens
/// worktrees also say when they were created and how many files have
/// uncommitted changes. Worktrees whose folder is gone are left out.
pub fn list(git: &Git, project: &Path, home: Option<&Path>) -> AppResult<Vec<Worktree>> {
    let mut worktrees = Vec::new();
    for entry in checkouts(git, project)? {
        let folder = folder(project, &entry.path, home)?;
        let (created_at, changed_files) = if folder.kind == FolderKind::Difflens {
            (
                created_at(&entry.path),
                git.working_tree(&entry.path)
                    .ok()
                    .map(|status| status.files_changed),
            )
        } else {
            (None, None)
        };
        worktrees.push(Worktree {
            folder,
            branch: entry.branch,
            created_at,
            changed_files,
        });
    }
    Ok(worktrees)
}

/// Removes the DiffLens worktree at `path`. Refused while `terminals_in`
/// counts a terminal session open in it, and, without `force`, while it has
/// uncommitted changes (which `force` loses). The branch is never deleted;
/// folders left empty under `.difflens/worktrees/` are.
pub fn remove(
    git: &Git,
    project: &Path,
    path: &Path,
    force: bool,
    terminals_in: impl Fn(&Path) -> usize,
) -> AppResult<()> {
    let path = canonical(path);
    let is_ours = checkouts(git, project)?
        .iter()
        .any(|entry| entry.path == path && kind_of(project, &path) == FolderKind::Difflens);
    if !is_ours {
        return Err(app_error!(
            UnknownWorktree,
            NotADifflensWorktree,
            "{path} is not a worktree DiffLens created.",
            path = path.display()
        ));
    }
    if terminals_in(&path) > 0 {
        return Err(app_error!(
            TerminalsOpen,
            WorktreeHasTerminals,
            "Close the terminals open in {path} before removing it.",
            path = path.display()
        ));
    }
    if !force && git.working_tree(&path)?.dirty {
        return Err(app_error!(
            WorktreeDirty,
            WorktreeDirty,
            "{path} has uncommitted changes, which removing it would lose.",
            path = path.display()
        ));
    }
    git.remove_worktree(project, &path, force)?;
    remove_empty_parents(project, &path);
    Ok(())
}

/// Where the branch's terminals open, from the worktrees listed in `entries`.
fn branch_location(
    git: &Git,
    project: &Path,
    home: Option<&Path>,
    entries: &[WorktreeEntry],
    branch: &str,
) -> AppResult<TerminalLocation> {
    if let Some(entry) = find_checkout(entries, branch) {
        return Ok(TerminalLocation {
            folder: Some(folder(project, &entry.path, home)?),
            branch: Some(branch.to_string()),
            plan: None,
        });
    }
    let upstream = match branch_source(git, project, branch)? {
        Source::Local => None,
        Source::Remote(upstream) => Some(upstream),
    };
    let path = worktree_path(project, branch)?;
    Ok(TerminalLocation {
        folder: None,
        branch: Some(branch.to_string()),
        plan: Some(WorktreePlan {
            relative_path: relative(project, &path)?,
            upstream,
        }),
    })
}

enum Source {
    Local,
    /// The remote-tracking branch, such as `origin/feature/x`.
    Remote(String),
}

/// Whether `branch` is a local branch or only a branch of origin. The name
/// is looked up among the refs git reported, so only a name git knows is
/// ever passed back to it.
fn branch_source(git: &Git, project: &Path, branch: &str) -> AppResult<Source> {
    let refs = git.branch_refs(project)?;
    let has = |kind: RefKind| {
        refs.iter()
            .any(|candidate| candidate.kind == kind && candidate.name == branch)
    };
    if has(RefKind::Local) {
        Ok(Source::Local)
    } else if has(RefKind::Origin) {
        Ok(Source::Remote(format!("{REMOTE}/{branch}")))
    } else {
        Err(unknown_branch(branch))
    }
}

/// The worktrees git lists whose folder exists, with canonical paths so they
/// compare with the (canonical) project path. A bare repository has none.
fn checkouts(git: &Git, project: &Path) -> AppResult<Vec<WorktreeEntry>> {
    Ok(git
        .worktrees(project)?
        .into_iter()
        .filter(|entry| !entry.bare && !entry.prunable && entry.path.is_dir())
        .map(|entry| WorktreeEntry {
            path: canonical(&entry.path),
            ..entry
        })
        .collect())
}

fn find_checkout<'a>(entries: &'a [WorktreeEntry], branch: &str) -> Option<&'a WorktreeEntry> {
    entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(branch))
}

/// `.difflens/worktrees/<branch>` in `project`, each part of the branch name a
/// folder. A name that would leave that folder is refused (git does not allow
/// one, but the path must not depend on it).
fn worktree_path(project: &Path, branch: &str) -> AppResult<PathBuf> {
    let mut path = worktrees_root(project);
    let relative = Path::new(branch);
    let only_names = !branch.is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !only_names {
        return Err(app_error!(
            Internal,
            WorktreeBranchNotAFolder,
            "\"{branch}\" cannot be used as a folder name.",
            branch = escaped(branch)
        ));
    }
    path.push(relative);
    Ok(path)
}

fn kind_of(project: &Path, path: &Path) -> FolderKind {
    if path == project {
        FolderKind::Project
    } else if path.starts_with(worktrees_root(project)) {
        FolderKind::Difflens
    } else {
        FolderKind::Worktree
    }
}

fn folder(project: &Path, path: &Path, home: Option<&Path>) -> AppResult<Folder> {
    let kind = kind_of(project, path);
    let text = path_text(path)?;
    Ok(Folder {
        display_path: display_path(&text, home),
        relative_path: match kind {
            FolderKind::Difflens => Some(relative(project, path)?),
            _ => None,
        },
        path: text,
        kind,
    })
}

/// `path` inside `project`, with `/` separators whatever the system.
fn relative(project: &Path, path: &Path) -> AppResult<String> {
    let inside = path.strip_prefix(project).unwrap_or(path);
    Ok(inside
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// When the folder was created, or last changed where the system does not
/// record creation.
fn created_at(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let time = metadata.created().or_else(|_| metadata.modified()).ok()?;
    OffsetDateTime::from(time).format(&Rfc3339).ok()
}

/// Deletes the folders `path` leaves empty, up to `.difflens/worktrees/`
/// (kept). A folder that is not empty stops it.
fn remove_empty_parents(project: &Path, path: &Path) {
    let root = worktrees_root(project);
    let mut current = path.parent();
    while let Some(folder) = current {
        if folder == root || !folder.starts_with(&root) || fs::remove_dir(folder).is_err() {
            break;
        }
        current = folder.parent();
    }
}

fn read_failed(path: &Path, error: &std::io::Error) -> AppError {
    app_error!(
        Internal,
        FileReadFailed,
        "Could not read {path}: {detail}",
        path = path.display(),
        detail = error
    )
}

fn write_failed(path: &Path, error: &std::io::Error) -> AppError {
    app_error!(
        Internal,
        FileWriteFailed,
        "Could not write {path}: {detail}",
        path = path.display(),
        detail = error
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::projects::ProjectService;
    use crate::store::Store;
    use crate::test_support::{isolated_git, run_git, TestRepo};

    /// A clone of a bare origin with `main` checked out, the local branch
    /// `feature/local` and `feature/remote` only on origin.
    struct Setup {
        repo: TestRepo,
        _origin: TestRepo,
    }

    fn setup() -> Setup {
        let source = TestRepo::init();
        source.commit_file("README.md", "hello\n", "initial commit");
        source.git(&["branch", "feature/local"]);
        source.git(&["branch", "feature/remote"]);
        let bare = source.parent().join("origin.git");
        run_git(
            source.parent(),
            &["clone", "--quiet", "--bare", source.path().to_str().unwrap(), bare.to_str().unwrap()],
        );
        let repo = TestRepo::clone_from(&bare);
        repo.git(&["branch", "feature/local", "origin/feature/local"]);
        Setup { repo, _origin: source }
    }

    fn branch(name: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: name.to_string(),
            base: "main".to_string(),
        }
    }

    fn exclude_of(repo: &TestRepo) -> String {
        fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap_or_default()
    }

    #[test]
    fn the_project_screen_and_the_uncommitted_changes_open_in_the_project_folder() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        for target in [None, Some(&ReviewTarget::WorkingTree)] {
            let location = location(&git, repo.path(), None, target).unwrap();
            let folder = location.folder.unwrap();
            assert_eq!(folder.path, repo.path().to_str().unwrap());
            assert_eq!(folder.kind, FolderKind::Project);
            assert_eq!(folder.relative_path, None);
            assert_eq!(location.branch.as_deref(), Some("main"));
            assert_eq!(location.plan, None);
        }
    }

    #[test]
    fn a_branch_opens_where_it_is_checked_out() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        let main = location(&git, repo.path(), None, Some(&branch("main"))).unwrap();
        assert_eq!(main.folder.unwrap().kind, FolderKind::Project);

        let elsewhere = repo.parent().join("my own worktree");
        repo.git(&["worktree", "add", "--quiet", elsewhere.to_str().unwrap(), "feature/local"]);
        let local = location(&git, repo.path(), Some(Path::new("/nowhere")), Some(&branch("feature/local"))).unwrap();
        let folder = local.folder.unwrap();
        assert_eq!(folder.kind, FolderKind::Worktree);
        assert_eq!(folder.path, elsewhere.canonicalize().unwrap().to_str().unwrap());
        assert_eq!(local.plan, None);
    }

    #[test]
    fn a_branch_checked_out_nowhere_has_a_plan_and_no_folder() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();

        let local = location(&git, repo.path(), None, Some(&branch("feature/local"))).unwrap();
        assert_eq!(local.folder, None);
        assert_eq!(local.branch.as_deref(), Some("feature/local"));
        assert_eq!(
            local.plan,
            Some(WorktreePlan {
                relative_path: ".difflens/worktrees/feature/local".to_string(),
                upstream: None,
            })
        );

        let remote = location(&git, repo.path(), None, Some(&branch("feature/remote"))).unwrap();
        assert_eq!(remote.plan.unwrap().upstream.as_deref(), Some("origin/feature/remote"));

        let unknown = location(&git, repo.path(), None, Some(&branch("feature/none"))).unwrap_err();
        assert_eq!(unknown.code, ErrorCode::UnknownBranch);
        assert!(!repo.path().join(DIFFLENS_FOLDER).exists(), "looking creates nothing");
    }

    #[test]
    fn the_exclude_line_is_added_once_and_keeps_what_was_there() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        let exclude = repo.path().join(".git/info/exclude");
        fs::write(&exclude, "# mine\n*.log").unwrap();

        ensure_excluded(&git, repo.path()).unwrap();
        ensure_excluded(&git, repo.path()).unwrap();
        assert_eq!(exclude_of(&repo), "# mine\n*.log\n.difflens/\n");

        fs::remove_dir_all(repo.path().join(".git/info")).unwrap();
        ensure_excluded(&git, repo.path()).unwrap();
        assert_eq!(exclude_of(&repo), ".difflens/\n");

        fs::write(&exclude, "/.difflens\n").unwrap();
        ensure_excluded(&git, repo.path()).unwrap();
        assert_eq!(exclude_of(&repo), "/.difflens\n", "an equivalent line is enough");
    }

    #[test]
    fn the_exclude_file_is_the_shared_one_when_the_project_is_a_linked_worktree() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        let linked = repo.parent().join("linked");
        repo.git(&["worktree", "add", "--quiet", linked.to_str().unwrap(), "feature/local"]);

        ensure_excluded(&git, &linked).unwrap();

        let exclude = exclude_of(&repo);
        assert!(exclude.ends_with("\n.difflens/\n"), "{exclude:?}");
        assert_eq!(exclude.matches(".difflens/").count(), 1);
        assert!(!linked.join(".git").is_dir(), "a linked worktree has a .git file, not a folder");
    }

    #[test]
    fn creating_a_worktree_of_a_local_branch_leaves_the_project_folder_alone() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        let head = repo.git(&["rev-parse", "HEAD"]);

        let created = create(&git, repo.path(), None, "feature/local").unwrap();

        let folder = created.folder.unwrap();
        let path = repo.path().join(".difflens/worktrees/feature/local");
        assert_eq!(folder.path, path.to_str().unwrap());
        assert_eq!(folder.kind, FolderKind::Difflens);
        assert_eq!(folder.relative_path.as_deref(), Some(".difflens/worktrees/feature/local"));
        assert_eq!(run_git(&path, &["branch", "--show-current"]), "feature/local");
        assert!(path.join("README.md").is_file());
        // The project folder: same branch and commit, nothing to commit, no
        // untracked .difflens/.
        assert_eq!(repo.git(&["branch", "--show-current"]), "main");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(repo.git(&["status", "--porcelain", "--untracked-files=all"]), "");
        assert!(git.untracked_files(repo.path(), &[]).unwrap().is_empty());
        assert_eq!(exclude_of(&repo).matches(".difflens/").count(), 1);

        let again = create(&git, repo.path(), None, "feature/local").unwrap();
        assert_eq!(again.folder.unwrap().path, folder.path, "creating twice opens the same one");
        let found = location(&git, repo.path(), None, Some(&branch("feature/local"))).unwrap();
        assert_eq!(found.folder.unwrap().kind, FolderKind::Difflens);
    }

    #[test]
    fn creating_a_worktree_of_a_remote_only_branch_creates_a_tracking_branch() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        assert!(repo.git(&["branch", "--list", "feature/remote"]).is_empty());

        let created = create(&git, repo.path(), None, "feature/remote").unwrap();

        let path = PathBuf::from(created.folder.unwrap().path);
        assert_eq!(run_git(&path, &["branch", "--show-current"]), "feature/remote");
        assert_eq!(
            repo.git(&["rev-parse", "--abbrev-ref", "feature/remote@{upstream}"]),
            "origin/feature/remote"
        );
        assert_eq!(repo.git(&["branch", "--show-current"]), "main");
    }

    #[test]
    fn a_taken_folder_or_an_unknown_branch_creates_nothing() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        fs::create_dir_all(repo.path().join(".difflens/worktrees/feature/local")).unwrap();

        let taken = create(&git, repo.path(), None, "feature/local").unwrap_err();
        assert_eq!(taken.code, ErrorCode::DestinationExists);
        let unknown = create(&git, repo.path(), None, "feature/none").unwrap_err();
        assert_eq!(unknown.code, ErrorCode::UnknownBranch);
        assert_eq!(exclude_of(&repo).matches(".difflens/").count(), 0);
        assert!(worktree_path(repo.path(), "../escape").is_err());
    }

    #[test]
    fn worktrees_are_listed_with_their_state() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        create(&git, repo.path(), None, "feature/local").unwrap();
        let path = repo.path().join(".difflens/worktrees/feature/local");

        let listed = list(&git, repo.path(), None).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].folder.kind, FolderKind::Project);
        assert_eq!(listed[0].branch.as_deref(), Some("main"));
        assert_eq!(listed[0].changed_files, None);
        let ours = &listed[1];
        assert_eq!(ours.folder.kind, FolderKind::Difflens);
        assert_eq!(ours.branch.as_deref(), Some("feature/local"));
        assert_eq!(ours.changed_files, Some(0));
        assert!(OffsetDateTime::parse(ours.created_at.as_deref().unwrap(), &Rfc3339).is_ok());

        fs::write(path.join("README.md"), "changed\n").unwrap();
        fs::write(path.join("new.txt"), "new\n").unwrap();
        assert_eq!(list(&git, repo.path(), None).unwrap()[1].changed_files, Some(2));
    }

    #[test]
    fn removing_a_worktree_asks_for_force_when_dirty_and_never_deletes_the_branch() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        create(&git, repo.path(), None, "feature/local").unwrap();
        create(&git, repo.path(), None, "feature/remote").unwrap();
        let local = repo.path().join(".difflens/worktrees/feature/local");
        let remote = repo.path().join(".difflens/worktrees/feature/remote");
        let none = |_: &Path| 0;

        remove(&git, repo.path(), &local, false, none).unwrap();
        assert!(!local.exists());
        assert!(repo.path().join(".difflens/worktrees/feature").is_dir(), "a sibling is still there");
        assert_eq!(repo.git(&["branch", "--list", "feature/local"]), "feature/local");

        fs::write(remote.join("README.md"), "changed\n").unwrap();
        let dirty = remove(&git, repo.path(), &remote, false, none).unwrap_err();
        assert_eq!(dirty.code, ErrorCode::WorktreeDirty);
        assert!(remote.exists());
        remove(&git, repo.path(), &remote, true, none).unwrap();
        assert!(!remote.exists());
        assert!(!repo.path().join(".difflens/worktrees/feature").exists(), "empty folders go");
        assert!(repo.path().join(".difflens/worktrees").is_dir());
        assert_eq!(repo.git(&["branch", "--list", "feature/remote"]), "feature/remote");
        assert_eq!(list(&git, repo.path(), None).unwrap().len(), 1);
    }

    #[test]
    fn removing_is_refused_with_a_terminal_open_and_for_other_folders() {
        let Setup { repo, .. } = setup();
        let git = isolated_git();
        create(&git, repo.path(), None, "feature/local").unwrap();
        let local = repo.path().join(".difflens/worktrees/feature/local");

        let busy = remove(&git, repo.path(), &local, true, |folder| usize::from(folder == local)).unwrap_err();
        assert_eq!(busy.code, ErrorCode::TerminalsOpen);
        assert!(local.exists());

        let project = remove(&git, repo.path(), repo.path(), true, |_| 0).unwrap_err();
        assert_eq!(project.code, ErrorCode::UnknownWorktree);
        let elsewhere = repo.parent().join("mine");
        repo.git(&["worktree", "add", "--quiet", elsewhere.to_str().unwrap(), "-b", "mine"]);
        let theirs = remove(&git, repo.path(), &elsewhere, true, |_| 0).unwrap_err();
        assert_eq!(theirs.code, ErrorCode::UnknownWorktree);
        assert!(elsewhere.exists());
    }

    #[test]
    fn the_project_screen_still_sees_only_its_own_checkout() {
        let Setup { repo, .. } = setup();
        let service = ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None);
        repo.git(&["checkout", "--quiet", "feature/local"]);
        let project = service.add_local(repo.path().to_str().unwrap()).unwrap();

        service.create_worktree(&project.id, "feature/remote").unwrap();

        let detail = service.detail(&project.id).unwrap();
        let current: Vec<&str> = detail
            .branches
            .iter()
            .filter(|branch| branch.is_current)
            .map(|branch| branch.name.as_str())
            .collect();
        assert_eq!(current, ["feature/local"]);
        assert!(!detail.working_tree.dirty);
        assert_eq!(detail.worktrees.len(), 2);
        assert_eq!(detail.worktrees[1].branch.as_deref(), Some("feature/remote"));
        let summary = service.compare_working_tree(&project.id).unwrap();
        assert_eq!(summary.files_changed, 0);
        let changes = service.changed_files(&project.id, ReviewTarget::WorkingTree).unwrap();
        assert!(changes.files.is_empty());
        let kind = |name: &str| {
            service
                .terminal_location(&project.id, Some(&branch(name)))
                .unwrap()
                .folder
                .unwrap()
                .kind
        };
        assert_eq!(kind("feature/local"), FolderKind::Project);
        assert_eq!(kind("feature/remote"), FolderKind::Difflens);
    }
}
