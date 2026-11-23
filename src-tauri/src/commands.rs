//! Tauri command layer: one command per function of `src/lib/api.ts`. Each
//! command only moves the work off the async runtime and delegates to
//! `ProjectService`, the analysis services, `review_state`,
//! `PullRequestService`, `JiraService`, `ModelService`, `InsightsService`,
//! `OverviewService`, `ExplanationService`, the full analyses (`Jobs`) or the
//! terminal sessions (`Terminals`), because git, SQLite, host and model calls
//! block.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::ai::ModelService;
use crate::analyzer_envs::AnalyzerEnvs;
use crate::error::{app_error, AppResult};
use crate::explanations::{self, ExplanationService};
use crate::flow::FlowService;
use crate::full_analysis::{self, JobRequest, Jobs, ServiceSteps};
use crate::insights::InsightsService;
use crate::jira::JiraService;
use crate::language::{self, Language, LanguagePreference};
use crate::mutation_runs::MutationService;
use crate::model::{
    Account, AnalyzerProvision, Branch, CardLookup, ChangeSet, CompareSummary, Connection, Effort, ExplanationLevel,
    Explanations, FetchResult, FileVersions, FindingVerdict, FlowOptions, FlowResult,
    FullAnalysisJob, FullAnalysisPlan, FunctionSpan, Insights, InsightsPlan, Isolation,
    JiraConnection,
    ModelInUse, ModelInfo, ModelKeySaved, ModelSettings, ModelTask, ModelTaskChoice, ModelTest,
    MutationProgress, MutationRun, MutationState, Overview, OverviewPlan,
    Project, ProjectDetail, PublishReport, PullRequestLink, PullRequestText, QualityResult,
    ReviewRef, ReviewSide, ReviewState, ReviewTarget, RunOptions, RunSelection, RunningDiagnosis,
    SignalDraft, TerminalEvent, TerminalKind, TerminalLocation, TerminalSession, TestDiagnosis,
    TestProgress, TestRun, TestsState, TranslationPlan,
    Verdict, WorkingTreeSummary, WrittenReviewComment,
};
use crate::overview::OverviewService;
use crate::projects::ProjectService;
use crate::pull_requests::PullRequestService;
use crate::quality::QualityService;
use crate::review_state::{self, ReviewSession};
use crate::signals;
use crate::terminals::{
    self, ClaudeCommandMode, ClaudeCommandSetting, OpenRequest, TerminalSink, Terminals,
};
use crate::test_diagnosis::{self, TestDiagnoses};
use crate::test_runs::{RunRequest, TestsService};

pub type SharedProjectService = Arc<ProjectService>;
pub type SharedFlowService = Arc<FlowService>;
pub type SharedTestsService = Arc<TestsService>;
pub type SharedMutationService = Arc<MutationService>;
pub type SharedQualityService = Arc<QualityService>;
pub type SharedPullRequestService = Arc<PullRequestService>;
pub type SharedModelService = Arc<ModelService>;
pub type SharedInsightsService = Arc<InsightsService>;
pub type SharedJiraService = Arc<JiraService>;
pub type SharedOverviewService = Arc<OverviewService>;
pub type SharedExplanationService = Arc<ExplanationService>;
pub type SharedTestDiagnoses = Arc<TestDiagnoses>;
pub type SharedServiceSteps = Arc<ServiceSteps>;
pub type SharedTerminals = Arc<Terminals>;
pub type SharedAnalyzerEnvs = Arc<AnalyzerEnvs>;

