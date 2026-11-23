//! What the commands ask of the model: its settings, its keys, the models
//! a provider offers, and questions, composed from the database, the
//! Keychain, the HTTP transport and Claude Code.
//!
//! Nothing here asks a model on its own. Saving settings or a key never
//! does (a key is checked by listing models, which costs nothing); only
//! `ask` does, and only a click leads to it.
//!
//! Every question is asked for a task (`ModelTask`), which has a model of
//! its own or uses the default one (the provider in use, with its model and
//! effort). A task whose provider is not configured any more fails naming
//! the task, rather than falling back to the default without saying so.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use super::anthropic::{self, Anthropic};
use super::claude_cli::{self, ClaudeCode, SystemProcesses};
use super::gemini::{self, Gemini};
use super::openai::{self, OpenAi};
use super::openrouter::{self, OpenRouter};
use super::provider::{provider_phrase, ModelAnswer, ModelError, ModelProvider, ModelRequest};
use super::runs::ModelRuns;
use crate::error::{app_error, core_text, AppError, AppResult, CoreText};
use crate::flow;
use crate::http::{ReqwestTransport, Secret, Transport};
use crate::model::{
    Effort, ModelInUse, ModelInfo, ModelKeySaved, ModelProviderKind, ModelProviderState,
    ModelSettings, ModelTask, ModelTaskChoice, ModelTaskState, ModelTest,
};
use crate::secrets::{KeychainSecrets, SecretStore};
use crate::store::{ModelTaskRecord, Store};
use crate::tool_path::ToolPath;

impl ModelTask {
    pub const ALL: [ModelTask; 7] = [
        ModelTask::Insights,
        ModelTask::Summary,
        ModelTask::BriefExplanation,
        ModelTask::DetailedExplanation,
        ModelTask::TestDiagnosis,
        ModelTask::ReviewComment,
        ModelTask::Translation,
    ];

    /// Stable key the task's model is saved under.
    pub fn key(self) -> &'static str {
        match self {
            Self::Insights => "insights",
            Self::Summary => "summary",
            Self::BriefExplanation => "brief_explanation",
            Self::DetailedExplanation => "detailed_explanation",
            Self::TestDiagnosis => "test_diagnosis",
            Self::ReviewComment => "review_comment",
            Self::Translation => "translation",
        }
    }
}

/// Setting that holds the key of the provider in use.
const ACTIVE_PROVIDER: &str = "model.provider";
/// Used until the user picks one: it needs no key.
const DEFAULT_PROVIDER: ModelProviderKind = ModelProviderKind::ClaudeCli;

impl From<AppError> for ModelError {
    fn from(error: AppError) -> Self {
        ModelError::Internal(error)
    }
}

pub struct ModelService {
    secrets: Arc<dyn SecretStore>,
    transport: Arc<dyn Transport>,
    claude: ClaudeCode,
    runs: ModelRuns,
    /// Tests of what asks the model answer with this provider, whatever the
    /// settings say.
    #[cfg(test)]
    fixed: Option<Fixed>,
    /// And the tasks given one of their own (`with_task`) with that one.
    #[cfg(test)]
    fixed_tasks: Vec<(ModelTask, Fixed)>,
}

#[cfg(test)]
struct Fixed {
    provider: Arc<dyn ModelProvider>,
    kind: ModelProviderKind,
    model: String,
}

