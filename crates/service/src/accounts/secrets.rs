use std::collections::BTreeMap;

use overleaf_storage::{
    account_cookie_secret_key, account_git_token_secret_key, account_password_secret_key,
    AccountRecord, AccountStore, AccountsDocument, SecretBackend, SecretBackendError,
    SecretReference,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountSecretStoreError {
    Missing {
        alias: String,
        category: &'static str,
    },
    ReadFailed {
        alias: String,
        category: &'static str,
    },
    WriteFailed {
        alias: String,
        category: &'static str,
    },
}

#[derive(Default)]
pub(crate) struct AccountSecretWriteJournal {
    entries: Vec<AccountSecretRestorePoint>,
}

struct AccountSecretRestorePoint {
    alias: String,
    category: &'static str,
    reference: SecretReference,
    previous_value: Option<String>,
}

impl AccountSecretWriteJournal {
    pub(crate) fn commit<E: From<AccountSecretStoreError>>(
        self,
        store: &AccountStore,
        document: &AccountsDocument,
        backend: &dyn SecretBackend,
        from_io: impl FnOnce(std::io::Error) -> E,
    ) -> Result<(), E> {
        if let Err(error) = store.save(document) {
            self.rollback(backend)?;
            return Err(from_io(error));
        }
        Ok(())
    }

    fn capture(
        &mut self,
        alias: &str,
        category: &'static str,
        backend: &dyn SecretBackend,
        reference: &SecretReference,
    ) -> Result<(), AccountSecretStoreError> {
        if self
            .entries
            .iter()
            .any(|entry| entry.reference == *reference)
        {
            return Ok(());
        }

        let previous_value = match backend.read_secret(reference) {
            Ok(value) => Some(value),
            Err(SecretBackendError::SecretNotFound { .. }) => None,
            Err(_) => {
                return Err(AccountSecretStoreError::ReadFailed {
                    alias: alias.to_string(),
                    category,
                });
            }
        };
        self.entries.push(AccountSecretRestorePoint {
            alias: alias.to_string(),
            category,
            reference: reference.clone(),
            previous_value,
        });
        Ok(())
    }

    pub(crate) fn rollback(
        self,
        backend: &dyn SecretBackend,
    ) -> Result<(), AccountSecretStoreError> {
        let mut first_error = None;
        for entry in self.entries.into_iter().rev() {
            let result = match entry.previous_value {
                Some(value) => backend.write_secret(&entry.reference, &value),
                None => match backend.delete_secret(&entry.reference) {
                    Ok(()) | Err(SecretBackendError::SecretNotFound { .. }) => Ok(()),
                    Err(error) => Err(error),
                },
            };
            if result.is_err() && first_error.is_none() {
                first_error = Some(AccountSecretStoreError::WriteFailed {
                    alias: entry.alias,
                    category: entry.category,
                });
            }
        }

        first_error.map_or(Ok(()), Err)
    }
}

pub fn write_account_password_secret(
    record: &mut AccountRecord,
    alias: &str,
    password: &str,
    backend: &dyn SecretBackend,
) -> Result<(), AccountSecretStoreError> {
    let mut journal = AccountSecretWriteJournal::default();
    match write_account_password_secret_tracked(record, alias, password, backend, &mut journal) {
        Ok(()) => Ok(()),
        Err(error) => match journal.rollback(backend) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

pub(crate) fn write_account_password_secret_tracked(
    record: &mut AccountRecord,
    alias: &str,
    password: &str,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountSecretStoreError> {
    let reference = record
        .password_ref
        .clone()
        .unwrap_or_else(|| SecretReference::new("password", account_password_secret_key(alias)));
    write_secret_tracked(alias, "password", backend, &reference, password, journal)?;
    record.password_ref = Some(reference);
    record.password = None;
    Ok(())
}

pub fn write_account_cookie_secrets(
    record: &mut AccountRecord,
    alias: &str,
    cookies: &BTreeMap<String, String>,
    backend: &dyn SecretBackend,
) -> Result<(), AccountSecretStoreError> {
    let mut journal = AccountSecretWriteJournal::default();
    match write_account_cookie_secrets_tracked(record, alias, cookies, backend, &mut journal) {
        Ok(()) => Ok(()),
        Err(error) => match journal.rollback(backend) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

pub(crate) fn write_account_cookie_secrets_tracked(
    record: &mut AccountRecord,
    alias: &str,
    cookies: &BTreeMap<String, String>,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountSecretStoreError> {
    for (cookie_name, cookie_value) in cookies {
        if cookie_value.trim().is_empty() {
            continue;
        }
        let reference = record
            .cookie_refs
            .get(cookie_name)
            .cloned()
            .unwrap_or_else(|| {
                SecretReference::new("cookie", account_cookie_secret_key(alias, cookie_name))
            });
        write_secret_tracked(alias, "cookie", backend, &reference, cookie_value, journal)?;
        record.cookie_refs.insert(cookie_name.clone(), reference);
    }
    record.cookies.clear();
    Ok(())
}

pub fn write_account_git_token_secret(
    record: &mut AccountRecord,
    alias: &str,
    token: &str,
    backend: &dyn SecretBackend,
) -> Result<(), AccountSecretStoreError> {
    let mut journal = AccountSecretWriteJournal::default();
    match write_account_git_token_secret_tracked(record, alias, token, backend, &mut journal) {
        Ok(()) => Ok(()),
        Err(error) => match journal.rollback(backend) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

pub(crate) fn write_account_git_token_secret_tracked(
    record: &mut AccountRecord,
    alias: &str,
    token: &str,
    backend: &dyn SecretBackend,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountSecretStoreError> {
    let reference = record
        .git_token_ref
        .clone()
        .unwrap_or_else(|| SecretReference::new("git_token", account_git_token_secret_key(alias)));
    write_secret_tracked(alias, "git_token", backend, &reference, token, journal)?;
    record.git_token_ref = Some(reference);
    record.git_token = None;
    Ok(())
}

pub fn resolve_account_cookies(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<BTreeMap<String, String>, AccountSecretStoreError> {
    if record.cookie_refs.is_empty() {
        return Err(AccountSecretStoreError::Missing {
            alias: alias.to_string(),
            category: "cookie",
        });
    }

    let mut cookies = BTreeMap::new();
    for (cookie_name, reference) in &record.cookie_refs {
        let value = read_secret(alias, "cookie", backend, reference)?;
        cookies.insert(cookie_name.clone(), value);
    }
    Ok(cookies)
}

pub fn resolve_account_cookies_for_recovery(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<BTreeMap<String, String>, AccountSecretStoreError> {
    if record.cookie_refs.is_empty() {
        return Err(AccountSecretStoreError::Missing {
            alias: alias.to_string(),
            category: "cookie",
        });
    }

    let mut cookies = BTreeMap::new();
    for (cookie_name, reference) in &record.cookie_refs {
        let value = read_secret_for_recovery(alias, "cookie", backend, reference)?;
        cookies.insert(cookie_name.clone(), value);
    }
    Ok(cookies)
}

pub fn read_account_cookie_secret(
    record: &AccountRecord,
    alias: &str,
    cookie_name: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .cookie_refs
            .get(cookie_name)
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "cookie",
            })?;
    read_secret(alias, "cookie", backend, reference)
}

pub fn read_account_cookie_secret_for_recovery(
    record: &AccountRecord,
    alias: &str,
    cookie_name: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .cookie_refs
            .get(cookie_name)
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "cookie",
            })?;
    read_secret_for_recovery(alias, "cookie", backend, reference)
}

pub fn read_account_password_secret(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .password_ref
            .as_ref()
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "password",
            })?;
    read_secret(alias, "password", backend, reference)
}

pub fn read_account_password_secret_for_recovery(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .password_ref
            .as_ref()
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "password",
            })?;
    read_secret_for_recovery(alias, "password", backend, reference)
}

pub fn read_account_git_token_secret(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .git_token_ref
            .as_ref()
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "git_token",
            })?;
    read_secret(alias, "git_token", backend, reference)
}

pub fn read_account_git_token_secret_for_recovery(
    record: &AccountRecord,
    alias: &str,
    backend: &dyn SecretBackend,
) -> Result<String, AccountSecretStoreError> {
    let reference =
        record
            .git_token_ref
            .as_ref()
            .ok_or_else(|| AccountSecretStoreError::Missing {
                alias: alias.to_string(),
                category: "git_token",
            })?;
    read_secret_for_recovery(alias, "git_token", backend, reference)
}

fn write_secret(
    alias: &str,
    category: &'static str,
    backend: &dyn SecretBackend,
    reference: &SecretReference,
    value: &str,
) -> Result<(), AccountSecretStoreError> {
    backend
        .write_secret(reference, value)
        .map_err(|_| AccountSecretStoreError::WriteFailed {
            alias: alias.to_string(),
            category,
        })
}

fn write_secret_tracked(
    alias: &str,
    category: &'static str,
    backend: &dyn SecretBackend,
    reference: &SecretReference,
    value: &str,
    journal: &mut AccountSecretWriteJournal,
) -> Result<(), AccountSecretStoreError> {
    journal.capture(alias, category, backend, reference)?;
    write_secret(alias, category, backend, reference, value)
}

fn read_secret(
    alias: &str,
    category: &'static str,
    backend: &dyn SecretBackend,
    reference: &SecretReference,
) -> Result<String, AccountSecretStoreError> {
    normalize_secret_read(backend.read_secret(reference), alias, category, false)
}

fn read_secret_for_recovery(
    alias: &str,
    category: &'static str,
    backend: &dyn SecretBackend,
    reference: &SecretReference,
) -> Result<String, AccountSecretStoreError> {
    normalize_secret_read(backend.read_secret(reference), alias, category, true)
}

fn normalize_secret_read(
    result: Result<String, SecretBackendError>,
    alias: &str,
    category: &'static str,
    recover_missing: bool,
) -> Result<String, AccountSecretStoreError> {
    let missing = || AccountSecretStoreError::Missing {
        alias: alias.to_string(),
        category,
    };
    let value = result.map_err(|error| {
        if recover_missing && matches!(error, SecretBackendError::SecretNotFound { .. }) {
            missing()
        } else {
            AccountSecretStoreError::ReadFailed {
                alias: alias.to_string(),
                category,
            }
        }
    })?;
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(missing())
    } else {
        Ok(value)
    }
}