async fn run_blocking<T, F>(service: &State<'_, SharedProjectService>, work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&ProjectService) -> AppResult<T> + Send + 'static,
{
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(|error| {
            app_error!(
                Internal,
                OperationInterrupted,
                "The operation was interrupted: {detail}",
                detail = error
            )
        })?
}

#[tauri::command]
pub async fn list_projects(service: State<'_, SharedProjectService>) -> AppResult<Vec<Project>> {
    run_blocking(&service, |service| service.list()).await
}

#[tauri::command]
pub async fn add_local_project(
    service: State<'_, SharedProjectService>,
    path: String,
) -> AppResult<Project> {
    run_blocking(&service, move |service| service.add_local(&path)).await
}

#[tauri::command]
pub async fn clone_project(
    service: State<'_, SharedProjectService>,
    url: String,
    destination: String,
) -> AppResult<Project> {
    run_blocking(&service, move |service| {
        service.clone_repository(&url, &destination)
    })
    .await
}

#[tauri::command]
pub async fn remove_project(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    id: String,
) -> AppResult<()> {
    let flow = Arc::clone(flow.inner());
    run_blocking(&service, move |service| {
        service.remove(&id)?;
        // What the project's builds kept between runs goes with it; one
        // that cannot be deleted is left behind rather than failing.
        let _ = flow.build_caches().remove_project(&id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn get_project(
    service: State<'_, SharedProjectService>,
    id: String,
) -> AppResult<ProjectDetail> {
    run_blocking(&service, move |service| service.detail(&id)).await
}

#[tauri::command]
pub async fn compare_branches(
    service: State<'_, SharedProjectService>,
    project_id: String,
    branch: String,
    base: String,
) -> AppResult<CompareSummary> {
    run_blocking(&service, move |service| {
        service.compare(&project_id, &branch, &base)
    })
    .await
}

#[tauri::command]
pub async fn fetch_project(
    service: State<'_, SharedProjectService>,
    id: String,
) -> AppResult<FetchResult> {
    run_blocking(&service, move |service| service.fetch(&id)).await
}

#[tauri::command]
pub async fn compare_working_tree(
    service: State<'_, SharedProjectService>,
    project_id: String,
) -> AppResult<WorkingTreeSummary> {
    run_blocking(&service, move |service| {
        service.compare_working_tree(&project_id)
    })
    .await
}

#[tauri::command]
pub async fn list_changed_files(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    project_id: String,
    target: ReviewTarget,
) -> AppResult<ChangeSet> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        // A branch with a pull request compares with what the pull request
        // compares with; the host is only read.
        let pull_request_base = match &target {
            ReviewTarget::Branch { branch, base } => {
                let repo = service.repository(&project_id)?;
                pull_requests.base_commit(service.store(), service.git(), &repo, branch, base)
            }
            ReviewTarget::WorkingTree => None,
        };
        service.changed_files_against(&project_id, target, pull_request_base.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn get_file_versions(
    service: State<'_, SharedProjectService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
    path: String,
    old_path: Option<String>,
) -> AppResult<FileVersions> {
    run_blocking(&service, move |service| {
        service.file_versions(
            &project_id,
            &old_sha,
            new_sha.as_deref(),
            &path,
            old_path.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn get_flow(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
    options: FlowOptions,
) -> AppResult<FlowResult> {
    let flow = Arc::clone(flow.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        flow.flow(&project_id, &repo, &old_sha, new_sha.as_deref(), options)
    })
    .await
}

#[tauri::command]
pub async fn get_tests_state(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
) -> AppResult<TestsState> {
    let tests = Arc::clone(tests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let isolation = service.test_isolation(&project_id)?;
        tests.state(&project_id, &repo, &old_sha, new_sha.as_deref(), isolation)
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn run_tests(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
    isolation: Isolation,
    options: Option<RunOptions>,
    selection: Option<RunSelection>,
) -> AppResult<TestRun> {
    let tests = Arc::clone(tests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        service.set_test_isolation(&project_id, isolation)?;
        let request = RunRequest {
            isolation,
            options: options.unwrap_or_default(),
            selection,
        };
        tests.run_with(&project_id, &repo, &old_sha, new_sha.as_deref(), &request)
    })
    .await
}

#[tauri::command]
pub async fn get_tests_progress(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    project_id: String,
    new_sha: Option<String>,
) -> AppResult<Option<TestProgress>> {
    let tests = Arc::clone(tests.inner());
    run_blocking(&service, move |_| Ok(tests.progress(&project_id, new_sha.as_deref()))).await
}

#[tauri::command]
pub async fn cancel_tests(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
) -> AppResult<()> {
    // The old side does not matter to a run.
    let _ = old_sha;
    let tests = Arc::clone(tests.inner());
    run_blocking(&service, move |_| {
        tests.cancel(&project_id, new_sha.as_deref());
        Ok(())
    })
    .await
}

/// The last mutation testing run of the review, whether one goes on, and
/// why none can run now. Runs nothing but analyzers.
#[tauri::command]
pub async fn get_mutation_state(
    service: State<'_, SharedProjectService>,
    mutation: State<'_, SharedMutationService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
) -> AppResult<MutationState> {
    let mutation = Arc::clone(mutation.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        mutation.state(&project_id, &repo, &old_sha, new_sha.as_deref())
    })
    .await
}

/// Runs mutation testing on the lines the review adds that the last test
/// run with coverage executed, isolated as the project's tests are. Only on
/// a click: it can take minutes.
#[tauri::command]
pub async fn run_mutation(
    service: State<'_, SharedProjectService>,
    mutation: State<'_, SharedMutationService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
) -> AppResult<MutationRun> {
    let mutation = Arc::clone(mutation.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let isolation = service.test_isolation(&project_id)?;
        mutation.run(&project_id, &repo, &old_sha, new_sha.as_deref(), isolation)
    })
    .await
}

#[tauri::command]
pub async fn get_mutation_progress(
    service: State<'_, SharedProjectService>,
    mutation: State<'_, SharedMutationService>,
    project_id: String,
    new_sha: Option<String>,
) -> AppResult<Option<MutationProgress>> {
    let mutation = Arc::clone(mutation.inner());
    run_blocking(&service, move |_| Ok(mutation.progress(&project_id, new_sha.as_deref()))).await
}

#[tauri::command]
pub async fn cancel_mutation(
    service: State<'_, SharedProjectService>,
    mutation: State<'_, SharedMutationService>,
    project_id: String,
    new_sha: Option<String>,
) -> AppResult<()> {
    let mutation = Arc::clone(mutation.inner());
    run_blocking(&service, move |_| {
        mutation.cancel(&project_id, new_sha.as_deref());
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn get_quality(
    service: State<'_, SharedProjectService>,
    quality: State<'_, SharedQualityService>,
    project_id: String,
    old_sha: String,
    new_sha: Option<String>,
) -> AppResult<QualityResult> {
    let quality = Arc::clone(quality.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        quality.quality(&project_id, &repo, &old_sha, new_sha.as_deref())
    })
    .await
}

/// Runs `work` on the review `review` of a project that is still available.
fn with_review<T>(
    service: &ProjectService,
    project_id: &str,
    review: ReviewRef,
    work: impl FnOnce(&ReviewSession<'_>) -> AppResult<T>,
) -> AppResult<T> {
    let repo = service.repository(project_id)?;
    work(&ReviewSession::new(
        service.store(),
        service.git(),
        project_id,
        &repo,
        review,
    )?)
}

#[tauri::command]
pub async fn get_review_state(
    service: State<'_, SharedProjectService>,
    project_id: String,
    review: ReviewRef,
    functions: Vec<FunctionSpan>,
) -> AppResult<ReviewState> {
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            session.state(&functions)
        })
    })
    .await
}

#[tauri::command]
pub async fn set_file_reviewed(
    service: State<'_, SharedProjectService>,
    project_id: String,
    review: ReviewRef,
    path: String,
    reviewed: bool,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            session.set_file_reviewed(&path, reviewed)
        })
    })
    .await
}

#[tauri::command]
pub async fn set_function_reviewed(
    service: State<'_, SharedProjectService>,
    project_id: String,
    review: ReviewRef,
    function: FunctionSpan,
    reviewed: bool,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            session.set_function_reviewed(&function, reviewed)
        })
    })
    .await
}

