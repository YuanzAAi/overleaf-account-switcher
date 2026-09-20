use super::*;

pub(super) fn continue_registration_with_new_credentials_response(
    state: &mut ApiState,
    task_id: &str,
    value: &str,
    now_unix: i64,
) -> Option<ApiResponse> {
    let mut entry = take_registration_session(state, task_id)?;
    let _ = state
        .tasks
        .discard_user_inputs(task_id, TaskUserInputKind::NewRegistrationCredentials);
    let input: RegistrationCredentialsTaskInput = match serde_json::from_str(value) {
        Ok(input) => input,
        Err(error) => {
            let old_email = entry.context.email.clone();
            if let Err(response) = insert_registration_session(state, task_id, entry) {
                return Some(response);
            }
            let _ = state
                .tasks
                .append_log(task_id, TaskLogLevel::Warning, "替换注册信息格式无效");
            wait_for_registration_new_credentials(state, task_id, &old_email);
            return Some(json_response(
                400,
                &ApiErrorBody {
                    error: format!("invalid registration credentials input: {error}"),
                },
            ));
        }
    };

    let alias_hint = normalized_optional_text(input.alias);
    let email = input.email.trim().to_string();
    if let Err(error) =
        validate_registration_input(&email, &input.password, entry.context.trial_days)
    {
        return Some(reinsert_registration_session_with_input_error(
            state,
            task_id,
            entry,
            account_registration_error_message(&error),
        ));
    }

    let alias = make_alias(alias_hint.as_deref(), &email);
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => {
            cleanup_registration_session(entry);
            return Some(io_error_response(error));
        }
    };
    if let Some(existing_alias) = document.duplicate_alias_by_email(&email) {
        let trial_days = entry.context.trial_days;
        cleanup_registration_session(entry);
        let report = AccountRegistrationReport {
            alias,
            email,
            trial_days,
            status: AccountRegistrationStatus::SkippedDuplicateEmail,
            existing_alias: Some(existing_alias.to_string()),
            subscription_succeeded: false,
            cookie_saved: false,
            git_token_saved: false,
            trial_expiry: None,
            git_token_expiry: None,
            missing_recoverable_artifacts: Vec::new(),
        };
        complete_tracked_task(state, Some(task_id), "邮箱已在本地存在，跳过注册", &report);
        return Some(json_response(200, &report));
    }
    if document.alias_conflicts_with_email(&alias, &email) {
        return Some(reinsert_registration_session_with_input_error(
            state,
            task_id,
            entry,
            format!("account alias already exists: {alias}"),
        ));
    }

    let trial_days = entry.context.trial_days;
    let auto_fetch_git_token = entry.context.auto_fetch_git_token;
    let card_selection_strategy = entry.context.card_selection_strategy;
    let card_bin = entry.context.card_bin.clone();
    let password = SecretText::new(input.password);
    entry.context = RegistrationSessionContext {
        alias_hint: alias_hint.clone(),
        existing_alias: None,
        email: email.clone(),
        password: Some(password.clone()),
        trial_days,
        auto_fetch_git_token,
        card_selection_strategy,
        card_bin,
    };
    set_registration_task_step(state, task_id, trial_days, RegistrationState::OpenSignup);
    set_registration_task_step(
        state,
        task_id,
        trial_days,
        RegistrationState::FillCredentials,
    );

    let result = entry
        .runtime
        .block_on(
            entry
                .session
                .automation()
                .register_and_subscribe(RegistrationInput {
                    alias_hint: alias_hint.clone(),
                    email: email.clone(),
                    password: password.clone(),
                    trial_days,
                    auto_fetch_git_token,
                }),
        );

    Some(handle_registration_session_result(
        state, task_id, entry, result, now_unix,
    ))
}

pub(super) fn continue_registration_after_captcha_response(
    state: &mut ApiState,
    task_id: &str,
    now_unix: i64,
) -> Option<ApiResponse> {
    let mut entry = take_registration_session(state, task_id)?;
    let _ = state
        .tasks
        .discard_user_inputs(task_id, TaskUserInputKind::CaptchaCompleted);
    entry.last_activity = Instant::now();
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::DetectRegisteredEmail,
    );
    let result = entry
        .runtime
        .block_on(entry.session.automation().current_registration_state());

    Some(match result {
        Ok(registration_state) => continue_registration_from_state_response(
            state,
            task_id,
            entry,
            registration_state,
            now_unix,
        ),
        Err(error) => {
            cleanup_registration_session(entry);
            handle_registration_error(state, Some(task_id), AccountRegistrationError::from(error))
        }
    })
}

pub(super) fn continue_registration_with_email_code_response(
    state: &mut ApiState,
    task_id: &str,
    code: &str,
    now_unix: i64,
) -> Option<ApiResponse> {
    let mut entry = take_registration_session(state, task_id)?;
    let _ = state
        .tasks
        .discard_user_inputs(task_id, TaskUserInputKind::EmailCode);
    entry.last_activity = Instant::now();
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::SubmitEmailCode,
    );
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "正在填写邮箱验证码并等待确认页面跳转",
    );
    let browser_focused = focus_browser_process_window(entry.session.process_id());
    let _ = state.tasks.append_log(
        task_id,
        if browser_focused {
            TaskLogLevel::Info
        } else {
            TaskLogLevel::Warning
        },
        if browser_focused {
            "已切回注册浏览器继续流程"
        } else {
            "未能自动前置注册浏览器，请手动切换；任务将继续运行"
        },
    );
    let result = entry.runtime.block_on(
        entry
            .session
            .automation()
            .submit_registration_email_code(code),
    );

    match result {
        Ok(CdpRegistrationState::AccountCreated) => Some(
            continue_registration_account_created_response(state, task_id, entry, now_unix),
        ),
        Ok(CdpRegistrationState::EmailVerificationFailed) => {
            wait_for_registration_email_code(state, task_id, &entry.context.email);
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                "邮箱验证码无效，请重新输入",
            );
            if let Err(response) = insert_registration_session(state, task_id, entry) {
                return Some(response);
            }
            Some(registration_waiting_response(state, task_id))
        }
        Ok(CdpRegistrationState::EmailVerificationRequired | CdpRegistrationState::Pending) => {
            wait_for_registration_email_code(state, task_id, &entry.context.email);
            if let Err(response) = insert_registration_session(state, task_id, entry) {
                return Some(response);
            }
            Some(registration_waiting_response(state, task_id))
        }
        Ok(CdpRegistrationState::ChallengeRequired) => Some(
            registration_waiting_for_captcha_response(state, task_id, entry),
        ),
        Ok(CdpRegistrationState::RegisteredEmail) => {
            let email = entry.context.email.clone();
            Some(registration_waiting_for_new_credentials_response(
                state, task_id, email, entry,
            ))
        }
        Ok(CdpRegistrationState::SubscriptionSucceeded) => Some(
            continue_detected_registration_subscription_response(state, task_id, entry, now_unix),
        ),
        Err(error) if registration_email_code_error_is_recoverable(&error) => {
            wait_for_registration_email_code(state, task_id, &entry.context.email);
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                "邮箱验证码页面尚未完成，请重新提交验证码",
            );
            if let Err(response) = insert_registration_session(state, task_id, entry) {
                return Some(response);
            }
            Some(registration_waiting_response(state, task_id))
        }
        Err(error) => {
            let _ = entry.runtime.block_on(entry.session.cleanup());
            Some(handle_registration_error(
                state,
                Some(task_id),
                AccountRegistrationError::from(error),
            ))
        }
    }
}

fn registration_email_code_error_is_recoverable(error: &BrowserAutomationError) -> bool {
    match error {
        BrowserAutomationError::ElementNotFound { selector } => {
            selector.starts_with("email verification")
        }
        BrowserAutomationError::Timeout { operation } => operation
            .to_ascii_lowercase()
            .contains("email verification"),
        BrowserAutomationError::Other { message } => message
            .to_ascii_lowercase()
            .contains("email verification failed"),
        _ => false,
    }
}

