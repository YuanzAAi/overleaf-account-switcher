use super::*;

pub(super) fn refresh_account_subscriptions_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request = match parse_alias_batch_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(task_id) = request.task_id else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing task_id".into(),
            },
        );
    };
    if let Err(response) = start_task_if_requested(state, Some(&task_id), "刷新账号订阅状态")
    {
        return response;
    }
    let store = AccountStore::new(state.config.accounts_path());
    let backend = Arc::clone(&state.secret_backend);
    let refresher = Arc::clone(&state.metadata_refresher);
    let commit_lock = state.account_commit_lock();
    let job_id = task_id.clone();
    let job = ApiBackgroundJob::new("account-subscriptions", move |shared_state| {
        let mut items = Vec::new();
        let total = request.aliases.len() as u32;
        for alias in request.aliases {
            if browser_task_cancel_requested(&shared_state, &job_id) {
                if let Ok(mut state) = shared_state.lock() {
                    let _ = state.tasks.cancel_task(&job_id, "订阅刷新已取消");
                }
                return;
            }
            let result = block_on_api(
                crate::account_session::refresh_saved_account_session_with_commit_lock(
                    &store,
                    &alias,
                    now_unix,
                    backend.as_ref(),
                    refresher.as_ref(),
                    Some(&commit_lock),
                ),
            );
            let item = AccountSessionRefreshBatchItem::from_result(
                alias.clone(),
                result.unwrap_or_else(|response| {
                    Err(AccountSessionError::Io {
                        message: response.body,
                    })
                }),
            );
            if let Ok(mut state) = shared_state.lock() {
                let detail = item
                    .error
                    .as_ref()
                    .map(account_session_error_message)
                    .unwrap_or_else(|| "订阅状态已更新".into());
                let _ = state.tasks.append_log(
                    &job_id,
                    if item.refreshed {
                        TaskLogLevel::Info
                    } else {
                        TaskLogLevel::Warning
                    },
                    format!("{alias}：{detail}"),
                );
                let _ = state
                    .tasks
                    .set_progress_value(&job_id, items.len() as u32 + 1, total);
            }
            items.push(item);
        }
        let cancelled = browser_task_cancel_requested(&shared_state, &job_id);
        let report = AccountSessionRefreshBatchReport::from_items(items);
        if let Ok(mut state) = shared_state.lock() {
            if cancelled {
                let _ = state.tasks.cancel_task(&job_id, "订阅刷新已取消");
            } else if report.failed_count == 0 {
                complete_tracked_task(&mut state, Some(&job_id), "账号订阅状态已刷新", &report);
            } else {
                fail_tracked_task_with_result(
                    &mut state,
                    Some(&job_id),
                    "部分账号订阅状态未更新，请检查 Cookie 或稍后重试",
                    &report,
                );
            }
        }
    });
    if let Err(response) = state.enqueue_background_job(job) {
        fail_tracked_task(state, Some(&task_id), response.body.clone());
        return response;
    }
    registration_waiting_response(state, &task_id)
}

pub(super) fn import_accounts_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request: AccountImportRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    let post_actions = AccountImportPostActionOptions {
        refresh_session_metadata: request.refresh_session_metadata,
        fetch_git_token: request.fetch_git_token,
    };
    let path = request
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let (None, Some(input)) = (path, request.json.as_deref()) {
        let store = AccountStore::new(state.config.accounts_path());
        return import_accounts_from_text_response(
            state,
            &store,
            input,
            task_id.as_deref(),
            post_actions,
            now_unix,
        );
    }
    if path.is_some() && request.json.is_some() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "请只选择文件路径或 JSON 内容中的一种".to_string(),
            },
        );
    }
    let Some(path) = path else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing account import file path".to_string(),
            },
        );
    };

    let store = AccountStore::new(state.config.accounts_path());
    match fs::read_to_string(path) {
        Ok(input) => import_accounts_from_text_response(
            state,
            &store,
            &input,
            task_id.as_deref(),
            post_actions,
            now_unix,
        ),
        Err(error) => {
            fail_tracked_task(state, task_id.as_deref(), error.to_string());
            io_error_response(error)
        }
    }
}

pub(super) fn import_manual_cookie_accounts_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request: AccountManualCookieImportRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    if request.entries.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "missing manual cookie import entries".to_string(),
            },
        );
    }

    let candidates = match manual_cookie_import_candidates_from_entries(request.entries) {
        Ok(candidates) => candidates,
        Err(response) => return response,
    };

    let store = AccountStore::new(state.config.accounts_path());
    let task_id = normalized_optional_text(request.task_id);
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    let aliases = import_candidate_lock_aliases(&document, &candidates);
    if let Err(response) =
        start_alias_batch_task(state, task_id.as_deref(), "手动导入 Cookie 账号", &aliases)
    {
        return response;
    }
    if let Some(task_id) = task_id.as_deref() {
        let _ = state
            .tasks
            .append_log(task_id, TaskLogLevel::Info, "正在验证 Cookie 登录身份");
    }
    let validation = match block_on_api(validate_manual_cookie_import_candidates(
        &candidates,
        state.session_identity_validator.as_ref(),
    )) {
        Ok(report) => report,
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            return response;
        }
    };
    if let Some(task_id) = task_id.as_deref() {
        for item in validation.items.iter().filter(|item| !item.valid) {
            let message = item
                .error
                .as_ref()
                .map(account_session_error_message)
                .unwrap_or_else(|| "Cookie 登录身份验证失败".to_string());
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!("Cookie 身份验证失败: {} ({message})", item.alias),
            );
        }
    }
    let valid_indices = validation
        .items
        .iter()
        .filter(|item| item.valid)
        .map(|item| item.index)
        .collect::<BTreeSet<_>>();
    let candidates = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| valid_indices.contains(&index).then_some(candidate))
        .collect::<Vec<_>>();
    if let Some(task_id) = task_id.as_deref() {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Info,
            format!(
                "Cookie 身份验证完成：{} 个通过，{} 个失败",
                validation.valid_count, validation.failed_count
            ),
        );
    }
    match apply_account_candidates_report(state, &store, document, candidates, task_id.as_deref()) {
        Ok(import) => finish_account_import_seed_response(
            state,
            &store,
            AccountImportPostActionSeed::ManualCookie { import, validation },
            task_id.as_deref(),
            AccountImportPostActionOptions {
                refresh_session_metadata: request.refresh_session_metadata,
                fetch_git_token: request.fetch_git_token,
            },
            now_unix,
        ),
        Err(response) => response,
    }
}