#[tauri::command]
pub async fn add_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    review: ReviewRef,
    path: String,
    side: ReviewSide,
    line: u32,
    body: String,
) -> AppResult<String> {
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            session.add_comment(&path, side, line, &body)
        })
    })
    .await
}

/// Comments the pull request as a whole (a general comment, on no file or
/// line); a draft until published, like a line comment.
#[tauri::command]
pub async fn add_general_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    review: ReviewRef,
    body: String,
) -> AppResult<String> {
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            session.add_general_comment(&body)
        })
    })
    .await
}

/// Writes a draft review comment from a model finding or a static check
/// finding (its own text, no model), on its line when the review adds or
/// changes it, on the pull request as a whole otherwise; returns the one
/// written before while it is there.
#[tauri::command]
pub async fn draft_signal_comment(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    project_id: String,
    review: ReviewRef,
    request: signals::DraftRequest,
    language: Language,
) -> AppResult<SignalDraft> {
    let flow = Arc::clone(flow.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let changed = flow.changed_lines(&repo, &review.old_sha, review.new_sha.as_deref())?;
        with_review(service, &project_id, review, |session| {
            signals::draft(session, &changed, &request, language)
        })
    })
    .await
}

#[tauri::command]
pub async fn reply_to_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    parent_id: String,
    body: String,
) -> AppResult<String> {
    run_blocking(&service, move |service| {
        review_state::reply(service.store(), &project_id, &parent_id, &body)
    })
    .await
}

#[tauri::command]
pub async fn edit_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    comment_id: String,
    body: String,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        review_state::edit_comment(service.store(), &project_id, &comment_id, &body)
    })
    .await
}

#[tauri::command]
pub async fn delete_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    comment_id: String,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        review_state::delete_comment(service.store(), &project_id, &comment_id)
    })
    .await
}

#[tauri::command]
pub async fn restore_comment(
    service: State<'_, SharedProjectService>,
    project_id: String,
    comment_id: String,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        review_state::restore_comment(service.store(), &project_id, &comment_id)
    })
    .await
}

/// A thread already on the pull request is resolved there too, at once.
#[tauri::command]
pub async fn set_comment_resolved(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    project_id: String,
    comment_id: String,
    resolved: bool,
) -> AppResult<()> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        pull_requests.set_resolved(
            service.store(),
            service.git(),
            &project_id,
            &repo,
            &comment_id,
            resolved,
        )
    })
    .await
}

#[tauri::command]
pub async fn get_connections(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
) -> AppResult<Vec<Connection>> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        pull_requests.connections(service.store())
    })
    .await
}

#[tauri::command]
pub async fn test_connection(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    host: String,
    username: String,
    token: Option<String>,
) -> AppResult<Account> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        pull_requests.test_connection(service.store(), &host, &username, token.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn save_connection(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    host: String,
    username: String,
    token: String,
) -> AppResult<Connection> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        pull_requests.save_connection(service.store(), &host, &username, &token)
    })
    .await
}

#[tauri::command]
pub async fn disconnect(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    host: String,
) -> AppResult<()> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        pull_requests.disconnect(service.store(), &host)
    })
    .await
}

#[tauri::command]
pub async fn link_pull_request(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<PullRequestLink> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        pull_requests.link(service.store(), service.git(), &project_id, &repo, &review)
    })
    .await
}

/// Approves or requests changes (`given` true), or takes either back; answers
/// the pull request as it is afterwards.
#[tauri::command]
pub async fn set_pull_request_verdict(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    project_id: String,
    review: ReviewRef,
    verdict: Verdict,
    given: bool,
) -> AppResult<PullRequestLink> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        pull_requests.set_verdict(
            service.store(),
            service.git(),
            &project_id,
            &repo,
            &review,
            verdict,
            given,
        )
    })
    .await
}

#[tauri::command]
pub async fn publish_comments(
    service: State<'_, SharedProjectService>,
    pull_requests: State<'_, SharedPullRequestService>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<PublishReport> {
    let pull_requests = Arc::clone(pull_requests.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        pull_requests.publish(service.store(), service.git(), &project_id, &repo, &review)
    })
    .await
}

#[tauri::command]
pub async fn get_language_preference(
    service: State<'_, SharedProjectService>,
) -> AppResult<LanguagePreference> {
    run_blocking(&service, |service| language::language_preference(service.store())).await
}

#[tauri::command]
pub async fn set_language_preference(
    service: State<'_, SharedProjectService>,
    preference: LanguagePreference,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        language::set_language_preference(service.store(), preference)
    })
    .await
}

#[tauri::command]
pub async fn get_model_settings(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
) -> AppResult<ModelSettings> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| models.settings(service.store())).await
}

#[tauri::command]
pub async fn set_model_provider(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    provider: String,
) -> AppResult<ModelSettings> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.set_provider(service.store(), &provider)
    })
    .await
}

#[tauri::command]
pub async fn save_model_choice(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    provider: String,
    model: String,
    effort: Option<Effort>,
) -> AppResult<ModelSettings> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.save_choice(service.store(), &provider, &model, effort)
    })
    .await
}

/// Gives `task` a model of its own, or (`None`) puts it back on the default.
#[tauri::command]
pub async fn set_model_task(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    task: ModelTask,
    choice: Option<ModelTaskChoice>,
) -> AppResult<ModelSettings> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.set_task(service.store(), task, choice)
    })
    .await
}

/// The key arrives once, here; it never goes back to the frontend.
#[tauri::command]
pub async fn set_model_key(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    provider: String,
    key: String,
) -> AppResult<ModelKeySaved> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.set_key(service.store(), &provider, &key)
    })
    .await
}

#[tauri::command]
pub async fn remove_model_key(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    provider: String,
) -> AppResult<ModelSettings> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.remove_key(service.store(), &provider)
    })
    .await
}