fn handle_registration_session_result(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    result: BrowserAutomationResult<RegistrationResult>,
    now_unix: i64,
) -> ApiResponse {
    if !matches!(
        &result,
        Err(BrowserAutomationError::ChallengeRequired { .. })
    ) {
        set_registration_task_step(
            state,
            task_id,
            entry.context.trial_days,
            RegistrationState::DetectRegisteredEmail,
        );
    }
    match result {
        Ok(result) => continue_registration_from_state_response(
            state,
            task_id,
            entry,
            if result.subscription_succeeded {
                CdpRegistrationState::SubscriptionSucceeded
            } else {
                CdpRegistrationState::AccountCreated
            },
            now_unix,
        ),
        Err(BrowserAutomationError::EmailCodeRequired { email }) => {
            registration_waiting_for_email_code_response(state, task_id, email, entry)
        }
        Err(BrowserAutomationError::RegisteredEmail { email }) => {
            registration_waiting_for_new_credentials_response(state, task_id, email, entry)
        }
        Err(BrowserAutomationError::ChallengeRequired { .. }) => {
            registration_waiting_for_captcha_response(state, task_id, entry)
        }
        Err(error) => {
            cleanup_registration_session(entry);
            handle_registration_error(state, Some(task_id), AccountRegistrationError::from(error))
        }
    }
}

pub(super) fn continue_registration_from_state_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    registration_state: CdpRegistrationState,
    now_unix: i64,
) -> ApiResponse {
    match registration_state {
        CdpRegistrationState::AccountCreated => {
            continue_registration_account_created_response(state, task_id, entry, now_unix)
        }
        CdpRegistrationState::SubscriptionSucceeded => {
            continue_detected_registration_subscription_response(state, task_id, entry, now_unix)
        }
        CdpRegistrationState::EmailVerificationRequired
        | CdpRegistrationState::EmailVerificationFailed => {
            let email = entry.context.email.clone();
            registration_waiting_for_email_code_response(state, task_id, email, entry)
        }
        CdpRegistrationState::RegisteredEmail => {
            let email = entry.context.email.clone();
            registration_waiting_for_new_credentials_response(state, task_id, email, entry)
        }
        CdpRegistrationState::ChallengeRequired | CdpRegistrationState::Pending => {
            registration_waiting_for_captcha_response(state, task_id, entry)
        }
    }
}

fn continue_registration_account_created_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
) -> ApiResponse {
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::OpenSubscription,
    );
    let mut result = entry.runtime.block_on(
        entry
            .session
            .automation()
            .open_registration_subscription_page(entry.context.trial_days),
    );
    if result
        .as_ref()
        .err()
        .is_some_and(registration_subscription_open_error_is_retryable)
        && entry.session.is_active()
    {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Warning,
            "确认邮箱后的页面仍在切换，保留当前浏览器并重试订阅页一次",
        );
        entry
            .runtime
            .block_on(tokio::time::sleep(Duration::from_millis(500)));
        result = entry.runtime.block_on(
            entry
                .session
                .automation()
                .open_registration_subscription_page(entry.context.trial_days),
        );
    }

    match result {
        Ok(CdpRegistrationState::AccountCreated) => {
            continue_registration_subscription_forms_response(state, task_id, entry, now_unix)
        }
        Ok(CdpRegistrationState::SubscriptionSucceeded) => {
            continue_detected_registration_subscription_response(state, task_id, entry, now_unix)
        }
        Ok(CdpRegistrationState::ChallengeRequired | CdpRegistrationState::Pending) => {
            registration_waiting_for_captcha_response(state, task_id, entry)
        }
        Ok(CdpRegistrationState::RegisteredEmail) => {
            let email = entry.context.email.clone();
            registration_waiting_for_new_credentials_response(state, task_id, email, entry)
        }
        Ok(CdpRegistrationState::EmailVerificationRequired)
        | Ok(CdpRegistrationState::EmailVerificationFailed) => {
            let email = entry.context.email.clone();
            registration_waiting_for_email_code_response(state, task_id, email, entry)
        }
        Err(error) => {
            cleanup_registration_session(entry);
            handle_registration_error(state, Some(task_id), AccountRegistrationError::from(error))
        }
    }
}

fn registration_subscription_open_error_is_retryable(error: &BrowserAutomationError) -> bool {
    match error {
        BrowserAutomationError::Timeout { operation } => operation
            .to_ascii_lowercase()
            .contains("overleaf subscription page"),
        BrowserAutomationError::ExternalDriver { message } => {
            let message = message.to_ascii_lowercase();
            message.contains("execution context was destroyed")
                || message.contains("cannot find default execution context")
                || message.contains("cannot find context with specified id")
                || message.contains("inspected target navigated or closed")
        }
        _ => false,
    }
}

fn registration_waiting_for_email_code_response(
    state: &mut ApiState,
    task_id: &str,
    email: String,
    mut entry: RegistrationSessionEntry,
) -> ApiResponse {
    entry.last_activity = Instant::now();
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::WaitEmailCode,
    );
    if let Err(response) = insert_registration_session(state, task_id, entry) {
        return response;
    }
    wait_for_registration_email_code(state, task_id, &email);
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Warning,
        format!("等待邮箱验证码: {email}"),
    );
    registration_waiting_response(state, task_id)
}

fn registration_waiting_for_new_credentials_response(
    state: &mut ApiState,
    task_id: &str,
    email: String,
    mut entry: RegistrationSessionEntry,
) -> ApiResponse {
    entry.last_activity = Instant::now();
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::DetectRegisteredEmail,
    );
    if let Err(response) = insert_registration_session(state, task_id, entry) {
        return response;
    }
    wait_for_registration_new_credentials(state, task_id, &email);
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Warning,
        format!("邮箱已注册: {email}"),
    );
    registration_waiting_response(state, task_id)
}

fn registration_waiting_for_captcha_response(
    state: &mut ApiState,
    task_id: &str,
    mut entry: RegistrationSessionEntry,
) -> ApiResponse {
    entry.last_activity = Instant::now();
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::WaitCaptcha,
    );
    if let Err(response) = insert_registration_session(state, task_id, entry) {
        return response;
    }
    wait_for_registration_captcha(state, task_id);
    let _ = state
        .tasks
        .append_log(task_id, TaskLogLevel::Warning, "等待用户完成 reCAPTCHA");
    registration_waiting_response(state, task_id)
}

fn reinsert_registration_session_with_input_error(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    message: impl Into<String>,
) -> ApiResponse {
    let message = message.into();
    let old_email = entry.context.email.clone();
    if let Err(response) = insert_registration_session(state, task_id, entry) {
        return response;
    }
    wait_for_registration_new_credentials(state, task_id, &old_email);
    let _ = state
        .tasks
        .append_log(task_id, TaskLogLevel::Warning, message.clone());
    json_response(400, &ApiErrorBody { error: message })
}

fn continue_registration_subscription_forms_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
) -> ApiResponse {
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::FetchAddress,
    );
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "邮箱验证码已提交，已进入订阅页面",
    );

    let address_store = AddressStore::new(state.config.addresses_path());
    let address_provider = Arc::clone(&state.address_provider);
    let address_selection = if let Some(selection) = state.take_prefetched_registration_address() {
        selection
    } else {
        match block_on_api(select_registration_address(
            address_provider.as_ref(),
            &address_store,
            0,
        )) {
            Ok(Ok(selection)) => selection,
            Ok(Err(error)) => {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!(
                        "订阅页已打开，但还没有可用地址: {}",
                        address_action_error_message(&error)
                    ),
                );
                if let Err(response) = insert_registration_session(state, task_id, entry) {
                    return response;
                }
                return registration_waiting_response(state, task_id);
            }
            Err(response) => {
                if let Err(insert_response) = insert_registration_session(state, task_id, entry) {
                    return insert_response;
                }
                return response;
            }
        }
    };

    match address_selection.source {
        RegistrationAddressSource::Api => {
            let _ = state
                .tasks
                .append_log(task_id, TaskLogLevel::Info, "从地址 API 获取地址成功");
        }
        RegistrationAddressSource::Fallback => {
            let message = address_selection
                .api_error
                .as_ref()
                .map(address_action_error_message)
                .unwrap_or_else(|| "unknown API error".to_string());
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!("地址 API 获取失败，使用 fallback 地址: {message}"),
            );
        }
    }

    let address = address_selection.address;
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::FillAddress,
    );

    let address = StripeAddressInput {
        name: address.name.clone(),
        line1: address.line1.clone(),
        city: address.city.clone(),
        state: address.state.clone(),
        zip: address.zip.clone(),
    };
    continue_registration_payment_with_card_retries_response(
        state, task_id, entry, address, now_unix,
    )
}

