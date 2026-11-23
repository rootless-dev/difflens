//! The macOS sandbox `check` runs clippy in. `cargo clippy` compiles the
//! project, which runs its build scripts and proc macros: project code. So
//! it runs under `sandbox-exec` with the network off and writes allowed only
//! in a scratch folder of the check (its copy and TMPDIR), the folders it
//! is given besides (the target folder kept in --cache-dir) and the devices
//! programs write to, as the core's own sandbox for tests does. Paths enter
//! the profile as parameters, never inside its text.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `sandbox-exec` is on every macOS.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The profile, with `extra` more writable folders (`WRITABLE<n>`).
fn profile(extra: usize) -> String {
    let writable: String = (0..extra)
        .map(|index| format!("\n    (subpath (param \"WRITABLE{index}\"))"))
        .collect();
    format!(
        r#"(version 1)
(allow default)
(deny network*)
(allow network* (remote unix-socket (subpath (param "SCRATCH"))))
(deny file-write*)
(allow file-write*
    (subpath (param "SCRATCH")){writable}
    (literal "/dev/null")
    (literal "/dev/zero")
    (literal "/dev/tty")
    (literal "/dev/stdout")
    (literal "/dev/stderr")
    (literal "/dev/dtracehelper")
    (subpath "/dev/fd"))
"#
    )
}

/// Whether the sandbox can be used here.
pub fn available() -> bool {
    cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file()
}

/// A command running `program` with `args` under the sandbox, writing only
/// inside `scratch` and the `writable` folders (which must exist).
pub fn command(scratch: &Path, writable: &[&Path], program: &str, args: &[&str]) -> Command {
    let scratch = canonical(scratch);
    let mut command = Command::new(SANDBOX_EXEC);
    command
        .arg("-p")
        .arg(profile(writable.len()))
        .arg("-D")
        .arg(format!("SCRATCH={}", scratch.display()));
    for (index, folder) in writable.iter().enumerate() {
        command
            .arg("-D")
            .arg(format!("WRITABLE{index}={}", canonical(folder).display()));
    }
    command.arg("--").arg(program).args(args);
    command
}

/// The sandbox compares resolved paths (`/var` is `/private/var`).
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_outside_the_scratch_folder_are_denied() {
        if !available() {
            return;
        }
        let scratch = std::env::temp_dir().join(format!("difflens-sandbox-{}", std::process::id()));
        let cache = std::env::temp_dir().join(format!("difflens-cache-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let writable = [cache.as_path()];
        for allowed in [scratch.join("inside"), cache.join("inside")] {
            let status = command(&scratch, &writable, "touch", &[allowed.to_str().unwrap()])
                .status()
                .unwrap();
            assert!(status.success(), "{}", allowed.display());
        }
        let outside = std::env::temp_dir().join(format!("difflens-outside-{}", std::process::id()));
        let status = command(&scratch, &writable, "touch", &[outside.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(!outside.exists());
        // Without the folder, only the scratch folder is writable.
        let denied = cache.join("denied");
        let status = command(&scratch, &[], "touch", &[denied.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(!denied.exists());
        std::fs::remove_dir_all(&scratch).unwrap();
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
