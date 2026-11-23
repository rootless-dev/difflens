//! The files a review looks at and both versions of each one.
//!
//! Listing costs a fixed number of git processes whatever the number of
//! files. Reading one file is validated without keeping any state: both sides
//! must be commits of the repository, the paths must be plain relative paths,
//! and git must report that file as changed between those two sides. Nothing
//! here writes to the repository or its index.

use std::ffi::OsStr;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, escaped, AppError, AppResult};
use crate::file_language;
use crate::fingerprint;
use crate::git::{Blob, ChangeKind, FileChange, Git, TreeEntry, GITLINK_MODE};
use crate::line_count::{self, AddedFile};
use crate::model::{ChangedFile, FileSide, FileStatus, FileVersions, SideReason};

/// Text above this size is not returned.
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// Files changed from `old_sha` to `new_sha`, sorted by path.
pub fn between_commits(
    git: &Git,
    repo: &Path,
    old_sha: &str,
    new_sha: &str,
) -> AppResult<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = git
        .diff_files(repo, old_sha, Some(new_sha), &[])?
        .iter()
        .map(changed_file)
        .collect();
    sort_by_path(&mut files);
    Ok(files)
}

/// Uncommitted changes against `old_sha` (HEAD, or the empty tree before the
/// first commit): tracked files, staged or not, then untracked files that are
/// not ignored, as added. Counted as `compare_working_tree` counts them.
pub fn in_working_tree(git: &Git, repo: &Path, old_sha: &str) -> AppResult<Vec<ChangedFile>> {
    let mut files: Vec<ChangedFile> = git
        .diff_files(repo, old_sha, None, &[])?
        .iter()
        .map(changed_file)
        .collect();
    for untracked in git.untracked_files(repo, &[])? {
        // A file that cannot be read still counts as added, with no lines.
        let added = line_count::added_file(&repo.join(&untracked)).unwrap_or(AddedFile {
            lines: 0,
            binary: false,
        });
        // An untracked nested repository is reported with a trailing `/`.
        let path = untracked
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        files.push(ChangedFile {
            language: file_language::of_path(&path),
            path,
            old_path: None,
            status: FileStatus::Added,
            additions: added.lines,
            deletions: 0,
            binary: added.binary,
        });
    }
    sort_by_path(&mut files);
    Ok(files)
}

/// Both sides of `path`: at `old_sha` (under `old_path` for a rename), and at
/// `new_sha`, or on disk when `new_sha` is `None`.
///
/// Fails with `unknown_file` when a path is not a plain relative path inside
/// the repository, or when git does not report that file (with that rename)
/// as changed between the two sides; with `unknown_branch` when a sha is not
/// a commit of this repository.
pub fn file_versions(
    git: &Git,
    repo: &Path,
    old_sha: &str,
    new_sha: Option<&str>,
    path: &str,
    old_path: Option<&str>,
) -> AppResult<FileVersions> {
    check_path(path)?;
    // An old path equal to the path is no rename.
    let old_path = old_path.filter(|old| *old != path);
    if let Some(old) = old_path {
        check_path(old)?;
    }
    check_commits(git, repo, old_sha, new_sha)?;

    let mut pathspecs = vec![path];
    pathspecs.extend(old_path);
    let changes = git.diff_files(repo, old_sha, new_sha, &pathspecs)?;
    let listed = changes.iter().find(|change| {
        change.path == path.as_bytes() && listed_source(change) == old_path.map(str::as_bytes)
    });
    let not_listed = || {
        app_error!(
            UnknownFile,
            NotAChangedFile,
            "\"{path}\" is not a changed file of this review.",
            path = escaped(path)
        )
    };
    let (old, new) = match listed {
        Some(change) => {
            // Limited to these paths, git sees one side of a rename as a
            // plain addition or deletion; the whole diff lists it as a rename.
            if is_one_side_of_a_rename(git, repo, old_sha, new_sha, change)? {
                return Err(not_listed());
            }
            sources(change, new_sha.is_none())
        }
        None if new_sha.is_none() && old_path.is_none() && is_untracked(git, repo, path)? => {
            (Source::Absent, Source::Disk)
        }
        None => return Err(not_listed()),
    };

    let mut sides = read_sources(git, repo, vec![(path, old), (path, new)])?.into_iter();
    match (sides.next(), sides.next()) {
        (Some(old), Some(new)) => Ok(FileVersions { old, new }),
        _ => Err(app_error!(
            Internal,
            TooFewSidesForFile,
            "Reading the file returned too few sides."
        )),
    }
}

/// Where one side of a file comes from.
#[derive(Debug, Clone)]
pub enum Source {
    Absent,
    Object(TreeEntry),
    Disk,
}

/// Reads one side of each file, in order. All the blobs are read with the
/// same two git processes, whatever their number.
pub fn read_sources(
    git: &Git,
    repo: &Path,
    sources: Vec<(&str, Source)>,
) -> AppResult<Vec<FileSide>> {
    let oids: Vec<&str> = sources
        .iter()
        .filter_map(|(_, source)| match source {
            Source::Object(entry) if entry.mode != GITLINK_MODE => Some(entry.oid.as_str()),
            _ => None,
        })
        .collect();
    let mut blobs = git.read_blobs(repo, &oids, MAX_TEXT_BYTES)?.into_iter();
    sources
        .into_iter()
        .map(|(path, source)| read_side(git, repo, path, source, &mut blobs))
        .collect()
}

