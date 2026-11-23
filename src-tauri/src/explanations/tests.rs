use std::sync::Arc;

use serde_json::{json, Value};

use super::inputs::tests::{edge, graph, node};
use super::*;
use crate::ai::ModelProvider;
use crate::ai::{Calls, ModelError, ModelRequest};
use crate::error::ErrorCode;
use crate::language::Language;
use crate::model::{
    ChangeSet, ExplanationKind, ExplanationLevel, FlowGraph, FlowNodeState, ModelProviderKind,
    ModelTask, ReviewRef, ReviewTarget,
};
use crate::projects::ProjectService;
use crate::review_state::ReviewSession;
use crate::store::Store;
use crate::test_support::{isolated_git, ScriptedModel, TestRepo};

const BEFORE: &str = "package refund\n\
    \n\
    func Request(id string) error {\n\
    \treturn save(id)\n\
    }\n";

/// Request now spans lines 3 to 7.
const AFTER: &str = "package refund\n\
    \n\
    func Request(id string) error {\n\
    \tpay(id)\n\
    \treturn save(id)\n\
    \n\
    }\n";

const HANDLER: &str = "package api\n\
    \n\
    func Handle() {\n\
    \trefund.Request(\"1\")\n\
    }\n";

struct Fixture {
    repo: TestRepo,
    projects: ProjectService,
    id: String,
    model: Arc<ScriptedModel>,
    explanations: Arc<ExplanationService>,
}

/// Explains every function it is asked about, in short or in detail, or
/// translates them into Portuguese.
fn answering() -> Arc<ScriptedModel> {
    answering_as(ModelProviderKind::Anthropic, "claude-opus-5-5-20261101")
}

/// As `answering`, its answers said to be written by `model` of `provider`.
fn answering_as(provider: ModelProviderKind, model: &str) -> Arc<ScriptedModel> {
    ScriptedModel::writing_as(provider, model, |request: &ModelRequest| {
        let ids = request.schema["properties"]["functions"]["items"]["properties"]["id"]["enum"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let translating = request.schema_name == "translated_explanations";
        let brief = request.schema_name == "function_briefs";
        let functions: Vec<Value> = ids
            .iter()
            .map(|id| {
                let changed = id == "refund.Request";
                if translating {
                    json!({
                        "id": id,
                        "brief": "Agora paga antes de gravar.",
                        "what_changed": "Paga antes de gravar.",
                        "why": "O commit diz que o card pede.",
                        "purpose": "Atende o pedido.",
                    })
                } else if brief {
                    json!({
                        "id": id,
                        "text": if changed {
                            "Now pays the refund before saving it, as PAY-482 asks."
                        } else {
                            "Serves the refund request."
                        },
                    })
                } else {
                    json!({
                        "id": id,
                        "what_changed": if changed { "Pays the refund before saving it." } else { "" },
                        "why": if changed { "The commit says PAY-482 asks for it." } else { "" },
                        "why_inferred": false,
                        "purpose": if changed { "Requests a refund." } else { "Serves the request." },
                    })
                }
            })
            .collect();
        Some(Ok(json!({ "functions": functions })))
    })
}

impl Fixture {
    fn new(model: Arc<ScriptedModel>) -> Self {
        let repo = TestRepo::init();
        repo.write("refund/service.go", BEFORE);
        repo.write("api/handler.go", HANDLER);
        repo.commit_all("initial commit");
        repo.git(&["checkout", "--quiet", "-b", "feature/PAY-482"]);
        repo.write("refund/service.go", AFTER);
        repo.commit_all("Pay the refund before saving it\n\nPAY-482 asks for gateway refunds.");
        let projects = ProjectService::new(Store::open_in_memory().unwrap(), isolated_git(), None);
        let id = projects
            .add_local(repo.path().to_str().unwrap())
            .unwrap()
            .id;
        Self {
            repo,
            projects,
            id,
            model,
            explanations: Arc::new(ExplanationService::new()),
        }
    }

    fn review(&self) -> ReviewRef {
        let ChangeSet {
            target,
            old_sha,
            new_sha,
            ..
        } = self
            .projects
            .changed_files(
                &self.id,
                ReviewTarget::Branch {
                    branch: "feature/PAY-482".to_string(),
                    base: "main".to_string(),
                },
            )
            .unwrap();
        ReviewRef {
            target,
            old_sha,
            new_sha,
        }
    }

    fn session(&self) -> ReviewSession<'_> {
        ReviewSession::new(
            self.projects.store(),
            self.projects.git(),
            &self.id,
            self.repo.path(),
            self.review(),
        )
        .unwrap()
    }

    fn context(&self) -> Context {
        let review = self.review();
        Context {
            target: Some(review.target.clone()),
            card: None,
            pull_request: None,
            commits: self
                .projects
                .git()
                .commit_messages(
                    self.repo.path(),
                    &review.old_sha,
                    review.new_sha.as_deref().unwrap(),
                    MAX_COMMITS,
                )
                .unwrap(),
        }
    }

    /// What the Explain buttons do: short explanations.
    fn explain(
        &self,
        graph: &FlowGraph,
        ids: &[&str],
        language: Language,
    ) -> crate::error::AppResult<crate::model::Explanations> {
        self.explain_at(graph, ids, ExplanationLevel::Brief, language)
    }

    fn explain_at(
        &self,
        graph: &FlowGraph,
        ids: &[&str],
        level: ExplanationLevel,
        language: Language,
    ) -> crate::error::AppResult<crate::model::Explanations> {
        let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        self.explanations.explain(
            &self.session(),
            &self.model.service(),
            graph,
            &ids,
            level,
            &self.context(),
            language,
            "explain-1",
        )
    }

    fn read(&self, graph: &FlowGraph) -> crate::model::Explanations {
        self.explanations
            .explanations(&self.session(), &spans_of(graph))
            .unwrap()
    }

    /// A detailed explanation of `function_id` as it was kept before short
    /// ones existed.
    fn keep_legacy(&self, graph: &FlowGraph, function_id: &str) {
        let session = self.session();
        let span = spans_of(graph)
            .into_iter()
            .find(|span| span.id == function_id)
            .unwrap();
        let text = std::fs::read_to_string(self.repo.path().join(&span.file)).unwrap();
        self.projects
            .store()
            .save_explanations(&[crate::store::ExplanationRecord {
                review_id: session.record().unwrap().id,
                function_id: function_id.to_string(),
                kind: ExplanationKind::Changed,
                fingerprint: crate::fingerprint::of_lines(&text, span.start_line, span.end_line),
                head_sha: self.review().new_sha,
                provider: "claude_cli".to_string(),
                model: "claude-opus-5-5".to_string(),
                created_at: "2026-11-20T00:31:55Z".to_string(),
                language: Language::BrazilianPortuguese,
                what_changed: "Passa a pagar antes de gravar.".to_string(),
                why: "Inferência: o card pede reembolsos.".to_string(),
                why_inferred: true,
                purpose: "Pede um reembolso.".to_string(),
            }])
            .unwrap();
    }
}

