//! Claude Code on this machine: the `claude` command, run with `-p`, with
//! the sign-in it already has, so no key is needed.
//!
//! Flags (from `claude --help`, version 2.1.328, and
//! https://code.claude.com/docs/en/headless):
//! - `-p --output-format json`: one JSON result on stdout;
//! - `--json-schema=<schema>`: the answer comes back validated, in the
//!   result's `structured_output`;
//! - `--system-prompt=<text>`: replaces Claude Code's own coding-agent prompt;
//! - `--model=<alias or name>`;
//! - `--tools ""`: no built-in tool at all, so the model can only answer,
//!   never read or edit files or run commands;
//! - `--strict-mcp-config` (with no `--mcp-config`): no MCP server, whose
//!   tools `--tools` does not cover;
//! - `--disable-slash-commands`: no skills;
//! - `--no-session-persistence`: the question is not kept as a session.
//!
//! `--bare` is left out: it would skip hooks and CLAUDE.md, but it also
//! stops reading the user's sign-in (OAuth and the keychain), which is the
//! reason to use Claude Code at all. Instead the command runs in an empty
//! folder of its own, so no project's settings, hooks or MCP servers apply.
//! The prompt goes on standard input, since diffs are large.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::api::{self, MODEL_TIMEOUT};
use super::provider::{ModelAnswer, ModelError, ModelProvider, ModelRequest, Usage};
use crate::error::{core_text, CoreText};
use crate::model::{ClaudeCliStatus, ModelInfo, ModelProviderKind};
use crate::process::{self, Ending, Limits};
use crate::tool_path::ToolPath;

const KIND: ModelProviderKind = ModelProviderKind::ClaudeCli;
pub const DEFAULT_MODEL: &str = "opus";
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// Set by a Claude Code session for the commands it runs; a `claude`
/// started from DiffLens is not part of one.
const SESSION_VARIABLES: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

/// The model aliases `claude --model` accepts, for the latest of each line.
pub fn aliases() -> Vec<ModelInfo> {
    [
        ("opus", "Opus (latest)"),
        ("sonnet", "Sonnet (latest)"),
        ("haiku", "Haiku (latest)"),
        ("fable", "Fable (latest)"),
    ]
    .into_iter()
    .map(|(id, name)| ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
    })
    .collect()
}

/// A command to run, all of it explicit so tests can check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Written to standard input, which is then closed.
    pub input: Option<Vec<u8>>,
    pub working_dir: PathBuf,
    pub path: OsString,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// The exit code; `None` when a signal ended it.
    Exited(Option<i32>),
    TimedOut,
    OutputTooLarge,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub finish: Finish,
    pub stdout: Vec<u8>,
    pub stderr_tail: Vec<u8>,
}

/// Starts processes; tests answer instead, so they never run `claude`.
pub trait ProcessRunner: Send + Sync {
    fn run(&self, invocation: &Invocation, cancel: &AtomicBool) -> io::Result<Finished>;
}

pub struct SystemProcesses;

impl ProcessRunner for SystemProcesses {
    fn run(&self, invocation: &Invocation, cancel: &AtomicBool) -> io::Result<Finished> {
        std::fs::create_dir_all(&invocation.working_dir)?;
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .current_dir(&invocation.working_dir)
            .env("PATH", &invocation.path);
        for name in SESSION_VARIABLES {
            command.env_remove(name);
        }
        let captured = process::run_captured_with_input(
            command,
            invocation.input.clone(),
            &Limits {
                timeout: invocation.timeout,
                max_stdout_bytes: MAX_OUTPUT_BYTES,
                stderr_tail_bytes: STDERR_TAIL_BYTES,
            },
            cancel,
        )?;
        Ok(Finished {
            finish: match captured.ending {
                Ending::Exited(status) => Finish::Exited(status.code()),
                Ending::TimedOut => Finish::TimedOut,
                Ending::OutputTooLarge => Finish::OutputTooLarge,
                Ending::Cancelled => Finish::Cancelled,
            },
            stdout: captured.stdout,
            stderr_tail: captured.stderr_tail,
        })
    }
}

