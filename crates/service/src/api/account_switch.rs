use super::*;
use crate::account_switch::{execute_account_switch_with_commit_lock, AccountSwitchContext};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountSwitchPlanRequest {
    alias: String,
    #[serde(default)]
    migrate_projects: bool,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountSwitchExecuteRequest {
    alias: String,
    #[serde(default)]
    migrate_projects: bool,
    #[serde(default)]
    sync_skills: bool,
    #[serde(default)]
    project_ids: Option<Vec<String>>,
    #[serde(default)]
    source_alias: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AccountSwitchProjectPreviewRequest {
    alias: String,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Clone)]
struct TaskProjectMigrationControl {
    shared_state: Arc<StdMutex<ApiState>>,
    task_id: String,
    selection: TaskRetryPayload,
}

impl TaskProjectMigrationControl {
    fn new(shared_state: Arc<StdMutex<ApiState>>, task_id: impl Into<String>) -> Self {
        let task_id = task_id.into();
        let selection = shared_state
            .lock()
            .ok()
            .and_then(|state| state.tasks.snapshot(&task_id).ok())
            .and_then(|task| task.retry_descriptor)
            .map(|retry| retry.payload)
            .unwrap_or_default();
        Self {
            shared_state,
            task_id,
            selection,
        }
    }
}

impl ProjectMigrationControl for TaskProjectMigrationControl {
    fn selected_project_ids(&self) -> Option<&[String]> {
        self.selection.project_ids.as_deref()
    }

    fn expected_source_alias(&self) -> Option<&str> {
        self.selection.source_alias.as_deref()
    }

    fn is_cancel_requested(&self) -> bool {
        browser_task_cancel_requested(&self.shared_state, &self.task_id)
    }

    fn on_progress(&self, event: ProjectMigrationProgressEvent) {
        let Ok(mut state) = self.shared_state.lock() else {
            return;
        };
        let is_terminal = state
            .tasks
            .snapshot(&self.task_id)
            .map(|snapshot| {
                matches!(
                    snapshot.phase,
                    crate::ServiceTaskPhase::Completed
                        | crate::ServiceTaskPhase::Failed
                        | crate::ServiceTaskPhase::Cancelled
                )
            })
            .unwrap_or(true);
        if is_terminal {
            return;
        }
        let _ = state.tasks.set_progress(
            &self.task_id,
            event.current,
            event.total,
            event.message.clone(),
        );
        let level = match event.level {
            ProjectMigrationProgressLevel::Info => TaskLogLevel::Info,
            ProjectMigrationProgressLevel::Warning => TaskLogLevel::Warning,
            ProjectMigrationProgressLevel::Error => TaskLogLevel::Error,
        };
        let _ = state.tasks.append_log(&self.task_id, level, event.message);
    }
}

pub(super) fn plan_account_switch_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: AccountSwitchPlanRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let alias = request.alias.trim().to_string();
    if alias.is_empty() {
        return account_switch_error_response(AccountSwitchError::AccountAliasNotFound { alias });
    }
    let task_id = normalized_optional_text(request.task_id);
    if let Err(response) = start_alias_batch_task_with_retry_payload(
        state,
        task_id.as_deref(),
        "规划无感换号扩展命令",
        std::slice::from_ref(&alias),
        TaskRetryPayload {
            migrate_projects: Some(request.migrate_projects),
            ..Default::default()
        },
    ) {
        return response;
    }

    let store = AccountStore::new(state.config.accounts_path());
    match plan_account_switch_in_store(
        &store,
        &alias,
        request.migrate_projects,
        state.secret_backend.as_ref(),
    ) {
        Ok(plan) => {
            let report = plan.redacted_report();
            complete_tracked_task(state, task_id.as_deref(), "无感换号命令计划已生成", &report);
            json_response(200, &report)
        }
        Err(error) => {
            fail_tracked_task(
                state,
                task_id.as_deref(),
                account_switch_error_message(&error),
            );
            account_switch_error_response(error)
        }
    }
}

