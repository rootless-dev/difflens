//! The Google Gemini API (the developer API, with an API key), over raw
//! HTTP: `generateContent` with a JSON Schema, and the models list.
//!
//! Request shape (https://aistudio.google.com/docs/structured-output,
//! /api-reference/generate-content and /api-reference/models/list-models):
//! `POST /v1beta/models/{model}:generateContent` with `x-goog-api-key`; the
//! body carries `systemInstruction`, `contents` and
//! `generationConfig = { maxOutputTokens, responseMimeType:
//! "application/json", responseJsonSchema }`. `responseJsonSchema` takes a
//! standard JSON Schema (the older `responseSchema` took an OpenAPI subset).
//! Google now calls `generateContent` its legacy API, still supported; its
//! Interactions API is newer.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use reqwest::Url;
use serde_json::{json, Value};

use super::api::{self, MODEL_TIMEOUT, REASONING_OUTPUT_TOKENS};
use super::provider::{ModelAnswer, ModelError, ModelProvider, ModelRequest, Usage};
use crate::error::core_text;
use crate::http::{Auth, HttpRequest, Method, Secret, Transport};
use crate::model::{ModelInfo, ModelProviderKind};

pub const API: &str = "https://generativelanguage.googleapis.com";
const KIND: ModelProviderKind = ModelProviderKind::Gemini;
const MAX_PAGES: usize = 10;

/// Finish reasons that mean the model, or Google's filters, declined.
const DECLINED: &[&str] = &[
    "SAFETY",
    "RECITATION",
    "LANGUAGE",
    "BLOCKLIST",
    "PROHIBITED_CONTENT",
    "SPII",
    "IMAGE_SAFETY",
];

pub struct Gemini {
    api: String,
    key: Secret,
    model: String,
    transport: Arc<dyn Transport>,
}

impl Gemini {
    pub fn new(api: &str, key: Secret, model: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: api.trim_end_matches('/').to_string(),
            key,
            // The models list names them "models/…"; the URL adds that.
            model: model.trim_start_matches("models/").to_string(),
            transport,
        }
    }

    fn request(&self, method: Method, url: String) -> HttpRequest {
        HttpRequest::new(
            method,
            url,
            Auth::Header {
                name: "x-goog-api-key",
                value: self.key.clone(),
            },
        )
    }

    pub fn body(&self, request: &ModelRequest) -> Value {
        json!({
            "systemInstruction": { "parts": [{ "text": request.system }] },
            "contents": [{ "role": "user", "parts": [{ "text": request.prompt }] }],
            "generationConfig": {
                "maxOutputTokens": REASONING_OUTPUT_TOKENS,
                "responseMimeType": "application/json",
                "responseJsonSchema": request.schema,
            },
        })
    }
}

impl ModelProvider for Gemini {
    fn ask(&self, request: &ModelRequest, _: &AtomicBool) -> Result<ModelAnswer, ModelError> {
        let mut url =
            Url::parse(&self.api).map_err(|error| api::bad_url("Gemini", Some(&error)))?;
        url.path_segments_mut()
            .map_err(|()| api::bad_url("Gemini", None))?
            .extend([
                "v1beta",
                "models",
                &format!("{}:generateContent", self.model),
            ]);
        let http = self
            .request(Method::Post, url.to_string())
            .json(self.body(request))
            .timeout(MODEL_TIMEOUT);
        let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
        read_generation(&api::parse_body(KIND, &body)?, &self.model, &request.schema)
    }

