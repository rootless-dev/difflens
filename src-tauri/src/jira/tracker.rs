//! The boundary an issue tracker sits behind. Nothing outside its own module
//! knows a tracker's URLs or JSON: the rest of DiffLens speaks in the
//! tracker-neutral `Account` and `Card`.

use crate::error::AppResult;
use crate::model::{Account, Card};

/// One tracker as one connected account sees it. Calls block; they run on
/// the commands' blocking threads.
pub trait IssueTracker: Send + Sync {
    /// The account the credential belongs to.
    fn account(&self) -> AppResult<Account>;

    /// The card `key` with its comments, oldest first. `None` when the
    /// tracker has no such card or the account cannot see it.
    fn card(&self, key: &str) -> AppResult<Option<Card>>;
}
