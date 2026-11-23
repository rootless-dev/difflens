//! Explanations of failures being written, kept by the core rather than by
//! the screen that asked: leaving a review and coming back finds them still
//! going (`running`), waits for them (`join`) and stops them (`cancel`).
//!
//! One failure of a review has at most one call at a time: asking again
//! while one goes on, from any screen, joins it instead of asking the model
//! twice. Calls take turns, as many asking at once as the model takes
//! (`parallel_calls`, one for Claude Code); the others wait, queued, until
//! one ends or they are stopped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::inputs::Subject;
use crate::ai::ModelError;
use crate::error::{app_error, AppResult};
use crate::model::{ReviewTarget, RunningDiagnosis, TestDiagnosis};

/// How often a queued call looks again whether it was stopped.
const QUEUE_POLL: Duration = Duration::from_millis(100);

/// Which call: a failure of a review of a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallKey {
    project: String,
    target: ReviewTarget,
    /// The failure's key (see `Subject::key`).
    failure: String,
}

impl CallKey {
    pub fn new(project_id: &str, target: &ReviewTarget, failure: &str) -> Self {
        Self {
            project: project_id.to_string(),
            target: target.clone(),
            failure: failure.to_string(),
        }
    }
}

type Outcome = AppResult<TestDiagnosis>;

struct Call {
    subject: Subject,
    /// Raised by `cancel`: stops the model call, or the wait for a turn.
    cancel: Arc<AtomicBool>,
    /// Waiting for a turn, not asking the model yet.
    queued: AtomicBool,
    result: Mutex<Option<Outcome>>,
    done: Condvar,
}

impl Call {
    fn wait(&self) -> Outcome {
        let mut result = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = result.as_ref() {
                return outcome.clone();
            }
            result = self.done.wait(result).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

#[derive(Default)]
pub struct DiagnosisCalls {
    calls: Mutex<HashMap<CallKey, Arc<Call>>>,
    /// How many calls ask the model now.
    asking: Mutex<usize>,
    /// Signalled when a turn frees up or a queued call is stopped.
    turn: Condvar,
}

impl DiagnosisCalls {
    /// Runs `work` for the failure `key` once a turn is free, as many at
    /// once as `parallel_calls` says (asked only when no call goes on), with
    /// the flag `cancel` raises; or, when a call for it already goes on,
    /// waits for that one and returns what it returns. `work` keeps what it
    /// writes before returning, so a call is never listed by `running` once
    /// what it wrote can be read.
    pub fn explain(
        &self,
        key: CallKey,
        subject: &Subject,
        parallel_calls: impl FnOnce() -> u32,
        work: impl FnOnce(Arc<AtomicBool>) -> Outcome,
    ) -> Outcome {
        let (call, leader) = {
            let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
            match calls.get(&key) {
                Some(call) => (Arc::clone(call), false),
                None => {
                    let call = Arc::new(Call {
                        subject: subject.clone(),
                        cancel: Arc::new(AtomicBool::new(false)),
                        queued: AtomicBool::new(true),
                        result: Mutex::new(None),
                        done: Condvar::new(),
                    });
                    calls.insert(key.clone(), Arc::clone(&call));
                    (call, true)
                }
            }
        };
        if !leader {
            return call.wait();
        }
        let landing = Landing {
            calls: self,
            key,
            call: Arc::clone(&call),
            finished: false,
        };
        let outcome = match self.take_turn(&call, parallel_calls().max(1) as usize) {
            Some(_turn) => {
                call.queued.store(false, Ordering::SeqCst);
                work(Arc::clone(&call.cancel))
            }
            None => Err(ModelError::Cancelled.into()),
        };
        landing.finish(outcome.clone());
        outcome
    }

    /// What the call for `key` returns, once it ends; None when none goes
    /// on. Never asks the model.
    pub fn join(&self, key: &CallKey) -> Option<Outcome> {
        let call = self
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()?;
        Some(call.wait())
    }

    /// The calls going on for the review `target` of `project_id`, by
    /// failure.
    pub fn running(&self, project_id: &str, target: &ReviewTarget) -> Vec<RunningDiagnosis> {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let mut running: Vec<RunningDiagnosis> = calls
            .iter()
            .filter(|(key, _)| key.project == project_id && key.target == *target)
            .map(|(key, call)| RunningDiagnosis {
                key: key.failure.clone(),
                package: call.subject.package.clone(),
                test: call.subject.test.clone(),
                queued: call.queued.load(Ordering::SeqCst),
            })
            .collect();
        running.sort_by(|a, b| a.key.cmp(&b.key));
        running
    }

    /// Stops the call for `key`, asking or queued; those waiting for it get
    /// `cancelled`. Nothing happens when none goes on.
    pub fn cancel(&self, key: &CallKey) {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(call) = calls.get(key) {
            call.cancel.store(true, Ordering::SeqCst);
        }
        drop(calls);
        // Under the lock the queued wait on, so none misses it.
        let _asking = self.asking.lock().unwrap_or_else(PoisonError::into_inner);
        self.turn.notify_all();
    }

    /// Waits for a turn to ask the model; None when stopped first.
    fn take_turn(&self, call: &Call, limit: usize) -> Option<Turn<'_>> {
        let mut asking = self.asking.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if call.cancel.load(Ordering::SeqCst) {
                return None;
            }
            if *asking < limit {
                *asking += 1;
                return Some(Turn { calls: self });
            }
            asking = self
                .turn
                .wait_timeout(asking, QUEUE_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A turn to ask the model, given back when the call ends.
struct Turn<'a> {
    calls: &'a DiagnosisCalls,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut asking = self
            .calls
            .asking
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *asking = asking.saturating_sub(1);
        self.calls.turn.notify_all();
    }
}

/// Publishes how a call ended and forgets it, also when its work unwinds.
struct Landing<'a> {
    calls: &'a DiagnosisCalls,
    key: CallKey,
    call: Arc<Call>,
    finished: bool,
}

impl Landing<'_> {
    fn finish(mut self, outcome: Outcome) {
        self.finished = true;
        self.publish(outcome);
    }

