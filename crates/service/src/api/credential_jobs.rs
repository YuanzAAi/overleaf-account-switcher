use super::*;

pub(super) fn queue_credential_add_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    login_accounts: Vec<CredentialAccountPlan>,
    skipped_duplicate_emails: Vec<SkippedDuplicateEmail>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    if login_accounts.is_empty() {
        let report = credential_report_from_skipped_duplicates(&skipped_duplicate_emails);
        finish_credential_batch_task(state, Some(task_id), "账号密码添加账号", &report);
        return json_response(200, &report);
    }

    for skipped in &skipped_duplicate_emails {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Info,
            format!("邮箱已存在，跳过重复账号: {}", skipped.alias),
        );
    }
    let initial_report = credential_report_from_skipped_duplicates(&skipped_duplicate_emails);
    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::Add,
            plans: login_accounts
                .into_iter()
                .map(CredentialBrowserPlan::Add)
                .collect(),
        },
        initial_report
            .items
            .into_iter()
            .map(CredentialBrowserBatchItem::Credential)
            .collect(),
        CredentialBrowserBatchCompletion::Standard,
        local_chrome,
        now_unix,
    )
}

pub(super) fn queue_credential_refresh_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    plans: Vec<AliasPasswordPlan>,
    completion: CredentialBrowserBatchCompletion,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    if plans.is_empty() {
        let report = CredentialBatchReport::default();
        finish_credential_batch_task(state, Some(task_id), "账号密码刷新 Cookie", &report);
        return json_response(200, &report);
    }

    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::Refresh,
            plans: plans
                .into_iter()
                .map(CredentialBrowserPlan::Refresh)
                .collect(),
        },
        Vec::new(),
        completion,
        local_chrome,
        now_unix,
    )
}

pub(super) fn queue_overleaf_password_change_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    plans: Vec<AliasPasswordPlan>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    if plans.is_empty() {
        let report = OverleafPasswordChangeBatchReport {
            changed_count: 0,
            failed_count: 0,
            items: Vec::new(),
        };
        finish_overleaf_password_change_batch_task(state, Some(task_id), &report);
        return json_response(200, &report);
    }

    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            let response = io_error_response(error);
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    let plans = plans
        .into_iter()
        .map(|plan| {
            let (current_password, startup_error) = match document.accounts.get(&plan.alias) {
                Some(record) => match read_account_password_secret_for_recovery(
                    record,
                    &plan.alias,
                    state.secret_backend.as_ref(),
                ) {
                    Ok(password) => (password, None),
                    Err(AccountSecretStoreError::Missing { .. }) => (String::new(), None),
                    Err(AccountSecretStoreError::ReadFailed { .. }) => (
                        String::new(),
                        Some(format!(
                            "failed to read stored password for account alias: {}",
                            plan.alias
                        )),
                    ),
                    Err(AccountSecretStoreError::WriteFailed { .. }) => (
                        String::new(),
                        Some(format!(
                            "failed to prepare stored password for account alias: {}",
                            plan.alias
                        )),
                    ),
                },
                None => (
                    String::new(),
                    Some(format!("account alias not found: {}", plan.alias)),
                ),
            };
            CredentialBrowserPlan::PasswordChange(RemotePasswordChangeBrowserPlan {
                alias: plan.alias,
                email: plan.email,
                current_password,
                new_password: plan.password,
                startup_error,
            })
        })
        .collect();

    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::PasswordChange,
            plans,
        },
        Vec::new(),
        CredentialBrowserBatchCompletion::Standard,
        local_chrome,
        now_unix,
    )
}

pub(super) fn queue_git_token_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    aliases: Vec<String>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    let plans = match load_git_token_browser_plans(state, aliases, now_unix, true) {
        Ok(plans) => plans,
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::GitToken,
            plans: plans
                .into_iter()
                .map(CredentialBrowserPlan::GitToken)
                .collect(),
        },
        Vec::new(),
        CredentialBrowserBatchCompletion::Standard,
        local_chrome,
        now_unix,
    )
}

pub(super) fn queue_git_token_refresh_browser_session_response(
    state: &mut ApiState,
    task_id: &str,
    aliases: Vec<String>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    let plans = match load_git_token_browser_plans(state, aliases, now_unix, false) {
        Ok(plans) => plans,
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::GitTokenRefresh,
            plans: plans
                .into_iter()
                .map(CredentialBrowserPlan::GitTokenRefresh)
                .collect(),
        },
        Vec::new(),
        CredentialBrowserBatchCompletion::Standard,
        local_chrome,
        now_unix,
    )
}

pub(super) fn load_git_token_browser_plans(
    state: &mut ApiState,
    aliases: Vec<String>,
    now_unix: i64,
    allow_skip_existing: bool,
) -> Result<Vec<GitTokenBrowserPlan>, ApiResponse> {
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            return Err(io_error_response(error));
        }
    };
    let plans = aliases
        .into_iter()
        .map(|alias| {
            let (email, password, startup_error, recovery_error) =
                match document.accounts.get(&alias) {
                    Some(record) => {
                        let email = record.email.clone();
                        match read_account_password_secret_for_recovery(
                            record,
                            &alias,
                            state.secret_backend.as_ref(),
                        ) {
                            Ok(password) => (email, password, None, None),
                            Err(AccountSecretStoreError::Missing { .. }) => {
                                (email, String::new(), None, None)
                            }
                            Err(AccountSecretStoreError::ReadFailed { .. }) => (
                                email,
                                String::new(),
                                None,
                                Some(format!(
                                    "failed to read stored password for account alias: {alias}"
                                )),
                            ),
                            Err(AccountSecretStoreError::WriteFailed { .. }) => (
                                email,
                                String::new(),
                                None,
                                Some(format!(
                                    "failed to prepare stored password for account alias: {alias}"
                                )),
                            ),
                        }
                    }
                    None => (
                        None,
                        String::new(),
                        Some(format!("account alias not found: {alias}")),
                        None,
                    ),
                };
            let skip_existing_token = allow_skip_existing
                && document.accounts.get(&alias).is_some_and(|record| {
                    has_reusable_git_token(record, &alias, now_unix, state.secret_backend.as_ref())
                });
            GitTokenBrowserPlan {
                alias,
                email,
                password,
                startup_error,
                recovery_error,
                skip_existing_token,
                force_credentials_login: false,
            }
        })
        .collect();
    Ok(plans)
}

#[derive(Clone, Copy)]
pub(super) enum CredentialBrowserOperation {
    Add,
    Refresh,
    PasswordChange,
    GitTokenRefresh,
    GitToken,
    BrowserLogin,
}

impl CredentialBrowserOperation {
    fn task_label(self) -> &'static str {
        match self {
            Self::Add => "账号密码添加账号",
            Self::Refresh => "账号密码刷新 Cookie",
            Self::PasswordChange => "修改 Overleaf 密码",
            Self::GitTokenRefresh => "刷新 Git Integration token 状态",
            Self::GitToken => "获取 Git Integration token",
            Self::BrowserLogin => "浏览器登录",
        }
    }

    fn job_name(self) -> &'static str {
        match self {
            Self::Add => "credential-add-batch",
            Self::Refresh => "credential-refresh-batch",
            Self::PasswordChange => "overleaf-password-change-batch",
            Self::GitTokenRefresh => "git-token-refresh-batch",
            Self::GitToken => "git-token-generate-batch",
            Self::BrowserLogin => "browser-login-credential-batch",
        }
    }

    fn worker_name(self) -> &'static str {
        match self {
            Self::Add => "overleaf-credential-add-item",
            Self::Refresh => "overleaf-credential-refresh-item",
            Self::PasswordChange => "overleaf-password-change-item",
            Self::GitTokenRefresh => "overleaf-git-token-refresh-item",
            Self::GitToken => "overleaf-git-token-item",
            Self::BrowserLogin => "overleaf-browser-login-item",
        }
    }

    fn queue_message(self, max_concurrency: usize) -> String {
        match self {
            Self::Add => format!("账号添加批次已进入独立后台队列，并发上限 {max_concurrency}"),
            Self::Refresh => {
                format!("Cookie 刷新批次已进入独立后台队列，并发上限 {max_concurrency}")
            }
            Self::PasswordChange => {
                format!("远端改密批次已进入独立后台队列，并发上限 {max_concurrency}")
            }
            Self::GitTokenRefresh => {
                format!("Git token 状态刷新批次已进入独立后台队列，并发上限 {max_concurrency}")
            }
            Self::GitToken => {
                format!("Git token 批次已进入独立后台队列，并发上限 {max_concurrency}")
            }
            Self::BrowserLogin => {
                format!("浏览器登录批次已进入独立后台队列，并发上限 {max_concurrency}")
            }
        }
    }

    fn running_message(self) -> &'static str {
        match self {
            Self::Add => "登录并添加账号",
            Self::Refresh => "刷新 Cookie",
            Self::PasswordChange => "登录并修改远端密码",
            Self::GitTokenRefresh => "刷新 Git token 状态",
            Self::GitToken => "检查 Cookie 并获取 Git token",
            Self::BrowserLogin => "打开浏览器并登录",
        }
    }

    fn invalid_credentials_message(self) -> &'static str {
        match self {
            Self::Add => "邮箱或密码错误，请重新输入",
            Self::Refresh => "密码错误，请重新输入",
            Self::PasswordChange => "当前密码错误，请重新输入",
            Self::GitTokenRefresh => "密码错误，请重新输入后继续刷新 Git token 状态",
            Self::GitToken => "密码错误，请重新输入后继续获取 Git token",
            Self::BrowserLogin => "没有可用登录态，请重新输入密码",
        }
    }

    fn success_message(self, alias: &str) -> String {
        match self {
            Self::Add => format!("登录并保存完成: {alias}"),
            Self::Refresh => format!("Cookie 刷新完成: {alias}"),
            Self::PasswordChange => format!("Overleaf 远端密码修改完成: {alias}"),
            Self::GitTokenRefresh => format!("Git token 状态刷新完成: {alias}"),
            Self::GitToken => format!("Git token 获取完成: {alias}"),
            Self::BrowserLogin => format!("浏览器登录完成: {alias}"),
        }
    }

    fn terminal_item_message(self) -> &'static str {
        match self {
            Self::Add => "账号添加完成",
            Self::Refresh => "Cookie 刷新完成",
            Self::PasswordChange => "远端密码修改完成",
            Self::GitTokenRefresh => "Git token 状态刷新完成",
            Self::GitToken => "Git token 获取完成",
            Self::BrowserLogin => "浏览器登录完成",
        }
    }

    pub(super) fn git_token_batch_success_message(self) -> &'static str {
        match self {
            Self::GitTokenRefresh => "Git Integration token 状态刷新完成",
            Self::GitToken => "Git Integration token 获取完成",
            _ => "Git Integration token 操作完成",
        }
    }

    pub(super) fn git_token_batch_failure_prefix(self) -> &'static str {
        match self {
            Self::GitTokenRefresh => "Git Integration token 状态刷新未完全完成",
            Self::GitToken => "Git Integration token 获取未完全完成",
            _ => "Git Integration token 操作未完全完成",
        }
    }

    fn failure_message(self, alias: &str, message: &str) -> String {
        match self {
            Self::Add => format!("账号未完成: {alias} ({message})"),
            Self::Refresh => format!("Cookie 刷新未完成: {alias} ({message})"),
            Self::PasswordChange => format!("远端密码修改未完成: {alias} ({message})"),
            Self::GitTokenRefresh => {
                format!("Git token 状态刷新未完成: {alias} ({message})")
            }
            Self::GitToken => format!("Git token 获取未完成: {alias} ({message})"),
            Self::BrowserLogin => format!("浏览器登录未完成: {alias} ({message})"),
        }
    }

    fn invalid_input_message(self) -> &'static str {
        match self {
            Self::Add => "重新输入格式无效，请检查邮箱和密码",
            Self::Refresh => "重新输入格式无效，请检查密码",
            Self::PasswordChange => "重新输入格式无效，请检查当前密码",
            Self::GitTokenRefresh => "重新输入格式无效，请检查账号密码",
            Self::GitToken => "重新输入格式无效，请检查账号密码",
            Self::BrowserLogin => "重新输入格式无效，请检查密码",
        }
    }
}

