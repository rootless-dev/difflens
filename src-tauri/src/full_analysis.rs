//! A full analysis of a review, run in the core after the user confirms it:
//! the tests in the sandbox, the flow and quality metrics, every call of an
//! Insights analysis, the Overview summary and the explanations of the
//! changed functions, all started at once (`steps`). Jobs are kept per
//! project and review while the app runs (`jobs`), at most as many at once
//! as Preferences allows, the others queued; each emits its progress and
//! log lines as Tauri events, so the user can leave the review and come
//! back to it. A step that fails does not stop the others; Cancel stops the
//! steps in flight and keeps what they already saved.

mod jobs;
mod steps;

pub use jobs::{clamp_limit, Events, JobRequest, Jobs, DEFAULT_LIMIT};
pub use steps::ServiceSteps;

use tauri::Emitter;

use crate::error::AppResult;
use crate::model::{FullAnalysisEvent, ModelTask};
use crate::store::Store;

/// The Tauri event every change of a full analysis is emitted as.
pub const EVENT: &str = "full-analysis";

/// The tasks a full analysis asks the model for, each of its own model: it
/// starts only when every one of them can be asked.
pub const TASKS: [ModelTask; 3] = [
    ModelTask::Insights,
    ModelTask::Summary,
    ModelTask::BriefExplanation,
];

/// The setting holding how many full analyses may run at once.
const LIMIT_SETTING: &str = "full_analysis.limit";

/// The limit the user chose, or the default.
pub fn saved_limit(store: &Store) -> AppResult<u32> {
    Ok(store
        .setting(LIMIT_SETTING)?
        .and_then(|value| value.parse::<u32>().ok())
        .map_or(DEFAULT_LIMIT, clamp_limit))
}

/// Keeps `limit` (within 1 and the most allowed) and returns it.
pub fn save_limit(store: &Store, limit: u32) -> AppResult<u32> {
    let limit = clamp_limit(limit);
    store.set_setting(LIMIT_SETTING, &limit.to_string())?;
    Ok(limit)
}

/// Emits a job's changes to the window.
pub struct TauriEvents(pub tauri::AppHandle);

impl Events for TauriEvents {
    fn emit(&self, event: &FullAnalysisEvent) {
        // A window that is gone has nothing to show it in.
        let _ = self.0.emit(EVENT, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_four_until_the_user_picks_another() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(saved_limit(&store).unwrap(), 4);
        assert_eq!(save_limit(&store, 2).unwrap(), 2);
        assert_eq!(saved_limit(&store).unwrap(), 2);
        assert_eq!(save_limit(&store, 0).unwrap(), 1);
        store.set_setting(LIMIT_SETTING, "many").unwrap();
        assert_eq!(saved_limit(&store).unwrap(), 4);
    }
}
