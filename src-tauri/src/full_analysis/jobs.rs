//! The registry of full analyses while the app runs: one per project and
//! review target, each on a thread of its own with its steps side by side,
//! at most `limit` running at once and the others queued in order. Every
//! change of a job is emitted (`Events`) with the log line it added, and the
//! whole log is kept in memory, so a screen that comes back shows it all.
//! Nothing is kept on disk: quitting the app stops every job, and what the
//! steps already saved stays.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use uuid::Uuid;

use crate::ai::Calls;
use crate::error::{app_error, AppError};
use crate::flow;
use crate::language::Language;
use crate::model::{
    AnalysisStep, FullAnalysisEvent, FullAnalysisJob, JobOutcome, JobStatus, JobSummary, LogEvent,
    LogLine, PullRequestText, ReviewRef, StepState, StepStatus,
};

/// Full analyses allowed at once until the user picks another number.
pub const DEFAULT_LIMIT: u32 = 4;
pub const MAX_LIMIT: u32 = 16;

/// Where a job's changes go: the screens, through Tauri events.
pub trait Events: Send + Sync {
    fn emit(&self, event: &FullAnalysisEvent);
}

/// What a full analysis was started for.
#[derive(Debug, Clone)]
pub struct JobRequest {
    pub project_id: String,
    pub project_name: String,
    pub review: ReviewRef,
    pub pull_request: Option<PullRequestText>,
    /// The language shown when it was confirmed: what the model writes in.
    pub language: Language,
}

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEnd {
    Done,
    /// It had nothing to do.
    Skipped,
    Failed(AppError),
    /// The job was cancelled while it ran.
    Cancelled,
}

/// The work of each step. A step logs what it does, reports its progress,
/// asks the model only through `JobRun::calls` and stops soon after
/// `JobRun::is_cancelled`.
pub trait Steps: Send + Sync {
    fn run(&self, step: AnalysisStep, job: &JobRun) -> StepEnd;
}

struct Job {
    summary: JobSummary,
    log: Vec<LogLine>,
    request: Arc<JobRequest>,
    calls: Arc<Calls>,
}

impl Job {
    fn view(&self) -> FullAnalysisJob {
        FullAnalysisJob {
            summary: self.summary.clone(),
            log: self.log.clone(),
        }
    }

    fn step(&mut self, step: AnalysisStep) -> Option<&mut StepState> {
        self.summary
            .steps
            .iter_mut()
            .find(|state| state.step == step)
    }

    fn same_review(&self, request: &JobRequest) -> bool {
        self.request.project_id == request.project_id
            && self.request.review.target == request.review.target
    }
}

struct State {
    /// In the order they were started.
    jobs: Vec<Job>,
    limit: u32,
}

struct Shared {
    state: Mutex<State>,
    /// Raised when a step ends, for a step that waits for another.
    step_ended: Condvar,
    events: Arc<dyn Events>,
    steps: Arc<dyn Steps>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes job `id`, if it is still there, and emits it with the line
    /// `change` logged, if any.
    fn change(&self, id: &str, change: impl FnOnce(&mut Job) -> Option<LogEvent>) {
        let mut state = self.lock();
        if let Some(job) = state.jobs.iter_mut().find(|job| job.summary.id == id) {
            let event = change(job);
            self.emit(job, event);
        }
    }

    fn emit(&self, job: &mut Job, event: Option<LogEvent>) {
        job.summary.progress = progress(&job.summary.steps);
        let line = event.map(|event| LogLine { at: now(), event });
        if let Some(line) = &line {
            job.log.push(line.clone());
        }
        self.events.emit(&FullAnalysisEvent {
            job: job.summary.clone(),
            line,
        });
    }

    /// Starts queued jobs, oldest first, while fewer than the limit run.
    fn promote(self: &Arc<Self>, state: &mut State) {
        loop {
            let running = count_running(state);
            if running >= state.limit {
                return;
            }
            let Some(job) = state
                .jobs
                .iter_mut()
                .find(|job| job.summary.status == JobStatus::Queued)
            else {
                return;
            };
            job.summary.status = JobStatus::Running;
            job.summary.started_at = Some(now());
            for step in &mut job.summary.steps {
                step.status = StepStatus::Running;
            }
            self.emit(job, Some(LogEvent::Started));
            let shared = Arc::clone(self);
            let run = JobRun {
                shared: Arc::clone(self),
                id: job.summary.id.clone(),
                request: Arc::clone(&job.request),
                calls: Arc::clone(&job.calls),
            };
            thread::spawn(move || {
                run_job(&run);
                shared.finish(&run);
            });
        }
    }

