//! Plain source trees for analyzers, so an analyzer never runs inside the
//! user's repository and nothing is ever written to it.
//!
//! A commit is extracted from `git archive` into
//! `<trees>/<project id>/<sha>/`: into a temporary folder first, renamed into
//! place when complete, and reused afterwards. The 20 most recently used trees
//! of each project are kept (`KEEP_PER_PROJECT`); older ones are deleted when a
//! new one is added, unless an analysis is using them.
//!
//! The working tree is copied (tracked and untracked files that are not
//! ignored) into a fresh scratch folder that is deleted after use.
//!
//! Entries that would land outside the tree are skipped, and symlinks are kept
//! only when they resolve inside it.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use uuid::Uuid;

use crate::error::{app_error, AppError, AppResult};
use crate::git::Git;

/// Trees of one project kept on disk.
pub const KEEP_PER_PROJECT: usize = 20;

const PARTIAL_PREFIX: &str = ".partial-";
const STAMP_SUFFIX: &str = ".used";

pub struct TreeStore {
    root: PathBuf,
    keep: usize,
    /// Trees an analysis is reading right now, with the number of readers.
    in_use: Mutex<HashMap<PathBuf, usize>>,
}

/// A tree that is not evicted while this is alive.
pub struct TreeLease<'a> {
    store: &'a TreeStore,
    path: PathBuf,
}

impl TreeLease<'_> {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TreeLease<'_> {
    fn drop(&mut self) {
        let mut in_use = self
            .store
            .in_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = in_use.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                in_use.remove(&self.path);
            }
        }
    }
}

impl TreeStore {
    pub fn new(root: PathBuf, keep: usize) -> Self {
        Self {
            root,
            keep,
            in_use: Mutex::new(HashMap::new()),
        }
    }

    /// Deletes extractions left unfinished by an earlier run of the app.
    pub fn remove_leftovers(&self) {
        let Ok(projects) = fs::read_dir(&self.root) else {
            return;
        };
        for project in projects.flatten() {
            let Ok(entries) = fs::read_dir(project.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PARTIAL_PREFIX)
                {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
    }

    /// The files of `sha` in `repo`, extracted on first use. `project_id` and
    /// `sha` must already be validated (a project id and a commit id), since
    /// they name folders.
    pub fn commit_tree(
        &self,
        git: &Git,
        repo: &Path,
        project_id: &str,
        sha: &str,
    ) -> AppResult<TreeLease<'_>> {
        let project_directory = self.root.join(project_id);
        let path = project_directory.join(sha);
        let lease = self.lease(path.clone());
        let stamp = project_directory.join(format!("{sha}{STAMP_SUFFIX}"));
        if path.is_dir() {
            touch(&stamp);
            return Ok(lease);
        }

        fs::create_dir_all(&project_directory)
            .map_err(|error| disk_error(&project_directory, &error))?;
        let partial = project_directory.join(format!("{PARTIAL_PREFIX}{}", Uuid::new_v4()));
        fs::create_dir(&partial).map_err(|error| disk_error(&partial, &error))?;
        let extracted = git
            .archive(repo, sha, |archive| extract_tar(archive, &partial))
            .and_then(|()| match fs::rename(&partial, &path) {
                Ok(()) => Ok(()),
                // Another analysis extracted the same commit meanwhile.
                Err(_) if path.is_dir() => Ok(()),
                Err(error) => Err(disk_error(&path, &error)),
            });
        if partial.exists() {
            let _ = fs::remove_dir_all(&partial);
        }
        extracted?;
        touch(&stamp);
        self.evict(&project_directory);
        Ok(lease)
    }

    fn lease(&self, path: PathBuf) -> TreeLease<'_> {
        *self
            .in_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(path.clone())
            .or_insert(0) += 1;
        TreeLease { store: self, path }
    }

