use std::collections::BTreeMap;

use overleaf_browser::{
    cookies_to_map, BrowserAutomation, BrowserAutomationError, CredentialsLoginInput, LoginResult,
    PasswordChangeInput, SecretText,
};
use overleaf_core::normalize_email;
use overleaf_storage::{AccountRecord, AccountStore, AccountsDocument, SecretBackend};
use serde::Serialize;

use crate::account_secrets::{
    read_account_password_secret, write_account_cookie_secrets_tracked,
    write_account_password_secret_tracked, AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::account_session::{apply_login_metadata, overleaf_session_expiry};
use crate::password_policy::validate_new_password;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverleafPasswordChangeStatus {
    Changed,
    FailedButLoginUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverleafPasswordChangeReport {
    pub alias: String,
    pub email: String,
    pub status: OverleafPasswordChangeStatus,
    pub password_updated: bool,
    pub login_updated: bool,
    pub cookie_expiry: Option<i64>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverleafPasswordChangeError {
    Io {
        message: String,
    },
    AccountAliasNotFound {
        alias: String,
    },
    MissingEmail {
        alias: String,
    },
    MissingCurrentPassword {
        alias: String,
    },
    MissingNewPassword,
    PasswordTooShort {
        minimum: usize,
    },
    MissingSessionCookie,
    InvalidCredentials,
    ChallengeRequired,
    RobotVerificationBlocked,
    LoginEmailMismatch {
        expected: String,
        actual: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
    Browser {
        message: String,
    },
}

pub async fn change_account_overleaf_password_with_backend<B>(
    document: &mut AccountsDocument,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let (report, _) = change_account_overleaf_password_internal(
        document,
        alias,
        new_password,
        browser,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

async fn change_account_overleaf_password_internal<B>(
    document: &mut AccountsDocument,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(OverleafPasswordChangeReport, AccountSecretWriteJournal), OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let alias = alias.trim();
    if new_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingNewPassword);
    }
    if let Err(error) = validate_new_password(new_password.expose()) {
        return Err(OverleafPasswordChangeError::PasswordTooShort {
            minimum: error.minimum,
        });
    }

    let original = document.accounts.get(alias).cloned();
    let record =
        original
            .as_ref()
            .ok_or_else(|| OverleafPasswordChangeError::AccountAliasNotFound {
                alias: alias.to_string(),
            })?;
    let email = non_empty(record.email.as_deref())
        .ok_or_else(|| OverleafPasswordChangeError::MissingEmail {
            alias: alias.to_string(),
        })?
        .to_string();
    let current_password = SecretText::new(current_password_for_change(record, alias, backend)?);
    let login = browser
        .login_with_credentials(CredentialsLoginInput {
            email: email.to_string(),
            password: current_password.clone(),
        })
        .await
        .map_err(OverleafPasswordChangeError::from_browser)?;

    change_account_overleaf_password_from_login_result_internal(
        document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn change_account_overleaf_password_from_login_result_with_backend<B>(
    document: &mut AccountsDocument,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let (report, _) = change_account_overleaf_password_from_login_result_internal(
        document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn change_account_overleaf_password_from_login_result_internal<B>(
    document: &mut AccountsDocument,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<(OverleafPasswordChangeReport, AccountSecretWriteJournal), OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let alias = alias.trim();
    let input = prepare_password_change_input(
        document,
        alias,
        current_password,
        new_password.clone(),
        &login,
    )?;
    let email = input.email.clone();
    let cookies = cookies_to_map(&login.cookies);
    let cookie_expiry = overleaf_session_expiry(&login.cookies);
    let mut staged_document = document.clone();
    let mut journal = AccountSecretWriteJournal::default();
    if let Err(error) = apply_login_update(
        &mut staged_document,
        alias,
        &login,
        &cookies,
        now_unix,
        backend,
        &mut journal,
    ) {
        return Err(rollback_password_change_error(journal, backend, error));
    }

    let change_result = browser.change_password_with_cookie(input).await;

    match change_result {
        Ok(result) if result.changed => {
            let record = staged_document.accounts.get_mut(alias).ok_or_else(|| {
                OverleafPasswordChangeError::AccountAliasNotFound {
                    alias: alias.to_string(),
                }
            })?;
            if let Err(error) = write_changed_password(
                record,
                alias,
                new_password.into_inner(),
                backend,
                &mut journal,
            ) {
                return Err(rollback_password_change_error(journal, backend, error));
            }
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email: email.clone(),
                    status: OverleafPasswordChangeStatus::Changed,
                    password_updated: true,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: None,
                },
                journal,
            ))
        }
        Ok(_) => {
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email: email.clone(),
                    status: OverleafPasswordChangeStatus::FailedButLoginUpdated,
                    password_updated: false,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: Some("browser reported password was not changed".to_string()),
                },
                journal,
            ))
        }
        Err(error) => {
            *document = staged_document;
            Ok((
                OverleafPasswordChangeReport {
                    alias: alias.to_string(),
                    email,
                    status: OverleafPasswordChangeStatus::FailedButLoginUpdated,
                    password_updated: false,
                    login_updated: true,
                    cookie_expiry,
                    failure_message: Some(error.to_string()),
                },
                journal,
            ))
        }
    }
}

fn prepare_password_change_input(
    document: &AccountsDocument,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: &LoginResult,
) -> Result<PasswordChangeInput, OverleafPasswordChangeError> {
    let alias = alias.trim();
    if new_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingNewPassword);
    }
    if let Err(error) = validate_new_password(new_password.expose()) {
        return Err(OverleafPasswordChangeError::PasswordTooShort {
            minimum: error.minimum,
        });
    }
    if current_password.is_empty() {
        return Err(OverleafPasswordChangeError::MissingCurrentPassword {
            alias: alias.to_string(),
        });
    }
    let record = document.accounts.get(alias).ok_or_else(|| {
        OverleafPasswordChangeError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let email = non_empty(record.email.as_deref())
        .ok_or_else(|| OverleafPasswordChangeError::MissingEmail {
            alias: alias.to_string(),
        })?
        .to_string();

    if let Some(actual) = login
        .email
        .as_deref()
        .and_then(|value| non_empty(Some(value)))
    {
        if normalize_email(actual) != normalize_email(&email) {
            return Err(OverleafPasswordChangeError::LoginEmailMismatch {
                expected: email.clone(),
                actual: actual.to_string(),
            });
        }
    }

    let overleaf_session = session_cookie(&cookies_to_map(&login.cookies))?;
    Ok(PasswordChangeInput {
        email,
        overleaf_session: SecretText::new(overleaf_session),
        current_password,
        new_password,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn change_password_with_commit_lock(
    store: &AccountStore,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &(dyn BrowserAutomation + Send + Sync),
    now_unix: i64,
    backend: &dyn SecretBackend,
    commit_lock: &std::sync::Mutex<()>,
    expected_session: Option<&crate::account_session::AccountSessionSnapshot>,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError> {
    let alias = alias.trim();
    let (input, original) = {
        let _guard = commit_lock
            .lock()
            .map_err(|_| OverleafPasswordChangeError::Io {
                message: "account commit lock poisoned".to_string(),
            })?;
        let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
        if expected_session.is_some_and(|expected| {
            !document
                .accounts
                .get(alias)
                .is_some_and(|record| expected.matches(record, alias, backend))
        }) {
            return Err(OverleafPasswordChangeError::Io {
                message: "目标账号在浏览器登录期间已变更，请重新提交任务".to_string(),
            });
        }
        let input = prepare_password_change_input(
            &document,
            alias,
            current_password,
            new_password.clone(),
            &login,
        )?;
        let mut journal = AccountSecretWriteJournal::default();
        if let Err(error) = apply_login_update(
            &mut document,
            alias,
            &login,
            &cookies_to_map(&login.cookies),
            now_unix,
            backend,
            &mut journal,
        ) {
            return Err(rollback_password_change_error(journal, backend, error));
        }
        journal.commit(
            store,
            &document,
            backend,
            OverleafPasswordChangeError::from_io,
        )?;
        let original = crate::account_session::AccountSessionSnapshot::capture(
            &document.accounts[alias],
            alias,
            backend,
        );
        (input, original)
    };
    let email = input.email.clone();
    let result = browser.change_password_with_cookie(input).await;
    if !result.as_ref().is_ok_and(|result| result.changed) {
        return Ok(OverleafPasswordChangeReport {
            alias: alias.to_string(),
            email,
            status: OverleafPasswordChangeStatus::FailedButLoginUpdated,
            password_updated: false,
            login_updated: true,
            cookie_expiry: overleaf_session_expiry(&login.cookies),
            failure_message: Some(
                result
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "browser reported password was not changed".to_string()),
            ),
        });
    }
    let _guard = commit_lock
        .lock()
        .map_err(|_| OverleafPasswordChangeError::Io {
            message: "远端密码已修改，但本地保存锁不可用，请使用新密码重新登录".to_string(),
        })?;
    let document = store.load().map_err(|error| OverleafPasswordChangeError::Io {
        message: format!("远端密码已修改，但本地账号读取失败：{error}；请使用新密码重新登录"),
    })?;
    if !document
        .accounts
        .get(alias)
        .is_some_and(|record| original.matches(record, alias, backend))
    {
        return Err(OverleafPasswordChangeError::Io {
            message: "远端密码已修改，但目标账号在改密期间已变更，请使用新密码重新登录".to_string(),
        });
    }
    let mut report = save_confirmed_password_change_in_store_with_backend(
        store,
        alias,
        new_password,
        None,
        now_unix,
        backend,
    )
    .map_err(|error| OverleafPasswordChangeError::Io {
        message: format!("远端密码已修改，但本地密码保存失败：{error:?}；请使用新密码重新登录"),
    })?;
    report.login_updated = true;
    Ok(report)
}

pub async fn change_account_overleaf_password_in_store_with_backend<B>(
    store: &AccountStore,
    alias: &str,
    new_password: SecretText,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
    let (report, journal) = change_account_overleaf_password_internal(
        &mut document,
        alias,
        new_password,
        browser,
        now_unix,
        backend,
    )
    .await?;
    if report.login_updated || report.password_updated {
        journal.commit(
            store,
            &document,
            backend,
            OverleafPasswordChangeError::from_io,
        )?;
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
pub async fn change_account_overleaf_password_from_login_result_in_store_with_backend<B>(
    store: &AccountStore,
    alias: &str,
    current_password: SecretText,
    new_password: SecretText,
    login: LoginResult,
    browser: &B,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError>
where
    B: BrowserAutomation + Sync + ?Sized,
{
    let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
    let (report, journal) = change_account_overleaf_password_from_login_result_internal(
        &mut document,
        alias,
        current_password,
        new_password,
        login,
        browser,
        now_unix,
        backend,
    )
    .await?;
    if report.login_updated || report.password_updated {
        journal.commit(
            store,
            &document,
            backend,
            OverleafPasswordChangeError::from_io,
        )?;
    }
    Ok(report)
}

pub fn save_confirmed_password_change_in_store_with_backend(
    store: &AccountStore,
    alias: &str,
    new_password: SecretText,
    login: Option<LoginResult>,
    now_unix: i64,
    backend: &dyn SecretBackend,
) -> Result<OverleafPasswordChangeReport, OverleafPasswordChangeError> {
    let alias = alias.trim();
    if let Err(error) = validate_new_password(new_password.expose()) {
        return Err(OverleafPasswordChangeError::PasswordTooShort {
            minimum: error.minimum,
        });
    }
    let mut document = store.load().map_err(OverleafPasswordChangeError::from_io)?;
    let record = document.accounts.get(alias).ok_or_else(|| {
        OverleafPasswordChangeError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    let email = non_empty(record.email.as_deref())
        .ok_or_else(|| OverleafPasswordChangeError::MissingEmail {
            alias: alias.to_string(),
        })?
        .to_string();
    if let Some(actual) = login.as_ref().and_then(|login| login.email.as_deref()) {
        if normalize_email(actual) != normalize_email(&email) {
            return Err(OverleafPasswordChangeError::LoginEmailMismatch {
                expected: email,
                actual: actual.to_string(),
            });
        }
    }
    let mut journal = AccountSecretWriteJournal::default();
    if let Some(login) = login.as_ref() {
        let cookies = cookies_to_map(&login.cookies);
        session_cookie(&cookies)?;
        if let Err(error) = apply_login_update(
            &mut document,
            alias,
            login,
            &cookies,
            now_unix,
            backend,
            &mut journal,
        ) {
            return Err(rollback_password_change_error(journal, backend, error));
        }
    }
    let record = document
        .accounts
        .get_mut(alias)
        .expect("account checked above");
    if let Err(error) = write_changed_password(
        record,
        alias,
        new_password.into_inner(),
        backend,
        &mut journal,
    ) {
        return Err(rollback_password_change_error(journal, backend, error));
    }
    let cookie_expiry = document
        .accounts
        .get(alias)
        .and_then(|record| record.cookie_expiry);
    journal.commit(
        store,
        &document,
        backend,
        OverleafPasswordChangeError::from_io,
    )?;
    Ok(OverleafPasswordChangeReport {
        alias: alias.to_string(),
        email,
        status: OverleafPasswordChangeStatus::Changed,
        password_updated: true,
        login_updated: login.is_some(),
        cookie_expiry: cookie_expiry.map(|value| value as i64),
        failure_message: None,
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn session_cookie(
    cookies: &BTreeMap<String, String>,
) -> Result<String, OverleafPasswordChangeError> {
    cookies
        .get(overleaf_api::OVERLEAF_SESSION_COOKIE)
        .map(String::as_str)
        .and_then(|value| non_empty(Some(value)))
        .map(ToOwned::to_owned)
        .ok_or(OverleafPasswordChangeError::MissingSessionCookie)
}

fn current_password_for_change(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, OverleafPasswordChangeError> {
    read_account_password_secret(record, alias, backend).map_err(OverleafPasswordChangeError::from)
}

fn write_changed_password(
    record: &mut AccountRecord,
    alias: &str,
    new_password: String,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), OverleafPasswordChangeError> {
    write_account_password_secret_tracked(record, alias, &new_password, backend, journal)
        .map_err(OverleafPasswordChangeError::from)?;
    Ok(())
}

fn apply_login_update(
    document: &mut AccountsDocument,
    alias: &str,
    login: &LoginResult,
    cookies: &BTreeMap<String, String>,
    now_unix: i64,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), OverleafPasswordChangeError> {
    let record = document.accounts.get_mut(alias).ok_or_else(|| {
        OverleafPasswordChangeError::AccountAliasNotFound {
            alias: alias.to_string(),
        }
    })?;
    write_account_cookie_secrets_tracked(record, alias, cookies, backend, journal)
        .map_err(OverleafPasswordChangeError::from)?;
    apply_login_metadata(record, login, now_unix);
    Ok(())
}

fn rollback_password_change_error(
    journal: AccountSecretWriteJournal,
    backend: &dyn SecretBackend,
    error: OverleafPasswordChangeError,
) -> OverleafPasswordChangeError {
    match journal.rollback(backend) {
        Ok(()) => error,
        Err(rollback_error) => OverleafPasswordChangeError::from(rollback_error),
    }
}

impl OverleafPasswordChangeError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_browser(error: BrowserAutomationError) -> Self {
        match error {
            BrowserAutomationError::InvalidCredentials => Self::InvalidCredentials,
            BrowserAutomationError::ChallengeRequired { .. } => Self::ChallengeRequired,
            BrowserAutomationError::RobotVerificationBlocked => Self::RobotVerificationBlocked,
            error => Self::Browser {
                message: error.to_string(),
            },
        }
    }
}

impl From<AccountSecretStoreError> for OverleafPasswordChangeError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, category } => {
                if category == "password" {
                    Self::MissingCurrentPassword { alias }
                } else {
                    Self::SecretReadFailed { alias, category }
                }
            }
            AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
            AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use overleaf_browser::{
        BrowserAutomationResult, GitTokenInput, GitTokenResult, PasswordChangeResult,
        RegistrationInput, RegistrationResult,
    };
    use overleaf_storage::{SecretBackendError, SecretReference};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct UnwritableSecrets;
    impl SecretBackend for UnwritableSecrets {
        fn backend_id(&self) -> &str {
            "memory"
        }
        fn read_secret(&self, reference: &SecretReference) -> Result<String, SecretBackendError> {
            Err(SecretBackendError::SecretNotFound {
                key: reference.key.clone(),
            })
        }
        fn write_secret(
            &self,
            reference: &SecretReference,
            _value: &str,
        ) -> Result<(), SecretBackendError> {
            Err(SecretBackendError::OperationFailed {
                operation: "write",
                key: reference.key.clone(),
                message: "unavailable".into(),
            })
        }
        fn delete_secret(&self, _reference: &SecretReference) -> Result<(), SecretBackendError> {
            Ok(())
        }
    }

    struct PasswordBrowser<'a> {
        calls: AtomicUsize,
        commit_lock: &'a Mutex<()>,
    }
    #[async_trait::async_trait]
    impl BrowserAutomation for PasswordBrowser<'_> {
        async fn login_with_credentials(
            &self,
            _input: CredentialsLoginInput,
        ) -> BrowserAutomationResult<LoginResult> {
            unreachable!()
        }
        async fn register_and_subscribe(
            &self,
            _input: RegistrationInput,
        ) -> BrowserAutomationResult<RegistrationResult> {
            unreachable!()
        }
        async fn extract_git_token_with_cookie(
            &self,
            _input: GitTokenInput,
        ) -> BrowserAutomationResult<GitTokenResult> {
            unreachable!()
        }
        async fn change_password_with_cookie(
            &self,
            _input: PasswordChangeInput,
        ) -> BrowserAutomationResult<PasswordChangeResult> {
            assert!(self.commit_lock.try_lock().is_ok());
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(PasswordChangeResult { changed: true })
        }
    }

    #[test]
    fn password_change_requires_cookie_persistence_before_remote_write() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::new(directory.path().join("accounts.json"));
        let mut document = AccountsDocument::default();
        document.accounts.insert(
            "alpha".into(),
            AccountRecord {
                email: Some("alpha@example.com".into()),
                ..Default::default()
            },
        );
        store.save(&document).unwrap();
        let lock = Mutex::new(());
        let browser = PasswordBrowser {
            calls: AtomicUsize::new(0),
            commit_lock: &lock,
        };
        let login = LoginResult {
            email: Some("alpha@example.com".into()),
            user_id: None,
            cookies: vec![overleaf_browser::CookieCapture {
                name: "overleaf_session2".into(),
                value: SecretText::new("session-alpha"),
                expiration_date: Some(300),
            }],
            trial_expiry: None,
            subscription_status: None,
            subscription_label: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let legacy = runtime.block_on(
            change_account_overleaf_password_from_login_result_in_store_with_backend(
                &store,
                "alpha",
                SecretText::new("old-password"),
                SecretText::new("new-password"),
                login.clone(),
                &browser,
                100,
                &UnwritableSecrets,
            ),
        );
        let concurrent = runtime.block_on(change_password_with_commit_lock(
            &store,
            "alpha",
            SecretText::new("old-password"),
            SecretText::new("new-password"),
            login.clone(),
            &browser,
            100,
            &UnwritableSecrets,
            &lock,
            None,
        ));
        assert!(legacy.is_err());
        assert!(concurrent.is_err());
        assert_eq!(browser.calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.load().unwrap(), document);

        let backend = crate::account_secrets::tests::MemorySecrets::default();
        let report = runtime
            .block_on(change_password_with_commit_lock(
                &store,
                "alpha",
                SecretText::new("old-password"),
                SecretText::new("new-password"),
                login,
                &browser,
                100,
                &backend,
                &lock,
                None,
            ))
            .unwrap();
        assert!(report.password_updated && report.login_updated);
        assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
        let saved = store.load().unwrap();
        assert_eq!(
            read_account_password_secret(&saved.accounts["alpha"], "alpha", &backend).unwrap(),
            "new-password"
        );
    }
}