pub(super) fn export_accounts_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: AccountExportRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    match load_accounts(state) {
        Ok(document) => {
            let aliases = split_comma_values(&request.aliases);
            if let Err(response) =
                start_alias_batch_task(state, task_id.as_deref(), "导出账号", &aliases)
            {
                return response;
            }
            append_optional_task_log(
                state,
                task_id.as_deref(),
                TaskLogLevel::Info,
                format!("正在读取 {} 个选中账号并准备导出文件", aliases.len()),
            );
            match export_accounts_to_directory_with_backend(
                &document,
                aliases,
                request.mode,
                request.output_dir.trim(),
                state.config.accounts_path(),
                request.confirm_overwrite,
                state.secret_backend.as_ref(),
            ) {
                Ok(report) => {
                    let public_report = redact_export_report_paths(report.clone());
                    append_optional_task_log(
                        state,
                        task_id.as_deref(),
                        TaskLogLevel::Info,
                        format!(
                            "导出文件写入完成：{} 个账号，{} 个文件，覆盖 {} 个文件",
                            report.exported_account_count,
                            report.files.len(),
                            report.overwritten_file_count
                        ),
                    );
                    complete_tracked_task(state, task_id.as_deref(), "账号导出完成", &report);
                    json_response(200, &public_report)
                }
                Err(error) => {
                    append_optional_task_log(
                        state,
                        task_id.as_deref(),
                        TaskLogLevel::Warning,
                        format!("账号导出失败：{}", account_io_error_message(&error)),
                    );
                    fail_tracked_task(state, task_id.as_deref(), account_io_error_message(&error));
                    account_io_error_response(error)
                }
            }
        }
        Err(error) => {
            fail_tracked_task(state, task_id.as_deref(), error.to_string());
            io_error_response(error)
        }
    }
}

pub(super) fn export_accounts_json_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: AccountExportJsonRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    match load_accounts(state) {
        Ok(document) => {
            match export_accounts_to_json_with_backend(
                &document,
                split_comma_values(&request.aliases),
                state.secret_backend.as_ref(),
            ) {
                Ok(json) => ApiResponse {
                    status_code: 200,
                    content_type: "application/json; charset=utf-8",
                    body: json,
                },
                Err(error) => account_io_error_response(error),
            }
        }
        Err(error) => io_error_response(error),
    }
}

pub(super) fn update_default_export_dir_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: DefaultExportDirRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let path = request.default_export_dir.trim();
    if path.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "default_export_dir is required".to_string(),
            },
        );
    }

    match state.config.set_default_export_dir(PathBuf::from(path)) {
        Ok(()) => json_response(200, &config_body(state)),
        Err(error) => io_error_response(error),
    }
}

fn import_accounts_from_text_response(
    state: &mut ApiState,
    store: &AccountStore,
    input: &str,
    task_id: Option<&str>,
    post_actions: AccountImportPostActionOptions,
    now_unix: i64,
) -> ApiResponse {
    let candidates = match parse_account_import_json(input) {
        Ok(candidates) => candidates,
        Err(error) => {
            fail_tracked_task(state, task_id, account_io_error_message(&error));
            return account_io_error_response(error);
        }
    };
    import_account_candidates_with_post_actions_response(
        state,
        store,
        candidates,
        task_id,
        "导入账号",
        post_actions,
        now_unix,
    )
}

fn manual_cookie_import_candidates_from_entries(
    entries: Vec<AccountManualCookieImportEntryRequest>,
) -> Result<Vec<AccountImportCandidate>, ApiResponse> {
    if entries.is_empty() {
        return Err(json_response(
            400,
            &ApiErrorBody {
                error: "missing manual cookie import entries".to_string(),
            },
        ));
    }

    let mut candidates = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        let entry_number = index + 1;
        let alias_hint = normalized_optional_text(entry.alias);
        let email = entry.email.trim().to_string();
        let cookie = entry.cookie.trim().to_string();
        if email.is_empty() {
            return Err(json_response(
                400,
                &ApiErrorBody {
                    error: format!("missing email for entry {entry_number}"),
                },
            ));
        }
        if cookie.is_empty() {
            return Err(json_response(
                400,
                &ApiErrorBody {
                    error: format!("missing cookie for entry {entry_number}"),
                },
            ));
        }

        let candidate = manual_cookie_import_candidate(alias_hint, email, &cookie);
        if overleaf_api::overleaf_session_cookie(&candidate.record.cookies).is_none() {
            return Err(json_response(
                400,
                &ApiErrorBody {
                    error: format!("missing overleaf_session2 cookie for entry {entry_number}"),
                },
            ));
        }
        candidates.push(candidate);
    }

    Ok(candidates)
}

fn import_account_candidates_with_post_actions_response(
    state: &mut ApiState,
    store: &AccountStore,
    candidates: Vec<AccountImportCandidate>,
    task_id: Option<&str>,
    task_name: &str,
    post_actions: AccountImportPostActionOptions,
    now_unix: i64,
) -> ApiResponse {
    match import_account_candidates_report(state, store, candidates, task_id, task_name) {
        Ok(report) => {
            finish_account_import_response(state, store, report, task_id, post_actions, now_unix)
        }
        Err(response) => response,
    }
}

fn finish_account_import_response(
    state: &mut ApiState,
    store: &AccountStore,
    report: AccountImportReport,
    task_id: Option<&str>,
    post_actions: AccountImportPostActionOptions,
    now_unix: i64,
) -> ApiResponse {
    finish_account_import_seed_response(
        state,
        store,
        AccountImportPostActionSeed::Accounts(report),
        task_id,
        post_actions,
        now_unix,
    )
}

