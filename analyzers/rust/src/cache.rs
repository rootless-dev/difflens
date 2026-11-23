//! The `--cache-dir` of `test` and `check`: a folder of the core, one per
//! project, kept between runs, that holds the build output
//! (`<cache-dir>/target` for the tests, `<cache-dir>/clippy` for clippy),
//! so that a run does not compile every dependency again.
//!
//! Every run builds a fresh copy of the tree at another path, and Cargo
//! shares artifacts between such copies: the metadata hash of a path
//! package uses its path relative to the workspace root, and its freshness
//! compares the mtimes of its source files with those of the earlier
//! build. A copy whose files keep older mtimes (a copy that preserves
//! them; `std::fs::copy`, which clones on macOS) would be taken as fresh
//! against artifacts built from other code. So, holding the folder's lock,
//! every file of the tree is set modified now right before each build
//! (`touch_tree`): the tree's own crates (workspace members, path
//! dependencies, build scripts) are always built again, while the
//! registry's dependencies, which Cargo fingerprints by version, stay
//! built.
//!
//! The lock (`<folder>.lock`, an advisory file lock) is held for the whole
//! build and run, so that two runs of one project never interleave: a
//! build that starts after another copy's touch would make that copy look
//! fresh, and test binaries and coverage profiles are used outside Cargo's
//! own lock.

use std::fs::{File, FileTimes, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often a busy lock is tried again.
const LOCK_POLL: Duration = Duration::from_millis(200);

/// One folder of the cache, held by this run until dropped.
pub struct Held {
    pub dir: PathBuf,
    _lock: File,
}

/// Takes `<cache_dir>/<name>` for this run: creates it and waits for its
/// lock until `deadline`. The error says why it cannot be used.
pub fn acquire(cache_dir: &Path, name: &str, deadline: Instant) -> Result<Held, String> {
    let dir = cache_dir.join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("{} could not be created: {error}", dir.display()))?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let lock_path = cache_dir.join(format!("{name}.lock"));
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("{} could not be opened: {error}", lock_path.display()))?;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(Held { dir, _lock: lock }),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(format!(
                    "{} stayed in use by another run until the time limit",
                    dir.display()
                ))
            }
            Err(TryLockError::Error(error)) => {
                return Err(format!(
                    "{} could not be locked: {error}",
                    lock_path.display()
                ))
            }
        }
    }
}

/// Sets every file under `root` modified now, so that Cargo builds the
/// crates of the tree again whatever an earlier copy left in a shared
/// target folder. Links are not followed; VCS data, `node_modules` and
/// Cargo's own target folders (marked by CACHEDIR.TAG) are left alone.
pub fn touch_tree(root: &Path) -> std::io::Result<()> {
    touch(root, FileTimes::new().set_modified(SystemTime::now()))
}

fn touch(dir: &Path, times: FileTimes) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let skipped = matches!(entry.file_name().to_str(), Some(".git" | "node_modules"))
                || path.join("CACHEDIR.TAG").is_file();
            if !skipped {
                touch(&path, times)?;
            }
        } else if kind.is_file() {
            // The owner may set the times through a read-only handle; a
            // file it cannot read is opened for writing instead.
            let file = File::open(&path).or_else(|_| File::options().write(true).open(&path))?;
            file.set_times(times)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn modified(path: &Path) -> SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    fn set_old(path: &Path) -> SystemTime {
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
        old
    }

    #[test]
    fn every_source_file_is_set_modified_now_but_build_output_is_not() {
        let folder = TempDir::new();
        let root = &folder.0;
        for (path, text) in [
            ("Cargo.toml", "[package]\n"),
            ("src/lib.rs", "pub fn a() {}\n"),
            ("src/target/mod.rs", "// a module named target\n"),
            (
                "target/CACHEDIR.TAG",
                "Signature: 8a477f597d28d172789f06886806bc55\n",
            ),
            ("target/debug/out", "built\n"),
            (".git/HEAD", "ref\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        std::os::unix::fs::symlink("/definitely/not/here", root.join("src/dangling")).unwrap();
        let mut old = SystemTime::now();
        for path in [
            "Cargo.toml",
            "src/lib.rs",
            "src/target/mod.rs",
            "target/debug/out",
            ".git/HEAD",
        ] {
            old = set_old(&root.join(path));
        }
        let before = SystemTime::now() - Duration::from_secs(1);
        touch_tree(root).unwrap();
        for path in ["Cargo.toml", "src/lib.rs", "src/target/mod.rs"] {
            assert!(modified(&root.join(path)) >= before, "{path}");
        }
        for path in ["target/debug/out", ".git/HEAD"] {
            assert_eq!(modified(&root.join(path)), old, "{path}");
        }
    }

    #[test]
    fn a_folder_is_held_by_one_run_at_a_time() {
        // A child another test forks shares the lock until it executes, so
        // a free lock may take a moment.
        let soon = || Instant::now() + Duration::from_secs(10);
        let folder = TempDir::new();
        let first = acquire(&folder.0, "target", soon()).unwrap();
        assert!(first.dir.is_dir() && first.dir.ends_with("target"));
        let busy = acquire(
            &folder.0,
            "target",
            Instant::now() + Duration::from_millis(300),
        );
        assert!(busy.err().unwrap().contains("in use by another run"));
        assert!(acquire(&folder.0, "clippy", soon()).is_ok());
        drop(first);
        assert!(acquire(&folder.0, "target", soon()).is_ok());
    }
}
