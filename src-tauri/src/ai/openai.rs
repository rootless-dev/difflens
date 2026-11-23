//! The OpenAI API, over raw HTTP: the Responses API with a strict JSON
//! Schema, and the Models API to list what a key can use.
//!
//! Request shape (https://developers.openai.com/api/docs/guides/structured-outputs,
//! /api/reference/resources/responses/methods/create and
//! /api/docs/guides/migrate-to-responses): `POST /v1/responses` with
//! `Authorization: Bearer <key>`; the body carries `instructions`, `input`,
//! `max_output_tokens`, `store: false` and
//! `text.format = { type: "json_schema", name, schema, strict: true }`.
//! No `temperature`: reasoning models reject it, and their reasoning counts
//! against `max_output_tokens`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};

use super::api::{self, MODEL_TIMEOUT, REASONING_OUTPUT_TOKENS};
use super::provider::{ModelAnswer, ModelError, ModelProvider, ModelRequest, Usage};
use crate::error::{core_text, CoreText};
use crate::http::{Auth, HttpRequest, Method, Secret, Transport};
use crate::model::{ModelInfo, ModelProviderKind};

pub const API: &str = "https://api.openai.com";
const KIND: ModelProviderKind = ModelProviderKind::OpenAi;

/// Words in the ids of models that do not write text (embeddings, speech,
/// images, moderation) or are not asked through the Responses API.
const NOT_FOR_TEXT: &[&str] = &[
    "embedding",
    "tts",
    "whisper",
    "transcribe",
    "dall-e",
    "image",
    "audio",
    "realtime",
    "moderation",
    "sora",
    "davinci",
    "babbage",
    "search",
];

pub struct OpenAi {
    api: String,
    key: Secret,
    model: String,
    transport: Arc<dyn Transport>,
}

impl OpenAi {
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
    }

    pub fn body(&self, request: &ModelRequest) -> Value {
        json!({
            "model": self.model,
            "instructions": request.system,
            "input": request.prompt,
            "max_output_tokens": REASONING_OUTPUT_TOKENS,
            "store": false,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": request.schema_name,
                    "schema": request.schema,
                    "strict": true,
                }
            },
        })
    }
}

impl ModelProvider for OpenAi {
    fn ask(&self, request: &ModelRequest, _: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        let http = self
            .request(Method::Post, format!("{}/v1/responses", self.api))
            .json(self.body(request))
            .timeout(MODEL_TIMEOUT);
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        read_response(&api::parse_body(KIND, &body)?, &self.model, &request.schema)
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let http = self.request(Method::Get, format!("{}/v1/models", self.api));
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        let page = api::parse_body(KIND, &body)?;
        let mut entries: Vec<(&str, i64)> = page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some((
                    entry["id"].as_str()?,
                    entry["created"].as_i64().unwrap_or(0),
                ))
            })
            .filter(|(id, _)| !NOT_FOR_TEXT.iter().any(|word| id.contains(word)))
            .collect();
        // Newest first, as the other providers list them.
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        Ok(entries
            .into_iter()
            .map(|(id, _)| ModelInfo {
                id: id.to_string(),
                name: id.to_string(),
            })
            .collect())
    }
}