#[derive(Serialize)]
#[serde(untagged)]
enum CompletedAccountImportPostActionReport {
    Accounts(AccountImportWithPostActionsReport),
    ManualCookie(ManualCookieImportBatchReport),
}

impl CompletedAccountImportPostActionReport {
    fn has_failures(&self) -> bool {
        match self {
            Self::Accounts(report) => !report.post_action_errors.is_empty(),
            Self::ManualCookie(report) => {
                report.validation.failed_count > 0 || !report.post_action_errors.is_empty()
            }
        }
    }

    fn terminal_message(&self) -> String {
        match self {
            Self::Accounts(report) if report.post_action_errors.is_empty() => {
                "账号导入完成".to_string()
            }
            Self::Accounts(_) => "账号导入完成，部分后置动作失败".to_string(),
            Self::ManualCookie(report)
                if report.validation.failed_count == 0 && report.post_action_errors.is_empty() =>
            {
                "Cookie 账号导入完成".to_string()
            }
            Self::ManualCookie(report) => format!(
                "Cookie 账号导入未完全完成：{} 个身份验证失败，{} 个后置动作失败",
                report.validation.failed_count,
                report.post_action_errors.len()
            ),
        }
    }
}

fn build_completed_account_import_post_action_report(
    seed: AccountImportPostActionSeed,
    session_refresh: Option<AccountSessionRefreshBatchReport>,
    git_token_generation: Option<AccountGitTokenRefreshBatchReport>,
    post_action_errors: Vec<AccountImportPostActionError>,
) -> CompletedAccountImportPostActionReport {
    match seed {
        AccountImportPostActionSeed::Accounts(import) => {
            CompletedAccountImportPostActionReport::Accounts(AccountImportWithPostActionsReport {
                import,
                session_refresh,
                git_token_generation,
                post_action_errors,
            })
        }
        AccountImportPostActionSeed::ManualCookie { import, validation } => {
            CompletedAccountImportPostActionReport::ManualCookie(ManualCookieImportBatchReport {
                import,
                validation,
                session_refresh,
                git_token_generation,
                post_action_errors,
            })
        }
    }
}

fn finish_completed_account_import_post_action_task(
    state: &mut ApiState,
    task_id: &str,
    seed: AccountImportPostActionSeed,
    session_refresh: Option<AccountSessionRefreshBatchReport>,
    git_token_generation: Option<AccountGitTokenRefreshBatchReport>,
    post_action_errors: Vec<AccountImportPostActionError>,
) -> CompletedAccountImportPostActionReport {
    let report = build_completed_account_import_post_action_report(
        seed,
        session_refresh,
        git_token_generation,
        post_action_errors,
    );
    let message = report.terminal_message();
    if report.has_failures() {
        fail_tracked_task_with_result(state, Some(task_id), &message, &report);
    } else {
        complete_tracked_task(state, Some(task_id), &message, &report);
    }
    report
}

fn finish_account_import_seed_response(
    state: &mut ApiState,
    store: &AccountStore,
    seed: AccountImportPostActionSeed,
    task_id: Option<&str>,
    post_actions: AccountImportPostActionOptions,
    now_unix: i64,
) -> ApiResponse {
    if !post_actions.any() {
        match seed {
            AccountImportPostActionSeed::Accounts(report) => {
                complete_tracked_task(state, task_id, "账号导入完成", &report);
                json_response(200, &report)
            }
            AccountImportPostActionSeed::ManualCookie { import, validation } => {
                let report = ManualCookieImportBatchReport {
                    import,
                    validation,
                    session_refresh: None,
                    git_token_generation: None,
                    post_action_errors: Vec::new(),
                };
                if report.validation.failed_count == 0 {
                    complete_tracked_task(state, task_id, "Cookie 账号导入完成", &report);
                } else {
                    let message = format!(
                        "Cookie 账号导入未完全完成：{} 个身份验证失败，0 个后置动作失败",
                        report.validation.failed_count
                    );
                    fail_tracked_task_with_result(state, task_id, &message, &report);
                }
                json_response(200, &report)
            }
        }
    } else if let (Some(task_id), Some(local_chrome)) =
        (task_id, state.local_chrome_browser.clone())
    {
        let (context, batch) = match prepare_import_post_action_batch_context(
            state,
            store,
            seed.clone(),
            task_id,
            post_actions,
            now_unix,
        ) {
            Ok(context) => context,
            Err(response) => return response,
        };
        if let Some(batch) = batch {
            return queue_import_post_action_browser_batch_response(
                state,
                task_id,
                context,
                batch,
                local_chrome,
                now_unix,
            );
        }
        // 没有需要执行的浏览器后置批次时，仍返回完整的包装报告并结束原 task。
        let session_refresh = merge_session_refresh_batch_reports(
            context.initial_session_refresh,
            context.session_retry_report,
        );
        let report = finish_completed_account_import_post_action_task(
            state,
            task_id,
            context.seed,
            session_refresh,
            context.git_token_generation,
            context.post_action_errors,
        );
        json_response(200, &report)
    } else {
        // 未提供任务 ID 时保持同步调用的返回语义。
        match seed {
            AccountImportPostActionSeed::Accounts(import) => {
                let report = account_import_post_action_report(
                    state,
                    store,
                    import,
                    task_id,
                    post_actions,
                    now_unix,
                );
                let message = if report.post_action_errors.is_empty() {
                    "账号导入完成"
                } else {
                    "账号导入完成，部分后置动作失败"
                };
                if report.post_action_errors.is_empty() {
                    complete_tracked_task(state, task_id, message, &report);
                } else {
                    fail_tracked_task_with_result(state, task_id, message, &report);
                }
                json_response(200, &report)
            }
            AccountImportPostActionSeed::ManualCookie { import, validation } => {
                let post_report = account_import_post_action_report(
                    state,
                    store,
                    import,
                    task_id,
                    post_actions,
                    now_unix,
                );
                let report = ManualCookieImportBatchReport {
                    import: post_report.import,
                    validation,
                    session_refresh: post_report.session_refresh,
                    git_token_generation: post_report.git_token_generation,
                    post_action_errors: post_report.post_action_errors,
                };
                let message = if report.validation.failed_count == 0
                    && report.post_action_errors.is_empty()
                {
                    "Cookie 账号导入完成".to_string()
                } else {
                    format!(
                        "Cookie 账号导入未完全完成：{} 个身份验证失败，{} 个后置动作失败",
                        report.validation.failed_count,
                        report.post_action_errors.len()
                    )
                };
                if report.validation.failed_count == 0 && report.post_action_errors.is_empty() {
                    complete_tracked_task(state, task_id, &message, &report);
                } else {
                    fail_tracked_task_with_result(state, task_id, &message, &report);
                }
                json_response(200, &report)
            }
        }
    }
}