fn the_graph(request_lines: (u32, u32)) -> FlowGraph {
    graph(
        vec![
            node(
                "api.Handle",
                FlowNodeState::Untouched,
                "api/handler.go",
                (3, 5),
            ),
            node(
                "refund.Request",
                FlowNodeState::Modified,
                "refund/service.go",
                request_lines,
            ),
        ],
        vec![edge("api.Handle", "refund.Request")],
    )
}

#[test]
fn a_click_explains_a_changed_function_and_its_caller_in_short_and_keeps_them() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    let explained = fixture
        .explain(&graph, &["refund.Request", "api.Handle"], Language::English)
        .unwrap();

    let asked = fixture.model.asked();
    assert_eq!(asked.len(), 1, "both functions go in one call");
    let request = &asked[0];
    assert_eq!(request.system, service::BRIEF_SYSTEM_PROMPT);
    assert!(request.system.contains("at most about 35 words"));
    assert!(request.system.contains("never guess one from the code"));
    assert_eq!(request.schema_name, "function_briefs");
    assert_eq!(
        request.schema["properties"]["functions"]["items"]["required"],
        json!(["id", "text"])
    );
    let prompt = &request.prompt;
    assert!(prompt.starts_with("Write every prose field of your answer in English.\n\n"));
    assert!(prompt.contains("branch feature/PAY-482 compared with main"));
    assert!(prompt.contains("<untrusted_commits>\n--- commit:\nPay the refund before saving it"));
    assert!(prompt.contains("+     4 | \tpay(id)\n"), "{prompt}");
    assert!(prompt.contains("kind: context\n"));

    assert!(explained.functions.is_empty(), "no detailed one");
    let briefs = &explained.briefs;
    assert_eq!(briefs.len(), 2);
    assert_eq!(
        (briefs[0].function_id.as_str(), briefs[0].kind, briefs[0].text.as_str()),
        ("api.Handle", ExplanationKind::Context, "Serves the refund request.")
    );
    let request = &briefs[1];
    assert_eq!(request.kind, ExplanationKind::Changed);
    assert_eq!(request.text, "Now pays the refund before saving it, as PAY-482 asks.");
    assert_eq!(request.provider, ModelProviderKind::Anthropic);
    assert_eq!(request.model, "claude-opus-5-5-20261101");
    assert_eq!(request.head_sha, fixture.review().new_sha);
    assert_eq!(request.language, Language::English);
    assert!(!request.outdated);

    // Read again, without a model call.
    assert_eq!(fixture.read(&graph), explained);
    assert_eq!(fixture.model.asked().len(), 1);
}