    fn models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let mut models = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = Url::parse(&format!("{}/v1beta/models", self.api))
                .map_err(|error| api::bad_url("Gemini", Some(&error)))?;
            url.query_pairs_mut().append_pair("pageSize", "1000");
            if let Some(token) = &token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let http = self.request(Method::Get, url.to_string());
            let body = api::send(self.transport.as_ref(), KIND, &http, self.key.expose())?;
            let page = api::parse_body(KIND, &body)?;
            for entry in page["models"].as_array().into_iter().flatten() {
                let generates =
                    entry["supportedGenerationMethods"]
                        .as_array()
                        .is_some_and(|methods| {
                            methods.iter().any(|method| method == "generateContent")
                        });
                let Some(id) = entry["name"]
                    .as_str()
                    .map(|name| name.trim_start_matches("models/"))
                else {
                    continue;
                };
                if generates {
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name: entry["displayName"].as_str().unwrap_or(id).to_string(),
                    });
                }
            }
            match page["nextPageToken"]
                .as_str()
                .filter(|next| !next.is_empty())
            {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        Ok(models)
    }
}

/// Reads a `generateContent` answer: a blocked prompt, then how the first
/// candidate finished, then its text (thought parts left out).
pub fn read_generation(
    response: &Value,
    asked: &str,
    schema: &Value,
) -> Result<ModelAnswer, ModelError> {
    if let Some(reason) = response["promptFeedback"]["blockReason"].as_str() {
        return Err(ModelError::Refused {
            provider: KIND,
            category: Some(reason.to_ascii_lowercase()),
            explanation: response["promptFeedback"]["blockReasonMessage"]
                .as_str()
                .map(str::to_string),
        });
    }
    let candidate = &response["candidates"][0];
    if candidate.is_null() {
        return Err(ModelError::InvalidOutput {
            provider: KIND,
            detail: core_text!(NoAnswerReturned, "it returned no answer"),
        });
    }
    let explanation = candidate["finishMessage"].as_str().map(str::to_string);
    match candidate["finishReason"].as_str() {
        None | Some("STOP") | Some("FINISH_REASON_UNSPECIFIED") => {}
        Some("MAX_TOKENS") => {
            return Err(ModelError::Truncated {
                provider: KIND,
                reason: core_text!(
                    OutputLimitWithThinking,
                    "it reached the limit of {tokens} output tokens, thinking included",
                    tokens = REASONING_OUTPUT_TOKENS
                ),
            })
        }
        Some(reason) if DECLINED.contains(&reason) => {
            return Err(ModelError::Refused {
                provider: KIND,
                category: Some(reason.to_ascii_lowercase()),
                explanation,
            })
        }
        Some(reason) => {
            return Err(ModelError::InvalidOutput {
                provider: KIND,
                detail: core_text!(StoppedWith, "it stopped with {reason}", reason = reason),
            })
        }
    }
    let text: String = candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["thought"] != true)
        .filter_map(|part| part["text"].as_str())
        .collect();
    let value = api::parse_answer(KIND, &text, schema)?;
    let usage = &response["usageMetadata"];
    Ok(ModelAnswer {
        value,
        provider: KIND,
        model: response["modelVersion"]
            .as_str()
            .unwrap_or(asked)
            .to_string(),
        usage: Some(Usage {
            input_tokens: usage["promptTokenCount"].as_u64(),
            output_tokens: api::sum(&[
                usage["candidatesTokenCount"].as_u64(),
                usage["thoughtsTokenCount"].as_u64(),
            ]),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::tests::{request, schema};
    use crate::http::tests::CannedTransport;

    fn client(transport: &Arc<CannedTransport>) -> Gemini {
        Gemini::new(
            "https://gemini.test",
            Secret::new("AIza-secret"),
            "models/gemini-3-pro",
            Arc::clone(transport) as Arc<dyn Transport>,
        )
    }

    fn generation(finish_reason: &str, parts: Value) -> Value {
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": parts },
                "finishReason": finish_reason,
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 50,
                "candidatesTokenCount": 12,
                "thoughtsTokenCount": 200,
                "totalTokenCount": 262
            },
            "modelVersion": "gemini-3-pro",
            "responseId": "r1"
        })
    }

    #[test]
    fn asks_for_json_matching_the_schema_and_reads_the_answer() {
        let transport = CannedTransport::answering(vec![(
            200,
            generation(
                "STOP",
                json!([
                    { "text": "Thinking about it", "thought": true },
                    { "text": "{\"ok\":true,\"reply\":\"Hi\"}" }
                ]),
            ),
        )]);
        let answer = client(&transport)
            .ask(&request(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(answer.value, json!({"ok": true, "reply": "Hi"}));
        assert_eq!(answer.model, "gemini-3-pro");
        assert_eq!(
            answer.usage,
            Some(Usage {
                input_tokens: Some(50),
                output_tokens: Some(212)
            })
        );

        let sent = &transport.sent()[0];
        assert_eq!(
            sent.url,
            "https://gemini.test/v1beta/models/gemini-3-pro:generateContent"
        );
        assert_eq!(
            sent.auth,
            Auth::Header {
                name: "x-goog-api-key",
                value: Secret::new("AIza-secret")
            }
        );
        assert_eq!(
            sent.body,
            Some(json!({
                "systemInstruction": { "parts": [{ "text": "You check that a connection works." }] },
                "contents": [{ "role": "user", "parts": [{ "text": "Say hello." }] }],
                "generationConfig": {
                    "maxOutputTokens": 32000,
                    "responseMimeType": "application/json",
                    "responseJsonSchema": schema()
                }
            }))
        );
    }

    #[test]
    fn a_block_a_safety_stop_and_a_cut_off_answer_are_distinct_errors() {
        let blocked = json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" } });
        assert_eq!(
            read_generation(&blocked, "gemini-3-pro", &schema()),
            Err(ModelError::Refused {
                provider: KIND,
                category: Some("prohibited_content".to_string()),
                explanation: None
            })
        );
        assert!(matches!(
            read_generation(&generation("SAFETY", json!([])), "gemini-3-pro", &schema()),
            Err(ModelError::Refused { .. })
        ));
        assert!(matches!(
            read_generation(
                &generation("MAX_TOKENS", json!([{ "text": "{\"ok\"" }])),
                "gemini-3-pro",
                &schema()
            ),
            Err(ModelError::Truncated { .. })
        ));
        assert!(matches!(
            read_generation(
                &generation("MALFORMED_RESPONSE", json!([])),
                "gemini-3-pro",
                &schema()
            ),
            Err(ModelError::InvalidOutput { .. })
        ));
        assert!(matches!(
            read_generation(&json!({ "candidates": [] }), "gemini-3-pro", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
    }

    #[test]
    fn lists_the_models_that_generate_content_across_pages() {
        let transport = CannedTransport::answering(vec![
            (
                200,
                json!({
                    "models": [
                        {
                            "name": "models/gemini-3-pro",
                            "displayName": "Gemini 3 Pro",
                            "supportedGenerationMethods": ["generateContent", "countTokens"]
                        },
                        {
                            "name": "models/text-embedding-005",
                            "displayName": "Text Embedding",
                            "supportedGenerationMethods": ["embedContent"]
                        }
                    ],
                    "nextPageToken": "page 2"
                }),
            ),
            (
                200,
                json!({
                    "models": [{
                        "name": "models/gemini-3-flash",
                        "displayName": "Gemini 3 Flash",
                        "supportedGenerationMethods": ["generateContent"]
                    }]
                }),
            ),
        ]);
        let models = client(&transport).models().unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "gemini-3-pro".to_string(),
                    name: "Gemini 3 Pro".to_string()
                },
                ModelInfo {
                    id: "gemini-3-flash".to_string(),
                    name: "Gemini 3 Flash".to_string()
                },
            ]
        );
        let sent = transport.sent();
        assert_eq!(
            sent[0].url,
            "https://gemini.test/v1beta/models?pageSize=1000"
        );
        assert_eq!(
            sent[1].url,
            "https://gemini.test/v1beta/models?pageSize=1000&pageToken=page+2"
        );
    }
}
