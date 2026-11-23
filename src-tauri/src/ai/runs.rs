//! Model calls in progress, each under a run id the frontend chose, so it
//! can stop one. A call runs on a thread of its own and the caller waits for
//! it; cancelling raises the call's flag, which stops a `claude` child
//! process, and returns at once. An HTTP request cannot be taken back once
//! sent: its answer is dropped when it arrives.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::provider::ModelError;
use crate::error::app_error;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Default)]
pub struct ModelRuns {
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Takes a run off the list when it ends, however it ends.
struct Registered<'a> {
    runs: &'a ModelRuns,
    id: String,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.runs.running.lock() {
            running.remove(&self.id);
        }
    }
}

impl ModelRuns {
    /// Runs `work` under `run_id` and waits for it, or for a cancel.
    pub fn run<T: Send + 'static>(
        &self,
        run_id: &str,
        work: impl FnOnce(&AtomicBool) -> Result<T, ModelError> + Send + 'static,
    ) -> Result<T, ModelError> {
        self.run_flagged(run_id, Arc::new(AtomicBool::new(false)), work)
    }

    /// As `run`, stopped by `cancel` too: a flag several runs share, so one
    /// raise stops them all, even one not registered yet.
    pub fn run_flagged<T: Send + 'static>(
        &self,
        run_id: &str,
        cancel: Arc<AtomicBool>,
        work: impl FnOnce(&AtomicBool) -> Result<T, ModelError> + Send + 'static,
    ) -> Result<T, ModelError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Err(ModelError::Internal(app_error!(
                Internal,
                RunIdMissing,
                "A model run needs an id."
            )));
        }
        {
            let mut running = self.lock()?;
            if running.contains_key(run_id) {
                return Err(ModelError::Internal(app_error!(
                    Internal,
                    RunAlreadyGoing,
                    "A model run with the id {id} is already going.",
                    id = run_id
                )));
            }
            running.insert(run_id.to_string(), Arc::clone(&cancel));
        }
        let _registered = Registered {
            runs: self,
            id: run_id.to_string(),
        };

        let (sender, receiver) = mpsc::channel();
        let flag = Arc::clone(&cancel);
        thread::spawn(move || {
            let _ = sender.send(work(&flag));
        });
        loop {
            match receiver.recv_timeout(POLL_INTERVAL) {
                Ok(_) if cancel.load(Ordering::Relaxed) => return Err(ModelError::Cancelled),
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => {
                    return Err(ModelError::Cancelled)
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ModelError::Internal(app_error!(
                        Internal,
                        ModelCallAbandoned,
                        "The model call stopped unexpectedly."
                    )))
                }
            }
        }
    }

    /// Stops the run `run_id`. Nothing happens when it already ended.
    pub fn cancel(&self, run_id: &str) {
        if let Ok(running) = self.running.lock() {
            if let Some(flag) = running.get(run_id.trim()) {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>>, ModelError> {
        self.running
            .lock()
            .map_err(|_| {
                ModelError::Internal(app_error!(
                    Internal,
                    RunListUnusable,
                    "The list of model runs is unusable."
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn returns_what_the_work_returns_and_forgets_the_run() {
        let runs = ModelRuns::default();
        assert_eq!(runs.run("r1", |_| Ok(42)), Ok(42));
        assert_eq!(
            runs.run("r1", |_| Err::<(), _>(ModelError::CliMissing)),
            Err(ModelError::CliMissing)
        );
        assert!(runs.running.lock().unwrap().is_empty());
    }

    #[test]
    fn a_cancel_returns_at_once_and_raises_the_works_flag() {
        let runs = Arc::new(ModelRuns::default());
        let saw_cancel = Arc::new(AtomicBool::new(false));
        let canceller = {
            let runs = Arc::clone(&runs);
            thread::spawn(move || {
                // Until the run is registered, a cancel has nothing to stop.
                while runs.running.lock().unwrap().is_empty() {
                    thread::sleep(Duration::from_millis(5));
                }
                runs.cancel("slow");
            })
        };
        let started = Instant::now();
        let seen = Arc::clone(&saw_cancel);
        let result = runs.run("slow", move |cancel| {
            // Stands in for a call that never answers on its own.
            while !cancel.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            seen.store(true, Ordering::Relaxed);
            Ok(())
        });
        canceller.join().unwrap();
        assert_eq!(result, Err(ModelError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        // The work saw the flag (a CLI run would stop its child here).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !saw_cancel.load(Ordering::Relaxed) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(saw_cancel.load(Ordering::Relaxed));
        assert!(runs.running.lock().unwrap().is_empty());
    }

    #[test]
    fn a_run_id_is_used_by_one_run_at_a_time_and_unknown_cancels_do_nothing() {
        let runs = Arc::new(ModelRuns::default());
        runs.cancel("nothing");
        let second = {
            let runs = Arc::clone(&runs);
            move || runs.run("same", |_| Ok(()))
        };
        let result = runs.run("same", move |_| Ok(second()));
        assert!(matches!(result, Ok(Err(ModelError::Internal(_)))));
        assert!(matches!(
            runs.run("  ", |_| Ok(())),
            Err(ModelError::Internal(_))
        ));
    }
}
