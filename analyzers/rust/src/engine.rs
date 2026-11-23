//! cargo-mutants, the mutation engine `mutate` drives (see `mutation`).
//!
//! It is never installed globally: `prepare --tools <folder>` installs the
//! pinned version into `<folder>/cargo-mutants/` (`bin/cargo-mutants` and a
//! `VERSION` marker) when it is missing, and `mutate --tools <folder>` runs
//! it from there. The install is `cargo install --locked --root <temporary
//! folder inside <folder>> cargo-mutants@<VERSION>`, built in a target
//! folder of its own that is deleted afterwards, then renamed into place: a
//! folder with the marker is complete, so two prepares racing both end
//! with a working install, and an install of another version is replaced.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::process;

/// The engine's name, as the mutation document reports it.
pub const NAME: &str = "cargo-mutants";
/// The pinned version: recent, stable, and built by Rust 1.88 or later.
pub const VERSION: &str = "27.1.0";

/// The engine's folder inside `--tools`.
const FOLDER: &str = "cargo-mutants";
/// The file of that folder that holds the installed version.
const MARKER: &str = "VERSION";

/// An installed engine.
#[derive(Debug, Clone)]
pub struct Engine {
    pub program: PathBuf,
    pub version: String,
}

impl Engine {
    /// A command running the engine as Cargo runs a subcommand.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("mutants");
        command
    }
}

/// The pinned engine installed in `tools`, if it is.
fn installed(tools: &Path) -> Option<Engine> {
    let dir = tools.join(FOLDER);
    let version = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let program = dir.join("bin").join("cargo-mutants");
    (version.trim() == VERSION && program.is_file()).then(|| Engine {
        program,
        version: VERSION.to_string(),
    })
}

/// The engine `mutate --tools` runs, or why there is none.
pub fn locate(tools: Option<&Path>) -> Result<Engine, String> {
    let Some(tools) = tools else {
        return Err(format!(
            "mutate needs --tools: the folder where `prepare --tools <folder>` installs {NAME} {VERSION}"
        ));
    };
    installed(tools).ok_or_else(|| {
        format!(
            "{NAME} {VERSION} is not installed in {}: run `prepare --tools {}` first",
            tools.display(),
            tools.display()
        )
    })
}

/// A folder removed when dropped.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A name no other process or call uses at the same time.
fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(".{prefix}-{}-{nanos}-{next}", std::process::id())
}

/// Installs the pinned engine into `tools` unless it is there already.
pub fn install(tools: &Path, deadline: Instant) -> Result<(), String> {
    if installed(tools).is_some() {
        return Ok(());
    }
    if !process::on_path("cargo") {
        return Err("cargo was not found on the PATH".to_string());
    }
    std::fs::create_dir_all(tools)
        .map_err(|error| format!("{} could not be created: {error}", tools.display()))?;
    let temporary = Removed(tools.join(unique(&format!("{FOLDER}-install"))));
    let root = temporary.0.join("root");
    let mut command = Command::new("cargo");
    command
        .arg("install")
        .arg("--locked")
        .arg("--root")
        .arg(&root)
        .arg("--target-dir")
        .arg(temporary.0.join("target"))
        .arg(format!("{NAME}@{VERSION}"))
        .current_dir(tools)
        .env("CARGO_TERM_COLOR", "never")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_INSTALL_ROOT");
    let (_, finished) = process::capture(command, deadline)
        .map_err(|error| format!("cargo could not run: {error}"))?;
    if !finished.success() {
        return Err(format!(
            "cargo install {NAME}@{VERSION} failed ({}):\n{}",
            finished.describe(),
            process::keep_end(&finished.stderr, 4 << 10)
        ));
    }
    // Cargo's records of what it installed there, of no use here.
    let _ = std::fs::remove_file(root.join(".crates.toml"));
    let _ = std::fs::remove_file(root.join(".crates2.json"));
    std::fs::write(root.join(MARKER), format!("{VERSION}\n"))
        .map_err(|error| format!("the version marker could not be written: {error}"))?;
    let destination = tools.join(FOLDER);
    if std::fs::rename(&root, &destination).is_ok() || installed(tools).is_some() {
        return Ok(());
    }
    // Another version, or an install that never finished, is in the way:
    // set it aside, then move this one in.
    let aside = Removed(tools.join(unique(&format!("{FOLDER}-old"))));
    let _ = std::fs::rename(&destination, &aside.0);
    match std::fs::rename(&root, &destination) {
        Ok(()) => Ok(()),
        // Another prepare moved its own in between.
        Err(_) if installed(tools).is_some() => Ok(()),
        Err(error) => Err(format!(
            "{NAME} could not be moved into {}: {error}",
            destination.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn an_engine_is_found_only_with_the_pinned_version() {
        let folder = TempDir::new();
        assert!(locate(None).unwrap_err().contains("mutate needs --tools"));
        assert!(locate(Some(&folder.0))
            .unwrap_err()
            .contains("run `prepare --tools"));
        let bin = folder.0.join(FOLDER).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cargo-mutants"), "").unwrap();
        std::fs::write(folder.0.join(FOLDER).join(MARKER), "1.0.0\n").unwrap();
        assert!(locate(Some(&folder.0)).is_err(), "another version");
        std::fs::write(folder.0.join(FOLDER).join(MARKER), format!("{VERSION}\n")).unwrap();
        let engine = locate(Some(&folder.0)).unwrap();
        assert_eq!(engine.version, VERSION);
        assert_eq!(engine.program, bin.join("cargo-mutants"));
        // Installed already: nothing to do, no cargo run.
        assert!(install(&folder.0, Instant::now()).is_ok());
    }
}