    /// Keeps the `keep` most recently used trees of a project. Holds the
    /// in-use lock throughout, so no tree can be leased while it is deleted.
    fn evict(&self, project_directory: &Path) {
        let in_use = self.in_use.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(entries) = fs::read_dir(project_directory) else {
            return;
        };
        let mut trees: Vec<(SystemTime, PathBuf, PathBuf)> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if name.starts_with(PARTIAL_PREFIX) {
                    return None;
                }
                let stamp = project_directory.join(format!("{name}{STAMP_SUFFIX}"));
                let used = fs::metadata(&stamp)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                Some((used, entry.path(), stamp))
            })
            .collect();
        trees.sort_by_key(|(used, _, _)| std::cmp::Reverse(*used));
        for (_, tree, stamp) in trees.into_iter().skip(self.keep) {
            if in_use.contains_key(&tree) {
                continue;
            }
            if fs::remove_dir_all(&tree).is_ok() {
                let _ = fs::remove_file(stamp);
            }
        }
    }
}

/// A folder deleted with everything in it when dropped.
pub struct ScratchTree {
    path: PathBuf,
}

impl ScratchTree {
    pub fn create(parent: &Path) -> AppResult<Self> {
        let path = parent.join(Uuid::new_v4().to_string());
        fs::create_dir_all(&path).map_err(|error| disk_error(&path, &error))?;
        Ok(Self { path })
    }

    /// A new folder in `parent` named `<prefix>` and 8 random hex digits,
    /// for paths that must stay short.
    pub fn create_short(parent: &Path, prefix: &str) -> AppResult<Self> {
        let random = Uuid::new_v4().simple().to_string();
        let path = parent.join(format!("{prefix}{}", &random[..8]));
        fs::create_dir(&path).map_err(|error| disk_error(&path, &error))?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Copies the files of the working tree of `repo` that git tracks or would
/// track (not ignored) into the empty folder `target`, as git sees them: a
/// symlink is copied as a symlink, a file under a symlinked folder is not
/// there, nested repositories and submodules are left out.
pub fn copy_working_tree(git: &Git, repo: &Path, target: &Path) -> AppResult<()> {
    let mut links = Vec::new();
    for listed in git.listed_files(repo)? {
        let Some(relative) = plain_relative(&listed) else {
            continue;
        };
        if !parents_are_folders(repo, &relative) {
            continue;
        }
        let source = repo.join(&relative);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            // Deleted since it was listed, or deleted but still tracked.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(disk_error(&source, &error)),
        };
        if metadata.file_type().is_symlink() {
            let link_target =
                fs::read_link(&source).map_err(|error| disk_error(&source, &error))?;
            links.push((relative, link_target));
        } else if metadata.is_file() {
            let destination = target.join(&relative);
            create_parent(&destination)?;
            fs::copy(&source, &destination).map_err(|error| disk_error(&source, &error))?;
        }
    }
    create_symlinks(target, &links)
}

/// A value that changes whenever a file `copy_working_tree` would copy is
/// added, removed, edited or retyped: a hash of each file's path, type, size,
/// modification time and link target. Nothing is read beyond the metadata.
pub fn working_tree_stamp(git: &Git, repo: &Path) -> AppResult<u64> {
    let mut hasher = DefaultHasher::new();
    for listed in git.listed_files(repo)? {
        let Some(relative) = plain_relative(&listed) else {
            continue;
        };
        let source = repo.join(&relative);
        relative.hash(&mut hasher);
        match fs::symlink_metadata(&source) {
            Ok(metadata) => {
                metadata.file_type().is_symlink().hash(&mut hasher);
                metadata.len().hash(&mut hasher);
                metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_nanos())
                    .hash(&mut hasher);
                if metadata.file_type().is_symlink() {
                    fs::read_link(&source).ok().hash(&mut hasher);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => "absent".hash(&mut hasher),
            Err(error) => return Err(disk_error(&source, &error)),
        }
    }
    Ok(hasher.finish())
}

/// Extracts a tar archive into the empty folder `target`. Entries whose path
/// leaves the folder are skipped, as are links other than symlinks and
/// special files.
pub fn extract_tar(archive: &mut dyn Read, target: &Path) -> AppResult<()> {
    let bad = |error: io::Error| {
        app_error!(
            Internal,
            ArchiveReadFailed,
            "Could not read the archive: {detail}",
            detail = error
        )
    };
    let mut archive = tar::Archive::new(archive);
    let mut links = Vec::new();
    for entry in archive.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        let path = entry.path().map_err(bad)?.into_owned();
        let Some(relative) = plain_relative(&path) else {
            continue;
        };
        let destination = target.join(&relative);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&destination)
                    .map_err(|error| disk_error(&destination, &error))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let executable = entry.header().mode().is_ok_and(|mode| mode & 0o111 != 0);
                create_parent(&destination)?;
                let mut file = new_file(&destination, executable)?;
                io::copy(&mut entry, &mut file)
                    .map_err(|error| disk_error(&destination, &error))?;
            }
            tar::EntryType::Symlink => {
                if let Some(link_target) = entry.link_name().map_err(bad)? {
                    links.push((relative, link_target.into_owned()));
                }
            }
            _ => {}
        }
    }
    create_symlinks(target, &links)
}

