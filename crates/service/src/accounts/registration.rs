use overleaf_browser::{
    cookies_to_map, BrowserAutomation, BrowserAutomationError, RegistrationInput,
    RegistrationResult, SecretText,
};
use overleaf_core::{make_alias, normalize_email, registration_trial_plan_code};
use overleaf_storage::{AccountRecord, AccountStore, SecretBackend};
use overleaf_workflows::{
    decide_registration_completion, RegistrationArtifact, RegistrationCompletionInput,
};
use serde::Serialize;

use crate::account_secrets::{
    write_account_cookie_secrets_tracked, write_account_git_token_secret_tracked,
    write_account_password_secret_tracked, AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::password_policy::validate_new_password;

const OVERLEAF_SESSION_COOKIE_NAME: &str = "overleaf_session2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountRegistrationReport {
    pub alias: String,
    pub email: String,
    pub trial_days: u32,
    pub status: AccountRegistrationStatus,
    pub existing_alias: Option<String>,
    pub subscription_succeeded: bool,
    pub cookie_saved: bool,
    pub git_token_saved: bool,
    pub trial_expiry: Option<i64>,
    pub git_token_expiry: Option<i64>,
    pub missing_recoverable_artifacts: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRegistrationStatus {
    Imported,
    UpdatedExisting,
    SkippedDuplicateEmail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRegistrationError {
    Io {
        message: String,
    },
    MissingEmail,
    MissingPassword,
    PasswordTooShort {
        minimum: usize,
    },
    InvalidTrialDays {
        trial_days: u32,
    },
    AliasConflict {
        alias: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    RegisteredEmail {
        email: String,
    },
    EmailCodeRequired {
        email: String,
    },
    ChallengeRequired {
        challenge: String,
    },
    SubscriptionMissing {
        alias: String,
    },
    IncompleteArtifacts {
        alias: String,
        missing: Vec<&'static str>,
    },
    SessionIdentity {
        message: String,
    },
    Browser {
        message: String,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
}

#[derive(Debug, Clone)]
pub struct AccountRegistrationInput<'a> {
    pub alias_hint: Option<&'a str>,
    pub email: &'a str,
    pub password: SecretText,
    pub trial_days: u32,
    pub auto_fetch_git_token: bool,
    pub now_unix: i64,
}

pub async fn register_account_with_subscription_in_store_with_backend(
    store: &AccountStore,
    input: AccountRegistrationInput<'_>,
    browser: &(dyn BrowserAutomation + Send + Sync),
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    let AccountRegistrationInput {
        alias_hint,
        email,
        password,
        trial_days,
        auto_fetch_git_token,
        now_unix,
    } = input;
    let email = email.trim();
    let alias_hint = alias_hint.map(str::trim).filter(|value| !value.is_empty());
    validate_registration_input(email, password.expose(), trial_days)?;

    let document = store.load().map_err(|error| AccountRegistrationError::Io {
        message: error.to_string(),
    })?;
    let alias = make_alias(alias_hint, email);
    if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
        return Ok(AccountRegistrationReport::skipped_duplicate_email(
            alias,
            email.to_string(),
            existing_alias.to_string(),
            trial_days,
        ));
    }
    if document.alias_conflicts_with_email(&alias, email) {
        return Err(AccountRegistrationError::AliasConflict { alias });
    }

    let result = browser
        .register_and_subscribe(RegistrationInput {
            alias_hint: alias_hint.map(ToOwned::to_owned),
            email: email.to_string(),
            password: password.clone(),
            trial_days,
            auto_fetch_git_token,
        })
        .await
        .map_err(AccountRegistrationError::from_browser)?;
    ensure_registration_result_artifacts(&alias, &result, auto_fetch_git_token)?;

    save_registration_result_in_store_with_backend(
        store,
        alias_hint,
        password,
        result,
        auto_fetch_git_token,
        now_unix,
        backend,
    )
}

pub fn ensure_registration_result_artifacts(
    alias: &str,
    result: &RegistrationResult,
    require_git_token: bool,
) -> Result<(), AccountRegistrationError> {
    if !result.subscription_succeeded {
        return Err(AccountRegistrationError::SubscriptionMissing {
            alias: alias.to_string(),
        });
    }

    let mut missing = Vec::new();
    if !result.cookies.iter().any(|cookie| {
        cookie.name == OVERLEAF_SESSION_COOKIE_NAME && !cookie.value.expose().trim().is_empty()
    }) {
        missing.push("cookie");
    }
    if require_git_token
        && result
            .git_token
            .as_ref()
            .and_then(|token| token.token.as_ref())
            .is_none_or(|token| token.expose().trim().is_empty())
    {
        missing.push("git_token");
    }
    if result.trial_expiry.is_none() {
        missing.push("trial_expiry");
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(AccountRegistrationError::IncompleteArtifacts {
            alias: alias.to_string(),
            missing,
        })
    }
}

pub fn save_registration_result_in_store_with_backend(
    store: &AccountStore,
    alias_hint: Option<&str>,
    password: SecretText,
    result: RegistrationResult,
    require_git_token: bool,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    save_registration_result_in_store_internal(
        store,
        alias_hint,
        Some(password),
        result,
        require_git_token,
        now_unix,
        backend,
    )
}

pub fn save_registration_result_without_password_in_store_with_backend(
    store: &AccountStore,
    alias_hint: Option<&str>,
    result: RegistrationResult,
    require_git_token: bool,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    save_registration_result_in_store_internal(
        store,
        alias_hint,
        None,
        result,
        require_git_token,
        now_unix,
        backend,
    )
}

pub fn save_existing_trial_result_in_store_with_backend(
    store: &AccountStore,
    alias: &str,
    result: RegistrationResult,
    require_git_token: bool,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    let alias = alias.trim();
    let mut document = store.load().map_err(|error| AccountRegistrationError::Io {
        message: error.to_string(),
    })?;
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        AccountRegistrationError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let result_email = result.email.trim();
    if let Some(stored_email) = record.email.as_deref() {
        if normalize_email(stored_email) != normalize_email(result_email) {
            return Err(AccountRegistrationError::SessionIdentity {
                message: format!(
                    "registration result email mismatch for {alias}: stored {stored_email}, fetched {result_email}"
                ),
            });
        }
    }
    ensure_registration_result_artifacts(alias, &result, require_git_token)?;

    let cookie_saved = result
        .cookies
        .iter()
        .any(|cookie| cookie.name == OVERLEAF_SESSION_COOKIE_NAME && !cookie.value.is_empty());
    let git_token_saved = result
        .git_token
        .as_ref()
        .and_then(|token| token.token.as_ref())
        .is_some_and(|token| !token.is_empty());
    let decision = decide_registration_completion(RegistrationCompletionInput {
        subscription_succeeded: result.subscription_succeeded,
        cookie_captured: cookie_saved,
        git_token_captured: git_token_saved,
        trial_expiry_captured: result.trial_expiry.is_some(),
        git_token_required: require_git_token,
    });
    if !decision.can_save_account {
        return Err(AccountRegistrationError::SubscriptionMissing {
            alias: alias.to_string(),
        });
    }

    let cookies = cookies_to_map(&result.cookies);
    let cookie_expiry = result
        .cookies
        .iter()
        .find(|cookie| cookie.name == OVERLEAF_SESSION_COOKIE_NAME)
        .and_then(|cookie| cookie.expiration_date)
        .map(|value| value as f64);
    let git_token = result
        .git_token
        .as_ref()
        .and_then(|token| token.token.as_ref())
        .map(|token| token.expose().to_string());
    let git_token_expiry = result.git_token.as_ref().and_then(|token| token.expires_at);

    let mut journal = AccountSecretWriteJournal::default();
    if let Err(error) =
        write_account_cookie_secrets_tracked(record, alias, &cookies, backend, &mut journal)
    {
        return Err(rollback_registration_secret_error(journal, backend, error));
    }
    if let Some(token) = git_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if let Err(error) =
            write_account_git_token_secret_tracked(record, alias, token, backend, &mut journal)
        {
            return Err(rollback_registration_secret_error(journal, backend, error));
        }
        record.git_token_expiry = git_token_expiry.map(|value| value as f64);
    }
    record.email = Some(result.email.clone());
    record.cookie_expiry = cookie_expiry;
    record.cookie_updated_at = Some(now_unix as f64);
    record.last_login_at = Some(now_unix as f64);
    record.trial_started_at = Some(now_unix as f64);
    record.set_trial_expiry(result.trial_expiry.map(|value| value as f64));
    record.subscription_status = Some("pro".to_string());
    record.subscription_label = Some("Pro annual".into());
    record.subscription_checked_at = Some(now_unix as f64);
    record.password = None;
    record.git_token = None;
    let actual_trial_days = record.trial_days;

    save_registration_document_with_rollback(store, &document, journal, backend)?;

    Ok(AccountRegistrationReport {
        alias: alias.to_string(),
        email: result.email,
        trial_days: actual_trial_days.unwrap_or(result.trial_days),
        status: AccountRegistrationStatus::UpdatedExisting,
        existing_alias: Some(alias.to_string()),
        subscription_succeeded: result.subscription_succeeded,
        cookie_saved,
        git_token_saved,
        trial_expiry: result.trial_expiry,
        git_token_expiry,
        missing_recoverable_artifacts: decision
            .missing_recoverable_artifacts
            .into_iter()
            .map(registration_artifact_name)
            .collect(),
    })
}

fn save_registration_result_in_store_internal(
    store: &AccountStore,
    alias_hint: Option<&str>,
    password: Option<SecretText>,
    result: RegistrationResult,
    require_git_token: bool,
    now_unix: i64,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountRegistrationReport, AccountRegistrationError> {
    let email = result.email.trim();
    let alias_hint = alias_hint.map(str::trim).filter(|value| !value.is_empty());
    let mut document = store.load().map_err(|error| AccountRegistrationError::Io {
        message: error.to_string(),
    })?;
    let alias = make_alias(alias_hint, email);
    if let Some(existing_alias) = document.duplicate_alias_by_email(email) {
        return Ok(AccountRegistrationReport::skipped_duplicate_email(
            alias,
            email.to_string(),
            existing_alias.to_string(),
            result.trial_days,
        ));
    }
    if document.alias_conflicts_with_email(&alias, email) {
        return Err(AccountRegistrationError::AliasConflict { alias });
    }
    ensure_registration_result_artifacts(&alias, &result, require_git_token)?;

    let cookie_saved = result
        .cookies
        .iter()
        .any(|cookie| cookie.name == OVERLEAF_SESSION_COOKIE_NAME && !cookie.value.is_empty());
    let git_token_saved = result
        .git_token
        .as_ref()
        .and_then(|token| token.token.as_ref())
        .map(|token| !token.is_empty())
        .unwrap_or(false);
    let decision = decide_registration_completion(RegistrationCompletionInput {
        subscription_succeeded: result.subscription_succeeded,
        cookie_captured: cookie_saved,
        git_token_captured: git_token_saved,
        trial_expiry_captured: result.trial_expiry.is_some(),
        git_token_required: require_git_token,
    });

    if !decision.can_save_account {
        return Err(AccountRegistrationError::SubscriptionMissing {
            alias: alias.clone(),
        });
    }

    let cookies = cookies_to_map(&result.cookies);
    let cookie_expiry = result
        .cookies
        .iter()
        .find(|cookie| cookie.name == OVERLEAF_SESSION_COOKIE_NAME)
        .and_then(|cookie| cookie.expiration_date)
        .map(|value| value as f64);
    let git_token = result
        .git_token
        .as_ref()
        .and_then(|token| token.token.as_ref())
        .map(|token| token.expose().to_string());
    let git_token_expiry = result.git_token.as_ref().and_then(|token| token.expires_at);

    let mut record = AccountRecord {
        cookie_refs: Default::default(),
        password_ref: None,
        git_token_ref: None,
        cookies: Default::default(),
        user_id: None,
        email: Some(result.email.clone()),
        password: None,
        cookie_expiry,
        cookie_updated_at: Some(now_unix as f64),
        created_at: Some(now_unix as f64),
        last_login_at: Some(now_unix as f64),
        trial_days: None,
        trial_started_at: Some(now_unix as f64),
        trial_expiry: result.trial_expiry.map(|value| value as f64),
        subscription_status: Some("pro".to_string()),
        subscription_label: Some("Pro annual".into()),
        subscription_checked_at: Some(now_unix as f64),
        git_token: None,
        git_token_expiry: git_token_expiry.map(|value| value as f64),
    };
    record.set_trial_expiry(record.trial_expiry);
    let mut journal = AccountSecretWriteJournal::default();
    if let Some(password) = password {
        if let Err(error) = write_account_password_secret_tracked(
            &mut record,
            &alias,
            &password.into_inner(),
            backend,
            &mut journal,
        ) {
            return Err(rollback_registration_secret_error(journal, backend, error));
        }
    }
    if let Err(error) =
        write_account_cookie_secrets_tracked(&mut record, &alias, &cookies, backend, &mut journal)
    {
        return Err(rollback_registration_secret_error(journal, backend, error));
    }
    if let Some(token) = git_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if let Err(error) = write_account_git_token_secret_tracked(
            &mut record,
            &alias,
            token,
            backend,
            &mut journal,
        ) {
            return Err(rollback_registration_secret_error(journal, backend, error));
        }
    }
    let actual_trial_days = record.trial_days;
    document.accounts.insert(alias.clone(), record);
    save_registration_document_with_rollback(store, &document, journal, backend)?;

    Ok(AccountRegistrationReport {
        alias,
        email: result.email,
        trial_days: actual_trial_days.unwrap_or(result.trial_days),
        status: AccountRegistrationStatus::Imported,
        existing_alias: None,
        subscription_succeeded: result.subscription_succeeded,
        cookie_saved,
        git_token_saved,
        trial_expiry: result.trial_expiry,
        git_token_expiry,
        missing_recoverable_artifacts: decision
            .missing_recoverable_artifacts
            .into_iter()
            .map(registration_artifact_name)
            .collect(),
    })
}

fn save_registration_document_with_rollback(
    store: &AccountStore,
    document: &overleaf_storage::AccountsDocument,
    journal: AccountSecretWriteJournal,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<(), AccountRegistrationError> {
    match store.save(document) {
        Ok(()) => Ok(()),
        Err(error) => match journal.rollback(backend) {
            Ok(()) => Err(AccountRegistrationError::Io {
                message: error.to_string(),
            }),
            Err(rollback_error) => Err(AccountRegistrationError::from(rollback_error)),
        },
    }
}

fn rollback_registration_secret_error(
    journal: AccountSecretWriteJournal,
    backend: &(dyn SecretBackend + Send + Sync),
    error: AccountSecretStoreError,
) -> AccountRegistrationError {
    match journal.rollback(backend) {
        Ok(()) => AccountRegistrationError::from(error),
        Err(rollback_error) => AccountRegistrationError::from(rollback_error),
    }
}

impl From<AccountSecretStoreError> for AccountRegistrationError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, category }
            | AccountSecretStoreError::ReadFailed { alias, category }
            | AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}

