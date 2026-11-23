//! The macOS sandbox that tests run in when isolation is `sandbox`:
//! `sandbox-exec` with a profile (SBPL) generated here.
//!
//! What the profile allows, and why each part is there (found by running
//! `go test -race -cover` under it with a cold build cache):
//! - Everything by default, then two denials: the network and file writes.
//! - Network: outgoing connections only to loopback; binding to any local
//!   address (`net.Listen(":0")` is common) but accepting only on loopback;
//!   Unix sockets only inside the run folder and the run's TMPDIR. So a test
//!   can start and call a local server, while an outside host, and DNS (the
//!   resolver's socket is elsewhere), stay out of reach.
//! - File writes only inside the run folder (the copy of the commit), the
//!   run's TMPDIR (a short folder of its own, so socket paths under it fit
//!   the 104-byte limit), the folders the runner's prepare step
//!   reported (for Go, the build cache), the project's build cache of the
//!   analyzer (`build_cache`, for Rust its target folder), and the
//!   character devices programs
//!   expect to write to: `/dev/null` (os/exec gives a child without a
//!   stdout a `/dev/null` opened for writing), `/dev/zero`, the terminal,
//!   standard output and error through `/dev/fd`, and `/dev/dtracehelper`,
//!   which dyld opens at every program start.
//!
//! Paths enter the profile as parameters (`-D NAME=value`, read with
//! `(param "NAME")`), never inside its text, so no path needs quoting. They
//! are canonical, because the sandbox compares resolved paths (`/var` is
//! `/private/var` on macOS).
//!
//! Not covered: reading files is allowed, and so is talking to other
//! processes of the machine (loopback services, Mach services such as
//! LaunchServices). The profile stops a test from reaching the network
//! itself and from changing the user's files. Since reading and executing
//! are allowed everywhere, an analyzer's launcher runs under it as it is: a
//! sidecar, or a runtime (python, node) with the analyzer's environment in
//! the app's data folder (`analyzer_envs`), which stays read-only there (the
//! Python launcher writes no bytecode).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `sandbox-exec` is on every macOS.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

const RUN_DIRECTORY: &str = "RUN_DIRECTORY";
const TEMPORARY: &str = "TEMPORARY";
const WRITABLE: &str = "WRITABLE_";

/// Device files a test may write to.
const DEVICE_LITERALS: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/tty",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/dtracehelper",
];

/// What one sandboxed run may write to, besides the devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// Holds the copy of the commit.
    pub run_directory: PathBuf,
    /// The run's TMPDIR.
    pub temporary: PathBuf,
    /// Folders the runner reported in its prepare step, such as a build cache.
    pub writable: Vec<PathBuf>,
}

/// The profile for a policy with `writable_count` extra folders. Paths come
/// as parameters, see `parameters`.
pub fn profile(writable_count: usize) -> String {
    let mut writable = format!(
        "    (subpath (param \"{RUN_DIRECTORY}\"))\n    (subpath (param \"{TEMPORARY}\"))\n"
    );
    for index in 0..writable_count {
        writable.push_str(&format!("    (subpath (param \"{WRITABLE}{index}\"))\n"));
    }
    for device in DEVICE_LITERALS {
        writable.push_str(&format!("    (literal \"{device}\")\n"));
    }
    writable.push_str("    (subpath \"/dev/fd\")");
    format!(
        "(version 1)\n\
         (allow default)\n\
         (deny network*)\n\
         (allow network-outbound (remote ip \"localhost:*\"))\n\
         (allow network-bind (local ip \"*:*\"))\n\
         (allow network-inbound (local ip \"localhost:*\"))\n\
         (allow network-bind network-outbound\n    \
             (local unix-socket (subpath (param \"{RUN_DIRECTORY}\")))\n    \
             (remote unix-socket (subpath (param \"{RUN_DIRECTORY}\")))\n    \
             (local unix-socket (subpath (param \"{TEMPORARY}\")))\n    \
             (remote unix-socket (subpath (param \"{TEMPORARY}\"))))\n\
         (deny file-write*)\n\
         (allow file-write*\n{writable})\n"
    )
}