    /// Ends job `run` once its steps have, and starts the next queued one.
    fn finish(self: &Arc<Self>, run: &JobRun) {
        let mut state = self.lock();
        if let Some(job) = state.jobs.iter_mut().find(|job| job.summary.id == run.id) {
            let failed: Vec<AnalysisStep> = job
                .summary
                .steps
                .iter()
                .filter(|step| step.status == StepStatus::Failed)
                .map(|step| step.step)
                .collect();
            let (outcome, event) = if run.calls.is_cancelled() {
                (JobOutcome::Cancelled, LogEvent::Cancelled)
            } else if failed.is_empty() {
                (JobOutcome::Succeeded, LogEvent::Finished)
            } else {
                (
                    JobOutcome::Failed,
                    LogEvent::FinishedWithFailures { steps: failed },
                )
            };
            job.summary.status = JobStatus::Finished;
            job.summary.outcome = Some(outcome);
            job.summary.cancelling = false;
            job.summary.finished_at = Some(now());
            self.emit(job, Some(event));
        }
        self.promote(&mut state);
    }
}

/// A running job, as its steps see it.
pub struct JobRun {
    shared: Arc<Shared>,
    id: String,
    request: Arc<JobRequest>,
    calls: Arc<Calls>,
}

impl JobRun {
    pub fn request(&self) -> &JobRequest {
        &self.request
    }

    /// The model calls of the job: one Cancel stops them all.
    pub fn calls(&self) -> &Calls {
        &self.calls
    }

    pub fn is_cancelled(&self) -> bool {
        self.calls.is_cancelled()
    }

    pub fn log(&self, event: LogEvent) {
        self.shared.change(&self.id, |_| Some(event));
    }

    /// `done` of `total` parts of `step` are done.
    pub fn progress(&self, step: AnalysisStep, done: u32, total: u32) {
        self.shared.change(&self.id, |job| {
            if let Some(state) = job.step(step) {
                state.done = done.min(total);
                state.total = total;
            }
            None
        });
    }