    fn publish(&self, outcome: Outcome) {
        self.calls
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        let mut result = self.call.result.lock().unwrap_or_else(PoisonError::into_inner);
        if result.is_none() {
            *result = Some(outcome);
        }
        self.call.done.notify_all();
    }
}

impl Drop for Landing<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.publish(Err(app_error!(
                Internal,
                ModelCallAbandoned,
                "The model call stopped unexpectedly."
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use super::*;
    use crate::error::ErrorCode;
    use crate::model::{DiagnosisConfidence, DiagnosisFault, ModelProviderKind};
    use crate::language::Language;

    fn target() -> ReviewTarget {
        ReviewTarget::Branch {
            branch: "feature/AUTH-12".to_string(),
            base: "main".to_string(),
        }
    }

    fn subject(test: &str) -> Subject {
        Subject {
            package: "app/auth".to_string(),
            test: Some(test.to_string()),
        }
    }

    fn key(test: &str) -> CallKey {
        CallKey::new("project-1", &target(), &subject(test).key())
    }

    fn written(test: &str) -> TestDiagnosis {
        TestDiagnosis {
            key: subject(test).key(),
            package: "app/auth".to_string(),
            test: Some(test.to_string()),
            cause: "The goroutine writes `lastLogin` while the test reads it.".to_string(),
            fault: DiagnosisFault::Code,
            confidence: DiagnosisConfidence::High,
            location: None,
            fix: String::new(),
            fix_code: String::new(),
            language: Language::English,
            translations: Vec::new(),
            provider: ModelProviderKind::Anthropic,
            provider_name: "Anthropic API".to_string(),
            model: "claude-opus-5-5".to_string(),
            created_at: "2026-11-21T09:00:00Z".to_string(),
            run_started_at: "2026-11-21T09:00:00Z".to_string(),
            outdated: false,
            comment_id: None,
        }
    }

    fn until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_call_left_behind_is_listed_joined_and_never_asked_twice() {
        let calls = DiagnosisCalls::default();
        let asked = AtomicUsize::new(0);
        let (answer, answered) = mpsc::channel::<()>();
        let answered = Mutex::new(answered);
        thread::scope(|scope| {
            // The screen that asked is gone; its call goes on.
            let first = scope.spawn(|| {
                calls.explain(key("TestSignUp"), &subject("TestSignUp"), || 1, |_| {
                    asked.fetch_add(1, Ordering::SeqCst);
                    answered.lock().unwrap().recv().unwrap();
                    Ok(written("TestSignUp"))
                })
            });
            until(|| asked.load(Ordering::SeqCst) == 1);

            // Coming back: it shows as being written, not queued.
            let running = calls.running("project-1", &target());
            assert_eq!(
                running,
                vec![RunningDiagnosis {
                    key: "app/auth::TestSignUp".to_string(),
                    package: "app/auth".to_string(),
                    test: Some("TestSignUp".to_string()),
                    queued: false,
                }]
            );
            // Another review or project has none.
            assert!(calls.running("project-2", &target()).is_empty());
            assert!(calls.running("project-1", &ReviewTarget::WorkingTree).is_empty());

            // Asking again joins it, and so does waiting for it.
            let again = scope.spawn(|| {
                calls.explain(key("TestSignUp"), &subject("TestSignUp"), || 1, |_| {
                    asked.fetch_add(1, Ordering::SeqCst);
                    Ok(written("TestSignUp"))
                })
            });
            let joined = scope.spawn(|| calls.join(&key("TestSignUp")));
            // Time for both to reach the call before it ends.
            thread::sleep(Duration::from_millis(200));
            answer.send(()).unwrap();

            assert_eq!(first.join().unwrap(), Ok(written("TestSignUp")));
            assert_eq!(again.join().unwrap(), Ok(written("TestSignUp")));
            assert_eq!(joined.join().unwrap(), Some(Ok(written("TestSignUp"))));
        });
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the model was asked once");
        assert!(calls.running("project-1", &target()).is_empty());
        // Once over, there is nothing to join: the kept one is read instead.
        assert_eq!(calls.join(&key("TestSignUp")), None);
    }

    #[test]
    fn calls_take_turns_and_a_queued_or_asking_call_is_stopped() {
        let calls = DiagnosisCalls::default();
        let (answer, answered) = mpsc::channel::<()>();
        let answered = Mutex::new(answered);
        let second_asked = AtomicBool::new(false);
        thread::scope(|scope| {
            let first = scope.spawn(|| {
                calls.explain(key("TestSignUp"), &subject("TestSignUp"), || 1, |_| {
                    answered.lock().unwrap().recv().unwrap();
                    Ok(written("TestSignUp"))
                })
            });
            until(|| {
                calls
                    .running("project-1", &target())
                    .first()
                    .is_some_and(|call| !call.queued)
            });
            let second = scope.spawn(|| {
                calls.explain(key("TestLogin"), &subject("TestLogin"), || 1, |_| {
                    second_asked.store(true, Ordering::SeqCst);
                    Ok(written("TestLogin"))
                })
            });
            until(|| calls.running("project-1", &target()).len() == 2);
            let queued: Vec<(String, bool)> = calls
                .running("project-1", &target())
                .into_iter()
                .map(|call| (call.key, call.queued))
                .collect();
            assert_eq!(
                queued,
                vec![
                    ("app/auth::TestLogin".to_string(), true),
                    ("app/auth::TestSignUp".to_string(), false),
                ]
            );

            // Stopped while queued: it never asks.
            calls.cancel(&key("TestLogin"));
            let stopped = second.join().unwrap().unwrap_err();
            assert_eq!(stopped.code, ErrorCode::Cancelled);
            assert!(!second_asked.load(Ordering::SeqCst));

            answer.send(()).unwrap();
            assert!(first.join().unwrap().is_ok());
        });

        // Stopped while asking: the work sees the flag, and joiners get
        // cancelled too.
        thread::scope(|scope| {
            let asking = scope.spawn(|| {
                calls.explain(key("TestSignUp"), &subject("TestSignUp"), || 1, |cancel| {
                    while !cancel.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(ModelError::Cancelled.into())
                })
            });
            until(|| !calls.running("project-1", &target()).is_empty());
            let joined = scope.spawn(|| calls.join(&key("TestSignUp")));
            thread::sleep(Duration::from_millis(20));
            calls.cancel(&key("TestSignUp"));
            assert_eq!(asking.join().unwrap().unwrap_err().code, ErrorCode::Cancelled);
            assert!(joined
                .join()
                .unwrap()
                .is_some_and(|outcome| outcome.is_err_and(|error| error.code == ErrorCode::Cancelled)));
        });
        assert!(calls.running("project-1", &target()).is_empty());
    }

    #[test]
    fn a_call_whose_work_panics_ends_for_those_waiting() {
        let calls = DiagnosisCalls::default();
        let (go, started) = mpsc::channel::<()>();
        let started = Mutex::new(started);
        thread::scope(|scope| {
            let leader = scope.spawn(|| {
                calls.explain(key("TestSignUp"), &subject("TestSignUp"), || 1, |_| {
                    started.lock().unwrap().recv().unwrap();
                    panic!("the work failed");
                })
            });
            until(|| !calls.running("project-1", &target()).is_empty());
            let joined = scope.spawn(|| calls.join(&key("TestSignUp")));
            thread::sleep(Duration::from_millis(20));
            go.send(()).unwrap();
            assert!(leader.join().is_err());
            assert!(joined.join().unwrap().is_some_and(|outcome| outcome.is_err()));
        });
        assert!(calls.running("project-1", &target()).is_empty());
        // The turn was given back.
        assert_eq!(
            calls.explain(key("TestLogin"), &subject("TestLogin"), || 1, |_| Ok(written("TestLogin"))),
            Ok(written("TestLogin"))
        );
    }
}
