//! Temporary git repositories for tests, built with a fixed identity and
//! without the configuration of the machine running them.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tempfile::TempDir;

use crate::ai::{ModelAnswer, ModelError, ModelProvider, ModelRequest, ModelService};
use crate::git::Git;
use crate::model::{ModelInfo, ModelProviderKind};

/// 2026-01-01T00:00:00Z. Each commit of a `TestRepo` is one second later than
/// the previous one, so ordering by date is deterministic.
const FIRST_COMMIT_EPOCH: u64 = 1_767_225_600;

/// Seconds after `FIRST_COMMIT_EPOCH` at which the commits of a clone start.
const CLONE_FIRST_COMMIT_OFFSET: u64 = 100_000;

const ISOLATION: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];

/// The git layer under test, cut off from user and system configuration.
pub fn isolated_git() -> Git {
    ISOLATION.iter().fold(Git::new(), |git, (key, value)| {
        git.with_environment(key, value)
    })
}

/// Runs git in `directory` for test setup, panics when it fails, returns
/// trimmed stdout.
pub fn run_git(directory: &Path, arguments: &[&str]) -> String {
    run_git_at(directory, 0, arguments)
}

fn run_git_at(directory: &Path, seconds_after_first_commit: u64, arguments: &[&str]) -> String {
    let date = format!("{} +0200", FIRST_COMMIT_EPOCH + seconds_after_first_commit);
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .envs(ISOLATION.iter().copied())
        // Setup commits must not leave a detached maintenance process
        // writing to the repository while a test inspects it.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "gc.auto")
        .env("GIT_CONFIG_VALUE_0", "0")
        .env("GIT_CONFIG_KEY_1", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_1", "false")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A model that must not be asked: a test of what asks nothing.
pub struct NeverAsked;

impl ModelProvider for NeverAsked {
    fn ask(&self, _request: &ModelRequest, _cancel: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        panic!("the model was asked");
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        Ok(Vec::new())
    }
}

/// How a `ScriptedModel` answers a question; `None` never answers.
type Script = dyn Fn(&ModelRequest) -> Option<Result<Value, ModelError>> + Send + Sync;

/// Answers each question with `answer(request)`, keeping the questions.
/// A question whose answer is `None` waits until its cancel flag is raised,
/// as a call that never answers on its own.
pub struct ScriptedModel {
    answer: Box<Script>,
    asked: Mutex<Vec<ModelRequest>>,
    /// Who its answers say wrote them.
    provider: ModelProviderKind,
    model: String,
}

impl ScriptedModel {
    pub fn new(
        answer: impl Fn(&ModelRequest) -> Option<Result<Value, ModelError>> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::writing_as(
            ModelProviderKind::Anthropic,
            "claude-opus-5-5-20261101",
            answer,
        )
    }

    /// As `new`, its answers said to be written by `model` of `provider`.
    pub fn writing_as(
        provider: ModelProviderKind,
        model: &str,
        answer: impl Fn(&ModelRequest) -> Option<Result<Value, ModelError>> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            answer: Box::new(answer),
            asked: Mutex::default(),
            provider,
            model: model.to_string(),
        })
    }

    pub fn asked(&self) -> Vec<ModelRequest> {
        self.asked.lock().unwrap().clone()
    }

    /// A model service whose provider in use is this one.
    pub fn service(self: &Arc<Self>) -> ModelService {
        ModelService::answered_by(
            Arc::clone(self) as Arc<dyn ModelProvider>,
            ModelProviderKind::Anthropic,
            "claude-opus-5-5",
        )
    }
}

impl ModelProvider for ScriptedModel {
    fn ask(&self, request: &ModelRequest, cancel: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        self.asked.lock().unwrap().push(request.clone());
        let Some(answer) = (self.answer)(request) else {
            while !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            return Err(ModelError::Cancelled);
        };
        answer.map(|value| ModelAnswer {
            value,
            provider: self.provider,
            model: self.model.clone(),
            usage: None,
        })
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        Ok(Vec::new())
    }
}

pub struct TestRepo {
    path: PathBuf,
    commits: Cell<u64>,
    // Dropped last, which deletes the folder.
    _dir: TempDir,
}

impl TestRepo {
    /// An empty repository on branch `main`.
    pub fn init() -> Self {
        Self::init_with_branch("main")
    }

    pub fn init_with_branch(branch: &str) -> Self {
        let repo = Self::empty_folder(0);
        fs::create_dir(&repo.path).unwrap();
        repo.git(&["init", "--quiet", "--initial-branch", branch]);
        repo
    }

    /// A clone of `source` (a path or URL) in a new temporary folder, so it
    /// has `origin`. Its commits are dated after those of the source.
    pub fn clone_from(source: &Path) -> Self {
        let repo = Self::empty_folder(CLONE_FIRST_COMMIT_OFFSET);
        run_git(
            repo.parent(),
            &[
                "clone",
                "--quiet",
                source.to_str().unwrap(),
                repo.path.to_str().unwrap(),
            ],
        );
        repo
    }

    fn empty_folder(first_commit_offset: u64) -> Self {
        let dir = TempDir::new().unwrap();
        // Canonical, because the temporary folder is behind a symlink on macOS
        // and git reports resolved paths.
        let root = dir.path().canonicalize().unwrap();
        Self {
            path: root.join("repo with space"),
            commits: Cell::new(first_commit_offset),
            _dir: dir,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Folder that holds the repository; a place for sibling folders.
    pub fn parent(&self) -> &Path {
        self.path.parent().unwrap()
    }

    /// Runs git in the repository, dated at the latest commit made here.
    pub fn git(&self, arguments: &[&str]) -> String {
        run_git_at(&self.path, self.commits.get(), arguments)
    }

    pub fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let file = self.path.join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, contents).unwrap();
    }

    /// Writes one file and commits it.
    pub fn commit_file(&self, relative: &str, contents: impl AsRef<[u8]>, message: &str) {
        self.write(relative, contents);
        self.commit_all(message);
    }

    /// Commits everything in the working tree.
    pub fn commit_all(&self, message: &str) {
        self.commits.set(self.commits.get() + 1);
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
    }
}