    /// Waits until `step` of this job has ended, and says how.
    pub fn wait_for(&self, step: AnalysisStep) -> StepStatus {
        let mut state = self.shared.lock();
        loop {
            let status = state
                .jobs
                .iter()
                .find(|job| job.summary.id == self.id)
                .and_then(|job| job.summary.steps.iter().find(|state| state.step == step))
                .map(|state| state.status);
            match status {
                Some(StepStatus::Pending | StepStatus::Running) => {
                    state = self
                        .shared
                        .step_ended
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(status) => return status,
                None => return StepStatus::Cancelled,
            }
        }
    }

    fn end_step(&self, step: AnalysisStep, end: StepEnd) {
        self.shared.change(&self.id, |job| {
            let state = job.step(step)?;
            let (status, event) = match end {
                StepEnd::Done => (StepStatus::Done, None),
                StepEnd::Skipped => (StepStatus::Skipped, None),
                StepEnd::Failed(error) => {
                    state.error = Some(error.clone());
                    (
                        StepStatus::Failed,
                        Some(LogEvent::StepFailed { step, error }),
                    )
                }
                StepEnd::Cancelled => (
                    StepStatus::Cancelled,
                    Some(LogEvent::StepCancelled { step }),
                ),
            };
            state.status = status;
            if status == StepStatus::Done {
                state.done = state.total;
            }
            event
        });
        self.shared.step_ended.notify_all();
    }
}

/// Runs every step of the job at once and waits for them all.
fn run_job(run: &JobRun) {
    thread::scope(|scope| {
        for step in AnalysisStep::ALL {
            scope.spawn(move || {
                let end = if run.is_cancelled() {
                    StepEnd::Cancelled
                } else {
                    panic::catch_unwind(AssertUnwindSafe(|| run.shared.steps.run(step, run)))
                        .unwrap_or_else(|_| {
                            StepEnd::Failed(app_error!(
                                Internal,
                                StepStoppedUnexpectedly,
                                "The step stopped unexpectedly."
                            ))
                        })
                };
                run.end_step(step, end);
            });
        }
    });
}

/// Full analyses while the app runs.
#[derive(Clone)]
pub struct Jobs {
    shared: Arc<Shared>,
}

impl Jobs {
    pub fn new(steps: Arc<dyn Steps>, events: Arc<dyn Events>, limit: u32) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    jobs: Vec::new(),
                    limit: clamp_limit(limit),
                }),
                step_ended: Condvar::new(),
                events,
                steps,
            }),
        }
    }

    /// Starts a full analysis of the review, or queues it when as many as
    /// the limit already run. One already going for the same review is
    /// returned as it is; a finished one is replaced.
    pub fn start(&self, request: JobRequest) -> FullAnalysisJob {
        let mut state = self.shared.lock();
        if let Some(job) = state
            .jobs
            .iter()
            .find(|job| job.same_review(&request) && job.summary.status != JobStatus::Finished)
        {
            return job.view();
        }
        state.jobs.retain(|job| !job.same_review(&request));
        let id = Uuid::new_v4().to_string();
        let mut job = Job {
            summary: JobSummary {
                id: id.clone(),
                project_id: request.project_id.clone(),
                project_name: request.project_name.clone(),
                review: request.review.clone(),
                status: JobStatus::Queued,
                outcome: None,
                cancelling: false,
                steps: AnalysisStep::ALL
                    .into_iter()
                    .map(|step| StepState {
                        step,
                        status: StepStatus::Pending,
                        done: 0,
                        total: 0,
                        error: None,
                    })
                    .collect(),
                progress: 0,
                queued_at: now(),
                started_at: None,
                finished_at: None,
            },
            log: Vec::new(),
            request: Arc::new(request),
            calls: Arc::new(Calls::new(format!("full-{id}"))),
        };
        let waits = count_running(&state) >= state.limit;
        if waits {
            self.shared.emit(&mut job, Some(LogEvent::Queued));
        }
        state.jobs.push(job);
        self.shared.promote(&mut state);
        state
            .jobs
            .iter()
            .find(|job| job.summary.id == id)
            .map(Job::view)
            .expect("the job was just added")
    }

    /// Stops job `id`: a queued one ends at once; a running one stops its
    /// steps in flight and starts nothing more, keeping what was done.
    pub fn cancel(&self, id: &str) {
        let mut state = self.shared.lock();
        let Some(job) = state.jobs.iter_mut().find(|job| job.summary.id == id) else {
            return;
        };
        match job.summary.status {
            JobStatus::Queued => {
                job.calls.cancel();
                job.summary.status = JobStatus::Finished;
                job.summary.outcome = Some(JobOutcome::Cancelled);
                job.summary.finished_at = Some(now());
                for step in &mut job.summary.steps {
                    step.status = StepStatus::Cancelled;
                }
                self.shared.emit(job, Some(LogEvent::Cancelled));
            }
            JobStatus::Running if !job.summary.cancelling => {
                job.summary.cancelling = true;
                job.calls.cancel();
                self.shared.emit(job, Some(LogEvent::Cancelling));
            }
            _ => {}
        }
    }

    /// Forgets a finished job: its screen gives the review back.
    pub fn dismiss(&self, id: &str) {
        self.shared
            .lock()
            .jobs
            .retain(|job| job.summary.id != id || job.summary.status != JobStatus::Finished);
    }

    /// Every job, with its whole log.
    pub fn list(&self) -> Vec<FullAnalysisJob> {
        self.shared.lock().jobs.iter().map(Job::view).collect()
    }

    pub fn limit(&self) -> u32 {
        self.shared.lock().limit
    }

    /// Jobs running now.
    pub fn running(&self) -> u32 {
        count_running(&self.shared.lock())
    }

    /// Allows `limit` jobs at once (1 to `MAX_LIMIT`), starting queued ones
    /// when there is room now; returns the limit kept.
    pub fn set_limit(&self, limit: u32) -> u32 {
        let mut state = self.shared.lock();
        state.limit = clamp_limit(limit);
        self.shared.promote(&mut state);
        state.limit
    }
}

pub fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(1, MAX_LIMIT)
}

