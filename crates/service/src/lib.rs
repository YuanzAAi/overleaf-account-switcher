pub mod projects;
pub mod resources;
pub mod runtime;

// Preserve the existing module paths for library consumers.
pub use accounts::{
    actions as account_actions, browser as account_browser, credentials as account_credentials,
    git_token as account_git_token, io as account_io, password_change as account_password_change,
    password_policy, registration as account_registration, secrets as account_secrets,
    session as account_session, switch as account_switch,
};
pub use projects::{cleanup as project_cleanup, migration as project_migration};
pub use resources::{addresses as address_actions, cards as card_actions};
pub use runtime::{
    batch as browser_batch, dashboard, health, profiles as browser_profile, tasks as task_state,
    ServiceConfig,
};

pub mod api;
pub use account_actions::{
    plan_account_removals, remove_accounts_locally, remove_accounts_locally_in_store,
    update_local_passwords_in_store_with_backend, update_local_passwords_with_backend,
    AccountActionError, AccountRemovalAction, AccountRemovalItem, AccountRemovalReport,
    LocalPasswordUpdateItem, LocalPasswordUpdateReport,
};
pub use account_browser::{
    open_account_browser_login_with_backend, open_account_browser_logins_in_store_with_backend,
    BrowserAutoLoginBatchItem, BrowserAutoLoginBatchReport, BrowserAutoLoginError,
    BrowserAutoLoginReport,
};
pub use account_credentials::{
    add_account_from_login_result_in_store_with_backend,
    add_account_from_login_result_with_backend, add_account_with_credentials_in_store_with_backend,
    add_account_with_credentials_with_backend,
    refresh_account_cookie_from_login_result_in_store_with_backend,
    refresh_account_cookie_from_login_result_with_backend,
    refresh_account_cookie_with_credentials_in_store_with_backend,
    refresh_account_cookie_with_credentials_with_backend, AccountCredentialError,
    AccountSessionMetadataRefresher, CredentialLoginReport, CredentialLoginStatus,
    CredentialMetadataStatus, ReqwestAccountSessionMetadataRefresher,
};
pub use account_git_token::{
    apply_git_token_page_state_with_backend,
    generate_account_git_token_with_browser_in_store_with_backend,
    generate_account_git_token_with_browser_with_backend, has_reusable_git_token,
    refresh_account_git_token_metadata_in_store_with_backend,
    refresh_account_git_token_metadata_with_backend,
    refresh_account_git_token_metadata_with_browser_batch_in_store_with_backend,
    refresh_account_git_token_metadata_with_browser_in_store_with_backend,
    refresh_account_git_token_metadata_with_browser_with_backend,
    refresh_saved_git_token_metadata_in_store_with_backend, AccountGitTokenError,
    AccountGitTokenRefreshBatchItem, AccountGitTokenRefreshBatchReport,
    AccountGitTokenRefreshReport, GitTokenRefreshStatus, GIT_TOKEN_REUSE_WINDOW_SECONDS,
};
pub use account_password_change::{
    change_account_overleaf_password_from_login_result_in_store_with_backend,
    change_account_overleaf_password_from_login_result_with_backend,
    change_account_overleaf_password_in_store_with_backend,
    change_account_overleaf_password_with_backend, OverleafPasswordChangeError,
    OverleafPasswordChangeReport, OverleafPasswordChangeStatus,
};
pub use account_registration::{
    ensure_registration_result_artifacts, register_account_with_subscription_in_store_with_backend,
    save_existing_trial_result_in_store_with_backend,
    save_registration_result_in_store_with_backend,
    save_registration_result_without_password_in_store_with_backend, AccountRegistrationError,
    AccountRegistrationInput, AccountRegistrationReport, AccountRegistrationStatus,
};
pub use account_secrets::{
    read_account_cookie_secret, read_account_git_token_secret, read_account_password_secret,
    read_account_password_secret_for_recovery, resolve_account_cookies,
    resolve_account_cookies_for_recovery, write_account_cookie_secrets,
    write_account_git_token_secret, write_account_password_secret, AccountSecretStoreError,
};
pub use account_session::{
    inspect_account_trial_eligibility, inspect_saved_account_trial_eligibility_with_backend,
    refresh_account_session_metadata, refresh_saved_account_sessions_in_store_with_backend,
    validate_session_identity, AccountSessionError, AccountSessionIdentityValidator,
    AccountSessionRefreshBatchItem, AccountSessionRefreshBatchReport, AccountSessionRefreshReport,
    AccountTrialEligibilityBatchItem, AccountTrialEligibilityBatchReport,
    AccountTrialEligibilityReport, ReqwestAccountSessionIdentityValidator,
    SubscriptionRefreshStatus,
};
pub use account_switch::{
    execute_account_switch_in_store,
    execute_account_switch_with_project_migration_controlled_in_store,
    execute_account_switch_with_project_migration_in_store, plan_account_switch,
    plan_account_switch_in_store, AccountSwitchCommandExecutor, AccountSwitchCommandSummary,
    AccountSwitchError, AccountSwitchExecutionError, AccountSwitchExecutionReport,
    AccountSwitchExtensionPlan, AccountSwitchPlanReport, AccountSwitchResponseSummary,
};
pub use address_actions::{
    add_or_select_address_in_store, address_action_error_message,
    address_from_meiguodizhi_response, fetch_address_into_store, list_address_summaries,
    meiguodizhi_request_payload, select_address_by_index, select_fallback_address,
    select_fallback_address_in_store, select_registration_address, AddressActionError,
    AddressFetchReport, AddressFetchSource, AddressSelection, AddressSummary,
    RegistrationAddressProvider, RegistrationAddressSelection, RegistrationAddressSource,
    ReqwestMeiguodizhiAddressProvider,
};
pub use api::{
    handle_api_request_with_body_mut, reconcile_browser_sessions, ApiBackgroundJob, ApiErrorBody,
    ApiResponse, ApiState, ServiceApiRouteSpec, ServiceConfigBody, HEALTH_PATH, SERVICE_API_ROUTES,
};
pub use password_policy::{
    validate_new_password, NewPasswordTooShort, MINIMUM_NEW_PASSWORD_LENGTH,
};
pub mod accounts;
pub use account_io::{
    apply_account_import_with_backend, export_accounts_to_directory_with_backend,
    export_accounts_to_json_with_backend, import_accounts_from_json_file_with_backend,
    manual_cookie_import_candidate, parse_account_import_json, preview_account_import,
    validate_manual_cookie_import_candidates, AccountExportMode, AccountExportReport,
    AccountExportedFile, AccountImportCandidate, AccountImportDecision,
    AccountImportDecisionStatus, AccountImportReport, AccountIoError, ManualCookieValidationItem,
    ManualCookieValidationReport,
};
pub use accounts::{
    account_secret_for_copy_with_backend, list_account_summaries_with_backend, AccountSecretError,
    AccountSecretField, AccountSummary, ExpiringSecretStatus, SecretPresence, TimeStatus,
};
pub use browser_batch::{
    BrowserBatchCoordinator, BrowserBatchError, BrowserBatchItemLease, BrowserBatchItemPhase,
    BrowserBatchItemSnapshot, BrowserBatchSnapshot, DEFAULT_BROWSER_BATCH_CONCURRENCY,
};
pub use browser_profile::{
    detect_browser_profile_account, detect_browser_profile_account_in_store,
    BrowserProfileAccountError, BrowserProfileAccountReport, BrowserProfileSessionInspector,
    ReqwestBrowserProfileSessionInspector,
};
pub use card_actions::{
    add_card_from_line_in_store_with_backend, card_for_payment_with_backend, list_card_summaries,
    mark_card_failed_in_store_with_backend, mark_card_used_in_store_with_backend,
    remove_card_in_store_with_backend, select_payment_card_with_strategy_and_bin_with_backend,
    select_payment_card_with_strategy_and_bin_with_backend_at,
    select_payment_card_with_strategy_with_backend,
    select_payment_card_with_strategy_with_backend_at, CardActionError, CardAddDecisionStatus,
    CardAddReport, CardMutationReport, CardPaymentSecret, CardSelectionStrategy, CardSummary,
};
pub use dashboard::{dashboard_summary_with_backend, DashboardRuntimeState, DashboardSummary};
pub use project_cleanup::{
    cleanup_remote_projects_for_account, cleanup_remote_projects_in_store,
    cleanup_remote_projects_then_remove_account,
    cleanup_remote_projects_then_remove_account_in_store,
    cleanup_remote_projects_then_remove_account_with_reqwest_in_store,
    cleanup_remote_projects_with_reqwest_in_store, preview_remote_project_cleanup_for_account,
    preview_remote_project_cleanup_with_reqwest_in_store, CleanupThenRemoveAccountError,
    CleanupThenRemoveAccountReport, CleanupThenRemoveStatus, RemoteProjectCleanupAction,
    RemoteProjectCleanupError, RemoteProjectCleanupExecutor, RemoteProjectCleanupItem,
    RemoteProjectCleanupPreviewItem, RemoteProjectCleanupPreviewReport, RemoteProjectCleanupReport,
    RemoteProjectCleanupStatus, ReqwestRemoteProjectCleanupExecutor,
};
pub use project_migration::{
    migrate_projects_between_clients, migrate_projects_between_clients_controlled,
    migrate_projects_with_reqwest_in_store, migrate_projects_with_reqwest_in_store_controlled,
    preview_project_migrations_between_clients, preview_project_migrations_with_reqwest_in_store,
    ProjectMigrationControl, ProjectMigrationError, ProjectMigrationExecutor, ProjectMigrationItem,
    ProjectMigrationItemStatus, ProjectMigrationPreviewItem, ProjectMigrationPreviewReport,
    ProjectMigrationPreviewStatus, ProjectMigrationProgressEvent, ProjectMigrationProgressLevel,
    ProjectMigrationReport, ProjectMigrationStrategySummary, ProjectMigrationSummary,
    ReqwestProjectMigrationExecutor,
};
pub use task_state::{
    ServiceTaskPhase, TaskAvailableAction, TaskFailureKind, TaskLogEntry, TaskLogLevel,
    TaskOperationKind, TaskProgress, TaskReplaySafety, TaskRetryDescriptor, TaskRetryPayload,
    TaskSnapshot, TaskSnapshotFeed, TaskStateError, TaskStateStore, TaskUserInput,
    TaskUserInputKind, TaskUserInputReceipt, TaskWaitingItem,
};