#[tauri::command]
pub async fn list_models(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    provider: String,
) -> AppResult<Vec<ModelInfo>> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |_| models.list_models(&provider)).await
}

/// Asks the provider in use a tiny question; `run_id` lets
/// `cancel_model_run` stop it.
#[tauri::command]
pub async fn test_model(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    run_id: String,
) -> AppResult<ModelTest> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.test(service.store(), &run_id)
    })
    .await
}

#[tauri::command]
pub async fn cancel_model_run(
    models: State<'_, SharedModelService>,
    run_id: String,
) -> AppResult<()> {
    models.cancel(&run_id);
    Ok(())
}

/// The findings of the review and its latest analysis. Asks no model.
#[tauri::command]
pub async fn get_insights(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.insights(session)
        })
    })
    .await
}

/// What Analyze would send, in how many calls, and to whom. Asks no model.
#[tauri::command]
pub async fn plan_insights(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<InsightsPlan> {
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.plan(session, &models)
        })
    })
    .await
}

/// Records a new analysis and its calls. Asks no model: the frontend runs
/// each call with `run_insights_batch`.
#[tauri::command]
pub async fn start_insights(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.start(session, &models, language)
        })
    })
    .await
}

/// What translating the findings into `language` would send. Asks no model.
#[tauri::command]
pub async fn plan_insights_translation(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
) -> AppResult<TranslationPlan> {
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.plan_translation(session, &models, language)
        })
    })
    .await
}

/// Asks the model to translate the text of some findings, only after a
/// click on Translate; `run_id` lets `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn translate_insights(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
    finding_ids: Vec<String>,
    run_id: String,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.translate(session, &models, language, &finding_ids, &run_id)
        })
    })
    .await
}

/// Asks the model one call of the latest analysis; `run_id` lets
/// `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn run_insights_batch(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    analysis_id: String,
    batch: u32,
    run_id: String,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.run_batch(session, &models, &analysis_id, batch, &run_id)
        })
    })
    .await
}

#[tauri::command]
pub async fn set_finding_verdict(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    project_id: String,
    review: ReviewRef,
    finding_id: String,
    verdict: FindingVerdict,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.set_verdict(session, &finding_id, verdict)
        })
    })
    .await
}

/// Asks the model of the Insights task, only after a click on Write comment,
/// to write a short review comment from the finding `finding_id` in
/// `language`, and keeps it as a draft linked to the finding: on the last of
/// its lines the review adds or changes, on the pull request as a whole
/// otherwise. While that draft is there, returns it without a call.
/// `run_id` lets `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn write_finding_comment(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    insights: State<'_, SharedInsightsService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    finding_id: String,
    language: Language,
    run_id: String,
) -> AppResult<SignalDraft> {
    let flow = Arc::clone(flow.inner());
    let insights = Arc::clone(insights.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let changed = flow.changed_lines(&repo, &review.old_sha, review.new_sha.as_deref())?;
        with_review(service, &project_id, review, |session| {
            insights.write_comment(session, &models, &changed, &finding_id, language, &run_id)
        })
    })
    .await
}

/// Marks a finding fixed by the user, or opens it again. Asks no model.
#[tauri::command]
pub async fn set_finding_fixed(
    service: State<'_, SharedProjectService>,
    insights: State<'_, SharedInsightsService>,
    project_id: String,
    review: ReviewRef,
    finding_id: String,
    fixed: bool,
) -> AppResult<Insights> {
    let insights = Arc::clone(insights.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            insights.set_fixed(session, &finding_id, fixed)
        })
    })
    .await
}

#[tauri::command]
pub async fn get_jira_connection(
    service: State<'_, SharedProjectService>,
    jira: State<'_, SharedJiraService>,
) -> AppResult<JiraConnection> {
    let jira = Arc::clone(jira.inner());
    run_blocking(&service, move |service| jira.connection(service.store())).await
}

#[tauri::command]
pub async fn test_jira_connection(
    service: State<'_, SharedProjectService>,
    jira: State<'_, SharedJiraService>,
    site: String,
    email: String,
    token: Option<String>,
) -> AppResult<Account> {
    let jira = Arc::clone(jira.inner());
    run_blocking(&service, move |service| {
        jira.test_connection(service.store(), &site, &email, token.as_deref())
    })
    .await
}

/// The token arrives once, here; it never goes back to the frontend.
#[tauri::command]
pub async fn save_jira_connection(
    service: State<'_, SharedProjectService>,
    jira: State<'_, SharedJiraService>,
    site: String,
    email: String,
    token: String,
) -> AppResult<JiraConnection> {
    let jira = Arc::clone(jira.inner());
    run_blocking(&service, move |service| {
        jira.save_connection(service.store(), &site, &email, &token)
    })
    .await
}

#[tauri::command]
pub async fn disconnect_jira(
    service: State<'_, SharedProjectService>,
    jira: State<'_, SharedJiraService>,
) -> AppResult<()> {
    let jira = Arc::clone(jira.inner());
    run_blocking(&service, move |service| jira.disconnect(service.store())).await
}

/// The card of a review target, read from Jira (only read), or why there is
/// none.
#[tauri::command]
pub async fn find_card(
    service: State<'_, SharedProjectService>,
    jira: State<'_, SharedJiraService>,
    target: ReviewTarget,
    pull_request_title: Option<String>,
) -> AppResult<CardLookup> {
    let jira = Arc::clone(jira.inner());
    run_blocking(&service, move |service| {
        jira.find_card(service.store(), &target, pull_request_title.as_deref())
    })
    .await
}

/// The stored summary and what is derived from the change. Asks no model
/// and not Jira.
#[tauri::command]
pub async fn get_overview(
    service: State<'_, SharedProjectService>,
    overview: State<'_, SharedOverviewService>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<Overview> {
    let overview = Arc::clone(overview.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            overview.overview(session)
        })
    })
    .await
}

