use super::{ApiErrorBody, ApiResponse, ApiTypedErrorBody};
use crate::task_state::sanitize_task_text;
use crate::{
    AccountActionError, AccountCredentialError, AccountExportReport, AccountGitTokenError,
    AccountIoError, AccountRegistrationError, AccountSecretError, AccountSecretStoreError,
    AccountSessionError, AccountSwitchError, AccountSwitchExecutionError, AddressActionError,
    BrowserAutoLoginError, BrowserProfileAccountError, CardActionError,
    OverleafPasswordChangeError, ProjectMigrationError, TaskStateError,
};
use serde::Serialize;
use std::io;

pub(super) fn parse_json_request<T: serde::de::DeserializeOwned>(
    body: &str,
) -> Result<T, ApiResponse> {
    serde_json::from_str(body).map_err(|error| {
        json_response(
            400,
            &ApiErrorBody {
                error: format!("invalid json body: {error}"),
            },
        )
    })
}

pub(super) fn account_secret_error_response(error: AccountSecretError) -> ApiResponse {
    let status_code = match error {
        AccountSecretError::AliasNotFound { .. } | AccountSecretError::MissingSecret { .. } => 404,
        AccountSecretError::SecretReadFailed { .. } => 500,
    };
    let message = match error {
        AccountSecretError::AliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountSecretError::MissingSecret { alias, category } => {
            format!("{category} not available for account alias: {alias}")
        }
        AccountSecretError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
    };

    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_secret_store_error_response(error: AccountSecretStoreError) -> ApiResponse {
    let status_code = match &error {
        AccountSecretStoreError::Missing { .. } => 400,
        AccountSecretStoreError::ReadFailed { .. }
        | AccountSecretStoreError::WriteFailed { .. } => 500,
    };
    let message = account_secret_store_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_secret_store_error_message(error: &AccountSecretStoreError) -> String {
    match error {
        AccountSecretStoreError::Missing { alias, category } => {
            format!("missing stored {category} secret for account alias: {alias}")
        }
        AccountSecretStoreError::ReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
        AccountSecretStoreError::WriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn browser_auto_login_error_response(error: BrowserAutoLoginError) -> ApiResponse {
    let status_code = match error {
        BrowserAutoLoginError::Io { .. } => 500,
        BrowserAutoLoginError::SecretReadFailed { .. } => 500,
        BrowserAutoLoginError::AccountAliasNotFound { .. }
        | BrowserAutoLoginError::MissingCookie { .. }
        | BrowserAutoLoginError::MissingPassword { .. }
        | BrowserAutoLoginError::MissingEmail { .. } => 404,
        BrowserAutoLoginError::InvalidCookie { .. } | BrowserAutoLoginError::Browser { .. } => 503,
    };
    let message = browser_auto_login_error_message(&error);

    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn browser_auto_login_error_message(error: &BrowserAutoLoginError) -> String {
    match error {
        BrowserAutoLoginError::Io { message } | BrowserAutoLoginError::Browser { message } => {
            message.clone()
        }
        BrowserAutoLoginError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        BrowserAutoLoginError::MissingCookie { alias } => {
            format!("account alias missing overleaf_session2 cookie: {alias}")
        }
        BrowserAutoLoginError::InvalidCookie { alias } => {
            format!("saved Cookie is no longer authenticated for account alias: {alias}")
        }
        BrowserAutoLoginError::MissingPassword { alias } => {
            format!("account alias has no saved password for browser login: {alias}")
        }
        BrowserAutoLoginError::MissingEmail { alias } => {
            format!("account alias has no saved email for browser login: {alias}")
        }
        BrowserAutoLoginError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn browser_profile_account_error_response(
    error: BrowserProfileAccountError,
) -> ApiResponse {
    let status_code = match &error {
        BrowserProfileAccountError::Io { .. } => 500,
        BrowserProfileAccountError::ExtensionBridgeUnavailable => 503,
        BrowserProfileAccountError::ExtensionBridge { .. }
        | BrowserProfileAccountError::ExtensionCommandFailed { .. } => 503,
        BrowserProfileAccountError::MissingSessionCookie => 404,
        BrowserProfileAccountError::Session { .. } => 503,
    };
    json_response(
        status_code,
        &ApiErrorBody {
            error: browser_profile_account_error_message(&error),
        },
    )
}

pub(super) fn browser_profile_account_error_message(error: &BrowserProfileAccountError) -> String {
    match error {
        BrowserProfileAccountError::Io { message } => message.clone(),
        BrowserProfileAccountError::ExtensionBridgeUnavailable => {
            "extension bridge executor is not configured".to_string()
        }
        BrowserProfileAccountError::ExtensionBridge { message } => message.clone(),
        BrowserProfileAccountError::ExtensionCommandFailed {
            request_id,
            message,
        } => format!("extension get_cookie command failed for {request_id}: {message}"),
        BrowserProfileAccountError::MissingSessionCookie => {
            "current browser profile is not logged in to Overleaf".to_string()
        }
        BrowserProfileAccountError::Session { message } => {
            format!("failed to verify current browser account: {message}")
        }
    }
}

pub(super) fn account_action_error_response(error: AccountActionError) -> ApiResponse {
    let status_code = match error {
        AccountActionError::Io { .. } => 500,
        AccountActionError::SecretWriteFailed { .. } => 500,
        AccountActionError::AccountAliasNotFound { .. } => 404,
        _ => 400,
    };
    let message = account_action_error_message(&error);

    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_action_error_message(error: &AccountActionError) -> String {
    match error {
        AccountActionError::Io { message } => message.clone(),
        AccountActionError::MissingEmail => "missing email".to_string(),
        AccountActionError::MissingPassword => "missing password".to_string(),
        AccountActionError::EmptyValue { field } => format!("empty value: {field}"),
        AccountActionError::AliasCountMismatch { aliases, emails } => {
            format!("alias count mismatch: {aliases} aliases for {emails} emails")
        }
        AccountActionError::PasswordCountMismatch {
            passwords,
            accounts,
        } => {
            format!("password count mismatch: {passwords} passwords for {accounts} accounts")
        }
        AccountActionError::PasswordTooShort { minimum } => {
            format!("新密码至少需要 {minimum} 位")
        }
        AccountActionError::MultiplePasswordsForSingleAccount { passwords } => {
            format!("multiple passwords for one account: {passwords}")
        }
        AccountActionError::AliasConflict { alias } => {
            format!("account alias already exists: {alias}")
        }
        AccountActionError::AliasRepeatedInInput { alias } => {
            format!("account alias repeated in input: {alias}")
        }
        AccountActionError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountActionError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn account_switch_error_response(error: AccountSwitchError) -> ApiResponse {
    let status_code = match error {
        AccountSwitchError::Io { .. } => 500,
        AccountSwitchError::AccountAliasNotFound { .. } => 404,
        AccountSwitchError::MissingSessionCookie { .. } => 400,
        AccountSwitchError::SecretReadFailed { .. } => 500,
    };
    let message = account_switch_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_switch_error_message(error: &AccountSwitchError) -> String {
    match error {
        AccountSwitchError::Io { message } => message.clone(),
        AccountSwitchError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountSwitchError::MissingSessionCookie { alias } => {
            format!("missing overleaf session cookie for account alias: {alias}")
        }
        AccountSwitchError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn account_switch_execution_error_response(
    error: AccountSwitchExecutionError,
) -> ApiResponse {
    let error = match error {
        AccountSwitchExecutionError::Session(error) => {
            return account_session_error_response(error);
        }
        error => error,
    };
    let status_code = match &error {
        AccountSwitchExecutionError::Plan(AccountSwitchError::Io { .. }) => 500,
        AccountSwitchExecutionError::Plan(AccountSwitchError::AccountAliasNotFound { .. }) => 404,
        AccountSwitchExecutionError::Plan(AccountSwitchError::MissingSessionCookie { .. }) => 400,
        AccountSwitchExecutionError::Plan(AccountSwitchError::SecretReadFailed { .. }) => 500,
        AccountSwitchExecutionError::ProjectMigrationNotConfigured => 503,
        AccountSwitchExecutionError::ProjectMigration(ProjectMigrationError::Cancelled) => 409,
        AccountSwitchExecutionError::ProjectMigration { .. } => 503,
        AccountSwitchExecutionError::Bridge { .. }
        | AccountSwitchExecutionError::CommandFailed { .. } => 503,
        AccountSwitchExecutionError::Session(_) => unreachable!(),
    };
    let message = account_switch_execution_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_switch_execution_error_message(
    error: &AccountSwitchExecutionError,
) -> String {
    match error {
        AccountSwitchExecutionError::Plan(error) => account_switch_error_message(error),
        AccountSwitchExecutionError::Session(error) => account_session_error_message(error),
        AccountSwitchExecutionError::Bridge { message } => message.clone(),
        AccountSwitchExecutionError::ProjectMigrationNotConfigured => {
            "project migration executor is not configured".to_string()
        }
        AccountSwitchExecutionError::ProjectMigration(error) => {
            format!("project migration failed: {error:?}")
        }
        AccountSwitchExecutionError::CommandFailed {
            action,
            request_id,
            message,
        } => format!("extension command {action} failed for {request_id}: {message}"),
    }
}

pub(super) fn project_migration_error_response(error: ProjectMigrationError) -> ApiResponse {
    let status_code = match &error {
        ProjectMigrationError::Io { .. } => 500,
        ProjectMigrationError::NoCurrentAccount
        | ProjectMigrationError::SourceEqualsTarget { .. }
        | ProjectMigrationError::MissingCookies { .. } => 400,
        ProjectMigrationError::SecretReadFailed { .. } => 500,
        ProjectMigrationError::AccountAliasNotFound { .. } => 404,
        ProjectMigrationError::CsrfTokenFetchFailed { .. }
        | ProjectMigrationError::ProjectListFailed { .. }
        | ProjectMigrationError::AuditIncomplete { .. } => 503,
        ProjectMigrationError::Cancelled => 409,
    };
    json_response(
        status_code,
        &ApiErrorBody {
            error: project_migration_error_message(&error),
        },
    )
}

pub(super) fn project_migration_error_message(error: &ProjectMigrationError) -> String {
    match error {
        ProjectMigrationError::Io { message } => message.clone(),
        ProjectMigrationError::NoCurrentAccount => "no current account selected".to_string(),
        ProjectMigrationError::SourceEqualsTarget { alias } => {
            format!("source account is already target alias: {alias}")
        }
        ProjectMigrationError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        ProjectMigrationError::MissingCookies { alias } => {
            format!("missing overleaf session cookie for account alias: {alias}")
        }
        ProjectMigrationError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
        ProjectMigrationError::CsrfTokenFetchFailed { alias, message } => {
            format!("failed to fetch csrf token for account alias {alias}: {message}")
        }
        ProjectMigrationError::ProjectListFailed { alias, message } => {
            format!("failed to list projects for account alias {alias}: {message}")
        }
        ProjectMigrationError::AuditIncomplete {
            failed_count,
            missing_count,
        } => format!(
            "project migration audit incomplete: {failed_count} failed, {missing_count} missing"
        ),
        ProjectMigrationError::Cancelled => "project migration cancelled".to_string(),
    }
}

pub(super) fn account_credential_error_response(error: AccountCredentialError) -> ApiResponse {
    let status_code = match &error {
        AccountCredentialError::Io { .. }
        | AccountCredentialError::SessionIdentity { .. }
        | AccountCredentialError::SecretWriteFailed { .. } => 500,
        AccountCredentialError::Browser { .. }
        | AccountCredentialError::RobotVerificationBlocked => 503,
        _ => 400,
    };
    let message = account_credential_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_credential_error_message(error: &AccountCredentialError) -> String {
    match error {
        AccountCredentialError::Io { message } => message.clone(),
        AccountCredentialError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountCredentialError::MissingEmail { alias } => {
            format!("missing email for account alias: {alias}")
        }
        AccountCredentialError::AliasConflict { alias } => {
            format!("account alias already exists: {alias}")
        }
        AccountCredentialError::MissingSessionCookie => {
            "missing overleaf session cookie after login".to_string()
        }
        AccountCredentialError::InvalidCredentials => "邮箱或密码错误".to_string(),
        AccountCredentialError::ChallengeRequired => "需要完成人机验证".to_string(),
        AccountCredentialError::RobotVerificationBlocked => "机器人验证未通过".to_string(),
        AccountCredentialError::Browser { message } => message.clone(),
        AccountCredentialError::SessionIdentity { message } => message.clone(),
        AccountCredentialError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn account_registration_error_response(error: AccountRegistrationError) -> ApiResponse {
    let status_code = match error {
        AccountRegistrationError::Io { .. }
        | AccountRegistrationError::SecretWriteFailed { .. } => 500,
        AccountRegistrationError::RegisteredEmail { .. }
        | AccountRegistrationError::EmailCodeRequired { .. }
        | AccountRegistrationError::ChallengeRequired { .. } => 400,
        AccountRegistrationError::AccountAliasNotFound { .. } => 404,
        AccountRegistrationError::Browser { .. }
        | AccountRegistrationError::IncompleteArtifacts { .. }
        | AccountRegistrationError::SessionIdentity { .. } => 503,
        _ => 400,
    };
    let message = account_registration_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_registration_error_message(error: &AccountRegistrationError) -> String {
    match error {
        AccountRegistrationError::Io { message } => message.clone(),
        AccountRegistrationError::MissingEmail => "missing email".to_string(),
        AccountRegistrationError::MissingPassword => "missing password".to_string(),
        AccountRegistrationError::PasswordTooShort { minimum } => {
            format!("新密码至少需要 {minimum} 位")
        }
        AccountRegistrationError::InvalidTrialDays { trial_days } => {
            format!("trial_days must be 7 or 21, got {trial_days}")
        }
        AccountRegistrationError::AliasConflict { alias } => {
            format!("account alias already exists: {alias}")
        }
        AccountRegistrationError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountRegistrationError::RegisteredEmail { email } => {
            format!("email already registered: {email}")
        }
        AccountRegistrationError::EmailCodeRequired { email } => {
            format!("email verification code required: {email}")
        }
        AccountRegistrationError::ChallengeRequired { challenge } => {
            format!("manual challenge required: {challenge}")
        }
        AccountRegistrationError::SubscriptionMissing { alias } => {
            format!("registration did not reach subscription success for account alias: {alias}")
        }
        AccountRegistrationError::IncompleteArtifacts { alias, missing } => format!(
            "registration finalization is incomplete for account alias {alias}: missing {}",
            missing.join(", ")
        ),
        AccountRegistrationError::SessionIdentity { message } => message.clone(),
        AccountRegistrationError::Browser { message } => message.clone(),
        AccountRegistrationError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn account_io_error_response(error: AccountIoError) -> ApiResponse {
    let status_code = match error {
        AccountIoError::Io { .. }
        | AccountIoError::SecretReadFailed { .. }
        | AccountIoError::SecretWriteFailed { .. } => 500,
        AccountIoError::AliasNotFound { .. } => 404,
        _ => 400,
    };
    let message = account_io_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn redact_export_report_paths(mut report: AccountExportReport) -> AccountExportReport {
    for file in &mut report.files {
        file.path = sanitize_task_text(file.path.clone());
    }
    report
}

pub(super) fn account_io_error_message(error: &AccountIoError) -> String {
    match error {
        AccountIoError::Io { message } | AccountIoError::Json { message } => {
            sanitize_task_text(message.clone())
        }
        AccountIoError::UnsupportedImportShape => "unsupported import shape".to_string(),
        AccountIoError::NoAccountsSelected => "no accounts selected".to_string(),
        AccountIoError::EmptyOutputDirectory => "empty output directory".to_string(),
        AccountIoError::AliasNotFound { alias } => format!("account alias not found: {alias}"),
        AccountIoError::RefusesToOverwriteMainConfig { .. } => {
            "refuses to overwrite the main account configuration".to_string()
        }
        AccountIoError::RefusesToOverwriteExistingExport { .. } => {
            "export target already exists; enable overwrite to replace it".to_string()
        }
        AccountIoError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
        AccountIoError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn account_session_error_response(error: AccountSessionError) -> ApiResponse {
    if matches!(
        &error,
        AccountSessionError::InvalidSessionCookie { .. }
            | AccountSessionError::EmailMismatch { .. }
            | AccountSessionError::EmailBelongsToAnotherAlias { .. }
    ) {
        return json_response(
            400,
            &ApiTypedErrorBody {
                error: account_session_error_message(&error),
                kind: "invalid_session_cookie",
            },
        );
    }
    let status_code = match error {
        AccountSessionError::Io { .. } | AccountSessionError::Session { .. } => 500,
        AccountSessionError::AccountAliasNotFound { .. } => 404,
        _ => 400,
    };
    let message = account_session_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_session_error_message(error: &AccountSessionError) -> String {
    match error {
        AccountSessionError::Io { message } | AccountSessionError::Session { message } => {
            message.clone()
        }
        AccountSessionError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountSessionError::MissingCookies { alias } => {
            format!("missing cookies for account alias: {alias}")
        }
        AccountSessionError::InvalidSessionCookie { alias } => {
            format!(
                "Cookie 无效或未登录 Overleaf，请为账号 {alias} 重新粘贴 Cookie 或改用账号密码登录"
            )
        }
        AccountSessionError::EmailMismatch {
            alias,
            stored_email,
            fetched_email,
        } => format!(
            "account email mismatch for {alias}: stored {stored_email}, fetched {fetched_email}"
        ),
        AccountSessionError::EmailBelongsToAnotherAlias {
            alias,
            email,
            existing_alias,
        } => {
            format!("account {alias} cookie belongs to {email}, already used by {existing_alias}")
        }
    }
}

pub(super) fn overleaf_password_change_error_message(
    error: &OverleafPasswordChangeError,
) -> String {
    match error {
        OverleafPasswordChangeError::Io { message } => message.clone(),
        OverleafPasswordChangeError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        OverleafPasswordChangeError::MissingEmail { alias } => {
            format!("missing email for account alias: {alias}")
        }
        OverleafPasswordChangeError::MissingCurrentPassword { alias } => {
            format!("missing current password for account alias: {alias}")
        }
        OverleafPasswordChangeError::MissingNewPassword => "missing new password".to_string(),
        OverleafPasswordChangeError::PasswordTooShort { minimum } => {
            format!("新密码至少需要 {minimum} 位")
        }
        OverleafPasswordChangeError::MissingSessionCookie => {
            "missing overleaf session cookie after login".to_string()
        }
        OverleafPasswordChangeError::InvalidCredentials => "当前密码错误".to_string(),
        OverleafPasswordChangeError::ChallengeRequired => "需要完成人机验证".to_string(),
        OverleafPasswordChangeError::RobotVerificationBlocked => "机器人验证未通过".to_string(),
        OverleafPasswordChangeError::LoginEmailMismatch { expected, actual } => {
            format!("login email mismatch: expected {expected}, got {actual}")
        }
        OverleafPasswordChangeError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
        OverleafPasswordChangeError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
        OverleafPasswordChangeError::Browser { message } => message.clone(),
    }
}

pub(super) fn account_git_token_error_response(error: AccountGitTokenError) -> ApiResponse {
    let status_code = match error {
        AccountGitTokenError::Io { .. } | AccountGitTokenError::Session { .. } => 500,
        AccountGitTokenError::Browser { .. } => 503,
        AccountGitTokenError::AccountAliasNotFound { .. } => 404,
        AccountGitTokenError::MissingCookies { .. }
        | AccountGitTokenError::TokenNotAvailable { .. } => 400,
        AccountGitTokenError::SecretReadFailed { .. }
        | AccountGitTokenError::SecretWriteFailed { .. } => 500,
    };
    let message = account_git_token_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn account_git_token_error_message(error: &AccountGitTokenError) -> String {
    match error {
        AccountGitTokenError::Io { message }
        | AccountGitTokenError::Session { message }
        | AccountGitTokenError::Browser { message } => message.clone(),
        AccountGitTokenError::AccountAliasNotFound { alias } => {
            format!("account alias not found: {alias}")
        }
        AccountGitTokenError::MissingCookies { alias } => {
            format!("missing cookies for account alias: {alias}")
        }
        AccountGitTokenError::TokenNotAvailable { alias } => {
            format!("Git token was not available after browser generation: {alias}")
        }
        AccountGitTokenError::SecretReadFailed { alias, category } => {
            format!("failed to read stored {category} secret for account alias: {alias}")
        }
        AccountGitTokenError::SecretWriteFailed { alias, category } => {
            format!("failed to write stored {category} secret for account alias: {alias}")
        }
    }
}

pub(super) fn card_action_error_response(error: CardActionError) -> ApiResponse {
    let status_code = match error {
        CardActionError::Io { .. } => 500,
        CardActionError::SecretReadFailed { .. } | CardActionError::SecretWriteFailed { .. } => 500,
        CardActionError::CardNotFound { .. } => 404,
        _ => 400,
    };
    let message = card_action_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn card_action_error_message(error: &CardActionError) -> String {
    match error {
        CardActionError::Io { message } => message.clone(),
        CardActionError::InvalidFormat { message } => message.clone(),
        CardActionError::CardNotFound { number_or_suffix } => {
            format!("card not found: {number_or_suffix}")
        }
        CardActionError::AmbiguousCardSuffix { suffix, matches } => {
            format!("ambiguous card suffix: {suffix} matched {matches} cards")
        }
        CardActionError::SecretReadFailed { field } => {
            format!("failed to read card secret field: {field}")
        }
        CardActionError::SecretWriteFailed { field } => {
            format!("failed to write card secret field: {field}")
        }
    }
}

pub(super) fn address_action_error_response(error: AddressActionError) -> ApiResponse {
    let status_code = match error {
        AddressActionError::Io { .. } => 500,
        AddressActionError::Network { .. } => 503,
        AddressActionError::AddressIndexOutOfRange { .. } => 404,
        _ => 400,
    };
    let message = address_action_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn address_action_error_message(error: &AddressActionError) -> String {
    match error {
        AddressActionError::Io { message } => message.clone(),
        AddressActionError::Json { message } => message.clone(),
        AddressActionError::Network { message } => message.clone(),
        AddressActionError::InvalidApiResponse => "invalid address api response".to_string(),
        AddressActionError::MissingRequiredFields => "missing required address fields".to_string(),
        AddressActionError::EmptyAddressBook => "empty address book".to_string(),
        AddressActionError::AddressIndexOutOfRange { index, len } => {
            format!("address index out of range: {index} for {len} addresses")
        }
        AddressActionError::AddressUnavailable {
            api_message,
            fallback_message,
        } => format!("address API failed: {api_message}; fallback failed: {fallback_message}"),
    }
}

pub(super) fn task_state_error_response(error: TaskStateError) -> ApiResponse {
    let status_code = match error {
        TaskStateError::TaskNotFound { .. } => 404,
        TaskStateError::InvalidUserInput { .. } => 400,
        _ => 400,
    };
    let message = task_state_error_message(&error);
    json_response(status_code, &ApiErrorBody { error: message })
}

pub(super) fn task_state_error_message(error: &TaskStateError) -> String {
    match error {
        TaskStateError::TaskAlreadyExists { task_id } => format!("task already exists: {task_id}"),
        TaskStateError::TaskNotFound { task_id } => format!("task not found: {task_id}"),
        TaskStateError::InvalidProgress { current, total } => {
            format!("invalid task progress: {current}/{total}")
        }
        TaskStateError::InvalidUserInput { message } => message.clone(),
        TaskStateError::AccountAlreadyLocked { alias, task_id } => {
            format!("account alias locked by task {task_id}: {alias}")
        }
        TaskStateError::TerminalTask { task_id, phase } => {
            format!("task already terminal: {task_id} ({phase:?})")
        }
    }
}

pub(super) fn method_not_allowed_response() -> ApiResponse {
    json_response(
        405,
        &ApiErrorBody {
            error: "method not allowed".to_string(),
        },
    )
}

pub(super) fn io_error_response(error: io::Error) -> ApiResponse {
    json_response(
        500,
        &ApiErrorBody {
            error: error.to_string(),
        },
    )
}

pub(super) fn json_response<T: Serialize>(status_code: u16, value: &T) -> ApiResponse {
    let body = serde_json::to_string(value).unwrap_or_else(|error| {
        format!(
            r#"{{"error":"failed to serialize response: {}"}}"#,
            escape_json_string(&error.to_string())
        )
    });
    ApiResponse {
        status_code,
        content_type: "application/json; charset=utf-8",
        body,
    }
}

pub(super) fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