fn continue_registration_payment_with_card_retries_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    address: StripeAddressInput,
    now_unix: i64,
) -> ApiResponse {
    let card_store = CardStore::new(state.config.cards_path());
    let mut attempted_cards = BTreeSet::new();

    loop {
        let card_document = match card_store.load() {
            Ok(document) => document,
            Err(error) => {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!("订阅页已打开，但无法读取银行卡库: {error}"),
                );
                if attempted_cards.is_empty() {
                    if let Err(response) = insert_registration_session(state, task_id, entry) {
                        return response;
                    }
                    return registration_waiting_response(state, task_id);
                }
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::Browser {
                        message: "payment cards exhausted after retries".to_string(),
                    },
                );
            }
        };
        let card = match select_payment_card_with_strategy_and_bin_with_backend_at(
            &card_document,
            &attempted_cards,
            entry.context.card_selection_strategy,
            entry.context.card_bin.as_deref(),
            state.secret_backend.as_ref(),
            now_unix,
        ) {
            Ok(Some(card)) => card,
            Ok(None) => {
                if attempted_cards.is_empty() {
                    let _ = state.tasks.append_log(
                        task_id,
                        TaskLogLevel::Warning,
                        format!(
                            "订阅页已打开，但没有符合 {:?} 策略和当前 BIN 范围的可用银行卡",
                            entry.context.card_selection_strategy,
                        ),
                    );
                    if let Err(response) = insert_registration_session(state, task_id, entry) {
                        return response;
                    }
                    return registration_waiting_response(state, task_id);
                }
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::Browser {
                        message: "payment failed and no unattempted card remains".to_string(),
                    },
                );
            }
            Err(error) => {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!(
                        "读取银行卡敏感字段失败: {}",
                        card_action_error_message(&error)
                    ),
                );
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::Browser {
                        message: card_action_error_message(&error),
                    },
                );
            }
        };
        attempted_cards.insert(card.number.clone());

        set_registration_task_step(
            state,
            task_id,
            entry.context.trial_days,
            RegistrationState::SelectCard,
        );
        set_registration_task_step(
            state,
            task_id,
            entry.context.trial_days,
            RegistrationState::FillPayment,
        );

        let payment = StripePaymentInput {
            number: card.number.clone(),
            exp_month: card.exp_month,
            exp_year: card.exp_year,
            cvc: card.cvc,
        };
        let selected_card_number = card.number.clone();
        let fill_result = entry.runtime.block_on(
            entry
                .session
                .fill_registration_payment_frames(address.clone(), payment),
        );
        match fill_result {
            Ok(result) => {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Info,
                    format!(
                        "已填写订阅地址和支付 iframe 字段: address_fields={}, payment_fields={}",
                        result.address.filled.len(),
                        result.payment.filled.len()
                    ),
                );
            }
            Err(error) => {
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        }

        set_registration_task_step(
            state,
            task_id,
            entry.context.trial_days,
            RegistrationState::SubmitPayment,
        );
        let payment_result = entry.runtime.block_on(
            entry
                .session
                .automation()
                .submit_registration_payment_and_wait_thank_you(),
        );
        match payment_result {
            Ok(CdpRegistrationState::SubscriptionSucceeded) => {
                set_registration_task_step(
                    state,
                    task_id,
                    entry.context.trial_days,
                    RegistrationState::WaitThankYou,
                );
                let _ =
                    state
                        .tasks
                        .append_log(task_id, TaskLogLevel::Info, "支付成功，已确认订阅生效");
                if let Err(error) = mark_card_used_in_store_with_backend(
                    &card_store,
                    &selected_card_number,
                    state.secret_backend.as_ref(),
                ) {
                    let _ = state.tasks.append_log(
                        task_id,
                        TaskLogLevel::Warning,
                        format!(
                            "支付成功，但标记银行卡已使用失败: {}",
                            card_action_error_message(&error)
                        ),
                    );
                }
                break;
            }
            Ok(other_state) => {
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::Browser {
                        message: format!(
                            "payment did not reach subscription success: {other_state:?}"
                        ),
                    },
                );
            }
            Err(error @ BrowserAutomationError::Other { .. }) => {
                let message = error.to_string();
                if let Err(card_error) = mark_card_failed_in_store_with_backend(
                    &card_store,
                    &selected_card_number,
                    &message,
                    state.secret_backend.as_ref(),
                ) {
                    let _ = state.tasks.append_log(
                        task_id,
                        TaskLogLevel::Warning,
                        format!(
                            "支付失败，且标记银行卡失败状态失败: {}",
                            card_action_error_message(&card_error)
                        ),
                    );
                }
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "当前银行卡支付失败，正在尝试下一张可用卡",
                );
            }
            Err(error) => {
                let _ = entry.runtime.block_on(entry.session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        }
    }

    let accept_extra_trial_offer = entry.context.trial_days == 21;
    continue_registration_after_payment_success_response(
        state,
        task_id,
        entry,
        now_unix,
        accept_extra_trial_offer,
    )
}

fn continue_registration_after_payment_success_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
    accept_extra_trial_offer: bool,
) -> ApiResponse {
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::OpenSubscriptionManagement,
    );
    if let Err(error) = require_confirmed_registration_subscription_step(
        "open subscription management",
        entry.runtime.block_on(
            entry
                .session
                .automation()
                .open_registration_subscription_management(),
        ),
    ) {
        let _ = entry.runtime.block_on(entry.session.cleanup());
        return handle_registration_error(state, Some(task_id), error);
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "已确认打开订阅管理页并读取到有效订阅状态",
    );

    if accept_extra_trial_offer {
        set_registration_task_step(
            state,
            task_id,
            entry.context.trial_days,
            RegistrationState::AcceptExtraTrial,
        );
        if let Err(error) = accept_confirmed_extra_trial(&entry) {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!("额外试用未确认: {error:?}；继续取消自动续费并按实际到期时间保存账号"),
            );
        } else {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "已确认试用到期时间延长 14 天，正在重新加载订阅管理页",
            );
        }
        if let Err(error) = require_confirmed_registration_subscription_step(
            "reopen subscription management after extra trial offer",
            entry.runtime.block_on(
                entry
                    .session
                    .automation()
                    .open_registration_subscription_management(),
            ),
        ) {
            let _ = entry.runtime.block_on(entry.session.cleanup());
            return handle_registration_error(state, Some(task_id), error);
        }
    }

    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::ChangePlanToProAnnual,
    );
    if let Err(error) = require_confirmed_registration_subscription_step(
        "change subscription to Pro annual",
        entry.runtime.block_on(
            entry
                .session
                .automation()
                .change_registration_plan_to_pro_annual(),
        ),
    ) {
        let _ = entry.runtime.block_on(entry.session.cleanup());
        return handle_registration_error(state, Some(task_id), error);
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "已确认订阅计划切换为 Pro annual",
    );

    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::CancelSubscription,
    );
    if let Err(error) = require_confirmed_registration_subscription_step(
        "confirm final subscription cancellation",
        entry.runtime.block_on(
            entry
                .session
                .automation()
                .cancel_registration_subscription_final(),
        ),
    ) {
        let _ = entry.runtime.block_on(entry.session.cleanup());
        return handle_registration_error(state, Some(task_id), error);
    }
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "已确认订阅取消完成，继续提取账号资料",
    );

    finalize_registration_session_response(state, task_id, entry, now_unix)
}