/// What Summarize would send, and to whom. Reads the card from Jira; asks no
/// model.
#[tauri::command]
pub async fn plan_overview(
    service: State<'_, SharedProjectService>,
    overview: State<'_, SharedOverviewService>,
    jira: State<'_, SharedJiraService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    pull_request: Option<PullRequestText>,
) -> AppResult<OverviewPlan> {
    let overview = Arc::clone(overview.inner());
    let jira = Arc::clone(jira.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        let title = pull_request
            .as_ref()
            .map(|pull_request| pull_request.title.as_str());
        let card = JiraService::card_of(jira.find_card(service.store(), &review.target, title)?);
        with_review(service, &project_id, review, |session| {
            overview.plan(session, &models, card.as_ref(), pull_request.as_ref())
        })
    })
    .await
}

/// Reads the card from Jira, then asks the model for the summary; `run_id`
/// lets `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn summarize_overview(
    service: State<'_, SharedProjectService>,
    overview: State<'_, SharedOverviewService>,
    jira: State<'_, SharedJiraService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    pull_request: Option<PullRequestText>,
    run_id: String,
    language: Language,
) -> AppResult<Overview> {
    let overview = Arc::clone(overview.inner());
    let jira = Arc::clone(jira.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        let title = pull_request
            .as_ref()
            .map(|pull_request| pull_request.title.as_str());
        let card = JiraService::card_of(jira.find_card(service.store(), &review.target, title)?);
        with_review(service, &project_id, review, |session| {
            overview.summarize(
                session,
                &models,
                card.as_ref(),
                pull_request.as_ref(),
                language,
                &run_id,
            )
        })
    })
    .await
}

/// What translating the summary into `language` would send. Asks no model
/// and not Jira.
#[tauri::command]
pub async fn plan_overview_translation(
    service: State<'_, SharedProjectService>,
    overview: State<'_, SharedOverviewService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
) -> AppResult<TranslationPlan> {
    let overview = Arc::clone(overview.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            overview.plan_translation(session, &models, language)
        })
    })
    .await
}

/// Asks the model to translate the text of the summary, only after a click
/// on Translate; `run_id` lets `cancel_model_run` stop it.
#[tauri::command]
pub async fn translate_overview(
    service: State<'_, SharedProjectService>,
    overview: State<'_, SharedOverviewService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
    run_id: String,
) -> AppResult<Overview> {
    let overview = Arc::clone(overview.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            overview.translate(session, &models, language, &run_id)
        })
    })
    .await
}

/// What a full analysis of the review would do and send, and to whom. Reads
/// the card from Jira; asks no model and runs no test.
#[tauri::command]
pub async fn plan_full_analysis(
    service: State<'_, SharedProjectService>,
    steps: State<'_, SharedServiceSteps>,
    jobs: State<'_, Jobs>,
    project_id: String,
    review: ReviewRef,
    pull_request: Option<PullRequestText>,
) -> AppResult<FullAnalysisPlan> {
    let steps = Arc::clone(steps.inner());
    let jobs = jobs.inner().clone();
    run_blocking(&service, move |_| {
        let mut plan = steps.plan(&project_id, &review, pull_request.as_ref())?;
        plan.limit = jobs.limit();
        plan.running = jobs.running();
        Ok(plan)
    })
    .await
}

/// Starts a full analysis of the review in the core, after the user
/// confirmed it, or queues it; written in `language`.
#[tauri::command]
pub async fn start_full_analysis(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    jobs: State<'_, Jobs>,
    project_id: String,
    review: ReviewRef,
    pull_request: Option<PullRequestText>,
    language: Language,
) -> AppResult<FullAnalysisJob> {
    let models = Arc::clone(models.inner());
    let jobs = jobs.inner().clone();
    run_blocking(&service, move |service| {
        // The review must still be there, and the model of each of its
        // tasks ready.
        with_review(service, &project_id, review.clone(), |_| Ok(()))?;
        models.ensure_ready(service.store(), &full_analysis::TASKS)?;
        let project_name = service
            .repository(&project_id)?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(jobs.start(JobRequest {
            project_id,
            project_name,
            review,
            pull_request,
            language,
        }))
    })
    .await
}

#[tauri::command]
pub async fn cancel_full_analysis(jobs: State<'_, Jobs>, job_id: String) -> AppResult<()> {
    jobs.cancel(&job_id);
    Ok(())
}

/// Forgets a finished full analysis: its review shows its tabs again.
#[tauri::command]
pub async fn dismiss_full_analysis(jobs: State<'_, Jobs>, job_id: String) -> AppResult<()> {
    jobs.dismiss(&job_id);
    Ok(())
}

/// Every full analysis of this run of the app, with its whole log.
#[tauri::command]
pub async fn get_full_analyses(jobs: State<'_, Jobs>) -> AppResult<Vec<FullAnalysisJob>> {
    Ok(jobs.list())
}

/// The analyzer environments being built now (`analyzer_envs`), so a view
/// that opens during a build can show it; later changes come as
/// `analyzer-provision` events.
#[tauri::command]
pub async fn get_analyzer_provisions(
    envs: State<'_, SharedAnalyzerEnvs>,
) -> AppResult<Vec<AnalyzerProvision>> {
    Ok(envs.running())
}

#[tauri::command]
pub async fn get_full_analysis_limit(jobs: State<'_, Jobs>) -> AppResult<u32> {
    Ok(jobs.limit())
}

/// Keeps how many full analyses may run at once; queued ones start when
/// there is room now.
#[tauri::command]
pub async fn set_full_analysis_limit(
    service: State<'_, SharedProjectService>,
    jobs: State<'_, Jobs>,
    limit: u32,
) -> AppResult<u32> {
    let jobs = jobs.inner().clone();
    run_blocking(&service, move |service| {
        let limit = full_analysis::save_limit(service.store(), limit)?;
        Ok(jobs.set_limit(limit))
    })
    .await
}

