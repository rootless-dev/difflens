//! OpenRouter, over raw HTTP: its OpenAI-style Chat Completions with a strict
//! JSON Schema, routed only to endpoints that honour it, and its models list
//! filtered to those that give structured outputs.
//!
//! Request shape (https://openrouter.ai/docs/features/structured-outputs,
//! /docs/api/reference/parameters, /docs/api/reference/errors-and-debugging
//! and /docs/api/api-reference/models/get-models):
//! `POST /api/v1/chat/completions` with `Authorization: Bearer <key>`; the
//! body carries `messages`, `max_tokens`,
//! `response_format = { type: "json_schema", json_schema: { name, strict,
//! schema } }` and `provider.require_parameters: true`. An error can come
//! back with HTTP 200, in the body's `error`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};

use super::api::{self, MODEL_TIMEOUT, REASONING_OUTPUT_TOKENS};
use super::provider::{ModelAnswer, ModelError, ModelProvider, ModelRequest, Usage};
use crate::error::{core_text, CoreText};
use crate::http::{Auth, HttpRequest, Method, Secret, Transport};
use crate::model::{ModelInfo, ModelProviderKind};

pub const API: &str = "https://openrouter.ai/api";
const KIND: ModelProviderKind = ModelProviderKind::OpenRouter;

pub struct OpenRouter {
    api: String,
    key: Secret,
    model: String,
    transport: Arc<dyn Transport>,
}

impl OpenRouter {
    pub fn new(api: &str, key: Secret, model: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: api.trim_end_matches('/').to_string(),
            key,
            model: model.to_string(),
            transport,
        }
    }

    fn request(&self, method: Method, url: String) -> HttpRequest {
        HttpRequest::new(
            method,
            url,
            Auth::Header {
                name: "Authorization",
                value: Secret::new(format!("Bearer {}", self.key.expose())),
            },
        )
        // How the app is named in the user's OpenRouter activity.
        .header("X-OpenRouter-Title", "DiffLens")
    }

    pub fn body(&self, request: &ModelRequest) -> Value {
        json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.prompt },
            ],
            "max_tokens": REASONING_OUTPUT_TOKENS,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema,
                },
            },
            "provider": { "require_parameters": true },
        })
    }
}

impl ModelProvider for OpenRouter {
    fn ask(&self, request: &ModelRequest, _: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        let http = self
            .request(Method::Post, format!("{}/v1/chat/completions", self.api))
            .json(self.body(request))
            .timeout(MODEL_TIMEOUT);
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        read_completion(&api::parse_body(KIND, &body)?, &self.model, &request.schema)
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let http = self.request(
            Method::Get,
            format!(
                "{}/v1/models?supported_parameters=structured_outputs",
                self.api
            ),
        );
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        let page = api::parse_body(KIND, &body)?;
        Ok(page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let id = entry["id"].as_str()?;
                Some(ModelInfo {
                    id: id.to_string(),
                    name: entry["name"].as_str().unwrap_or(id).to_string(),
                })
            })
            .collect())
    }
}

/// Reads a chat completion: an error in the body first (OpenRouter sends
/// some with HTTP 200), then how the choice finished, then its content.
pub fn read_completion(
    response: &Value,
    asked: &str,
    schema: &Value,
) -> Result<ModelAnswer, ModelError> {
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        return Err(body_error(error));
    }
    let choice = &response["choices"][0];
    if choice.is_null() {
        return Err(ModelError::InvalidOutput {
            provider: KIND,
            detail: core_text!(NoAnswerReturned, "it returned no answer"),
        });
    }
    let native = choice["native_finish_reason"].as_str().map(str::to_string);
    match choice["finish_reason"].as_str() {
        Some("length") => {
            return Err(ModelError::Truncated {
                provider: KIND,
                reason: core_text!(
                    OutputLimitWithReasoning,
                    "it reached the limit of {tokens} output tokens, reasoning included",
                    tokens = REASONING_OUTPUT_TOKENS
                ),
            })
        }
        Some("content_filter") => {
            return Err(ModelError::Refused {
                provider: KIND,
                category: native,
                explanation: None,
            })
        }
        Some("error") => {
            return Err(ModelError::ProviderFailed {
                provider: KIND,
                status: None,
                message: match native.as_deref() {
                    Some(native) => core_text!(
                        UpstreamProviderFailed,
                        "the model's provider failed ({reason})",
                        reason = native
                    ),
                    None => core_text!(
                        UpstreamProviderFailedQuietly,
                        "the model's provider failed (no reason given)"
                    ),
                },
            })
        }
        _ => {}
    }
    let message = &choice["message"];
    if let Some(refusal) = message["refusal"].as_str().filter(|text| !text.is_empty()) {
        return Err(ModelError::Refused {
            provider: KIND,
            category: None,
            explanation: Some(refusal.to_string()),
        });
    }
    let value = api::parse_answer(KIND, message["content"].as_str().unwrap_or(""), schema)?;
    let usage = &response["usage"];
    Ok(ModelAnswer {
        value,
        provider: KIND,
        model: response["model"].as_str().unwrap_or(asked).to_string(),
        usage: Some(Usage {
            input_tokens: usage["prompt_tokens"].as_u64(),
            output_tokens: usage["completion_tokens"].as_u64(),
        }),
    })
}