/// Where Claude Code's installers put `claude`, besides the PATH. An app
/// started from the Dock does not get the shell's PATH, so these are looked
/// at too.
fn install_locations(home: Option<&Path>) -> Vec<PathBuf> {
    let mut locations = Vec::new();
    if let Some(home) = home {
        for relative in [
            ".local/bin/claude",
            ".claude/local/claude",
            ".npm-global/bin/claude",
            ".bun/bin/claude",
            ".volta/bin/claude",
        ] {
            locations.push(home.join(relative));
        }
    }
    locations.push(PathBuf::from("/opt/homebrew/bin/claude"));
    locations.push(PathBuf::from("/usr/local/bin/claude"));
    locations
}

/// The first `claude` on `path`, else in the usual install locations.
pub fn find_claude(path: &std::ffi::OsStr, home: Option<&Path>) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|entry| !entry.as_os_str().is_empty())
        .map(|entry| entry.join("claude"))
        .chain(install_locations(home))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Finds and runs Claude Code.
pub struct ClaudeCode {
    tool_path: ToolPath,
    home: Option<PathBuf>,
    working_dir: PathBuf,
    runner: Arc<dyn ProcessRunner>,
}

impl ClaudeCode {
    pub fn new(
        tool_path: ToolPath,
        home: Option<PathBuf>,
        working_dir: PathBuf,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        Self {
            tool_path,
            home,
            working_dir,
            runner,
        }
    }

    pub fn locate(&self) -> Option<PathBuf> {
        find_claude(self.tool_path.get(), self.home.as_deref())
    }

    /// Where `claude` is and its version, from `claude --version`.
    pub fn status(&self) -> ClaudeCliStatus {
        let Some(program) = self.locate() else {
            return ClaudeCliStatus {
                path: None,
                version: None,
            };
        };
        let version = self
            .runner
            .run(
                &Invocation {
                    program: program.clone(),
                    args: vec!["--version".to_string()],
                    input: None,
                    working_dir: self.working_dir.clone(),
                    path: self.tool_path.get().to_os_string(),
                    timeout: VERSION_TIMEOUT,
                },
                &AtomicBool::new(false),
            )
            .ok()
            .filter(|finished| finished.finish == Finish::Exited(Some(0)))
            .and_then(|finished| {
                String::from_utf8_lossy(&finished.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_string)
            });
        ClaudeCliStatus {
            path: Some(program.display().to_string()),
            version,
        }
    }

    /// The provider asking `model`, or why there is none.
    pub fn provider(&self, model: &str) -> Result<ClaudeCodeProvider, ModelError> {
        let program = self.locate().ok_or(ModelError::CliMissing)?;
        Ok(ClaudeCodeProvider {
            program,
            model: model.to_string(),
            working_dir: self.working_dir.clone(),
            path: self.tool_path.get().to_os_string(),
            runner: Arc::clone(&self.runner),
        })
    }
}

pub struct ClaudeCodeProvider {
    program: PathBuf,
    model: String,
    working_dir: PathBuf,
    path: OsString,
    runner: Arc<dyn ProcessRunner>,
}

impl ClaudeCodeProvider {
    pub fn invocation(&self, request: &ModelRequest) -> Invocation {
        Invocation {
            program: self.program.clone(),
            args: vec![
                "-p".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                format!("--json-schema={}", request.schema),
                format!("--model={}", self.model),
                format!("--system-prompt={}", request.system),
                "--tools".to_string(),
                String::new(),
                "--strict-mcp-config".to_string(),
                "--disable-slash-commands".to_string(),
                "--no-session-persistence".to_string(),
            ],
            input: Some(request.prompt.clone().into_bytes()),
            working_dir: self.working_dir.clone(),
            path: self.path.clone(),
            timeout: MODEL_TIMEOUT,
        }
    }
}

impl ModelProvider for ClaudeCodeProvider {
    fn ask(&self, request: &ModelRequest, cancel: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        let finished = self
            .runner
            .run(&self.invocation(request), cancel)
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => ModelError::CliMissing,
                _ => ModelError::CliFailed(core_text!(
                    CliNotStarted,
                    "it could not be started ({detail})",
                    detail = error
                )),
            })?;
        let code = match finished.finish {
            Finish::Cancelled => return Err(ModelError::Cancelled),
            Finish::TimedOut => {
                return Err(ModelError::CliFailed(core_text!(
                    CliTimedOut,
                    "it did not answer within {minutes} minutes",
                    minutes = MODEL_TIMEOUT.as_secs() / 60
                )))
            }
            Finish::OutputTooLarge => {
                return Err(ModelError::CliFailed(core_text!(
                    CliPrintedTooMuch,
                    "it printed far more than an answer"
                )))
            }
            Finish::Exited(code) => code,
        };
        match result_object(&finished.stdout) {
            Some(result) => read_result(&result, &self.model, &request.schema),
            None if code == Some(0) => Err(ModelError::InvalidOutput {
                provider: KIND,
                detail: core_text!(NoJsonResult, "it did not print a JSON result"),
            }),
            None => {
                let stderr = String::from_utf8_lossy(&finished.stderr_tail);
                let reason = stderr.trim();
                Err(ModelError::CliFailed(if reason.is_empty() {
                    match code {
                        Some(code) => {
                            core_text!(CliExited, "it exited with code {code}", code = code)
                        }
                        None => core_text!(CliKilled, "it was stopped by a signal"),
                    }
                } else {
                    CoreText::raw(reason)
                }))
            }
        }
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        Ok(aliases())
    }
}

