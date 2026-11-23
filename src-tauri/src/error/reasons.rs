//! Every message the core words, by the key the frontend's catalog
//! (`src/i18n/locales/<language>/errors.ts`, `errors.reasons.<key>`) gives
//! it. A test checks that the English catalog has exactly these keys.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! reasons {
    ($($(#[$doc:meta])* $variant:ident = $key:literal,)*) => {
        /// Which message an error or a `CoreText` is: finer than an
        /// `ErrorCode`, one per sentence the core composes.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Reason {
            $($(#[$doc])* $variant,)*
        }

        impl Reason {
            pub const ALL: &'static [Reason] = &[$(Reason::$variant,)*];

            /// The catalog key, such as "storage_failed".
            pub fn key(self) -> &'static str {
                match self {
                    $(Reason::$variant => $key,)*
                }
            }

            pub fn from_key(key: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|reason| reason.key() == key)
            }
        }
    };
}

reasons! {
    // Storage and the data folder.
    StorageFailed = "storage_failed",
    StorageUnavailable = "storage_unavailable",
    DatabaseTooNew = "database_too_new",
    DataFolderFailed = "data_folder_failed",
    ReviewNotRecorded = "review_not_recorded",
    StoredValueUnnamed = "stored_value_unnamed",
    StoredValueUnknown = "stored_value_unknown",
    StoreListFailed = "store_list_failed",
    StoredListUnreadable = "stored_list_unreadable",
    StoreSummaryFailed = "store_summary_failed",
    StoredSummaryUnreadable = "stored_summary_unreadable",
    KeychainOpenFailed = "keychain_open_failed",
    KeychainReadFailed = "keychain_read_failed",
    KeychainSaveFailed = "keychain_save_failed",
    KeychainDeleteFailed = "keychain_delete_failed",
    OperationInterrupted = "operation_interrupted",
    TimeFormatFailed = "time_format_failed",
    FileReadFailed = "file_read_failed",
    FileWriteFailed = "file_write_failed",
    ArchiveReadFailed = "archive_read_failed",
    /// A worded sentence that ends another one, with its full stop.
    Sentence = "sentence",

    // Projects.
    ProjectReadFailed = "project_read_failed",
    CountingCommitsFailed = "counting_commits_failed",
    CloneUrlInvalid = "clone_url_invalid",
    CloneDestinationNotAbsolute = "clone_destination_not_absolute",
    CloneDestinationExists = "clone_destination_exists",
    CloneFailed = "clone_failed",
    FetchFailed = "fetch_failed",
    FetchNoAnswer = "fetch_no_answer",
    NoRemote = "no_remote",
    PathNotFound = "path_not_found",
    PathGone = "path_gone",
    PathNotUnicode = "path_not_unicode",
    NotAGitRepository = "not_a_git_repository",
    NoLongerAGitRepository = "no_longer_a_git_repository",
    AlreadyAdded = "already_added",
    NoCommonHistory = "no_common_history",
    NotAnIntegrationBranch = "not_an_integration_branch",
    UnknownProject = "unknown_project",
    UnknownBranch = "unknown_branch",
    UnknownCommit = "unknown_commit",

    // git.
    GitNotFound = "git_not_found",
    GitTooOld = "git_too_old",
    GitRunFailed = "git_run_failed",
    GitWaitFailed = "git_wait_failed",
    GitUnexpectedOutput = "git_unexpected_output",
    GitNoDetails = "git_no_details",
    NotACommitId = "not_a_commit_id",
    NotAnObjectId = "not_an_object_id",
    GitListBranchesFailed = "git_list_branches_failed",
    GitWorkingTreeStatusFailed = "git_working_tree_status_failed",
    GitReadRemoteFailed = "git_read_remote_failed",
    GitCountCommitsFailed = "git_count_commits_failed",
    GitCompareCommitsFailed = "git_compare_commits_failed",
    GitListChangedFilesFailed = "git_list_changed_files_failed",
    GitListRenamedFilesFailed = "git_list_renamed_files_failed",
    GitListUntrackedFilesFailed = "git_list_untracked_files_failed",
    GitListWorkingTreeFilesFailed = "git_list_working_tree_files_failed",
    GitCompareSidesFailed = "git_compare_sides_failed",
    GitListRemotesFailed = "git_list_remotes_failed",
    GitListRemoteBranchesFailed = "git_list_remote_branches_failed",
    GitLookUpCommitsFailed = "git_look_up_commits_failed",
    GitLookUpDirectoriesFailed = "git_look_up_directories_failed",
    GitReadSizesFailed = "git_read_sizes_failed",
    GitReadFilesFailed = "git_read_files_failed",
    GitReadTreeFailed = "git_read_tree_failed",
    GitArchiveReadFailed = "git_archive_read_failed",
    GitArchiveEmpty = "git_archive_empty",
    GitReadCommitMessagesFailed = "git_read_commit_messages_failed",
    GitListWorktreesFailed = "git_list_worktrees_failed",
    GitReadCommonDirFailed = "git_read_common_dir_failed",
    GitAddWorktreeFailed = "git_add_worktree_failed",
    GitRemoveWorktreeFailed = "git_remove_worktree_failed",

    // Worktrees and terminals.
    BranchHasNoFolder = "branch_has_no_folder",
    WorktreeFolderTaken = "worktree_folder_taken",
    WorktreeBranchNotAFolder = "worktree_branch_not_a_folder",
    NotADifflensWorktree = "not_a_difflens_worktree",
    WorktreeDirty = "worktree_dirty",
    WorktreeHasTerminals = "worktree_has_terminals",
    UnknownTerminal = "unknown_terminal",
    TerminalStartFailed = "terminal_start_failed",
    TerminalResizeFailed = "terminal_resize_failed",
    ClaudeCommandEmpty = "claude_command_empty",
    ClaudeCommandNotOneLine = "claude_command_not_one_line",
    ClaudeCommandTooLong = "claude_command_too_long",

    // Reviews and comments.
    NotAChangedFile = "not_a_changed_file",
    PathOutsideRepository = "path_outside_repository",
    BlobMissing = "blob_missing",
    TooFewSidesForFile = "too_few_sides_for_file",
    TooFewSidesForFiles = "too_few_sides_for_files",
    SideHasNoText = "side_has_no_text",
    NoSuchLine = "no_such_line",
    CommentGone = "comment_gone",
    CommentNotYours = "comment_not_yours",
    CommentEmpty = "comment_empty",
    CommentTooLong = "comment_too_long",

    // Flow, tests and static checks.
    AnalyzerTitle = "analyzer_title",
    AnalyzerNotInstalled = "analyzer_not_installed",
    AnalyzerOverrideNotAFile = "analyzer_override_not_a_file",
    AnalyzerUnreadable = "analyzer_unreadable",
    AnalyzerRunFailed = "analyzer_run_failed",
    AnalyzerTimedOut = "analyzer_timed_out",
    AnalyzerOutputTooLarge = "analyzer_output_too_large",
    AnalyzerStopped = "analyzer_stopped",
    AnalyzerFailed = "analyzer_failed",
    AnalyzerNoDetails = "analyzer_no_details",
    // Preparing a provisioned analyzer's environment (`analyzer_envs`).
    AnalyzerSourceMissing = "analyzer_source_missing",
    AnalyzerRuntimeMissing = "analyzer_runtime_missing",
    RuntimePythonHint = "runtime_python_hint",
    RuntimeNodeHint = "runtime_node_hint",
    RuntimeCargoHint = "runtime_cargo_hint",
    AnalyzerPrepareFailed = "analyzer_prepare_failed",
    AnalyzerPrepareTimedOut = "analyzer_prepare_timed_out",
    AnalyzerPrepareStopped = "analyzer_prepare_stopped",
    AnalyzerPrepareDiskFailed = "analyzer_prepare_disk_failed",
    AnalyzerPrepareAbandoned = "analyzer_prepare_abandoned",
    ProtocolMismatch = "protocol_mismatch",
    InvalidAnalysis = "invalid_analysis",
    InvalidPrepareReport = "invalid_prepare_report",
    InvalidTestReport = "invalid_test_report",
    InvalidCheckReport = "invalid_check_report",
    AnalysisWrongLanguage = "analysis_wrong_language",
    FunctionLinesInvalid = "function_lines_invalid",
    FunctionValueBelowOne = "function_value_below_one",
    CallOnLineZero = "call_on_line_zero",
    DiagnosticOnLineZero = "diagnostic_on_line_zero",
    TestOnLineZero = "test_on_line_zero",
    TestHierarchyMismatch = "test_hierarchy_mismatch",
    ChecksWithoutDependencies = "checks_without_dependencies",
    CoverageBlockLinesInvalid = "coverage_block_lines_invalid",
    FindingOnLineZero = "finding_on_line_zero",
    SandboxUnavailable = "sandbox_unavailable",
    ProgramMissing = "program_missing",
    PrepareFailed = "prepare_failed",
    NoDetailsReported = "no_details_reported",
    PreparePrintedTooMuch = "prepare_printed_too_much",
    RunnerFailedQuietly = "runner_failed_quietly",
    NoAnalyzerForTests = "no_analyzer_for_tests",
    OldSideUnanalysed = "old_side_unanalysed",
    NewSideUnanalysed = "new_side_unanalysed",
    AnalysisAbandoned = "analysis_abandoned",
    NoTestRunner = "no_test_runner",
    NoTestRunnerNoLanguages = "no_test_runner_no_languages",
    TestRunAbandoned = "test_run_abandoned",
    TestRunBusy = "test_run_busy",
    StaticChecksAbandoned = "static_checks_abandoned",
    InvalidDuplicateReport = "invalid_duplicate_report",
    DuplicateLinesInvalid = "duplicate_lines_invalid",
    DuplicateWithoutOthers = "duplicate_without_others",
    DuplicateSearchAbandoned = "duplicate_search_abandoned",
    DuplicateInputFailed = "duplicate_input_failed",
    InvalidMutationReport = "invalid_mutation_report",
    MutantPositionInvalid = "mutant_position_invalid",
    MutantIdRepeated = "mutant_id_repeated",
    MutationNeedsCoverage = "mutation_needs_coverage",
    MutationNothingCovered = "mutation_nothing_covered",
    MutationRunAbandoned = "mutation_run_abandoned",
    MutationRunBusy = "mutation_run_busy",

    // Connections to Bitbucket, GitHub and Jira.
    AccountEmailMissing = "account_email_missing",
    AtlassianEmailMissing = "atlassian_email_missing",
    TokenMissing = "token_missing",
    TokenMissingForTest = "token_missing_for_test",
    UnknownIntegration = "unknown_integration",
    TransportClientFailed = "transport_client_failed",
    TransportReadFailed = "transport_read_failed",
    TransportTimeout = "transport_timeout",
    TransportConnectFailed = "transport_connect_failed",
    HostUnreachable = "host_unreachable",
    HostCredentialRefused = "host_credential_refused",
    HostRateLimited = "host_rate_limited",
    HostRateLimitedRetryAfter = "host_rate_limited_retry_after",
    HostServerError = "host_server_error",
    HostRefusedSaid = "host_refused_said",
    HostRefusedStatus = "host_refused_status",
    HostUnreadableAnswer = "host_unreadable_answer",
    HostTooManyPages = "host_too_many_pages",
    HostPageOutsideApi = "host_page_outside_api",
    ScopesDiffLensNeeds = "scopes_diff_lens_needs",
    ScopesThisNeeds = "scopes_this_needs",
    BitbucketMissingScope = "bitbucket_missing_scope",
    BitbucketMissingScopeSaid = "bitbucket_missing_scope_said",
    BitbucketAccountNotFound = "bitbucket_account_not_found",
    BitbucketRepositoryMissing = "bitbucket_repository_missing",
    BitbucketPullRequestMissing = "bitbucket_pull_request_missing",
    BitbucketReplyTargetMissing = "bitbucket_reply_target_missing",
    BitbucketVerdictTargetMissing = "bitbucket_verdict_target_missing",
    NotABitbucketCommentId = "not_a_bitbucket_comment_id",
    GithubTokenRefused = "github_token_refused",
    GithubPermissionsNeeded = "github_permissions_needed",
    GithubMissingPermission = "github_missing_permission",
    GithubMissingPermissionSaid = "github_missing_permission_said",
    GithubAccountNotFound = "github_account_not_found",
    GithubRepositoryMissing = "github_repository_missing",
    GithubPullRequestMissing = "github_pull_request_missing",
    GithubReplyTargetMissing = "github_reply_target_missing",
    GithubVerdictTargetMissing = "github_verdict_target_missing",
    GithubTakeBackRefused = "github_take_back_refused",
    GithubPendingReviewExists = "github_pending_review_exists",
    GithubReviewIncomplete = "github_review_incomplete",
    GithubReviewNotDeletable = "github_review_not_deletable",
    GithubThreadNotResolvable = "github_thread_not_resolvable",
    NotAGithubCommentId = "not_a_github_comment_id",
    RemoteCommentGone = "remote_comment_gone",
    JiraSiteMissing = "jira_site_missing",
    JiraSiteInvalid = "jira_site_invalid",
    JiraSiteNotFound = "jira_site_not_found",
    JiraCardMissing = "jira_card_missing",
    JiraScopesNeeded = "jira_scopes_needed",
    JiraTokenMissingScope = "jira_token_missing_scope",
    JiraForbidden = "jira_forbidden",
    JiraForbiddenSaid = "jira_forbidden_said",

    // Pull requests and publishing.
    CommentPullRequestUnknown = "comment_pull_request_unknown",
    NoPullRequestForWorkingTree = "no_pull_request_for_working_tree",
    NoPullRequestWithoutOrigin = "no_pull_request_without_origin",
    UnsupportedHost = "unsupported_host",
    HostNotConnected = "host_not_connected",
    NoOpenPullRequest = "no_open_pull_request",
    PullRequestNotOpen = "pull_request_not_open",
    PublishBaseMismatch = "publish_base_mismatch",
    PublishMergeBaseDiffers = "publish_merge_base_differs",
    PublishDestinationMissing = "publish_destination_missing",
    PublishLocalAhead = "publish_local_ahead",
    PublishLocalBehind = "publish_local_behind",
    PublishDiverged = "publish_diverged",
    PublishCommitMissing = "publish_commit_missing",
    NotSent = "not_sent",
    NotSentParentFailed = "not_sent_parent_failed",

    // The model.
    ProviderTheApi = "provider_the_api",
    ProviderTheApiCapitalized = "provider_the_api_capitalized",
    UnknownModelProvider = "unknown_model_provider",
    ProviderNeedsNoKey = "provider_needs_no_key",
    ApiKeyMissing = "api_key_missing",
    ApiKeyNotSaved = "api_key_not_saved",
    ModelMissing = "model_missing",
    ModelNotSetUp = "model_not_set_up",
    TaskModelMissing = "task_model_missing",
    TaskProviderNotSetUp = "task_provider_not_set_up",
    /// How a sentence names a task.
    TaskInsights = "task_insights",
    TaskSummary = "task_summary",
    TaskBriefExplanation = "task_brief_explanation",
    TaskDetailedExplanation = "task_detailed_explanation",
    TaskTestDiagnosis = "task_test_diagnosis",
    TaskReviewComment = "task_review_comment",
    TaskTranslation = "task_translation",
    ClaudeCodeSignedOut = "claude_code_signed_out",
    ClaudeCodeSignedOutSaid = "claude_code_signed_out_said",
    ModelKeyRefused = "model_key_refused",
    ModelKeyRefusedSaid = "model_key_refused_said",
    ModelRateLimited = "model_rate_limited",
    ModelRateLimitedSaid = "model_rate_limited_said",
    ModelRateLimitedRetryAfter = "model_rate_limited_retry_after",
    ModelRateLimitedRetryAfterSaid = "model_rate_limited_retry_after_said",
    ModelRefusedQuietly = "model_refused_quietly",
    ModelRefusedCategory = "model_refused_category",
    ModelRefusedExplained = "model_refused_explained",
    ModelRefusedCategoryExplained = "model_refused_category_explained",
    ModelTruncated = "model_truncated",
    ModelInvalidOutput = "model_invalid_output",
    ModelUnreachable = "model_unreachable",
    ModelProviderRefused = "model_provider_refused",
    ModelProviderRefusedStatus = "model_provider_refused_status",
    ClaudeCliMissing = "claude_cli_missing",
    ClaudeCodeFailed = "claude_code_failed",
    Stopped = "stopped",
    ModelCallAbandoned = "model_call_abandoned",
    RunIdMissing = "run_id_missing",
    RunAlreadyGoing = "run_already_going",
    RunListUnusable = "run_list_unusable",
    ProviderUrlInvalid = "provider_url_invalid",
    ProviderUrlUnusable = "provider_url_unusable",
    ProviderHadAProblem = "provider_had_a_problem",
    ProviderGaveNoExplanation = "provider_gave_no_explanation",
    ProviderAnswerNotJson = "provider_answer_not_json",
    ResponseFailed = "response_failed",
    UpstreamProviderFailed = "upstream_provider_failed",
    UpstreamProviderFailedQuietly = "upstream_provider_failed_quietly",
    AnswerEmpty = "answer_empty",
    AnswerNotJson = "answer_not_json",
    AnswerNotAnObject = "answer_not_an_object",
    AnswerLacksFields = "answer_lacks_fields",
    NoAnswerReturned = "no_answer_returned",
    StoppedWith = "stopped_with",
    StoppedEarly = "stopped_early",
    OutputLimitReached = "output_limit_reached",
    OutputLimitWithThinking = "output_limit_with_thinking",
    OutputLimitWithReasoning = "output_limit_with_reasoning",
    OwnOutputLimitReached = "own_output_limit_reached",
    ContextWindowFilled = "context_window_filled",
    CliNotStarted = "cli_not_started",
    CliTimedOut = "cli_timed_out",
    CliPrintedTooMuch = "cli_printed_too_much",
    CliExited = "cli_exited",
    CliKilled = "cli_killed",
    CliReportedError = "cli_reported_error",
    NoJsonResult = "no_json_result",
    SchemaNotMatched = "schema_not_matched",
    NoStructuredAnswer = "no_structured_answer",

    // Insights and the overview.
    NothingToSend = "nothing_to_send",
    AnalysisReplaced = "analysis_replaced",
    FindingGone = "finding_gone",
    InsightsUnavailable = "insights_unavailable",
    AnalysisProviderUnknown = "analysis_provider_unknown",
    SummaryProviderUnknown = "summary_provider_unknown",
    SummaryIncomplete = "summary_incomplete",

    // Explanations of functions.
    ExplanationNamesNone = "explanation_names_none",
    ExplanationProviderUnknown = "explanation_provider_unknown",

    // Explanations of failed tests.
    DiagnosisFailureGone = "diagnosis_failure_gone",
    DiagnosisNoCause = "diagnosis_no_cause",
    DiagnosisUnknownFault = "diagnosis_unknown_fault",
    DiagnosisProviderUnknown = "diagnosis_provider_unknown",
    DiagnosisGone = "diagnosis_gone",
    ReviewCommentNoBody = "review_comment_no_body",

    // Drafts written from a finding or a static check.
    SignalUnknown = "signal_unknown",

    // Full analyses.
    StepStoppedUnexpectedly = "step_stopped_unexpectedly",
    TestRunFailedWith = "test_run_failed_with",
    TestRunFailedQuietly = "test_run_failed_quietly",
    TestRunTimedOut = "test_run_timed_out",
    CallsFailed = "calls_failed",

    // Translating what the model wrote.
    TranslationNamesNone = "translation_names_none",
    TranslationIncomplete = "translation_incomplete",
    TranslationLacksCardSummary = "translation_lacks_card_summary",
    TranslationLacksCriterion = "translation_lacks_criterion",
    TranslationLacksPoints = "translation_lacks_points",
    TranslationNamesNoFunction = "translation_names_no_function",
    TranslationNamesNoDiagnosis = "translation_names_no_diagnosis",
    NoSummaryToTranslate = "no_summary_to_translate",
}

impl Serialize for Reason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

/// An unknown key (a reason a later version wrote) fails, so what holds it
/// is read as text without a reason.
impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Reason::from_key(&key)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown reason {key:?}")))
    }
}
