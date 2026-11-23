//! Pull requests on the project's remote: finding the one a review is
//! about, bringing its comments into the review, and publishing the review's
//! comments to it when the user asks.
//!
//! Every host sits behind `host::PullRequestHost`: Bitbucket Cloud and GitHub
//! are implemented. Reading happens when a review opens and on Reload;
//! nothing is ever sent to a host without a click: "Publish comments", or
//! Resolve and Reopen on a thread that is already on the pull request.

mod bitbucket;
mod github;
mod host;
mod publish;
mod remote;
mod service;
mod sync;

pub use publish::plan;
pub use service::PullRequestService;