fn prepare_import_post_action_batch_context(
    state: &mut ApiState,
    store: &AccountStore,
    seed: AccountImportPostActionSeed,
    task_id: &str,
    options: AccountImportPostActionOptions,
    now_unix: i64,
) -> Result<
    (
        AccountImportPostActionBatchContext,
        Option<CredentialBrowserBatchPlan>,
    ),
    ApiResponse,
> {
    let imported_aliases = imported_aliases_from_import_report(seed.import());
    append_optional_task_log(
        state,
        Some(task_id),
        TaskLogLevel::Info,
        format!(
            "账号导入写入完成：{} 个导入，{} 个重复邮箱跳过，{} 个别名冲突",
            seed.import().imported_count,
            seed.import().skipped_duplicate_email_count,
            seed.import().alias_conflict_count
        ),
    );

    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            fail_tracked_task(state, Some(task_id), error.to_string());
            return Err(io_error_response(error));
        }
    };
    let session_aliases = if options.refresh_session_metadata {
        imported_aliases
            .iter()
            .filter(|alias| {
                document
                    .accounts
                    .get(alias.as_str())
                    .is_some_and(|record| imported_account_needs_session_refresh(record, now_unix))
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut retry_session_aliases = BTreeSet::new();
    let mut initial_session_refresh = None;

    if options.refresh_session_metadata && !session_aliases.is_empty() {
        append_optional_task_log(
            state,
            Some(task_id),
            TaskLogLevel::Info,
            format!(
                "开始刷新导入账号状态元数据：{} 个账号",
                session_aliases.len()
            ),
        );
        match block_on_api(refresh_saved_account_sessions_in_store_with_backend(
            store,
            &session_aliases,
            now_unix,
            state.secret_backend.as_ref(),
        )) {
            Ok(Ok(report)) => {
                append_session_refresh_batch_logs(state, Some(task_id), &report, "导入后状态刷新");
                retry_session_aliases.extend(
                    report
                        .items
                        .iter()
                        .filter(|item| !item.refreshed)
                        .map(|item| item.alias.clone()),
                );
                initial_session_refresh = Some(report);
            }
            Ok(Err(error)) => {
                let message = account_session_error_message(&error);
                append_optional_task_log(
                    state,
                    Some(task_id),
                    TaskLogLevel::Warning,
                    format!("导入后状态刷新失败，将转账号密码恢复：{message}"),
                );
                retry_session_aliases.extend(session_aliases);
            }
            Err(response) => {
                let message = response.body.clone();
                append_optional_task_log(
                    state,
                    Some(task_id),
                    TaskLogLevel::Warning,
                    format!("导入后状态刷新失败，将转账号密码恢复：{message}"),
                );
                retry_session_aliases.extend(session_aliases);
            }
        }
    } else if options.refresh_session_metadata {
        append_optional_task_log(
            state,
            Some(task_id),
            TaskLogLevel::Info,
            "导入后状态刷新已跳过：没有需要更新的账号",
        );
    }

    let mut context = AccountImportPostActionBatchContext {
        seed,
        options,
        imported_aliases: imported_aliases.clone(),
        retry_session_aliases: retry_session_aliases.clone(),
        initial_session_refresh,
        session_retry_report: None,
        git_token_generation: None,
        post_action_errors: Vec::new(),
        now_unix,
    };

    if options.fetch_git_token && !imported_aliases.is_empty() {
        append_optional_task_log(
            state,
            Some(task_id),
            TaskLogLevel::Info,
            format!(
                "开始获取导入账号 Git 令牌：{} 个账号",
                imported_aliases.len()
            ),
        );
        let mut plans = load_git_token_browser_plans(state, imported_aliases, now_unix, true)?;
        for plan in &mut plans {
            if retry_session_aliases.contains(&plan.alias) {
                plan.force_credentials_login = true;
                plan.skip_existing_token = false;
            }
        }
        return Ok((
            context,
            Some(CredentialBrowserBatchPlan {
                operation: CredentialBrowserOperation::GitToken,
                plans: plans
                    .into_iter()
                    .map(CredentialBrowserPlan::GitToken)
                    .collect(),
            }),
        ));
    }

    if !retry_session_aliases.is_empty() {
        let aliases = retry_session_aliases.iter().cloned().collect::<Vec<_>>();
        let plans = plan_saved_password_refreshes(
            &document,
            &aliases.join(","),
            state.secret_backend.as_ref(),
        )
        .inspect_err(|response| {
            fail_tracked_task(state, Some(task_id), response.body.clone());
        })?;
        append_optional_task_log(
            state,
            Some(task_id),
            TaskLogLevel::Info,
            format!("{} 个账号需要密码恢复，进入凭据浏览器批次", plans.len()),
        );
        return Ok((
            context,
            Some(CredentialBrowserBatchPlan {
                operation: CredentialBrowserOperation::Refresh,
                plans: plans
                    .into_iter()
                    .map(CredentialBrowserPlan::Refresh)
                    .collect(),
            }),
        ));
    }

    context.options = options;
    Ok((context, None))
}

fn queue_import_post_action_browser_batch_response(
    state: &mut ApiState,
    task_id: &str,
    context: AccountImportPostActionBatchContext,
    batch: CredentialBrowserBatchPlan,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    queue_credential_browser_batch_response(
        state,
        task_id,
        batch,
        Vec::new(),
        CredentialBrowserBatchCompletion::ImportPostActions(Box::new(context)),
        local_chrome,
        now_unix,
    )
}

pub(super) fn credential_batch_report_from_browser_items(
    items: Vec<CredentialBrowserBatchItem>,
) -> CredentialBatchReport {
    let items = items
        .into_iter()
        .filter_map(|item| match item {
            CredentialBrowserBatchItem::Credential(item) => Some(item),
            CredentialBrowserBatchItem::PasswordChange(_)
            | CredentialBrowserBatchItem::GitToken(_)
            | CredentialBrowserBatchItem::BrowserLogin(_) => None,
        })
        .collect::<Vec<_>>();
    CredentialBatchReport {
        imported_count: items
            .iter()
            .filter(|item| item.status == CredentialBatchItemStatus::Imported)
            .count(),
        updated_count: items
            .iter()
            .filter(|item| item.status == CredentialBatchItemStatus::Updated)
            .count(),
        skipped_duplicate_email_count: items
            .iter()
            .filter(|item| item.status == CredentialBatchItemStatus::SkippedDuplicateEmail)
            .count(),
        failed_count: items
            .iter()
            .filter(|item| item.status == CredentialBatchItemStatus::Failed)
            .count(),
        items,
    }
}

fn git_token_batch_report_from_browser_items(
    items: Vec<CredentialBrowserBatchItem>,
) -> AccountGitTokenRefreshBatchReport {
    let items = items
        .into_iter()
        .filter_map(|item| match item {
            CredentialBrowserBatchItem::GitToken(item) => Some(item),
            CredentialBrowserBatchItem::Credential(_)
            | CredentialBrowserBatchItem::PasswordChange(_)
            | CredentialBrowserBatchItem::BrowserLogin(_) => None,
        })
        .collect::<Vec<_>>();
    AccountGitTokenRefreshBatchReport {
        refreshed_count: items.iter().filter(|item| item.refreshed).count(),
        skipped_count: items.iter().filter(|item| item.skipped).count(),
        failed_count: items
            .iter()
            .filter(|item| !item.refreshed && !item.skipped)
            .count(),
        items,
    }
}

fn session_refresh_report_from_credential_batch(
    report: &CredentialBatchReport,
) -> AccountSessionRefreshBatchReport {
    let items = report
        .items
        .iter()
        .map(|item| {
            let metadata = item
                .report
                .as_ref()
                .and_then(|report| report.metadata.clone());
            if let Some(metadata) = metadata.filter(|metadata| {
                metadata.subscription_refresh_status
                    != SubscriptionRefreshStatus::FailedButAccountUpdated
            }) {
                AccountSessionRefreshBatchItem {
                    alias: item.alias.clone(),
                    refreshed: true,
                    report: Some(metadata),
                    error: None,
                }
            } else {
                AccountSessionRefreshBatchItem {
                    alias: item.alias.clone(),
                    refreshed: false,
                    report: None,
                    error: Some(AccountSessionError::Session {
                        message: item
                            .error
                            .clone()
                            .unwrap_or_else(|| "账号密码登录后未返回状态元数据".to_string()),
                    }),
                }
            }
        })
        .collect::<Vec<_>>();
    AccountSessionRefreshBatchReport {
        refreshed_count: items.iter().filter(|item| item.refreshed).count(),
        failed_count: items.iter().filter(|item| !item.refreshed).count(),
        items,
    }
}

fn merge_session_refresh_batch_reports(
    first: Option<AccountSessionRefreshBatchReport>,
    second: Option<AccountSessionRefreshBatchReport>,
) -> Option<AccountSessionRefreshBatchReport> {
    let mut order = Vec::new();
    let mut by_alias = BTreeMap::new();
    for report in [first, second].into_iter().flatten() {
        for item in report.items {
            if !by_alias.contains_key(&item.alias) {
                order.push(item.alias.clone());
            }
            by_alias.insert(item.alias.clone(), item);
        }
    }
    if order.is_empty() {
        return None;
    }
    let items = order
        .into_iter()
        .filter_map(|alias| by_alias.remove(&alias))
        .collect::<Vec<_>>();
    Some(AccountSessionRefreshBatchReport {
        refreshed_count: items.iter().filter(|item| item.refreshed).count(),
        failed_count: items.iter().filter(|item| !item.refreshed).count(),
        items,
    })
}

fn account_record_session_refresh_report(
    alias: &str,
    record: &AccountRecord,
    now_unix: i64,
) -> AccountSessionRefreshReport {
    let status = record
        .subscription_status
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let subscription_refresh_status =
        if record.subscription_checked_at.is_some_and(|checked_at| {
            checked_at >= now_unix as f64 - IMPORT_SESSION_METADATA_FRESH_SECONDS
        }) && matches!(status.as_str(), "trial" | "pro" | "subscription" | "free")
        {
            SubscriptionRefreshStatus::Detected
        } else {
            SubscriptionRefreshStatus::Unknown
        };
    AccountSessionRefreshReport {
        alias: alias.to_string(),
        email: record.email.clone(),
        user_id: record.user_id.clone(),
        subscription_status: record.subscription_status.clone(),
        subscription_label: record.subscription_label.clone(),
        trial_expiry: record.trial_expiry.map(|value| value as i64),
        subscription_refresh_status,
    }
}

fn reconcile_import_session_recovery(
    state: &mut ApiState,
    context: &mut AccountImportPostActionBatchContext,
) {
    let store = AccountStore::new(state.config.accounts_path());
    let document = store.load().ok();
    let retry_aliases = context
        .retry_session_aliases
        .iter()
        .filter(|alias| {
            context
                .imported_aliases
                .iter()
                .any(|candidate| candidate == *alias)
        })
        .cloned()
        .collect::<Vec<_>>();
    if retry_aliases.is_empty() {
        return;
    }

    let mut report =
        context
            .initial_session_refresh
            .take()
            .unwrap_or(AccountSessionRefreshBatchReport {
                refreshed_count: 0,
                failed_count: 0,
                items: Vec::new(),
            });
    let mut recovered = BTreeSet::new();
    for alias in retry_aliases {
        let persisted = document
            .as_ref()
            .and_then(|document| document.accounts.get(alias.as_str()));
        if let Some(record) = persisted
            .filter(|record| !imported_account_needs_session_refresh(record, context.now_unix))
        {
            recovered.insert(alias.clone());
            let recovered_report =
                account_record_session_refresh_report(&alias, record, context.now_unix);
            if let Some(item) = report.items.iter_mut().find(|item| item.alias == alias) {
                item.refreshed = true;
                item.report = Some(recovered_report);
                item.error = None;
            } else {
                report.items.push(AccountSessionRefreshBatchItem {
                    alias,
                    refreshed: true,
                    report: Some(recovered_report),
                    error: None,
                });
            }
            continue;
        }

        let error = if document.is_none() {
            AccountSessionError::Io {
                message: "无法读取导入账号状态".to_string(),
            }
        } else if persisted.is_some() {
            AccountSessionError::Session {
                message: "账号密码恢复后状态元数据仍不完整".to_string(),
            }
        } else {
            AccountSessionError::AccountAliasNotFound {
                alias: alias.clone(),
            }
        };
        if let Some(item) = report.items.iter_mut().find(|item| item.alias == alias) {
            item.refreshed = false;
            item.report = None;
            if item.error.is_none() {
                item.error = Some(error);
            }
        } else {
            report.items.push(AccountSessionRefreshBatchItem {
                alias,
                refreshed: false,
                report: None,
                error: Some(error),
            });
        }
    }

    report.refreshed_count = report.items.iter().filter(|item| item.refreshed).count();
    report.failed_count = report.items.iter().filter(|item| !item.refreshed).count();
    context.initial_session_refresh = Some(report);

    context.post_action_errors.retain(|error| {
        if error.action != AccountImportPostAction::RefreshSessionMetadata {
            return true;
        }
        !recovered
            .iter()
            .any(|alias| error.message.starts_with(&format!("{alias}：")))
    });
}

pub(super) fn finish_import_post_action_browser_batch(
    state: &mut ApiState,
    task_id: &str,
    mut context: AccountImportPostActionBatchContext,
    operation: CredentialBrowserOperation,
    items: Vec<CredentialBrowserBatchItem>,
) {
    match operation {
        CredentialBrowserOperation::Refresh => {
            let credential_report = credential_batch_report_from_browser_items(items);
            let session_report = session_refresh_report_from_credential_batch(&credential_report);
            append_session_refresh_batch_logs(
                state,
                Some(task_id),
                &session_report,
                "导入后密码恢复",
            );
            context.session_retry_report = Some(session_report);
        }
        CredentialBrowserOperation::GitToken => {
            let git_report = git_token_batch_report_from_browser_items(items);
            append_git_token_batch_logs(state, Some(task_id), &git_report, "导入后 Git 令牌");
            reconcile_import_session_recovery(state, &mut context);
            append_failed_git_token_post_action_errors(
                &mut context.post_action_errors,
                &git_report,
            );
            context.git_token_generation = Some(git_report);
        }
        _ => {
            context
                .post_action_errors
                .push(account_import_post_action_error(
                    AccountImportPostAction::RefreshSessionMetadata,
                    "导入后置批次返回了不匹配的操作类型".to_string(),
                ));
        }
    }

    let session_refresh = merge_session_refresh_batch_reports(
        context.initial_session_refresh,
        context.session_retry_report,
    );
    if let Some(report) = session_refresh.as_ref() {
        append_failed_session_refresh_post_action_errors(&mut context.post_action_errors, report);
    }
    let _ = finish_completed_account_import_post_action_task(
        state,
        task_id,
        context.seed,
        session_refresh,
        context.git_token_generation,
        context.post_action_errors,
    );
}

fn import_account_candidates_report(
    state: &mut ApiState,
    store: &AccountStore,
    candidates: Vec<AccountImportCandidate>,
    task_id: Option<&str>,
    task_name: &str,
) -> Result<AccountImportReport, ApiResponse> {
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            fail_tracked_task(state, task_id, error.to_string());
            return Err(io_error_response(error));
        }
    };
    let aliases = import_candidate_lock_aliases(&document, &candidates);
    start_alias_batch_task(state, task_id, task_name, &aliases)?;

    apply_account_candidates_report(state, store, document, candidates, task_id)
}

