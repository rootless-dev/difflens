//! Jira Cloud: the card a review is about, read so the reviewer (and, on a
//! click on Summarize, the model) can hold the change against what was asked.
//!
//! The card's key comes from the branch name, else from the pull request's
//! title (`branches::card`). Jira sits behind `tracker::IssueTracker`; only
//! Jira Cloud is implemented (`cloud`), with an Atlassian API token, classic
//! or with scopes. Its rich text (Atlassian Document Format) is read as plain
//! text (`adf`), never as HTML. DiffLens only reads from Jira.

mod adf;
mod cloud;
mod service;
mod tracker;

pub use service::JiraService;