fn accept_confirmed_extra_trial(
    entry: &RegistrationSessionEntry,
) -> Result<(), AccountRegistrationError> {
    entry.runtime.block_on(async {
        let automation = entry.session.automation();
        let login = automation.finalize_login_result(None).await?;
        let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies_to_map(
            &login.cookies,
        )));
        let read_expiry = || async {
            client
                .fetch_trial_expiry()
                .await
                .map_err(|error| AccountRegistrationError::Browser {
                    message: format!("读取试用延期结果失败: {error}"),
                })
        };
        let before = read_expiry()
            .await?
            .ok_or_else(|| AccountRegistrationError::Browser {
                message: "未能读取延期前的到期时间，无法验证额外试用".into(),
            })?;
        automation.accept_registration_extra_trial_offer().await?;
        let verify = async {
            loop {
                if read_expiry()
                    .await?
                    .is_some_and(|after| after >= before + 14 * 86_400)
                {
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), verify)
            .await
            .map_err(|_| AccountRegistrationError::Browser {
                message: "未确认试用到期时间延长 14 天，额外试用未完成".into(),
            })?
    })
}

fn require_confirmed_registration_subscription_step(
    step: &'static str,
    result: BrowserAutomationResult<CdpRegistrationState>,
) -> Result<(), AccountRegistrationError> {
    match result {
        Ok(CdpRegistrationState::SubscriptionSucceeded) => Ok(()),
        Ok(other_state) => Err(AccountRegistrationError::Browser {
            message: format!(
                "registration step {step} did not reach a confirmed subscription state: {other_state:?}"
            ),
        }),
        Err(error) => Err(AccountRegistrationError::from(error)),
    }
}

fn continue_detected_registration_subscription_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
) -> ApiResponse {
    let accept_extra_trial_offer = entry.context.trial_days == 21;
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "已检测到订阅成功，继续完成改计划、取消订阅和账号收尾",
    );
    continue_registration_after_payment_success_response(
        state,
        task_id,
        entry,
        now_unix,
        accept_extra_trial_offer,
    )
}

fn finalize_registration_session_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
) -> ApiResponse {
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::ParseTrialExpiry,
    );
    if let Err(error) = require_confirmed_registration_subscription_step(
        "reopen subscription management for final metadata",
        entry.runtime.block_on(
            entry
                .session
                .automation()
                .open_registration_subscription_management(),
        ),
    ) {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Warning,
            format!("重新打开订阅管理页失败，将尝试从当前页面解析到期时间: {error:?}"),
        );
    }
    let parsed_trial_expiry = match entry
        .runtime
        .block_on(entry.session.automation().current_page_state())
    {
        Ok(page_state) => {
            let status =
                overleaf_api::parse_subscription_status_with_now(&page_state.text, None, now_unix);
            if let Some(expiry) = status.trial_expiry {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Info,
                    format!("已解析试用/订阅到期时间: {expiry}"),
                );
            } else {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "未从最终订阅页面解析到试用/订阅到期时间",
                );
            }
            status.trial_expiry
        }
        Err(error) => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!("读取最终订阅页面状态失败，跳过到期时间解析: {error}"),
            );
            None
        }
    };
    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::ReturnProject,
    );
    let result = entry
        .runtime
        .block_on(entry.session.automation().finalize_registration_result(
            entry.context.email.clone(),
            entry.context.trial_days,
            entry.context.auto_fetch_git_token,
        ));
    let report = match result {
        Ok(result) => {
            set_registration_task_step(
                state,
                task_id,
                entry.context.trial_days,
                RegistrationState::ExtractCookie,
            );
            if entry.context.auto_fetch_git_token {
                set_registration_task_step(
                    state,
                    task_id,
                    entry.context.trial_days,
                    RegistrationState::ExtractGitToken,
                );
            }
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                if entry.context.auto_fetch_git_token {
                    "已返回项目页并提取 Cookie 与 Git Integration token"
                } else {
                    "已返回项目页并提取 Cookie"
                },
            );
            save_finalized_registration_result(
                state,
                task_id,
                &entry,
                result,
                parsed_trial_expiry,
                now_unix,
            )
        }
        Err(error) => Err(AccountRegistrationError::from(error)),
    };
    let _ = entry.runtime.block_on(entry.session.cleanup());

    match report {
        Ok(report) => {
            let message = if report.trial_days < entry.context.trial_days {
                "账号已保存并取消续费，实际试用期限未达到所选天数"
            } else {
                "自动注册账号完成"
            };
            complete_tracked_task(state, Some(task_id), message, &report);
            json_response(200, &report)
        }
        Err(error) => handle_registration_error(state, Some(task_id), error),
    }
}

fn save_finalized_registration_result(
    state: &mut ApiState,
    task_id: &str,
    entry: &RegistrationSessionEntry,
    mut result: RegistrationResult,
    parsed_trial_expiry: Option<i64>,
    now_unix: i64,
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    result.trial_expiry = result.trial_expiry.or(parsed_trial_expiry);
    let alias =
        entry.context.existing_alias.clone().unwrap_or_else(|| {
            make_alias(entry.context.alias_hint.as_deref(), &entry.context.email)
        });
    let cookies = cookies_to_map(&result.cookies);
    ensure_registration_result_artifacts(&alias, &result, entry.context.auto_fetch_git_token)?;

    entry
        .runtime
        .block_on(state.session_identity_validator.validate(
            &alias,
            Some(&entry.context.email),
            &cookies,
        ))
        .map_err(|error| AccountRegistrationError::SessionIdentity {
            message: format!("注册账号认证身份验证失败: {error:?}"),
        })?;

    set_registration_task_step(
        state,
        task_id,
        entry.context.trial_days,
        RegistrationState::SaveAccount,
    );
    let store = AccountStore::new(state.config.accounts_path());
    let mut report = if let Some(existing_alias) = entry.context.existing_alias.as_deref() {
        save_existing_trial_result_in_store_with_backend(
            &store,
            existing_alias,
            result,
            entry.context.auto_fetch_git_token,
            now_unix,
            state.secret_backend.as_ref(),
        )?
    } else if let Some(password) = entry.context.password.clone() {
        save_registration_result_in_store_with_backend(
            &store,
            entry.context.alias_hint.as_deref(),
            password,
            result,
            entry.context.auto_fetch_git_token,
            now_unix,
            state.secret_backend.as_ref(),
        )?
    } else {
        save_registration_result_without_password_in_store_with_backend(
            &store,
            entry.context.alias_hint.as_deref(),
            result,
            entry.context.auto_fetch_git_token,
            now_unix,
            state.secret_backend.as_ref(),
        )?
    };
    let fallback_trial_expiry = report.trial_expiry;
    let mut document = store.load().map_err(|error| AccountRegistrationError::Io {
        message: error.to_string(),
    })?;

    match entry
        .runtime
        .block_on(state.metadata_refresher.refresh_with_cookies(
            &mut document,
            &report.alias,
            &cookies,
            now_unix,
        )) {
        Ok(metadata) => {
            report.trial_expiry = metadata.trial_expiry.or(fallback_trial_expiry);
            if metadata.trial_expiry.is_none() {
                if let Some(record) = document.accounts.get_mut(&report.alias) {
                    record.set_trial_expiry(fallback_trial_expiry.map(|value| value as f64));
                }
            }
            store
                .save(&document)
                .map_err(|error| AccountRegistrationError::Io {
                    message: error.to_string(),
                })?;
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "已验证账号身份并刷新订阅/试用元数据",
            );
        }
        Err(error) => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Warning,
                format!("账号身份已验证，但订阅元数据刷新失败，保留已解析结果: {error:?}"),
            );
        }
    }

    if let Some(days) = document
        .accounts
        .get(&report.alias)
        .and_then(|record| record.trial_duration_days())
    {
        report.trial_days = days;
    }
    Ok(report)
}