fn apply_account_candidates_report(
    state: &mut ApiState,
    store: &AccountStore,
    mut document: overleaf_storage::AccountsDocument,
    candidates: Vec<AccountImportCandidate>,
    task_id: Option<&str>,
) -> Result<AccountImportReport, ApiResponse> {
    let report = match apply_account_import_with_backend(
        &mut document,
        &candidates,
        state.secret_backend.as_ref(),
    ) {
        Ok(report) => report,
        Err(error) => {
            fail_tracked_task(state, task_id, account_io_error_message(&error));
            return Err(account_io_error_response(error));
        }
    };
    if report.imported_count > 0 {
        if let Err(error) = store.save(&document) {
            fail_tracked_task(state, task_id, error.to_string());
            return Err(io_error_response(error));
        }
    }

    Ok(report)
}

fn import_candidate_lock_aliases(
    document: &overleaf_storage::AccountsDocument,
    candidates: &[AccountImportCandidate],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    for candidate in candidates {
        let alias = make_alias(
            candidate.alias_hint.as_deref(),
            candidate.record.email.as_deref().unwrap_or_default(),
        );
        if seen.insert(alias.clone()) {
            aliases.push(alias);
        }
        if let Some(email) = candidate.record.email.as_deref() {
            if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
                let existing_alias = existing_alias.to_string();
                if seen.insert(existing_alias.clone()) {
                    aliases.push(existing_alias);
                }
            }
        }
    }
    aliases
}