/// Reads a Responses API answer: an incomplete or failed response first,
/// then a refusal part, then the text, which is the JSON.
pub fn read_response(
    response: &Value,
    asked: &str,
    schema: &Value,
) -> Result<ModelAnswer, ModelError> {
    match response["status"].as_str() {
        Some("incomplete") => {
            let reason = response["incomplete_details"]["reason"]
                .as_str()
                .unwrap_or("unknown");
            return Err(if reason == "content_filter" {
                ModelError::Refused {
                    provider: KIND,
                    category: Some("content_filter".to_string()),
                    explanation: None,
                }
            } else if reason == "max_output_tokens" {
                ModelError::Truncated {
                    provider: KIND,
                    reason: core_text!(
                        OutputLimitWithReasoning,
                        "it reached the limit of {tokens} output tokens, reasoning included",
                        tokens = REASONING_OUTPUT_TOKENS
                    ),
                }
            } else {
                ModelError::Truncated {
                    provider: KIND,
                    reason: core_text!(
                        StoppedEarly,
                        "it stopped early ({reason})",
                        reason = reason
                    ),
                }
            });
        }
        Some("failed") => {
            return Err(ModelError::ProviderFailed {
                provider: KIND,
                status: None,
                message: match response["error"]["message"].as_str() {
                    Some(message) => CoreText::raw(message),
                    None => core_text!(ResponseFailed, "the response failed"),
                },
            })
        }
        _ => {}
    }
    let parts: Vec<&Value> = response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .collect();
    if let Some(refusal) = parts.iter().find(|part| part["type"] == "refusal") {
        return Err(ModelError::Refused {
            provider: KIND,
            category: None,
            explanation: refusal["refusal"].as_str().map(str::to_string),
        });
    }
    let text: String = parts
        .iter()
        .filter(|part| part["type"] == "output_text")
        .filter_map(|part| part["text"].as_str())
        .collect();
    let value = api::parse_answer(KIND, &text, schema)?;
    let usage = &response["usage"];
    Ok(ModelAnswer {
        value,
        provider: KIND,
        model: response["model"].as_str().unwrap_or(asked).to_string(),
        usage: Some(Usage {
            input_tokens: usage["input_tokens"].as_u64(),
            output_tokens: usage["output_tokens"].as_u64(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::tests::{request, schema};
    use crate::http::tests::CannedTransport;

    fn client(transport: &Arc<CannedTransport>) -> OpenAi {
        OpenAi::new(
            "https://api.openai.test",
            Secret::new("sk-proj-secret"),
            "gpt-5.5",
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    fn response(status: &str, content: Value) -> Value {
        json!({
            "id": "resp_01",
            "object": "response",
            "status": status,
            "model": "gpt-5.5-2026-09-01",
            "output": [
                { "type": "reasoning", "id": "rs_01", "summary": [] },
                { "type": "message", "id": "msg_01", "role": "assistant", "content": content }
            ],
            "usage": {
                "input_tokens": 90,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 300,
                "output_tokens_details": { "reasoning_tokens": 260 },
                "total_tokens": 390
            }
        })
    }

    #[test]
    fn asks_the_responses_api_with_a_strict_schema_and_reads_the_answer() {
        let transport = CannedTransport::answering(vec![(
            200,
            response(
                "completed",
                json!([{ "type": "output_text", "text": "{\"ok\":true,\"reply\":\"Hi\"}", "annotations": [] }]),
            ),
        )]);
        let answer = client(&transport)
            .ask(&request(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(answer.value, json!({"ok": true, "reply": "Hi"}));
        assert_eq!(answer.model, "gpt-5.5-2026-09-01");
        assert_eq!(
            answer.usage,
            Some(Usage {
                input_tokens: Some(90),
                output_tokens: Some(300)
            })
        );

        let sent = &transport.sent()[0];
        assert_eq!(sent.url, "https://api.openai.test/v1/responses");
        assert_eq!(
            sent.auth,
            Auth::Header {
                name: "Authorization",
                value: Secret::new("Bearer sk-proj-secret")
            }
        );
        assert_eq!(
            sent.body,
            Some(json!({
                "model": "gpt-5.5",
                "instructions": "You check that a connection works.",
                "input": "Say hello.",
                "max_output_tokens": 32000,
                "store": false,
                "text": { "format": {
                    "type": "json_schema",
                    "name": "connection_check",
                    "schema": schema(),
                    "strict": true
                } }
            }))
        );
    }

    #[test]
    fn a_refusal_a_filter_and_a_cut_off_answer_are_distinct_errors() {
        let refused = response(
            "completed",
            json!([{ "type": "refusal", "refusal": "I can't help with that." }]),
        );
        assert_eq!(
            read_response(&refused, "gpt-5.5", &schema()),
            Err(ModelError::Refused {
                provider: KIND,
                category: None,
                explanation: Some("I can't help with that.".to_string())
            })
        );

        let mut filtered = response("incomplete", json!([]));
        filtered["incomplete_details"] = json!({ "reason": "content_filter" });
        assert!(matches!(
            read_response(&filtered, "gpt-5.5", &schema()),
            Err(ModelError::Refused { .. })
        ));

        let mut cut = response("incomplete", json!([]));
        cut["incomplete_details"] = json!({ "reason": "max_output_tokens" });
        assert!(matches!(
            read_response(&cut, "gpt-5.5", &schema()),
            Err(ModelError::Truncated { .. })
        ));

        let mut failed = response("failed", json!([]));
        failed["error"] = json!({ "code": "server_error", "message": "The server had an error" });
        assert!(matches!(
            read_response(&failed, "gpt-5.5", &schema()),
            Err(ModelError::ProviderFailed { .. })
        ));
    }

    #[test]
    fn lists_text_models_newest_first() {
        let transport = CannedTransport::answering(vec![(
            200,
            json!({
                "object": "list",
                "data": [
                    { "id": "gpt-4.1", "object": "model", "created": 100, "owned_by": "openai" },
                    { "id": "text-embedding-3-large", "object": "model", "created": 300, "owned_by": "openai" },
                    { "id": "gpt-5.5", "object": "model", "created": 200, "owned_by": "openai" },
                    { "id": "gpt-image-2", "object": "model", "created": 400, "owned_by": "openai" }
                ]
            }),
        )]);
        let ids: Vec<String> = client(&transport)
            .models()
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, ["gpt-5.5", "gpt-4.1"]);
        assert_eq!(transport.sent()[0].url, "https://api.openai.test/v1/models");
    }
}
