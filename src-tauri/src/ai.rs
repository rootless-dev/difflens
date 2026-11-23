//! The model: a provider-neutral way to ask a model one question and get a
//! JSON answer matching a schema, and the settings that choose who answers.
//!
//! Every provider sits behind `provider::ModelProvider`: Claude Code on this
//! machine (`claude_cli`), and the Anthropic, OpenAI, Google Gemini and
//! OpenRouter APIs over raw HTTP. API keys live in the Keychain only.
//!
//! A model is asked only after a click: Test in Preferences › Model, and
//! Analyze (or Retry) on the Insights tab. Using it sends what the question
//! holds (for reviews, the code) to the chosen provider.

mod anthropic;
mod api;
mod calls;
mod claude_cli;
mod gemini;
mod openai;
mod openrouter;
mod provider;
mod runs;
mod service;

pub use calls::Calls;
#[cfg(test)]
pub use provider::{provider_phrase, ModelProvider};
pub use provider::{ModelAnswer, ModelError, ModelRequest};
pub use service::ModelService;