#[derive(Clone)]
pub(super) struct RemotePasswordChangeBrowserPlan {
    alias: String,
    email: Option<String>,
    current_password: String,
    new_password: String,
    startup_error: Option<String>,
}

#[derive(Clone)]
pub(super) struct GitTokenBrowserPlan {
    pub(super) alias: String,
    email: Option<String>,
    password: String,
    startup_error: Option<String>,
    recovery_error: Option<String>,
    pub(super) skip_existing_token: bool,
    pub(super) force_credentials_login: bool,
}

#[derive(Clone)]
pub(super) struct BrowserLoginBrowserPlan {
    alias: String,
    email: Option<String>,
    password: String,
    startup_error: Option<String>,
    password_error: Option<String>,
}

#[derive(Clone)]
pub(super) enum CredentialBrowserPlan {
    Add(CredentialAccountPlan),
    Refresh(AliasPasswordPlan),
    PasswordChange(RemotePasswordChangeBrowserPlan),
    GitTokenRefresh(GitTokenBrowserPlan),
    GitToken(GitTokenBrowserPlan),
    BrowserLogin(BrowserLoginBrowserPlan),
}

impl CredentialBrowserPlan {
    fn alias(&self) -> &str {
        match self {
            Self::Add(plan) => &plan.alias,
            Self::Refresh(plan) => &plan.alias,
            Self::PasswordChange(plan) => &plan.alias,
            Self::GitTokenRefresh(plan) => &plan.alias,
            Self::GitToken(plan) => &plan.alias,
            Self::BrowserLogin(plan) => &plan.alias,
        }
    }

    fn email(&self) -> Option<&str> {
        match self {
            Self::Add(plan) => Some(&plan.email),
            Self::Refresh(plan) => plan.email.as_deref(),
            Self::PasswordChange(plan) => plan.email.as_deref(),
            Self::GitTokenRefresh(plan) => plan.email.as_deref(),
            Self::GitToken(plan) => plan.email.as_deref(),
            Self::BrowserLogin(plan) => plan.email.as_deref(),
        }
    }

    fn email_owned(&self) -> Option<String> {
        self.email().map(ToOwned::to_owned)
    }

    fn password(&self) -> &str {
        match self {
            Self::Add(plan) => &plan.password,
            Self::Refresh(plan) => &plan.password,
            Self::PasswordChange(plan) => &plan.current_password,
            Self::GitTokenRefresh(plan) => &plan.password,
            Self::GitToken(plan) => &plan.password,
            Self::BrowserLogin(plan) => &plan.password,
        }
    }

    fn startup_error(&self) -> Option<&str> {
        match self {
            Self::PasswordChange(plan) => plan.startup_error.as_deref(),
            Self::GitTokenRefresh(plan) => plan.startup_error.as_deref(),
            Self::GitToken(plan) => plan.startup_error.as_deref(),
            Self::BrowserLogin(plan) => plan.startup_error.as_deref(),
            Self::Add(_) | Self::Refresh(_) => None,
        }
    }

    fn recovery_error(&self) -> Option<&str> {
        match self {
            Self::GitTokenRefresh(plan) | Self::GitToken(plan) => plan.recovery_error.as_deref(),
            Self::BrowserLogin(plan) => plan.password_error.as_deref(),
            Self::Add(_) | Self::Refresh(_) | Self::PasswordChange(_) => None,
        }
    }

    fn skip_existing_token(&self) -> bool {
        matches!(self, Self::GitToken(plan) if plan.skip_existing_token)
    }

    fn apply_credentials(&mut self, input: BrowserCredentialsTaskInput) {
        match self {
            Self::Add(plan) => {
                if let Some(email) = input
                    .email
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    plan.email = email.to_string();
                    plan.alias = make_alias(plan.alias_hint.as_deref(), email);
                }
                plan.password = input.password;
            }
            Self::Refresh(plan) => {
                plan.password = input.password;
            }
            Self::PasswordChange(plan) => {
                plan.current_password = input.password;
            }
            Self::GitTokenRefresh(plan) => {
                plan.password = input.password;
            }
            Self::GitToken(plan) => {
                plan.password = input.password;
            }
            Self::BrowserLogin(plan) => {
                if let Some(email) = input
                    .email
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    plan.email = Some(email.to_string());
                }
                plan.password = input.password;
                plan.password_error = None;
            }
        }
    }
}

fn credential_browser_batch_item_from_report(
    report: CredentialLoginReport,
) -> (CredentialBatchItem, bool, Option<String>) {
    let failure_message = credential_report_failure_message(&report);
    let status = if failure_message.is_some() {
        CredentialBatchItemStatus::Failed
    } else {
        match report.status {
            crate::CredentialLoginStatus::Imported => CredentialBatchItemStatus::Imported,
            crate::CredentialLoginStatus::SkippedDuplicateEmail => {
                CredentialBatchItemStatus::SkippedDuplicateEmail
            }
            crate::CredentialLoginStatus::UpdatedExisting => CredentialBatchItemStatus::Updated,
        }
    };
    (
        CredentialBatchItem {
            alias: report.alias.clone(),
            email: report.email.clone(),
            status,
            existing_alias: report.existing_alias.clone(),
            report: Some(report),
            error: failure_message.clone(),
        },
        failure_message.is_none(),
        failure_message,
    )
}

pub(super) fn classify_credential_batch_item(
    batch: &mut CredentialBatchReport,
    report: CredentialLoginReport,
) -> (CredentialBatchItem, bool, Option<String>) {
    let (item, completed, failure_message) = credential_browser_batch_item_from_report(report);
    match item.status {
        CredentialBatchItemStatus::Imported => batch.imported_count += 1,
        CredentialBatchItemStatus::Updated => batch.updated_count += 1,
        CredentialBatchItemStatus::SkippedDuplicateEmail => {
            batch.skipped_duplicate_email_count += 1
        }
        CredentialBatchItemStatus::Failed => batch.failed_count += 1,
    }
    (item, completed, failure_message)
}

enum CredentialBrowserCommitReport {
    Credential(CredentialLoginReport),
    PasswordChange(OverleafPasswordChangeReport),
    GitToken(AccountGitTokenRefreshReport),
    GitTokenSkipped,
    BrowserLogin(BrowserAutoLoginReport),
}

#[derive(Clone)]
pub(super) enum CredentialBrowserBatchItem {
    Credential(CredentialBatchItem),
    PasswordChange(OverleafPasswordChangeBatchItem),
    GitToken(AccountGitTokenRefreshBatchItem),
    BrowserLogin(BrowserAutoLoginBatchItem),
}

pub(super) enum CredentialBrowserBatchCompletion {
    Standard,
    ImportPostActions(Box<AccountImportPostActionBatchContext>),
    CredentialRefresh(CredentialRefreshContinuation),
}

pub(super) struct CredentialBrowserBatchPlan {
    pub operation: CredentialBrowserOperation,
    pub plans: Vec<CredentialBrowserPlan>,
}

