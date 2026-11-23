//! The Anthropic API (Claude models), over raw HTTP: the Messages API with
//! structured output, refusal fallback on, and the Models API to list what a
//! key can use.
//!
//! Request shape (https://platform.claude.com/docs/en/build-with-claude/structured-outputs,
//! /effort and /refusals-and-fallback):
//! `POST /v1/messages` with `x-api-key`, `anthropic-version: 2026-10-21` and
//! `anthropic-beta: server-side-fallback-2026-07-01`; the body carries
//! `output_config: { effort, format: { type: "json_schema", schema } }` and
//! `fallbacks: "default"`. No `temperature`, `top_p`, `top_k` or `thinking`:
//! the version rejects the first three, and thinking is always on for the
//! current models, steered by the effort.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use reqwest::Url;
use serde_json::{json, Value};

use super::api::{self, MODEL_TIMEOUT};
use super::provider::{ModelAnswer, ModelError, ModelProvider, ModelRequest, Usage};
use crate::error::core_text;
use crate::http::{Auth, HttpRequest, Method, Secret, Transport};
use crate::model::{Effort, ModelInfo, ModelProviderKind};

pub const API: &str = "https://api.anthropic.com";
const VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const DEFAULT_EFFORT: Effort = Effort::High;
const MAX_TOKENS: u64 = 16_000;
/// Pages of the Models API read at most; there are far fewer models.
const MAX_PAGES: usize = 10;
const KIND: ModelProviderKind = ModelProviderKind::Anthropic;

pub struct Anthropic {
    api: String,
    key: Secret,
    model: String,
    effort: Effort,
    transport: Arc<dyn Transport>,
}

impl Anthropic {
    pub fn new(
        api: &str,
        key: Secret,
        model: &str,
        effort: Effort,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            api: api.trim_end_matches('/').to_string(),
            key,
            model: model.to_string(),
            effort,
            transport,
        }
    }

    fn request(&self, method: Method, url: impl Into<String>) -> HttpRequest {
        HttpRequest::new(
            method,
            url,
            Auth::Header {
                name: "x-api-key",
                value: self.key.clone(),
            },
        )
        .header("anthropic-version", VERSION)
    }

    pub fn body(&self, request: &ModelRequest) -> Value {
        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": request.system,
            "messages": [{ "role": "user", "content": request.prompt }],
            "output_config": {
                "effort": self.effort,
                "format": { "type": "json_schema", "schema": request.schema },
            },
            "fallbacks": "default",
        })
    }
}

impl ModelProvider for Anthropic {
    fn ask(&self, request: &ModelRequest, _: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        let http = self
            .request(Method::Post, format!("{}/v1/messages", self.api))
            .header("anthropic-beta", FALLBACK_BETA)
            .json(self.body(request))
            .timeout(MODEL_TIMEOUT);
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        read_message(&api::parse_body(KIND, &body)?, &self.model, &request.schema)
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = Url::parse(&format!("{}/v1/models", self.api))
                .map_err(|error| api::bad_url("Anthropic", Some(&error)))?;
            url.query_pairs_mut().append_pair("limit", "1000");
            if let Some(after) = &after {
                url.query_pairs_mut().append_pair("after_id", after);
            }
            let http = self.request(Method::Get, url.to_string());
            let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
            let page = api::parse_body(KIND, &body)?;
            for entry in page["data"].as_array().into_iter().flatten() {
                // Models whose capabilities say they cannot give a structured
                // answer are of no use here; unknown capabilities are kept.
                if entry["capabilities"]["structured_outputs"]["supported"] == json!(false) {
                    continue;
                }
                if let Some(id) = entry["id"].as_str() {
                    let name = entry["display_name"].as_str().unwrap_or(id);
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name: name.to_string(),
                    });
                }
            }
            match (page["has_more"].as_bool(), page["last_id"].as_str()) {
                (Some(true), Some(last)) => after = Some(last.to_string()),
                _ => break,
            }
        }
        Ok(models)
    }
}