/// The `-D` arguments that give the profile its paths, canonical.
pub fn parameters(policy: &Policy) -> Vec<OsString> {
    let mut arguments = Vec::new();
    let mut define = |name: String, path: &Path| {
        arguments.push(OsString::from("-D"));
        let mut value = OsString::from(format!("{name}="));
        value.push(canonical(path).as_os_str());
        arguments.push(value);
    };
    define(RUN_DIRECTORY.to_string(), &policy.run_directory);
    define(TEMPORARY.to_string(), &policy.temporary);
    for (index, folder) in policy.writable.iter().enumerate() {
        define(format!("{WRITABLE}{index}"), folder);
    }
    arguments
}

/// A command that runs `program` with `arguments` under the sandbox, with
/// `sandbox_exec` (normally `SANDBOX_EXEC`).
pub fn command(
    sandbox_exec: &Path,
    policy: &Policy,
    program: &Path,
    arguments: &[OsString],
) -> Command {
    let mut command = Command::new(sandbox_exec);
    command
        .arg("-p")
        .arg(profile(policy.writable.len()))
        .args(parameters(policy))
        .arg("--")
        .arg(program)
        .args(arguments);
    command
}

/// `path` with its existing part resolved: the deepest folder that exists
/// is canonicalised and the rest appended, so a cache folder that is created
/// only later is still named as the sandbox will see it.
pub fn canonical(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(resolved, |resolved, part| resolved.join(part));
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_network_and_writes_then_opens_what_tests_need() {
        let text = profile(2);
        assert!(text.starts_with("(version 1)\n(allow default)\n(deny network*)\n"));
        assert!(text.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
        assert!(text.contains("(allow network-inbound (local ip \"localhost:*\"))"));
        assert!(
            !text.contains("(allow network* (local ip"),
            "a local-address rule on every operation lets connections out"
        );
        let writes = text.split("(deny file-write*)").nth(1).unwrap();
        assert!(text.contains("(local unix-socket (subpath (param \"TEMPORARY\")))"));
        for expected in [
            "(subpath (param \"RUN_DIRECTORY\"))",
            "(subpath (param \"TEMPORARY\"))",
            "(subpath (param \"WRITABLE_0\"))",
            "(subpath (param \"WRITABLE_1\"))",
            "(literal \"/dev/null\")",
            "(literal \"/dev/dtracehelper\")",
            "(subpath \"/dev/fd\")",
        ] {
            assert!(writes.contains(expected), "{expected} missing from {text}");
        }
        assert!(!writes.contains("WRITABLE_2"));
        let opened = text.matches('(').count();
        assert_eq!(opened, text.matches(')').count(), "balanced: {text}");
    }

    #[test]
    fn no_extra_folder_means_only_the_run_folder() {
        let text = profile(0);
        assert!(!text.contains("WRITABLE_"));
        assert!(text.contains("(subpath (param \"RUN_DIRECTORY\"))"));
    }

    #[test]
    fn paths_are_passed_as_canonical_parameters() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run \"quoted\"");
        std::fs::create_dir(&run).unwrap();
        let later = dir.path().join("cache").join("go-build");
        let policy = Policy {
            run_directory: run.clone(),
            temporary: dir.path().join("tmp"),
            writable: vec![later],
        };

        let arguments = parameters(&policy);

        let root = dir.path().canonicalize().unwrap();
        let expected = |name: &str, path: PathBuf| {
            let mut value = OsString::from(format!("{name}="));
            value.push(path.as_os_str());
            value
        };
        assert_eq!(
            arguments,
            vec![
                OsString::from("-D"),
                expected("RUN_DIRECTORY", root.join("run \"quoted\"")),
                OsString::from("-D"),
                expected("TEMPORARY", root.join("tmp")),
                OsString::from("-D"),
                expected("WRITABLE_0", root.join("cache").join("go-build")),
            ]
        );
    }

    #[test]
    fn the_command_wraps_the_program() {
        let policy = Policy {
            run_directory: PathBuf::from("/nonexistent-run"),
            temporary: PathBuf::from("/nonexistent-tmp"),
            writable: Vec::new(),
        };
        let command = command(
            Path::new(SANDBOX_EXEC),
            &policy,
            Path::new("/bin/runner"),
            &[OsString::from("test"), OsString::from("--offline")],
        );
        assert_eq!(command.get_program(), SANDBOX_EXEC);
        let arguments: Vec<_> = command.get_args().collect();
        assert_eq!(arguments[0], "-p");
        assert_eq!(arguments[2], "-D");
        assert_eq!(arguments[3], "RUN_DIRECTORY=/nonexistent-run");
        assert_eq!(arguments[5], "TEMPORARY=/nonexistent-tmp");
        assert_eq!(&arguments[6..], ["--", "/bin/runner", "test", "--offline"]);
    }

    /// Runs shell commands under the generated profile: writes inside the run
    /// folder and to /dev/null work; writes elsewhere and connections to an
    /// outside host do not; a local server can be reached.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "runs sandbox-exec and needs nc and python3; checks the profile on this machine"]
    fn the_profile_blocks_outside_writes_and_hosts_but_not_loopback() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = dir.path().join("run");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let policy = Policy {
            run_directory: run.clone(),
            temporary: dir.path().join("tmp"),
            writable: Vec::new(),
        };
        let shell = |script: &str| {
            command(
                Path::new(SANDBOX_EXEC),
                &policy,
                Path::new("/bin/sh"),
                &[OsString::from("-c"), OsString::from(script)],
            )
            .status()
            .unwrap()
            .success()
        };
        let quoted = |path: &Path| format!("'{}'", path.display());

        assert!(shell(&format!("echo x > {}/inside", quoted(&run))));
        assert!(shell("echo x > /dev/null"));
        assert!(!shell(&format!("echo x > {}/escaped", quoted(&outside))));
        assert!(!outside.join("escaped").exists());
        assert!(!shell("/usr/bin/nc -z -w 3 1.1.1.1 443"));
        let server = "import socket,threading\n\
            s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(1)\n\
            threading.Thread(target=lambda: s.accept()[0].close()).start()\n\
            c=socket.create_connection(('127.0.0.1', s.getsockname()[1]), 3); c.close()";
        assert!(shell(&format!("/usr/bin/python3 -c \"{server}\"")));
    }

    /// `go test -race -cover` of a module whose tests start a local server,
    /// use t.TempDir, a Unix socket, a subprocess, and probe the walls: the
    /// probes fail the run if an outside host or an outside write works.
    /// The build cache is the only folder opened besides the run folder,
    /// as the Go runner's prepare step reports it.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "runs the Go toolchain under sandbox-exec; needs go on the PATH"]
    fn go_test_with_race_and_cover_works_in_the_sandbox() {
        let dir = tempfile::TempDir::new().unwrap();
        let run = canonical(dir.path()).join("run");
        let tree = run.join("tree");
        // Short and outside the run folder, as TestsService makes it, so a
        // socket under t.TempDir() fits the 104-byte limit.
        let short = tempfile::Builder::new().prefix("dl-").tempdir_in("/private/tmp").unwrap();
        let temporary = canonical(short.path());
        let outside = canonical(dir.path()).join("outside");
        for folder in [&tree, &outside] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(tree.join("go.mod"), "module example.com/demo\n\ngo 1.22\n").unwrap();
        std::fs::write(
            tree.join("demo.go"),
            "package demo\n\nfunc Add(a, b int) int {\n\tif a > 100 {\n\t\treturn a\n\t}\n\treturn a + b\n}\n",
        )
        .unwrap();
        std::fs::write(
            tree.join("demo_test.go"),
            super::fixtures::go_probe_tests(&outside),
        )
        .unwrap();
        let output = std::process::Command::new("go")
            .args(["env", "GOCACHE"])
            .output()
            .unwrap();
        let gocache = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let policy = Policy {
            run_directory: run.clone(),
            temporary: temporary.clone(),
            writable: vec![gocache],
        };
        let go = String::from_utf8(
            std::process::Command::new("/bin/sh")
                .args(["-c", "command -v go"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();

        let result = command(
            Path::new(SANDBOX_EXEC),
            &policy,
            Path::new(go.trim()),
            &["test", "-race", "-cover", "-count=1", "-v", "./..."].map(OsString::from),
        )
        .current_dir(&tree)
        .env("TMPDIR", &temporary)
        .env("GOFLAGS", "-mod=mod")
        .env("GOPROXY", "off")
        .output()
        .unwrap();

        let printed = String::from_utf8_lossy(&result.stdout);
        println!("{printed}{}", String::from_utf8_lossy(&result.stderr));
        assert!(result.status.success(), "go test failed in the sandbox");
        assert!(printed.contains("coverage:"), "{printed}");
        assert!(!outside.join("escaped").exists());
        assert!(tree.join("written-in-tree").exists());
    }
}

#[cfg(test)]
pub mod fixtures {
    use std::path::Path;

    /// Go tests of a package `demo` with `Add(a, b int) int`: a local
    /// server, t.TempDir, a Unix socket, a subprocess, and `TestWalls`, which
    /// passes only where writing to `outside` and reaching an outside host
    /// fail.
    pub fn go_probe_tests(outside: &Path) -> String {
        let outside = outside.display();
        format!(
            r#"package demo

import (
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func TestAdd(t *testing.T) {{
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {{ defer wg.Done(); _ = Add(1, 2) }}()
	wg.Wait()
	if Add(1, 2) != 3 {{
		t.Fatal("bad sum")
	}}
}}

func TestLocalServer(t *testing.T) {{
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		_, _ = io.WriteString(w, "ok")
	}}))
	defer server.Close()
	response, err := http.Get(server.URL)
	if err != nil {{
		t.Fatal(err)
	}}
	body, _ := io.ReadAll(response.Body)
	if string(body) != "ok" {{
		t.Fatal(string(body))
	}}
	listener, err := net.Listen("tcp", ":0")
	if err != nil {{
		t.Fatal(err)
	}}
	listener.Close()
}}

func TestFiles(t *testing.T) {{
	if err := os.WriteFile(filepath.Join(t.TempDir(), "x"), []byte("x"), 0o644); err != nil {{
		t.Fatal(err)
	}}
	if err := os.WriteFile("written-in-tree", []byte("x"), 0o644); err != nil {{
		t.Fatal(err)
	}}
	if err := exec.Command("true").Run(); err != nil {{
		t.Fatal(err)
	}}
	// Relative: the run folder's absolute path is longer than a socket
	// address may be.
	unixRoundTrip(t, "s.sock")
	// Where tests usually put one: under t.TempDir(), in the short TMPDIR.
	unixRoundTrip(t, filepath.Join(t.TempDir(), "s.sock"))
}}

func unixRoundTrip(t *testing.T, path string) {{
	t.Helper()
	socket, err := net.Listen("unix", path)
	if err != nil {{
		t.Fatal(err)
	}}
	defer socket.Close()
	go func() {{
		if conn, err := socket.Accept(); err == nil {{
			conn.Close()
		}}
	}}()
	conn, err := net.Dial("unix", path)
	if err != nil {{
		t.Fatal(err)
	}}
	conn.Close()
}}

func TestWalls(t *testing.T) {{
	if err := os.WriteFile("{outside}/escaped", []byte("x"), 0o644); err == nil {{
		t.Error("wrote outside the run folder")
	}}
	if conn, err := net.DialTimeout("tcp", "1.1.1.1:443", 3*time.Second); err == nil {{
		conn.Close()
		t.Error("reached an outside host")
	}}
	if _, err := net.LookupHost("example.com"); err == nil {{
		t.Error("resolved an outside name")
	}}
}}
"#
        )
    }
}