fn count_running(state: &State) -> u32 {
    u32::try_from(
        state
            .jobs
            .iter()
            .filter(|job| job.summary.status == JobStatus::Running)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Each step counts the same; one in progress by the parts it has done.
fn progress(steps: &[StepState]) -> u32 {
    if steps.is_empty() {
        return 0;
    }
    let done: f64 = steps
        .iter()
        .map(|step| match step.status {
            StepStatus::Pending => 0.0,
            StepStatus::Running if step.total > 0 => f64::from(step.done) / f64::from(step.total),
            StepStatus::Running => 0.0,
            _ => 1.0,
        })
        .sum();
    (done / steps.len() as f64 * 100.0).floor() as u32
}

fn now() -> String {
    flow::now().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::model::ReviewTarget;

    #[derive(Default)]
    struct Recorded {
        events: Mutex<Vec<FullAnalysisEvent>>,
    }

    impl Events for Recorded {
        fn emit(&self, event: &FullAnalysisEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    /// Steps that end as told, the ones listed in `blocked` waiting until
    /// they are released or the job is cancelled.
    #[derive(Default)]
    struct FakeSteps {
        failing: Vec<AnalysisStep>,
        blocked: Vec<AnalysisStep>,
        released: AtomicBool,
        order: Mutex<Vec<(AnalysisStep, &'static str)>>,
    }

    impl Steps for FakeSteps {
        fn run(&self, step: AnalysisStep, job: &JobRun) -> StepEnd {
            self.order.lock().unwrap().push((step, "start"));
            if step == AnalysisStep::Metrics {
                job.log(LogEvent::CoverageWaiting);
                job.wait_for(AnalysisStep::Tests);
            }
            if step == AnalysisStep::Insights {
                job.progress(step, 1, 2);
            }
            if self.blocked.contains(&step) {
                while !self.released.load(Ordering::SeqCst) {
                    if job.is_cancelled() {
                        return StepEnd::Cancelled;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            self.order.lock().unwrap().push((step, "end"));
            if self.failing.contains(&step) {
                return StepEnd::Failed(app_error!(
                    Internal,
                    StepStoppedUnexpectedly,
                    "The step stopped unexpectedly."
                ));
            }
            StepEnd::Done
        }
    }

    fn request(branch: &str) -> JobRequest {
        JobRequest {
            project_id: "p".to_string(),
            project_name: "pay".to_string(),
            review: ReviewRef {
                target: ReviewTarget::Branch {
                    branch: branch.to_string(),
                    base: "main".to_string(),
                },
                old_sha: "a".repeat(40),
                new_sha: Some("b".repeat(40)),
            },
            pull_request: None,
            language: Language::BrazilianPortuguese,
        }
    }

    fn jobs(steps: FakeSteps, limit: u32) -> (Jobs, Arc<FakeSteps>, Arc<Recorded>) {
        let steps = Arc::new(steps);
        let events = Arc::new(Recorded::default());
        let jobs = Jobs::new(
            Arc::clone(&steps) as Arc<dyn Steps>,
            Arc::clone(&events) as Arc<dyn Events>,
            limit,
        );
        (jobs, steps, events)
    }

    fn job(jobs: &Jobs, id: &str) -> FullAnalysisJob {
        jobs.list()
            .into_iter()
            .find(|job| job.summary.id == id)
            .unwrap()
    }

    fn wait_until(
        jobs: &Jobs,
        id: &str,
        done: impl Fn(&FullAnalysisJob) -> bool,
    ) -> FullAnalysisJob {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let job = job(jobs, id);
            if done(&job) {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "the job never got there: {job:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn finished(jobs: &Jobs, id: &str) -> FullAnalysisJob {
        wait_until(jobs, id, |job| job.summary.status == JobStatus::Finished)
    }

    fn codes(job: &FullAnalysisJob) -> Vec<String> {
        job.log
            .iter()
            .map(|line| {
                serde_json::to_value(&line.event).unwrap()["code"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn every_step_runs_at_once_and_the_metrics_wait_for_the_tests() {
        let (jobs, steps, events) = jobs(FakeSteps::default(), 4);
        let id = jobs.start(request("feature/a")).summary.id;
        let job = finished(&jobs, &id);
        assert_eq!(job.summary.outcome, Some(JobOutcome::Succeeded));
        assert_eq!(job.summary.progress, 100);
        assert!(job
            .summary
            .steps
            .iter()
            .all(|step| step.status == StepStatus::Done));
        assert_eq!(
            job.summary.steps[2].done, 2,
            "a step done counts all its parts"
        );
        assert_eq!(codes(&job), ["started", "coverage_waiting", "finished"]);
        let order = steps.order.lock().unwrap().clone();
        let at = |entry| order.iter().position(|other| *other == entry).unwrap();
        assert!(at((AnalysisStep::Tests, "end")) < at((AnalysisStep::Metrics, "end")));
        // Every change was emitted, the lines with the change that logged them.
        let events = events.events.lock().unwrap();
        assert!(events.len() > 5);
        assert_eq!(
            events.last().unwrap().line.as_ref().unwrap().event,
            LogEvent::Finished
        );
        assert_eq!(events.last().unwrap().job.status, JobStatus::Finished);
    }

    #[test]
    fn a_failing_step_does_not_stop_the_others() {
        let (jobs, _, _) = jobs(
            FakeSteps {
                failing: vec![AnalysisStep::Summary],
                ..FakeSteps::default()
            },
            4,
        );
        let id = jobs.start(request("feature/a")).summary.id;
        let job = finished(&jobs, &id);
        assert_eq!(job.summary.outcome, Some(JobOutcome::Failed));
        let statuses: Vec<StepStatus> = job.summary.steps.iter().map(|step| step.status).collect();
        assert_eq!(
            statuses,
            [
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Failed,
                StepStatus::Done
            ]
        );
        assert!(job.summary.steps[3].error.is_some());
        assert!(codes(&job).contains(&"step_failed".to_string()));
        assert_eq!(
            job.log.last().unwrap().event,
            LogEvent::FinishedWithFailures {
                steps: vec![AnalysisStep::Summary]
            }
        );
    }

    #[test]
    fn beyond_the_limit_a_job_waits_in_the_queue() {
        let (jobs, steps, _) = jobs(
            FakeSteps {
                blocked: vec![AnalysisStep::Insights],
                ..FakeSteps::default()
            },
            1,
        );
        let first = jobs.start(request("feature/a")).summary.id;
        let second = jobs.start(request("feature/b"));
        assert_eq!(second.summary.status, JobStatus::Queued);
        assert_eq!(codes(&second), ["queued"]);
        assert_eq!(jobs.running(), 1);
        // The same review again is the job already there.
        assert_eq!(jobs.start(request("feature/a")).summary.id, first);

        // More room starts the queued job at once.
        assert_eq!(jobs.set_limit(2), 2);
        wait_until(&jobs, &second.summary.id, |job| {
            job.summary.status == JobStatus::Running
        });
        assert_eq!(jobs.set_limit(0), 1);
        assert_eq!(jobs.set_limit(99), MAX_LIMIT);

        steps.released.store(true, Ordering::SeqCst);
        finished(&jobs, &first);
        finished(&jobs, &second.summary.id);
        // A finished one is replaced by a new start, and can be dismissed.
        let again = jobs.start(request("feature/a")).summary.id;
        assert_ne!(again, first);
        finished(&jobs, &again);
        jobs.dismiss(&again);
        assert_eq!(jobs.list().len(), 1);
    }

    #[test]
    fn cancel_stops_the_steps_in_flight_and_keeps_what_is_done() {
        let (jobs, _, _) = jobs(
            FakeSteps {
                blocked: vec![AnalysisStep::Insights, AnalysisStep::Explanations],
                ..FakeSteps::default()
            },
            1,
        );
        let id = jobs.start(request("feature/a")).summary.id;
        let queued = jobs.start(request("feature/b")).summary.id;
        wait_until(&jobs, &id, |job| {
            job.summary.steps[3].status == StepStatus::Done
        });

        // A queued job ends at once, without running.
        jobs.cancel(&queued);
        let cancelled = job(&jobs, &queued);
        assert_eq!(cancelled.summary.outcome, Some(JobOutcome::Cancelled));
        assert_eq!(codes(&cancelled), ["queued", "cancelled"]);

        jobs.cancel(&id);
        let job = finished(&jobs, &id);
        assert_eq!(job.summary.outcome, Some(JobOutcome::Cancelled));
        let statuses: Vec<StepStatus> = job.summary.steps.iter().map(|step| step.status).collect();
        assert_eq!(
            statuses,
            [
                StepStatus::Done,
                StepStatus::Done,
                StepStatus::Cancelled,
                StepStatus::Done,
                StepStatus::Cancelled
            ]
        );
        let codes = codes(&job);
        assert!(codes.contains(&"cancelling".to_string()));
        assert_eq!(codes.last().unwrap(), "cancelled");
        assert!(!job.summary.cancelling);
    }

    #[test]
    fn progress_counts_each_step_the_same() {
        let step = |status, done, total| StepState {
            step: AnalysisStep::Tests,
            status,
            done,
            total,
            error: None,
        };
        assert_eq!(
            progress(&[
                step(StepStatus::Done, 0, 0),
                step(StepStatus::Running, 1, 4),
                step(StepStatus::Running, 0, 0),
                step(StepStatus::Pending, 0, 0),
            ]),
            31
        );
    }
}