pub(super) fn preview_account_switch_projects_response(
    state: &mut ApiState,
    body: &str,
) -> ApiResponse {
    let request: AccountSwitchProjectPreviewRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let alias = request.alias.trim().to_string();
    if alias.is_empty() {
        return project_migration_error_response(ProjectMigrationError::AccountAliasNotFound {
            alias,
        });
    }
    let task_id = normalized_optional_text(request.task_id);
    if let Err(response) = start_alias_batch_task(
        state,
        task_id.as_deref(),
        "预览项目迁移",
        std::slice::from_ref(&alias),
    ) {
        return response;
    }

    let store = AccountStore::new(state.config.accounts_path());
    let project_migration_executor = state.project_migration_executor.clone();
    match block_on_api(project_migration_executor.preview_projects(
        &store,
        &alias,
        state.secret_backend.as_ref(),
    )) {
        Ok(Ok(report)) => {
            complete_tracked_task(state, task_id.as_deref(), "项目迁移预览已生成", &report);
            json_response(200, &report)
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                task_id.as_deref(),
                project_migration_error_message(&error),
            );
            project_migration_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

pub(super) fn execute_account_switch_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request: AccountSwitchExecuteRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.project_ids.as_ref().is_some_and(|ids| {
        ids.iter()
            .any(|id| id.len() != 24 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
    }) || (request.migrate_projects
        && request.project_ids.is_some()
        && request
            .source_alias
            .as_deref()
            .is_none_or(|alias| alias.trim().is_empty()))
    {
        return json_response(
            400,
            &ApiErrorBody {
                error: "迁移项目选择无效，请重新选择".into(),
            },
        );
    }

    let alias = request.alias.trim().to_string();
    if alias.is_empty() {
        return account_switch_error_response(AccountSwitchError::AccountAliasNotFound { alias });
    }
    let task_id = normalized_optional_text(request.task_id).unwrap_or_else(|| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("account-switch-{stamp}")
    });
    if request.migrate_projects {
        let source = request.source_alias.clone().or_else(|| {
            AccountStore::new(state.config.accounts_path())
                .load()
                .ok()
                .and_then(|document| document.current)
        });
        if source.is_some_and(|source| {
            state.tasks.list_snapshots().iter().any(|task| {
                task.locked_aliases.contains(&source)
                    && !matches!(
                        task.phase,
                        ServiceTaskPhase::Completed
                            | ServiceTaskPhase::Failed
                            | ServiceTaskPhase::Cancelled
                    )
            })
        }) {
            return json_response(
                409,
                &ApiErrorBody {
                    error: "源账号还有任务正在执行，请等待完成后再迁移".into(),
                },
            );
        }
    }
    if state.tasks.list_snapshots().iter().any(|task| {
        task.operation_kind == Some(TaskOperationKind::AccountSwitchExecute)
            && matches!(
                task.phase,
                crate::ServiceTaskPhase::Pending
                    | crate::ServiceTaskPhase::Running
                    | crate::ServiceTaskPhase::WaitingForUser
            )
    }) {
        return json_response(
            409,
            &ApiErrorBody {
                error: "已有无感换号任务正在执行，请等待完成后再切换".into(),
            },
        );
    }
    if let Err(response) = start_alias_batch_task_with_retry_payload(
        state,
        Some(&task_id),
        "执行无感换号扩展命令",
        std::slice::from_ref(&alias),
        TaskRetryPayload {
            migrate_projects: Some(request.migrate_projects),
            sync_skills: Some(request.sync_skills),
            project_ids: request.project_ids,
            source_alias: request.source_alias,
        },
    ) {
        return response;
    }

    execute_account_switch_task_response(
        state,
        &task_id,
        &alias,
        request.migrate_projects,
        now_unix,
        true,
    )
}

pub(super) fn execute_account_switch_task_response(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    migrate_projects: bool,
    now_unix: i64,
    allow_recovery: bool,
) -> ApiResponse {
    if state.extension_bridge_executor.is_none() {
        let message = "extension bridge executor is not configured".to_string();
        fail_tracked_task(state, Some(task_id), message.clone());
        return json_response(503, &ApiErrorBody { error: message });
    }

    let owned_task_id = task_id.to_string();
    let owned_alias = alias.to_string();
    // HTTP 层持有提交锁；换号必须在请求释放锁后执行。
    let job = ApiBackgroundJob::new("account-switch", move |shared_state| {
        run_account_switch_background(
            shared_state,
            owned_task_id,
            owned_alias,
            migrate_projects,
            now_unix,
            allow_recovery,
        );
    });
    if let Err(response) = state.enqueue_background_job(job) {
        fail_tracked_task(state, Some(task_id), response.body.clone());
        return response;
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        if migrate_projects {
            "项目迁移已进入后台，进度将按项目实时更新"
        } else {
            "无感换号已进入后台"
        },
    );
    match state.tasks.snapshot(task_id) {
        Ok(snapshot) => json_response(202, &snapshot),
        Err(error) => task_state_error_response(error),
    }
}

fn run_account_switch_background(
    shared_state: Arc<StdMutex<ApiState>>,
    task_id: String,
    alias: String,
    migrate_projects: bool,
    now_unix: i64,
    allow_recovery: bool,
) {
    if browser_task_cancel_requested(&shared_state, &task_id) {
        if let Ok(mut state) = shared_state.lock() {
            let _ = cancel_task_with_registration_release(&mut state, &task_id, "无感换号已取消");
        }
        return;
    }

    let dependencies = shared_state.lock().ok().and_then(|mut state| {
        let executor = state.extension_bridge_executor.clone()?;
        let _ = state.tasks.append_log(
            &task_id,
            TaskLogLevel::Info,
            "正在验证目标账号 Cookie 登录身份",
        );
        Some((
            state.config.accounts_path(),
            executor,
            state.project_migration_executor.clone(),
            state.secret_backend.clone(),
            state.session_identity_validator.clone(),
            state.metadata_refresher.clone(),
            state.account_commit_lock(),
        ))
    });
    let Some((
        accounts_path,
        executor,
        project_migration_executor,
        secret_backend,
        validator,
        metadata_refresher,
        commit_lock,
    )) = dependencies
    else {
        if let Ok(mut state) = shared_state.lock() {
            fail_tracked_task(
                &mut state,
                Some(&task_id),
                "extension bridge executor is not configured",
            );
        }
        return;
    };

    let control = TaskProjectMigrationControl::new(Arc::clone(&shared_state), task_id.clone());
    let store = AccountStore::new(accounts_path);
    let result = block_on_api(execute_account_switch_with_commit_lock(
        &store,
        &alias,
        migrate_projects,
        AccountSwitchContext {
            executor: executor.as_ref(),
            secret_backend: secret_backend.as_ref(),
            session_identity_validator: validator.as_ref(),
            project_migration_executor: Some(project_migration_executor.as_ref()),
            project_migration_control: Some(&control),
            commit_lock: Some(&commit_lock),
        },
    ));

    if matches!(&result, Ok(Ok(_))) {
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &task_id,
                TaskLogLevel::Info,
                "换号命令已执行，正在读取目标账号订阅状态",
            );
        }
        let metadata = block_on_api(
            crate::account_session::refresh_saved_account_session_with_commit_lock(
                &store,
                &alias,
                now_unix,
                secret_backend.as_ref(),
                metadata_refresher.as_ref(),
                Some(&commit_lock),
            ),
        );
        if let Ok(mut state) = shared_state.lock() {
            let detected = matches!(metadata, Ok(Ok(ref report)) if report.subscription_refresh_status == SubscriptionRefreshStatus::Detected);
            let _ = state.tasks.append_log(
                &task_id,
                if detected {
                    TaskLogLevel::Info
                } else {
                    TaskLogLevel::Warning
                },
                if detected {
                    "目标账号订阅状态已更新"
                } else {
                    "换号已完成，订阅状态暂未更新，可稍后刷新"
                },
            );
        }
    }

    let follow_up_jobs = if let Ok(mut state) = shared_state.lock() {
        let _ = finish_account_switch_attempt(
            &mut state,
            Some(&task_id),
            &alias,
            migrate_projects,
            now_unix,
            allow_recovery,
            result,
        );
        state.take_background_jobs()
    } else {
        Vec::new()
    };
    spawn_follow_up_jobs(&shared_state, &task_id, follow_up_jobs);
}

