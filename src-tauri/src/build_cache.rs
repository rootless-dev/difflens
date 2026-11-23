//! The build caches analyzers keep between runs of a project: one folder per
//! project and analyzer, `<root>/<project id>/<analyzer id>/`, given to
//! `prepare`, `test` and `check` as `--cache-dir` (see
//! `analyzers/protocol/tests.schema.json`) and opened for writes in the test
//! sandbox. Every run starts from a fresh copy of the code, so without one a
//! Rust project would compile all its dependencies on every run.
//!
//! What an analyzer keeps there is its own (for Rust, its target folders);
//! the core only bounds it: a folder grown past the limit is emptied when a
//! run takes it and nothing else holds it, so the next build starts cold
//! once, and a project's folders are deleted when the project is removed
//! from DiffLens. They live with the app's data rather than its cache, like
//! the analyzers' environments: a cache cleaner throwing them away would
//! only cost a cold build, but they are rebuilt per project, not per commit.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::error::{app_error, AppError, AppResult};

/// Most a project's build cache of one analyzer may hold before it is
/// emptied: a Rust workspace's dependencies built for tests, coverage and
/// clippy fit well under it.
pub const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub struct BuildCaches {
    root: PathBuf,
    max_bytes: u64,
    /// How many runs hold each folder now.
    held: Mutex<HashMap<PathBuf, usize>>,
}

impl BuildCaches {
    pub fn new(root: PathBuf, max_bytes: u64) -> Self {
        Self {
            root,
            max_bytes,
            held: Mutex::new(HashMap::new()),
        }
    }

    /// The folder of `project_id` and `analyzer`, created when missing, and
    /// held until the lease is dropped. When nothing else holds it and it
    /// holds more than the limit, it is emptied first.
    pub fn lease(&self, project_id: &str, analyzer: &str) -> AppResult<Lease<'_>> {
        let path = self.root.join(project_id).join(analyzer);
        let first = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            let count = held.entry(path.clone()).or_insert(0);
            *count += 1;
            *count == 1
        };
        let lease = Lease {
            caches: self,
            path: path.clone(),
        };
        if first && size_of(&path) > self.max_bytes {
            // A folder that cannot be emptied is only used as it is.
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).map_err(|error| disk_error(&path, &error))?;
        Ok(lease)
    }

    /// Deletes the build caches of `project_id`, of every analyzer. Runs of
    /// it going on lose what they build there.
    pub fn remove_project(&self, project_id: &str) -> AppResult<()> {
        let path = self.root.join(project_id);
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(disk_error(&path, &error)),
        }
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// A build cache folder held by one run.
pub struct Lease<'a> {
    caches: &'a BuildCaches,
    path: PathBuf,
}

impl Lease<'_> {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut held = self.caches.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.path);
            }
        }
    }
}

/// The bytes of the files under `path`, symbolic links not followed; 0
/// when it cannot be read.
fn size_of(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut folders = vec![path.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                folders.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_makes_the_folder_of_the_project_and_analyzer() {
        let dir = tempfile::TempDir::new().unwrap();
        let caches = BuildCaches::new(dir.path().join("build-cache"), MAX_BYTES);

        let lease = caches.lease("project-1", "rust").unwrap();

        assert_eq!(lease.path(), dir.path().join("build-cache").join("project-1").join("rust"));
        assert!(lease.path().is_dir());
        fs::write(lease.path().join("kept"), "x").unwrap();
        drop(lease);
        let again = caches.lease("project-1", "rust").unwrap();
        assert!(again.path().join("kept").exists(), "kept between runs");
    }

    #[test]
    fn a_folder_past_the_limit_is_emptied_only_when_nothing_holds_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let caches = BuildCaches::new(dir.path().to_path_buf(), 10);
        let held = caches.lease("p", "rust").unwrap();
        fs::create_dir_all(held.path().join("target/debug")).unwrap();
        fs::write(held.path().join("target/debug/big"), "more than ten bytes").unwrap();

        // Another run while the first builds: it keeps what is there.
        let second = caches.lease("p", "rust").unwrap();
        assert!(second.path().join("target/debug/big").exists());
        drop(second);
        drop(held);

        let next = caches.lease("p", "rust").unwrap();
        assert!(next.path().is_dir());
        assert!(!next.path().join("target").exists(), "emptied, then made again");
        fs::write(next.path().join("small"), "1").unwrap();
        drop(next);
        assert!(caches.lease("p", "rust").unwrap().path().join("small").exists(), "under the limit");
    }

    #[test]
    fn removing_a_project_deletes_its_folders_and_no_other() {
        let dir = tempfile::TempDir::new().unwrap();
        let caches = BuildCaches::new(dir.path().to_path_buf(), MAX_BYTES);
        let rust = caches.lease("p", "rust").unwrap().path().to_path_buf();
        let other = caches.lease("q", "rust").unwrap().path().to_path_buf();

        caches.remove_project("p").unwrap();

        assert!(!rust.exists() && !dir.path().join("p").exists());
        assert!(other.is_dir());
        caches.remove_project("never-seen").unwrap();
        assert_eq!(caches.root(), dir.path());
    }
}
