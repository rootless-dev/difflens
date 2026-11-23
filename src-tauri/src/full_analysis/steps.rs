//! The steps of a full analysis, done by the services the tabs use: the
//! tests run in the sandbox, the flow and quality metrics (coverage and CRAP
//! once the tests ran), every call of an Insights analysis, the Overview
//! summary, and the short explanations of the changed production functions.
//! Each logs what it does and asks the model only through the job's calls.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::jobs::{JobRun, StepEnd, Steps};
use crate::ai::ModelService;
use crate::error::{app_error, AppError, AppResult, ErrorCode, Reason};
use crate::explanations::{self, Context, ExplanationService, Scope, MAX_COMMITS};
use crate::flow::FlowService;
use crate::insights::{CallEnd, InsightsService};
use crate::jira::JiraService;
use crate::model::{
    AnalysisStep, Card, ExplanationKind, ExplanationLevel, ExplanationPlan, FlowGraph, FlowNode,
    FlowNodeState,
    FlowOptions, FlowResult, FullAnalysisPlan, Isolation, LogEvent, ModelTask, PullRequestText,
    QualityResult, ReviewRef, StepStatus, TestRunStatus,
};
use crate::overview::OverviewService;
use crate::projects::ProjectService;
use crate::quality::QualityService;
use crate::review_state::ReviewSession;
use crate::test_runs::TestsService;

/// How often a step waiting on a test run checks for a cancel.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// The graph the explanations of a full analysis are written for: the
/// changed functions without tests, and the callers and callees around them.
pub const EXPLAINED_GRAPH: FlowOptions = FlowOptions {
    include_tests: false,
    include_callers: true,
    include_callees: true,
};

pub struct ServiceSteps {
    pub projects: Arc<ProjectService>,
    pub flow: Arc<FlowService>,
    pub tests: Arc<TestsService>,
    pub quality: Arc<QualityService>,
    pub models: Arc<ModelService>,
    pub insights: Arc<InsightsService>,
    pub overview: Arc<OverviewService>,
    pub jira: Arc<JiraService>,
    pub explanations: Arc<ExplanationService>,
}

impl Steps for ServiceSteps {
    fn run(&self, step: AnalysisStep, job: &JobRun) -> StepEnd {
        let result = match step {
            AnalysisStep::Tests => self.tests(job),
            AnalysisStep::Metrics => self.metrics(job),
            AnalysisStep::Insights => self.insights(job),
            AnalysisStep::Summary => self.summary(job),
            AnalysisStep::Explanations => self.explanations(job),
        };
        match result {
            Ok(end) => end,
            Err(error) if error.code == ErrorCode::Cancelled => StepEnd::Cancelled,
            // Every changed file is deleted, binary or too large: nothing to
            // read, which is not a failure.
            Err(error) if error.reason == Some(Reason::NothingToSend) => StepEnd::Skipped,
            Err(_) if job.is_cancelled() => StepEnd::Cancelled,
            Err(error) => StepEnd::Failed(error),
        }
    }
}

impl ServiceSteps {
    fn repo(&self, job: &JobRun) -> AppResult<PathBuf> {
        self.projects.repository(&job.request().project_id)
    }

