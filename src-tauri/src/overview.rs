//! The Overview tab's own data: the summary a model writes of a change (what
//! it does, what its Jira card asks, and the card's acceptance criteria
//! judged against it), and what can be told of the change without a model
//! (its shape, and what a model reading it would not see).
//!
//! Summarize asks one call: the card and the pull request, marked as
//! untrusted data, and the change, whole when it fits a call (`compose`),
//! else condensed. The answer is checked against the change (`summary`) and
//! kept per review with the head it read. The summary reads the production
//! code only: test files are left out of the call. Nothing here asks a model
//! on its own: only a click on Summarize (or on a full analysis) does.

mod compose;
mod service;
mod shape;
mod summary;

pub use service::{clip, pull_request_block, untrusted, OverviewService};
