//! The PATH analyzers run with, so they find the language toolchains (such as
//! `go`). An app started from the Dock or Finder gets a minimal PATH, so on
//! Unix the user's login shell is asked for its PATH once, and that comes
//! first, followed by the entries the app was started with.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::process::{self, Ending, Limits};

/// Printed right before the PATH, so whatever the shell's startup files print
/// can be told apart from it.
const MARKER: &str = "__DIFFLENS_PATH__";
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SHELL_OUTPUT_BYTES: usize = 1024 * 1024;

pub struct ToolPath {
    value: OnceLock<OsString>,
    shell: Option<PathBuf>,
}

impl ToolPath {
    /// Asks the login shell named by `$SHELL` on first use.
    pub fn from_login_shell() -> Self {
        let shell = if cfg!(unix) {
            Some(
                std::env::var_os("SHELL")
                    .filter(|shell| !shell.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/bin/sh")),
            )
        } else {
            None
        };
        Self {
            value: OnceLock::new(),
            shell,
        }
    }

    /// Always `value`; for tests.
    #[cfg(test)]
    pub fn fixed(value: impl Into<OsString>) -> Self {
        let path = Self {
            value: OnceLock::new(),
            shell: None,
        };
        let _ = path.value.set(value.into());
        path
    }

    /// The PATH, computed on the first call. Falls back to the PATH of this
    /// process when the shell cannot be asked.
    pub fn get(&self) -> &OsStr {
        self.value.get_or_init(|| {
            let current = std::env::var_os("PATH");
            let from_shell = self
                .shell
                .as_ref()
                .and_then(|shell| login_shell_path(shell, SHELL_TIMEOUT));
            merge_paths(from_shell.as_deref(), current.as_deref())
        })
    }
}

/// The PATH `shell` reports as a login shell, or `None` when it fails, prints
/// nothing usable or takes longer than `timeout`.
pub fn login_shell_path(shell: &std::path::Path, timeout: Duration) -> Option<OsString> {
    let mut command = Command::new(shell);
    command.args(["-l", "-c", &format!("printf '%s%s' '{MARKER}' \"$PATH\"")]);
    let captured = process::run_captured(
        command,
        &Limits {
            timeout,
            max_stdout_bytes: MAX_SHELL_OUTPUT_BYTES,
            stderr_tail_bytes: 0,
        },
    )
    .ok()?;
    match captured.ending {
        Ending::Exited(status) if status.success() => path_from_shell_output(&captured.stdout),
        _ => None,
    }
}

/// What follows the last marker, without a trailing newline.
fn path_from_shell_output(stdout: &[u8]) -> Option<OsString> {
    let marker = MARKER.as_bytes();
    let start = stdout
        .windows(marker.len())
        .rposition(|window| window == marker)?
        + marker.len();
    let value = stdout[start..]
        .strip_suffix(b"\n")
        .unwrap_or(&stdout[start..]);
    (!value.is_empty()).then(|| os_string_from_bytes(value))
}

#[cfg(unix)]
fn os_string_from_bytes(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(bytes).to_os_string()
}

#[cfg(not(unix))]
fn os_string_from_bytes(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

/// The entries of `first` then those of `second`, each once, empty ones left
/// out.
fn merge_paths(first: Option<&OsStr>, second: Option<&OsStr>) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    for list in [first, second].into_iter().flatten() {
        for entry in std::env::split_paths(list) {
            if !entry.as_os_str().is_empty() && !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }
    std::env::join_paths(entries)
        .ok()
        .or_else(|| second.map(OsStr::to_os_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_follows_the_last_marker() {
        let output = format!("Welcome!\nfake {MARKER}x\n{MARKER}/opt/go/bin:/usr/bin\n");
        assert_eq!(
            path_from_shell_output(output.as_bytes()),
            Some(OsString::from("/opt/go/bin:/usr/bin"))
        );
        assert_eq!(path_from_shell_output(b"no marker"), None);
        assert_eq!(path_from_shell_output(MARKER.as_bytes()), None);
    }

    #[cfg(unix)]
    #[test]
    fn merging_keeps_order_and_drops_duplicates() {
        let merged = merge_paths(
            Some(OsStr::new("/opt/go/bin:/usr/bin")),
            Some(OsStr::new("/usr/bin::/bin")),
        );
        assert_eq!(merged, OsString::from("/opt/go/bin:/usr/bin:/bin"));
        assert_eq!(
            merge_paths(None, Some(OsStr::new("/bin"))),
            OsString::from("/bin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn asks_a_login_shell_and_gives_up_on_a_slow_or_broken_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let write_shell = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        // Stands in for a shell: checks it was asked for a login shell, then
        // runs the command with a PATH set by "startup files".
        let good = write_shell(
            "good",
            "[ \"$1\" = -l ] || exit 9\necho 'motd from profile'\nPATH=/opt/tools/bin:/usr/bin exec /bin/sh -c \"$3\"",
        );
        let slow = write_shell("slow", "exec sleep 30");
        let broken = write_shell("broken", "exit 1");

        assert_eq!(
            login_shell_path(&good, Duration::from_secs(5)),
            Some(OsString::from("/opt/tools/bin:/usr/bin"))
        );
        let started = std::time::Instant::now();
        assert_eq!(login_shell_path(&slow, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(login_shell_path(&broken, Duration::from_secs(5)), None);
        assert_eq!(
            login_shell_path(&dir.path().join("missing"), Duration::from_secs(1)),
            None
        );
    }
}