pub(super) fn queue_credential_browser_batch_response(
    state: &mut ApiState,
    task_id: &str,
    batch: CredentialBrowserBatchPlan,
    initial_items: Vec<CredentialBrowserBatchItem>,
    completion: CredentialBrowserBatchCompletion,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    let CredentialBrowserBatchPlan { operation, plans } = batch;
    let coordinator = state.browser_batch_coordinator();
    let registered = match coordinator
        .register_batch(task_id, plans.iter().map(|plan| plan.alias().to_string()))
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let message = format!("failed to register browser credential batch: {error:?}");
            fail_tracked_task(state, Some(task_id), message.clone());
            return json_response(500, &ApiErrorBody { error: message });
        }
    };
    let ordered_item_ids = registered
        .items
        .iter()
        .map(|item| item.item_id.clone())
        .collect::<Vec<_>>();
    let accumulator = Arc::new(StdMutex::new(CredentialBrowserBatchAccumulator {
        initial_items,
        ordered_item_ids: ordered_item_ids.clone(),
        items: BTreeMap::new(),
        completion,
        finalized: false,
    }));
    let store = AccountStore::new(state.config.accounts_path());
    let metadata_refresher = Arc::clone(&state.metadata_refresher);
    let session_identity_validator = Arc::clone(&state.session_identity_validator);
    let secret_backend = Arc::clone(&state.secret_backend);
    let commit_lock = state.account_commit_lock();
    let has_proxy_fallback = state.config.browser_proxy_policy.has_fallback();
    let plans = plans
        .into_iter()
        .zip(ordered_item_ids)
        .map(|(plan, item_id)| (item_id, plan))
        .collect::<BTreeMap<_, _>>();
    let batch_job = CredentialBrowserBatchJob {
        task_id: task_id.to_string(),
        plans,
        operation,
        local_chrome,
        store,
        metadata_refresher,
        session_identity_validator,
        secret_backend,
        commit_lock,
        coordinator: coordinator.clone(),
        accumulator,
        has_proxy_fallback,
        now_unix,
    };
    let job = ApiBackgroundJob::new(operation.job_name(), move |shared_state| {
        batch_job.run(shared_state);
    });
    if let Err(response) = state.enqueue_background_job(job) {
        let _ = coordinator.remove_batch(task_id);
        fail_tracked_task(state, Some(task_id), response.body.clone());
        return response;
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        operation.queue_message(registered.max_concurrency),
    );
    registration_waiting_response(state, task_id)
}

pub(super) fn queue_browser_login_credential_batch_response(
    state: &mut ApiState,
    task_id: &str,
    aliases: Vec<String>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            fail_tracked_task(state, Some(task_id), error.to_string());
            return io_error_response(error);
        }
    };
    let plans = aliases
        .into_iter()
        .map(|alias| {
            let Some(record) = document.accounts.get(&alias) else {
                return CredentialBrowserPlan::BrowserLogin(BrowserLoginBrowserPlan {
                    alias: alias.clone(),
                    email: None,
                    password: String::new(),
                    startup_error: Some(format!("account alias not found: {alias}")),
                    password_error: None,
                });
            };
            let (password, password_error) = match read_account_password_secret_for_recovery(
                record,
                &alias,
                state.secret_backend.as_ref(),
            ) {
                Ok(password) => (password, None),
                Err(AccountSecretStoreError::Missing { .. }) => (String::new(), None),
                Err(error) => (
                    String::new(),
                    Some(account_secret_store_error_message(&error)),
                ),
            };
            CredentialBrowserPlan::BrowserLogin(BrowserLoginBrowserPlan {
                alias,
                email: record.email.clone(),
                password,
                startup_error: None,
                password_error,
            })
        })
        .collect();
    queue_credential_browser_batch_response(
        state,
        task_id,
        CredentialBrowserBatchPlan {
            operation: CredentialBrowserOperation::BrowserLogin,
            plans,
        },
        Vec::new(),
        CredentialBrowserBatchCompletion::Standard,
        local_chrome,
        now_unix,
    )
}

struct CredentialBrowserBatchAccumulator {
    initial_items: Vec<CredentialBrowserBatchItem>,
    ordered_item_ids: Vec<String>,
    items: BTreeMap<String, CredentialBrowserBatchItem>,
    completion: CredentialBrowserBatchCompletion,
    finalized: bool,
}

struct CredentialBrowserBatchJob {
    task_id: String,
    plans: BTreeMap<String, CredentialBrowserPlan>,
    operation: CredentialBrowserOperation,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    store: AccountStore,
    metadata_refresher: Arc<dyn AccountSessionMetadataRefresher + Send + Sync>,
    session_identity_validator: Arc<dyn AccountSessionIdentityValidator + Send + Sync>,
    secret_backend: Arc<dyn SecretBackend + Send + Sync>,
    commit_lock: Arc<StdMutex<()>>,
    coordinator: BrowserBatchCoordinator,
    accumulator: Arc<StdMutex<CredentialBrowserBatchAccumulator>>,
    has_proxy_fallback: bool,
    now_unix: i64,
}

#[derive(Clone)]
struct CredentialBrowserItemJob {
    task_id: String,
    item_id: String,
    plan: CredentialBrowserPlan,
    operation: CredentialBrowserOperation,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    store: AccountStore,
    metadata_refresher: Arc<dyn AccountSessionMetadataRefresher + Send + Sync>,
    session_identity_validator: Arc<dyn AccountSessionIdentityValidator + Send + Sync>,
    secret_backend: Arc<dyn SecretBackend + Send + Sync>,
    commit_lock: Arc<StdMutex<()>>,
    coordinator: BrowserBatchCoordinator,
    accumulator: Arc<StdMutex<CredentialBrowserBatchAccumulator>>,
    has_proxy_fallback: bool,
    now_unix: i64,
}

enum CredentialBrowserResumeAction {
    Credentials(BrowserCredentialsTaskInput),
    Login(LoginResult),
    PromptCredentials,
    RetryAutomation,
    Cancelled(String),
    Failed(String),
}

enum GitTokenSavedCookieAction {
    NotApplicable,
    Completed(AccountGitTokenRefreshReport),
    Skipped,
    Recover(String),
    Failed(String),
}

enum BrowserLoginSavedCookieAction {
    NotApplicable,
    Completed(BrowserAutoLoginReport),
    Recover(String),
    Failed(String),
}

enum SavedCookieValidation {
    Valid {
        session_cookie: String,
        cookie_expiry: Option<f64>,
    },
    Recover(String),
    Failed(String),
}

fn validate_saved_account_cookie(
    document: &AccountsDocument,
    alias: &str,
    email: Option<&str>,
    now_unix: i64,
    runtime: &tokio::runtime::Runtime,
    validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
    backend: &(dyn SecretBackend + Send + Sync),
) -> SavedCookieValidation {
    let Some(record) = document.accounts.get(alias) else {
        return SavedCookieValidation::Failed(format!("account alias not found: {alias}"));
    };

    if record
        .cookie_expiry
        .is_some_and(|expiry| expiry <= now_unix as f64)
    {
        return SavedCookieValidation::Recover("已保存 Cookie 已过期".to_string());
    }

    let cookies = match resolve_account_cookies_for_recovery(record, alias, backend) {
        Ok(cookies) => cookies,
        Err(AccountSecretStoreError::Missing { .. }) => {
            return SavedCookieValidation::Recover("没有可用 Cookie".to_string());
        }
        Err(AccountSecretStoreError::ReadFailed { alias, category })
        | Err(AccountSecretStoreError::WriteFailed { alias, category }) => {
            return SavedCookieValidation::Failed(format!(
                "failed to read stored {category} secret for account alias: {alias}"
            ));
        }
    };
    let Some(session_cookie) = cookies
        .get(OVERLEAF_SESSION_COOKIE_NAME)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return SavedCookieValidation::Recover("没有可用 Cookie".to_string());
    };

    match runtime.block_on(validator.validate(alias, email, &cookies)) {
        Ok(_) => SavedCookieValidation::Valid {
            session_cookie,
            cookie_expiry: record.cookie_expiry,
        },
        Err(error) if error.requires_cookie_recovery() => {
            SavedCookieValidation::Recover("已保存 Cookie 无效或身份不匹配".to_string())
        }
        Err(error) => SavedCookieValidation::Failed(account_session_error_message(&error)),
    }
}

impl CredentialBrowserBatchJob {
    fn run(mut self, shared_state: Arc<StdMutex<ApiState>>) {
        let (completed_tx, completed_rx) = std::sync::mpsc::channel::<String>();
        let mut handles = BTreeMap::new();

        loop {
            if browser_task_cancel_requested(&shared_state, &self.task_id) {
                if let Ok(snapshot) = self.coordinator.snapshot(&self.task_id) {
                    for item in snapshot.items {
                        if !item.phase.is_terminal() {
                            let _ = self
                                .coordinator
                                .request_item_cancel(&self.task_id, &item.item_id);
                        }
                    }
                }
                finish_credential_browser_batch_if_terminal(
                    &shared_state,
                    &self.coordinator,
                    &self.accumulator,
                    &self.task_id,
                    self.operation,
                );
            }

            let leases = match self.coordinator.claim_available_for(&self.task_id) {
                Ok(leases) => leases,
                Err(error) => {
                    self.finish_scheduler_error(&shared_state, error);
                    break;
                }
            };
            for lease in leases {
                let Some(plan) = self.plans.remove(&lease.item_id) else {
                    self.finish_missing_plan(&shared_state, &lease);
                    continue;
                };
                let item_job = CredentialBrowserItemJob {
                    task_id: self.task_id.clone(),
                    item_id: lease.item_id.clone(),
                    plan,
                    operation: self.operation,
                    local_chrome: Arc::clone(&self.local_chrome),
                    store: self.store.clone(),
                    metadata_refresher: Arc::clone(&self.metadata_refresher),
                    session_identity_validator: Arc::clone(&self.session_identity_validator),
                    secret_backend: Arc::clone(&self.secret_backend),
                    commit_lock: Arc::clone(&self.commit_lock),
                    coordinator: self.coordinator.clone(),
                    accumulator: Arc::clone(&self.accumulator),
                    has_proxy_fallback: self.has_proxy_fallback,
                    now_unix: self.now_unix,
                };
                let worker = item_job.clone();
                let panic_fallback = item_job.clone();
                let completed_tx = completed_tx.clone();
                let completed_item_id = lease.item_id.clone();
                let worker_state = Arc::clone(&shared_state);
                let panic_state = Arc::clone(&shared_state);
                match std::thread::Builder::new()
                    .name(self.operation.worker_name().to_string())
                    .spawn(move || {
                        let result = catch_unwind(AssertUnwindSafe(|| worker.run(worker_state)));
                        if result.is_err() {
                            panic_fallback.finish_failed(
                                &panic_state,
                                "浏览器 worker 异常退出，已将该账号标记为失败",
                            );
                        }
                        let _ = completed_tx.send(completed_item_id);
                    }) {
                    Ok(handle) => {
                        handles.insert(lease.item_id, handle);
                    }
                    Err(error) => {
                        item_job.finish_failed(
                            &shared_state,
                            format!("failed to spawn credential browser worker: {error}"),
                        );
                    }
                }
            }

            while let Ok(item_id) = completed_rx.try_recv() {
                if let Some(handle) = handles.remove(&item_id) {
                    let _ = handle.join();
                }
            }

            match self.coordinator.snapshot(&self.task_id) {
                Ok(snapshot) if snapshot.is_terminal() => {
                    finish_credential_browser_batch_if_terminal(
                        &shared_state,
                        &self.coordinator,
                        &self.accumulator,
                        &self.task_id,
                        self.operation,
                    );
                    break;
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    self.finish_scheduler_error(&shared_state, error);
                    break;
                }
            }
        }

        for (_, handle) in handles {
            let _ = handle.join();
        }
    }