pub(crate) fn validate_registration_input(
    email: &str,
    password: &str,
    trial_days: u32,
) -> Result<(), AccountRegistrationError> {
    if email.is_empty() {
        return Err(AccountRegistrationError::MissingEmail);
    }
    if password.trim().is_empty() {
        return Err(AccountRegistrationError::MissingPassword);
    }
    if let Err(error) = validate_new_password(password) {
        return Err(AccountRegistrationError::PasswordTooShort {
            minimum: error.minimum,
        });
    }
    if registration_trial_plan_code(trial_days).is_none() {
        return Err(AccountRegistrationError::InvalidTrialDays { trial_days });
    }
    Ok(())
}

fn registration_artifact_name(artifact: RegistrationArtifact) -> &'static str {
    match artifact {
        RegistrationArtifact::Cookie => "cookie",
        RegistrationArtifact::GitToken => "git_token",
        RegistrationArtifact::TrialExpiry => "trial_expiry",
    }
}

impl AccountRegistrationReport {
    fn skipped_duplicate_email(
        alias: String,
        email: String,
        existing_alias: String,
        trial_days: u32,
    ) -> Self {
        Self {
            alias,
            email,
            trial_days,
            status: AccountRegistrationStatus::SkippedDuplicateEmail,
            existing_alias: Some(existing_alias),
            subscription_succeeded: false,
            cookie_saved: false,
            git_token_saved: false,
            trial_expiry: None,
            git_token_expiry: None,
            missing_recoverable_artifacts: Vec::new(),
        }
    }
}