fn new_file(path: &Path, executable: bool) -> AppResult<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    options.open(path).map_err(|error| disk_error(path, &error))
}

/// `path` made only of plain names, or `None` when it is absolute, empty or
/// climbs with `..`.
fn plain_relative(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// Every folder above `relative` under `root` is a real folder, not a
/// symlink: git does not look through symlinks.
fn parents_are_folders(root: &Path, relative: &Path) -> bool {
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            return true;
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            _ => return false,
        }
    }
    true
}

fn create_parent(path: &Path) -> AppResult<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(|error| disk_error(parent, &error)),
        None => Ok(()),
    }
}

/// Whether `target`, read from the folder of the link `link`, stays inside
/// the tree without looking at the disk.
fn lexically_inside(link: &Path, target: &Path) -> bool {
    let mut depth: Vec<&std::ffi::OsStr> = link
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter_map(|component| match component {
                    Component::Normal(name) => Some(name),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for component in target.components() {
        match component {
            Component::Normal(name) => depth.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Creates the symlinks after every file is in place, so no file is ever
/// written through one, then removes those that resolve outside `root` (a
/// chain of links can escape even when each looks fine alone) or to nothing.
#[cfg(unix)]
fn create_symlinks(root: &Path, links: &[(PathBuf, PathBuf)]) -> AppResult<()> {
    let canonical_root = fs::canonicalize(root).map_err(|error| disk_error(root, &error))?;
    let mut created = Vec::new();
    for (relative, link_target) in links {
        if !lexically_inside(relative, link_target) || !parents_are_folders(root, relative) {
            continue;
        }
        let link = root.join(relative);
        create_parent(&link)?;
        if std::os::unix::fs::symlink(link_target, &link).is_ok() {
            created.push(link);
        }
    }
    loop {
        let before = created.len();
        created.retain(|link| {
            let inside = fs::canonicalize(link)
                .map(|resolved| resolved.starts_with(&canonical_root))
                .unwrap_or(false);
            if !inside {
                let _ = fs::remove_file(link);
            }
            inside
        });
        if created.len() == before {
            return Ok(());
        }
    }
}

/// Symlinks need privileges on Windows; analyzers get the files only.
#[cfg(not(unix))]
fn create_symlinks(_root: &Path, _links: &[(PathBuf, PathBuf)]) -> AppResult<()> {
    Ok(())
}

/// Marks a tree as just used. A failure only makes it older for eviction.
fn touch(stamp: &Path) {
    let _ = fs::write(stamp, b"");
}

fn disk_error(path: &Path, error: &io::Error) -> AppError {
    app_error!(
        Internal,
        FileWriteFailed,
        "Could not write {path}: {detail}",
        path = path.display(),
        detail = error
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::{isolated_git, TestRepo};
    use std::collections::BTreeMap;
    use std::os::unix::fs::symlink;

    /// Every path under `root` with what it is: file contents, link target
    /// or a folder marker.
    fn listing(root: &Path) -> BTreeMap<String, String> {
        let mut found = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap().flatten() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let metadata = fs::symlink_metadata(&path).unwrap();
                let value = if metadata.file_type().is_symlink() {
                    format!("-> {}", fs::read_link(&path).unwrap().display())
                } else if metadata.is_dir() {
                    pending.push(path.clone());
                    "/".to_string()
                } else {
                    fs::read_to_string(&path).unwrap_or_else(|_| "<binary>".to_string())
                };
                found.insert(relative, value);
            }
        }
        found
    }

    fn repo_with_links() -> (TestRepo, String) {
        let repo = TestRepo::init();
        repo.write("dir with space/file one.go", "package a\n");
        repo.write("run.sh", "#!/bin/sh\n");
        fs::set_permissions(
            repo.path().join("run.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        symlink(
            "dir with space/file one.go",
            repo.path().join("inside link"),
        )
        .unwrap();
        symlink("../../outside.txt", repo.path().join("escaping link")).unwrap();
        symlink("/etc/hosts", repo.path().join("absolute link")).unwrap();
        repo.commit_all("first");
        let sha = repo.git(&["rev-parse", "HEAD"]);
        (repo, sha)
    }

    #[test]
    fn extracts_a_commit_keeping_only_symlinks_that_stay_inside() {
        let (repo, sha) = repo_with_links();
        let cache = tempfile::TempDir::new().unwrap();
        let store = TreeStore::new(cache.path().to_path_buf(), 5);

        let lease = store
            .commit_tree(&isolated_git(), repo.path(), "project", &sha)
            .unwrap();

        assert_eq!(lease.path(), cache.path().join("project").join(&sha));
        let files = listing(lease.path());
        assert_eq!(
            files,
            BTreeMap::from([
                ("dir with space".to_string(), "/".to_string()),
                (
                    "dir with space/file one.go".to_string(),
                    "package a\n".to_string()
                ),
                (
                    "inside link".to_string(),
                    "-> dir with space/file one.go".to_string()
                ),
                ("run.sh".to_string(), "#!/bin/sh\n".to_string()),
            ])
        );
        let mode = fs::metadata(lease.path().join("run.sh"))
            .unwrap()
            .permissions();
        assert_ne!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o111, 0);
        // No partial folder is left behind.
        let leftovers: Vec<_> = fs::read_dir(cache.path().join("project"))
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PARTIAL_PREFIX)
            })
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn an_extracted_tree_is_reused() {
        let (repo, sha) = repo_with_links();
        let cache = tempfile::TempDir::new().unwrap();
        let store = TreeStore::new(cache.path().to_path_buf(), 5);
        let git = isolated_git();
        let first = store.commit_tree(&git, repo.path(), "p", &sha).unwrap();
        let marker = first.path().join("marker");
        fs::write(&marker, "still here").unwrap();
        drop(first);

        let second = store.commit_tree(&git, repo.path(), "p", &sha).unwrap();

        assert!(second.path().join("marker").exists());
    }

    #[test]
    fn old_trees_are_evicted_unless_in_use() {
        let repo = TestRepo::init();
        let git = isolated_git();
        let cache = tempfile::TempDir::new().unwrap();
        let store = TreeStore::new(cache.path().to_path_buf(), 2);
        let mut shas = Vec::new();
        for n in 0..4 {
            repo.commit_file("a.txt", format!("{n}\n"), "change");
            shas.push(repo.git(&["rev-parse", "HEAD"]));
        }
        let held = store.commit_tree(&git, repo.path(), "p", &shas[0]).unwrap();
        for sha in &shas[1..] {
            // Distinct stamp times.
            std::thread::sleep(std::time::Duration::from_millis(20));
            drop(store.commit_tree(&git, repo.path(), "p", sha).unwrap());
        }

        let exists = |sha: &String| cache.path().join("p").join(sha).is_dir();
        assert!(exists(&shas[0]), "in use, so kept");
        assert!(!exists(&shas[1]));
        assert!(exists(&shas[2]));
        assert!(exists(&shas[3]));
        drop(held);
    }

    #[test]
    fn tar_entries_that_escape_are_skipped() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut add = |name: &[u8], kind: tar::EntryType, link: Option<&str>, data: &[u8]| {
            let mut header = tar::Header::new_old();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_entry_type(kind);
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            if let Some(link) = link {
                header.set_link_name(link).unwrap();
            }
            header.set_cksum();
            builder.append(&header, data).unwrap();
        };
        add(b"../escaped.txt", tar::EntryType::Regular, None, b"x");
        add(b"/absolute.txt", tar::EntryType::Regular, None, b"x");
        add(b"ok/file.txt", tar::EntryType::Regular, None, b"fine");
        add(b"self", tar::EntryType::Symlink, Some("."), b"");
        // Each looks inside alone; together they climb out of the tree.
        add(b"climb", tar::EntryType::Symlink, Some("self/.."), b"");
        add(b"through", tar::EntryType::Symlink, Some("climb/x"), b"");
        add(b"hard", tar::EntryType::Link, Some("ok/file.txt"), b"");
        let bytes = builder.into_inner().unwrap();

        let outer = tempfile::TempDir::new().unwrap();
        let target = outer.path().join("tree");
        fs::create_dir(&target).unwrap();
        extract_tar(&mut bytes.as_slice(), &target).unwrap();

        let files = listing(&target);
        assert_eq!(files.get("ok/file.txt").map(String::as_str), Some("fine"));
        assert_eq!(files.get("self").map(String::as_str), Some("-> ."));
        assert!(!files.contains_key("climb"), "{files:?}");
        assert!(!files.contains_key("through"), "{files:?}");
        assert!(!files.contains_key("hard"));
        assert!(!outer.path().join("escaped.txt").exists());
        assert_eq!(files.len(), 3, "{files:?}");
    }

    #[test]
    fn copies_the_working_tree_as_git_sees_it() {
        let (repo, _) = repo_with_links();
        repo.commit_file(".gitignore", "*.log\n", "ignore logs");
        repo.write("untracked new.go", "package b\n");
        repo.write("debug.log", "ignored\n");
        repo.write("run.sh", "#!/bin/sh\necho changed\n");
        fs::remove_file(repo.path().join("dir with space/file one.go")).unwrap();
        let target = tempfile::TempDir::new().unwrap();

        copy_working_tree(&isolated_git(), repo.path(), target.path()).unwrap();

        let files = listing(target.path());
        assert_eq!(
            files,
            BTreeMap::from([
                (".gitignore".to_string(), "*.log\n".to_string()),
                (
                    "run.sh".to_string(),
                    "#!/bin/sh\necho changed\n".to_string()
                ),
                ("untracked new.go".to_string(), "package b\n".to_string()),
            ]),
            "the link to the deleted file dangles and is dropped"
        );
    }

    #[test]
    fn scratch_trees_are_deleted_when_dropped() {
        let parent = tempfile::TempDir::new().unwrap();
        let scratch = ScratchTree::create(parent.path()).unwrap();
        let path = scratch.path().to_path_buf();
        fs::write(path.join("x"), "x").unwrap();
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn lexical_check_counts_the_folder_of_the_link() {
        assert!(lexically_inside(Path::new("a/b/link"), Path::new("../c")));
        assert!(lexically_inside(Path::new("a/link"), Path::new("../c")));
        assert!(!lexically_inside(Path::new("link"), Path::new("../c")));
        assert!(!lexically_inside(Path::new("a/link"), Path::new("/etc")));
    }
}