/// The result object Claude Code prints: the whole output, or its last line
/// when something printed before it.
fn result_object(stdout: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(stdout);
    let parse = |text: &str| {
        serde_json::from_str::<Value>(text.trim())
            .ok()
            .filter(|value| value["type"] == "result")
    };
    parse(&text).or_else(|| {
        text.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .and_then(parse)
    })
}

/// Reads Claude Code's result object (`SDKResultMessage` of the Agent SDK).
pub fn read_result(result: &Value, asked: &str, schema: &Value) -> Result<ModelAnswer, ModelError> {
    let subtype = result["subtype"].as_str().unwrap_or("");
    if subtype == "error_max_structured_output_retries" {
        return Err(ModelError::InvalidOutput {
            provider: KIND,
            detail: core_text!(
                SchemaNotMatched,
                "it could not write an answer matching the schema"
            ),
        });
    }
    if result["is_error"] == true || subtype != "success" {
        let text = result["result"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                let errors: Vec<&str> = result["errors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                (!errors.is_empty()).then(|| errors.join("; "))
            })
            .filter(|text| !text.trim().is_empty());
        let status = result["api_error_status"].as_u64();
        let signed_out = text.as_deref().is_some_and(|text| text.contains("/login"));
        return Err(match status {
            Some(401 | 403) => ModelError::AuthFailed {
                provider: KIND,
                detail: text,
            },
            _ if signed_out => ModelError::AuthFailed {
                provider: KIND,
                detail: text,
            },
            Some(429) => ModelError::RateLimited {
                provider: KIND,
                detail: text,
                retry_after: None,
            },
            _ => ModelError::CliFailed(match text {
                Some(text) => CoreText::raw(text),
                None => core_text!(
                    CliReportedError,
                    "it reported an error ({subtype})",
                    subtype = subtype
                ),
            }),
        });
    }
    let value = match result
        .get("structured_output")
        .filter(|value| !value.is_null())
    {
        Some(value) => value.clone(),
        None => {
            return Err(match result["stop_reason"].as_str() {
                Some("refusal") => ModelError::Refused {
                    provider: KIND,
                    category: None,
                    explanation: result["result"].as_str().map(str::to_string),
                },
                Some("max_tokens") => ModelError::Truncated {
                    provider: KIND,
                    reason: core_text!(OwnOutputLimitReached, "it reached its output limit"),
                },
                _ => ModelError::InvalidOutput {
                    provider: KIND,
                    detail: core_text!(
                        NoStructuredAnswer,
                        "it did not return a structured answer"
                    ),
                },
            })
        }
    };
    api::check_outline(KIND, &value, schema)?;
    let usage = &result["usage"];
    Ok(ModelAnswer {
        value,
        provider: KIND,
        model: answering_model(result).unwrap_or_else(|| asked.to_string()),
        usage: Some(Usage {
            input_tokens: api::sum(&[
                usage["input_tokens"].as_u64(),
                usage["cache_read_input_tokens"].as_u64(),
                usage["cache_creation_input_tokens"].as_u64(),
            ]),
            output_tokens: usage["output_tokens"].as_u64(),
        }),
    })
}

/// Claude Code reports usage per model; the one that wrote the most is the
/// one that answered (a smaller model can do housekeeping on the side).
fn answering_model(result: &Value) -> Option<String> {
    result["modelUsage"]
        .as_object()?
        .iter()
        .max_by_key(|(_, usage)| usage["outputTokens"].as_u64().unwrap_or(0))
        .map(|(model, _)| model.clone())
}