/// An `error` object in a body: its `code` is the HTTP status it stands for.
fn body_error(error: &Value) -> ModelError {
    let detail = error["message"].as_str().map(str::to_string);
    match error["code"].as_u64() {
        Some(401) => ModelError::AuthFailed {
            provider: KIND,
            detail,
        },
        Some(429) => ModelError::RateLimited {
            provider: KIND,
            detail,
            retry_after: None,
        },
        status => ModelError::ProviderFailed {
            provider: KIND,
            status: status.and_then(|code| u16::try_from(code).ok()),
            message: match detail {
                Some(detail) => CoreText::raw(detail),
                None => core_text!(ProviderGaveNoExplanation, "it gave no reason"),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::tests::{request, schema};
    use crate::http::tests::CannedTransport;

    fn client(transport: &Arc<CannedTransport>) -> OpenRouter {
        OpenRouter::new(
            "https://openrouter.test/api",
            Secret::new("sk-or-secret"),
            "anthropic/claude-opus-5.5",
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    fn completion(finish_reason: &str, message: Value) -> Value {
        json!({
            "id": "gen-01",
            "object": "chat.completion",
            "created": 1_790_000_000,
            "model": "anthropic/claude-opus-5.5",
            "choices": [{
                "index": 0,
                "finish_reason": finish_reason,
                "native_finish_reason": "end_turn",
                "message": message
            }],
            "usage": { "prompt_tokens": 70, "completion_tokens": 25, "total_tokens": 95 }
        })
    }

    #[test]
    fn asks_for_a_strict_schema_on_endpoints_that_honour_it() {
        let transport = CannedTransport::answering(vec![(
            200,
            completion(
                "stop",
                json!({ "role": "assistant", "content": "{\"ok\":true,\"reply\":\"Hi\"}" }),
            ),
        )]);
        let answer = client(&transport)
            .ask(&request(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(answer.value, json!({"ok": true, "reply": "Hi"}));
        assert_eq!(answer.model, "anthropic/claude-opus-5.5");
        assert_eq!(
            answer.usage,
            Some(Usage {
                input_tokens: Some(70),
                output_tokens: Some(25)
            })
        );

        let sent = &transport.sent()[0];
        assert_eq!(sent.url, "https://openrouter.test/api/v1/chat/completions");
        assert_eq!(
            sent.auth,
            Auth::Header {
                name: "Authorization",
                value: Secret::new("Bearer sk-or-secret")
            }
        );
        assert_eq!(
            sent.body,
            Some(json!({
                "model": "anthropic/claude-opus-5.5",
                "messages": [
                    { "role": "system", "content": "You check that a connection works." },
                    { "role": "user", "content": "Say hello." }
                ],
                "max_tokens": 32000,
                "response_format": { "type": "json_schema", "json_schema": {
                    "name": "connection_check",
                    "strict": true,
                    "schema": schema()
                } },
                "provider": { "require_parameters": true }
            }))
        );
    }

    #[test]
    fn errors_in_a_200_body_refusals_and_cut_off_answers_are_distinct() {
        let in_body = json!({ "error": { "code": 402, "message": "Insufficient credits" } });
        assert_eq!(
            read_completion(&in_body, "m", &schema()),
            Err(ModelError::ProviderFailed {
                provider: KIND,
                status: Some(402),
                message: CoreText::raw("Insufficient credits")
            })
        );
        assert!(matches!(
            read_completion(
                &json!({ "error": { "code": 401, "message": "No auth credentials found" } }),
                "m",
                &schema()
            ),
            Err(ModelError::AuthFailed { .. })
        ));
        assert!(matches!(
            read_completion(
                &completion("length", json!({ "content": "{" })),
                "m",
                &schema()
            ),
            Err(ModelError::Truncated { .. })
        ));
        assert!(matches!(
            read_completion(
                &completion("content_filter", json!({ "content": null })),
                "m",
                &schema()
            ),
            Err(ModelError::Refused { .. })
        ));
        assert!(matches!(
            read_completion(
                &completion(
                    "stop",
                    json!({ "content": null, "refusal": "I can't help with that." })
                ),
                "m",
                &schema()
            ),
            Err(ModelError::Refused { .. })
        ));
        assert!(matches!(
            read_completion(
                &completion("error", json!({ "content": null })),
                "m",
                &schema()
            ),
            Err(ModelError::ProviderFailed { .. })
        ));
    }

    #[test]
    fn lists_the_models_that_give_structured_outputs() {
        let transport = CannedTransport::answering(vec![(
            200,
            json!({
                "data": [
                    { "id": "anthropic/claude-opus-5.5", "name": "Anthropic: Claude Opus 5.5", "supported_parameters": ["structured_outputs"] },
                    { "id": "openai/gpt-5.5", "name": "OpenAI: GPT-5.5", "supported_parameters": ["structured_outputs"] }
                ]
            }),
        )]);
        let models = client(&transport).models().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].name, "OpenAI: GPT-5.5");
        assert_eq!(
            transport.sent()[0].url,
            "https://openrouter.test/api/v1/models?supported_parameters=structured_outputs"
        );
    }
}