fn imported_aliases_from_import_report(report: &AccountImportReport) -> Vec<String> {
    report
        .decisions
        .iter()
        .filter(|decision| decision.status == AccountImportDecisionStatus::Imported)
        .map(|decision| decision.resolved_alias.clone())
        .collect()
}

fn account_import_post_action_report(
    state: &mut ApiState,
    store: &AccountStore,
    import: AccountImportReport,
    task_id: Option<&str>,
    post_actions: AccountImportPostActionOptions,
    now_unix: i64,
) -> AccountImportWithPostActionsReport {
    let imported_aliases = imported_aliases_from_import_report(&import);
    append_optional_task_log(
        state,
        task_id,
        TaskLogLevel::Info,
        format!(
            "账号导入写入完成：{} 个导入，{} 个重复邮箱跳过，{} 个别名冲突",
            import.imported_count,
            import.skipped_duplicate_email_count,
            import.alias_conflict_count
        ),
    );
    let mut report = AccountImportWithPostActionsReport {
        import,
        session_refresh: None,
        git_token_generation: None,
        post_action_errors: Vec::new(),
    };

    let imported_document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Error,
                format!("导入后置动作读取账号失败：{error}"),
            );
            if post_actions.refresh_session_metadata {
                report
                    .post_action_errors
                    .push(account_import_post_action_error(
                        AccountImportPostAction::RefreshSessionMetadata,
                        error.to_string(),
                    ));
            }
            if post_actions.fetch_git_token {
                report
                    .post_action_errors
                    .push(account_import_post_action_error(
                        AccountImportPostAction::FetchGitToken,
                        error.to_string(),
                    ));
            }
            return report;
        }
    };
    let session_refresh_aliases = imported_aliases
        .iter()
        .filter(|alias| {
            imported_document
                .accounts
                .get(alias.as_str())
                .is_some_and(|record| imported_account_needs_session_refresh(record, now_unix))
        })
        .cloned()
        .collect::<Vec<_>>();
    let git_token_aliases = imported_aliases.clone();

    if !session_refresh_aliases.is_empty() && post_actions.refresh_session_metadata {
        append_optional_task_log(
            state,
            task_id,
            TaskLogLevel::Info,
            format!(
                "开始刷新导入账号状态元数据：{} 个账号",
                session_refresh_aliases.len()
            ),
        );
        match block_on_api(refresh_saved_account_sessions_in_store_with_backend(
            store,
            &session_refresh_aliases,
            now_unix,
            state.secret_backend.as_ref(),
        )) {
            Ok(Ok(refresh_report)) => {
                append_session_refresh_batch_logs(
                    state,
                    task_id,
                    &refresh_report,
                    "导入后状态刷新",
                );
                append_failed_session_refresh_post_action_errors(
                    &mut report.post_action_errors,
                    &refresh_report,
                );
                report.session_refresh = Some(refresh_report);
            }
            Ok(Err(error)) => {
                append_optional_task_log(
                    state,
                    task_id,
                    TaskLogLevel::Warning,
                    format!(
                        "导入后状态刷新失败：{}",
                        account_session_error_message(&error)
                    ),
                );
                report
                    .post_action_errors
                    .push(account_import_post_action_error(
                        AccountImportPostAction::RefreshSessionMetadata,
                        account_session_error_message(&error),
                    ));
            }
            Err(response) => {
                let message = response.body.clone();
                append_optional_task_log(
                    state,
                    task_id,
                    TaskLogLevel::Warning,
                    format!("导入后状态刷新失败：{message}"),
                );
                report
                    .post_action_errors
                    .push(account_import_post_action_error(
                        AccountImportPostAction::RefreshSessionMetadata,
                        message,
                    ));
            }
        }
    } else if post_actions.refresh_session_metadata {
        append_optional_task_log(
            state,
            task_id,
            TaskLogLevel::Info,
            "导入后状态刷新已跳过：没有需要更新的账号",
        );
    }

    if !git_token_aliases.is_empty() && post_actions.fetch_git_token {
        append_optional_task_log(
            state,
            task_id,
            TaskLogLevel::Info,
            format!(
                "开始获取导入账号 Git 令牌：{} 个账号",
                git_token_aliases.len()
            ),
        );
        if let Some(browser) = state.browser_automation.clone() {
            match block_on_api(
                generate_account_git_token_with_browser_in_store_with_backend(
                    store,
                    &git_token_aliases,
                    browser.as_ref(),
                    now_unix,
                    state.secret_backend.as_ref(),
                ),
            ) {
                Ok(Ok(git_report)) => {
                    append_git_token_batch_logs(state, task_id, &git_report, "导入后 Git 令牌");
                    append_failed_git_token_post_action_errors(
                        &mut report.post_action_errors,
                        &git_report,
                    );
                    report.git_token_generation = Some(git_report);
                }
                Ok(Err(error)) => {
                    append_optional_task_log(
                        state,
                        task_id,
                        TaskLogLevel::Warning,
                        format!(
                            "导入后 Git 令牌获取失败：{}",
                            account_git_token_error_message(&error)
                        ),
                    );
                    report
                        .post_action_errors
                        .push(account_import_post_action_error(
                            AccountImportPostAction::FetchGitToken,
                            account_git_token_error_message(&error),
                        ));
                }
                Err(response) => {
                    let message = response.body.clone();
                    append_optional_task_log(
                        state,
                        task_id,
                        TaskLogLevel::Warning,
                        format!("导入后 Git 令牌获取失败：{message}"),
                    );
                    report
                        .post_action_errors
                        .push(account_import_post_action_error(
                            AccountImportPostAction::FetchGitToken,
                            message,
                        ));
                }
            }
        } else {
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Warning,
                "导入后 Git 令牌获取失败：浏览器自动化未配置",
            );
            report
                .post_action_errors
                .push(account_import_post_action_error(
                    AccountImportPostAction::FetchGitToken,
                    "browser automation is not configured".to_string(),
                ));
        }
    } else if post_actions.fetch_git_token {
        append_optional_task_log(
            state,
            task_id,
            TaskLogLevel::Info,
            "导入后 Git 令牌获取已跳过：没有需要处理的账号",
        );
    }

    report
}