/// Where the old and the new side of a listed change come from; the new side
/// is read from disk when `from_disk`.
pub fn sources(change: &FileChange, from_disk: bool) -> (Source, Source) {
    let object = |entry: &Option<TreeEntry>| entry.clone().map_or(Source::Absent, Source::Object);
    let old = match change.kind {
        // A copy is listed as an added file.
        ChangeKind::Added | ChangeKind::Copied => Source::Absent,
        _ => object(&change.old),
    };
    let new = match change.kind {
        ChangeKind::Deleted => Source::Absent,
        _ if from_disk => Source::Disk,
        _ => object(&change.new),
    };
    (old, new)
}

fn read_side(
    git: &Git,
    repo: &Path,
    path: &str,
    source: Source,
    blobs: &mut impl Iterator<Item = Blob>,
) -> AppResult<FileSide> {
    match source {
        Source::Absent => Ok(absent()),
        Source::Object(entry) if entry.mode == GITLINK_MODE => {
            Ok(side_from_bytes(submodule_text(&entry.oid)))
        }
        Source::Object(entry) => match blobs.next() {
            Some(Blob::Content(content)) => Ok(side_from_bytes(content)),
            Some(Blob::TooLarge(size)) => Ok(too_large(size)),
            Some(Blob::Missing) | None => Err(app_error!(
                Internal,
                BlobMissing,
                "The content of \"{path}\" ({id}) is missing from the repository.",
                path = escaped(path),
                id = &entry.oid
            )),
        },
        Source::Disk => read_disk(git, repo, path),
    }
}

/// Reads `path` under `repo` the way git sees it: a symlink is never
/// followed, neither as the file itself (its target is the content, as git
/// stores it) nor as one of the folders above it (git would not look through
/// it, so the file is absent).
fn read_disk(git: &Git, repo: &Path, path: &str) -> AppResult<FileSide> {
    match disk_entry(repo, path)? {
        Some((full, metadata)) => read_entry(git, &full, &metadata),
        None => Ok(absent()),
    }
}

/// Fingerprint of `path` under `repo`, read as `read_disk` reads it but
/// whatever its size: its content, a symlink's target, or the commit checked
/// out in a nested repository. `None` when git would see no file there.
pub fn disk_fingerprint(git: &Git, repo: &Path, path: &str) -> AppResult<Option<String>> {
    let Some((full, metadata)) = disk_entry(repo, path)? else {
        return Ok(None);
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(&full).map_err(|error| disk_error(&full, &error))?;
        return Ok(Some(fingerprint::of_bytes(&os_bytes(target.as_os_str()))));
    }
    if file_type.is_file() {
        let file = File::open(&full).map_err(|error| disk_error(&full, &error))?;
        return fingerprint::of_reader(file)
            .map(Some)
            .map_err(|error| disk_error(&full, &error));
    }
    if file_type.is_dir() {
        return Ok(submodule_head(git, &full)?.map(|head| fingerprint::of_bytes(head.as_bytes())));
    }
    Ok(None)
}

/// The entry at `path` under `repo` and its metadata, or `None` when there
/// is none or it sits behind a symlinked folder, which git would not look
/// through.
fn disk_entry(repo: &Path, path: &str) -> AppResult<Option<(PathBuf, Metadata)>> {
    let mut full = repo.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        full.push(part);
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(disk_error(&full, &error)),
        };
        if parts.peek().is_none() {
            return Ok(Some((full, metadata)));
        }
        // symlink_metadata does not follow links: a symlink is not a folder.
        if !metadata.is_dir() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn read_entry(git: &Git, full: &Path, metadata: &Metadata) -> AppResult<FileSide> {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(full).map_err(|error| disk_error(full, &error))?;
        return Ok(side_from_bytes(os_bytes(target.as_os_str())));
    }
    if file_type.is_file() {
        if metadata.len() > MAX_TEXT_BYTES {
            return Ok(too_large(metadata.len()));
        }
        let mut content = Vec::new();
        File::open(full)
            // One byte more than allowed tells a file that grew meanwhile.
            .and_then(|file| file.take(MAX_TEXT_BYTES + 1).read_to_end(&mut content))
            .map_err(|error| disk_error(full, &error))?;
        let size = byte_count(&content);
        if size > MAX_TEXT_BYTES {
            return Ok(too_large(size));
        }
        return Ok(side_from_bytes(content));
    }
    if file_type.is_dir() {
        // A submodule, or an untracked nested repository: shown as git shows
        // a gitlink, by the commit checked out in it.
        return Ok(match submodule_head(git, full)? {
            Some(head) => side_from_bytes(submodule_text(&head)),
            None => absent(),
        });
    }
    Ok(absent())
}

/// Commit checked out in the repository whose root is `folder`, or `None`
/// when `folder` is not the root of a repository with commits.
fn submodule_head(git: &Git, folder: &Path) -> AppResult<Option<String>> {
    let Some(root) = git.toplevel(folder)? else {
        return Ok(None);
    };
    let is_root = match (fs::canonicalize(&root), fs::canonicalize(folder)) {
        (Ok(root), Ok(folder)) => root == folder,
        _ => false,
    };
    if !is_root {
        return Ok(None);
    }
    git.head_commit(folder)
}

/// What `git diff` shows for a gitlink.
fn submodule_text(commit: &str) -> Vec<u8> {
    format!("Subproject commit {commit}\n").into_bytes()
}

fn is_untracked(git: &Git, repo: &Path, path: &str) -> AppResult<bool> {
    // The pathspec also matches files inside a folder of that name.
    Ok(git
        .untracked_files(repo, &[path])?
        .iter()
        .any(|untracked| untracked == Path::new(path)))
}

