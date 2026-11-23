//! Explanations of functions on the Flow tab, written by a model, at two
//! levels. The short one (what a full analysis and the Explain buttons
//! write): one or two sentences on what changed and what the function is
//! for, a reason only when the card, the pull request or a commit gives one;
//! for a function of the context only what it is for. The detailed one (on
//! Explain in detail, one function at a time): what changed, why (from the
//! card, the pull request and the commit messages, or said to be an
//! inference) and what it is for.
//!
//! A call sends the change's context as untrusted data and each function
//! with its callers, callees and code (`inputs`), batched by file within the
//! per-call budget, and reads back an answer keyed by function id. Each
//! explanation is kept per review, function and level with a fingerprint of
//! the function's lines and its language, so a later look tells when the
//! code changed since (outdated) and the Translate flow can add a version in
//! another language.
//!
//! Nothing here asks a model on its own: only a click on a node's or a
//! group's Explain button, on Explain in detail, the confirmation of a full
//! analysis, or Translate does.

mod inputs;
mod service;

pub use inputs::{Context, FileTexts, MAX_COMMITS};
#[cfg(test)]
pub use service::spans_of;
pub use service::{ensure_ready, file_texts, ExplanationService, Scope};

#[cfg(test)]
mod tests;
