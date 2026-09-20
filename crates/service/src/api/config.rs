use super::errors::path_string;
use super::runtime::runtime_storage_status;
use super::{
    extension_bridge_client_count, ApiState, ServiceApiRouteSpec, HEALTH_PATH, SERVICE_API_ROUTES,
};
use crate::{TaskAvailableAction, TaskOperationKind, TaskUserInputKind};
use overleaf_browser::{
    detect_chrome_executable, BrowserCleanupPlan, BrowserCleanupStep, BrowserSessionDescriptor,
    BrowserTaskKind, ChromeProxyPolicy, CleanupTrigger, ClosePolicy, DEFAULT_CDP_PORT_START,
    DEFAULT_CHROME_EXIT_WAIT_MS, ENV_CDP_PORT_START, ENV_CHROME_PATH, OWNED_TEMP_PROFILE_PREFIXES,
    PROFILE_REMOVE_RETRY_ATTEMPTS, PROFILE_REMOVE_RETRY_DELAY_MS, USER_WINDOW_PROFILE_PREFIX,
};
use overleaf_storage::{current_secret_storage_status, SecretStorageStatus};
use serde::Serialize;
use std::{env, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceConfigBody {
    pub skills: Vec<super::SkillInstallation>,
    pub app_version: &'static str,
    pub deployment: &'static str,
    pub browser_viewer_port: Option<u16>,
    pub data_dir: String,
    pub tmp_dir: String,
    pub workspace_dir: String,
    pub extension_dir: String,
    pub accounts_path: String,
    pub cards_path: String,
    pub addresses_path: String,
    pub sqlite_dir: String,
    pub default_export_dir: String,
    pub browser_proxy_policy: ChromeProxyPolicy,
    pub secret_storage: SecretStorageStatus,
    pub storage_status: RuntimeStorageStatus,
    pub browser_automation: BrowserAutomationStatus,
    pub api_capabilities: ApiCapabilityStatus,
    pub browser_automation_configured: bool,
    pub extension_bridge_configured: bool,
    pub extension_bridge_connected: bool,
    pub extension_bridge_client_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeStorageStatus {
    pub data_dir_exists: bool,
    pub tmp_dir_exists: bool,
    pub export_dir_exists: bool,
    pub export_file_count: usize,
    pub export_total_bytes: u64,
    pub accounts_file_bytes: Option<u64>,
    pub cards_file_bytes: Option<u64>,
    pub addresses_file_bytes: Option<u64>,
    pub owned_temp_profile_count: usize,
    pub owned_temp_profile_bytes: u64,
    pub user_window_profile_count: usize,
    pub user_window_profile_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserAutomationStatus {
    pub configured: bool,
    pub backend: &'static str,
    pub chrome_executable: Option<String>,
    pub detected_chrome_executable: Option<String>,
    pub tmp_dir: String,
    pub debug_port_start: u16,
    pub exit_wait_ms: u64,
    pub chrome_path_env_present: bool,
    pub cdp_port_start_env: Option<String>,
    pub unavailable_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiCapabilityStatus {
    pub contract_version: u32,
    pub service_readiness_policy: ServiceReadinessPolicySpec,
    pub service_api_routes: Vec<ServiceApiRouteSpec>,
    pub task_events_sse: bool,
    pub task_since_cursor: bool,
    pub task_summary: bool,
    pub explicit_task_waiting_input: bool,
    pub task_retry_descriptor: bool,
    pub task_retry_endpoint: bool,
    pub task_replay_safety_modes: Vec<&'static str>,
    pub task_phases: Vec<&'static str>,
    pub task_active_phases: Vec<&'static str>,
    pub task_terminal_phases: Vec<&'static str>,
    pub task_lock_policy: TaskLockPolicySpec,
    pub task_summary_breakdowns: Vec<&'static str>,
    pub task_result_sections: Vec<&'static str>,
    pub task_result_summary_sources: Vec<&'static str>,
    pub task_result_sensitive_key_rules: Vec<TaskResultSensitiveKeyRuleSpec>,
    pub task_result_sensitive_value_markers: Vec<&'static str>,
    pub task_list_actions: Vec<&'static str>,
    pub runtime_diagnostics: Vec<&'static str>,
    pub runtime_cleanup_actions: Vec<&'static str>,
    pub runtime_cleanup_policy: Vec<&'static str>,
    pub development_cache_policy: Vec<&'static str>,
    pub embedded_ui_assets: Vec<EmbeddedUiAssetSpec>,
    pub browser_lifecycle_policy: BrowserLifecyclePolicySpec,
    pub desktop_dialog_open_globals: Vec<&'static str>,
    pub desktop_dialog_save_globals: Vec<&'static str>,
    pub desktop_file_write_globals: Vec<&'static str>,
    pub desktop_clipboard_write_globals: Vec<&'static str>,
    pub secret_storage_actions: Vec<&'static str>,
    pub browser_profile_actions: Vec<&'static str>,
    pub account_import_sources: Vec<&'static str>,
    pub account_import_post_actions: Vec<&'static str>,
    pub account_export_modes: Vec<&'static str>,
    pub account_export_layout_modes: Vec<&'static str>,
    pub account_export_safety: Vec<&'static str>,
    pub account_row_actions: Vec<&'static str>,
    pub account_bulk_actions: Vec<&'static str>,
    pub account_switch_actions: Vec<&'static str>,
    pub account_removal_actions: Vec<&'static str>,
    pub account_credential_actions: Vec<&'static str>,
    pub account_password_actions: Vec<&'static str>,
    pub account_secret_actions: Vec<&'static str>,
    pub card_collection_actions: Vec<&'static str>,
    pub card_mutation_actions: Vec<&'static str>,
    pub address_collection_actions: Vec<&'static str>,
    pub address_selection_actions: Vec<&'static str>,
    pub registration_actions: Vec<&'static str>,
    pub registration_trial_days: Vec<u32>,
    pub registration_card_selection_strategies: Vec<&'static str>,
    pub task_operation_kinds: Vec<&'static str>,
    pub task_failure_kinds: Vec<&'static str>,
    pub task_recoverable_failure_kinds: Vec<&'static str>,
    pub task_user_input_kinds: Vec<&'static str>,
    pub task_available_actions: Vec<&'static str>,
    pub task_input_action_map: Vec<TaskInputActionSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceReadinessPolicySpec {
    pub path: &'static str,
    pub method: &'static str,
    pub success_status: u16,
    pub response_field: &'static str,
    pub expected_value: &'static str,
    pub poll_before_showing_ui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmbeddedUiAssetSpec {
    pub path: &'static str,
    pub methods: &'static [&'static str],
    pub content_type: &'static str,
    #[serde(skip)]
    pub body: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserLifecyclePolicySpec {
    pub temporary_profile_prefixes: Vec<&'static str>,
    pub user_window_profile_prefix: &'static str,
    pub temporary_task_close_policy: &'static str,
    pub user_window_close_policy: &'static str,
    pub task_finished_temporary_steps: Vec<String>,
    pub task_finished_user_window_steps: Vec<String>,
    pub user_window_exited_steps: Vec<String>,
    pub profile_remove_retry_attempts: u32,
    pub profile_remove_retry_delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskInputActionSpec {
    pub action: &'static str,
    pub input_kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskResultSensitiveKeyRuleSpec {
    pub matcher: &'static str,
    pub value: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskLockPolicySpec {
    pub lock_scope: &'static str,
    pub snapshot_field: &'static str,
    pub summary_field: &'static str,
    pub conflict_status: u16,
    pub duplicate_aliases_deduped: bool,
    pub release_on_terminal: bool,
    pub release_on_clear_terminal: bool,
}

pub(super) fn config_body(state: &ApiState) -> ServiceConfigBody {
    let bridge_client_count = extension_bridge_client_count(state);
    ServiceConfigBody {
        skills: super::skills::installations(),
        app_version: env!("CARGO_PKG_VERSION"),
        deployment: if env::var("OVERLEAF_SWITCHER_DEPLOYMENT").as_deref() == Ok("docker") {
            "docker"
        } else {
            "native"
        },
        browser_viewer_port: env::var("OVERLEAF_SWITCHER_BROWSER_VIEWER_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port > 0),
        data_dir: path_string(&state.config.data_dir),
        tmp_dir: path_string(&state.config.tmp_dir),
        workspace_dir: path_string(&state.config.workspace_dir),
        extension_dir: path_string(&state.config.extension_dir()),
        accounts_path: path_string(&state.config.accounts_path()),
        cards_path: path_string(&state.config.cards_path()),
        addresses_path: path_string(&state.config.addresses_path()),
        sqlite_dir: path_string(&state.config.sqlite_dir()),
        default_export_dir: path_string(&state.config.default_export_dir()),
        browser_proxy_policy: state.config.browser_proxy_policy.clone(),
        secret_storage: secret_storage_status(),
        storage_status: runtime_storage_status(state),
        browser_automation: browser_automation_status(state),
        api_capabilities: api_capabilities_status(),
        browser_automation_configured: state.browser_automation.is_some(),
        extension_bridge_configured: state.extension_bridge_executor.is_some(),
        extension_bridge_connected: bridge_client_count > 0,
        extension_bridge_client_count: bridge_client_count,
    }
}

fn browser_automation_status(state: &ApiState) -> BrowserAutomationStatus {
    let chrome_path_env = non_empty_env(ENV_CHROME_PATH);
    let cdp_port_start_env = non_empty_env(ENV_CDP_PORT_START);
    let detected_chrome = detect_chrome_executable();
    let debug_port_start = cdp_port_start_env
        .as_deref()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_CDP_PORT_START);

    if let Some(local_chrome) = &state.local_chrome_browser {
        let config = local_chrome.config();
        return BrowserAutomationStatus {
            configured: state.browser_automation.is_some(),
            backend: "local_chrome",
            chrome_executable: Some(path_string(&config.chrome_executable)),
            detected_chrome_executable: detected_chrome.as_deref().map(path_string),
            tmp_dir: path_string(&config.tmp_dir),
            debug_port_start: config.debug_port_start,
            exit_wait_ms: config.exit_wait_ms,
            chrome_path_env_present: chrome_path_env.is_some(),
            cdp_port_start_env,
            unavailable_reason: None,
        };
    }

    if state.browser_automation.is_some() {
        return BrowserAutomationStatus {
            configured: true,
            backend: "injected",
            chrome_executable: None,
            detected_chrome_executable: detected_chrome.as_deref().map(path_string),
            tmp_dir: path_string(&state.config.tmp_dir),
            debug_port_start,
            exit_wait_ms: DEFAULT_CHROME_EXIT_WAIT_MS,
            chrome_path_env_present: chrome_path_env.is_some(),
            cdp_port_start_env,
            unavailable_reason: None,
        };
    }

    BrowserAutomationStatus {
        configured: false,
        backend: "none",
        chrome_executable: chrome_path_env.clone(),
        detected_chrome_executable: detected_chrome.as_deref().map(path_string),
        tmp_dir: path_string(&state.config.tmp_dir),
        debug_port_start,
        exit_wait_ms: DEFAULT_CHROME_EXIT_WAIT_MS,
        chrome_path_env_present: chrome_path_env.is_some(),
        cdp_port_start_env,
        unavailable_reason: Some(if chrome_path_env.is_some() {
            "configured_chrome_path_not_found"
        } else {
            "chrome_executable_not_found"
        }),
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn secret_storage_status() -> SecretStorageStatus {
    current_secret_storage_status()
}

fn service_readiness_policy_spec() -> ServiceReadinessPolicySpec {
    ServiceReadinessPolicySpec {
        path: HEALTH_PATH,
        method: "GET",
        success_status: 200,
        response_field: "status",
        expected_value: "ok",
        poll_before_showing_ui: true,
    }
}

pub const EMBEDDED_UI_ASSETS: &[EmbeddedUiAssetSpec] = &[
    ui_asset(
        "/ui/",
        "text/html; charset=utf-8",
        include_str!("../../../../apps/web/dist/index.html"),
    ),
    ui_asset(
        "/ui/app.js",
        "application/javascript; charset=utf-8",
        include_str!("../../../../apps/web/dist/app.js"),
    ),
    ui_asset(
        "/ui/styles.css",
        "text/css; charset=utf-8",
        include_str!("../../../../apps/web/dist/styles.css"),
    ),
];

const fn ui_asset(
    path: &'static str,
    content_type: &'static str,
    body: &'static str,
) -> EmbeddedUiAssetSpec {
    EmbeddedUiAssetSpec {
        path,
        methods: &["GET", "HEAD"],
        content_type,
        body,
    }
}

fn browser_lifecycle_policy_spec() -> BrowserLifecyclePolicySpec {
    let temporary_session = BrowserSessionDescriptor {
        task_kind: BrowserTaskKind::AccountPasswordLogin,
        close_policy: ClosePolicy::AutoCloseTemp,
        debug_port: 0,
        user_data_dir: PathBuf::from("<temporary_profile_dir>"),
        process_id: Some(0),
    };
    let user_window_session = BrowserSessionDescriptor {
        task_kind: BrowserTaskKind::BrowserAutoLogin,
        close_policy: ClosePolicy::KeepUserWindow,
        debug_port: 0,
        user_data_dir: PathBuf::from("<user_window_profile_dir>"),
        process_id: Some(0),
    };

    BrowserLifecyclePolicySpec {
        temporary_profile_prefixes: OWNED_TEMP_PROFILE_PREFIXES.to_vec(),
        user_window_profile_prefix: USER_WINDOW_PROFILE_PREFIX,
        temporary_task_close_policy: close_policy_label(temporary_session.close_policy),
        user_window_close_policy: close_policy_label(user_window_session.close_policy),
        task_finished_temporary_steps: cleanup_step_labels(&BrowserCleanupPlan::for_session(
            &temporary_session,
            CleanupTrigger::TaskFinished,
        )),
        task_finished_user_window_steps: cleanup_step_labels(&BrowserCleanupPlan::for_session(
            &user_window_session,
            CleanupTrigger::TaskFinished,
        )),
        user_window_exited_steps: cleanup_step_labels(&BrowserCleanupPlan::for_session(
            &user_window_session,
            CleanupTrigger::UserWindowExited,
        )),
        profile_remove_retry_attempts: PROFILE_REMOVE_RETRY_ATTEMPTS,
        profile_remove_retry_delay_ms: PROFILE_REMOVE_RETRY_DELAY_MS,
    }
}

fn close_policy_label(policy: ClosePolicy) -> &'static str {
    match policy {
        ClosePolicy::AutoCloseTemp => "auto_close_temporary",
        ClosePolicy::KeepUserWindow => "keep_user_window",
    }
}

fn cleanup_step_labels(plan: &BrowserCleanupPlan) -> Vec<String> {
    plan.steps
        .iter()
        .map(|step| match step {
            BrowserCleanupStep::SendCdpBrowserClose { .. } => "cdp_browser_close",
            BrowserCleanupStep::WaitForProcessExit { .. } => "wait_process_exit",
            BrowserCleanupStep::KillProcessesByDebugPort { .. } => "scoped_kill_by_debug_port",
            BrowserCleanupStep::KillProcessesByProfileDir { .. } => "scoped_kill_by_profile_dir",
            BrowserCleanupStep::WaitForUserWindowExit { .. } => "wait_user_window_exit",
            BrowserCleanupStep::RemoveProfileDirWithRetry { .. } => "remove_profile_dir_with_retry",
        })
        .map(ToOwned::to_owned)
        .collect()
}

fn api_capabilities_status() -> ApiCapabilityStatus {
    ApiCapabilityStatus {
        contract_version: 1,
        service_readiness_policy: service_readiness_policy_spec(),
        service_api_routes: SERVICE_API_ROUTES.to_vec(),
        task_events_sse: true,
        task_since_cursor: true,
        task_summary: true,
        explicit_task_waiting_input: true,
        task_retry_descriptor: true,
        task_retry_endpoint: true,
        task_replay_safety_modes: vec!["safe", "requires_confirmation"],
        task_phases: task_phase_names(),
        task_active_phases: vec!["pending", "running", "waiting_for_user"],
        task_terminal_phases: vec!["completed", "failed", "cancelled"],
        task_lock_policy: TaskLockPolicySpec {
            lock_scope: "account_alias_write",
            snapshot_field: "locked_aliases",
            summary_field: "locked_alias_count",
            conflict_status: 409,
            duplicate_aliases_deduped: true,
            release_on_terminal: true,
            release_on_clear_terminal: true,
        },
        task_summary_breakdowns: vec![
            "waiting_for_input_by_kind",
            "failed_by_kind",
            "available_actions_count",
        ],
        task_result_sections: vec![
            "summary_chips",
            "import_decisions",
            "project_migration",
            "top_level_items",
            "nested_items",
            "post_action_errors",
        ],
        task_result_summary_sources: vec![
            "root",
            "import",
            "session_refresh",
            "git_token_generation",
            "project_migration",
        ],
        task_result_sensitive_key_rules: vec![
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "cookie",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "ends_with",
                value: "_cookie",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "contains",
                value: "password",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "session",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "ends_with",
                value: "_session",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "secret",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "ends_with",
                value: "_secret",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "token",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "ends_with",
                value: "_token",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "cvc",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "number",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "card_number",
            },
            TaskResultSensitiveKeyRuleSpec {
                matcher: "equals",
                value: "number_or_suffix",
            },
        ],
        task_result_sensitive_value_markers: vec![
            "git_token_prefix",
            "overleaf_session_cookie_assignment",
            "encoded_overleaf_session_prefix",
            "raw_overleaf_session_prefix",
        ],
        task_list_actions: vec!["clear-terminal"],
        runtime_diagnostics: vec![
            "browser_profiles",
            "runtime_artifacts",
            "runtime_temp_profiles",
        ],
        runtime_cleanup_actions: vec!["runtime_artifacts_cleanup", "runtime_temp_profiles_cleanup"],
        runtime_cleanup_policy: vec![
            "requires_explicit_confirmation",
            "workspace_artifacts_only",
            "protect_data_backups_and_git",
            "owned_temp_profiles_only",
            "skip_user_window_profiles",
        ],
        development_cache_policy: vec![
            "preserve_reusable_docker_and_cargo_cache_during_active_iteration",
            "use_space_reports_and_warning_thresholds_for_routine_monitoring",
            "deep_cache_cleanup_only_for_final_delivery_cache_bloat_low_space_or_explicit_request",
        ],
        embedded_ui_assets: EMBEDDED_UI_ASSETS.to_vec(),
        browser_lifecycle_policy: browser_lifecycle_policy_spec(),
        desktop_dialog_open_globals: vec!["window.__OVERLEAF_DESKTOP__.openDialog"],
        desktop_dialog_save_globals: vec!["window.__OVERLEAF_DESKTOP__.saveDialog"],
        desktop_file_write_globals: vec!["window.__OVERLEAF_DESKTOP__.writeTextFile"],
        desktop_clipboard_write_globals: vec!["window.__OVERLEAF_DESKTOP__.writeClipboardText"],
        secret_storage_actions: vec!["status"],
        browser_profile_actions: vec!["discover-profiles", "detect-current-account"],
        account_import_sources: vec!["json_path", "json_text", "manual_cookie"],
        account_import_post_actions: vec!["refresh_session_metadata", "fetch_git_token"],
        account_export_modes: vec!["single_file", "multiple_files", "browser_download_json"],
        account_export_layout_modes: vec!["single_file", "multiple_files"],
        account_export_safety: vec![
            "main_config_overwrite_block",
            "existing_export_overwrite_confirmation",
            "sensitive_export_warning",
        ],
        account_row_actions: vec![
            "refresh-session",
            "refresh-git",
            "generate-git",
            "browser-login",
            "switch-plan",
            "switch-execute",
        ],
        account_bulk_actions: vec![
            "refresh-session",
            "refresh-git",
            "generate-git",
            "browser-login",
        ],
        account_switch_actions: vec!["plan", "preview-projects", "execute"],
        account_removal_actions: vec![
            "preview-remote-cleanup",
            "remove-local",
            "cleanup-remote-and-remove",
        ],
        account_credential_actions: vec!["add-with-password", "refresh-cookie-with-password"],
        account_password_actions: vec!["update-local-password", "change-overleaf-password"],
        account_secret_actions: vec!["copy-account-secret"],
        card_collection_actions: vec!["add", "export"],
        card_mutation_actions: vec!["used", "failed", "remove"],
        address_collection_actions: vec!["fetch"],
        address_selection_actions: vec!["select", "fallback"],
        registration_actions: vec!["start", "auto-fetch-git-token"],
        registration_trial_days: vec![21, 7],
        registration_card_selection_strategies: vec!["new_then_used_then_failed", "new_only"],
        task_operation_kinds: task_operation_kind_names(),
        task_failure_kinds: vec![
            "retryable",
            "needs_user_input",
            "needs_login",
            "page_changed",
            "fatal",
        ],
        task_recoverable_failure_kinds: vec![
            "retryable",
            "needs_user_input",
            "needs_login",
            "page_changed",
        ],
        task_user_input_kinds: TaskUserInputKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect(),
        task_available_actions: TaskAvailableAction::ALL
            .iter()
            .map(|action| action.as_str())
            .collect(),
        task_input_action_map: TaskAvailableAction::ALL
            .iter()
            .filter_map(|action| {
                action.input_kind().map(|input_kind| TaskInputActionSpec {
                    action: action.as_str(),
                    input_kind: input_kind.as_str(),
                })
            })
            .collect(),
    }
}

fn task_operation_kind_names() -> Vec<&'static str> {
    TaskOperationKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect()
}

fn task_phase_names() -> Vec<&'static str> {
    vec![
        "pending",
        "running",
        "waiting_for_user",
        "completed",
        "failed",
        "cancelled",
    ]
}