fn normalize_registration_card_bin(value: &str) -> Result<Option<String>, ApiResponse> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Some(value.to_string()));
    }
    Err(json_response(
        400,
        &ApiTypedErrorBody {
            error: "card_bin must be empty or exactly six digits".to_string(),
            kind: "invalid_card_bin",
        },
    ))
}

pub(super) fn register_account_response(
    state: &mut ApiState,
    body: &str,
    now_unix: i64,
) -> ApiResponse {
    let request: RegistrationStartRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let task_id = normalized_optional_text(request.task_id);
    if !matches!(request.trial_days, 7 | 21) {
        return account_registration_error_response(AccountRegistrationError::InvalidTrialDays {
            trial_days: request.trial_days,
        });
    }
    let card_bin = match normalize_registration_card_bin(&request.card_bin) {
        Ok(card_bin) => card_bin,
        Err(response) => return response,
    };
    match request.existing_login_source {
        Some(ExistingTrialLoginSource::Alias) => {
            let Some(existing_alias) = normalized_optional_text(request.existing_alias.clone())
            else {
                return json_response(
                    400,
                    &ApiErrorBody {
                        error: "missing existing_alias".to_string(),
                    },
                );
            };
            return register_existing_account_trial_response_inner(
                state,
                &existing_alias,
                request.trial_days,
                request.auto_fetch_git_token,
                request.card_selection_strategy,
                card_bin,
                task_id.as_deref(),
                now_unix,
                true,
            );
        }
        Some(ExistingTrialLoginSource::Cookie) => {
            return register_existing_cookie_trial_response(
                state,
                request.alias.as_deref(),
                &request.email,
                &request.session_cookie,
                request.trial_days,
                request.auto_fetch_git_token,
                request.card_selection_strategy,
                card_bin,
                task_id.as_deref(),
                now_unix,
            );
        }
        Some(ExistingTrialLoginSource::Credentials) => {
            return register_existing_credentials_trial_response(
                state,
                request.alias.as_deref(),
                &request.email,
                &request.password,
                request.trial_days,
                request.auto_fetch_git_token,
                request.card_selection_strategy,
                card_bin,
                task_id.as_deref(),
                now_unix,
            );
        }
        None => {}
    }
    if let Some(existing_alias) = normalized_optional_text(request.existing_alias.clone()) {
        return register_existing_account_trial_response_inner(
            state,
            &existing_alias,
            request.trial_days,
            request.auto_fetch_git_token,
            request.card_selection_strategy,
            card_bin,
            task_id.as_deref(),
            now_unix,
            true,
        );
    }
    let alias_hint = request
        .alias
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let email = request.email.trim();
    if let Err(error) = validate_registration_input(email, &request.password, request.trial_days) {
        return account_registration_error_response(error);
    }
    let alias = make_alias(alias_hint, email);
    let registration_slot_reserved = if let Some(task_id) = task_id.as_deref() {
        if let Err(response) = reserve_registration_slot(state, task_id) {
            return response;
        }
        true
    } else {
        false
    };
    if let Err(response) = start_registration_task(
        state,
        task_id.as_deref(),
        &alias,
        request.trial_days,
        "自动注册账号",
    ) {
        if registration_slot_reserved {
            release_registration_slot(state, task_id.as_deref().unwrap_or_default());
        }
        return response;
    }

    let store = AccountStore::new(state.config.accounts_path());
    if let Ok(document) = store.load() {
        if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
            let report = AccountRegistrationReport {
                alias,
                email: email.to_string(),
                trial_days: request.trial_days,
                status: AccountRegistrationStatus::SkippedDuplicateEmail,
                existing_alias: Some(existing_alias.to_string()),
                subscription_succeeded: false,
                cookie_saved: false,
                git_token_saved: false,
                trial_expiry: None,
                git_token_expiry: None,
                missing_recoverable_artifacts: Vec::new(),
            };
            complete_tracked_task(
                state,
                task_id.as_deref(),
                "邮箱已在本地存在，跳过注册",
                &report,
            );
            return json_response(200, &report);
        }
    }

    if let (Some(task_id), Some(local_chrome)) =
        (task_id.as_deref(), state.local_chrome_browser.clone())
    {
        return register_account_with_local_chrome_session_response(
            state,
            task_id,
            alias_hint.map(ToOwned::to_owned),
            email.to_string(),
            request.password,
            request.trial_days,
            request.auto_fetch_git_token,
            request.card_selection_strategy,
            card_bin,
            local_chrome,
            now_unix,
        );
    }

    let Some(browser) = state.browser_automation.clone() else {
        return browser_not_configured_response(state, task_id.as_deref());
    };

    match block_on_api(register_account_with_subscription_in_store_with_backend(
        &store,
        AccountRegistrationInput {
            alias_hint,
            email,
            password: SecretText::new(request.password),
            trial_days: request.trial_days,
            auto_fetch_git_token: request.auto_fetch_git_token,
            now_unix,
        },
        browser.as_ref(),
        state.secret_backend.as_ref(),
    )) {
        Ok(Ok(report)) => {
            complete_tracked_task(state, task_id.as_deref(), "自动注册账号完成", &report);
            json_response(200, &report)
        }
        Ok(Err(error)) => handle_registration_error(state, task_id.as_deref(), error),
        Err(response) => {
            fail_tracked_task(state, task_id.as_deref(), response.body.clone());
            response
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn register_existing_credentials_trial_response(
    state: &mut ApiState,
    alias_hint: Option<&str>,
    email: &str,
    password: &str,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    task_id: Option<&str>,
    now_unix: i64,
) -> ApiResponse {
    let Some(task_id) = task_id else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "existing account credentials trial requires task_id".to_string(),
            },
        );
    };
    let email = email.trim();
    if email.is_empty() {
        return account_registration_error_response(AccountRegistrationError::MissingEmail);
    }
    if password.trim().is_empty() {
        return account_registration_error_response(AccountRegistrationError::MissingPassword);
    }
    let alias_hint = alias_hint.map(str::trim).filter(|value| !value.is_empty());
    let alias = make_alias(alias_hint, email);
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
        return json_response(
            409,
            &ApiTypedErrorBody {
                error: format!("邮箱已保存为账号 {existing_alias}，请改用账号别名来源"),
                kind: "existing_account_already_saved",
            },
        );
    }
    if document.alias_conflicts_with_email(&alias, email) {
        return account_registration_error_response(AccountRegistrationError::AliasConflict {
            alias,
        });
    }
    if let Err(response) = reserve_registration_slot(state, task_id) {
        return response;
    }
    if let Err(response) = start_registration_task(
        state,
        Some(task_id),
        &alias,
        trial_days,
        "已有账号密码登录开通试用",
    ) {
        release_registration_slot(state, task_id);
        return response;
    }
    let Some(local_chrome) = state.local_chrome_browser.clone() else {
        return browser_not_configured_response(state, Some(task_id));
    };
    let (runtime, session) =
        match open_account_browser_session(local_chrome, ChromeProxyAttempt::Primary) {
            Ok(value) => value,
            Err(response) => {
                fail_tracked_task(state, Some(task_id), response.body.clone());
                return response;
            }
        };
    continue_account_browser_session_response(
        state,
        task_id,
        AccountBrowserSessionEntry {
            runtime,
            session,
            context: AccountBrowserSessionContext::ExistingTrialCredentials {
                alias_hint: alias_hint.map(str::to_string),
                email: email.to_string(),
                password: SecretText::new(password),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
                session_restart_count: 0,
                now_unix,
            },
            last_activity: Instant::now(),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn register_existing_cookie_trial_response(
    state: &mut ApiState,
    alias_hint: Option<&str>,
    email: &str,
    session_cookie: &str,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    task_id: Option<&str>,
    now_unix: i64,
) -> ApiResponse {
    let Some(task_id) = task_id else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "existing account Cookie trial requires task_id".to_string(),
            },
        );
    };
    let email = email.trim();
    let session_cookie = session_cookie.trim();
    if email.is_empty() {
        return account_registration_error_response(AccountRegistrationError::MissingEmail);
    }
    if session_cookie.is_empty() {
        return json_response(
            400,
            &ApiTypedErrorBody {
                error: "missing overleaf_session2 Cookie".to_string(),
                kind: "invalid_session_cookie",
            },
        );
    }
    let alias_hint = alias_hint.map(str::trim).filter(|value| !value.is_empty());
    let alias = make_alias(alias_hint, email);
    let store = AccountStore::new(state.config.accounts_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
        return json_response(
            409,
            &ApiTypedErrorBody {
                error: format!("邮箱已保存为账号 {existing_alias}，请改用账号别名来源"),
                kind: "existing_account_already_saved",
            },
        );
    }
    if document.alias_conflicts_with_email(&alias, email) {
        return account_registration_error_response(AccountRegistrationError::AliasConflict {
            alias,
        });
    }
    if let Err(response) = reserve_registration_slot(state, task_id) {
        return response;
    }
    if let Err(response) = start_registration_task(
        state,
        Some(task_id),
        &alias,
        trial_days,
        "已有账号 Cookie 开通试用",
    ) {
        release_registration_slot(state, task_id);
        return response;
    }

    let cookies = BTreeMap::from([(
        OVERLEAF_SESSION_COOKIE_NAME.to_string(),
        session_cookie.to_string(),
    )]);
    let runtime = match build_api_runtime() {
        Ok(runtime) => runtime,
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    let evidence = match runtime.block_on(validate_trial_cookie_eligibility(
        state, task_id, &alias, email, &cookies, trial_days, now_unix,
    )) {
        Ok(evidence) => evidence,
        Err(response) => return response,
    };
    let preflight_continuation = match existing_trial_preflight(&evidence) {
        Ok(continuation) => continuation,
        Err(eligibility) => {
            return existing_account_trial_not_eligible_response(
                state,
                task_id,
                &alias,
                eligibility,
            );
        }
    };

    let Some(local_chrome) = state.local_chrome_browser.clone() else {
        return browser_not_configured_response(state, Some(task_id));
    };
    let browser_result = runtime.block_on(async {
        let session = local_chrome
            .start_registration_cdp_session_for_attempt(ChromeProxyAttempt::Primary)
            .await?;
        let result = session
            .automation()
            .login_with_session_cookie(
                BrowserAutoLoginInput {
                    overleaf_session: SecretText::new(session_cookie),
                    cookie_expiry: None,
                },
                Some(email.to_string()),
            )
            .await;
        Ok::<_, BrowserAutomationError>((session, result))
    });
    let (session, login_result) = match browser_result {
        Ok((session, Ok(result))) => (session, result),
        Ok((session, Err(error))) => {
            let _ = runtime.block_on(session.cleanup());
            return handle_registration_error(
                state,
                Some(task_id),
                AccountRegistrationError::from(error),
            );
        }
        Err(error) => {
            return handle_registration_error(
                state,
                Some(task_id),
                AccountRegistrationError::from(error),
            );
        }
    };
    let authenticated_cookies = cookies_to_map(&login_result.cookies);
    if let Err(error) = runtime.block_on(state.session_identity_validator.validate(
        &alias,
        Some(email),
        &authenticated_cookies,
    )) {
        let _ = runtime.block_on(session.cleanup());
        fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
        return account_session_error_response(error);
    }

    let continuation = match preflight_continuation {
        Some(continuation) => continuation,
        None => match inspect_rendered_existing_trial_continuation(
            state, task_id, &alias, &runtime, &session, trial_days, now_unix,
        ) {
            Ok(continuation) => continuation,
            Err(ExistingTrialResolutionError::NotEligible(eligibility)) => {
                let _ = runtime.block_on(session.cleanup());
                return existing_account_trial_not_eligible_response(
                    state,
                    task_id,
                    &alias,
                    eligibility,
                );
            }
            Err(ExistingTrialResolutionError::Browser(error)) => {
                let _ = runtime.block_on(session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        },
    };

    let entry = RegistrationSessionEntry {
        runtime,
        session,
        context: RegistrationSessionContext {
            alias_hint: alias_hint.map(str::to_string),
            existing_alias: None,
            email: email.to_string(),
            password: None,
            trial_days,
            auto_fetch_git_token,
            card_selection_strategy,
            card_bin,
        },
        last_activity: Instant::now(),
    };
    continue_existing_trial_response(state, task_id, entry, now_unix, continuation)
}

pub(super) async fn validate_trial_cookie_eligibility(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    email: &str,
    cookies: &BTreeMap<String, String>,
    trial_days: u32,
    now_unix: i64,
) -> Result<ExistingTrialEligibilityEvidence, ApiResponse> {
    if let Err(error) = state
        .session_identity_validator
        .validate(alias, Some(email), cookies)
        .await
    {
        fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
        return Err(account_session_error_response(error));
    }
    let client = OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
    let status = match client.fetch_trial_eligibility(now_unix, trial_days).await {
        Ok(status) => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                format!(
                    "试用资格检查完成: eligibility={:?}, subscription_state={:?}, trial_expiry_present={}, plan_availability={:?}",
                    status.eligibility,
                    status.subscription.state,
                    status.subscription.trial_expiry.is_some(),
                    status.plan_availability
                ),
            );
            status
        }
        Err(error) => {
            let message = format!("检查账号试用资格失败: {error}");
            fail_tracked_task(state, Some(task_id), message.clone());
            return Err(json_response(503, &ApiErrorBody { error: message }));
        }
    };
    Ok(existing_trial_evidence_from_status(status))
}