    fn finish_scheduler_error(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        error: crate::BrowserBatchError,
    ) {
        let Ok(mut state) = shared_state.lock() else {
            return;
        };
        let task_is_terminal = state
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
        if task_is_terminal {
            return;
        }
        let message = format!("浏览器批次调度异常，任务已停止：{error:?}");
        let _ = state
            .tasks
            .append_log(&self.task_id, TaskLogLevel::Error, message.clone());
        let _ = state.tasks.fail_task(&self.task_id, message);
    }

    fn finish_missing_plan(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        lease: &crate::BrowserBatchItemLease,
    ) {
        let message = "浏览器批次内部计划缺失，已标记该账号失败".to_string();
        if let Ok(mut accumulator) = self.accumulator.lock() {
            accumulator
                .items
                .entry(lease.item_id.clone())
                .or_insert_with(|| {
                    missing_credential_browser_batch_item(self.operation, &lease.alias, false)
                });
        }
        let _ = self
            .coordinator
            .mark_failed(&self.task_id, &lease.item_id, message.clone());
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &self.task_id,
                TaskLogLevel::Error,
                format!("{}：{}", lease.alias, message),
            );
        }
        update_credential_browser_batch_progress(shared_state, &self.coordinator, &self.task_id);
        finish_credential_browser_batch_if_terminal(
            shared_state,
            &self.coordinator,
            &self.accumulator,
            &self.task_id,
            self.operation,
        );
    }
}

impl CredentialBrowserItemJob {
    fn commit(
        &self,
        runtime: &tokio::runtime::Runtime,
        login: LoginResult,
        browser: &(dyn BrowserAutomation + Send + Sync),
    ) -> Result<CredentialBrowserCommitReport, String> {
        if matches!(
            self.plan,
            CredentialBrowserPlan::GitTokenRefresh(_)
                | CredentialBrowserPlan::GitToken(_)
                | CredentialBrowserPlan::BrowserLogin(_)
        ) {
            let report = runtime
                .block_on(
                    refresh_account_cookie_from_login_result_in_store_with_backend(
                        &self.store,
                        self.plan.alias(),
                        SecretText::new(self.plan.password().to_string()),
                        login.clone(),
                        self.metadata_refresher.as_ref(),
                        self.now_unix,
                        self.secret_backend.as_ref(),
                    ),
                )
                .map_err(|error| account_credential_error_message(&error))?;
            if let Some(error) = credential_report_failure_message(&report) {
                return Err(error);
            }
        }
        match &self.plan {
            CredentialBrowserPlan::Add(plan) => runtime
                .block_on(add_account_from_login_result_in_store_with_backend(
                    &self.store,
                    plan.alias_hint.as_deref(),
                    &plan.email,
                    SecretText::new(plan.password.clone()),
                    login,
                    self.metadata_refresher.as_ref(),
                    self.now_unix,
                    self.secret_backend.as_ref(),
                ))
                .map(CredentialBrowserCommitReport::Credential)
                .map_err(|error| account_credential_error_message(&error)),
            CredentialBrowserPlan::Refresh(plan) => runtime
                .block_on(
                    refresh_account_cookie_from_login_result_in_store_with_backend(
                        &self.store,
                        &plan.alias,
                        SecretText::new(plan.password.clone()),
                        login,
                        self.metadata_refresher.as_ref(),
                        self.now_unix,
                        self.secret_backend.as_ref(),
                    ),
                )
                .map(CredentialBrowserCommitReport::Credential)
                .map_err(|error| account_credential_error_message(&error)),
            CredentialBrowserPlan::PasswordChange(plan) => runtime
                .block_on(
                    change_account_overleaf_password_from_login_result_in_store_with_backend(
                        &self.store,
                        &plan.alias,
                        SecretText::new(plan.current_password.clone()),
                        SecretText::new(plan.new_password.clone()),
                        login,
                        browser,
                        self.now_unix,
                        self.secret_backend.as_ref(),
                    ),
                )
                .map(CredentialBrowserCommitReport::PasswordChange)
                .map_err(|error| overleaf_password_change_error_message(&error)),
            CredentialBrowserPlan::GitTokenRefresh(plan) => runtime
                .block_on(
                    refresh_account_git_token_metadata_with_browser_in_store_with_backend(
                        &self.store,
                        &plan.alias,
                        browser,
                        self.secret_backend.as_ref(),
                    ),
                )
                .map(CredentialBrowserCommitReport::GitToken)
                .map_err(|error| account_git_token_error_message(&error)),
            CredentialBrowserPlan::GitToken(plan) => runtime
                .block_on(
                    generate_account_git_token_with_browser_in_store_with_backend(
                        &self.store,
                        std::slice::from_ref(&plan.alias),
                        browser,
                        self.now_unix,
                        self.secret_backend.as_ref(),
                    ),
                )
                .map_err(|error| account_git_token_error_message(&error))?
                .items
                .into_iter()
                .next()
                .ok_or_else(|| "Git token generation returned no item".to_string())
                .and_then(|item| match (item.report, item.error) {
                    (Some(report), _) if item.refreshed => {
                        Ok(CredentialBrowserCommitReport::GitToken(report))
                    }
                    (_, _) if item.skipped => Ok(CredentialBrowserCommitReport::GitTokenSkipped),
                    (_, Some(error)) => Err(account_git_token_error_message(&error)),
                    _ => Err("Git token generation did not complete".to_string()),
                }),
            CredentialBrowserPlan::BrowserLogin(plan) => runtime
                .block_on(open_account_browser_login_with_login_result(
                    &plan.alias,
                    plan.email.clone(),
                    &login,
                    self.local_chrome.as_ref(),
                ))
                .map(CredentialBrowserCommitReport::BrowserLogin)
                .map_err(|error| browser_auto_login_error_message(&error)),
        }
    }