pub(super) fn append_optional_task_log(
    state: &mut ApiState,
    task_id: Option<&str>,
    level: TaskLogLevel,
    message: impl Into<String>,
) {
    if let Some(task_id) = task_id {
        let _ = state.tasks.append_log(task_id, level, message);
    }
}

fn append_session_refresh_batch_logs(
    state: &mut ApiState,
    task_id: Option<&str>,
    report: &AccountSessionRefreshBatchReport,
    label: &str,
) {
    for item in &report.items {
        if item.refreshed {
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Info,
                format!("{label}：{} 完成", item.alias),
            );
        } else {
            let detail = item
                .error
                .as_ref()
                .map(account_session_error_message)
                .unwrap_or_else(|| "未知错误".to_string());
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Warning,
                format!("{label}：{} 失败：{detail}", item.alias),
            );
        }
    }
    append_optional_task_log(
        state,
        task_id,
        if report.failed_count == 0 {
            TaskLogLevel::Info
        } else {
            TaskLogLevel::Warning
        },
        format!(
            "{label}完成：成功 {} 个，失败 {} 个",
            report.refreshed_count, report.failed_count
        ),
    );
}

pub(super) fn append_git_token_batch_logs(
    state: &mut ApiState,
    task_id: Option<&str>,
    report: &AccountGitTokenRefreshBatchReport,
    label: &str,
) {
    for item in &report.items {
        if item.skipped {
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Info,
                format!("{label}：{} 跳过（已有有效 token）", item.alias),
            );
        } else if item.refreshed {
            let status = item
                .report
                .as_ref()
                .map(|report| match report.status {
                    GitTokenRefreshStatus::VisibleTokenSaved => "已读取并保存 token",
                    GitTokenRefreshStatus::ExistingMaskedTokenOnly => "已确认已有 token",
                    GitTokenRefreshStatus::TokenActionAvailable => "未发现已保存 token，可生成",
                    GitTokenRefreshStatus::NotFound => "未发现 token 或生成入口",
                })
                .unwrap_or("状态已读取");
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Info,
                format!("{label}：{} 完成（{status}）", item.alias),
            );
        } else {
            let detail = item
                .error
                .as_ref()
                .map(account_git_token_error_message)
                .unwrap_or_else(|| "未知错误".to_string());
            append_optional_task_log(
                state,
                task_id,
                TaskLogLevel::Warning,
                format!("{label}：{} 失败：{detail}", item.alias),
            );
        }
    }
    append_optional_task_log(
        state,
        task_id,
        if report.failed_count == 0 {
            TaskLogLevel::Info
        } else {
            TaskLogLevel::Warning
        },
        format!(
            "{label}完成：成功 {} 个，跳过 {} 个，失败 {} 个",
            report.refreshed_count, report.skipped_count, report.failed_count
        ),
    );
}