fn existing_trial_continuation(
    eligibility: TrialEligibility,
) -> Result<ExistingTrialContinuation, TrialEligibility> {
    match eligibility {
        TrialEligibility::Eligible => Ok(ExistingTrialContinuation::Purchase),
        TrialEligibility::ActiveTrial => Ok(ExistingTrialContinuation::ResumeActiveTrial),
        TrialEligibility::Ineligible | TrialEligibility::Unknown => Err(eligibility),
    }
}

fn existing_trial_evidence_from_status(
    status: TrialEligibilityStatus,
) -> ExistingTrialEligibilityEvidence {
    ExistingTrialEligibilityEvidence {
        eligibility: status.eligibility,
        plan_availability: status.plan_availability,
        subscription_state: status.subscription.state,
        trial_expiry: status.subscription.trial_expiry,
    }
}

pub(super) fn existing_trial_preflight(
    evidence: &ExistingTrialEligibilityEvidence,
) -> Result<Option<ExistingTrialContinuation>, TrialEligibility> {
    match existing_trial_continuation(evidence.eligibility) {
        Ok(continuation) => Ok(Some(continuation)),
        Err(_)
            if evidence.eligibility == TrialEligibility::Ineligible
                && evidence.plan_availability == TrialPlanAvailability::ExistingSubscription
                && matches!(
                    evidence.subscription_state,
                    SubscriptionState::Free | SubscriptionState::Unknown
                )
                && evidence.trial_expiry.is_none() =>
        {
            Ok(None)
        }
        Err(eligibility) => Err(eligibility),
    }
}

pub(super) fn inspect_rendered_existing_trial_continuation(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    runtime: &tokio::runtime::Runtime,
    session: &LocalChromeCdpSession,
    trial_days: u32,
    now_unix: i64,
) -> Result<ExistingTrialContinuation, ExistingTrialResolutionError> {
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        "HTTP 页面资格结果存在冲突，正在使用已登录浏览器核验试用购买页",
    );
    let (purchase_availability, page_state) = runtime
        .block_on(async {
            let purchase_availability = session
                .automation()
                .inspect_registration_trial_purchase_availability(trial_days)
                .await?;
            let page_state = session.automation().current_page_state().await?;
            Ok::<_, BrowserAutomationError>((purchase_availability, page_state))
        })
        .map_err(ExistingTrialResolutionError::Browser)?;
    let subscription = parse_subscription_status_with_now(&page_state.text, None, now_unix);
    let eligibility = match purchase_availability {
        CdpTrialPurchaseAvailability::Available => TrialEligibility::Eligible,
        CdpTrialPurchaseAvailability::Unavailable => TrialEligibility::Ineligible,
        CdpTrialPurchaseAvailability::Unknown => {
            classify_trial_eligibility(&subscription, TrialPlanAvailability::Unknown, now_unix)
        }
    };
    let _ = state.tasks.append_log(
        task_id,
        TaskLogLevel::Info,
        format!(
            "浏览器试用资格核验完成: alias={alias}, purchase_availability={purchase_availability:?}, eligibility={eligibility:?}, subscription_state={:?}, trial_expiry_present={}",
            subscription.state,
            subscription.trial_expiry.is_some()
        ),
    );
    if purchase_availability == CdpTrialPurchaseAvailability::Available {
        Ok(ExistingTrialContinuation::PurchasePageReady)
    } else {
        existing_trial_continuation(eligibility).map_err(ExistingTrialResolutionError::NotEligible)
    }
}