impl ModelService {
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        transport: Arc<dyn Transport>,
        claude: ClaudeCode,
    ) -> Self {
        Self {
            secrets,
            transport,
            claude,
            runs: ModelRuns::default(),
            #[cfg(test)]
            fixed: None,
            #[cfg(test)]
            fixed_tasks: Vec::new(),
        }
    }

    /// A service whose provider in use is `provider`, said to be `kind`
    /// asking `model`, so code that asks the model can be tested without
    /// reaching one.
    #[cfg(test)]
    pub fn answered_by(
        provider: Arc<dyn ModelProvider>,
        kind: ModelProviderKind,
        model: &str,
    ) -> Self {
        let mut service = Self::unanswered();
        service.fixed = Some(Fixed {
            provider,
            kind,
            model: model.to_string(),
        });
        service
    }

    /// A service with no provider fixed and no key saved, reaching nothing:
    /// its settings decide whether a task can be asked, as in the app.
    #[cfg(test)]
    pub fn unanswered() -> Self {
        use super::claude_cli::tests::FakeProcesses;
        use crate::http::tests::CannedTransport;
        use crate::secrets::tests::MemorySecrets;

        Self::new(
            Arc::new(MemorySecrets::default()),
            Arc::new(CannedTransport::default()),
            ClaudeCode::new(
                ToolPath::fixed(""),
                None,
                std::env::temp_dir(),
                FakeProcesses::printing(0, ""),
            ),
        )
    }

    /// As `answered_by`, with `task` answered by `provider` instead, said to
    /// be `kind` asking `model`.
    #[cfg(test)]
    pub fn with_task(
        mut self,
        task: ModelTask,
        provider: Arc<dyn ModelProvider>,
        kind: ModelProviderKind,
        model: &str,
    ) -> Self {
        self.fixed_tasks.push((
            task,
            Fixed {
                provider,
                kind,
                model: model.to_string(),
            },
        ));
        self
    }

    #[cfg(test)]
    fn fixed_for(&self, task: Option<ModelTask>) -> Option<&Fixed> {
        task.and_then(|task| {
            self.fixed_tasks
                .iter()
                .find(|(fixed, _)| *fixed == task)
                .map(|(_, fixed)| fixed)
        })
        .or(self.fixed.as_ref())
    }

    /// Keys in the Keychain, HTTPS, and the real `claude`, run from
    /// `claude_directory`, an empty folder of DiffLens's own.
    pub fn standard(home: Option<PathBuf>, claude_directory: PathBuf) -> Self {
        Self::new(
            Arc::new(KeychainSecrets),
            Arc::new(ReqwestTransport::new()),
            ClaudeCode::new(
                ToolPath::from_login_shell(),
                home,
                claude_directory,
                Arc::new(SystemProcesses),
            ),
        )
    }

    pub fn settings(&self, store: &Store) -> AppResult<ModelSettings> {
        let providers: Vec<ModelProviderState> = ModelProviderKind::ALL
            .into_iter()
            .map(|kind| self.state(store, kind))
            .collect::<AppResult<_>>()?;
        let chosen = store.model_tasks()?;
        let mut tasks = Vec::new();
        for task in ModelTask::ALL {
            let choice = match chosen.iter().find(|record| record.task == task.key()) {
                Some(record) => Some(choice_of(record)?),
                None => None,
            };
            let needs_attention = choice.as_ref().is_some_and(|choice| {
                !providers
                    .iter()
                    .any(|state| state.provider == choice.provider && state.configured)
            });
            tasks.push(ModelTaskState {
                task,
                choice,
                needs_attention,
            });
        }
        Ok(ModelSettings {
            active_provider: active_provider(store)?,
            providers,
            tasks,
        })
    }

    /// Gives `task` a model of its own (`Some`), with the same checks as the
    /// default one's, or puts it back on the default (`None`). Only a
    /// configured provider can be chosen.
    pub fn set_task(
        &self,
        store: &Store,
        task: ModelTask,
        choice: Option<ModelTaskChoice>,
    ) -> AppResult<ModelSettings> {
        let Some(choice) = choice else {
            store.clear_model_task(task.key())?;
            return self.settings(store);
        };
        let kind = choice.provider;
        let model = choice.model.trim();
        if model.is_empty() {
            return Err(ModelError::NotConfigured(app_error!(
                ModelNotConfigured,
                TaskModelMissing,
                "Choose a model for {task} in Preferences › Model.",
                task = task_phrase(task)
            ))
            .into());
        }
        if !self.state(store, kind)?.configured {
            return Err(if kind.needs_key() {
                ModelError::NotConfigured(key_missing(kind)).into()
            } else {
                ModelError::CliMissing.into()
            });
        }
        let effort = takes_effort(kind)
            .then(|| choice.effort.unwrap_or(anthropic::DEFAULT_EFFORT))
            .map(effort_key);
        store.set_model_task(&ModelTaskRecord {
            task: task.key().to_string(),
            provider: kind.key().to_string(),
            model: model.to_string(),
            effort,
        })?;
        self.settings(store)
    }

    /// Makes `provider` the one reviews are read by.
    pub fn set_provider(&self, store: &Store, provider: &str) -> AppResult<ModelSettings> {
        let kind = kind_of(provider)?;
        store.set_setting(ACTIVE_PROVIDER, kind.key())?;
        self.settings(store)
    }

    /// Saves the model (blank for none) and, for the providers that take
    /// one, the effort chosen for `provider`.
    pub fn save_choice(
        &self,
        store: &Store,
        provider: &str,
        model: &str,
        effort: Option<Effort>,
    ) -> AppResult<ModelSettings> {
        let kind = kind_of(provider)?;
        let model = Some(model.trim()).filter(|model| !model.is_empty());
        let effort = takes_effort(kind)
            .then(|| effort.unwrap_or(anthropic::DEFAULT_EFFORT))
            .map(effort_key);
        store.save_model_choice(kind.key(), model, effort.as_deref())?;
        self.settings(store)
    }

    /// Checks `key` with the provider by listing its models, then keeps it in
    /// the Keychain. Nothing is saved when the provider refuses it.
    pub fn set_key(&self, store: &Store, provider: &str, key: &str) -> AppResult<ModelKeySaved> {
        let kind = kind_of(provider)?;
        if !kind.needs_key() {
            return Err(app_error!(
                Internal,
                ProviderNeedsNoKey,
                "{provider} needs no API key.",
                provider = kind.name()
            ));
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(ModelError::NotConfigured(app_error!(
                ModelNotConfigured,
                ApiKeyMissing,
                "Enter the API key."
            ))
            .into());
        }
        let models = self
            .http_provider(kind, Secret::new(key), "", None)
            .models()?;
        self.secrets.set(&secret_key(kind), key)?;
        store.set_model_key_saved(kind.key(), Some(&flow::now()?))?;
        Ok(ModelKeySaved {
            settings: self.settings(store)?,
            models,
        })
    }

    /// Deletes the provider's key from the Keychain.
    pub fn remove_key(&self, store: &Store, provider: &str) -> AppResult<ModelSettings> {
        let kind = kind_of(provider)?;
        self.secrets.delete(&secret_key(kind))?;
        store.set_model_key_saved(kind.key(), None)?;
        self.settings(store)
    }

    /// The models `provider` offers: Claude Code's aliases, or what the
    /// provider lists for the saved key.
    pub fn list_models(&self, provider: &str) -> AppResult<Vec<ModelInfo>> {
        let kind = kind_of(provider)?;
        if !kind.needs_key() {
            return Ok(claude_cli::aliases());
        }
        let key = self.saved_key(kind)?;
        Ok(self.http_provider(kind, key, "", None).models()?)
    }

    /// Asks the default model a tiny question, under `run_id`.
    pub fn test(&self, store: &Store, run_id: &str) -> AppResult<ModelTest> {
        let started = Instant::now();
        let provider = self.provider(store, None)?;
        let request = test_request();
        let answer = self
            .runs
            .run(run_id, move |cancel| provider.ask(&request, cancel))?;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let reply = answer.value["reply"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        Ok(ModelTest {
            provider: answer.provider,
            provider_name: answer.provider.name().to_string(),
            model: answer.model,
            reply,
            elapsed_ms,
            input_tokens: answer.usage.and_then(|usage| usage.input_tokens),
            output_tokens: answer.usage.and_then(|usage| usage.output_tokens),
        })
    }

    /// Asks the model of `task` `request`, under `run_id`, which `cancel`
    /// stops. The only way DiffLens asks a model.
    pub fn ask(
        &self,
        store: &Store,
        task: ModelTask,
        run_id: &str,
        request: ModelRequest,
    ) -> Result<ModelAnswer, ModelError> {
        let provider = self.provider(store, Some(task))?;
        self.runs
            .run(run_id, move |cancel| provider.ask(&request, cancel))
    }

    /// As `ask`, also stopped when `cancel` is raised: the flag of a piece of
    /// work of several calls (see `Calls`).
    pub fn ask_flagged(
        &self,
        store: &Store,
        task: ModelTask,
        run_id: &str,
        request: ModelRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<ModelAnswer, ModelError> {
        let provider = self.provider(store, Some(task))?;
        self.runs
            .run_flagged(run_id, cancel, move |cancel| provider.ask(&request, cancel))
    }

    pub fn cancel(&self, run_id: &str) {
        self.runs.cancel(run_id);
    }

    /// Who `ask` would ask now for `task`, and what it lacks when it cannot
    /// be asked. Asks no model; for Claude Code it runs `claude --version`.
    pub fn in_use(&self, store: &Store, task: ModelTask) -> AppResult<ModelInUse> {
        #[cfg(test)]
        if let Some(fixed) = self.fixed_for(Some(task)) {
            return Ok(ModelInUse {
                provider: fixed.kind,
                provider_name: fixed.kind.name().to_string(),
                model: Some(fixed.model.clone()),
                ready: true,
                problem: None,
                parallel_calls: fixed.kind.parallel_calls(),
            });
        }
        let Some(choice) = task_choice(store, task)? else {
            return self.default_in_use(store);
        };
        let kind = choice.provider;
        let ready = self.state(store, kind)?.configured;
        Ok(ModelInUse {
            provider: kind,
            provider_name: kind.name().to_string(),
            model: Some(choice.model),
            ready,
            problem: (!ready).then(|| task_provider_not_set_up(task, kind)),
            parallel_calls: kind.parallel_calls(),
        })
    }

    /// Fails, as asking would, when the model of one of `tasks` cannot be
    /// asked: a piece of work of several tasks starts only when each can.
    pub fn ensure_ready(&self, store: &Store, tasks: &[ModelTask]) -> AppResult<()> {
        for &task in tasks {
            let in_use = self.in_use(store, task)?;
            if !in_use.ready {
                let problem = in_use.problem.unwrap_or_else(|| {
                    app_error!(
                        ModelNotConfigured,
                        ModelNotSetUp,
                        "Set up a model in Preferences › Model."
                    )
                });
                return Err(ModelError::NotConfigured(problem).into());
            }
        }
        Ok(())
    }

    /// Who the default model is, and what it lacks when it cannot be asked.
    fn default_in_use(&self, store: &Store) -> AppResult<ModelInUse> {
        let kind = active_provider(store)?;
        let state = self.state(store, kind)?;
        let problem = if state.ready {
            None
        } else if state.model.is_none() {
            Some(model_missing(kind))
        } else if state.needs_key && !state.key_saved {
            Some(key_missing(kind))
        } else {
            Some(AppError::from(ModelError::CliMissing))
        };
        Ok(ModelInUse {
            provider: kind,
            provider_name: state.name,
            model: state.model,
            ready: state.ready,
            problem,
            parallel_calls: kind.parallel_calls(),
        })
    }

    fn state(&self, store: &Store, kind: ModelProviderKind) -> AppResult<ModelProviderState> {
        let record = store.model_provider(kind.key())?.unwrap_or_default();
        let key_saved = record.key_saved_at.is_some();
        let model = record
            .model
            .or_else(|| default_model(kind).map(str::to_string));
        let effort = takes_effort(kind).then(|| {
            record
                .effort
                .as_deref()
                .and_then(effort_from_key)
                .unwrap_or(anthropic::DEFAULT_EFFORT)
        });
        let cli = (kind == ModelProviderKind::ClaudeCli).then(|| self.claude.status());
        let configured =
            (!kind.needs_key() || key_saved) && cli.as_ref().is_none_or(|cli| cli.path.is_some());
        let ready = model.is_some() && configured;
        Ok(ModelProviderState {
            provider: kind,
            name: kind.name().to_string(),
            needs_key: kind.needs_key(),
            key_saved,
            key_url: kind.key_url().to_string(),
            model,
            effort,
            suggested_models: if kind.needs_key() {
                Vec::new()
            } else {
                claude_cli::aliases()
            },
            cli,
            ready,
            configured,
        })
    }

    /// Who answers for `task` (the default model for `None`), configured,
    /// or what it lacks.
    fn provider(
        &self,
        store: &Store,
        task: Option<ModelTask>,
    ) -> Result<Arc<dyn ModelProvider>, ModelError> {
        #[cfg(test)]
        if let Some(fixed) = self.fixed_for(task) {
            return Ok(Arc::clone(&fixed.provider));
        }
        let Some(task) = task else {
            return self.active(store);
        };
        let Some(choice) = task_choice(store, task)? else {
            return self.active(store);
        };
        let kind = choice.provider;
        let unavailable = || ModelError::NotConfigured(task_provider_not_set_up(task, kind));
        if !kind.needs_key() {
            return match self.claude.provider(&choice.model) {
                Ok(provider) => Ok(Arc::new(provider)),
                Err(ModelError::CliMissing) => Err(unavailable()),
                Err(error) => Err(error),
            };
        }
        let key = self
            .secrets
            .get(&secret_key(kind))?
            .map(Secret::new)
            .ok_or_else(unavailable)?;
        Ok(Arc::from(self.http_provider(
            kind,
            key,
            &choice.model,
            choice.effort,
        )))
    }

    /// The default model, configured, or what it lacks.
    fn active(&self, store: &Store) -> Result<Arc<dyn ModelProvider>, ModelError> {
        let kind = active_provider(store)?;
        let record = store.model_provider(kind.key())?.unwrap_or_default();
        let model = record
            .model
            .or_else(|| default_model(kind).map(str::to_string))
            .ok_or_else(|| ModelError::NotConfigured(model_missing(kind)))?;
        if !kind.needs_key() {
            return Ok(Arc::new(self.claude.provider(&model)?));
        }
        let key = self.saved_key(kind)?;
        let effort = record.effort.as_deref().and_then(effort_from_key);
        Ok(Arc::from(self.http_provider(kind, key, &model, effort)))
    }

    fn saved_key(&self, kind: ModelProviderKind) -> Result<Secret, ModelError> {
        self.secrets
            .get(&secret_key(kind))?
            .map(Secret::new)
            .ok_or_else(|| ModelError::NotConfigured(key_missing(kind)))
    }

    fn http_provider(
        &self,
        kind: ModelProviderKind,
        key: Secret,
        model: &str,
        effort: Option<Effort>,
    ) -> Box<dyn ModelProvider> {
        let transport = Arc::clone(&self.transport);
        match kind {
            ModelProviderKind::Anthropic => Box::new(Anthropic::new(
                anthropic::API,
                key,
                model,
                effort.unwrap_or(anthropic::DEFAULT_EFFORT),
                transport,
            )),
            ModelProviderKind::OpenAi => Box::new(OpenAi::new(openai::API, key, model, transport)),
            ModelProviderKind::Gemini => Box::new(Gemini::new(gemini::API, key, model, transport)),
            ModelProviderKind::OpenRouter => {
                Box::new(OpenRouter::new(openrouter::API, key, model, transport))
            }
            ModelProviderKind::ClaudeCli => {
                unreachable!("Claude Code is not reached over HTTP")
            }
        }
    }
}

fn active_provider(store: &Store) -> AppResult<ModelProviderKind> {
    Ok(store
        .setting(ACTIVE_PROVIDER)?
        .as_deref()
        .and_then(ModelProviderKind::from_key)
        .unwrap_or(DEFAULT_PROVIDER))
}

fn kind_of(provider: &str) -> AppResult<ModelProviderKind> {
    ModelProviderKind::from_key(provider).ok_or_else(|| {
        app_error!(
            Internal,
            UnknownModelProvider,
            "DiffLens does not know the model provider {provider}.",
            provider = provider
        )
    })
}

/// No model is chosen for `kind`, and it has no default.
fn model_missing(kind: ModelProviderKind) -> AppError {
    app_error!(
        ModelNotConfigured,
        ModelMissing,
        "Choose a model for {provider} in Preferences › Model.",
        provider = provider_phrase(kind, false)
    )
}

/// `kind` needs an API key and none is saved.
fn key_missing(kind: ModelProviderKind) -> AppError {
    app_error!(
        ModelNotConfigured,
        ApiKeyNotSaved,
        "Save an API key for {provider} in Preferences › Model.",
        provider = provider_phrase(kind, false)
    )
}

/// The provider chosen for `task` is not configured any more: its calls fail
/// with this until it is set up again or another model is chosen.
fn task_provider_not_set_up(task: ModelTask, kind: ModelProviderKind) -> AppError {
    app_error!(
        ModelNotConfigured,
        TaskProviderNotSetUp,
        "{provider} was chosen for {task}, and it is not set up any more. Set it up again, or \
         choose another model for {task}, in Preferences › Model.",
        provider = provider_phrase(kind, true),
        task = task_phrase(task)
    )
}

/// How a sentence names `task`.
fn task_phrase(task: ModelTask) -> CoreText {
    match task {
        ModelTask::Insights => core_text!(TaskInsights, "Insights"),
        ModelTask::Summary => core_text!(TaskSummary, "the summary"),
        ModelTask::BriefExplanation => core_text!(TaskBriefExplanation, "short explanations"),
        ModelTask::DetailedExplanation => {
            core_text!(TaskDetailedExplanation, "detailed explanations")
        }
        ModelTask::TestDiagnosis => core_text!(TaskTestDiagnosis, "test diagnoses"),
        ModelTask::ReviewComment => core_text!(TaskReviewComment, "review comments"),
        ModelTask::Translation => core_text!(TaskTranslation, "translations"),
    }
}

/// The model chosen for `task`; `None` when it uses the default.
fn task_choice(store: &Store, task: ModelTask) -> AppResult<Option<ModelTaskChoice>> {
    store
        .model_task(task.key())?
        .map(|record| choice_of(&record))
        .transpose()
}

fn choice_of(record: &ModelTaskRecord) -> AppResult<ModelTaskChoice> {
    let kind = kind_of(&record.provider)?;
    let effort = takes_effort(kind).then(|| {
        record
            .effort
            .as_deref()
            .and_then(effort_from_key)
            .unwrap_or(anthropic::DEFAULT_EFFORT)
    });
    Ok(ModelTaskChoice {
        provider: kind,
        model: record.model.clone(),
        effort,
    })
}

/// The Keychain entry of a provider's key, such as "model:anthropic".
fn secret_key(kind: ModelProviderKind) -> String {
    format!("model:{}", kind.key())
}

fn default_model(kind: ModelProviderKind) -> Option<&'static str> {
    match kind {
        ModelProviderKind::ClaudeCli => Some(claude_cli::DEFAULT_MODEL),
        ModelProviderKind::Anthropic => Some(anthropic::DEFAULT_MODEL),
        _ => None,
    }
}