fn finish_account_switch_attempt(
    state: &mut ApiState,
    task_id: Option<&str>,
    alias: &str,
    migrate_projects: bool,
    now_unix: i64,
    allow_recovery: bool,
    result: Result<
        Result<crate::AccountSwitchExecutionReport, AccountSwitchExecutionError>,
        ApiResponse,
    >,
) -> ApiResponse {
    match result {
        Ok(Err(AccountSwitchExecutionError::ProjectMigration(
            ProjectMigrationError::Cancelled,
        ))) => {
            if let Some(task_id) = task_id {
                let _ = cancel_task_with_registration_release(state, task_id, "项目迁移已取消");
            }
            account_switch_execution_error_response(AccountSwitchExecutionError::ProjectMigration(
                ProjectMigrationError::Cancelled,
            ))
        }
        Ok(Ok(report)) => {
            let result = skills::finish_switch(state, task_id, &report);
            json_response(200, &result)
        }
        Ok(Err(error)) if allow_recovery && error.requires_cookie_recovery() => {
            let Some(task_id) = task_id else {
                return account_switch_execution_error_response(error);
            };
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!(
                    "{}；正在尝试账号密码恢复",
                    account_switch_execution_error_message(&error)
                ),
            );
            start_account_switch_recovery_response(
                state,
                task_id,
                alias,
                migrate_projects,
                now_unix,
            )
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                task_id,
                account_switch_execution_error_message(&error),
            );
            account_switch_execution_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, task_id, response.body.clone());
            response
        }
    }
}