fn subscription_state_from_key(value: &str) -> SubscriptionState {
    match value.trim().to_ascii_lowercase().as_str() {
        "trial" => SubscriptionState::Trial,
        "pro" => SubscriptionState::Pro,
        "subscription" => SubscriptionState::Subscription,
        "free" => SubscriptionState::Free,
        _ => SubscriptionState::Unknown,
    }
}

pub(super) fn continue_existing_trial_response(
    state: &mut ApiState,
    task_id: &str,
    entry: RegistrationSessionEntry,
    now_unix: i64,
    continuation: ExistingTrialContinuation,
) -> ApiResponse {
    match continuation {
        ExistingTrialContinuation::Purchase => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "已确认已有账号登录态与试用资格，进入统一订阅流程",
            );
            continue_registration_account_created_response(state, task_id, entry, now_unix)
        }
        ExistingTrialContinuation::PurchasePageReady => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "已确认已有账号试用购买页就绪，进入统一订阅流程",
            );
            continue_registration_subscription_forms_response(state, task_id, entry, now_unix)
        }
        ExistingTrialContinuation::ResumeActiveTrial => {
            let _ = state.tasks.append_log(
                task_id,
                TaskLogLevel::Info,
                "检测到账号已处于试用中，跳过重复购买和额外试用领取，继续订阅收尾",
            );
            continue_registration_after_payment_success_response(
                state, task_id, entry, now_unix, false,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register_existing_account_trial_response_inner(
    state: &mut ApiState,
    alias: &str,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    task_id: Option<&str>,
    now_unix: i64,
    initialize_task: bool,
) -> ApiResponse {
    let Some(task_id) = task_id else {
        return json_response(
            400,
            &ApiErrorBody {
                error: "existing account trial requires task_id".to_string(),
            },
        );
    };
    if initialize_task {
        if let Err(response) = reserve_registration_slot(state, task_id) {
            return response;
        }
        if let Err(response) =
            start_registration_task(state, Some(task_id), alias, trial_days, "已有账号开通试用")
        {
            release_registration_slot(state, task_id);
            return response;
        }
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
    let Some(record) = document.accounts.get(alias).cloned() else {
        let error = AccountSessionError::AccountAliasNotFound {
            alias: alias.to_string(),
        };
        fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
        return account_session_error_response(error);
    };
    let Some(email) = record
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        let message = format!("missing email for account alias: {alias}");
        fail_tracked_task(state, Some(task_id), message.clone());
        return json_response(400, &ApiErrorBody { error: message });
    };

    let cookies =
        match resolve_account_cookies_for_recovery(&record, alias, state.secret_backend.as_ref()) {
            Ok(cookies) => cookies,
            Err(error) => {
                if initialize_task && matches!(error, AccountSecretStoreError::Missing { .. }) {
                    return start_existing_account_trial_cookie_recovery_response(
                        state,
                        task_id,
                        alias,
                        trial_days,
                        auto_fetch_git_token,
                        card_selection_strategy,
                        card_bin,
                        now_unix,
                    );
                }
                let message = account_secret_store_error_message(&error);
                fail_tracked_task(state, Some(task_id), message.clone());
                return account_secret_store_error_response(error);
            }
        };
    let Some(session_cookie) = cookies
        .get(OVERLEAF_SESSION_COOKIE_NAME)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        if initialize_task {
            return start_existing_account_trial_cookie_recovery_response(
                state,
                task_id,
                alias,
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
                now_unix,
            );
        }
        let error = AccountSessionError::MissingCookies {
            alias: alias.to_string(),
        };
        fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
        return account_session_error_response(error);
    };
    let eligibility_client =
        OverleafSessionClient::new(ReqwestSessionTransport::new(cookies.clone()));
    let evidence = match block_on_api(inspect_account_trial_eligibility(
        &document,
        alias,
        &eligibility_client,
        trial_days,
        now_unix,
    )) {
        Ok(Ok(report)) => ExistingTrialEligibilityEvidence {
            eligibility: report.eligibility,
            plan_availability: report.plan_availability,
            subscription_state: subscription_state_from_key(&report.subscription_status),
            trial_expiry: report.trial_expiry,
        },
        Ok(Err(error)) => {
            if initialize_task && error.requires_cookie_recovery() {
                return start_existing_account_trial_cookie_recovery_response(
                    state,
                    task_id,
                    alias,
                    trial_days,
                    auto_fetch_git_token,
                    card_selection_strategy,
                    card_bin,
                    now_unix,
                );
            }
            fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
            return account_session_error_response(error);
        }
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    let preflight_continuation = match existing_trial_preflight(&evidence) {
        Ok(continuation) => continuation,
        Err(eligibility) => {
            return existing_account_trial_not_eligible_response(
                state,
                task_id,
                alias,
                eligibility,
            );
        }
    };
    let Some(local_chrome) = state.local_chrome_browser.clone() else {
        return browser_not_configured_response(state, Some(task_id));
    };
    let runtime = match build_api_runtime() {
        Ok(runtime) => runtime,
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    let browser_result = runtime.block_on(async {
        let session = local_chrome
            .start_registration_cdp_session_for_attempt(ChromeProxyAttempt::Primary)
            .await?;
        let result = session
            .automation()
            .login_with_session_cookie(
                BrowserAutoLoginInput {
                    overleaf_session: SecretText::new(session_cookie),
                    cookie_expiry: record.cookie_expiry,
                },
                Some(email.clone()),
            )
            .await;
        Ok::<_, BrowserAutomationError>((session, result))
    });
    let (session, login_result) = match browser_result {
        Ok((session, Ok(result))) => (session, result),
        Ok((session, Err(error))) => {
            let _ = runtime.block_on(session.cleanup());
            return handle_registration_error(
                state,
                Some(task_id),
                AccountRegistrationError::from(error),
            );
        }
        Err(error) => {
            return handle_registration_error(
                state,
                Some(task_id),
                AccountRegistrationError::from(error),
            );
        }
    };
    let authenticated_cookies = cookies_to_map(&login_result.cookies);
    if let Err(error) = runtime.block_on(state.session_identity_validator.validate(
        alias,
        Some(&email),
        &authenticated_cookies,
    )) {
        let _ = runtime.block_on(session.cleanup());
        if initialize_task && error.requires_cookie_recovery() {
            return start_existing_account_trial_cookie_recovery_response(
                state,
                task_id,
                alias,
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
                now_unix,
            );
        }
        fail_tracked_task(state, Some(task_id), account_session_error_message(&error));
        return account_session_error_response(error);
    }

    let continuation = match preflight_continuation {
        Some(continuation) => continuation,
        None => match inspect_rendered_existing_trial_continuation(
            state, task_id, alias, &runtime, &session, trial_days, now_unix,
        ) {
            Ok(continuation) => continuation,
            Err(ExistingTrialResolutionError::NotEligible(eligibility)) => {
                let _ = runtime.block_on(session.cleanup());
                return existing_account_trial_not_eligible_response(
                    state,
                    task_id,
                    alias,
                    eligibility,
                );
            }
            Err(ExistingTrialResolutionError::Browser(error)) => {
                let _ = runtime.block_on(session.cleanup());
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        },
    };

    let entry = RegistrationSessionEntry {
        runtime,
        session,
        context: RegistrationSessionContext {
            alias_hint: Some(alias.to_string()),
            existing_alias: Some(alias.to_string()),
            email,
            password: None,
            trial_days,
            auto_fetch_git_token,
            card_selection_strategy,
            card_bin,
        },
        last_activity: Instant::now(),
    };
    continue_existing_trial_response(state, task_id, entry, now_unix, continuation)
}

#[allow(clippy::too_many_arguments)]
fn start_existing_account_trial_cookie_recovery_response(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    now_unix: i64,
) -> ApiResponse {
    let plan = match account_cookie_recovery_plan(state, task_id, alias) {
        Ok(plan) => plan,
        Err(response) => return response,
    };
    let Some(local_chrome) = state.local_chrome_browser.clone() else {
        return browser_not_configured_response(state, Some(task_id));
    };
    let needs_password = plan.password.is_empty();
    if needs_password {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Warning,
            "账号 Cookie 已失效且没有可用本地密码，请在账号工作台重新输入",
        );
    } else {
        let _ = state.tasks.append_log(
            task_id,
            TaskLogLevel::Warning,
            "账号 Cookie 已失效，正在使用已保存密码恢复登录",
        );
    }
    queue_credential_refresh_browser_session_response(
        state,
        task_id,
        vec![plan],
        CredentialBrowserBatchCompletion::CredentialRefresh(
            CredentialRefreshContinuation::StartExistingTrial {
                alias: alias.to_string(),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
                now_unix,
            },
        ),
        local_chrome,
        now_unix,
    )
}

pub(super) fn existing_account_trial_not_eligible_response(
    state: &mut ApiState,
    task_id: &str,
    alias: &str,
    eligibility: TrialEligibility,
) -> ApiResponse {
    let message = match eligibility {
        TrialEligibility::ActiveTrial => format!("账号 {alias} 已处于试用中"),
        TrialEligibility::Ineligible => format!("账号 {alias} 没有可用试用资格"),
        TrialEligibility::Unknown => format!("无法确认账号 {alias} 的试用资格，已停止购买"),
        TrialEligibility::Eligible => unreachable!("eligible accounts pass the trial gate"),
    };
    fail_tracked_task(state, Some(task_id), message.clone());
    json_response(
        409,
        &ApiTypedErrorBody {
            error: message,
            kind: "trial_not_eligible",
        },
    )
}

fn reserve_registration_slot(state: &ApiState, task_id: &str) -> Result<(), ApiResponse> {
    let mut owner = state.registration_slot_owner.lock().map_err(|_| {
        json_response(
            500,
            &ApiErrorBody {
                error: "registration slot lock poisoned".to_string(),
            },
        )
    })?;
    if owner.is_some() {
        return Err(json_response(
            409,
            &ApiTypedErrorBody {
                error: "已有注册任务正在运行，请等待其完成或取消".to_string(),
                kind: "registration_busy",
            },
        ));
    }
    *owner = Some(task_id.to_string());
    Ok(())
}

pub(super) fn release_registration_slot(state: &ApiState, task_id: &str) {
    if let Ok(mut owner) = state.registration_slot_owner.lock() {
        if owner.as_deref() == Some(task_id) {
            *owner = None;
        }
    }
}

pub(super) fn cancel_task_with_registration_release(
    state: &mut ApiState,
    task_id: &str,
    message: impl Into<String>,
) -> Result<TaskSnapshot, TaskStateError> {
    let result = state.tasks.cancel_task(task_id, message);
    release_registration_slot(state, task_id);
    result
}

#[allow(clippy::too_many_arguments)]
fn register_account_with_local_chrome_session_response(
    state: &mut ApiState,
    task_id: &str,
    alias_hint: Option<String>,
    email: String,
    password: String,
    trial_days: u32,
    auto_fetch_git_token: bool,
    card_selection_strategy: CardSelectionStrategy,
    card_bin: Option<String>,
    local_chrome: Arc<LocalChromeBrowserAutomation>,
    now_unix: i64,
) -> ApiResponse {
    let password = SecretText::new(password);
    let runtime = match build_api_runtime() {
        Ok(runtime) => runtime,
        Err(response) => {
            fail_tracked_task(state, Some(task_id), response.body.clone());
            return response;
        }
    };
    let mut proxy_attempt = ChromeProxyAttempt::Primary;
    let (session, result) = loop {
        let browser_result = runtime.block_on(async {
            let session = local_chrome
                .start_registration_cdp_session_for_attempt(proxy_attempt)
                .await?;
            let result = session
                .automation()
                .register_and_subscribe(RegistrationInput {
                    alias_hint: alias_hint.clone(),
                    email: email.clone(),
                    password: password.clone(),
                    trial_days,
                    auto_fetch_git_token,
                })
                .await;
            Ok::<_, BrowserAutomationError>((session, result))
        });

        if let Ok((session, Err(BrowserAutomationError::RobotVerificationBlocked))) =
            &browser_result
        {
            if let Ok(page) = runtime.block_on(session.automation().current_page_state()) {
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    format!(
                        "验证诊断：页面拒绝验证，验证码框 {} 个，可见挑战 {} 个；{}",
                        page.recaptcha_iframe_count,
                        page.recaptcha_challenge_count,
                        page.visible_error_messages.join("；")
                    ),
                );
            }
        }
        match browser_result {
            Ok((session, Err(BrowserAutomationError::RobotVerificationBlocked)))
                if proxy_attempt == ChromeProxyAttempt::Primary
                    && state.config.browser_proxy_policy.has_fallback() =>
            {
                let _ = runtime.block_on(session.cleanup());
                proxy_attempt = ChromeProxyAttempt::Fallback;
                let _ = state.tasks.append_log(
                    task_id,
                    TaskLogLevel::Warning,
                    "机器人验证未通过，自动切换系统代理重试一次",
                );
            }
            Ok(value) => break value,
            Err(error) => {
                return handle_registration_error(
                    state,
                    Some(task_id),
                    AccountRegistrationError::from(error),
                );
            }
        }
    };

    match result {
        Ok(result) => {
            let entry = RegistrationSessionEntry {
                runtime,
                session,
                context: RegistrationSessionContext {
                    alias_hint,
                    existing_alias: None,
                    email,
                    password: Some(password),
                    trial_days,
                    auto_fetch_git_token,
                    card_selection_strategy,
                    card_bin,
                },
                last_activity: Instant::now(),
            };
            handle_registration_session_result(state, task_id, entry, Ok(result), now_unix)
        }
        Err(BrowserAutomationError::EmailCodeRequired { email }) => {
            let context = RegistrationSessionContext {
                alias_hint,
                existing_alias: None,
                email: email.clone(),
                password: Some(password),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
            };
            registration_waiting_for_email_code_response(
                state,
                task_id,
                email,
                RegistrationSessionEntry {
                    runtime,
                    session,
                    context,
                    last_activity: Instant::now(),
                },
            )
        }
        Err(BrowserAutomationError::RegisteredEmail { email }) => {
            let context = RegistrationSessionContext {
                alias_hint,
                existing_alias: None,
                email: email.clone(),
                password: Some(password),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
            };
            registration_waiting_for_new_credentials_response(
                state,
                task_id,
                email,
                RegistrationSessionEntry {
                    runtime,
                    session,
                    context,
                    last_activity: Instant::now(),
                },
            )
        }
        Err(BrowserAutomationError::ChallengeRequired { .. }) => {
            let context = RegistrationSessionContext {
                alias_hint,
                existing_alias: None,
                email,
                password: Some(password),
                trial_days,
                auto_fetch_git_token,
                card_selection_strategy,
                card_bin,
            };
            registration_waiting_for_captcha_response(
                state,
                task_id,
                RegistrationSessionEntry {
                    runtime,
                    session,
                    context,
                    last_activity: Instant::now(),
                },
            )
        }
        Err(error) => {
            let _ = runtime.block_on(session.cleanup());
            handle_registration_error(state, Some(task_id), AccountRegistrationError::from(error))
        }
    }
}