fn takes_effort(kind: ModelProviderKind) -> bool {
    kind == ModelProviderKind::Anthropic
}

fn effort_key(effort: Effort) -> String {
    serde_json::to_value(effort)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn effort_from_key(key: &str) -> Option<Effort> {
    serde_json::from_value(Value::String(key.to_string())).ok()
}

/// The question Test asks: tiny, with a tiny schema, about nothing the user
/// is reviewing.
fn test_request() -> ModelRequest {
    ModelRequest {
        system:
            "You are being asked by DiffLens, a code review app, to check that it can reach you. \
                 Answer only with the JSON the schema asks for."
                .to_string(),
        prompt: "Set ok to true, and in reply write one short, friendly sentence to the person \
                 reviewing code with DiffLens."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "reply": { "type": "string" }
            },
            "required": ["ok", "reply"],
            "additionalProperties": false
        }),
        schema_name: "connection_check".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::ai::claude_cli::tests::{success, FakeProcesses};
    use crate::ai::claude_cli::ProcessRunner;
    use crate::error::ErrorCode;
    use crate::http::tests::CannedTransport;
    use crate::http::{Auth, Method};
    use crate::secrets::tests::MemorySecrets;

    struct Fixture {
        store: Store,
        secrets: Arc<MemorySecrets>,
        transport: Arc<CannedTransport>,
        processes: Arc<FakeProcesses>,
        service: ModelService,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        /// A machine where `claude` is on the PATH and answers with
        /// `processes`.
        fn new(processes: Arc<FakeProcesses>) -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let program = dir.path().join("claude");
                std::fs::write(&program, "#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            Self::on_path(processes, dir, |dir| dir.as_os_str().to_os_string())
        }

        /// A machine without `claude` anywhere DiffLens looks.
        fn without_claude() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            Self::on_path(FakeProcesses::printing(0, ""), dir, |_| OsString::new())
        }

        fn on_path(
            processes: Arc<FakeProcesses>,
            dir: tempfile::TempDir,
            path: impl Fn(&std::path::Path) -> OsString,
        ) -> Self {
            let secrets = Arc::new(MemorySecrets::default());
            let transport = Arc::new(CannedTransport::default());
            let claude = ClaudeCode::new(
                ToolPath::fixed(path(dir.path())),
                Some(dir.path().join("home")),
                dir.path().join("work"),
                Arc::clone(&processes) as Arc<dyn ProcessRunner>,
            );
            let service = ModelService::new(
                Arc::clone(&secrets) as Arc<dyn SecretStore>,
                Arc::clone(&transport) as Arc<dyn Transport>,
                claude,
            );
            Self {
                store: Store::open_in_memory().unwrap(),
                secrets,
                transport,
                processes,
                service,
                _dir: dir,
            }
        }

        fn state(&self, kind: ModelProviderKind) -> ModelProviderState {
            self.service
                .settings(&self.store)
                .unwrap()
                .providers
                .into_iter()
                .find(|state| state.provider == kind)
                .unwrap()
        }
    }

    fn anthropic_models() -> Value {
        json!({
            "data": [{ "type": "model", "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5" }],
            "has_more": false,
            "first_id": "claude-opus-5-5",
            "last_id": "claude-opus-5-5"
        })
    }

    #[test]
    fn starts_on_claude_code_with_every_provider_listed_and_asks_nothing() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328 (Claude Code)\n"));
        let settings = fixture.service.settings(&fixture.store).unwrap();
        assert_eq!(settings.active_provider, ModelProviderKind::ClaudeCli);
        let kinds: Vec<_> = settings
            .providers
            .iter()
            .map(|state| state.provider)
            .collect();
        assert_eq!(kinds, ModelProviderKind::ALL);

        let cli = &settings.providers[0];
        assert!(cli.ready && !cli.needs_key);
        assert_eq!(cli.model.as_deref(), Some("opus"));
        assert_eq!(
            cli.cli.as_ref().unwrap().version.as_deref(),
            Some("2.1.328 (Claude Code)")
        );
        assert!(!cli.suggested_models.is_empty());

        let anthropic = &settings.providers[1];
        assert_eq!(anthropic.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(anthropic.effort, Some(Effort::High));
        assert!(!anthropic.ready && !anthropic.key_saved);
        let openai = &settings.providers[2];
        assert_eq!((openai.model.as_deref(), openai.effort), (None, None));

        // Reading the settings only asked `claude --version`.
        assert!(fixture.transport.sent().is_empty());
        let invocations = fixture.processes.invocations.lock().unwrap();
        assert!(invocations
            .iter()
            .all(|invocation| invocation.args == ["--version"]));
    }

    #[test]
    fn without_claude_code_its_state_says_so_and_a_test_fails_clearly() {
        let fixture = Fixture::without_claude();
        let cli = fixture.state(ModelProviderKind::ClaudeCli);
        let found_elsewhere = cli.cli.as_ref().unwrap().path.is_some();
        if found_elsewhere {
            // This machine has claude in a standard location the test
            // cannot hide; the rest of the test does not apply.
            return;
        }
        assert!(!cli.ready);
        let error = fixture.service.test(&fixture.store, "run-1").unwrap_err();
        assert_eq!(error.code, ErrorCode::ClaudeCliNotFound);
    }

    #[test]
    fn a_key_is_checked_by_listing_models_then_kept_only_in_the_keychain() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        fixture.transport.push(200, anthropic_models());

        let saved = fixture
            .service
            .set_key(&fixture.store, "anthropic", "  sk-ant-secret  ")
            .unwrap();
        assert_eq!(saved.models[0].id, "claude-opus-5-5");
        let anthropic = saved
            .settings
            .providers
            .iter()
            .find(|state| state.provider == ModelProviderKind::Anthropic)
            .unwrap();
        assert!(anthropic.key_saved && anthropic.ready);
        assert_eq!(
            fixture
                .secrets
                .secrets
                .lock()
                .unwrap()
                .get("model:anthropic")
                .map(String::as_str),
            Some("sk-ant-secret")
        );
        // Only the models list was asked for: saving a key asks no model.
        let sent = fixture.transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, Method::Get);
        assert!(sent[0]
            .url
            .starts_with("https://api.anthropic.com/v1/models"));
        // The database knows a key was saved, not the key.
        let record = fixture.store.model_provider("anthropic").unwrap().unwrap();
        assert!(record.key_saved_at.is_some());
        assert!(!format!("{record:?}").contains("sk-ant-secret"));
        assert!(!format!("{saved:?}").contains("sk-ant-secret"));

        fixture
            .service
            .remove_key(&fixture.store, "anthropic")
            .unwrap();
        assert!(fixture.secrets.secrets.lock().unwrap().is_empty());
        assert!(!fixture.state(ModelProviderKind::Anthropic).key_saved);
    }

    #[test]
    fn a_refused_key_is_not_saved() {
        let fixture = Fixture::new(FakeProcesses::printing(0, ""));
        fixture.transport.push(
            401,
            json!({"error": {"message": "Incorrect API key provided: sk-bad", "type": "invalid_request_error"}}),
        );
        let error = fixture
            .service
            .set_key(&fixture.store, "openai", "sk-bad")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::AuthFailed);
        assert!(!error.message.contains("sk-bad"), "{}", error.message);
        assert!(fixture.secrets.secrets.lock().unwrap().is_empty());
        assert!(!fixture.state(ModelProviderKind::OpenAi).key_saved);

        let blank = fixture
            .service
            .set_key(&fixture.store, "openai", "  ")
            .unwrap_err();
        assert_eq!(blank.code, ErrorCode::ModelNotConfigured);
    }

    #[test]
    fn tests_the_active_provider_with_its_saved_model_effort_and_key() {
        let fixture = Fixture::new(FakeProcesses::printing(0, ""));
        fixture.transport.push(200, anthropic_models());
        fixture
            .service
            .set_key(&fixture.store, "anthropic", "sk-ant-secret")
            .unwrap();
        fixture
            .service
            .save_choice(
                &fixture.store,
                "anthropic",
                " claude-sonnet-5-5 ",
                Some(Effort::Low),
            )
            .unwrap();
        let settings = fixture
            .service
            .set_provider(&fixture.store, "anthropic")
            .unwrap();
        assert_eq!(settings.active_provider, ModelProviderKind::Anthropic);

        fixture.transport.push(
            200,
            json!({
                "model": "claude-sonnet-5-5",
                "content": [{ "type": "text", "text": "{\"ok\":true,\"reply\":\"Happy reviewing!\"}" }],
                "stop_reason": "end_turn",
                "stop_details": null,
                "usage": { "input_tokens": 60, "output_tokens": 15 }
            }),
        );
        let test = fixture.service.test(&fixture.store, "run-1").unwrap();
        assert_eq!(test.reply, "Happy reviewing!");
        assert_eq!(test.model, "claude-sonnet-5-5");
        assert_eq!(test.provider_name, "Anthropic API");
        assert_eq!(
            (test.input_tokens, test.output_tokens),
            (Some(60), Some(15))
        );

        let sent = fixture.transport.sent();
        let asked = sent.last().unwrap();
        assert_eq!(asked.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(
            asked.auth,
            Auth::Header {
                name: "x-api-key",
                value: Secret::new("sk-ant-secret")
            }
        );
        let body = asked.body.as_ref().unwrap();
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn an_api_provider_without_a_key_or_model_is_not_configured() {
        let fixture = Fixture::new(FakeProcesses::printing(0, ""));
        fixture
            .service
            .set_provider(&fixture.store, "gemini")
            .unwrap();
        let error = fixture.service.test(&fixture.store, "run-1").unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelNotConfigured);
        assert!(
            error.message.contains("Choose a model"),
            "{}",
            error.message
        );

        fixture
            .service
            .save_choice(&fixture.store, "gemini", "gemini-3-pro", Some(Effort::Max))
            .unwrap();
        // Gemini takes no effort; none is kept.
        assert_eq!(fixture.state(ModelProviderKind::Gemini).effort, None);
        let error = fixture.service.test(&fixture.store, "run-1").unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelNotConfigured);
        assert!(
            error.message.contains("Save an API key"),
            "{}",
            error.message
        );
        assert!(fixture.transport.sent().is_empty());

        assert_eq!(
            fixture.service.list_models("gemini").unwrap_err().code,
            ErrorCode::ModelNotConfigured
        );
    }

    #[test]
    fn tests_claude_code_and_lists_its_aliases_without_running_it() {
        let fixture = Fixture::new(FakeProcesses::printing(
            0,
            &success(json!({"ok": true, "reply": "Hello there."})).to_string(),
        ));
        let models = fixture.service.list_models("claude_cli").unwrap();
        assert!(models.iter().any(|model| model.id == "opus"));
        assert!(fixture.processes.invocations.lock().unwrap().is_empty());

        let test = fixture.service.test(&fixture.store, "run-1").unwrap();
        assert_eq!(test.reply, "Hello there.");
        assert_eq!(test.model, "claude-opus-5-5");
        assert_eq!(test.provider, ModelProviderKind::ClaudeCli);
        let invocations = fixture.processes.invocations.lock().unwrap();
        assert!(invocations[0].args.contains(&"--model=opus".to_string()));
    }

    fn anthropic_answer(model: &str) -> Value {
        json!({
            "model": model,
            "content": [{ "type": "text", "text": "{\"ok\":true,\"reply\":\"Hi.\"}" }],
            "stop_reason": "end_turn",
            "stop_details": null,
            "usage": { "input_tokens": 60, "output_tokens": 15 }
        })
    }

    fn choice(provider: ModelProviderKind, model: &str, effort: Option<Effort>) -> ModelTaskChoice {
        ModelTaskChoice {
            provider,
            model: model.to_string(),
            effort,
        }
    }

    #[test]
    fn every_task_starts_on_the_default_model() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        let settings = fixture.service.settings(&fixture.store).unwrap();
        let tasks: Vec<_> = settings.tasks.iter().map(|state| state.task).collect();
        assert_eq!(tasks, ModelTask::ALL);
        assert!(settings
            .tasks
            .iter()
            .all(|state| state.choice.is_none() && !state.needs_attention));
        for task in ModelTask::ALL {
            let in_use = fixture.service.in_use(&fixture.store, task).unwrap();
            assert_eq!(
                (in_use.provider, in_use.model.as_deref(), in_use.ready),
                (ModelProviderKind::ClaudeCli, Some("opus"), true),
                "{task:?}"
            );
        }
        // The default changes every task on it.
        fixture
            .service
            .set_provider(&fixture.store, "anthropic")
            .unwrap();
        let summary = fixture
            .service
            .in_use(&fixture.store, ModelTask::Summary)
            .unwrap();
        assert_eq!(summary.provider, ModelProviderKind::Anthropic);
        assert!(!summary.ready);
        assert_eq!(
            summary.problem.unwrap().reason,
            Some(crate::error::Reason::ApiKeyNotSaved)
        );
    }

    #[test]
    fn test_diagnosis_is_a_task_with_its_own_model_and_the_same_attention_rule() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        fixture.transport.push(200, anthropic_models());
        fixture
            .service
            .set_key(&fixture.store, "anthropic", "sk-ant-secret")
            .unwrap();
        let settings = fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::TestDiagnosis,
                Some(choice(ModelProviderKind::Anthropic, "claude-sonnet-5-5", None)),
            )
            .unwrap();
        let state = |settings: &ModelSettings| {
            settings
                .tasks
                .iter()
                .find(|state| state.task == ModelTask::TestDiagnosis)
                .cloned()
                .unwrap()
        };
        assert_eq!(
            state(&settings).choice.map(|choice| choice.model),
            Some("claude-sonnet-5-5".to_string())
        );
        let in_use = fixture
            .service
            .in_use(&fixture.store, ModelTask::TestDiagnosis)
            .unwrap();
        assert_eq!(
            (in_use.provider, in_use.model.as_deref(), in_use.ready),
            (ModelProviderKind::Anthropic, Some("claude-sonnet-5-5"), true)
        );
        // The other tasks stay on the default.
        assert_eq!(
            fixture
                .service
                .in_use(&fixture.store, ModelTask::DetailedExplanation)
                .unwrap()
                .provider,
            ModelProviderKind::ClaudeCli
        );

        // Its provider set up no more: it needs attention and never falls
        // back to the default.
        let settings = fixture
            .service
            .remove_key(&fixture.store, "anthropic")
            .unwrap();
        assert!(state(&settings).needs_attention);
        let in_use = fixture
            .service
            .in_use(&fixture.store, ModelTask::TestDiagnosis)
            .unwrap();
        assert!(!in_use.ready);
        assert_eq!(
            in_use.problem.unwrap().message,
            "The Anthropic API was chosen for test diagnoses, and it is not set up any more. Set \
             it up again, or choose another model for test diagnoses, in Preferences › Model."
        );
    }

    #[test]
    fn a_task_asks_its_own_model_and_the_others_the_default() {
        let fixture = Fixture::new(FakeProcesses::printing(
            0,
            &success(json!({"ok": true, "reply": "Hello there."})).to_string(),
        ));
        fixture.transport.push(200, anthropic_models());
        fixture
            .service
            .set_key(&fixture.store, "anthropic", "sk-ant-secret")
            .unwrap();
        let settings = fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Insights,
                Some(choice(
                    ModelProviderKind::Anthropic,
                    " claude-sonnet-5-5 ",
                    Some(Effort::Low),
                )),
            )
            .unwrap();
        assert_eq!(
            settings.tasks[0],
            ModelTaskState {
                task: ModelTask::Insights,
                choice: Some(choice(
                    ModelProviderKind::Anthropic,
                    "claude-sonnet-5-5",
                    Some(Effort::Low)
                )),
                needs_attention: false,
            }
        );
        assert!(settings.tasks[1..]
            .iter()
            .all(|state| state.choice.is_none()));
        // The default stays as it was.
        assert_eq!(settings.active_provider, ModelProviderKind::ClaudeCli);

        let insights = fixture
            .service
            .in_use(&fixture.store, ModelTask::Insights)
            .unwrap();
        assert_eq!(
            (insights.provider, insights.model.as_deref(), insights.ready),
            (
                ModelProviderKind::Anthropic,
                Some("claude-sonnet-5-5"),
                true
            )
        );
        assert_eq!(insights.parallel_calls, 3);
        let summary = fixture
            .service
            .in_use(&fixture.store, ModelTask::Summary)
            .unwrap();
        assert_eq!(
            (summary.provider, summary.model.as_deref()),
            (ModelProviderKind::ClaudeCli, Some("opus"))
        );

        fixture
            .transport
            .push(200, anthropic_answer("claude-sonnet-5-5"));
        let answer = fixture
            .service
            .ask(&fixture.store, ModelTask::Insights, "run-1", test_request())
            .unwrap();
        assert_eq!(answer.model, "claude-sonnet-5-5");
        let sent = fixture.transport.sent();
        let body = sent.last().unwrap().body.clone().unwrap();
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["output_config"]["effort"], "low");

        let answer = fixture
            .service
            .ask(&fixture.store, ModelTask::Summary, "run-2", test_request())
            .unwrap();
        assert_eq!(answer.provider, ModelProviderKind::ClaudeCli);
        assert!(fixture
            .processes
            .invocations
            .lock()
            .unwrap()
            .iter()
            .any(|invocation| invocation.args.contains(&"--model=opus".to_string())));
        // Only the Insights call went over HTTP, after the models list.
        assert_eq!(fixture.transport.sent().len(), 2);

        // Back on the default.
        let settings = fixture
            .service
            .set_task(&fixture.store, ModelTask::Insights, None)
            .unwrap();
        assert!(settings.tasks[0].choice.is_none());
        assert_eq!(
            fixture
                .service
                .in_use(&fixture.store, ModelTask::Insights)
                .unwrap()
                .provider,
            ModelProviderKind::ClaudeCli
        );
    }

    #[test]
    fn a_task_whose_provider_is_no_longer_set_up_needs_attention_and_fails_naming_it() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        fixture.transport.push(200, anthropic_models());
        fixture
            .service
            .set_key(&fixture.store, "anthropic", "sk-ant-secret")
            .unwrap();
        fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Translation,
                Some(choice(ModelProviderKind::Anthropic, "claude-haiku-5", None)),
            )
            .unwrap();
        let settings = fixture
            .service
            .remove_key(&fixture.store, "anthropic")
            .unwrap();
        let translation = settings
            .tasks
            .iter()
            .find(|state| state.task == ModelTask::Translation)
            .unwrap();
        // Still chosen, not put back on the default.
        assert!(translation.needs_attention);
        assert_eq!(
            translation.choice.as_ref().map(|choice| choice.provider),
            Some(ModelProviderKind::Anthropic)
        );
        assert!(settings
            .tasks
            .iter()
            .filter(|state| state.task != ModelTask::Translation)
            .all(|state| !state.needs_attention));

        let in_use = fixture
            .service
            .in_use(&fixture.store, ModelTask::Translation)
            .unwrap();
        assert_eq!(in_use.provider, ModelProviderKind::Anthropic);
        assert!(!in_use.ready);
        let problem = in_use.problem.unwrap();
        assert_eq!(problem.code, ErrorCode::ModelNotConfigured);
        assert_eq!(
            problem.reason,
            Some(crate::error::Reason::TaskProviderNotSetUp)
        );
        assert_eq!(
            problem.message,
            "The Anthropic API was chosen for translations, and it is not set up any more. Set it \
             up again, or choose another model for translations, in Preferences › Model."
        );
        let error = fixture
            .service
            .ask(
                &fixture.store,
                ModelTask::Translation,
                "run-1",
                test_request(),
            )
            .unwrap_err();
        assert_eq!(AppError::from(error), problem);
        // The default answers the other tasks as before.
        assert!(
            fixture
                .service
                .in_use(&fixture.store, ModelTask::Summary)
                .unwrap()
                .ready
        );
        assert_eq!(fixture.transport.sent().len(), 1, "only the models list");
    }

    #[test]
    fn work_of_several_tasks_is_ready_only_when_each_of_its_models_is() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        let tasks = [ModelTask::Insights, ModelTask::Summary];
        fixture
            .service
            .ensure_ready(&fixture.store, &tasks)
            .unwrap();

        fixture
            .store
            .set_model_key_saved("openai", Some("2026-11-19T10:00:00Z"))
            .unwrap();
        fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Summary,
                Some(choice(ModelProviderKind::OpenAi, "gpt-5.5", None)),
            )
            .unwrap();
        fixture
            .service
            .ensure_ready(&fixture.store, &tasks)
            .unwrap();
        fixture.store.set_model_key_saved("openai", None).unwrap();
        let error = fixture
            .service
            .ensure_ready(&fixture.store, &tasks)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelNotConfigured);
        assert_eq!(
            error.reason,
            Some(crate::error::Reason::TaskProviderNotSetUp)
        );
        assert!(
            error.message.contains("for the summary"),
            "{}",
            error.message
        );
        // Work that does not ask for the summary is not held back by it.
        fixture
            .service
            .ensure_ready(&fixture.store, &[ModelTask::Insights])
            .unwrap();
    }

    #[test]
    fn only_a_configured_provider_with_a_model_can_be_chosen_for_a_task() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        let keyless = fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Summary,
                Some(choice(ModelProviderKind::OpenAi, "gpt-5.5", None)),
            )
            .unwrap_err();
        assert_eq!(keyless.code, ErrorCode::ModelNotConfigured);
        assert_eq!(
            keyless.message,
            "Save an API key for the OpenAI API in Preferences › Model."
        );
        let blank = fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Summary,
                Some(choice(ModelProviderKind::ClaudeCli, "  ", None)),
            )
            .unwrap_err();
        assert_eq!(blank.reason, Some(crate::error::Reason::TaskModelMissing));
        assert_eq!(
            blank.message,
            "Choose a model for the summary in Preferences › Model."
        );
        assert!(fixture.store.model_tasks().unwrap().is_empty());

        // Only the providers that take an effort keep one; the Anthropic API
        // gets the default one when none is given.
        fixture
            .store
            .set_model_key_saved("gemini", Some("2026-11-19T10:00:00Z"))
            .unwrap();
        fixture
            .store
            .set_model_key_saved("anthropic", Some("2026-11-19T10:00:00Z"))
            .unwrap();
        fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::Summary,
                Some(choice(
                    ModelProviderKind::Gemini,
                    "gemini-3-pro",
                    Some(Effort::Max),
                )),
            )
            .unwrap();
        let settings = fixture
            .service
            .set_task(
                &fixture.store,
                ModelTask::DetailedExplanation,
                Some(choice(
                    ModelProviderKind::Anthropic,
                    "claude-opus-5-5",
                    None,
                )),
            )
            .unwrap();
        assert_eq!(
            settings.tasks[1].choice,
            Some(choice(ModelProviderKind::Gemini, "gemini-3-pro", None))
        );
        assert_eq!(
            settings.tasks[3].choice,
            Some(choice(
                ModelProviderKind::Anthropic,
                "claude-opus-5-5",
                Some(Effort::High)
            ))
        );
        let configured: Vec<_> = settings
            .providers
            .iter()
            .filter(|state| state.configured)
            .map(|state| state.provider)
            .collect();
        assert_eq!(
            configured,
            [
                ModelProviderKind::ClaudeCli,
                ModelProviderKind::Anthropic,
                ModelProviderKind::Gemini
            ]
        );
        assert!(fixture.transport.sent().is_empty());
    }

    #[test]
    fn cancelling_a_claude_code_test_stops_it() {
        let fixture = Arc::new(Fixture::new(FakeProcesses::slow()));
        let canceller = {
            let fixture = Arc::clone(&fixture);
            std::thread::spawn(move || {
                while fixture.processes.invocations.lock().unwrap().is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                fixture.service.cancel("run-1");
            })
        };
        let error = fixture.service.test(&fixture.store, "run-1").unwrap_err();
        canceller.join().unwrap();
        assert_eq!(error.code, ErrorCode::Cancelled);
    }

    #[test]
    fn says_who_would_be_asked_and_what_it_lacks_without_asking() {
        let fixture = Fixture::new(FakeProcesses::printing(0, "2.1.328\n"));
        let cli = fixture
            .service
            .in_use(&fixture.store, ModelTask::Insights)
            .unwrap();
        assert_eq!(cli.provider, ModelProviderKind::ClaudeCli);
        assert_eq!(cli.model.as_deref(), Some("opus"));
        assert!(cli.ready && cli.problem.is_none());
        assert_eq!(cli.parallel_calls, 1);

        fixture
            .service
            .set_provider(&fixture.store, "openai")
            .unwrap();
        let openai = fixture
            .service
            .in_use(&fixture.store, ModelTask::Insights)
            .unwrap();
        assert!(!openai.ready);
        let problem = openai.problem.unwrap();
        assert_eq!(
            problem.message,
            "Choose a model for the OpenAI API in Preferences › Model."
        );
        assert_eq!(problem.code, ErrorCode::ModelNotConfigured);
        assert_eq!(problem.reason, Some(crate::error::Reason::ModelMissing));
        fixture
            .service
            .save_choice(&fixture.store, "openai", "gpt-5.5", None)
            .unwrap();
        let keyless = fixture
            .service
            .in_use(&fixture.store, ModelTask::Insights)
            .unwrap();
        assert_eq!(
            keyless.problem.map(|problem| problem.message).as_deref(),
            Some("Save an API key for the OpenAI API in Preferences › Model.")
        );
        assert_eq!(keyless.parallel_calls, 3);
        assert!(fixture.transport.sent().is_empty());
    }

    #[test]
    fn an_unknown_provider_is_refused() {
        let fixture = Fixture::new(FakeProcesses::printing(0, ""));
        assert!(fixture
            .service
            .set_provider(&fixture.store, "acme")
            .is_err());
        assert_eq!(
            fixture
                .service
                .settings(&fixture.store)
                .unwrap()
                .active_provider,
            ModelProviderKind::ClaudeCli
        );
    }
}