/// True when `change`, an addition or a deletion found in a diff limited to
/// its path, is the destination or the source of a rename in the whole diff.
fn is_one_side_of_a_rename(
    git: &Git,
    repo: &Path,
    old_sha: &str,
    new_sha: Option<&str>,
    change: &FileChange,
) -> AppResult<bool> {
    if !matches!(change.kind, ChangeKind::Added | ChangeKind::Deleted) {
        return Ok(false);
    }
    Ok(git.renames(repo, old_sha, new_sha)?.iter().any(|rename| {
        rename.path == change.path || rename.source.as_deref() == Some(change.path.as_slice())
    }))
}

/// The old path under which a change is listed: only renames have one.
fn listed_source(change: &FileChange) -> Option<&[u8]> {
    match change.kind {
        ChangeKind::Renamed => change.source.as_deref(),
        _ => None,
    }
}

/// A repository path: relative, `/`-separated, with no empty, `.` or `..`
/// part, so it can only name something inside the repository.
pub fn check_path(path: &str) -> AppResult<()> {
    let plain = !path.is_empty()
        && !path.contains('\0')
        && !(cfg!(windows) && path.contains('\\'))
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(app_error!(
            UnknownFile,
            PathOutsideRepository,
            "\"{path}\" is not a path inside the repository.",
            path = escaped(path)
        ))
    }
}

/// Both shas must be commits of the repository. Before the first commit the
/// working tree is compared with the empty tree, which is accepted as the old
/// side of a working tree review.
pub fn check_commits(
    git: &Git,
    repo: &Path,
    old_sha: &str,
    new_sha: Option<&str>,
) -> AppResult<()> {
    let mut shas = vec![old_sha];
    shas.extend(new_sha);
    let found = git.are_commits(repo, &shas)?;
    for (sha, found) in shas.into_iter().zip(found) {
        if found {
            continue;
        }
        let empty_tree_side =
            new_sha.is_none() && sha == old_sha && sha == git.empty_tree(repo)?.as_str();
        if !empty_tree_side {
            return Err(app_error!(
                UnknownBranch,
                UnknownCommit,
                "\"{commit}\" is not a commit of this repository.",
                commit = escaped(sha)
            ));
        }
    }
    Ok(())
}

/// How the change set lists a change: path, status and counts.
pub fn changed_file(change: &FileChange) -> ChangedFile {
    let path = String::from_utf8_lossy(&change.path).into_owned();
    let (status, old_path) = match change.kind {
        ChangeKind::Added | ChangeKind::Copied => (FileStatus::Added, None),
        ChangeKind::Modified | ChangeKind::Unmerged => (FileStatus::Modified, None),
        ChangeKind::Deleted => (FileStatus::Deleted, None),
        ChangeKind::Renamed => (
            FileStatus::Renamed,
            change
                .source
                .as_deref()
                .map(|source| String::from_utf8_lossy(source).into_owned()),
        ),
        ChangeKind::TypeChanged => (FileStatus::TypeChanged, None),
    };
    ChangedFile {
        language: file_language::of_path(&path),
        path,
        old_path,
        status,
        additions: change.additions,
        deletions: change.deletions,
        binary: change.binary,
    }
}

/// Stable, so a path both deleted from the index and present as an untracked
/// file keeps the tracked entry first.
fn sort_by_path(files: &mut [ChangedFile]) {
    files.sort_by(|left, right| left.path.cmp(&right.path));
}

fn side_from_bytes(content: Vec<u8>) -> FileSide {
    let size = byte_count(&content);
    if line_count::looks_binary(&content) {
        return without_text(SideReason::Binary, size);
    }
    match String::from_utf8(content) {
        Ok(text) => FileSide {
            text: Some(text),
            reason: None,
            size,
        },
        Err(_) => without_text(SideReason::NotUtf8, size),
    }
}

fn absent() -> FileSide {
    without_text(SideReason::Absent, 0)
}

fn too_large(size: u64) -> FileSide {
    without_text(SideReason::TooLarge, size)
}

fn without_text(reason: SideReason, size: u64) -> FileSide {
    FileSide {
        text: None,
        reason: Some(reason),
        size,
    }
}

fn byte_count(content: &[u8]) -> u64 {
    u64::try_from(content.len()).unwrap_or(u64::MAX)
}

fn disk_error(path: &Path, error: &io::Error) -> AppError {
    app_error!(
        Internal,
        FileReadFailed,
        "Could not read {path}: {detail}",
        path = path.display(),
        detail = error
    )
}

#[cfg(unix)]
fn os_bytes(text: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    text.as_bytes().to_vec()
}