#[test]
fn explain_in_detail_asks_one_call_for_one_function_and_keeps_the_short_one() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    fixture
        .explain(&graph, &["refund.Request", "api.Handle"], Language::English)
        .unwrap();
    let explained = fixture
        .explain_at(&graph, &["refund.Request"], ExplanationLevel::Detail, Language::English)
        .unwrap();

    let asked = fixture.model.asked();
    assert_eq!(asked.len(), 2);
    let request = &asked[1];
    assert_eq!(request.system, service::SYSTEM_PROMPT);
    assert_eq!(request.schema_name, "function_explanations");
    let prompt = &request.prompt;
    assert!(prompt.starts_with("Write every prose field of your answer in English.\n\n"));
    assert!(prompt.contains("branch feature/PAY-482 compared with main"));
    assert!(prompt.contains(
        "<untrusted_commits>\n--- commit:\nPay the refund before saving it\n\nPAY-482 asks"
    ));
    assert!(prompt.contains("=== refund.Request (modified) in refund/service.go, lines 3-7 ==="));
    assert!(prompt.contains("+     4 | \tpay(id)\n"), "{prompt}");
    assert!(prompt.contains("called by: api.Handle\n"));
    assert!(prompt.contains("Functions to explain (1):"), "only that function");
    assert!(!prompt.contains("kind: context\n"));

    let functions = &explained.functions;
    assert_eq!(functions.len(), 1);
    let request = &functions[0];
    assert_eq!(request.function_id, "refund.Request");
    assert_eq!(request.kind, ExplanationKind::Changed);
    assert_eq!(request.what_changed, "Pays the refund before saving it.");
    assert_eq!(request.why, "The commit says PAY-482 asks for it.");
    assert!(!request.why_inferred);
    assert_eq!(request.purpose, "Requests a refund.");
    assert!(!request.outdated);
    // Both short ones are still there.
    assert_eq!(explained.briefs.len(), 2);

    // A context function explained in detail by its own button: its purpose.
    let context = fixture
        .explain_at(&graph, &["api.Handle"], ExplanationLevel::Detail, Language::English)
        .unwrap();
    let handle = context
        .functions
        .iter()
        .find(|function| function.function_id == "api.Handle")
        .unwrap();
    assert_eq!(
        (handle.kind, handle.what_changed.as_str(), handle.purpose.as_str()),
        (ExplanationKind::Context, "", "Serves the request.")
    );
}

