//! The boundary every model provider sits behind. A provider is asked one
//! question (a system prompt, a user prompt and the JSON Schema the answer
//! must match) and gives back the parsed answer, which model wrote it and the
//! tokens it reports. Nothing outside a provider's own module knows its URLs,
//! JSON or quirks.

use std::fmt;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::error::{app_error, core_text, AppError, CoreText, ErrorCode};
use crate::model::{ModelInfo, ModelProviderKind};

impl ModelProviderKind {
    pub const ALL: [ModelProviderKind; 5] = [
        ModelProviderKind::ClaudeCli,
        ModelProviderKind::Anthropic,
        ModelProviderKind::OpenAi,
        ModelProviderKind::Gemini,
        ModelProviderKind::OpenRouter,
    ];

    /// Stable key the provider's settings and Keychain entry are saved under.
    pub fn key(self) -> &'static str {
        match self {
            Self::ClaudeCli => "claude_cli",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCli => "Claude Code",
            Self::Anthropic => "Anthropic API",
            Self::OpenAi => "OpenAI API",
            Self::Gemini => "Google Gemini API",
            Self::OpenRouter => "OpenRouter",
        }
    }

    /// Claude Code uses the sign-in it already has; the APIs need a key.
    pub fn needs_key(self) -> bool {
        self != Self::ClaudeCli
    }

    /// How many questions of one piece of work may be asked at once: Claude
    /// Code runs one process per question on this machine, so one at a time;
    /// the APIs take a few side by side.
    pub fn parallel_calls(self) -> u32 {
        match self {
            Self::ClaudeCli => 1,
            _ => 3,
        }
    }

    /// Where the user creates an API key.
    pub fn key_url(self) -> &'static str {
        match self {
            Self::ClaudeCli => "",
            Self::Anthropic => "https://platform.claude.com/settings/keys",
            Self::OpenAi => "https://platform.openai.com/api-keys",
            Self::Gemini => "https://aistudio.google.com/apikey",
            Self::OpenRouter => "https://openrouter.ai/settings/keys",
        }
    }
}

/// One question for a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub system: String,
    /// Can be large (a diff); providers send it in the request body or on
    /// standard input, never as a command-line argument.
    pub prompt: String,
    /// JSON Schema of the answer. It must be an object schema in the subset
    /// every provider's strict mode accepts: `additionalProperties: false`
    /// on every object, every property listed in `required` (optional fields
    /// as a union with `null`), no `oneOf`, `not`, recursion, or numeric and
    /// string length constraints.
    pub schema: Value,
    /// Names the schema where a provider wants a name, such as
    /// "review_findings": letters, digits, `_` and `-` only.
    pub schema_name: String,
}

/// Tokens a provider reports having read and written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Every token read, cached ones included.
    pub input_tokens: Option<u64>,
    /// Every token written, thinking included.
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelAnswer {
    /// The answer, parsed; it matches the request's schema.
    pub value: Value,
    pub provider: ModelProviderKind,
    /// The model that answered, as the provider names it. It can differ from
    /// the one asked for, such as after a refusal fallback.
    pub model: String,
    pub usage: Option<Usage>,
}

/// Why a model gave no usable answer. Each case maps to an error code of the
/// frontend contract; messages never include a key. Texts the core words
/// are `CoreText`s; the provider's own words stay as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// No key, no model, or no provider chosen; the error says what to do.
    NotConfigured(AppError),
    AuthFailed {
        provider: ModelProviderKind,
        detail: Option<String>,
    },
    RateLimited {
        provider: ModelProviderKind,
        detail: Option<String>,
        retry_after: Option<String>,
    },
    Refused {
        provider: ModelProviderKind,
        /// The policy area, such as "cyber", when the provider names one.
        category: Option<String>,
        explanation: Option<String>,
    },
    /// The answer was cut off; `reason` says by what.
    Truncated {
        provider: ModelProviderKind,
        reason: CoreText,
    },
    InvalidOutput {
        provider: ModelProviderKind,
        detail: CoreText,
    },
    /// The provider could not be reached at all.
    Network {
        provider: ModelProviderKind,
        reason: CoreText,
    },
    /// The provider answered with an error of its own.
    ProviderFailed {
        provider: ModelProviderKind,
        status: Option<u16>,
        message: CoreText,
    },
    CliMissing,
    CliFailed(CoreText),
    Cancelled,
    Internal(AppError),
}