/// git stores symlink targets with `/` separators on other systems.
#[cfg(not(unix))]
fn os_bytes(text: &OsStr) -> Vec<u8> {
    text.to_string_lossy().replace('\\', "/").into_bytes()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{ChangeSet, ReviewBase, ReviewBaseKind, ReviewTarget};
    use crate::projects::ProjectService;
    use crate::store::Store;
    use crate::test_support::{isolated_git, run_git, TestRepo};

    const MAX: usize = 2 * 1024 * 1024;

    fn service() -> ProjectService {
        ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None)
    }

    fn add(service: &ProjectService, repo: &TestRepo) -> String {
        service.add_local(repo.path().to_str().unwrap()).unwrap().id
    }

    fn branch(branch: &str, base: &str) -> ReviewTarget {
        ReviewTarget::Branch {
            branch: branch.to_string(),
            base: base.to_string(),
        }
    }

    fn code<T: std::fmt::Debug>(result: AppResult<T>) -> ErrorCode {
        result.unwrap_err().code
    }

    #[cfg(unix)]
    fn symlink(target: &str, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn symlink(target: &str, link: &Path) {
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }

    type Row = (
        String,
        Option<String>,
        FileStatus,
        u32,
        u32,
        bool,
        Option<&'static str>,
    );

    fn rows(change_set: &ChangeSet) -> Vec<Row> {
        change_set
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    file.old_path.clone(),
                    file.status,
                    file.additions,
                    file.deletions,
                    file.binary,
                    file.language,
                )
            })
            .collect()
    }

    fn row(
        path: &str,
        status: FileStatus,
        additions: u32,
        deletions: u32,
        binary: bool,
        language: Option<&'static str>,
    ) -> Row {
        (
            path.to_string(),
            None,
            status,
            additions,
            deletions,
            binary,
            language,
        )
    }

    fn text(content: &str) -> FileSide {
        FileSide {
            text: Some(content.to_string()),
            reason: None,
            size: content.len() as u64,
        }
    }

    fn no_text(reason: SideReason, size: u64) -> FileSide {
        FileSide {
            text: None,
            reason: Some(reason),
            size,
        }
    }

    fn sides(old: FileSide, new: FileSide) -> FileVersions {
        FileVersions { old, new }
    }

    fn totals(change_set: &ChangeSet) -> (u32, u32, u32) {
        (
            change_set.files.len() as u32,
            change_set.files.iter().map(|file| file.additions).sum(),
            change_set.files.iter().map(|file| file.deletions).sum(),
        )
    }

    fn big(extra: usize) -> String {
        format!("{}\n", "a".repeat(MAX - 1 + extra))
    }

    /// `feature/review` changes one file of every kind on top of `main`,
    /// which moved on after the branch was cut.
    fn review_repo() -> TestRepo {
        let repo = TestRepo::init();
        repo.write("src/app.go", "package app\n\nfunc A() {}\n");
        repo.write("gone.txt", "bye\nbye\n");
        let old_doc: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        repo.write("docs/old name.md", &old_doc);
        repo.write("kind.txt", "plain file\n");
        repo.write("image.bin", [0u8, 1, 2, 3]);
        repo.write("crlf.txt", "one\r\ntwo\r\n");
        repo.write("same.txt", "unchanged\n");
        repo.write("latin1.txt", b"caf\xe9\n");
        repo.write("big.txt", big(0));
        repo.commit_all("initial commit");

        repo.git(&["checkout", "--quiet", "-b", "feature/review"]);
        repo.write("src/app.go", "package app\n\nfunc A() {}\n\nfunc B() {}\n");
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        fs::remove_file(repo.path().join("docs/old name.md")).unwrap();
        repo.write(
            "docs/new name.md",
            old_doc.replace("line 7\n", "line seven\n"),
        );
        fs::remove_file(repo.path().join("kind.txt")).unwrap();
        symlink("src/app.go", &repo.path().join("kind.txt"));
        repo.write("image.bin", [0u8, 9, 9]);
        repo.write("crlf.txt", "one\r\nTWO\r\n");
        repo.write("latin1.txt", b"caf\xe9s\n");
        repo.write("big.txt", big(1));
        repo.write("-dash ü file.ts", "export const x = 1;\n");
        repo.write("dir with space/new:colon.py", "print(1)\n");
        repo.commit_all("every kind of change");

        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit_file("main-only.txt", "main\n", "main moves on");
        repo
    }

    #[test]
    fn a_branch_lists_every_kind_of_change_from_the_merge_base() {
        let repo = review_repo();
        let service = service();
        let id = add(&service, &repo);

        let change_set = service
            .changed_files(&id, branch("feature/review", "main"))
            .unwrap();

        assert_eq!(change_set.target, branch("feature/review", "main"));
        assert_eq!(
            change_set.old_sha,
            repo.git(&["merge-base", "main", "feature/review"])
        );
        assert_eq!(
            change_set.new_sha,
            Some(repo.git(&["rev-parse", "feature/review"]))
        );
        let mut renamed = row(
            "docs/new name.md",
            FileStatus::Renamed,
            1,
            1,
            false,
            Some("markdown"),
        );
        renamed.1 = Some("docs/old name.md".to_string());
        assert_eq!(
            rows(&change_set),
            vec![
                row(
                    "-dash ü file.ts",
                    FileStatus::Added,
                    1,
                    0,
                    false,
                    Some("typescript")
                ),
                row("big.txt", FileStatus::Modified, 1, 1, false, None),
                row("crlf.txt", FileStatus::Modified, 1, 1, false, None),
                row(
                    "dir with space/new:colon.py",
                    FileStatus::Added,
                    1,
                    0,
                    false,
                    Some("python")
                ),
                renamed,
                row("gone.txt", FileStatus::Deleted, 0, 2, false, None),
                row("image.bin", FileStatus::Modified, 0, 0, true, None),
                row("kind.txt", FileStatus::TypeChanged, 1, 1, false, None),
                row("latin1.txt", FileStatus::Modified, 1, 1, false, None),
                row("src/app.go", FileStatus::Modified, 2, 0, false, Some("go")),
            ]
        );
    }

    #[test]
    fn listed_totals_equal_compare_branches() {
        let repo = review_repo();
        repo.git(&["branch", "develop", "main"]);
        repo.git(&["checkout", "--quiet", "-b", "feature/second", "main"]);
        repo.write("main-only.txt", "main\nchanged\n");
        repo.write("more/a.rs", "fn main() {}\n");
        repo.commit_all("second branch");
        let service = service();
        let id = add(&service, &repo);

        for (name, base) in [
            ("feature/review", "main"),
            ("feature/review", "develop"),
            ("feature/second", "main"),
            ("main", "main"),
        ] {
            let change_set = service.changed_files(&id, branch(name, base)).unwrap();
            let summary = service.compare(&id, name, base).unwrap();
            assert_eq!(
                totals(&change_set),
                (summary.files_changed, summary.additions, summary.deletions),
                "{name} against {base}"
            );
            assert_eq!(change_set.old_sha, summary.merge_base_sha);
            assert_eq!(
                change_set.new_sha.as_deref(),
                Some(summary.head_sha.as_str())
            );
        }
    }

    #[test]
    fn both_versions_of_each_changed_file_of_a_branch() {
        let repo = review_repo();
        let service = service();
        let id = add(&service, &repo);
        let change_set = service
            .changed_files(&id, branch("feature/review", "main"))
            .unwrap();
        let versions = |path: &str, old_path: Option<&str>| {
            service
                .file_versions(
                    &id,
                    &change_set.old_sha,
                    change_set.new_sha.as_deref(),
                    path,
                    old_path,
                )
                .unwrap()
        };

        assert_eq!(
            versions("src/app.go", None),
            sides(
                text("package app\n\nfunc A() {}\n"),
                text("package app\n\nfunc A() {}\n\nfunc B() {}\n")
            )
        );
        let renamed = versions("docs/new name.md", Some("docs/old name.md"));
        assert!(renamed.old.text.as_deref().unwrap().contains("line 7\n"));
        assert!(renamed
            .new
            .text
            .as_deref()
            .unwrap()
            .contains("line seven\n"));
        assert_eq!(
            versions("-dash ü file.ts", None),
            sides(
                no_text(SideReason::Absent, 0),
                text("export const x = 1;\n")
            )
        );
        assert_eq!(
            versions("dir with space/new:colon.py", None).new,
            text("print(1)\n")
        );
        assert_eq!(
            versions("gone.txt", None),
            sides(text("bye\nbye\n"), no_text(SideReason::Absent, 0))
        );
        assert_eq!(
            versions("image.bin", None),
            sides(
                no_text(SideReason::Binary, 4),
                no_text(SideReason::Binary, 3)
            )
        );
        // The symlink is shown as git stores it: its target, as text.
        assert_eq!(
            versions("kind.txt", None),
            sides(text("plain file\n"), text("src/app.go"))
        );
        assert_eq!(
            versions("crlf.txt", None),
            sides(text("one\r\ntwo\r\n"), text("one\r\nTWO\r\n"))
        );
        assert_eq!(
            versions("latin1.txt", None),
            sides(
                no_text(SideReason::NotUtf8, 5),
                no_text(SideReason::NotUtf8, 6)
            )
        );
        // Exactly 2 MB is still shown; one byte more is not.
        let big = versions("big.txt", None);
        assert_eq!(big.old.size, MAX as u64);
        assert_eq!(big.old.text.map(|text| text.len()), Some(MAX));
        assert_eq!(big.new, no_text(SideReason::TooLarge, MAX as u64 + 1));
    }

    #[test]
    fn file_versions_refuse_anything_the_change_set_does_not_list() {
        let repo = review_repo();
        let outside = repo.parent().join("secret.txt");
        fs::write(&outside, "secret\n").unwrap();
        let service = service();
        let id = add(&service, &repo);
        let change_set = service
            .changed_files(&id, branch("feature/review", "main"))
            .unwrap();
        let (old_sha, new_sha) = (change_set.old_sha.as_str(), change_set.new_sha.as_deref());
        let versions = |path: &str, old_path: Option<&str>| {
            service.file_versions(&id, old_sha, new_sha, path, old_path)
        };

        let outside_text = outside.to_str().unwrap();
        for path in [
            "same.txt",
            "main-only.txt",
            "missing.txt",
            "src",
            "docs/old name.md",
            "../secret.txt",
            "src/../gone.txt",
            "./src/app.go",
            "src//app.go",
            "src/app.go/",
            "/src/app.go",
            outside_text,
            "",
            ".git/config",
            "src/app.go\0",
        ] {
            assert_eq!(
                code(versions(path, None)),
                ErrorCode::UnknownFile,
                "{path:?}"
            );
        }
        // A rename is only listed with its own old path, and neither of its
        // sides is listed alone.
        assert_eq!(
            code(versions("docs/new name.md", None)),
            ErrorCode::UnknownFile
        );
        for old_path in ["same.txt", "gone.txt", "../docs/old name.md", outside_text] {
            assert_eq!(
                code(versions("docs/new name.md", Some(old_path))),
                ErrorCode::UnknownFile,
                "{old_path:?}"
            );
        }
        assert_eq!(
            code(versions("src/app.go", Some("same.txt"))),
            ErrorCode::UnknownFile
        );
        // An old path equal to the path is no rename.
        assert!(versions("src/app.go", Some("src/app.go")).is_ok());

        let tree = repo.git(&["rev-parse", "main^{tree}"]);
        let blob = repo.git(&["rev-parse", "main:same.txt"]);
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let marker = repo.parent().join("written-by-git");
        let option = format!("--output={}", marker.display());
        for sha in [
            unknown,
            tree.as_str(),
            blob.as_str(),
            "HEAD",
            "main",
            "",
            option.as_str(),
        ] {
            assert_eq!(
                code(service.file_versions(&id, sha, new_sha, "src/app.go", None)),
                ErrorCode::UnknownBranch,
                "old {sha:?}"
            );
            assert_eq!(
                code(service.file_versions(&id, old_sha, Some(sha), "src/app.go", None)),
                ErrorCode::UnknownBranch,
                "new {sha:?}"
            );
        }
        // The empty tree only stands for an unborn HEAD, against the disk.
        let empty_tree = repo.git(&["hash-object", "-t", "tree", "/dev/null"]);
        assert_eq!(
            code(service.file_versions(&id, &empty_tree, new_sha, "src/app.go", None)),
            ErrorCode::UnknownBranch
        );
        assert!(!marker.exists());
    }

    #[test]
    fn branch_targets_are_resolved_as_compare_branches_resolves_them() {
        let repo = review_repo();
        repo.git(&["checkout", "--quiet", "--orphan", "island"]);
        repo.commit_file("island.txt", "alone\n", "unrelated root");
        let service = service();
        let id = add(&service, &repo);

        for (name, base, expected) in [
            ("missing", "main", ErrorCode::UnknownBranch),
            ("--all", "main", ErrorCode::UnknownBranch),
            ("feature/review", "feature/review", ErrorCode::UnknownBranch),
            ("feature/review", "origin/main", ErrorCode::UnknownBranch),
            ("island", "main", ErrorCode::NoCommonHistory),
        ] {
            assert_eq!(
                code(service.changed_files(&id, branch(name, base))),
                expected,
                "{name} against {base}"
            );
        }
        assert_eq!(
            code(service.changed_files("no such project", ReviewTarget::WorkingTree)),
            ErrorCode::UnknownProject
        );
    }

    #[test]
    fn a_remote_only_branch_can_be_reviewed() {
        let source = TestRepo::init();
        source.commit_file("a.txt", "a\n", "first");
        source.git(&["checkout", "--quiet", "-b", "feature/remote"]);
        source.commit_file("a.txt", "a\nb\n", "remote work");
        source.git(&["checkout", "--quiet", "main"]);
        let clone = TestRepo::clone_from(source.path());
        let service = service();
        let id = add(&service, &clone);

        let change_set = service
            .changed_files(&id, branch("feature/remote", "main"))
            .unwrap();

        assert_eq!(
            change_set.new_sha,
            Some(clone.git(&["rev-parse", "origin/feature/remote"]))
        );
        assert_eq!(
            rows(&change_set),
            vec![row("a.txt", FileStatus::Modified, 1, 0, false, None)]
        );
        assert_eq!(
            service
                .file_versions(
                    &id,
                    &change_set.old_sha,
                    change_set.new_sha.as_deref(),
                    "a.txt",
                    None
                )
                .unwrap(),
            sides(text("a\n"), text("a\nb\n"))
        );
    }

    /// Committed state of the working tree tests.
    fn working_repo() -> TestRepo {
        let repo = TestRepo::init();
        repo.write("a.txt", "1\n2\n3\n");
        repo.write("gone.txt", "x\ny\n");
        let to_rename: String = (1..=10).map(|n| format!("row {n}\n")).collect();
        repo.write("to rename.txt", to_rename);
        repo.write("crlf.txt", "one\r\ntwo\r\n");
        repo.write("touched.txt", "same content\n");
        repo.write("conf/settings.txt", "inside\n");
        repo.write(".gitignore", "*.log\n");
        repo.commit_all("initial commit");
        repo
    }

    /// Staged, unstaged and untracked changes, an ignored file, a file whose
    /// stat changed but not its content, and two symlinks that point out of
    /// the repository.
    fn change_working_tree(repo: &TestRepo) -> PathBuf {
        let outside = repo.parent().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("settings.txt"), "outside\n").unwrap();
        fs::write(outside.join("secret.txt"), "secret\n").unwrap();

        repo.write("a.txt", "1\nchanged\n3\n4\n");
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        repo.write("staged.go", "package main\n");
        repo.git(&["add", "staged.go"]);
        repo.git(&["mv", "to rename.txt", "renamed.txt"]);
        let renamed = fs::read_to_string(repo.path().join("renamed.txt")).unwrap();
        repo.write("renamed.txt", renamed.replace("row 5\n", "row five\n"));
        repo.write("crlf.txt", "one\r\nTWO\r\n");
        // Same content, older mtime: the index entry no longer matches its
        // stat, and the file is not recent enough for git to recheck it.
        fs::File::options()
            .write(true)
            .open(repo.path().join("touched.txt"))
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
            .unwrap();
        repo.write("new dir/untracked.txt", "u1\nu2\nu3");
        repo.write("blob.dat", [7u8, 0, 7, b'\n']);
        repo.write("debug.log", "ignored\n");
        // A tracked folder replaced by a symlink to a folder outside.
        fs::remove_dir_all(repo.path().join("conf")).unwrap();
        symlink(outside.to_str().unwrap(), &repo.path().join("conf"));
        symlink(outside.to_str().unwrap(), &repo.path().join("escape"));
        outside
    }

    fn index(repo: &TestRepo) -> Vec<u8> {
        fs::read(repo.path().join(".git/index")).unwrap()
    }

    #[test]
    fn the_working_tree_lists_staged_unstaged_and_untracked_changes() {
        let repo = working_repo();
        let outside = change_working_tree(&repo);
        // Only the stat of touched.txt changed: the raw diff lists it, the
        // change set must not.
        let raw = repo.git(&["-c", "diff.autoRefreshIndex=false", "diff", "--raw", "HEAD"]);
        assert!(raw.contains("touched.txt"), "{raw}");
        let before = index(&repo);
        let service = service();
        let id = add(&service, &repo);

        let change_set = service
            .changed_files(&id, ReviewTarget::WorkingTree)
            .unwrap();

        assert_eq!(change_set.target, ReviewTarget::WorkingTree);
        assert_eq!(change_set.old_sha, repo.git(&["rev-parse", "HEAD"]));
        assert_eq!(change_set.new_sha, None);
        let mut renamed = row("renamed.txt", FileStatus::Renamed, 1, 1, false, None);
        renamed.1 = Some("to rename.txt".to_string());
        assert!(outside.is_dir());
        assert_eq!(
            rows(&change_set),
            vec![
                row("a.txt", FileStatus::Modified, 2, 1, false, None),
                row("blob.dat", FileStatus::Added, 0, 0, true, None),
                row("conf", FileStatus::Added, 1, 0, false, None),
                row("conf/settings.txt", FileStatus::Deleted, 0, 1, false, None),
                row("crlf.txt", FileStatus::Modified, 1, 1, false, None),
                row("escape", FileStatus::Added, 1, 0, false, None),
                row("gone.txt", FileStatus::Deleted, 0, 2, false, None),
                row(
                    "new dir/untracked.txt",
                    FileStatus::Added,
                    3,
                    0,
                    false,
                    None
                ),
                renamed,
                row("staged.go", FileStatus::Added, 1, 0, false, Some("go")),
            ]
        );
        let summary = service.compare_working_tree(&id).unwrap();
        assert_eq!(
            totals(&change_set),
            (summary.files_changed, summary.additions, summary.deletions)
        );
        assert_eq!(index(&repo), before);
    }

    #[test]
    fn working_tree_versions_come_from_disk_without_following_symlinks() {
        let repo = working_repo();
        let outside = change_working_tree(&repo);
        let service = service();
        let id = add(&service, &repo);
        // Several rounds: the index must stay untouched every time.
        for _ in 0..3 {
            let before = index(&repo);
            let change_set = service
                .changed_files(&id, ReviewTarget::WorkingTree)
                .unwrap();
            let old_sha = change_set.old_sha.as_str();
            let versions = |path: &str, old_path: Option<&str>| {
                service.file_versions(&id, old_sha, None, path, old_path)
            };

            assert_eq!(
                versions("a.txt", None).unwrap(),
                sides(text("1\n2\n3\n"), text("1\nchanged\n3\n4\n"))
            );
            assert_eq!(
                versions("crlf.txt", None).unwrap(),
                sides(text("one\r\ntwo\r\n"), text("one\r\nTWO\r\n"))
            );
            assert_eq!(
                versions("staged.go", None).unwrap(),
                sides(no_text(SideReason::Absent, 0), text("package main\n"))
            );
            assert_eq!(
                versions("gone.txt", None).unwrap(),
                sides(text("x\ny\n"), no_text(SideReason::Absent, 0))
            );
            let renamed = versions("renamed.txt", Some("to rename.txt")).unwrap();
            assert!(renamed.old.text.unwrap().contains("row 5\n"));
            assert!(renamed.new.text.unwrap().contains("row five\n"));
            assert_eq!(
                versions("new dir/untracked.txt", None).unwrap(),
                sides(no_text(SideReason::Absent, 0), text("u1\nu2\nu3"))
            );
            assert_eq!(
                versions("blob.dat", None).unwrap().new,
                no_text(SideReason::Binary, 4)
            );
            // The symlinks are read as links: their target is the text.
            let target = outside.to_str().unwrap();
            assert_eq!(versions("escape", None).unwrap().new, text(target));
            assert_eq!(versions("conf", None).unwrap().new, text(target));
            // git sees the file under the replaced folder as deleted, and so
            // is it here, instead of reading the file outside.
            assert_eq!(
                versions("conf/settings.txt", None).unwrap(),
                sides(text("inside\n"), no_text(SideReason::Absent, 0))
            );
            for path in [
                "escape/secret.txt",
                "escape/../escape",
                "../outside/secret.txt",
                target,
                "debug.log",
                "touched.txt",
                ".gitignore",
                "new dir",
                "new dir/missing.txt",
                "renamed.txt",
                "to rename.txt",
            ] {
                assert_eq!(
                    code(versions(path, None)),
                    ErrorCode::UnknownFile,
                    "{path:?}"
                );
            }
            // An untracked file has no old path.
            assert_eq!(
                code(versions("new dir/untracked.txt", Some("a.txt"))),
                ErrorCode::UnknownFile
            );
            assert_eq!(index(&repo), before);
        }
    }

    #[test]
    fn an_unborn_head_is_compared_with_the_empty_tree() {
        let repo = TestRepo::init();
        repo.write("staged.txt", "s\n");
        repo.git(&["add", "staged.txt"]);
        repo.write("untracked.txt", "a\nb\n");
        let before = index(&repo);
        let service = service();
        let id = add(&service, &repo);

        let change_set = service
            .changed_files(&id, ReviewTarget::WorkingTree)
            .unwrap();

        let empty_tree = repo.git(&["hash-object", "-t", "tree", "/dev/null"]);
        assert_eq!(change_set.old_sha, empty_tree);
        assert_eq!(
            rows(&change_set),
            vec![
                row("staged.txt", FileStatus::Added, 1, 0, false, None),
                row("untracked.txt", FileStatus::Added, 2, 0, false, None),
            ]
        );
        let summary = service.compare_working_tree(&id).unwrap();
        assert_eq!(
            totals(&change_set),
            (summary.files_changed, summary.additions, summary.deletions)
        );
        for (path, content) in [("staged.txt", "s\n"), ("untracked.txt", "a\nb\n")] {
            assert_eq!(
                service
                    .file_versions(&id, &empty_tree, None, path, None)
                    .unwrap(),
                sides(no_text(SideReason::Absent, 0), text(content))
            );
        }
        assert_eq!(index(&repo), before);
    }

    #[test]
    fn an_untracked_nested_repository_is_shown_by_its_commit() {
        let repo = working_repo();
        let nested = TestRepo::init();
        nested.commit_file("x.txt", "x\n", "nested");
        fs::rename(nested.path(), repo.path().join("nested")).unwrap();
        let head = run_git(&repo.path().join("nested"), &["rev-parse", "HEAD"]);
        let service = service();
        let id = add(&service, &repo);

        let change_set = service
            .changed_files(&id, ReviewTarget::WorkingTree)
            .unwrap();
        assert_eq!(
            rows(&change_set),
            vec![row("nested", FileStatus::Added, 0, 0, false, None)]
        );
        assert_eq!(
            service
                .file_versions(&id, &change_set.old_sha, None, "nested", None)
                .unwrap()
                .new,
            text(&format!("Subproject commit {head}\n"))
        );
    }

    /// Every git process the service starts appends a line to a file.
    fn counting_service(log: &Path) -> ProjectService {
        let script = log.with_file_name("counting-git");
        fs::write(
            &script,
            "#!/bin/sh\necho \"$1\" >> \"$DIFFLENS_TEST_GIT_LOG\"\nexec git \"$@\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let git = isolated_git()
            .with_program(script.to_str().unwrap())
            .with_environment("DIFFLENS_TEST_GIT_LOG", log.to_str().unwrap());
        ProjectService::new(Store::open_in_memory().unwrap(), git, None)
    }

    fn processes(log: &Path) -> usize {
        fs::read_to_string(log)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[test]
    fn listing_costs_the_same_git_processes_for_three_or_three_hundred_files() {
        let repo = TestRepo::init();
        repo.commit_file("base.txt", "base\n", "initial commit");
        for (name, files) in [("small", 3), ("large", 300)] {
            repo.git(&["checkout", "--quiet", "-b", name, "main"]);
            for n in 0..files {
                repo.write(
                    &format!("{name}/file {n}.go"),
                    format!("package x // {n}\n"),
                );
            }
            repo.commit_all(name);
        }
        repo.git(&["checkout", "--quiet", "main"]);
        let log = repo.parent().join("git.log");
        let service = counting_service(&log);
        let id = add(&service, &repo);

        let mut cost = Vec::new();
        for name in ["small", "large"] {
            let start = processes(&log);
            let change_set = service.changed_files(&id, branch(name, "main")).unwrap();
            cost.push((change_set.files.len(), processes(&log) - start));
        }
        for files in [3, 300] {
            for n in 0..files {
                repo.write(&format!("untracked {files}/{n}.txt"), "u\n");
            }
            let start = processes(&log);
            let change_set = service
                .changed_files(&id, ReviewTarget::WorkingTree)
                .unwrap();
            cost.push((change_set.files.len(), processes(&log) - start));
        }

        assert_eq!(cost[0].0, 3);
        assert_eq!(cost[1].0, 300);
        assert_eq!(cost[0].1, cost[1].1, "{cost:?}");
        assert_eq!(cost[2].0, 3);
        assert_eq!(cost[3].0, 303);
        assert_eq!(cost[2].1, cost[3].1, "{cost:?}");
        assert!(
            cost.iter().all(|(_, processes)| *processes <= 6),
            "{cost:?}"
        );
    }

    #[test]
    fn requests_and_responses_use_the_shapes_of_the_contract() {
        let target: ReviewTarget =
            serde_json::from_str(r#"{ "kind": "branch", "branch": "feature/x", "base": "main" }"#)
                .unwrap();
        assert_eq!(target, branch("feature/x", "main"));
        let target: ReviewTarget = serde_json::from_str(r#"{ "kind": "working_tree" }"#).unwrap();
        assert_eq!(target, ReviewTarget::WorkingTree);
        assert!(serde_json::from_str::<ReviewTarget>(r#"{ "kind": "branch" }"#).is_err());

        let change_set = ChangeSet {
            target: ReviewTarget::WorkingTree,
            old_sha: "abc".to_string(),
            new_sha: None,
            base: None,
            files: vec![ChangedFile {
                path: "b.ts".to_string(),
                old_path: Some("a.ts".to_string()),
                status: FileStatus::TypeChanged,
                additions: 1,
                deletions: 2,
                binary: false,
                language: Some("typescript"),
            }],
        };
        assert_eq!(
            serde_json::to_value(&change_set).unwrap(),
            serde_json::json!({
                "target": { "kind": "working_tree" },
                "oldSha": "abc",
                "newSha": null,
                "base": null,
                "files": [{
                    "path": "b.ts",
                    "oldPath": "a.ts",
                    "status": "type_changed",
                    "additions": 1,
                    "deletions": 2,
                    "binary": false,
                    "language": "typescript"
                }]
            })
        );
        assert_eq!(
            serde_json::to_value(branch("f", "main")).unwrap(),
            serde_json::json!({ "kind": "branch", "branch": "f", "base": "main" })
        );
        let base = ReviewBase {
            name: "develop".to_string(),
            kind: ReviewBaseKind::RemoteTracking,
            ref_name: Some("origin/develop".to_string()),
            sha: "abc".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&base).unwrap(),
            serde_json::json!({
                "name": "develop",
                "kind": "remote_tracking",
                "refName": "origin/develop",
                "sha": "abc"
            })
        );
        assert_eq!(
            serde_json::to_value(ReviewBaseKind::PullRequest).unwrap(),
            "pull_request"
        );
        let versions = sides(text("x"), no_text(SideReason::NotUtf8, 3));
        assert_eq!(
            serde_json::to_value(&versions).unwrap(),
            serde_json::json!({
                "old": { "text": "x", "reason": null, "size": 1 },
                "new": { "text": null, "reason": "not_utf8", "size": 3 }
            })
        );
        for (reason, expected) in [
            (SideReason::Absent, "absent"),
            (SideReason::Binary, "binary"),
            (SideReason::TooLarge, "too_large"),
            (SideReason::NotUtf8, "not_utf8"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), expected);
        }
    }
}
