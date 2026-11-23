//! What the HTTP providers share: sending a request and reading a failure,
//! and checking that the text a model wrote is the JSON it was asked for.

use std::time::Duration;

use serde_json::Value;

use super::provider::ModelError;
use crate::error::{app_error, core_text, CoreText, Param};
use crate::http::{HttpRequest, Transport};
use crate::model::ModelProviderKind;

/// A model can think for minutes before it answers.
pub const MODEL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Output limit of the providers whose limit also covers the model's
/// reasoning (OpenAI, Gemini, OpenRouter): room to think, then answer.
pub const REASONING_OUTPUT_TOKENS: u64 = 32_000;

/// Sends `request` and returns the body of a successful answer. `key` is
/// scrubbed from any text a failure carries.
pub fn send(
    transport: &dyn Transport,
    provider: ModelProviderKind,
    request: &HttpRequest,
    key: &str,
) -> Result<String, ModelError> {
    let response = transport
        .send(request)
        .map_err(|reason| ModelError::Network {
            provider,
            reason: scrub_text(reason, key),
        })?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let detail = error_message(&response.body).map(|message| scrub(&message, key));
    Err(match response.status {
        401 | 403 => ModelError::AuthFailed { provider, detail },
        429 => ModelError::RateLimited {
            provider,
            detail,
            retry_after: response.retry_after,
        },
        status => ModelError::ProviderFailed {
            provider,
            status: Some(status),
            message: match detail {
                Some(detail) => CoreText::raw(detail),
                None if status >= 500 => core_text!(
                    ProviderHadAProblem,
                    "it had a problem answering; try again in a moment"
                ),
                None => core_text!(ProviderGaveNoExplanation, "it gave no reason"),
            },
        },
    })
}

/// The `error.message` every provider puts in a failure, as an object or as
/// the first item of a list.
pub fn error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let value = match value {
        Value::Array(items) => items.into_iter().next()?,
        value => value,
    };
    let error = value.get("error")?;
    let message = match error {
        Value::String(message) => message.clone(),
        error => error.get("message")?.as_str()?.to_string(),
    };
    let message = message.trim();
    (!message.is_empty()).then(|| message.to_string())
}

fn scrub(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else {
        text.replace(key, "<redacted>")
    }
}

/// A provider's API address that does not make a URL (`provider` as in
/// "Bad Gemini URL").
pub fn bad_url(provider: &str, error: Option<&dyn std::fmt::Display>) -> ModelError {
    ModelError::Internal(match error {
        Some(error) => app_error!(
            Internal,
            ProviderUrlInvalid,
            "Bad {provider} URL: {detail}",
            provider = provider,
            detail = error.to_string()
        ),
        None => app_error!(
            Internal,
            ProviderUrlUnusable,
            "Bad {provider} URL",
            provider = provider
        ),
    })
}

/// `text` with `key` scrubbed from it and from the values that fill it.
fn scrub_text(mut text: CoreText, key: &str) -> CoreText {
    text.message = scrub(&text.message, key);
    for value in text.params.values_mut() {
        if let Param::Text(value) = value {
            *value = scrub(value, key);
        }
    }
    text
}

/// Parses a successful body, which is the provider's JSON.
pub fn parse_body(provider: ModelProviderKind, body: &str) -> Result<Value, ModelError> {
    serde_json::from_str(body).map_err(|error| ModelError::ProviderFailed {
        provider,
        status: None,
        message: core_text!(
            ProviderAnswerNotJson,
            "its answer was not JSON ({detail})",
            detail = error
        ),
    })
}

/// Parses what the model wrote as the answer and checks the outline of the
/// schema: an object with every required property. Providers enforce the
/// schema themselves; this catches the ones that only try.
pub fn parse_answer(
    provider: ModelProviderKind,
    text: &str,
    schema: &Value,
) -> Result<Value, ModelError> {
    let text = text.trim();
    // Some models wrap JSON in a Markdown fence even when told not to.
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(text);
    if text.is_empty() {
        return Err(ModelError::InvalidOutput {
            provider,
            detail: core_text!(AnswerEmpty, "the answer was empty"),
        });
    }
    let value: Value = serde_json::from_str(text).map_err(|error| ModelError::InvalidOutput {
        provider,
        detail: core_text!(
            AnswerNotJson,
            "it is not valid JSON ({detail})",
            detail = error
        ),
    })?;
    check_outline(provider, &value, schema)?;
    Ok(value)
}