#[test]
fn each_level_and_translating_ask_the_model_of_their_own_task() {
    let fixture = Fixture::new(answering());
    let detail = answering_as(ModelProviderKind::OpenAi, "gpt-5.5");
    let translator = answering_as(ModelProviderKind::Gemini, "gemini-3-pro");
    let models = fixture
        .model
        .service()
        .with_task(
            ModelTask::DetailedExplanation,
            Arc::clone(&detail) as Arc<dyn ModelProvider>,
            ModelProviderKind::OpenAi,
            "gpt-5.5",
        )
        .with_task(
            ModelTask::Translation,
            Arc::clone(&translator) as Arc<dyn ModelProvider>,
            ModelProviderKind::Gemini,
            "gemini-3-pro",
        );
    let graph = the_graph((3, 7));
    let ids = ["refund.Request".to_string()];
    let explain = |level| {
        fixture
            .explanations
            .explain(
                &fixture.session(),
                &models,
                &graph,
                &ids,
                level,
                &fixture.context(),
                Language::English,
                "explain-1",
            )
            .unwrap()
    };

    explain(ExplanationLevel::Brief);
    let both = explain(ExplanationLevel::Detail);
    // The short one by the default model, the detailed one by its own; each
    // names who wrote it.
    assert_eq!(fixture.model.asked().len(), 1);
    assert_eq!(fixture.model.asked()[0].schema_name, "function_briefs");
    assert_eq!(detail.asked().len(), 1);
    assert_eq!(detail.asked()[0].schema_name, "function_explanations");
    assert_eq!(
        (both.briefs[0].provider, both.briefs[0].model.as_str()),
        (ModelProviderKind::Anthropic, "claude-opus-5-5-20261101")
    );
    assert_eq!(
        (both.functions[0].provider, both.functions[0].model.as_str()),
        (ModelProviderKind::OpenAi, "gpt-5.5")
    );

    let plan = fixture
        .explanations
        .plan_translation(&fixture.session(), &models, Language::BrazilianPortuguese)
        .unwrap();
    assert_eq!(plan.model.provider, ModelProviderKind::Gemini);
    assert_eq!(plan.model.model.as_deref(), Some("gemini-3-pro"));
    fixture
        .explanations
        .translate(
            &fixture.session(),
            &models,
            Language::BrazilianPortuguese,
            &plan.calls[0].ids,
            &spans_of(&graph),
            "translate-1",
        )
        .unwrap();
    assert_eq!(translator.asked().len(), 1);
    assert_eq!((fixture.model.asked().len(), detail.asked().len()), (1, 1));
}

#[test]
fn an_explanation_kept_before_short_ones_reads_as_the_detailed_one() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    fixture.keep_legacy(&graph, "refund.Request");

    let read = fixture.read(&graph);
    assert!(read.briefs.is_empty());
    assert_eq!(read.functions.len(), 1);
    let legacy = &read.functions[0];
    assert_eq!(legacy.what_changed, "Passa a pagar antes de gravar.");
    assert!(legacy.why_inferred);
    assert!(!legacy.outdated);

    // Explaining it in short adds the short one beside it.
    let both = fixture
        .explain(&graph, &["refund.Request"], Language::English)
        .unwrap();
    assert_eq!(both.briefs.len(), 1);
    assert_eq!(both.functions[0].what_changed, "Passa a pagar antes de gravar.");
}

#[test]
fn both_levels_are_outdated_once_their_function_changes() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    fixture
        .explain(&graph, &["refund.Request", "api.Handle"], Language::English)
        .unwrap();
    fixture
        .explain_at(&graph, &["refund.Request"], ExplanationLevel::Detail, Language::English)
        .unwrap();
    fixture.repo.commit_file(
        "refund/service.go",
        AFTER.replace("pay(id)", "pay(id, true)"),
        "pay with retries",
    );
    let read = fixture.read(&graph);
    let briefs: Vec<(&str, bool)> = read
        .briefs
        .iter()
        .map(|brief| (brief.function_id.as_str(), brief.outdated))
        .collect();
    assert_eq!(briefs, [("api.Handle", false), ("refund.Request", true)]);
    assert!(read.functions[0].outdated);

    // Explained again in short, the short one reads as current; the
    // detailed one stays outdated until it is detailed again.
    let again = fixture
        .explain(&graph, &["refund.Request"], Language::English)
        .unwrap();
    assert!(again.briefs.iter().all(|brief| !brief.outdated));
    assert!(again.functions[0].outdated);
    let detailed = fixture
        .explain_at(&graph, &["refund.Request"], ExplanationLevel::Detail, Language::English)
        .unwrap();
    assert!(!detailed.functions[0].outdated);
}