#[cfg(test)]
pub mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::ai::api::tests::{request, schema};

    /// Answers every run with `answer`, keeping the invocations; with
    /// `wait_for_cancel`, it blocks until the run is cancelled, as a slow
    /// model would.
    pub struct FakeProcesses {
        pub answer: Mutex<io::Result<Finished>>,
        pub invocations: Mutex<Vec<Invocation>>,
        pub wait_for_cancel: bool,
    }

    impl FakeProcesses {
        pub fn answering(answer: io::Result<Finished>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                invocations: Mutex::new(Vec::new()),
                wait_for_cancel: false,
            })
        }

        pub fn printing(code: i32, stdout: &str) -> Arc<Self> {
            Self::answering(Ok(Finished {
                finish: Finish::Exited(Some(code)),
                stdout: stdout.as_bytes().to_vec(),
                stderr_tail: Vec::new(),
            }))
        }

        pub fn slow() -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(Ok(Finished {
                    finish: Finish::Exited(Some(0)),
                    stdout: Vec::new(),
                    stderr_tail: Vec::new(),
                })),
                invocations: Mutex::new(Vec::new()),
                wait_for_cancel: true,
            })
        }
    }

    impl ProcessRunner for FakeProcesses {
        fn run(&self, invocation: &Invocation, cancel: &AtomicBool) -> io::Result<Finished> {
            self.invocations.lock().unwrap().push(invocation.clone());
            if self.wait_for_cancel {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                return Ok(Finished {
                    finish: Finish::Cancelled,
                    stdout: Vec::new(),
                    stderr_tail: Vec::new(),
                });
            }
            match &*self.answer.lock().unwrap() {
                Ok(finished) => Ok(finished.clone()),
                Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
            }
        }
    }

    pub fn success(structured: Value) -> Value {
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "duration_ms": 4100,
            "num_turns": 1,
            "result": "",
            "stop_reason": "end_turn",
            "session_id": "s1",
            "total_cost_usd": 0.01,
            "usage": {
                "input_tokens": 12,
                "cache_read_input_tokens": 3000,
                "cache_creation_input_tokens": 0,
                "output_tokens": 30
            },
            "modelUsage": {
                "claude-haiku-5-5": { "inputTokens": 200, "outputTokens": 5 },
                "claude-opus-5-5": { "inputTokens": 12, "outputTokens": 30 }
            },
            "permission_denials": [],
            "structured_output": structured
        })
    }

    fn provider(runner: Arc<FakeProcesses>) -> ClaudeCodeProvider {
        ClaudeCodeProvider {
            program: PathBuf::from("/opt/claude/bin/claude"),
            model: "opus".to_string(),
            working_dir: PathBuf::from("/tmp/difflens-model"),
            path: OsString::from("/usr/bin:/bin"),
            runner,
        }
    }

    fn ask(runner: &Arc<FakeProcesses>) -> Result<ModelAnswer, ModelError> {
        provider(Arc::clone(runner)).ask(&request(), &AtomicBool::new(false))
    }

    #[test]
    fn runs_claude_without_tools_with_the_prompt_on_stdin_and_reads_the_answer() {
        let runner =
            FakeProcesses::printing(0, &success(json!({"ok": true, "reply": "Hi"})).to_string());
        let answer = ask(&runner).unwrap();
        assert_eq!(answer.value, json!({"ok": true, "reply": "Hi"}));
        assert_eq!(answer.model, "claude-opus-5-5");
        assert_eq!(
            answer.usage,
            Some(Usage {
                input_tokens: Some(3012),
                output_tokens: Some(30)
            })
        );

        let invocation = &runner.invocations.lock().unwrap()[0];
        assert_eq!(invocation.program, PathBuf::from("/opt/claude/bin/claude"));
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                format!("--json-schema={}", schema()),
                "--model=opus".to_string(),
                "--system-prompt=You check that a connection works.".to_string(),
                "--tools".to_string(),
                String::new(),
                "--strict-mcp-config".to_string(),
                "--disable-slash-commands".to_string(),
                "--no-session-persistence".to_string(),
            ]
        );
        // The prompt never goes on the command line.
        assert!(!invocation.args.iter().any(|arg| arg.contains("Say hello.")));
        assert_eq!(invocation.input.as_deref(), Some("Say hello.".as_bytes()));
        assert_eq!(invocation.timeout, MODEL_TIMEOUT);
    }

    #[test]
    fn a_signed_out_cli_an_api_error_and_a_crash_read_as_what_went_wrong() {
        let signed_out = FakeProcesses::printing(
            1,
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": true,
                "result": "Invalid API key · Please run /login",
                "stop_reason": null,
                "usage": {},
                "modelUsage": {}
            })
            .to_string(),
        );
        assert!(matches!(
            ask(&signed_out),
            Err(ModelError::AuthFailed { provider: KIND, .. })
        ));

        let limited = FakeProcesses::printing(
            1,
            &json!({
                "type": "result",
                "subtype": "success",
                "is_error": true,
                "api_error_status": 429,
                "result": "API Error: rate limited"
            })
            .to_string(),
        );
        assert!(matches!(ask(&limited), Err(ModelError::RateLimited { .. })));

        let retries = FakeProcesses::printing(
            1,
            &json!({
                "type": "result",
                "subtype": "error_max_structured_output_retries",
                "is_error": true,
                "errors": ["could not match the schema"]
            })
            .to_string(),
        );
        assert!(matches!(
            ask(&retries),
            Err(ModelError::InvalidOutput { .. })
        ));

        let crashed = FakeProcesses::answering(Ok(Finished {
            finish: Finish::Exited(Some(2)),
            stdout: Vec::new(),
            stderr_tail: b"error: unknown option '--json-schema'".to_vec(),
        }));
        assert_eq!(
            ask(&crashed),
            Err(ModelError::CliFailed(CoreText::raw(
                "error: unknown option '--json-schema'"
            )))
        );

        let missing = FakeProcesses::answering(Err(io::Error::from(io::ErrorKind::NotFound)));
        assert_eq!(ask(&missing), Err(ModelError::CliMissing));

        let timed_out = FakeProcesses::answering(Ok(Finished {
            finish: Finish::TimedOut,
            stdout: Vec::new(),
            stderr_tail: Vec::new(),
        }));
        assert!(matches!(ask(&timed_out), Err(ModelError::CliFailed(_))));
    }

    #[test]
    fn a_result_without_structured_output_says_why() {
        let mut refused = success(Value::Null);
        refused.as_object_mut().unwrap().remove("structured_output");
        refused["stop_reason"] = json!("refusal");
        assert!(matches!(
            read_result(&refused, "opus", &schema()),
            Err(ModelError::Refused { .. })
        ));
        refused["stop_reason"] = json!("end_turn");
        assert!(matches!(
            read_result(&refused, "opus", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
        let incomplete = success(json!({"ok": true}));
        assert!(matches!(
            read_result(&incomplete, "opus", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
    }

    #[test]
    fn a_cancelled_run_reads_as_cancelled() {
        let runner = FakeProcesses::slow();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            provider(runner).ask(&request(), &cancel),
            Err(ModelError::Cancelled)
        );
    }

    #[cfg(unix)]
    #[test]
    fn finds_claude_on_the_path_then_where_installers_put_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let make = |relative: &str, mode: u32| {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let home = dir.path().join("home");
        let installed = make("home/.local/bin/claude", 0o755);
        let not_executable = make("bin-a/claude", 0o644);
        let on_path = make("bin-b/claude", 0o755);

        let path =
            std::env::join_paths([not_executable.parent().unwrap(), on_path.parent().unwrap()])
                .unwrap();
        assert_eq!(find_claude(&path, Some(&home)), Some(on_path));
        assert_eq!(
            find_claude(OsString::from("").as_os_str(), Some(&home)),
            Some(installed)
        );
        assert_eq!(
            find_claude(
                OsString::from("").as_os_str(),
                Some(&dir.path().join("nobody"))
            )
            .filter(|found| found.starts_with(dir.path())),
            None
        );
    }

    #[test]
    fn reports_where_claude_is_and_its_version() {
        let dir = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.path().join("claude");
            std::fs::write(&program, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let runner = FakeProcesses::printing(0, "2.1.328 (Claude Code)\n");
        let claude = ClaudeCode::new(
            ToolPath::fixed(dir.path().as_os_str()),
            None,
            dir.path().join("work"),
            Arc::clone(&runner) as Arc<dyn ProcessRunner>,
        );
        let status = claude.status();
        assert_eq!(status.version.as_deref(), Some("2.1.328 (Claude Code)"));
        assert_eq!(
            status.path,
            Some(dir.path().join("claude").display().to_string())
        );
        assert_eq!(runner.invocations.lock().unwrap()[0].args, ["--version"]);
    }
}