/// An answer that is already parsed (Claude Code's `structured_output`).
pub fn check_outline(
    provider: ModelProviderKind,
    value: &Value,
    schema: &Value,
) -> Result<(), ModelError> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err(ModelError::InvalidOutput {
            provider,
            detail: core_text!(AnswerNotAnObject, "it is not a JSON object"),
        });
    };
    let missing: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| !object.contains_key(*name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ModelError::InvalidOutput {
            provider,
            detail: core_text!(
                AnswerLacksFields,
                "it lacks {fields}",
                fields = missing.join(", ")
            ),
        })
    }
}

/// Adds two token counts a provider reports separately, either of which can
/// be missing.
pub fn sum(counts: &[Option<u64>]) -> Option<u64> {
    counts
        .iter()
        .flatten()
        .copied()
        .reduce(|total, count| total + count)
}

#[cfg(test)]
pub mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::AppError;
    use crate::http::tests::CannedTransport;
    use crate::http::HttpResponse;
    use crate::http::{Auth, Method, Secret};

    /// The schema of the answers tests ask for.
    pub fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "reply": { "type": "string" }
            },
            "required": ["ok", "reply"],
            "additionalProperties": false
        })
    }

    pub fn request() -> super::super::provider::ModelRequest {
        super::super::provider::ModelRequest {
            system: "You check that a connection works.".to_string(),
            prompt: "Say hello.".to_string(),
            schema: schema(),
            schema_name: "connection_check".to_string(),
        }
    }

    fn post() -> HttpRequest {
        HttpRequest::new(
            Method::Post,
            "https://api.example.com/v1/x",
            Auth::Header {
                name: "x-api-key",
                value: Secret::new("sk-secret"),
            },
        )
    }

    #[test]
    fn failures_map_to_typed_errors_without_the_key() {
        let transport = CannedTransport::answering(vec![
            (
                401,
                json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key sk-secret"}}),
            ),
            (404, json!({"error":{"message":"model: nope"}})),
            (500, json!({})),
        ]);
        transport.push_raw(Ok(HttpResponse {
            status: 429,
            body: json!([{"error":{"code":429,"message":"Resource exhausted","status":"RESOURCE_EXHAUSTED"}}]).to_string(),
            retry_after: Some("30".to_string()),
            headers: Vec::new(),
        }));
        transport.push_raw(Err("the connection failed".to_string()));
        let provider = ModelProviderKind::Anthropic;
        let send = || send(transport.as_ref(), provider, &post(), "sk-secret").unwrap_err();

        assert_eq!(
            send(),
            ModelError::AuthFailed {
                provider,
                detail: Some("invalid x-api-key <redacted>".to_string())
            }
        );
        assert_eq!(
            send(),
            ModelError::ProviderFailed {
                provider,
                status: Some(404),
                message: CoreText::raw("model: nope")
            }
        );
        assert!(matches!(
            send(),
            ModelError::ProviderFailed {
                status: Some(500),
                ..
            }
        ));
        assert_eq!(
            send(),
            ModelError::RateLimited {
                provider,
                detail: Some("Resource exhausted".to_string()),
                retry_after: Some("30".to_string())
            }
        );
        assert!(matches!(send(), ModelError::Network { .. }));
    }

    #[test]
    fn an_answer_must_be_the_json_object_the_schema_outlines() {
        let provider = ModelProviderKind::OpenAi;
        assert_eq!(
            parse_answer(
                provider,
                "```json\n{\"ok\":true,\"reply\":\"hi\"}\n```",
                &schema()
            )
            .unwrap(),
            json!({"ok": true, "reply": "hi"})
        );
        assert!(matches!(
            parse_answer(provider, "Sure! Here it is", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
        let lacking = parse_answer(provider, "{\"ok\":true}", &schema()).unwrap_err();
        let ModelError::InvalidOutput { detail, .. } = &lacking else {
            panic!("{lacking:?}");
        };
        assert_eq!(detail.message, "it lacks reply");
        assert_eq!(detail.reason, Some(crate::error::Reason::AnswerLacksFields));
        assert_eq!(
            AppError::from(lacking).message,
            "The OpenAI API answered, but not with the JSON that was asked for: it lacks reply."
        );
        assert!(matches!(
            parse_answer(provider, "  ", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
        assert!(matches!(
            parse_answer(provider, "[1]", &schema()),
            Err(ModelError::InvalidOutput { .. })
        ));
    }

    #[test]
    fn sums_what_is_reported() {
        assert_eq!(sum(&[Some(3), None, Some(4)]), Some(7));
        assert_eq!(sum(&[None, None]), None);
    }
}