/// Who would be asked now for `task`, for the tooltips of the buttons that
/// ask a model. Asks no model; for Claude Code it runs `claude --version`.
#[tauri::command]
pub async fn get_model_in_use(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    task: ModelTask,
) -> AppResult<ModelInUse> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        models.in_use(service.store(), task)
    })
    .await
}

/// The review's explanations of functions, read against `functions` as the
/// graph has them now. Asks no model.
#[tauri::command]
pub async fn get_explanations(
    service: State<'_, SharedProjectService>,
    explanations: State<'_, SharedExplanationService>,
    project_id: String,
    review: ReviewRef,
    functions: Vec<FunctionSpan>,
) -> AppResult<Explanations> {
    let explanations = Arc::clone(explanations.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            explanations.explanations(session, &functions)
        })
    })
    .await
}

/// Asks the model to explain the functions `function_ids` at `level`, only
/// after a click on a node's or a group's Explain button (short) or on a
/// function's Explain in detail (detail); `run_id` lets `cancel_explanation`
/// stop it. Reads the card from Jira.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn explain_functions(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    jira: State<'_, SharedJiraService>,
    models: State<'_, SharedModelService>,
    explanations: State<'_, SharedExplanationService>,
    project_id: String,
    review: ReviewRef,
    function_ids: Vec<String>,
    level: ExplanationLevel,
    pull_request: Option<PullRequestText>,
    language: Language,
    run_id: String,
) -> AppResult<Explanations> {
    let flow = Arc::clone(flow.inner());
    let jira = Arc::clone(jira.inner());
    let models = Arc::clone(models.inner());
    let explanations = Arc::clone(explanations.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let all = FlowOptions {
            include_tests: true,
            include_callers: true,
            include_callees: true,
        };
        let FlowResult::Graph { graph } = flow.flow(
            &project_id,
            &repo,
            &review.old_sha,
            review.new_sha.as_deref(),
            all,
        )?
        else {
            return Ok(Explanations::none());
        };
        let title = pull_request
            .as_ref()
            .map(|pull_request| pull_request.title.as_str());
        let card = jira
            .find_card(service.store(), &review.target, title)
            .ok()
            .and_then(JiraService::card_of);
        let commits = match review.new_sha.as_deref() {
            Some(head) => service
                .git()
                .commit_messages(&repo, &review.old_sha, head, explanations::MAX_COMMITS)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let context = explanations::Context {
            target: Some(review.target.clone()),
            card,
            pull_request,
            commits,
        };
        with_review(service, &project_id, review, |session| {
            explanations.explain(
                session,
                &models,
                &graph,
                &function_ids,
                level,
                &context,
                language,
                &run_id,
            )
        })
    })
    .await
}

#[tauri::command]
pub async fn cancel_explanation(
    explanations: State<'_, SharedExplanationService>,
    run_id: String,
) -> AppResult<()> {
    explanations.cancel(&run_id);
    Ok(())
}

/// What translating the review's explanations into `language` would send.
/// Asks no model.
#[tauri::command]
pub async fn plan_explanations_translation(
    service: State<'_, SharedProjectService>,
    explanations: State<'_, SharedExplanationService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
) -> AppResult<TranslationPlan> {
    let explanations = Arc::clone(explanations.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            explanations.plan_translation(session, &models, language)
        })
    })
    .await
}

/// Asks the model to translate some explanations, only after a click on
/// Translate; `run_id` lets `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn translate_explanations(
    service: State<'_, SharedProjectService>,
    explanations: State<'_, SharedExplanationService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
    function_ids: Vec<String>,
    functions: Vec<FunctionSpan>,
    run_id: String,
) -> AppResult<Explanations> {
    let explanations = Arc::clone(explanations.inner());
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            explanations.translate(
                session,
                &models,
                language,
                &function_ids,
                &functions,
                &run_id,
            )
        })
    })
    .await
}

/// The last run of the review's new side and the new side's functions in its
/// language, for the explanations of failed tests, with the calls prepared
/// for them kept in `diagnoses`. Runs nothing but analyzers (cached), and
/// derives nothing the Tests tab (or an earlier read) already derived for
/// the same run and sides.
fn with_failures<T>(
    service: &ProjectService,
    tests: &TestsService,
    diagnoses: &TestDiagnoses,
    project_id: &str,
    review: ReviewRef,
    work: impl FnOnce(&ReviewSession<'_>, test_diagnosis::Facts<'_>) -> AppResult<T>,
) -> AppResult<T> {
    let repo = service.repository(project_id)?;
    let derived = tests.last_run(project_id, &repo, &review.old_sha, review.new_sha.as_deref())?;
    let functions = derived
        .as_ref()
        .and_then(|derived| derived.new_side.as_ref())
        .map_or(&[][..], |analysis| &analysis.functions[..]);
    let kept = derived
        .as_ref()
        .and_then(|derived| derived.key.as_ref())
        .map(|key| test_diagnosis::KeptInputs {
            memo: &diagnoses.inputs,
            key,
        });
    with_review(service, project_id, review, |session| {
        work(
            session,
            test_diagnosis::Facts {
                run: derived.as_ref().map(|derived| &*derived.run),
                functions,
                kept,
            },
        )
    })
}