    fn session<'a>(&'a self, job: &'a JobRun, repo: &'a Path) -> AppResult<ReviewSession<'a>> {
        ReviewSession::new(
            self.projects.store(),
            self.projects.git(),
            &job.request().project_id,
            repo,
            job.request().review.clone(),
        )
    }

    /// Runs the tests of the new side in the sandbox, with coverage, as the
    /// Tests tab does; a cancel stops the run.
    fn tests(&self, job: &JobRun) -> AppResult<StepEnd> {
        let repo = self.repo(job)?;
        let request = job.request();
        let new_sha = request.review.new_sha.as_deref();
        job.log(LogEvent::TestsStarted);
        let run = thread::scope(|scope| {
            let running = scope.spawn(|| {
                self.tests.run(
                    &request.project_id,
                    &repo,
                    &request.review.old_sha,
                    new_sha,
                    Isolation::Sandbox,
                )
            });
            while !running.is_finished() {
                if job.is_cancelled() {
                    self.tests.cancel(&request.project_id, new_sha);
                }
                thread::sleep(CANCEL_POLL);
            }
            running.join().unwrap_or_else(|_| {
                Err(app_error!(
                    Internal,
                    TestRunAbandoned,
                    "The test run stopped unexpectedly."
                ))
            })
        });
        let run = match run {
            Ok(run) => run,
            Err(error)
                if matches!(
                    error.reason,
                    Some(Reason::NoTestRunner | Reason::NoTestRunnerNoLanguages)
                ) =>
            {
                job.log(LogEvent::TestsUnavailable);
                return Ok(StepEnd::Skipped);
            }
            Err(error) => return Err(error),
        };
        match run.status {
            TestRunStatus::Completed => {
                job.log(LogEvent::TestsFinished {
                    passed: run.summary.passed,
                    failed: run.summary.failed,
                    skipped: run.summary.skipped,
                    duration_ms: run.duration_ms,
                });
                Ok(StepEnd::Done)
            }
            TestRunStatus::Cancelled => Ok(StepEnd::Cancelled),
            TestRunStatus::TimedOut => Err(app_error!(
                Internal,
                TestRunTimedOut,
                "The test run did not finish in time."
            )),
            TestRunStatus::Failed => Err(match run.message {
                Some(message) => app_error!(
                    Internal,
                    TestRunFailedWith,
                    "The test run failed: {detail}",
                    detail = message
                ),
                None => app_error!(Internal, TestRunFailedQuietly, "The test run failed."),
            }),
        }
    }

    /// The flow and the quality report right away; coverage and CRAP once
    /// the tests ran.
    fn metrics(&self, job: &JobRun) -> AppResult<StepEnd> {
        let repo = self.repo(job)?;
        let request = job.request();
        let (old_sha, new_sha) = (&request.review.old_sha, request.review.new_sha.as_deref());
        job.progress(AnalysisStep::Metrics, 0, 3);
        match self.flow.flow(
            &request.project_id,
            &repo,
            old_sha,
            new_sha,
            FlowOptions::default(),
        )? {
            FlowResult::Graph { graph } => job.log(LogEvent::FlowMeasured {
                changed: count(graph.nodes.iter().filter(|node| changed(node)).count()),
            }),
            FlowResult::Unavailable(_) => job.log(LogEvent::FlowUnavailable),
        }
        job.progress(AnalysisStep::Metrics, 1, 3);
        let measured = self
            .quality
            .quality(&request.project_id, &repo, old_sha, new_sha)?;
        if let QualityResult::Report { report } = &measured {
            job.log(LogEvent::QualityMeasured {
                crossed: count(report.crossed.len()),
            });
        }
        job.progress(AnalysisStep::Metrics, 2, 3);
        if job.is_cancelled() {
            return Ok(StepEnd::Cancelled);
        }
        job.log(LogEvent::CoverageWaiting);
        if job.wait_for(AnalysisStep::Tests) != StepStatus::Done
            || matches!(measured, QualityResult::Unavailable(_))
        {
            job.log(LogEvent::CoverageSkipped);
            return Ok(StepEnd::Done);
        }
        if let QualityResult::Report { report } =
            self.quality
                .quality(&request.project_id, &repo, old_sha, new_sha)?
        {
            job.log(LogEvent::CoverageMeasured {
                crossed: count(report.crossed.len()),
            });
        }
        Ok(StepEnd::Done)
    }

    /// Every call of a new Insights analysis, as many at once as the
    /// provider allows.
    fn insights(&self, job: &JobRun) -> AppResult<StepEnd> {
        let repo = self.repo(job)?;
        let session = self.session(job, &repo)?;
        let started = self
            .insights
            .start(&session, &self.models, job.request().language)?;
        let Some(analysis) = started.analysis else {
            return Ok(StepEnd::Skipped);
        };
        let total = count(analysis.batches.len());
        let files: usize = analysis.batches.iter().map(|batch| batch.paths.len()).sum();
        job.log(LogEvent::InsightsPlanned {
            files: count(files),
            calls: total,
        });
        job.progress(AnalysisStep::Insights, 0, total);
        let indexes: Vec<u32> = analysis.batches.iter().map(|batch| batch.index).collect();
        let done = AtomicU32::new(0);
        let ends = self.insights.run_calls(
            &session,
            &self.models,
            job.calls(),
            &analysis.id,
            &indexes,
            analysis.parallel_calls as usize,
            &|index, end| {
                let number = index + 1;
                let event = match end {
                    None => LogEvent::InsightsCallStarted { number, total },
                    Some(Ok(CallEnd::Answered { findings })) => LogEvent::InsightsCallAnswered {
                        number,
                        total,
                        findings: *findings,
                    },
                    Some(Ok(CallEnd::Settled)) => LogEvent::InsightsCallAnswered {
                        number,
                        total,
                        findings: 0,
                    },
                    Some(Ok(CallEnd::Failed(error)) | Err(error)) => {
                        if error.code == ErrorCode::Cancelled {
                            return;
                        }
                        LogEvent::InsightsCallFailed {
                            number,
                            total,
                            error: error.clone(),
                        }
                    }
                };
                if end.is_some() {
                    let done = done.fetch_add(1, Ordering::Relaxed) + 1;
                    job.progress(AnalysisStep::Insights, done, total);
                }
                job.log(event);
            },
        );
        Ok(calls_end(
            job.is_cancelled(),
            ends.into_iter().map(|end| {
                end.map(|end| match end {
                    Ok(CallEnd::Failed(error)) | Err(error) => Err(error),
                    Ok(_) => Ok(()),
                })
            }),
        ))
    }

    /// The Overview summary of the production code.
    fn summary(&self, job: &JobRun) -> AppResult<StepEnd> {
        let repo = self.repo(job)?;
        let session = self.session(job, &repo)?;
        let request = job.request();
        let card = self.card(job)?;
        job.log(LogEvent::SummaryStarted);
        let store = self.projects.store();
        self.overview.summarize_with(
            &session,
            &self.models,
            card.as_ref(),
            request.pull_request.as_ref(),
            request.language,
            &|question| {
                job.calls()
                    .ask(&self.models, store, ModelTask::Summary, question)
            },
        )?;
        job.log(LogEvent::SummaryWritten);
        Ok(StepEnd::Done)
    }

    /// Short explanations of the changed production functions; the
    /// functions drawn around them are left to their own buttons.
    fn explanations(&self, job: &JobRun) -> AppResult<StepEnd> {
        let repo = self.repo(job)?;
        let session = self.session(job, &repo)?;
        let Some(graph) = self.graph(job, &repo)? else {
            job.log(LogEvent::NothingToExplain);
            return Ok(StepEnd::Skipped);
        };
        let context = self.context(job, &repo);
        let planned = self.explanations.prepare(
            &session,
            &graph,
            &Scope::Full,
            ExplanationLevel::Brief,
            &context,
        )?;
        if planned.calls.is_empty() {
            job.log(LogEvent::NothingToExplain);
            return Ok(StepEnd::Skipped);
        }
        let model = self
            .models
            .in_use(self.projects.store(), ModelTask::BriefExplanation)?;
        explanations::ensure_ready(&model)?;
        let total = count(planned.calls.len());
        job.log(LogEvent::ExplanationsPlanned {
            changed: planned.count(ExplanationKind::Changed),
            calls: total,
        });
        job.progress(AnalysisStep::Explanations, 0, total);
        let done = AtomicU32::new(0);
        let ends = self.explanations.run(
            &session,
            &self.models,
            &planned,
            job.request().language,
            job.calls(),
            model.parallel_calls as usize,
            &|index, end| {
                let number = index + 1;
                let event = match end {
                    None => LogEvent::ExplanationsCallStarted { number, total },
                    Some(Ok(functions)) => LogEvent::ExplanationsCallAnswered {
                        number,
                        total,
                        functions: *functions,
                    },
                    Some(Err(error)) => {
                        if error.code == ErrorCode::Cancelled {
                            return;
                        }
                        LogEvent::ExplanationsCallFailed {
                            number,
                            total,
                            error: error.clone(),
                        }
                    }
                };
                if end.is_some() {
                    let done = done.fetch_add(1, Ordering::Relaxed) + 1;
                    job.progress(AnalysisStep::Explanations, done, total);
                }
                job.log(event);
            },
        );
        Ok(calls_end(
            job.is_cancelled(),
            ends.into_iter().map(|end| end.map(|end| end.map(|_| ()))),
        ))
    }

    /// The review's graph as its explanations see it, or `None` when no
    /// analyzer could draw it.
    fn graph(&self, job: &JobRun, repo: &Path) -> AppResult<Option<FlowGraph>> {
        let request = job.request();
        Ok(
            match self.flow.flow(
                &request.project_id,
                repo,
                &request.review.old_sha,
                request.review.new_sha.as_deref(),
                EXPLAINED_GRAPH,
            )? {
                FlowResult::Graph { graph } => Some(*graph),
                FlowResult::Unavailable(_) => None,
            },
        )
    }

    fn card(&self, job: &JobRun) -> AppResult<Option<Card>> {
        let request = job.request();
        self.card_of(&request.review, request.pull_request.as_ref())
    }

    /// The card of a review, read from Jira (only read).
    fn card_of(
        &self,
        review: &ReviewRef,
        pull_request: Option<&PullRequestText>,
    ) -> AppResult<Option<Card>> {
        let title = pull_request.map(|pull_request| pull_request.title.as_str());
        Ok(JiraService::card_of(self.jira.find_card(
            self.projects.store(),
            &review.target,
            title,
        )?))
    }

    fn context(&self, job: &JobRun, repo: &Path) -> Context {
        let request = job.request();
        let card = self.card(job).ok().flatten();
        self.context_of(&request.review, request.pull_request.as_ref(), repo, card)
    }

    /// What every call explaining a review's functions repeats. Commits
    /// that cannot be read are left out rather than fail the step.
    fn context_of(
        &self,
        review: &ReviewRef,
        pull_request: Option<&PullRequestText>,
        repo: &Path,
        card: Option<Card>,
    ) -> Context {
        let commits = match review.new_sha.as_deref() {
            Some(head) => self
                .projects
                .git()
                .commit_messages(repo, &review.old_sha, head, MAX_COMMITS)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Context {
            target: Some(review.target.clone()),
            card,
            pull_request: pull_request.cloned(),
            commits,
        }
    }

    /// What a full analysis of the review would do and send, and to whom:
    /// each part to the model of its own task. Reads the card from Jira;
    /// asks no model and runs no test.
    pub fn plan(
        &self,
        project_id: &str,
        review: &ReviewRef,
        pull_request: Option<&PullRequestText>,
    ) -> AppResult<FullAnalysisPlan> {
        let repo = self.projects.repository(project_id)?;
        let session = ReviewSession::new(
            self.projects.store(),
            self.projects.git(),
            project_id,
            &repo,
            review.clone(),
        )?;
        let tests = self
            .tests
            .state(
                project_id,
                &repo,
                &review.old_sha,
                review.new_sha.as_deref(),
                Isolation::Sandbox,
            )?
            .unavailable
            .is_none();
        let model = self
            .models
            .in_use(self.projects.store(), ModelTask::BriefExplanation)?;
        let insights = self.insights.plan(&session, &self.models)?;
        // A card that cannot be read now is planned without; the summary
        // reads it again when it runs.
        let card = self.card_of(review, pull_request).ok().flatten();
        let summary = self
            .overview
            .plan(&session, &self.models, card.as_ref(), pull_request)?;
        let graph = match self.flow.flow(
            project_id,
            &repo,
            &review.old_sha,
            review.new_sha.as_deref(),
            EXPLAINED_GRAPH,
        )? {
            FlowResult::Graph { graph } => Some(*graph),
            FlowResult::Unavailable(_) => None,
        };
        let explanations = match graph {
            Some(graph) => {
                let context = self.context_of(review, pull_request, &repo, card);
                let planned = self.explanations.prepare(
                    &session,
                    &graph,
                    &Scope::Full,
                    ExplanationLevel::Brief,
                    &context,
                )?;
                self.explanations.plan(&planned, model)
            }
            None => ExplanationPlan {
                changed: 0,
                calls: Vec::new(),
                tokens: 0,
                model,
            },
        };
        Ok(FullAnalysisPlan {
            tests,
            insights,
            summary,
            explanations,
            limit: 0,
            running: 0,
        })
    }
}