#[test]
fn explanations_are_written_in_the_language_shown_and_translated_on_request() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    let written = fixture
        .explain(&graph, &["refund.Request", "api.Handle"], Language::English)
        .unwrap();
    assert!(written
        .briefs
        .iter()
        .all(|brief| brief.language == Language::English));
    fixture
        .explain_at(&graph, &["refund.Request"], ExplanationLevel::Detail, Language::English)
        .unwrap();

    let models = fixture.model.service();
    let plan = fixture
        .explanations
        .plan_translation(&fixture.session(), &models, Language::BrazilianPortuguese)
        .unwrap();
    // One item per function, both of its levels in it.
    assert_eq!(plan.items, 2);
    assert_eq!(plan.calls.len(), 1);
    assert_eq!(fixture.model.asked().len(), 2);

    let translated = fixture
        .explanations
        .translate(
            &fixture.session(),
            &models,
            Language::BrazilianPortuguese,
            &plan.calls[0].ids,
            &spans_of(&graph),
            "translate-1",
        )
        .unwrap();
    let asked = fixture.model.asked();
    assert_eq!(asked[2].schema_name, "translated_explanations");
    assert!(
        !asked[2].prompt.contains("pay(id)"),
        "only the prose is sent"
    );
    assert!(asked[2]
        .prompt
        .contains("\"brief\": \"Now pays the refund before saving it, as PAY-482 asks.\""));
    // The detailed one and its translation.
    let request = &translated.functions[0];
    assert_eq!(request.what_changed, "Pays the refund before saving it.");
    assert_eq!(request.translations.len(), 1);
    assert_eq!(
        request.translations[0].what_changed,
        "Paga antes de gravar."
    );
    // Both short ones and theirs; the caller's had no detailed one to translate.
    for brief in &translated.briefs {
        assert_eq!(brief.translations.len(), 1, "{}", brief.function_id);
        assert_eq!(brief.translations[0].text, "Agora paga antes de gravar.");
    }
    assert_eq!(translated.functions.len(), 1);
    assert_eq!(
        fixture
            .explanations
            .plan_translation(&fixture.session(), &models, Language::BrazilianPortuguese)
            .unwrap()
            .items,
        0
    );
}

#[test]
fn a_failed_or_stopped_call_keeps_nothing() {
    let fixture = Fixture::new(ScriptedModel::new(|_| {
        Some(Err(ModelError::RateLimited {
            provider: ModelProviderKind::Anthropic,
            detail: None,
            retry_after: None,
        }))
    }));
    let error = fixture
        .explain(&the_graph((3, 7)), &["refund.Request"], Language::English)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RateLimited);
    assert!(fixture
        .explanations
        .explanations(&fixture.session(), &[])
        .unwrap()
        .functions
        .is_empty());

    // A call that never answers by itself is stopped by its run id.
    let fixture = Fixture::new(ScriptedModel::new(|_| None));
    let canceller = {
        let model = Arc::clone(&fixture.model);
        let explanations = Arc::clone(&fixture.explanations);
        std::thread::spawn(move || {
            while model.asked().is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            explanations.cancel("explain-1");
        })
    };
    let error = fixture
        .explain(&the_graph((3, 7)), &["refund.Request"], Language::English)
        .unwrap_err();
    canceller.join().unwrap();
    assert_eq!(error.code, ErrorCode::Cancelled);
}

#[test]
fn a_full_analysis_explains_only_the_changed_functions_in_short() {
    let fixture = Fixture::new(answering());
    let graph = the_graph((3, 7));
    let session = fixture.session();
    let planned = fixture
        .explanations
        .prepare(
            &session,
            &graph,
            &Scope::Full,
            ExplanationLevel::Brief,
            &fixture.context(),
        )
        .unwrap();
    let models = fixture.model.service();
    let plan = fixture.explanations.plan(
        &planned,
        models
            .in_use(session.store(), ModelTask::BriefExplanation)
            .unwrap(),
    );
    // Not the unchanged caller api.Handle.
    assert_eq!((plan.changed, plan.calls.len()), (1, 1));
    assert_eq!(plan.calls[0].ids, ["refund.Request"]);
    assert!(plan.tokens > 0);
    assert!(fixture.model.asked().is_empty());
    // The short prompt is the one counted.
    let detailed = fixture
        .explanations
        .prepare(
            &session,
            &graph,
            &Scope::Full,
            ExplanationLevel::Detail,
            &fixture.context(),
        )
        .unwrap();
    assert!(planned.calls[0].tokens < detailed.calls[0].tokens);

    let calls = Calls::new("full-1");
    let ends = fixture.explanations.run(
        &session,
        &models,
        &planned,
        Language::BrazilianPortuguese,
        &calls,
        3,
        &|_, _| {},
    );
    assert_eq!(ends, [Some(Ok(1))]);
    let asked = &fixture.model.asked()[0];
    assert_eq!(asked.system, service::BRIEF_SYSTEM_PROMPT);
    assert!(asked
        .prompt
        .starts_with("Write every prose field of your answer in Brazilian Portuguese."));
    assert!(!asked.prompt.contains("kind: context"));
    let read = fixture.read(&graph);
    assert_eq!(read.briefs.len(), 1);
    assert!(read.functions.is_empty());
}
