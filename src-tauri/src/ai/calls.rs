//! The model calls of one piece of work (a full analysis, the explanations
//! of a group): each gets a run id of its own, and one cancel stops every
//! call in flight and every call not asked yet, as a shared flag.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::provider::{ModelAnswer, ModelError, ModelRequest};
use super::service::ModelService;
use crate::model::ModelTask;
use crate::store::Store;

pub struct Calls {
    cancel: Arc<AtomicBool>,
    prefix: String,
    next: AtomicU64,
}

impl Calls {
    /// Run ids are `prefix` and a counter, so they never meet the ids the
    /// frontend chooses.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            prefix: prefix.into(),
            next: AtomicU64::new(0),
        }
    }

    /// Asks `request` of the model of `task`, unless the work was cancelled.
    /// One piece of work can ask several tasks, each of its own model.
    pub fn ask(
        &self,
        models: &ModelService,
        store: &Store,
        task: ModelTask,
        request: ModelRequest,
    ) -> Result<ModelAnswer, ModelError> {
        if self.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        let number = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let run_id = format!("{}-{number}", self.prefix);
        models.ask_flagged(store, task, &run_id, request, Arc::clone(&self.cancel))
    }

    /// Stops the calls in flight and refuses the next ones.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::ModelProvider;
    use crate::model::ModelProviderKind;
    use crate::test_support::ScriptedModel;

    fn request(schema_name: &str) -> ModelRequest {
        ModelRequest {
            system: String::new(),
            prompt: String::new(),
            schema: serde_json::json!({}),
            schema_name: schema_name.to_string(),
        }
    }

    #[test]
    fn one_piece_of_work_asks_each_task_of_its_own_model_and_one_cancel_stops_them_all() {
        let answer = |_: &ModelRequest| Some(Ok(serde_json::json!({})));
        let insights = ScriptedModel::writing_as(ModelProviderKind::OpenAi, "gpt-5.5", answer);
        let summary = ScriptedModel::writing_as(ModelProviderKind::Gemini, "gemini-3-pro", answer);
        let briefs = ScriptedModel::new(answer);
        let models = briefs
            .service()
            .with_task(
                ModelTask::Insights,
                Arc::clone(&insights) as Arc<dyn ModelProvider>,
                ModelProviderKind::OpenAi,
                "gpt-5.5",
            )
            .with_task(
                ModelTask::Summary,
                Arc::clone(&summary) as Arc<dyn ModelProvider>,
                ModelProviderKind::Gemini,
                "gemini-3-pro",
            );
        let store = Store::open_in_memory().unwrap();
        // As a full analysis: its steps share the job's calls.
        let calls = Calls::new("full-1");
        let ask = |task| calls.ask(&models, &store, task, request("x")).unwrap();
        assert_eq!(ask(ModelTask::Insights).model, "gpt-5.5");
        assert_eq!(ask(ModelTask::Insights).model, "gpt-5.5");
        assert_eq!(ask(ModelTask::Summary).model, "gemini-3-pro");
        assert_eq!(
            ask(ModelTask::BriefExplanation).model,
            "claude-opus-5-5-20261101"
        );
        let asked = || {
            (
                insights.asked().len(),
                summary.asked().len(),
                briefs.asked().len(),
            )
        };
        assert_eq!(asked(), (2, 1, 1));

        calls.cancel();
        for task in [
            ModelTask::Insights,
            ModelTask::Summary,
            ModelTask::BriefExplanation,
        ] {
            assert_eq!(
                calls.ask(&models, &store, task, request("x")).unwrap_err(),
                ModelError::Cancelled
            );
        }
        assert_eq!(asked(), (2, 1, 1));
    }

    #[test]
    fn a_cancelled_piece_of_work_asks_nothing_more() {
        let calls = Calls::new("work");
        assert!(!calls.is_cancelled());
        calls.cancel();
        assert!(calls.is_cancelled());
        let store = Store::open_in_memory().unwrap();
        let models = ModelService::answered_by(
            Arc::new(crate::test_support::NeverAsked),
            crate::model::ModelProviderKind::Anthropic,
            "m",
        );
        let request = ModelRequest {
            system: String::new(),
            prompt: String::new(),
            schema: serde_json::json!({}),
            schema_name: "x".to_string(),
        };
        assert_eq!(
            calls
                .ask(&models, &store, ModelTask::Insights, request)
                .unwrap_err(),
            ModelError::Cancelled
        );
    }
}