/// The review's explanations of failed tests, read against the last run.
/// Asks no model.
#[tauri::command]
pub async fn get_test_diagnoses(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<Vec<TestDiagnosis>> {
    let tests = Arc::clone(tests.inner());
    let diagnoses = Arc::clone(diagnoses.inner());
    run_blocking(&service, move |service| {
        with_failures(service, &tests, &diagnoses, &project_id, review, |session, facts| {
            test_diagnosis::diagnoses(session, facts)
        })
    })
    .await
}

/// Asks the model why the failing test `test` of `package` (or the package
/// itself, `test` None) failed in the last run, only after a click on
/// Explain the failure, once the model has room for one more call; joins
/// the call already going on for that failure instead, from any screen.
/// `run_id` lets `cancel_model_run` stop it once it asks, and
/// `cancel_test_diagnosis` stops it queued or asking.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn diagnose_test_failure(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    models: State<'_, SharedModelService>,
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
    package: String,
    test: Option<String>,
    language: Language,
    run_id: String,
) -> AppResult<TestDiagnosis> {
    let tests = Arc::clone(tests.inner());
    let models = Arc::clone(models.inner());
    let diagnoses = Arc::clone(diagnoses.inner());
    run_blocking(&service, move |service| {
        let subject = test_diagnosis::Subject { package, test };
        let key = test_diagnosis::CallKey::new(&project_id, &review.target, &subject.key());
        let parallel_calls = || {
            models
                .in_use(service.store(), ModelTask::TestDiagnosis)
                .map_or(1, |model| model.parallel_calls)
        };
        diagnoses.calls.explain(key, &subject, parallel_calls, |cancel| {
            with_failures(service, &tests, &diagnoses, &project_id, review, |session, facts| {
                test_diagnosis::diagnose(
                    session, &models, facts, &subject, language, &run_id, cancel,
                )
            })
        })
    })
    .await
}

/// The explanations of the review's failures being written, from this
/// screen or one left since. Asks no model.
#[tauri::command]
pub async fn get_running_test_diagnoses(
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
) -> AppResult<Vec<RunningDiagnosis>> {
    Ok(diagnoses.calls.running(&project_id, &review.target))
}

/// Waits for the explanation of the failure `key` being written and returns
/// it; None when none is being written (it ended before: read the kept
/// ones). Never asks the model.
#[tauri::command]
pub async fn join_test_diagnosis(
    service: State<'_, SharedProjectService>,
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
    key: String,
) -> AppResult<Option<TestDiagnosis>> {
    let diagnoses = Arc::clone(diagnoses.inner());
    run_blocking(&service, move |_| {
        let key = test_diagnosis::CallKey::new(&project_id, &review.target, &key);
        diagnoses.calls.join(&key).transpose()
    })
    .await
}

/// Stops the explanation of the failure `key` being written, queued or
/// asking the model; it then fails with `cancelled`.
#[tauri::command]
pub async fn cancel_test_diagnosis(
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
    key: String,
) -> AppResult<()> {
    let key = test_diagnosis::CallKey::new(&project_id, &review.target, &key);
    diagnoses.calls.cancel(&key);
    Ok(())
}

/// Asks the model of the review_comment task to write, from the explanation
/// of the failure `key`, a review comment for the author of the change, only
/// after a click on Write review comment, and keeps it as a draft: on the
/// line the explanation names when the review changes it, on the pull
/// request as a whole otherwise. The last run tells it the failed tests and
/// which of the code the failure points at the review edits. `run_id` lets
/// `cancel_model_run` stop it.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn write_review_comment(
    service: State<'_, SharedProjectService>,
    flow: State<'_, SharedFlowService>,
    tests: State<'_, SharedTestsService>,
    models: State<'_, SharedModelService>,
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
    key: String,
    language: Language,
    run_id: String,
) -> AppResult<WrittenReviewComment> {
    let flow = Arc::clone(flow.inner());
    let tests = Arc::clone(tests.inner());
    let models = Arc::clone(models.inner());
    let diagnoses = Arc::clone(diagnoses.inner());
    run_blocking(&service, move |service| {
        let repo = service.repository(&project_id)?;
        let changed = flow.changed_lines(&repo, &review.old_sha, review.new_sha.as_deref())?;
        with_failures(service, &tests, &diagnoses, &project_id, review, |session, facts| {
            test_diagnosis::write_review_comment(
                session, &models, facts, &changed, &key, language, &run_id,
            )
        })
    })
    .await
}

/// What translating the explanations of failures `keys` into `language`
/// would send. Asks no model.
#[tauri::command]
pub async fn plan_test_diagnoses_translation(
    service: State<'_, SharedProjectService>,
    models: State<'_, SharedModelService>,
    project_id: String,
    review: ReviewRef,
    language: Language,
    keys: Vec<String>,
) -> AppResult<TranslationPlan> {
    let models = Arc::clone(models.inner());
    run_blocking(&service, move |service| {
        with_review(service, &project_id, review, |session| {
            test_diagnosis::plan_translation(session, &models, language, &keys)
        })
    })
    .await
}

/// Asks the model to translate some explanations of failures, only after a
/// click on Translate; `run_id` lets `cancel_model_run` stop it.
// Each argument is one the frontend passes by name, plus the services.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn translate_test_diagnoses(
    service: State<'_, SharedProjectService>,
    tests: State<'_, SharedTestsService>,
    models: State<'_, SharedModelService>,
    diagnoses: State<'_, SharedTestDiagnoses>,
    project_id: String,
    review: ReviewRef,
    language: Language,
    keys: Vec<String>,
    run_id: String,
) -> AppResult<Vec<TestDiagnosis>> {
    let tests = Arc::clone(tests.inner());
    let models = Arc::clone(models.inner());
    let diagnoses = Arc::clone(diagnoses.inner());
    run_blocking(&service, move |service| {
        with_failures(service, &tests, &diagnoses, &project_id, review, |session, facts| {
            test_diagnosis::translate(session, &models, facts, language, &keys, &run_id)
        })
    })
    .await
}

#[tauri::command]
pub async fn set_branch_base(
    service: State<'_, SharedProjectService>,
    project_id: String,
    branch: String,
    base: Option<String>,
) -> AppResult<Branch> {
    run_blocking(&service, move |service| {
        service.set_branch_base(&project_id, &branch, base.as_deref())
    })
    .await
}