    fn log_stage(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        level: TaskLogLevel,
        stage: impl AsRef<str>,
    ) {
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &self.task_id,
                level,
                format!("{}：{}", self.plan.alias(), stage.as_ref()),
            );
        }
    }

    fn cleanup_session(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        runtime: &tokio::runtime::Runtime,
        session: LocalChromeCdpSession,
        reason: &str,
    ) {
        self.log_stage(
            shared_state,
            TaskLogLevel::Info,
            format!("阶段：清理浏览器会话（{reason}）"),
        );
        if runtime.block_on(session.cleanup()).is_err() {
            self.log_stage(
                shared_state,
                TaskLogLevel::Warning,
                "阶段：浏览器会话清理返回错误",
            );
        }
    }

    fn current_git_token_is_reusable(&self) -> Result<bool, String> {
        let document = self
            .store
            .load()
            .map_err(|error| format!("failed to recheck stored Git token: {error}"))?;
        Ok(document
            .accounts
            .get(self.plan.alias())
            .is_some_and(|record| {
                has_reusable_git_token(
                    record,
                    self.plan.alias(),
                    self.now_unix,
                    self.secret_backend.as_ref(),
                )
            }))
    }

    fn run(mut self, shared_state: Arc<StdMutex<ApiState>>) {
        self.log_stage(&shared_state, TaskLogLevel::Info, "阶段：开始处理");
        if let Some(message) = self.plan.startup_error().map(ToOwned::to_owned) {
            self.log_stage(&shared_state, TaskLogLevel::Warning, "阶段：启动校验失败");
            self.finish_failed(&shared_state, message);
            return;
        }
        if self.plan.skip_existing_token() {
            self.log_stage(
                &shared_state,
                TaskLogLevel::Info,
                "阶段：检查已有 Git token",
            );
            match self.current_git_token_is_reusable() {
                Ok(true) => {
                    self.finish_skipped(&shared_state);
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    self.finish_failed(&shared_state, error);
                    return;
                }
            }
        }

        match self.try_browser_login_with_saved_cookie(&shared_state) {
            BrowserLoginSavedCookieAction::NotApplicable => {}
            BrowserLoginSavedCookieAction::Completed(report) => {
                self.finish_completed(
                    &shared_state,
                    CredentialBrowserCommitReport::BrowserLogin(report),
                );
                return;
            }
            BrowserLoginSavedCookieAction::Recover(message) => {
                if let Some(error) = self.plan.recovery_error().map(ToOwned::to_owned) {
                    self.finish_failed(&shared_state, error);
                    return;
                }
                self.log_stage(
                    &shared_state,
                    TaskLogLevel::Info,
                    format!("{message}，转账号密码登录"),
                );
            }
            BrowserLoginSavedCookieAction::Failed(message) => {
                self.finish_failed(&shared_state, message);
                return;
            }
        }

        self.log_stage(&shared_state, TaskLogLevel::Info, "阶段：启动浏览器会话");
        let (mut runtime, mut session) = match open_account_browser_session(
            Arc::clone(&self.local_chrome),
            ChromeProxyAttempt::Primary,
        ) {
            Ok(value) => value,
            Err(response) => {
                self.log_stage(
                    &shared_state,
                    TaskLogLevel::Warning,
                    "阶段：浏览器会话启动失败",
                );
                self.finish_failed(&shared_state, response.body);
                return;
            }
        };

        self.log_stage(
            &shared_state,
            TaskLogLevel::Info,
            format!("阶段：{}", self.operation.running_message()),
        );

        match self.try_git_token_with_saved_cookie(&shared_state, &runtime, &session) {
            GitTokenSavedCookieAction::NotApplicable => {}
            GitTokenSavedCookieAction::Completed(report) => {
                self.cleanup_session(&shared_state, &runtime, session, "Git token 已完成");
                self.finish_completed(
                    &shared_state,
                    CredentialBrowserCommitReport::GitToken(report),
                );
                return;
            }
            GitTokenSavedCookieAction::Skipped => {
                self.cleanup_session(&shared_state, &runtime, session, "已有有效 Git token");
                self.finish_skipped(&shared_state);
                return;
            }
            GitTokenSavedCookieAction::Recover(message) => {
                if let Some(error) = self.plan.recovery_error().map(ToOwned::to_owned) {
                    self.cleanup_session(&shared_state, &runtime, session, "恢复不可用");
                    self.finish_failed(&shared_state, error);
                    return;
                }
                self.log_stage(
                    &shared_state,
                    TaskLogLevel::Warning,
                    format!("{message}，正在转账号密码恢复"),
                );
            }
            GitTokenSavedCookieAction::Failed(message) => {
                self.cleanup_session(&shared_state, &runtime, session, "Cookie/Git 校验失败");
                self.finish_failed(&shared_state, message);
                return;
            }
        }

        let mut proxy_fallback_used = false;
        let login = 'login: loop {
            if browser_task_cancel_requested(&shared_state, &self.task_id) {
                self.cleanup_session(&shared_state, &runtime, session, "用户取消");
                self.finish_cancelled(&shared_state, "用户取消");
                return;
            }
            self.log_stage(&shared_state, TaskLogLevel::Info, "阶段：提交账号密码");
            let result = if self.plan.password().trim().is_empty() {
                Err(AccountCredentialError::InvalidCredentials)
            } else {
                match self.plan.email().map(str::trim) {
                    Some(email) if !email.is_empty() => runtime
                        .block_on(session.automation().login_with_credentials(
                            CredentialsLoginInput {
                                email: email.to_string(),
                                password: SecretText::new(self.plan.password().to_string()),
                            },
                        ))
                        .map_err(AccountCredentialError::from_browser),
                    _ => Err(AccountCredentialError::MissingEmail {
                        alias: self.plan.alias().to_string(),
                    }),
                }
            };

            match result {
                Ok(login) => break 'login login,
                Err(AccountCredentialError::InvalidCredentials) => {
                    self.log_stage(
                        &shared_state,
                        TaskLogLevel::Warning,
                        "阶段：账号密码未通过，等待重新输入",
                    );
                    match self.wait_for_input(
                        &shared_state,
                        &runtime,
                        &session,
                        TaskUserInputKind::NewBrowserCredentials,
                        self.operation.invalid_credentials_message(),
                    ) {
                        CredentialBrowserResumeAction::Credentials(input) => {
                            let _ = self.coordinator.resume_waiting(
                                &self.task_id,
                                &self.item_id,
                                TaskUserInputKind::NewBrowserCredentials,
                            );
                            self.plan.apply_credentials(input);
                        }
                        CredentialBrowserResumeAction::Cancelled(message) => {
                            self.cleanup_session(&shared_state, &runtime, session, "用户取消");
                            self.finish_cancelled(&shared_state, &message);
                            return;
                        }
                        CredentialBrowserResumeAction::Failed(message) => {
                            self.cleanup_session(&shared_state, &runtime, session, "等待输入失败");
                            self.finish_failed(&shared_state, message);
                            return;
                        }
                        _ => continue,
                    }
                }
                Err(AccountCredentialError::ChallengeRequired) => loop {
                    self.log_stage(&shared_state, TaskLogLevel::Warning, "阶段：等待 reCAPTCHA");
                    match self.wait_for_input(
                        &shared_state,
                        &runtime,
                        &session,
                        TaskUserInputKind::CaptchaCompleted,
                        "检测到可见 reCAPTCHA，请在对应浏览器窗口中完成验证",
                    ) {
                        CredentialBrowserResumeAction::Login(login) => {
                            self.log_stage(
                                &shared_state,
                                TaskLogLevel::Info,
                                "阶段：收到验证结果，继续登录",
                            );
                            let _ = self.coordinator.resume_waiting(
                                &self.task_id,
                                &self.item_id,
                                TaskUserInputKind::CaptchaCompleted,
                            );
                            break 'login login;
                        }
                        CredentialBrowserResumeAction::PromptCredentials => {
                            self.log_stage(
                                &shared_state,
                                TaskLogLevel::Warning,
                                "阶段：验证后需要重新输入账号密码",
                            );
                            let _ = self.coordinator.resume_waiting(
                                &self.task_id,
                                &self.item_id,
                                TaskUserInputKind::CaptchaCompleted,
                            );
                            match self.wait_for_input(
                                &shared_state,
                                &runtime,
                                &session,
                                TaskUserInputKind::NewBrowserCredentials,
                                self.operation.invalid_credentials_message(),
                            ) {
                                CredentialBrowserResumeAction::Credentials(input) => {
                                    let _ = self.coordinator.resume_waiting(
                                        &self.task_id,
                                        &self.item_id,
                                        TaskUserInputKind::NewBrowserCredentials,
                                    );
                                    self.plan.apply_credentials(input);
                                    continue 'login;
                                }
                                CredentialBrowserResumeAction::Cancelled(message) => {
                                    self.cleanup_session(
                                        &shared_state,
                                        &runtime,
                                        session,
                                        "用户取消",
                                    );
                                    self.finish_cancelled(&shared_state, &message);
                                    return;
                                }
                                CredentialBrowserResumeAction::Failed(message) => {
                                    self.cleanup_session(
                                        &shared_state,
                                        &runtime,
                                        session,
                                        "补充凭据失败",
                                    );
                                    self.finish_failed(&shared_state, message);
                                    return;
                                }
                                _ => continue,
                            }
                        }
                        CredentialBrowserResumeAction::RetryAutomation => {
                            self.log_stage(
                                &shared_state,
                                TaskLogLevel::Info,
                                "阶段：重试浏览器自动化",
                            );
                            let _ = self.coordinator.resume_waiting(
                                &self.task_id,
                                &self.item_id,
                                TaskUserInputKind::CaptchaCompleted,
                            );
                            continue 'login;
                        }
                        CredentialBrowserResumeAction::Cancelled(message) => {
                            self.cleanup_session(&shared_state, &runtime, session, "用户取消");
                            self.finish_cancelled(&shared_state, &message);
                            return;
                        }
                        CredentialBrowserResumeAction::Failed(message) => {
                            self.cleanup_session(
                                &shared_state,
                                &runtime,
                                session,
                                "验证码等待失败",
                            );
                            self.finish_failed(&shared_state, message);
                            return;
                        }
                        CredentialBrowserResumeAction::Credentials(_) => continue,
                    }
                },
                Err(AccountCredentialError::RobotVerificationBlocked)
                    if self.has_proxy_fallback && !proxy_fallback_used =>
                {
                    self.cleanup_session(&shared_state, &runtime, session, "切换代理");
                    self.log_stage(&shared_state, TaskLogLevel::Warning, "阶段：切换代理后重试");
                    match open_account_browser_session(
                        Arc::clone(&self.local_chrome),
                        ChromeProxyAttempt::Fallback,
                    ) {
                        Ok((new_runtime, new_session)) => {
                            runtime = new_runtime;
                            session = new_session;
                            proxy_fallback_used = true;
                            self.log_stage(
                                &shared_state,
                                TaskLogLevel::Warning,
                                "机器人验证未通过，已切换系统代理重试一次",
                            );
                        }
                        Err(response) => {
                            self.log_stage(
                                &shared_state,
                                TaskLogLevel::Warning,
                                "阶段：代理重试启动失败",
                            );
                            self.finish_failed(&shared_state, response.body);
                            return;
                        }
                    }
                }
                Err(error) => {
                    self.cleanup_session(&shared_state, &runtime, session, "登录失败");
                    self.finish_failed(&shared_state, account_credential_error_message(&error));
                    return;
                }
            }
        };

        self.log_stage(
            &shared_state,
            TaskLogLevel::Info,
            "阶段：登录成功，保存账号结果",
        );
        let result = match self.commit_lock.lock() {
            Ok(_guard) => self.commit(&runtime, login, session.automation()),
            Err(_) => Err("account commit lock poisoned".to_string()),
        };
        self.cleanup_session(&shared_state, &runtime, session, "任务完成");
        match result {
            Ok(item_report) => self.finish_completed(&shared_state, item_report),
            Err(error) => self.finish_failed(&shared_state, error),
        }
    }

    fn try_browser_login_with_saved_cookie(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
    ) -> BrowserLoginSavedCookieAction {
        let CredentialBrowserPlan::BrowserLogin(plan) = &self.plan else {
            return BrowserLoginSavedCookieAction::NotApplicable;
        };
        let document = match self.store.load() {
            Ok(document) => document,
            Err(error) => return BrowserLoginSavedCookieAction::Failed(error.to_string()),
        };
        let runtime = match build_api_runtime() {
            Ok(runtime) => runtime,
            Err(response) => return BrowserLoginSavedCookieAction::Failed(response.body),
        };
        self.log_stage(
            shared_state,
            TaskLogLevel::Info,
            "阶段：校验已保存 Cookie 身份",
        );
        let (session_cookie, cookie_expiry) = match validate_saved_account_cookie(
            &document,
            &plan.alias,
            plan.email.as_deref(),
            self.now_unix,
            &runtime,
            self.session_identity_validator.as_ref(),
            self.secret_backend.as_ref(),
        ) {
            SavedCookieValidation::Valid {
                session_cookie,
                cookie_expiry,
            } => {
                self.log_stage(
                    shared_state,
                    TaskLogLevel::Info,
                    "阶段：Cookie 身份校验通过，打开登录窗口",
                );
                (session_cookie, cookie_expiry)
            }
            SavedCookieValidation::Recover(message) => {
                return BrowserLoginSavedCookieAction::Recover(message);
            }
            SavedCookieValidation::Failed(message) => {
                return BrowserLoginSavedCookieAction::Failed(message);
            }
        };
        match runtime.block_on(open_account_browser_login_with_session(
            &plan.alias,
            plan.email.clone(),
            session_cookie,
            cookie_expiry,
            self.local_chrome.as_ref(),
        )) {
            Ok(report) => BrowserLoginSavedCookieAction::Completed(report),
            Err(BrowserAutoLoginError::InvalidCookie { .. }) => {
                BrowserLoginSavedCookieAction::Recover(
                    "已保存 Cookie 在打开窗口时失效，转账号密码登录".to_string(),
                )
            }
            Err(error) => {
                BrowserLoginSavedCookieAction::Failed(browser_auto_login_error_message(&error))
            }
        }
    }

    fn try_git_token_with_saved_cookie(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        runtime: &tokio::runtime::Runtime,
        session: &LocalChromeCdpSession,
    ) -> GitTokenSavedCookieAction {
        let (plan, generate_token) = match &self.plan {
            CredentialBrowserPlan::GitToken(plan) => (plan, true),
            CredentialBrowserPlan::GitTokenRefresh(plan) => (plan, false),
            _ => return GitTokenSavedCookieAction::NotApplicable,
        };
        if plan.force_credentials_login {
            return GitTokenSavedCookieAction::NotApplicable;
        }
        self.log_stage(
            shared_state,
            TaskLogLevel::Info,
            "阶段：校验已保存 Cookie 和 Git token 状态",
        );
        let document = match self.store.load() {
            Ok(document) => document,
            Err(error) => return GitTokenSavedCookieAction::Failed(error.to_string()),
        };
        match validate_saved_account_cookie(
            &document,
            &plan.alias,
            plan.email.as_deref(),
            self.now_unix,
            runtime,
            self.session_identity_validator.as_ref(),
            self.secret_backend.as_ref(),
        ) {
            SavedCookieValidation::Valid { .. } => {}
            SavedCookieValidation::Recover(message) => {
                return GitTokenSavedCookieAction::Recover(message);
            }
            SavedCookieValidation::Failed(message) => {
                return GitTokenSavedCookieAction::Failed(message);
            }
        }

        self.log_stage(
            shared_state,
            TaskLogLevel::Info,
            "阶段：提交 Git token 操作",
        );
        let result = match self.commit_lock.lock() {
            Ok(_guard) if generate_token => {
                match runtime.block_on(
                    generate_account_git_token_with_browser_in_store_with_backend(
                        &self.store,
                        std::slice::from_ref(&plan.alias),
                        session.automation(),
                        self.now_unix,
                        self.secret_backend.as_ref(),
                    ),
                ) {
                    Ok(report) => match report.items.into_iter().next() {
                        Some(item) if item.skipped => {
                            return GitTokenSavedCookieAction::Skipped;
                        }
                        Some(item) if item.refreshed => {
                            item.report.ok_or_else(|| AccountGitTokenError::Browser {
                                message: "Git token generation returned no report".to_string(),
                            })
                        }
                        Some(item) => {
                            Err(item.error.unwrap_or_else(|| AccountGitTokenError::Browser {
                                message: "Git token generation did not complete".to_string(),
                            }))
                        }
                        None => Err(AccountGitTokenError::Browser {
                            message: "Git token generation returned no item".to_string(),
                        }),
                    },
                    Err(error) => Err(error),
                }
            }
            Ok(_guard) => runtime.block_on(
                refresh_account_git_token_metadata_with_browser_in_store_with_backend(
                    &self.store,
                    &plan.alias,
                    session.automation(),
                    self.secret_backend.as_ref(),
                ),
            ),
            Err(_) => {
                return GitTokenSavedCookieAction::Failed(
                    "account commit lock poisoned".to_string(),
                );
            }
        };
        match result {
            Ok(report) => GitTokenSavedCookieAction::Completed(report),
            Err(AccountGitTokenError::MissingCookies { .. }) => GitTokenSavedCookieAction::Recover(
                "Cookie 在 Git token 操作时失效，转账号密码登录".to_string(),
            ),
            Err(error) => {
                GitTokenSavedCookieAction::Failed(account_git_token_error_message(&error))
            }
        }
    }

    fn wait_for_input(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        runtime: &tokio::runtime::Runtime,
        session: &LocalChromeCdpSession,
        kind: TaskUserInputKind,
        message: &str,
    ) -> CredentialBrowserResumeAction {
        self.log_stage(
            shared_state,
            TaskLogLevel::Warning,
            format!("阶段：等待用户输入（{}）", task_user_input_kind_name(kind)),
        );
        let snapshot =
            match self
                .coordinator
                .mark_waiting(&self.task_id, &self.item_id, kind, message)
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return CredentialBrowserResumeAction::Failed(format!(
                        "browser batch wait transition failed: {error:?}"
                    ));
                }
            };
        let all_remaining_waiting = snapshot.running_count() == 0 && snapshot.queued_count() == 0;
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.wait_for_item_input(
                &self.task_id,
                self.item_id.clone(),
                self.plan.alias().to_string(),
                kind,
                message,
                all_remaining_waiting,
            );
        }

        let started = Instant::now();
        loop {
            if browser_task_cancel_requested(shared_state, &self.task_id) {
                return CredentialBrowserResumeAction::Cancelled("用户取消".to_string());
            }
            if !session.is_active() {
                return CredentialBrowserResumeAction::Cancelled(
                    "用户已关闭对应浏览器窗口".to_string(),
                );
            }
            if started.elapsed() >= ACCOUNT_BROWSER_SESSION_TIMEOUT {
                return CredentialBrowserResumeAction::Failed(
                    "浏览器会话等待超时，已自动清理".to_string(),
                );
            }

            if kind == TaskUserInputKind::NewBrowserCredentials {
                let input = shared_state.lock().ok().and_then(|mut state| {
                    state.tasks.take_item_user_input(
                        &self.task_id,
                        &self.item_id,
                        TaskUserInputKind::NewBrowserCredentials,
                    )
                });
                if let Some(input) = input {
                    match serde_json::from_str::<BrowserCredentialsTaskInput>(&input.value) {
                        Ok(input) if !input.password.trim().is_empty() => {
                            self.log_stage(
                                shared_state,
                                TaskLogLevel::Info,
                                "阶段：收到补充账号密码",
                            );
                            return CredentialBrowserResumeAction::Credentials(
                                BrowserCredentialsTaskInput {
                                    email: input.email,
                                    password: input.password.trim().to_string(),
                                },
                            );
                        }
                        _ => {
                            if let Ok(mut state) = shared_state.lock() {
                                let _ = state.tasks.wait_for_item_input(
                                    &self.task_id,
                                    self.item_id.clone(),
                                    self.plan.alias().to_string(),
                                    kind,
                                    self.operation.invalid_input_message(),
                                    all_remaining_waiting,
                                );
                            }
                            self.log_stage(
                                shared_state,
                                TaskLogLevel::Warning,
                                "阶段：补充凭据格式无效，继续等待",
                            );
                        }
                    }
                }
            } else {
                let captcha_input_received = shared_state
                    .lock()
                    .ok()
                    .and_then(|mut state| {
                        state.tasks.take_item_user_input(
                            &self.task_id,
                            &self.item_id,
                            TaskUserInputKind::CaptchaCompleted,
                        )
                    })
                    .is_some();
                match runtime.block_on(session.automation().current_login_state()) {
                    Ok(CdpLoginState::Success) => {
                        return match runtime.block_on(
                            session
                                .automation()
                                .finalize_login_result(self.plan.email_owned()),
                        ) {
                            Ok(login) => CredentialBrowserResumeAction::Login(login),
                            Err(error) => CredentialBrowserResumeAction::Failed(error.to_string()),
                        };
                    }
                    Ok(CdpLoginState::InvalidCredentials) => {
                        return CredentialBrowserResumeAction::PromptCredentials;
                    }
                    Ok(CdpLoginState::RobotVerificationBlocked)
                    | Ok(CdpLoginState::RegisteredEmail) => {
                        return CredentialBrowserResumeAction::RetryAutomation;
                    }
                    Ok(CdpLoginState::ChallengeRequired)
                    | Ok(CdpLoginState::ChallengePossible)
                    | Ok(CdpLoginState::Pending)
                    | Err(_) => {
                        if captcha_input_received {
                            if let Ok(mut state) = shared_state.lock() {
                                let _ = state.tasks.wait_for_item_input(
                                    &self.task_id,
                                    self.item_id.clone(),
                                    self.plan.alias().to_string(),
                                    TaskUserInputKind::CaptchaCompleted,
                                    "验证码尚未完成，请在对应浏览器窗口继续验证",
                                    all_remaining_waiting,
                                );
                            }
                            self.log_stage(
                                shared_state,
                                TaskLogLevel::Warning,
                                "阶段：验证码尚未完成，继续等待",
                            );
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn finish_completed(
        &self,
        shared_state: &Arc<StdMutex<ApiState>>,
        item_report: CredentialBrowserCommitReport,
    ) {
        let (item, completed, failure_message) = match item_report {
            CredentialBrowserCommitReport::Credential(item_report) => {
                let (item, completed, failure_message) =
                    credential_browser_batch_item_from_report(item_report);
                (
                    CredentialBrowserBatchItem::Credential(item),
                    completed,
                    failure_message,
                )
            }
            CredentialBrowserCommitReport::PasswordChange(item_report) => {
                let completed = item_report.password_updated;
                let failure_message = item_report.failure_message.clone();
                (
                    CredentialBrowserBatchItem::PasswordChange(OverleafPasswordChangeBatchItem {
                        alias: item_report.alias.clone(),
                        email: Some(item_report.email.clone()),
                        password_updated: item_report.password_updated,
                        report: Some(item_report),
                        error: None,
                    }),
                    completed,
                    failure_message,
                )
            }
            CredentialBrowserCommitReport::GitToken(item_report) => (
                CredentialBrowserBatchItem::GitToken(AccountGitTokenRefreshBatchItem {
                    alias: item_report.alias.clone(),
                    refreshed: true,
                    skipped: false,
                    report: Some(item_report),
                    error: None,
                }),
                true,
                None,
            ),
            CredentialBrowserCommitReport::GitTokenSkipped => (
                CredentialBrowserBatchItem::GitToken(AccountGitTokenRefreshBatchItem {
                    alias: self.plan.alias().to_string(),
                    refreshed: false,
                    skipped: true,
                    report: None,
                    error: None,
                }),
                true,
                None,
            ),
            CredentialBrowserCommitReport::BrowserLogin(item_report) => {
                let completed = item_report.opened;
                let failure_message =
                    (!completed).then(|| "浏览器登录结果未确认窗口已打开".to_string());
                (
                    CredentialBrowserBatchItem::BrowserLogin(BrowserAutoLoginBatchItem {
                        alias: item_report.alias.clone(),
                        opened: item_report.opened,
                        cancelled: false,
                        report: Some(item_report),
                        error: failure_message
                            .clone()
                            .map(|message| BrowserAutoLoginError::Browser { message }),
                    }),
                    completed,
                    failure_message,
                )
            }
        };
        self.store_terminal_item(item);
        if completed {
            self.log_stage(shared_state, TaskLogLevel::Info, "终态：完成");
            let _ = self.coordinator.mark_completed(
                &self.task_id,
                &self.item_id,
                self.operation.terminal_item_message(),
            );
            if let Ok(mut state) = shared_state.lock() {
                let _ = state.tasks.append_log(
                    &self.task_id,
                    TaskLogLevel::Info,
                    self.operation.success_message(self.plan.alias()),
                );
            }
        } else {
            let message = failure_message
                .unwrap_or_else(|| "browser reported password was not changed".to_string());
            self.log_stage(shared_state, TaskLogLevel::Warning, "终态：失败");
            let _ = self
                .coordinator
                .mark_failed(&self.task_id, &self.item_id, message.clone());
            if let Ok(mut state) = shared_state.lock() {
                let _ = state.tasks.append_log(
                    &self.task_id,
                    TaskLogLevel::Warning,
                    self.operation.failure_message(self.plan.alias(), &message),
                );
            }
        }
        update_credential_browser_batch_progress(shared_state, &self.coordinator, &self.task_id);
        self.finish_batch_if_terminal(shared_state);
    }

    fn finish_skipped(&self, shared_state: &Arc<StdMutex<ApiState>>) {
        self.log_stage(
            shared_state,
            TaskLogLevel::Info,
            "终态：跳过（已有有效 Git token）",
        );
        self.store_terminal_item(CredentialBrowserBatchItem::GitToken(
            AccountGitTokenRefreshBatchItem {
                alias: self.plan.alias().to_string(),
                refreshed: false,
                skipped: true,
                report: None,
                error: None,
            },
        ));
        let _ = self.coordinator.mark_completed(
            &self.task_id,
            &self.item_id,
            "已有有效 Git token，跳过浏览器",
        );
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &self.task_id,
                TaskLogLevel::Info,
                format!("{}：已有有效 Git token，跳过浏览器", self.plan.alias()),
            );
        }
        update_credential_browser_batch_progress(shared_state, &self.coordinator, &self.task_id);
        self.finish_batch_if_terminal(shared_state);
    }

    fn finish_failed(&self, shared_state: &Arc<StdMutex<ApiState>>, message: impl Into<String>) {
        let message = message.into();
        self.log_stage(shared_state, TaskLogLevel::Warning, "终态：失败");
        self.store_terminal_item(self.failure_item(message.clone()));
        let _ = self
            .coordinator
            .mark_failed(&self.task_id, &self.item_id, message.clone());
        update_credential_browser_batch_progress(shared_state, &self.coordinator, &self.task_id);
        if let Ok(mut state) = shared_state.lock() {
            let _ = state.tasks.append_log(
                &self.task_id,
                TaskLogLevel::Warning,
                self.operation.failure_message(self.plan.alias(), &message),
            );
        }
        self.finish_batch_if_terminal(shared_state);
    }

    fn finish_cancelled(&self, shared_state: &Arc<StdMutex<ApiState>>, message: impl Into<String>) {
        let message = message.into();
        self.log_stage(shared_state, TaskLogLevel::Warning, "终态：取消");
        self.store_terminal_item(self.cancelled_item(message.clone()));
        let _ = self
            .coordinator
            .request_item_cancel(&self.task_id, &self.item_id);
        let _ = self
            .coordinator
            .mark_cancelled(&self.task_id, &self.item_id, message.clone());
        update_credential_browser_batch_progress(shared_state, &self.coordinator, &self.task_id);
        self.finish_batch_if_terminal(shared_state);
    }

    fn cancelled_item(&self, message: String) -> CredentialBrowserBatchItem {
        match self.operation {
            CredentialBrowserOperation::BrowserLogin => {
                CredentialBrowserBatchItem::BrowserLogin(BrowserAutoLoginBatchItem {
                    alias: self.plan.alias().to_string(),
                    opened: false,
                    cancelled: true,
                    report: None,
                    error: None,
                })
            }
            CredentialBrowserOperation::Add
            | CredentialBrowserOperation::Refresh
            | CredentialBrowserOperation::PasswordChange
            | CredentialBrowserOperation::GitTokenRefresh
            | CredentialBrowserOperation::GitToken => self.failure_item(message),
        }
    }

    fn failure_item(&self, message: String) -> CredentialBrowserBatchItem {
        match self.operation {
            CredentialBrowserOperation::Add | CredentialBrowserOperation::Refresh => {
                CredentialBrowserBatchItem::Credential(CredentialBatchItem {
                    alias: self.plan.alias().to_string(),
                    email: self.plan.email().unwrap_or_default().to_string(),
                    status: CredentialBatchItemStatus::Failed,
                    existing_alias: None,
                    report: None,
                    error: Some(message),
                })
            }
            CredentialBrowserOperation::PasswordChange => {
                CredentialBrowserBatchItem::PasswordChange(OverleafPasswordChangeBatchItem {
                    alias: self.plan.alias().to_string(),
                    email: self.plan.email().map(ToOwned::to_owned),
                    password_updated: false,
                    report: None,
                    error: Some(message),
                })
            }
            CredentialBrowserOperation::GitTokenRefresh | CredentialBrowserOperation::GitToken => {
                CredentialBrowserBatchItem::GitToken(AccountGitTokenRefreshBatchItem {
                    alias: self.plan.alias().to_string(),
                    refreshed: false,
                    skipped: false,
                    report: None,
                    error: Some(AccountGitTokenError::Browser { message }),
                })
            }
            CredentialBrowserOperation::BrowserLogin => {
                CredentialBrowserBatchItem::BrowserLogin(BrowserAutoLoginBatchItem {
                    alias: self.plan.alias().to_string(),
                    opened: false,
                    cancelled: false,
                    report: None,
                    error: Some(BrowserAutoLoginError::Browser { message }),
                })
            }
        }
    }

    fn store_terminal_item(&self, item: CredentialBrowserBatchItem) {
        if let Ok(mut accumulator) = self.accumulator.lock() {
            // 取消和线程异常并发时，保留首次终态。
            accumulator
                .items
                .entry(self.item_id.clone())
                .or_insert(item);
        }
    }

    fn finish_batch_if_terminal(&self, shared_state: &Arc<StdMutex<ApiState>>) {
        finish_credential_browser_batch_if_terminal(
            shared_state,
            &self.coordinator,
            &self.accumulator,
            &self.task_id,
            self.operation,
        );
    }
}

fn update_credential_browser_batch_progress(
    shared_state: &Arc<StdMutex<ApiState>>,
    coordinator: &BrowserBatchCoordinator,
    task_id: &str,
) {
    let Ok(snapshot) = coordinator.snapshot(task_id) else {
        return;
    };
    let terminal_count = snapshot
        .items
        .iter()
        .filter(|item| item.phase.is_terminal())
        .count();
    if let Ok(mut state) = shared_state.lock() {
        let _ = state.tasks.set_progress_value(
            task_id,
            terminal_count.min(u32::MAX as usize) as u32,
            task_progress_total(snapshot.items.len()),
        );
    }
}

fn missing_credential_browser_batch_item(
    operation: CredentialBrowserOperation,
    alias: &str,
    cancelled: bool,
) -> CredentialBrowserBatchItem {
    let message = if cancelled {
        "任务已取消，账号没有返回浏览器结果"
    } else {
        "账号没有返回浏览器结果，已标记为失败"
    };
    match operation {
        CredentialBrowserOperation::Add | CredentialBrowserOperation::Refresh => {
            CredentialBrowserBatchItem::Credential(CredentialBatchItem {
                alias: alias.to_string(),
                email: String::new(),
                status: CredentialBatchItemStatus::Failed,
                existing_alias: None,
                report: None,
                error: Some(message.to_string()),
            })
        }
        CredentialBrowserOperation::PasswordChange => {
            CredentialBrowserBatchItem::PasswordChange(OverleafPasswordChangeBatchItem {
                alias: alias.to_string(),
                email: None,
                password_updated: false,
                report: None,
                error: Some(message.to_string()),
            })
        }
        CredentialBrowserOperation::GitTokenRefresh | CredentialBrowserOperation::GitToken => {
            CredentialBrowserBatchItem::GitToken(AccountGitTokenRefreshBatchItem {
                alias: alias.to_string(),
                refreshed: false,
                skipped: false,
                report: None,
                error: Some(AccountGitTokenError::Browser {
                    message: message.to_string(),
                }),
            })
        }
        CredentialBrowserOperation::BrowserLogin => {
            CredentialBrowserBatchItem::BrowserLogin(BrowserAutoLoginBatchItem {
                alias: alias.to_string(),
                opened: false,
                cancelled,
                report: None,
                error: (!cancelled).then(|| BrowserAutoLoginError::Browser {
                    message: message.to_string(),
                }),
            })
        }
    }
}

fn assemble_credential_browser_batch_items(
    operation: CredentialBrowserOperation,
    initial_items: &[CredentialBrowserBatchItem],
    ordered_item_ids: &[String],
    accumulated: &mut BTreeMap<String, CredentialBrowserBatchItem>,
    registered_items: &[crate::BrowserBatchItemSnapshot],
) -> (Vec<CredentialBrowserBatchItem>, Vec<String>) {
    let missing_items = registered_items
        .iter()
        .filter(|item| !accumulated.contains_key(&item.item_id))
        .cloned()
        .collect::<Vec<_>>();
    let missing_aliases = missing_items
        .iter()
        .map(|item| item.alias.clone())
        .collect::<Vec<_>>();
    for item in missing_items {
        accumulated.insert(
            item.item_id.clone(),
            missing_credential_browser_batch_item(
                operation,
                &item.alias,
                item.phase == crate::BrowserBatchItemPhase::Cancelled,
            ),
        );
    }

    let mut items = initial_items.to_vec();
    items.extend(
        ordered_item_ids
            .iter()
            .filter_map(|item_id| accumulated.get(item_id).cloned()),
    );
    (items, missing_aliases)
}

pub(super) fn browser_task_cancel_requested(
    shared_state: &Arc<StdMutex<ApiState>>,
    task_id: &str,
) -> bool {
    shared_state
        .lock()
        .ok()
        .and_then(|state| state.tasks.snapshot(task_id).ok())
        .map(|snapshot| {
            snapshot.cancel_requested
                || matches!(
                    snapshot.phase,
                    crate::ServiceTaskPhase::Completed
                        | crate::ServiceTaskPhase::Failed
                        | crate::ServiceTaskPhase::Cancelled
                )
        })
        .unwrap_or(true)
}

fn finish_credential_browser_batch_if_terminal(
    shared_state: &Arc<StdMutex<ApiState>>,
    coordinator: &BrowserBatchCoordinator,
    accumulator: &Arc<StdMutex<CredentialBrowserBatchAccumulator>>,
    task_id: &str,
    operation: CredentialBrowserOperation,
) {
    let Ok(snapshot) = coordinator.snapshot(task_id) else {
        return;
    };
    if !snapshot.is_terminal() {
        return;
    }
    let all_items_cancelled = snapshot
        .items
        .iter()
        .all(|item| item.phase == crate::BrowserBatchItemPhase::Cancelled);

    let (items, completion, missing_aliases) = {
        let Ok(mut accumulator) = accumulator.lock() else {
            return;
        };
        if accumulator.finalized {
            return;
        }
        accumulator.finalized = true;
        let initial_items = accumulator.initial_items.clone();
        let ordered_item_ids = accumulator.ordered_item_ids.clone();
        let (items, missing_aliases) = assemble_credential_browser_batch_items(
            operation,
            &initial_items,
            &ordered_item_ids,
            &mut accumulator.items,
            &snapshot.items,
        );
        let completion = std::mem::replace(
            &mut accumulator.completion,
            CredentialBrowserBatchCompletion::Standard,
        );
        (items, completion, missing_aliases)
    };
    let _ = coordinator.remove_terminal_batch(task_id);

    if let Ok(mut state) = shared_state.lock() {
        let snapshot = state.tasks.snapshot(task_id).ok();
        if snapshot.as_ref().is_some_and(|snapshot| {
            matches!(
                snapshot.phase,
                crate::ServiceTaskPhase::Completed
                    | crate::ServiceTaskPhase::Failed
                    | crate::ServiceTaskPhase::Cancelled
            )
        }) {
            return;
        }
        if !missing_aliases.is_empty() {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!(
                    "浏览器批次补全 {} 个无结果账号，已按失败处理",
                    missing_aliases.len()
                ),
            );
        }
        if all_items_cancelled
            || snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.cancel_requested)
        {
            let _ = cancel_task_with_registration_release(
                &mut state,
                task_id,
                "任务已取消，浏览器会话已清理",
            );
        } else {
            match completion {
                CredentialBrowserBatchCompletion::ImportPostActions(context) => {
                    finish_import_post_action_browser_batch(
                        &mut state, task_id, *context, operation, items,
                    );
                    return;
                }
                CredentialBrowserBatchCompletion::CredentialRefresh(continuation) => {
                    let report = credential_batch_report_from_browser_items(items);
                    let _ = finish_credential_refresh_continuation(
                        &mut state,
                        task_id,
                        report,
                        continuation,
                    );
                    let follow_up_jobs = state.take_background_jobs();
                    drop(state);
                    spawn_follow_up_jobs(shared_state, task_id, follow_up_jobs);
                    return;
                }
                CredentialBrowserBatchCompletion::Standard => {}
            }
            match operation {
                CredentialBrowserOperation::Add | CredentialBrowserOperation::Refresh => {
                    let report = credential_batch_report_from_browser_items(items);
                    finish_credential_batch_task(
                        &mut state,
                        Some(task_id),
                        operation.task_label(),
                        &report,
                    );
                }
                CredentialBrowserOperation::PasswordChange => {
                    let items = items
                        .into_iter()
                        .filter_map(|item| match item {
                            CredentialBrowserBatchItem::PasswordChange(item) => Some(item),
                            CredentialBrowserBatchItem::Credential(_)
                            | CredentialBrowserBatchItem::GitToken(_)
                            | CredentialBrowserBatchItem::BrowserLogin(_) => None,
                        })
                        .collect::<Vec<_>>();
                    let report = OverleafPasswordChangeBatchReport {
                        changed_count: items.iter().filter(|item| item.password_updated).count(),
                        failed_count: items.iter().filter(|item| !item.password_updated).count(),
                        items,
                    };
                    finish_overleaf_password_change_batch_task(&mut state, Some(task_id), &report);
                }
                CredentialBrowserOperation::GitTokenRefresh
                | CredentialBrowserOperation::GitToken => {
                    let items = items
                        .into_iter()
                        .filter_map(|item| match item {
                            CredentialBrowserBatchItem::GitToken(item) => Some(item),
                            CredentialBrowserBatchItem::Credential(_)
                            | CredentialBrowserBatchItem::PasswordChange(_)
                            | CredentialBrowserBatchItem::BrowserLogin(_) => None,
                        })
                        .collect::<Vec<_>>();
                    let report = AccountGitTokenRefreshBatchReport {
                        refreshed_count: items.iter().filter(|item| item.refreshed).count(),
                        skipped_count: items.iter().filter(|item| item.skipped).count(),
                        failed_count: items
                            .iter()
                            .filter(|item| !item.refreshed && !item.skipped)
                            .count(),
                        items,
                    };
                    finish_git_token_batch_task(&mut state, Some(task_id), &report, operation);
                }
                CredentialBrowserOperation::BrowserLogin => {
                    let items = items
                        .into_iter()
                        .filter_map(|item| match item {
                            CredentialBrowserBatchItem::BrowserLogin(item) => Some(item),
                            CredentialBrowserBatchItem::Credential(_)
                            | CredentialBrowserBatchItem::PasswordChange(_)
                            | CredentialBrowserBatchItem::GitToken(_) => None,
                        })
                        .collect::<Vec<_>>();
                    let report = BrowserAutoLoginBatchReport {
                        opened_count: items.iter().filter(|item| item.opened).count(),
                        failed_count: items
                            .iter()
                            .filter(|item| !item.opened && !item.cancelled)
                            .count(),
                        cancelled_count: items.iter().filter(|item| item.cancelled).count(),
                        items,
                    };
                    if report.failed_count == 0 && report.cancelled_count == 0 {
                        complete_tracked_task(
                            &mut state,
                            Some(task_id),
                            "浏览器自动登录窗口已打开",
                            &report,
                        );
                    } else {
                        let message = format!(
                            "浏览器自动登录未完全完成：{} 个失败，{} 个取消",
                            report.failed_count, report.cancelled_count
                        );
                        let total = task_completion_progress_total(&state, task_id);
                        let _ = state.tasks.set_progress(task_id, total, total, &message);
                        let result = serde_json::to_value(&report).ok();
                        let _ = state.tasks.fail_task_with_result(task_id, message, result);
                    }
                }
            }
        }
    }
}
