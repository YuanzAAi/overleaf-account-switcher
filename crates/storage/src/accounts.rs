use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use overleaf_core::{make_alias, normalize_email, Account, AccountId, SubscriptionState};
use serde::{Deserialize, Serialize};

use crate::secrets::SecretReference;

pub const SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountsDocument {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub extension_profile_clients: BTreeMap<String, String>,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountRecord {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cookies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cookie_refs: BTreeMap<String, SecretReference>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<SecretReference>,
    #[serde(default)]
    pub cookie_expiry: Option<f64>,
    #[serde(default)]
    pub cookie_updated_at: Option<f64>,
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub last_login_at: Option<f64>,
    #[serde(default)]
    pub trial_days: Option<u32>,
    #[serde(default)]
    pub trial_started_at: Option<f64>,
    #[serde(default)]
    pub trial_expiry: Option<f64>,
    #[serde(default)]
    pub subscription_status: Option<String>,
    #[serde(default)]
    pub subscription_label: Option<String>,
    #[serde(default)]
    pub subscription_checked_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_token_ref: Option<SecretReference>,
    #[serde(default)]
    pub git_token_expiry: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountImportOutcome {
    Imported {
        alias: String,
    },
    SkippedDuplicateEmail {
        alias: String,
        existing_alias: String,
    },
    AliasConflict {
        alias: String,
    },
}

#[derive(Debug, Clone)]
pub struct AccountStore {
    path: PathBuf,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for AccountsDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            current: None,
            extension_profile_clients: BTreeMap::new(),
            accounts: BTreeMap::new(),
        }
    }
}

impl AccountsDocument {
    pub fn from_json_str(input: &str) -> serde_json::Result<Self> {
        serde_json::from_str(input)
    }

    /// 仅用于显式导出；内部存储必须使用拒绝明文的保存函数。
    pub fn to_exchange_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// 明文仅允许作为导入导出的交换数据。
    pub fn contains_plaintext_secrets(&self) -> bool {
        self.accounts.values().any(|record| {
            record.password.is_some() || !record.cookies.is_empty() || record.git_token.is_some()
        })
    }

    pub fn duplicate_alias_by_email(&self, email: &str) -> Option<&str> {
        let expected = normalize_email(email);
        self.accounts.iter().find_map(|(alias, record)| {
            record.email.as_deref().and_then(|candidate| {
                (normalize_email(candidate) == expected).then_some(alias.as_str())
            })
        })
    }

    pub fn alias_conflicts_with_email(&self, alias: &str, email: &str) -> bool {
        let Some(existing) = self.accounts.get(alias) else {
            return false;
        };

        existing
            .email
            .as_deref()
            .map(|existing_email| normalize_email(existing_email) != normalize_email(email))
            .unwrap_or(true)
    }

    pub fn import_record(
        &mut self,
        alias_hint: Option<&str>,
        record: AccountRecord,
    ) -> AccountImportOutcome {
        let alias = make_alias(alias_hint, record.email.as_deref().unwrap_or_default());

        if let Some(email) = record.email.as_deref() {
            if let Some(existing_alias) = self.duplicate_alias_by_email(email) {
                return AccountImportOutcome::SkippedDuplicateEmail {
                    alias,
                    existing_alias: existing_alias.to_string(),
                };
            }
        }

        if self.accounts.contains_key(&alias) {
            return AccountImportOutcome::AliasConflict { alias };
        }

        self.accounts.insert(alias.clone(), record);
        AccountImportOutcome::Imported { alias }
    }

    pub fn selected_document<I, S>(&self, aliases: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeMap::new();
        for alias in aliases {
            let alias = alias.as_ref();
            let Some(record) = self.accounts.get(alias) else {
                return Err(format!("account alias not found: {alias}"));
            };
            selected.insert(alias.to_string(), record.clone());
        }

        let current = self
            .current
            .as_ref()
            .filter(|alias| selected.contains_key(*alias))
            .cloned();

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            current,
            extension_profile_clients: BTreeMap::new(),
            accounts: selected,
        })
    }
}

impl AccountRecord {
    pub fn trial_duration_days(&self) -> Option<u32> {
        let start = self.trial_started_at?;
        let end = self.trial_expiry?;
        if !start.is_finite() || !end.is_finite() || start <= 0.0 || end <= start {
            return None;
        }
        // Registration and payment timestamps can differ by a few minutes.
        Some(((end - start) / 86_400.0).round().max(1.0) as u32)
    }

    pub fn set_trial_expiry(&mut self, expiry: Option<f64>) {
        self.trial_expiry = expiry;
        self.trial_days = self.trial_duration_days();
    }

    pub fn to_domain(&self, alias: &str) -> Account {
        let id = self
            .user_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| alias.to_string());

        Account {
            id: AccountId(id),
            alias: alias.to_string(),
            email: self.email.clone().unwrap_or_default(),
            subscription_state: self.subscription_state(),
        }
    }

    pub fn subscription_state(&self) -> SubscriptionState {
        match self
            .subscription_status
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "trial" => SubscriptionState::Trial,
            "pro" | "subscription" => SubscriptionState::Pro,
            "free" => SubscriptionState::Free,
            _ => SubscriptionState::Unknown,
        }
    }
}

impl AccountStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<AccountsDocument> {
        load_accounts_document(&self.path)
    }

    pub fn save(&self, document: &AccountsDocument) -> io::Result<()> {
        save_accounts_document(&self.path, document)
    }
}

pub fn load_accounts_document(path: impl AsRef<Path>) -> io::Result<AccountsDocument> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(AccountsDocument::default());
    }

    let text = fs::read_to_string(path)?;
    let document = AccountsDocument::from_json_str(&text).map_err(invalid_data)?;
    ensure_internal_secret_boundary(&document)?;
    Ok(document)
}

pub fn save_accounts_document(
    path: impl AsRef<Path>,
    document: &AccountsDocument,
) -> io::Result<()> {
    ensure_internal_secret_boundary(document)?;
    crate::save_json_atomic(path, document)
}

/// 写入用户显式请求的明文导出文件。
pub fn save_accounts_exchange_document(
    path: impl AsRef<Path>,
    document: &AccountsDocument,
) -> io::Result<()> {
    crate::save_json_atomic(path, document)
}

fn ensure_internal_secret_boundary(document: &AccountsDocument) -> io::Result<()> {
    if document.contains_plaintext_secrets() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "internal accounts document contains plaintext secrets; use explicit import/export exchange flow",
        ));
    }
    Ok(())
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