/// A session's channel: its output as raw bytes (an `ArrayBuffer` in the
/// frontend), its other events as JSON.
struct ChannelSink(Channel<InvokeResponseBody>);

impl TerminalSink for ChannelSink {
    fn output(&self, bytes: &[u8]) {
        // A view that went away (a reload) loses what it was sent; the
        // scrollback still has it for the next one.
        let _ = self.0.send(InvokeResponseBody::Raw(bytes.to_vec()));
    }

    fn event(&self, event: &TerminalEvent) {
        if let Ok(json) = serde_json::to_string(event) {
            let _ = self.0.send(InvokeResponseBody::Json(json));
        }
    }
}

#[tauri::command]
pub async fn get_terminal_location(
    service: State<'_, SharedProjectService>,
    project_id: String,
    target: Option<ReviewTarget>,
) -> AppResult<TerminalLocation> {
    run_blocking(&service, move |service| {
        service.terminal_location(&project_id, target.as_ref())
    })
    .await
}

#[tauri::command]
pub async fn exclude_difflens_folder(
    service: State<'_, SharedProjectService>,
    project_id: String,
) -> AppResult<()> {
    run_blocking(&service, move |service| {
        service.exclude_difflens_folder(&project_id)
    })
    .await
}

#[tauri::command]
pub async fn create_worktree(
    service: State<'_, SharedProjectService>,
    project_id: String,
    branch: String,
) -> AppResult<TerminalLocation> {
    run_blocking(&service, move |service| {
        service.create_worktree(&project_id, &branch)
    })
    .await
}

#[tauri::command]
pub async fn remove_worktree(
    service: State<'_, SharedProjectService>,
    terminals: State<'_, SharedTerminals>,
    project_id: String,
    path: String,
    force: bool,
) -> AppResult<()> {
    let terminals = Arc::clone(terminals.inner());
    run_blocking(&service, move |service| {
        service.remove_worktree(&project_id, &path, force, |folder| {
            terminals.count_in(folder)
        })
    })
    .await
}

/// Opens a session where `target`'s terminals open (the project folder for
/// the project screen, when `target` is absent). The folder is worked out
/// here, never taken from the frontend.
#[tauri::command]
pub async fn open_terminal(
    service: State<'_, SharedProjectService>,
    terminals: State<'_, SharedTerminals>,
    project_id: String,
    target: Option<ReviewTarget>,
    kind: TerminalKind,
    output: Channel<InvokeResponseBody>,
) -> AppResult<TerminalSession> {
    let terminals = Arc::clone(terminals.inner());
    run_blocking(&service, move |service| {
        let location = service.terminal_location(&project_id, target.as_ref())?;
        let Some(folder) = location.folder else {
            return Err(app_error!(
                NoFolder,
                BranchHasNoFolder,
                "{branch} is not checked out in any folder yet.",
                branch = location.branch.unwrap_or_default()
            ));
        };
        let project_name = service.project_name(&project_id)?;
        terminals.open(
            OpenRequest {
                kind,
                cwd: PathBuf::from(folder.path),
                project_id,
                project_name,
                target,
                branch: location.branch,
            },
            service.store(),
            Arc::new(ChannelSink(output)),
        )
    })
    .await
}

/// What Claude Code sessions run: the default or a custom command, and the
/// custom command saved.
#[tauri::command]
pub async fn get_claude_command(
    service: State<'_, SharedProjectService>,
) -> AppResult<ClaudeCommandSetting> {
    run_blocking(&service, |service| {
        terminals::claude_command_setting(service.store())
    })
    .await
}

/// Chooses the default or the custom command for the sessions opened from
/// now on; the custom command stays saved either way.
#[tauri::command]
pub async fn set_claude_command_mode(
    service: State<'_, SharedProjectService>,
    mode: ClaudeCommandMode,
) -> AppResult<ClaudeCommandSetting> {
    run_blocking(&service, move |service| {
        terminals::set_claude_command_mode(service.store(), mode)
    })
    .await
}

/// Saves `command` as the custom command and chooses it. Fails when it is
/// empty, too long or not a single line.
#[tauri::command]
pub async fn save_claude_command(
    service: State<'_, SharedProjectService>,
    command: String,
) -> AppResult<ClaudeCommandSetting> {
    run_blocking(&service, move |service| {
        terminals::save_custom_claude_command(service.store(), &command)
    })
    .await
}

/// Points the session's output at `output`, which with `replay` first
/// receives its scrollback.
#[tauri::command]
pub async fn attach_terminal(
    terminals: State<'_, SharedTerminals>,
    id: String,
    replay: bool,
    output: Channel<InvokeResponseBody>,
) -> AppResult<TerminalSession> {
    terminals.attach(&id, Arc::new(ChannelSink(output)), replay)
}

#[tauri::command]
pub async fn write_terminal(
    terminals: State<'_, SharedTerminals>,
    id: String,
    data: String,
) -> AppResult<()> {
    terminals.write(&id, data.into_bytes())
}

#[tauri::command]
pub async fn resize_terminal(
    terminals: State<'_, SharedTerminals>,
    id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    terminals.resize(&id, cols, rows)
}

/// Ends the session; its processes get a short grace after the hang-up.
#[tauri::command]
pub async fn close_terminal(terminals: State<'_, SharedTerminals>, id: String) -> AppResult<()> {
    let terminals = Arc::clone(terminals.inner());
    tauri::async_runtime::spawn_blocking(move || terminals.close(&id))
        .await
        .map_err(|error| {
            app_error!(
                Internal,
                OperationInterrupted,
                "The operation was interrupted: {detail}",
                detail = error
            )
        })?
}

#[tauri::command]
pub async fn list_terminals(
    terminals: State<'_, SharedTerminals>,
) -> AppResult<Vec<TerminalSession>> {
    Ok(terminals.list())
}