/// Reads a Messages API answer. The stop reason comes first: a refusal or a
/// cut-off answer has no usable JSON. After a refusal fallback, text the
/// declining model wrote before the hand-off precedes the last `fallback`
/// block, so only what follows it is the answer.
pub fn read_message(
    message: &Value,
    asked: &str,
    schema: &Value,
) -> Result<ModelAnswer, ModelError> {
    match message["stop_reason"].as_str() {
        Some("refusal") => {
            let details = &message["stop_details"];
            return Err(ModelError::Refused {
                provider: KIND,
                category: details["category"].as_str().map(str::to_string),
                explanation: details["explanation"].as_str().map(str::to_string),
            });
        }
        Some("max_tokens") => {
            return Err(ModelError::Truncated {
                provider: KIND,
                reason: core_text!(
                    OutputLimitReached,
                    "it reached the limit of {tokens} output tokens",
                    tokens = MAX_TOKENS
                ),
            })
        }
        Some("model_context_window_exceeded") => {
            return Err(ModelError::Truncated {
                provider: KIND,
                reason: core_text!(ContextWindowFilled, "it filled the model's context window"),
            })
        }
        _ => {}
    }
    let blocks = message["content"].as_array().cloned().unwrap_or_default();
    let start = blocks
        .iter()
        .rposition(|block| block["type"] == "fallback")
        .map_or(0, |index| index + 1);
    let text: String = blocks[start..]
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    let value = api::parse_answer(KIND, &text, schema)?;
    let usage = &message["usage"];
    Ok(ModelAnswer {
        value,
        provider: KIND,
        model: message["model"].as_str().unwrap_or(asked).to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::tests::{request, schema};
    use crate::http::tests::CannedTransport;

    fn client(transport: &Arc<CannedTransport>) -> Anthropic {
        Anthropic::new(
            "https://api.anthropic.test",
            Secret::new("sk-ant-secret"),
            DEFAULT_MODEL,
            Effort::High,
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    fn message(content: Value, stop_reason: &str, stop_details: Value) -> Value {
        json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-5-5",
            "content": content,
            "stop_reason": stop_reason,
            "stop_details": stop_details,
            "usage": {
                "input_tokens": 120,
                "output_tokens": 40,
                "cache_read_input_tokens": 10,
                "cache_creation_input_tokens": 0
            }
        })
    }

    #[test]
    fn asks_for_structured_output_with_effort_and_fallback_and_reads_the_answer() {
        let transport = CannedTransport::answering(vec![(
            200,
            message(
                json!([
                    { "type": "thinking", "thinking": "", "signature": "x" },
                    { "type": "text", "text": "{\"ok\":true,\"reply\":\"Hello, reviewer.\"}" }
                ]),
                "end_turn",
                Value::Null,
            ),
        )]);
        let answer = client(&transport)
            .ask(&request(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            answer.value,
            json!({"ok": true, "reply": "Hello, reviewer."})
        );
        assert_eq!(answer.model, "claude-opus-5-5");
        assert_eq!(
            answer.usage,
            Some(Usage {
                input_tokens: Some(130),
                output_tokens: Some(40)
            })
        );

        let sent = &transport.sent()[0];
        assert_eq!(sent.method, Method::Post);
        assert_eq!(sent.url, "https://api.anthropic.test/v1/messages");
        assert_eq!(
            sent.auth,
            Auth::Header {
                name: "x-api-key",
                value: Secret::new("sk-ant-secret")
            }
        );
        assert!(sent
            .headers
            .contains(&("anthropic-version", "2026-10-21".to_string())));
        assert!(sent.headers.contains(&(
            "anthropic-beta",
            "server-side-fallback-2026-07-01".to_string()
        )));
        assert_eq!(sent.timeout, Some(MODEL_TIMEOUT));
        assert_eq!(
            sent.body,
            Some(json!({
                "model": "claude-opus-5-5",
                "max_tokens": 16000,
                "system": "You check that a connection works.",
                "messages": [{ "role": "user", "content": "Say hello." }],
                "output_config": {
                    "effort": "high",
                    "format": { "type": "json_schema", "schema": schema() }
                },
                "fallbacks": "default"
            }))
        );
        let body = sent.body.as_ref().unwrap();
        for absent in ["temperature", "top_p", "top_k", "thinking"] {
            assert!(body.get(absent).is_none(), "{absent} must not be sent");
        }
    }

    #[test]
    fn after_a_fallback_only_the_text_after_the_hand_off_is_the_answer() {
        let mut answered = message(
            json!([
                { "type": "text", "text": "{\"ok\":" },
                {
                    "type": "fallback",
                    "from": { "model": "claude-opus-5-5" },
                    "to": { "model": "claude-opus-5" },
                    "trigger": { "type": "refusal", "category": "cyber" }
                },
                { "type": "text", "text": "{\"ok\":true,\"reply\":\"Hi\"}" }
            ]),
            "end_turn",
            Value::Null,
        );
        answered["model"] = json!("claude-opus-5");
        let answer = read_message(&answered, DEFAULT_MODEL, &schema()).unwrap();
        assert_eq!(answer.value, json!({"ok": true, "reply": "Hi"}));
        assert_eq!(answer.model, "claude-opus-5");
    }

    #[test]
    fn a_refusal_and_a_cut_off_answer_are_distinct_errors() {
        let refused = message(
            json!([]),
            "refusal",
            json!({
                "type": "refusal",
                "category": "cyber",
                "explanation": "This request was declined because it could enable cyber harm.",
                "recommended_model": null
            }),
        );
        assert_eq!(
            read_message(&refused, DEFAULT_MODEL, &schema()),
            Err(ModelError::Refused {
                provider: KIND,
                category: Some("cyber".to_string()),
                explanation: Some(
                    "This request was declined because it could enable cyber harm.".to_string()
                ),
            })
        );
        let uncategorised = message(
            json!([]),
            "refusal",
            json!({ "category": null, "explanation": null }),
        );
        assert!(matches!(
            read_message(&uncategorised, DEFAULT_MODEL, &schema()),
            Err(ModelError::Refused {
                category: None,
                explanation: None,
                ..
            })
        ));

        let cut = message(
            json!([{ "type": "text", "text": "{\"ok\":tr" }]),
            "max_tokens",
            Value::Null,
        );
        assert!(matches!(
            read_message(&cut, DEFAULT_MODEL, &schema()),
            Err(ModelError::Truncated { .. })
        ));
        let full = message(json!([]), "model_context_window_exceeded", Value::Null);
        assert!(matches!(
            read_message(&full, DEFAULT_MODEL, &schema()),
            Err(ModelError::Truncated { .. })
        ));
    }

    #[test]
    fn a_bad_key_is_an_auth_failure() {
        let transport = CannedTransport::answering(vec![(
            401,
            json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "invalid x-api-key" },
                "request_id": "req_01"
            }),
        )]);
        let error = client(&transport)
            .ask(&request(), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error,
            ModelError::AuthFailed {
                provider: KIND,
                detail: Some("invalid x-api-key".to_string())
            }
        );
    }

    #[test]
    fn lists_models_across_pages_leaving_out_those_without_structured_output() {
        let transport = CannedTransport::answering(vec![
            (
                200,
                json!({
                    "data": [
                        {
                            "type": "model",
                            "id": "claude-opus-5-5",
                            "display_name": "Claude Opus 5.5",
                            "capabilities": { "structured_outputs": { "supported": true } }
                        },
                        {
                            "type": "model",
                            "id": "claude-old",
                            "display_name": "Claude Old",
                            "capabilities": { "structured_outputs": { "supported": false } }
                        }
                    ],
                    "has_more": true,
                    "first_id": "claude-opus-5-5",
                    "last_id": "claude-old"
                }),
            ),
            (
                200,
                json!({
                    "data": [{ "type": "model", "id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5", "capabilities": null }],
                    "has_more": false,
                    "first_id": "claude-haiku-5-5",
                    "last_id": "claude-haiku-5-5"
                }),
            ),
        ]);
        let models = client(&transport).models().unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "claude-opus-5-5".to_string(),
                    name: "Claude Opus 5.5".to_string()
                },
                ModelInfo {
                    id: "claude-haiku-5-5".to_string(),
                    name: "Claude Haiku 5.5".to_string()
                },
            ]
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://api.anthropic.test/v1/models?limit=1000"
        );
        assert_eq!(
            sent[1].url,
            "https://api.anthropic.test/v1/models?limit=1000&after_id=claude-old"
        );
        assert_eq!(sent[0].method, Method::Get);
    }
}
