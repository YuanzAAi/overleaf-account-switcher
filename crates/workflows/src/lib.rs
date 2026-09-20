use std::collections::{BTreeMap, BTreeSet};

use overleaf_core::{make_alias, normalize_email};
use overleaf_storage::AccountsDocument;

pub mod migrate_projects;
pub use migrate_projects::{
    audit_project_migrations, is_migration_confirmed, plan_project_migrations,
    plan_remigration_once, rate_limit_wait_seconds, resolve_join_attempt, ApiAttemptResult,
    JoinAttemptDecision, ProjectMigrationAudit, ProjectMigrationEvidence, ProjectMigrationPlan,
    ProjectMigrationStrategy,
};
pub mod registration;
pub use registration::{
    decide_registration_completion, registered_email_error_seen, registered_email_prompt,
    registration_steps_for_trial, RegistrationArtifact, RegistrationCompletionDecision,
    RegistrationCompletionInput, RegistrationCompletionStatus, RegistrationState,
    RegistrationTaskEvent, RegistrationUserPrompt, RegistrationWorkflow, RECAPTCHA_WAIT_SECONDS,
    REGISTERED_EMAIL_ERROR_TEXT,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchInputError {
    MissingEmail,
    MissingPassword,
    EmptyValue { field: &'static str },
    AliasCountMismatch { aliases: usize, emails: usize },
    PasswordCountMismatch { passwords: usize, accounts: usize },
    MultiplePasswordsForSingleAccount { passwords: usize },
    AliasConflict { alias: String },
    AliasRepeatedInInput { alias: String },
    AccountAliasNotFound { alias: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialBatchInput {
    pub aliases: String,
    pub emails: String,
    pub passwords: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAccountPlan {
    pub alias_hint: Option<String>,
    pub alias: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedDuplicateEmail {
    pub alias: String,
    pub email: String,
    pub existing_alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CredentialBatchPlan {
    pub login_accounts: Vec<CredentialAccountPlan>,
    pub skipped_duplicate_emails: Vec<SkippedDuplicateEmail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasPasswordPlan {
    pub alias: String,
    pub email: Option<String>,
    pub password: String,
}

pub fn split_comma_values(input: &str) -> Vec<String> {
    input
        .split([',', '，'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn plan_credential_accounts(
    accounts: &AccountsDocument,
    input: CredentialBatchInput,
) -> Result<CredentialBatchPlan, BatchInputError> {
    let emails = parse_required_list("邮箱", &input.emails)?;
    if emails.is_empty() {
        return Err(BatchInputError::MissingEmail);
    }

    let passwords = expand_passwords(emails.len(), &input.passwords)?;
    let aliases = parse_aliases(emails.len(), &input.aliases)?;

    let mut plan = CredentialBatchPlan::default();
    let mut planned_by_email = BTreeMap::<String, String>::new();
    let mut planned_aliases = BTreeSet::<String>::new();

    for ((alias_hint, email), password) in aliases.into_iter().zip(emails).zip(passwords) {
        let normalized_email = normalize_email(&email);
        let alias = make_alias(alias_hint.as_deref(), &email);

        if let Some(existing_alias) = accounts.duplicate_alias_by_email(&email) {
            plan.skipped_duplicate_emails.push(SkippedDuplicateEmail {
                alias,
                email,
                existing_alias: existing_alias.to_string(),
            });
            continue;
        }

        if let Some(existing_alias) = planned_by_email.get(&normalized_email) {
            plan.skipped_duplicate_emails.push(SkippedDuplicateEmail {
                alias,
                email,
                existing_alias: existing_alias.clone(),
            });
            continue;
        }

        if accounts.alias_conflicts_with_email(&alias, &email) {
            return Err(BatchInputError::AliasConflict { alias });
        }

        if !planned_aliases.insert(alias.clone()) {
            return Err(BatchInputError::AliasRepeatedInInput { alias });
        }

        planned_by_email.insert(normalized_email, alias.clone());
        plan.login_accounts.push(CredentialAccountPlan {
            alias_hint,
            alias,
            email,
            password,
        });
    }

    Ok(plan)
}

pub fn plan_alias_passwords(
    accounts: &AccountsDocument,
    aliases_input: &str,
    passwords_input: &str,
) -> Result<Vec<AliasPasswordPlan>, BatchInputError> {
    let aliases = parse_required_list("账号别名", aliases_input)?;
    if aliases.is_empty() {
        return Err(BatchInputError::AccountAliasNotFound {
            alias: String::new(),
        });
    }

    let passwords = expand_passwords(aliases.len(), passwords_input)?;
    let mut seen = BTreeSet::new();
    let mut plans = Vec::with_capacity(aliases.len());

    for (alias, password) in aliases.into_iter().zip(passwords) {
        if !seen.insert(alias.clone()) {
            return Err(BatchInputError::AliasRepeatedInInput { alias });
        }

        let Some(record) = accounts.accounts.get(&alias) else {
            return Err(BatchInputError::AccountAliasNotFound { alias });
        };

        plans.push(AliasPasswordPlan {
            alias,
            email: record.email.clone(),
            password,
        });
    }

    Ok(plans)
}

fn parse_aliases(
    account_count: usize,
    input: &str,
) -> Result<Vec<Option<String>>, BatchInputError> {
    if input.trim().is_empty() {
        return Ok(vec![None; account_count]);
    }

    let aliases = parse_required_list("账号别名", input)?;
    if aliases.len() != account_count {
        return Err(BatchInputError::AliasCountMismatch {
            aliases: aliases.len(),
            emails: account_count,
        });
    }

    Ok(aliases.into_iter().map(Some).collect())
}

fn expand_passwords(account_count: usize, input: &str) -> Result<Vec<String>, BatchInputError> {
    let passwords = parse_required_list("密码", input)?;
    if passwords.is_empty() {
        return Err(BatchInputError::MissingPassword);
    }

    if account_count == 1 && passwords.len() > 1 {
        return Err(BatchInputError::MultiplePasswordsForSingleAccount {
            passwords: passwords.len(),
        });
    }

    if passwords.len() == 1 {
        return Ok(vec![passwords[0].clone(); account_count]);
    }

    if passwords.len() == account_count {
        return Ok(passwords);
    }

    Err(BatchInputError::PasswordCountMismatch {
        passwords: passwords.len(),
        accounts: account_count,
    })
}

fn parse_required_list(field: &'static str, input: &str) -> Result<Vec<String>, BatchInputError> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    for raw in input.split([',', '，']) {
        let value = raw.trim();
        if value.is_empty() {
            return Err(BatchInputError::EmptyValue { field });
        }
        values.push(value.to_string());
    }

    Ok(values)
}