fn append_failed_session_refresh_post_action_errors(
    errors: &mut Vec<AccountImportPostActionError>,
    report: &AccountSessionRefreshBatchReport,
) {
    let mut represented_failures = 0;
    for item in &report.items {
        if item.refreshed {
            continue;
        }
        represented_failures += 1;
        let detail = item
            .error
            .as_ref()
            .map(account_session_error_message)
            .unwrap_or_else(|| "未知错误".to_string());
        errors.push(account_import_post_action_error(
            AccountImportPostAction::RefreshSessionMetadata,
            format!("{}：{}", item.alias, detail),
        ));
    }
    append_unrepresented_batch_failure(
        errors,
        AccountImportPostAction::RefreshSessionMetadata,
        report.failed_count,
        represented_failures,
    );
}

fn append_failed_git_token_post_action_errors(
    errors: &mut Vec<AccountImportPostActionError>,
    report: &AccountGitTokenRefreshBatchReport,
) {
    let mut represented_failures = 0;
    for item in &report.items {
        if item.refreshed || item.skipped {
            continue;
        }
        represented_failures += 1;
        let detail = item
            .error
            .as_ref()
            .map(account_git_token_error_message)
            .unwrap_or_else(|| "未知错误".to_string());
        errors.push(account_import_post_action_error(
            AccountImportPostAction::FetchGitToken,
            format!("{}：{}", item.alias, detail),
        ));
    }
    append_unrepresented_batch_failure(
        errors,
        AccountImportPostAction::FetchGitToken,
        report.failed_count,
        represented_failures,
    );
}

fn append_unrepresented_batch_failure(
    errors: &mut Vec<AccountImportPostActionError>,
    action: AccountImportPostAction,
    failed_count: usize,
    represented_failures: usize,
) {
    let missing_failures = failed_count.saturating_sub(represented_failures);
    if missing_failures > 0 {
        errors.push(account_import_post_action_error(
            action,
            format!("批次报告还有 {missing_failures} 个未归属失败账号"),
        ));
    }
}

fn imported_account_needs_session_refresh(record: &AccountRecord, now_unix: i64) -> bool {
    let status_key = record
        .subscription_status
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let status_is_known = matches!(
        status_key.as_str(),
        "trial" | "pro" | "subscription" | "free"
    );
    let has_user = record
        .user_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let checked_recently = record.subscription_checked_at.is_some_and(|checked_at| {
        checked_at >= now_unix as f64 - IMPORT_SESSION_METADATA_FRESH_SECONDS
    });
    let trial_metadata_missing = status_key == "trial" && record.trial_expiry.is_none();
    !(status_is_known && has_user && checked_recently && !trial_metadata_missing)
}

fn account_import_post_action_error(
    action: AccountImportPostAction,
    message: String,
) -> AccountImportPostActionError {
    AccountImportPostActionError { action, message }
}