/// How a sentence names a provider: "the Anthropic API", "OpenRouter", or
/// with a capital at the start of a sentence. Only the
/// article is worded; the provider's name stays as it is.
pub fn provider_phrase(provider: ModelProviderKind, capital: bool) -> CoreText {
    let name = provider.name();
    if !name.ends_with(" API") {
        return CoreText::raw(name);
    }
    if capital {
        core_text!(ProviderTheApiCapitalized, "The {name}", name = name)
    } else {
        core_text!(ProviderTheApi, "the {name}", name = name)
    }
}

/// What a provider said, ending with a full stop so it reads as a sentence.
fn said(text: &str) -> CoreText {
    CoreText::raw(text).sentence()
}

impl ModelError {
    /// The error of the frontend contract this stands for, worded by its
    /// reason.
    fn to_error(&self) -> AppError {
        let phrase = provider_phrase;
        match self {
            Self::NotConfigured(error) => AppError {
                code: ErrorCode::ModelNotConfigured,
                ..error.clone()
            },
            Self::AuthFailed {
                provider: ModelProviderKind::ClaudeCli,
                detail,
            } => match detail {
                Some(detail) => app_error!(
                    AuthFailed,
                    ClaudeCodeSignedOutSaid,
                    "Claude Code is not signed in, or its sign-in expired. Run `claude` in a \
                     terminal and sign in. It said: {detail}",
                    detail = said(detail)
                ),
                None => app_error!(
                    AuthFailed,
                    ClaudeCodeSignedOut,
                    "Claude Code is not signed in, or its sign-in expired. Run `claude` in a \
                     terminal and sign in."
                ),
            },
            Self::AuthFailed { provider, detail } => match detail {
                Some(detail) => app_error!(
                    AuthFailed,
                    ModelKeyRefusedSaid,
                    "{provider} did not accept the API key. It may be wrong, revoked or lack \
                     permission. It said: {detail}",
                    provider = phrase(*provider, true),
                    detail = said(detail)
                ),
                None => app_error!(
                    AuthFailed,
                    ModelKeyRefused,
                    "{provider} did not accept the API key. It may be wrong, revoked or lack \
                     permission.",
                    provider = phrase(*provider, true)
                ),
            },
            Self::RateLimited {
                provider,
                detail,
                retry_after,
            } => {
                let provider = phrase(*provider, true);
                match (retry_after, detail) {
                    (Some(after), Some(detail)) => app_error!(
                        RateLimited,
                        ModelRateLimitedRetryAfterSaid,
                        "{provider} is limiting requests; try again in {seconds} seconds. It \
                         said: {detail}",
                        provider = provider,
                        seconds = after,
                        detail = said(detail)
                    ),
                    (Some(after), None) => app_error!(
                        RateLimited,
                        ModelRateLimitedRetryAfter,
                        "{provider} is limiting requests; try again in {seconds} seconds.",
                        provider = provider,
                        seconds = after
                    ),
                    (None, Some(detail)) => app_error!(
                        RateLimited,
                        ModelRateLimitedSaid,
                        "{provider} is limiting requests; try again in a moment. It said: {detail}",
                        provider = provider,
                        detail = said(detail)
                    ),
                    (None, None) => app_error!(
                        RateLimited,
                        ModelRateLimited,
                        "{provider} is limiting requests; try again in a moment.",
                        provider = provider
                    ),
                }
            }
            Self::Refused {
                provider,
                category,
                explanation,
            } => {
                let provider = phrase(*provider, false);
                match (category, explanation) {
                    (Some(category), Some(explanation)) => app_error!(
                        ModelRefused,
                        ModelRefusedCategoryExplained,
                        "The model on {provider} declined to answer ({category}). {detail}",
                        provider = provider,
                        category = category,
                        detail = said(explanation)
                    ),
                    (Some(category), None) => app_error!(
                        ModelRefused,
                        ModelRefusedCategory,
                        "The model on {provider} declined to answer ({category}).",
                        provider = provider,
                        category = category
                    ),
                    (None, Some(explanation)) => app_error!(
                        ModelRefused,
                        ModelRefusedExplained,
                        "The model on {provider} declined to answer. {detail}",
                        provider = provider,
                        detail = said(explanation)
                    ),
                    (None, None) => app_error!(
                        ModelRefused,
                        ModelRefusedQuietly,
                        "The model on {provider} declined to answer.",
                        provider = provider
                    ),
                }
            }
            Self::Truncated { provider, reason } => app_error!(
                ModelTruncated,
                ModelTruncated,
                "The answer from {provider} was cut off before it was complete: {detail}",
                provider = phrase(*provider, false),
                detail = reason.clone().sentence()
            ),
            Self::InvalidOutput { provider, detail } => app_error!(
                ModelInvalidOutput,
                ModelInvalidOutput,
                "{provider} answered, but not with the JSON that was asked for: {detail}",
                provider = phrase(*provider, true),
                detail = detail.clone().sentence()
            ),
            Self::Network { provider, reason } => app_error!(
                NetworkFailed,
                ModelUnreachable,
                "Could not reach {provider}: {detail}",
                provider = phrase(*provider, false),
                detail = reason.clone().sentence()
            ),
            Self::ProviderFailed {
                provider,
                status: Some(status),
                message,
            } => app_error!(
                RemoteFailed,
                ModelProviderRefusedStatus,
                "{provider} refused the request (HTTP {status}): {detail}",
                provider = phrase(*provider, true),
                status = *status,
                detail = message.clone().sentence()
            ),
            Self::ProviderFailed {
                provider,
                status: None,
                message,
            } => app_error!(
                RemoteFailed,
                ModelProviderRefused,
                "{provider} refused the request: {detail}",
                provider = phrase(*provider, true),
                detail = message.clone().sentence()
            ),
            Self::CliMissing => app_error!(
                ClaudeCliNotFound,
                ClaudeCliMissing,
                "The claude command of Claude Code was not found. Install Claude Code, or \
                 choose another provider."
            ),
            Self::CliFailed(reason) => app_error!(
                ClaudeCliFailed,
                ClaudeCodeFailed,
                "Claude Code failed: {detail}",
                detail = reason.clone().sentence()
            ),
            Self::Cancelled => app_error!(Cancelled, Stopped, "Stopped."),
            Self::Internal(error) => AppError {
                code: ErrorCode::Internal,
                ..error.clone()
            },
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_error().message)
    }
}

