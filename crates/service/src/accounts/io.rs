use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use futures_util::{stream, StreamExt};
use overleaf_api::{parse_cookie_string, OVERLEAF_SESSION_COOKIE};
use overleaf_core::make_alias;
use overleaf_storage::{
    save_accounts_exchange_document, AccountImportOutcome, AccountRecord, AccountStore,
    AccountsDocument, SecretBackend,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account_secrets::{
    read_account_cookie_secret, read_account_git_token_secret, read_account_password_secret,
    write_account_cookie_secrets_tracked, write_account_git_token_secret_tracked,
    write_account_password_secret_tracked, AccountSecretStoreError, AccountSecretWriteJournal,
};
use crate::account_session::{AccountSessionError, AccountSessionIdentityValidator};
use crate::browser_batch::DEFAULT_BROWSER_BATCH_CONCURRENCY;

pub const ACCOUNT_EXPORT_SENSITIVE_WARNING: &str =
    "Exported account JSON may contain saved passwords, cookies, and Git Integration tokens.";

#[derive(Debug, Clone, PartialEq)]
pub struct AccountImportCandidate {
    pub alias_hint: Option<String>,
    pub record: AccountRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManualCookieValidationItem {
    pub index: usize,
    pub alias: String,
    pub email: String,
    pub valid: bool,
    pub error: Option<AccountSessionError>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ManualCookieValidationReport {
    pub valid_count: usize,
    pub failed_count: usize,
    pub items: Vec<ManualCookieValidationItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountImportDecisionStatus {
    Imported,
    SkippedDuplicateEmail,
    AliasConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountImportDecision {
    pub requested_alias: Option<String>,
    pub resolved_alias: String,
    pub email: Option<String>,
    pub status: AccountImportDecisionStatus,
    pub existing_alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountImportReport {
    pub decisions: Vec<AccountImportDecision>,
    pub imported_count: usize,
    pub skipped_duplicate_email_count: usize,
    pub alias_conflict_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountExportMode {
    SingleFile,
    MultipleFiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountExportedFile {
    pub alias: Option<String>,
    pub path: String,
    pub account_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountExportReport {
    pub files: Vec<AccountExportedFile>,
    pub exported_account_count: usize,
    pub overwritten_file_count: usize,
    pub contains_sensitive_fields: bool,
    pub sensitive_warning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountIoError {
    Io {
        message: String,
    },
    Json {
        message: String,
    },
    UnsupportedImportShape,
    NoAccountsSelected,
    EmptyOutputDirectory,
    AliasNotFound {
        alias: String,
    },
    RefusesToOverwriteMainConfig {
        path: String,
    },
    RefusesToOverwriteExistingExport {
        path: String,
    },
    SecretReadFailed {
        alias: String,
        category: &'static str,
    },
    SecretWriteFailed {
        alias: String,
        category: &'static str,
    },
}

pub fn parse_account_import_json(
    input: &str,
) -> Result<Vec<AccountImportCandidate>, AccountIoError> {
    let value: Value = serde_json::from_str(input).map_err(AccountIoError::from_json)?;
    let object = value
        .as_object()
        .ok_or(AccountIoError::UnsupportedImportShape)?;

    if let Some(accounts) = object.get("accounts") {
        let accounts = accounts
            .as_object()
            .ok_or(AccountIoError::UnsupportedImportShape)?;
        return accounts
            .iter()
            .map(|(alias, value)| parse_record_candidate(Some(alias), value.clone()))
            .collect();
    }

    if looks_like_account_record(object) {
        return Ok(vec![parse_record_candidate(None, value)?]);
    }

    if object.values().all(Value::is_object) {
        return object
            .iter()
            .map(|(alias, value)| parse_record_candidate(Some(alias), value.clone()))
            .collect();
    }

    Err(AccountIoError::UnsupportedImportShape)
}

pub fn manual_cookie_import_candidate(
    alias_hint: Option<String>,
    email: impl Into<String>,
    cookie_input: &str,
) -> AccountImportCandidate {
    AccountImportCandidate {
        alias_hint,
        record: AccountRecord {
            email: Some(email.into()),
            cookies: parse_cookie_string(cookie_input),
            ..empty_record()
        },
    }
}

pub async fn validate_manual_cookie_import_candidates(
    candidates: &[AccountImportCandidate],
    validator: &(dyn AccountSessionIdentityValidator + Send + Sync),
) -> ManualCookieValidationReport {
    let jobs = candidates
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, candidate)| async move {
            let expected_email = candidate
                .record
                .email
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string();
            let alias = make_alias(candidate.alias_hint.as_deref(), &expected_email);
            let has_session_cookie = candidate
                .record
                .cookies
                .get(OVERLEAF_SESSION_COOKIE)
                .is_some_and(|value| !value.trim().is_empty());
            let error = if expected_email.is_empty() || !has_session_cookie {
                Some(AccountSessionError::InvalidSessionCookie {
                    alias: alias.clone(),
                })
            } else {
                validator
                    .validate(&alias, Some(&expected_email), &candidate.record.cookies)
                    .await
                    .err()
            };
            ManualCookieValidationItem {
                index,
                alias,
                email: expected_email,
                valid: error.is_none(),
                error,
            }
        });
    let mut items = stream::iter(jobs)
        .buffer_unordered(DEFAULT_BROWSER_BATCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    items.sort_by_key(|item| item.index);
    ManualCookieValidationReport {
        valid_count: items.iter().filter(|item| item.valid).count(),
        failed_count: items.iter().filter(|item| !item.valid).count(),
        items,
    }
}

pub fn preview_account_import(
    document: &AccountsDocument,
    candidates: &[AccountImportCandidate],
) -> AccountImportReport {
    let mut shadow = document.clone();
    apply_account_import(&mut shadow, candidates)
}

fn apply_account_import(
    document: &mut AccountsDocument,
    candidates: &[AccountImportCandidate],
) -> AccountImportReport {
    let mut decisions = Vec::with_capacity(candidates.len());
    let mut imported_count = 0;
    let mut skipped_duplicate_email_count = 0;
    let mut alias_conflict_count = 0;

    for candidate in candidates {
        let requested_alias = candidate.alias_hint.clone();
        let email = candidate.record.email.clone();
        let outcome =
            document.import_record(candidate.alias_hint.as_deref(), candidate.record.clone());
        let decision = match outcome {
            AccountImportOutcome::Imported { alias } => {
                imported_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email,
                    status: AccountImportDecisionStatus::Imported,
                    existing_alias: None,
                }
            }
            AccountImportOutcome::SkippedDuplicateEmail {
                alias,
                existing_alias,
            } => {
                skipped_duplicate_email_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email,
                    status: AccountImportDecisionStatus::SkippedDuplicateEmail,
                    existing_alias: Some(existing_alias),
                }
            }
            AccountImportOutcome::AliasConflict { alias } => {
                alias_conflict_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email: candidate.record.email.clone(),
                    status: AccountImportDecisionStatus::AliasConflict,
                    existing_alias: None,
                }
            }
        };
        decisions.push(decision);
    }

    AccountImportReport {
        decisions,
        imported_count,
        skipped_duplicate_email_count,
        alias_conflict_count,
    }
}

pub fn apply_account_import_with_backend(
    document: &mut AccountsDocument,
    candidates: &[AccountImportCandidate],
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountImportReport, AccountIoError> {
    let (report, _) = apply_account_import_internal(document, candidates, backend)?;
    Ok(report)
}

fn apply_account_import_internal(
    document: &mut AccountsDocument,
    candidates: &[AccountImportCandidate],
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<(AccountImportReport, AccountSecretWriteJournal), AccountIoError> {
    let mut staged_document = document.clone();
    let mut journal = AccountSecretWriteJournal::default();
    let mut decisions = Vec::with_capacity(candidates.len());
    let mut imported_count = 0;
    let mut skipped_duplicate_email_count = 0;
    let mut alias_conflict_count = 0;

    for candidate in candidates {
        let requested_alias = candidate.alias_hint.clone();
        let email = candidate.record.email.clone();
        let alias = make_alias(
            candidate.alias_hint.as_deref(),
            candidate.record.email.as_deref().unwrap_or_default(),
        );
        let decision = if let Some(email) = candidate.record.email.as_deref() {
            if let Some(existing_alias) = staged_document.duplicate_alias_by_email(email) {
                skipped_duplicate_email_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email: candidate.record.email.clone(),
                    status: AccountImportDecisionStatus::SkippedDuplicateEmail,
                    existing_alias: Some(existing_alias.to_string()),
                }
            } else if staged_document.accounts.contains_key(&alias) {
                alias_conflict_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email: candidate.record.email.clone(),
                    status: AccountImportDecisionStatus::AliasConflict,
                    existing_alias: None,
                }
            } else {
                let mut record = candidate.record.clone();
                if let Err(error) =
                    persist_import_record_secrets(&mut record, &alias, backend, &mut journal)
                {
                    return Err(rollback_import_secret_error(journal, backend, error));
                }
                staged_document.accounts.insert(alias.clone(), record);
                imported_count += 1;
                AccountImportDecision {
                    requested_alias,
                    resolved_alias: alias,
                    email: candidate.record.email.clone(),
                    status: AccountImportDecisionStatus::Imported,
                    existing_alias: None,
                }
            }
        } else if staged_document.accounts.contains_key(&alias) {
            alias_conflict_count += 1;
            AccountImportDecision {
                requested_alias,
                resolved_alias: alias,
                email,
                status: AccountImportDecisionStatus::AliasConflict,
                existing_alias: None,
            }
        } else {
            let mut record = candidate.record.clone();
            if let Err(error) =
                persist_import_record_secrets(&mut record, &alias, backend, &mut journal)
            {
                return Err(rollback_import_secret_error(journal, backend, error));
            }
            staged_document.accounts.insert(alias.clone(), record);
            imported_count += 1;
            AccountImportDecision {
                requested_alias,
                resolved_alias: alias,
                email,
                status: AccountImportDecisionStatus::Imported,
                existing_alias: None,
            }
        };
        decisions.push(decision);
    }

    *document = staged_document;
    Ok((
        AccountImportReport {
            decisions,
            imported_count,
            skipped_duplicate_email_count,
            alias_conflict_count,
        },
        journal,
    ))
}

fn persist_import_record_secrets(
    record: &mut AccountRecord,
    alias: &str,
    backend: &(dyn SecretBackend + Send + Sync),
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountIoError> {
    let password = record.password.clone();
    let cookies = record.cookies.clone();
    let git_token = record.git_token.clone();

    // 交换文件中的密钥引用不能跨机器使用，导入后重新生成本机引用。
    record.password_ref = None;
    record.cookie_refs.clear();
    record.git_token_ref = None;
    record.password = None;
    record.cookies.clear();
    record.git_token = None;

    if let Some(password) = password.as_deref().filter(|value| !value.trim().is_empty()) {
        write_account_password_secret_tracked(record, alias, password, backend, journal)?;
    }
    if !cookies.is_empty() {
        write_account_cookie_secrets_tracked(record, alias, &cookies, backend, journal)?;
    }
    if let Some(token) = git_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        write_account_git_token_secret_tracked(record, alias, token, backend, journal)?;
    }
    Ok(())
}

fn rollback_import_secret_error(
    journal: AccountSecretWriteJournal,
    backend: &(dyn SecretBackend + Send + Sync),
    error: AccountIoError,
) -> AccountIoError {
    match journal.rollback(backend) {
        Ok(()) => error,
        Err(rollback_error) => AccountIoError::from(rollback_error),
    }
}

pub fn import_accounts_from_json_file_with_backend(
    store: &AccountStore,
    import_path: impl AsRef<Path>,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountImportReport, AccountIoError> {
    let input = fs::read_to_string(import_path).map_err(AccountIoError::from_io)?;
    let mut document = store.load().map_err(AccountIoError::from_io)?;
    let candidates = parse_account_import_json(&input)?;
    let (report, journal) = apply_account_import_internal(&mut document, &candidates, backend)?;
    if report.imported_count > 0 {
        match store.save(&document) {
            Ok(()) => {}
            Err(error) => {
                return Err(match journal.rollback(backend) {
                    Ok(()) => AccountIoError::from_io(error),
                    Err(rollback_error) => AccountIoError::from(rollback_error),
                });
            }
        }
    }
    Ok(report)
}

pub fn export_accounts_to_directory_with_backend<I, S>(
    document: &AccountsDocument,
    aliases: I,
    mode: AccountExportMode,
    output_dir: impl AsRef<Path>,
    main_config_path: impl AsRef<Path>,
    confirm_overwrite: bool,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountExportReport, AccountIoError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let aliases = normalized_aliases(aliases);
    if aliases.is_empty() {
        return Err(AccountIoError::NoAccountsSelected);
    }
    let selected = selected_export_document(document, &aliases, backend)?;
    write_selected_export_to_directory(
        &selected,
        aliases,
        mode,
        output_dir,
        main_config_path,
        confirm_overwrite,
    )
}

pub fn export_accounts_to_json_with_backend<I, S>(
    document: &AccountsDocument,
    aliases: I,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<String, AccountIoError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let aliases = normalized_aliases(aliases);
    if aliases.is_empty() {
        return Err(AccountIoError::NoAccountsSelected);
    }
    let selected = selected_export_document(document, &aliases, backend)?;
    selected
        .to_exchange_json()
        .map_err(AccountIoError::from_json)
}

fn normalized_aliases<I, S>(aliases: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    aliases
        .into_iter()
        .map(|alias| alias.as_ref().trim().to_string())
        .filter(|alias| !alias.is_empty())
        .collect()
}

fn selected_export_document(
    document: &AccountsDocument,
    aliases: &[String],
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<AccountsDocument, AccountIoError> {
    let mut selected = document
        .selected_document(aliases.iter().map(String::as_str))
        .map_err(|message| AccountIoError::AliasNotFound {
            alias: alias_from_not_found_message(&message),
        })?;
    for (alias, record) in selected.accounts.iter_mut() {
        resolve_record_secrets_for_export(alias, record, backend)?;
    }
    Ok(selected)
}

fn resolve_record_secrets_for_export(
    alias: &str,
    record: &mut AccountRecord,
    backend: &(dyn SecretBackend + Send + Sync),
) -> Result<(), AccountIoError> {
    if record.password_ref.is_some() {
        record.password = Some(read_account_password_secret(record, alias, backend)?);
    }
    if !record.cookie_refs.is_empty() {
        let cookie_names: Vec<String> = record.cookie_refs.keys().cloned().collect();
        for cookie_name in cookie_names {
            let value = read_account_cookie_secret(record, alias, &cookie_name, backend)?;
            record.cookies.insert(cookie_name, value);
        }
    }
    if record.git_token_ref.is_some() {
        record.git_token = Some(read_account_git_token_secret(record, alias, backend)?);
    }
    record.password_ref = None;
    record.cookie_refs.clear();
    record.git_token_ref = None;
    Ok(())
}

fn write_selected_export_to_directory(
    selected: &AccountsDocument,
    aliases: Vec<String>,
    mode: AccountExportMode,
    output_dir: impl AsRef<Path>,
    main_config_path: impl AsRef<Path>,
    confirm_overwrite: bool,
) -> Result<AccountExportReport, AccountIoError> {
    let output_dir = output_dir.as_ref();
    if output_dir.as_os_str().is_empty() {
        return Err(AccountIoError::EmptyOutputDirectory);
    }
    let main_config_path = main_config_path.as_ref();

    match mode {
        AccountExportMode::SingleFile => {
            let output_path = output_dir.join("accounts.json");
            let will_overwrite =
                ensure_export_write_allowed(&output_path, main_config_path, confirm_overwrite)?;
            fs::create_dir_all(output_dir).map_err(AccountIoError::from_io)?;
            save_accounts_exchange_document(&output_path, selected)
                .map_err(AccountIoError::from_io)?;
            let expected_aliases = selected.accounts.keys().cloned().collect::<Vec<_>>();
            verify_exported_accounts_file(&output_path, &expected_aliases)?;
            Ok(AccountExportReport {
                exported_account_count: selected.accounts.len(),
                overwritten_file_count: usize::from(will_overwrite),
                contains_sensitive_fields: selected_contains_sensitive_fields(selected),
                sensitive_warning: ACCOUNT_EXPORT_SENSITIVE_WARNING.to_string(),
                files: vec![AccountExportedFile {
                    alias: None,
                    path: output_path.to_string_lossy().to_string(),
                    account_count: selected.accounts.len(),
                }],
            })
        }
        AccountExportMode::MultipleFiles => {
            fs::create_dir_all(output_dir).map_err(AccountIoError::from_io)?;
            let mut used_paths = BTreeSet::new();
            let mut files = Vec::with_capacity(aliases.len());
            let mut overwritten_file_count = 0usize;
            for alias in aliases {
                let single = selected.selected_document([alias.as_str()]).map_err(|_| {
                    AccountIoError::AliasNotFound {
                        alias: alias.clone(),
                    }
                })?;
                let output_path = unique_account_output_path(output_dir, &alias, &mut used_paths);
                if ensure_export_write_allowed(&output_path, main_config_path, confirm_overwrite)? {
                    overwritten_file_count += 1;
                }
                save_accounts_exchange_document(&output_path, &single)
                    .map_err(AccountIoError::from_io)?;
                verify_exported_accounts_file(&output_path, std::slice::from_ref(&alias))?;
                files.push(AccountExportedFile {
                    alias: Some(alias),
                    path: output_path.to_string_lossy().to_string(),
                    account_count: single.accounts.len(),
                });
            }

            Ok(AccountExportReport {
                exported_account_count: files.iter().map(|file| file.account_count).sum(),
                overwritten_file_count,
                contains_sensitive_fields: selected_contains_sensitive_fields(selected),
                sensitive_warning: ACCOUNT_EXPORT_SENSITIVE_WARNING.to_string(),
                files,
            })
        }
    }
}

fn verify_exported_accounts_file(
    path: &Path,
    expected_aliases: &[String],
) -> Result<(), AccountIoError> {
    let text = fs::read_to_string(path).map_err(AccountIoError::from_io)?;
    let document = AccountsDocument::from_json_str(&text).map_err(AccountIoError::from_json)?;
    let actual = document.accounts.keys().cloned().collect::<BTreeSet<_>>();
    let expected = expected_aliases.iter().cloned().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(AccountIoError::Io {
            message: format!(
                "export verification failed for {}: expected {} account(s), found {}",
                path.display(),
                expected.len(),
                actual.len()
            ),
        });
    }
    Ok(())
}

fn selected_contains_sensitive_fields(document: &AccountsDocument) -> bool {
    document.accounts.values().any(|record| {
        record
            .password
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || record
                .git_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || !record.cookies.is_empty()
            || record.password_ref.is_some()
            || record.git_token_ref.is_some()
            || !record.cookie_refs.is_empty()
    })
}

fn parse_record_candidate(
    alias_hint: Option<&str>,
    value: Value,
) -> Result<AccountImportCandidate, AccountIoError> {
    let record: AccountRecord = serde_json::from_value(value).map_err(AccountIoError::from_json)?;
    Ok(AccountImportCandidate {
        alias_hint: alias_hint.map(str::to_string),
        record,
    })
}

fn looks_like_account_record(object: &serde_json::Map<String, Value>) -> bool {
    const ACCOUNT_FIELDS: &[&str] = &[
        "cookies",
        "user_id",
        "email",
        "password",
        "cookie_expiry",
        "cookie_updated_at",
        "created_at",
        "last_login_at",
        "trial_days",
        "trial_started_at",
        "trial_expiry",
        "subscription_status",
        "subscription_label",
        "subscription_checked_at",
        "git_token",
        "git_token_expiry",
    ];
    object
        .keys()
        .any(|key| ACCOUNT_FIELDS.iter().any(|field| field == key))
}

fn ensure_not_main_config(
    output_path: &Path,
    main_config_path: &Path,
) -> Result<(), AccountIoError> {
    if path_key(output_path) == path_key(main_config_path) {
        return Err(AccountIoError::RefusesToOverwriteMainConfig {
            path: output_path.to_string_lossy().to_string(),
        });
    }
    Ok(())
}

fn ensure_export_write_allowed(
    output_path: &Path,
    main_config_path: &Path,
    confirm_overwrite: bool,
) -> Result<bool, AccountIoError> {
    ensure_not_main_config(output_path, main_config_path)?;
    let will_overwrite = output_path.exists();
    if will_overwrite && !confirm_overwrite {
        return Err(AccountIoError::RefusesToOverwriteExistingExport {
            path: output_path.to_string_lossy().to_string(),
        });
    }
    Ok(will_overwrite)
}

fn unique_account_output_path(
    output_dir: &Path,
    alias: &str,
    used_paths: &mut BTreeSet<String>,
) -> PathBuf {
    let stem = safe_filename_stem(alias);
    let mut suffix = 0usize;
    loop {
        let file_name = if suffix == 0 {
            format!("{stem}.json")
        } else {
            format!("{stem}-{suffix}.json")
        };
        let path = output_dir.join(file_name);
        let key = path_key(&path);
        if used_paths.insert(key) {
            return path;
        }
        suffix += 1;
    }
}

fn safe_filename_stem(alias: &str) -> String {
    let value: String = alias
        .trim()
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let value = value.trim_matches(|ch| ch == '.' || ch == ' ');
    if value.is_empty() {
        "account".to_string()
    } else {
        value.to_string()
    }
}

fn path_key(path: &Path) -> String {
    let normalized = path_for_compare(path);
    let key = normalized.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn path_for_compare(path: &Path) -> PathBuf {
    if path.exists() {
        return path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    }

    if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
        if parent.exists() {
            if let Ok(parent) = parent.canonicalize() {
                return parent.join(file_name);
            }
        }
    }

    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::new())
            .join(path)
    }
}

fn alias_from_not_found_message(message: &str) -> String {
    message
        .strip_prefix("account alias not found: ")
        .unwrap_or(message)
        .to_string()
}

impl AccountIoError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }

    fn from_json(error: serde_json::Error) -> Self {
        Self::Json {
            message: error.to_string(),
        }
    }
}

impl From<AccountSecretStoreError> for AccountIoError {
    fn from(error: AccountSecretStoreError) -> Self {
        match error {
            AccountSecretStoreError::Missing { alias, category }
            | AccountSecretStoreError::ReadFailed { alias, category } => {
                Self::SecretReadFailed { alias, category }
            }
            AccountSecretStoreError::WriteFailed { alias, category } => {
                Self::SecretWriteFailed { alias, category }
            }
        }
    }
}

fn empty_record() -> AccountRecord {
    AccountRecord {
        cookie_refs: Default::default(),
        password_ref: None,
        git_token_ref: None,
        cookies: Default::default(),
        user_id: None,
        email: None,
        password: None,
        cookie_expiry: None,
        cookie_updated_at: None,
        created_at: None,
        last_login_at: None,
        trial_days: None,
        trial_started_at: None,
        trial_expiry: None,
        subscription_status: None,
        subscription_label: None,
        subscription_checked_at: None,
        git_token: None,
        git_token_expiry: None,
    }
}