/// How a step of calls ended: cancelled when a call did not start or was
/// stopped (or the job was), failed when one failed.
fn calls_end(cancelled: bool, ends: impl Iterator<Item = Option<Result<(), AppError>>>) -> StepEnd {
    let mut total = 0u32;
    let mut failed: Vec<AppError> = Vec::new();
    let mut stopped = false;
    for end in ends {
        total += 1;
        match end {
            None => stopped = true,
            Some(Err(error)) if error.code == ErrorCode::Cancelled => stopped = true,
            Some(Err(error)) => failed.push(error),
            Some(Ok(())) => {}
        }
    }
    if stopped || cancelled {
        return StepEnd::Cancelled;
    }
    match failed.len() {
        0 => StepEnd::Done,
        1 if total == 1 => StepEnd::Failed(failed.remove(0)),
        count => StepEnd::Failed(app_error!(
            Internal,
            CallsFailed,
            "{count} of {total} calls failed.",
            count = count,
            total = total
        )),
    }
}

fn changed(node: &FlowNode) -> bool {
    matches!(
        node.state,
        FlowNodeState::New | FlowNodeState::Modified | FlowNodeState::Deleted
    )
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure() -> AppError {
        app_error!(
            Internal,
            StepStoppedUnexpectedly,
            "The step stopped unexpectedly."
        )
    }

    #[test]
    fn a_step_of_calls_ends_by_how_its_calls_did() {
        assert_eq!(
            calls_end(false, [Some(Ok(())), Some(Ok(()))].into_iter()),
            StepEnd::Done
        );
        // One call alone keeps its own error.
        assert_eq!(
            calls_end(false, [Some(Err(failure()))].into_iter()),
            StepEnd::Failed(failure())
        );
        // Several say how many failed.
        let StepEnd::Failed(error) = calls_end(
            false,
            [Some(Ok(())), Some(Err(failure())), Some(Err(failure()))].into_iter(),
        ) else {
            panic!("expected a failure");
        };
        assert_eq!(error.reason, Some(Reason::CallsFailed));
        assert_eq!(error.message, "2 of 3 calls failed.");
        // A call not started, a stopped one, or a cancelled job is a cancel.
        assert_eq!(
            calls_end(false, [Some(Ok(())), None].into_iter()),
            StepEnd::Cancelled
        );
        let stopped: AppError = crate::ai::ModelError::Cancelled.into();
        assert_eq!(
            calls_end(false, [Some(Err(stopped))].into_iter()),
            StepEnd::Cancelled
        );
        assert_eq!(
            calls_end(true, [Some(Ok(()))].into_iter()),
            StepEnd::Cancelled
        );
    }
}
