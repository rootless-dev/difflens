//! Insights: defects a model reads in a review's change, for the reviewer
//! to confirm or dismiss.
//!
//! Analyze sends each changed file's diff against the review's old side,
//! with new-side line numbers to cite (`diff`, `plan`), in as few calls as
//! fit the input budget, to the model chosen in Preferences › Model through
//! `ModelService` only: nothing here knows a provider. The answer is checked
//! against the code it was shown (`findings`), and each finding is placed in
//! the code as the review shows it later, following its lines when they move
//! and telling when the code under a verdict changed (`locate`).
//!
//! A finding can also become a short review comment the same model writes
//! (`comment`), kept as a draft of the review.
//!
//! Nothing here asks a model on its own: a call is only asked when the
//! frontend runs it, after a click on Analyze, Retry or Write comment, or
//! when a full analysis the user confirmed runs every call in the core.

mod comment;
pub mod diff;
mod findings;
mod locate;
pub mod plan;
mod service;

pub use service::{left_out, CallEnd, InsightsService};
