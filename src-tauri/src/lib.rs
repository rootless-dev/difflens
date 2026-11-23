mod ai;
mod analysis;
mod analyzer_envs;
mod analyzers;
mod branches;
mod build_cache;
mod commands;
mod disk_cache;
mod duplication;
mod error;
mod explanations;
mod file_language;
mod fingerprint;
mod flow;
mod full_analysis;
mod flow_graph;
mod git;
mod http;
mod hunks;
mod insights;
mod jira;
mod language;
mod languages;
mod line_count;
mod model;
mod mutation;
mod mutation_runs;
mod overview;
mod parallel;
mod process;
mod projects;
mod pull_requests;
mod quality;
mod review;
mod review_state;
mod runner_protocol;
mod sandbox;
mod secrets;
mod signals;
mod single_flight;
mod store;
mod terminals;
mod test_diagnosis;
mod test_failures;
mod test_report;
mod test_runs;
#[cfg(test)]
mod test_support;
mod tool_path;
mod translation;
mod trees;
mod worktrees;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::Manager;

use crate::ai::ModelService;
use crate::analyzer_envs::{AnalyzerEnvs, ProvisionSettings, TauriProvisionEvents};
use crate::analyzers::{AnalyzerLocator, RunSettings};
use crate::commands::{
    SharedAnalyzerEnvs, SharedExplanationService, SharedFlowService, SharedInsightsService,
    SharedJiraService, SharedModelService, SharedMutationService, SharedOverviewService,
    SharedProjectService, SharedPullRequestService, SharedQualityService, SharedServiceSteps,
    SharedTerminals, SharedTestDiagnoses, SharedTestsService,
};
use crate::error::{app_error, AppResult};
use crate::explanations::ExplanationService;
use crate::flow::FlowService;
use crate::full_analysis::{Jobs, ServiceSteps, TauriEvents};
use crate::git::Git;
use crate::insights::InsightsService;
use crate::jira::JiraService;
use crate::mutation_runs::{MutationService, MutationSettings};
use crate::overview::OverviewService;
use crate::projects::ProjectService;
use crate::pull_requests::PullRequestService;
use crate::quality::QualityService;
use crate::store::Store;
use crate::terminals::{Launcher, Terminals};
use crate::test_diagnosis::TestDiagnoses;
use crate::test_runs::{TestSettings, TestsService};
use crate::tool_path::ToolPath;

const DATABASE_FILE: &str = "difflens.db";

fn build_project_service(
    data_directory: &Path,
    home: Option<PathBuf>,
) -> AppResult<SharedProjectService> {
    fs::create_dir_all(data_directory).map_err(|error| {
        app_error!(
            Internal,
            DataFolderFailed,
            "Could not create the data folder {path}: {detail}",
            path = data_directory.display(),
            detail = error
        )
    })?;
    let store = Store::open(&data_directory.join(DATABASE_FILE))?;
    // Registered paths are canonical, so the home folder must be too.
    let home = home.map(|home| fs::canonicalize(&home).unwrap_or(home));
    Ok(Arc::new(ProjectService::new(store, Git::new(), home)))
}

