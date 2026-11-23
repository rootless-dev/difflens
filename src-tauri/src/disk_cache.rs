//! Files kept in the app cache under
//! `<kind>/<project id>/<language>/<analyzer fingerprint>/<file>`: written
//! atomically, and pruned on each write to the current analyzer build and
//! the most recently used files.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use uuid::Uuid;

/// Writes `bytes` to `file` atomically, then deletes the folders of other
/// analyzer builds next to the file's folder and all but the `keep` most
/// recently used files in it.
pub fn write(file: &Path, bytes: &[u8], keep: usize) -> io::Result<()> {
    let (Some(directory), Some(language_directory)) =
        (file.parent(), file.parent().and_then(Path::parent))
    else {
        return Ok(());
    };
    fs::create_dir_all(directory)?;
    let partial = directory.join(format!(".partial-{}", Uuid::new_v4()));
    let written = fs::write(&partial, bytes).and_then(|()| fs::rename(&partial, file));
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
    written?;

    for entry in fs::read_dir(language_directory)?.flatten() {
        if entry.path() != directory {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    let mut cached: Vec<(SystemTime, PathBuf)> = fs::read_dir(directory)?
        .flatten()
        .filter_map(|entry| {
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    cached.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, stale) in cached.into_iter().skip(keep) {
        let _ = fs::remove_file(stale);
    }
    Ok(())
}

/// Marks a file as just used, for pruning.
pub fn touch(file: &Path) {
    if let Ok(handle) = OpenOptions::new().write(true).open(file) {
        let _ = handle.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn keeps_the_current_build_and_the_most_recent_files() {
        let root = tempfile::TempDir::new().unwrap();
        let old_build = root.path().join("go").join("old");
        fs::create_dir_all(&old_build).unwrap();
        fs::write(old_build.join("a.json"), "old").unwrap();
        let build = root.path().join("go").join("new");
        for name in ["a", "b", "c"] {
            write(&build.join(format!("{name}.json")), name.as_bytes(), 2).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(!old_build.exists());
        let mut kept: Vec<String> = fs::read_dir(&build)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        kept.sort();
        assert_eq!(kept, vec!["b.json", "c.json"]);
        assert_eq!(fs::read_to_string(build.join("c.json")).unwrap(), "c");
    }
}