fn start_account_switch_recovery_response(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    migrate_projects: bool,
    now_unix: i64,
) -> ApiResponse {
    let plan = match account_cookie_recovery_plan(state, task_id, alias) {
        Ok(plan) => plan,
        Err(response) => return response,
    };

    if plan.password.is_empty() {
        let message = "目标账号没有可用的本地密码，请重新输入密码后再试";
        fail_tracked_task(state, Some(task_id), message);
        return json_response(
            409,
            &ApiTypedErrorBody {
                error: message.to_string(),
                kind: "account_password_required",
            },
        );
    }

    if let Some(local_chrome) = state.local_chrome_browser.clone() {
        return queue_credential_refresh_browser_session_response(
            state,
            task_id,
            vec![plan],
            CredentialBrowserBatchCompletion::CredentialRefresh(
                CredentialRefreshContinuation::ExecuteAccountSwitch {
                    alias: alias.to_string(),
                    migrate_projects,
                    now_unix,
                },
            ),
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        let message = "interactive Chrome automation is not configured for Cookie recovery";
        fail_tracked_task(state, Some(task_id), message);
        return json_response(
            503,
            &ApiTypedErrorBody {
                error: message.to_string(),
                kind: "account_cookie_recovery_unavailable",
            },
        );
    };
    let store = AccountStore::new(state.config.accounts_path());
    match block_on_api(
        refresh_account_cookie_with_credentials_in_store_with_backend(
            &store,
            &plan.alias,
            SecretText::new(plan.password),
            browser.as_ref(),
            state.metadata_refresher.as_ref(),
            now_unix,
            state.secret_backend.as_ref(),
        ),
    ) {
        Ok(Ok(report)) => {
            if let Some(message) = credential_report_failure_message(&report) {
                fail_tracked_task(state, Some(task_id), message.clone());
                return json_response(400, &ApiErrorBody { error: message });
            }
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "Cookie 已恢复并验证，正在继续原无感换号任务",
            );
            execute_account_switch_task_response(
                state,
                task_id,
                alias,
                migrate_projects,
                now_unix,
                false,
            )
        }
        Ok(Err(error)) => {
            fail_tracked_task(
                state,
                Some(task_id),
                account_credential_error_message(&error),
            );
            account_credential_error_response(error)
        }
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            response
        }
    }
}

pub(super) fn account_cookie_recovery_plan(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
) -> Result<AliasPasswordPlan, ApiResponse> {
    let store = AccountStore::new(state.config.accounts_path());
    let document = store.load().map_err(|error| {
        let response = io_error_response(error);
        fail_tracked_task(state, Some(task_id), response.body.clone());
        response
    })?;
    let record = document.accounts.get(alias).ok_or_else(|| {
        let error = AccountSwitchError::AccountAliasNotFound {
            alias: alias.to_string(),
        };
        fail_tracked_task(state, Some(task_id), account_switch_error_message(&error));
        account_switch_error_response(error)
    })?;
    let email = record
        .email
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            let message = format!("missing email for account alias: {alias}");
            fail_tracked_task(state, Some(task_id), message.clone());
            json_response(400, &ApiErrorBody { error: message })
        })?;
    let password = match read_account_password_secret_for_recovery(
        record,
        alias,
        state.secret_backend.as_ref(),
    ) {
        Ok(password) => password,
        Err(AccountSecretStoreError::Missing { .. }) => String::new(),
        Err(error) => {
            let message = account_secret_store_error_message(&error);
            fail_tracked_task(state, Some(task_id), message.clone());
            return Err(json_response(500, &ApiErrorBody { error: message }));
        }
    };

    Ok(AliasPasswordPlan {
        alias: alias.to_string(),
        email: Some(email),
        password,
    })
}