impl std::error::Error for ModelError {}

impl From<ModelError> for AppError {
    fn from(error: ModelError) -> Self {
        error.to_error()
    }
}

/// One provider, configured with a key and a model. Calls block; they run on
/// the commands' blocking threads.
pub trait ModelProvider: Send + Sync {
    /// Asks the configured model. A provider that can stop early (a child
    /// process) stops when `cancel` is raised; for the others the caller
    /// stops waiting (see `runs`).
    fn ask(&self, request: &ModelRequest, cancel: &AtomicBool) -> Result<ModelAnswer, ModelError>;

    /// The models this key can ask for a structured answer, in the
    /// provider's order.
    fn models(&self) -> Result<Vec<ModelInfo>, ModelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_round_trip_and_match_the_contract() {
        for kind in ModelProviderKind::ALL {
            assert_eq!(ModelProviderKind::from_key(kind.key()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.key());
        }
        assert_eq!(ModelProviderKind::from_key("nope"), None);
    }

    #[test]
    fn every_error_reads_as_what_went_wrong_with_its_code() {
        let refused = AppError::from(ModelError::Refused {
            provider: ModelProviderKind::Anthropic,
            category: Some("cyber".to_string()),
            explanation: Some("This request could enable cyber harm".to_string()),
        });
        assert_eq!(refused.code, ErrorCode::ModelRefused);
        assert_eq!(
            refused.message,
            "The model on the Anthropic API declined to answer (cyber). This request could enable cyber harm."
        );

        let limited = AppError::from(ModelError::RateLimited {
            provider: ModelProviderKind::OpenAi,
            detail: None,
            retry_after: Some("20".to_string()),
        });
        assert_eq!(limited.code, ErrorCode::RateLimited);
        assert_eq!(
            limited.message,
            "The OpenAI API is limiting requests; try again in 20 seconds."
        );

        let cli = AppError::from(ModelError::AuthFailed {
            provider: ModelProviderKind::ClaudeCli,
            detail: None,
        });
        assert!(cli.message.contains("Run `claude` in a terminal"));
        assert_eq!(
            AppError::from(ModelError::CliMissing).code,
            ErrorCode::ClaudeCliNotFound
        );
        assert_eq!(
            AppError::from(ModelError::Cancelled).code,
            ErrorCode::Cancelled
        );
    }

    #[test]
    fn errors_carry_their_reason_the_provider_and_what_it_said() {
        use crate::error::{Param, Reason};

        let unreachable = AppError::from(ModelError::Network {
            provider: ModelProviderKind::Gemini,
            reason: core_text!(TransportTimeout, "it did not answer in time"),
        });
        assert_eq!(unreachable.reason, Some(Reason::ModelUnreachable));
        assert_eq!(
            unreachable.message,
            "Could not reach the Google Gemini API: it did not answer in time."
        );
        let Some(Param::Message(provider)) = unreachable.params.get("provider") else {
            panic!("{unreachable:?}");
        };
        assert_eq!(provider.reason, Some(Reason::ProviderTheApi));
        assert_eq!(
            provider.params.get("name"),
            Some(&Param::Text("Google Gemini API".to_string()))
        );
        // The core's own cause is worded too, with the full stop it gains.
        let Some(Param::Message(cause)) = unreachable.params.get("detail") else {
            panic!("{unreachable:?}");
        };
        assert_eq!(cause.reason, Some(Reason::Sentence));
        assert_eq!(unreachable.detail, None);

        // A provider without " API" in its name is named as it is.
        let refused = AppError::from(ModelError::ProviderFailed {
            provider: ModelProviderKind::OpenRouter,
            status: Some(402),
            message: CoreText::raw("Insufficient credits"),
        });
        assert_eq!(refused.reason, Some(Reason::ModelProviderRefusedStatus));
        assert_eq!(
            refused.params.get("provider"),
            Some(&Param::Text("OpenRouter".to_string()))
        );
        assert_eq!(refused.params.get("status"), Some(&Param::Number(402)));
        // What the provider said is the detail, as received plus a full stop.
        assert_eq!(refused.detail.as_deref(), Some("Insufficient credits."));
        assert_eq!(
            refused.message,
            "OpenRouter refused the request (HTTP 402): Insufficient credits."
        );
    }

    /// Every case, with and without its optional parts, reads as the
    /// English catalog words it (each conversion checks itself in tests).
    #[test]
    fn every_case_reads_as_the_catalog_words_it() {
        let text = |raw: bool| {
            if raw {
                CoreText::raw("it said no")
            } else {
                core_text!(TransportTimeout, "it did not answer in time")
            }
        };
        let some = |on: bool| on.then(|| "Be careful".to_string());
        for provider in ModelProviderKind::ALL {
            for on in [false, true] {
                let cases = [
                    ModelError::AuthFailed {
                        provider,
                        detail: some(on),
                    },
                    ModelError::RateLimited {
                        provider,
                        detail: some(on),
                        retry_after: None,
                    },
                    ModelError::RateLimited {
                        provider,
                        detail: some(!on),
                        retry_after: Some("5".to_string()),
                    },
                    ModelError::Refused {
                        provider,
                        category: some(on),
                        explanation: None,
                    },
                    ModelError::Refused {
                        provider,
                        category: some(!on),
                        explanation: Some("Not this".to_string()),
                    },
                    ModelError::Truncated {
                        provider,
                        reason: text(on),
                    },
                    ModelError::InvalidOutput {
                        provider,
                        detail: text(on),
                    },
                    ModelError::Network {
                        provider,
                        reason: text(on),
                    },
                    ModelError::ProviderFailed {
                        provider,
                        status: on.then_some(500),
                        message: text(on),
                    },
                    ModelError::CliFailed(text(on)),
                ];
                for case in cases {
                    let error = AppError::from(case);
                    assert!(error.reason.is_some(), "{error:?}");
                    assert_eq!(error.message, error.clone().verified().message);
                }
            }
        }
    }
}