impl AccountRegistrationError {
    fn from_browser(error: BrowserAutomationError) -> Self {
        match error {
            BrowserAutomationError::RegisteredEmail { email } => Self::RegisteredEmail { email },
            BrowserAutomationError::EmailCodeRequired { email } => {
                Self::EmailCodeRequired { email }
            }
            BrowserAutomationError::ChallengeRequired { challenge } => {
                Self::ChallengeRequired { challenge }
            }
            BrowserAutomationError::Navigation { message }
            | BrowserAutomationError::ExternalDriver { message }
            | BrowserAutomationError::Other { message } => Self::Browser { message },
            BrowserAutomationError::ElementNotFound { selector } => Self::Browser {
                message: format!("browser element not found: {selector}"),
            },
            BrowserAutomationError::Timeout { operation } => Self::Browser {
                message: format!("browser operation timed out: {operation}"),
            },
            BrowserAutomationError::InvalidCredentials => Self::Browser {
                message: "invalid registration credentials".to_string(),
            },
            BrowserAutomationError::RobotVerificationBlocked => Self::Browser {
                message: "robot verification could not be initialized".to_string(),
            },
            BrowserAutomationError::Canceled => Self::Browser {
                message: "browser automation canceled".to_string(),
            },
        }
    }
}

impl From<BrowserAutomationError> for AccountRegistrationError {
    fn from(error: BrowserAutomationError) -> Self {
        Self::from_browser(error)
    }
}