/// Sidecar analyzers are looked for next to the application executable,
/// where Tauri puts sidecars; provisioned ones are built by `envs` from the
/// sources in `resources` (the bundle's `analyzers/`), on first need. They
/// also run the tests and the static checkers.
fn build_analysis_services(
    cache_directory: &Path,
    data_directory: &Path,
    envs: SharedAnalyzerEnvs,
    resources: Option<PathBuf>,
) -> (SharedFlowService, SharedTestsService, SharedQualityService) {
    let sidecar_directory = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    // Build caches are kept with the app's data, like the environments: a
    // cache cleaner would make every project's next build start cold.
    let flow = Arc::new(
        FlowService::new(
            Git::new(),
            cache_directory.to_path_buf(),
            AnalyzerLocator::from_environment(sidecar_directory).with_provisioning(envs, resources),
            ToolPath::from_login_shell(),
            RunSettings::default(),
        )
        .with_build_caches(data_directory.join("build-cache")),
    );
    flow.warm_up();
    let tests = Arc::new(TestsService::new(
        Arc::clone(&flow),
        cache_directory.to_path_buf(),
        TestSettings::default(),
    ));
    let quality = Arc::new(QualityService::new(
        Arc::clone(&flow),
        Arc::clone(&tests),
        cache_directory.to_path_buf(),
    ));
    (flow, tests, quality)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Opens pull requests and Jira cards in the system browser.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_directory = app.path().app_data_dir()?;
            let home = app.path().home_dir().ok();
            let projects = build_project_service(&data_directory, home.clone())?;
            app.manage(Arc::clone(&projects));
            let cache_directory = app.path().app_cache_dir()?;
            // Environments are kept with the app's data, not its cache: they
            // take minutes to build and a cache cleaner would throw them away.
            let envs: SharedAnalyzerEnvs = Arc::new(AnalyzerEnvs::new(
                data_directory.join("analyzers"),
                Arc::new(TauriProvisionEvents(app.handle().clone())),
                ProvisionSettings::default(),
            ));
            app.manage(Arc::clone(&envs));
            let resources = app
                .path()
                .resource_dir()
                .ok()
                .map(|directory| directory.join("analyzers"));
            let (flow, tests, quality) =
                build_analysis_services(&cache_directory, &data_directory, envs, resources);
            app.manage(Arc::clone(&flow));
            app.manage(Arc::clone(&tests));
            app.manage(Arc::clone(&quality));
            // Only on a click in the Tests tab, never in a full analysis.
            let mutation: SharedMutationService = Arc::new(MutationService::new(
                Arc::clone(&flow),
                Arc::clone(&tests),
                cache_directory.clone(),
                MutationSettings::default(),
            ));
            app.manage(mutation);
            let pull_requests: SharedPullRequestService = Arc::new(PullRequestService::standard());
            app.manage(pull_requests);
            // Claude Code runs in an empty folder of its own, so no project's
            // settings, hooks or MCP servers apply to it.
            let models: SharedModelService = Arc::new(ModelService::standard(
                home,
                cache_directory.join("claude-code"),
            ));
            app.manage(Arc::clone(&models));
            let insights: SharedInsightsService = Arc::new(InsightsService::new());
            app.manage(Arc::clone(&insights));
            let jira: SharedJiraService = Arc::new(JiraService::standard());
            app.manage(Arc::clone(&jira));
            let overview: SharedOverviewService = Arc::new(OverviewService::new());
            app.manage(Arc::clone(&overview));
            let explanations: SharedExplanationService = Arc::new(ExplanationService::new());
            app.manage(Arc::clone(&explanations));
            let diagnoses: SharedTestDiagnoses = Arc::new(TestDiagnoses::new());
            app.manage(diagnoses);
            // Full analyses run in the core, with the services the tabs use;
            // they live as long as the app does.
            let limit = full_analysis::saved_limit(projects.store())?;
            let steps: SharedServiceSteps = Arc::new(ServiceSteps {
                projects,
                flow,
                tests,
                quality,
                models,
                insights,
                overview,
                jira,
                explanations,
            });
            app.manage(Arc::clone(&steps));
            app.manage(Jobs::new(
                steps,
                Arc::new(TauriEvents(app.handle().clone())),
                limit,
            ));
            let terminals: SharedTerminals = Arc::new(Terminals::new(
                Launcher::from_environment(),
                terminals::SCROLLBACK_BYTES,
            ));
            app.manage(terminals);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::add_local_project,
            commands::clone_project,
            commands::remove_project,
            commands::get_project,
            commands::fetch_project,
            commands::compare_branches,
            commands::compare_working_tree,
            commands::set_branch_base,
            commands::list_changed_files,
            commands::get_file_versions,
            commands::get_flow,
            commands::get_tests_state,
            commands::run_tests,
            commands::get_tests_progress,
            commands::cancel_tests,
            commands::get_mutation_state,
            commands::run_mutation,
            commands::get_mutation_progress,
            commands::cancel_mutation,
            commands::get_quality,
            commands::get_review_state,
            commands::set_file_reviewed,
            commands::set_function_reviewed,
            commands::add_comment,
            commands::add_general_comment,
            commands::draft_signal_comment,
            commands::reply_to_comment,
            commands::edit_comment,
            commands::delete_comment,
            commands::restore_comment,
            commands::set_comment_resolved,
            commands::get_connections,
            commands::test_connection,
            commands::save_connection,
            commands::disconnect,
            commands::link_pull_request,
            commands::publish_comments,
            commands::set_pull_request_verdict,
            commands::get_language_preference,
            commands::set_language_preference,
            commands::get_model_settings,
            commands::set_model_provider,
            commands::save_model_choice,
            commands::set_model_task,
            commands::set_model_key,
            commands::remove_model_key,
            commands::list_models,
            commands::test_model,
            commands::cancel_model_run,
            commands::get_insights,
            commands::plan_insights,
            commands::start_insights,
            commands::run_insights_batch,
            commands::set_finding_verdict,
            commands::set_finding_fixed,
            commands::write_finding_comment,
            commands::plan_insights_translation,
            commands::translate_insights,
            commands::get_jira_connection,
            commands::test_jira_connection,
            commands::save_jira_connection,
            commands::disconnect_jira,
            commands::find_card,
            commands::get_overview,
            commands::plan_overview,
            commands::summarize_overview,
            commands::plan_overview_translation,
            commands::translate_overview,
            commands::plan_full_analysis,
            commands::start_full_analysis,
            commands::cancel_full_analysis,
            commands::dismiss_full_analysis,
            commands::get_full_analyses,
            commands::get_full_analysis_limit,
            commands::set_full_analysis_limit,
            commands::get_model_in_use,
            commands::get_explanations,
            commands::explain_functions,
            commands::cancel_explanation,
            commands::plan_explanations_translation,
            commands::translate_explanations,
            commands::get_test_diagnoses,
            commands::diagnose_test_failure,
            commands::get_running_test_diagnoses,
            commands::join_test_diagnosis,
            commands::cancel_test_diagnosis,
            commands::write_review_comment,
            commands::plan_test_diagnoses_translation,
            commands::translate_test_diagnoses,
            commands::get_terminal_location,
            commands::exclude_difflens_folder,
            commands::create_worktree,
            commands::remove_worktree,
            commands::open_terminal,
            commands::attach_terminal,
            commands::write_terminal,
            commands::resize_terminal,
            commands::close_terminal,
            commands::list_terminals,
            commands::get_analyzer_provisions,
            commands::get_claude_command,
            commands::set_claude_command_mode,
            commands::save_claude_command,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Terminal sessions end with the app, processes included.
                if let Some(terminals) = app.try_state::<SharedTerminals>() {
                    terminals.close_all();
                }
                // An analyzer's build runs in process groups of its own.
                if let Some(envs) = app.try_state::<SharedAnalyzerEnvs>() {
                    envs.stop();
                }
            }
        });
}
