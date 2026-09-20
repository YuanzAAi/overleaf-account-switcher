use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

pub const SECRET_REFERENCE_VERSION: &str = "secret_ref_v1";
pub const SECRET_KEYRING_SERVICE: &str = "overleaf-account-switcher";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretStorageStatus {
    pub mode: &'static str,
    pub encrypted: bool,
    pub backend: &'static str,
    pub sensitive_fields: Vec<SecretFieldSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretFieldSpec {
    pub store: &'static str,
    pub json_path: &'static str,
    pub category: &'static str,
    pub target_store: &'static str,
    pub target_key_template: &'static str,
    pub ref_json_field: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretReference {
    pub version: String,
    pub backend: String,
    pub key: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretBackendError {
    BackendUnavailable {
        backend: String,
        message: String,
    },
    InvalidReference {
        expected_backend: String,
        actual_backend: String,
        key: String,
    },
    SecretNotFound {
        key: String,
    },
    OperationFailed {
        operation: &'static str,
        key: String,
        message: String,
    },
}

pub trait SecretBackend {
    fn backend_id(&self) -> &str;
    fn write_secret(
        &self,
        reference: &SecretReference,
        value: &str,
    ) -> Result<(), SecretBackendError>;
    fn read_secret(&self, reference: &SecretReference) -> Result<String, SecretBackendError>;
    fn delete_secret(&self, reference: &SecretReference) -> Result<(), SecretBackendError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemKeyringSecretBackend {
    service: String,
}

impl SecretStorageStatus {
    pub fn ref_secret_backend() -> Self {
        Self {
            mode: "ref_secret_backend",
            encrypted: true,
            backend: Self::ref_backend_id(),
            sensitive_fields: internal_ref_secret_fields(),
        }
    }

    pub fn ref_backend_id() -> &'static str {
        "system_keyring_or_encrypted_store"
    }
}

impl SecretReference {
    pub fn new(category: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            version: SECRET_REFERENCE_VERSION.to_string(),
            backend: SecretStorageStatus::ref_backend_id().to_string(),
            key: key.into(),
            category: category.into(),
        }
    }
}

impl fmt::Display for SecretBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendUnavailable { backend, message } => {
                write!(formatter, "secret backend {backend} is unavailable: {message}")
            }
            Self::InvalidReference {
                expected_backend,
                actual_backend,
                key,
            } => write!(
                formatter,
                "secret reference {key} targets backend {actual_backend}, expected {expected_backend}"
            ),
            Self::SecretNotFound { key } => write!(formatter, "secret not found: {key}"),
            Self::OperationFailed {
                operation,
                key,
                message,
            } => write!(formatter, "secret {operation} failed for {key}: {message}"),
        }
    }
}

impl Error for SecretBackendError {}

impl Default for SystemKeyringSecretBackend {
    fn default() -> Self {
        Self::new(SECRET_KEYRING_SERVICE)
    }
}

impl SystemKeyringSecretBackend {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service
    }
}

impl SecretBackend for SystemKeyringSecretBackend {
    fn backend_id(&self) -> &str {
        SecretStorageStatus::ref_backend_id()
    }

    fn write_secret(
        &self,
        reference: &SecretReference,
        value: &str,
    ) -> Result<(), SecretBackendError> {
        validate_secret_reference(self.backend_id(), reference)?;
        system_keyring_set_password(&self.service, reference, value)
    }

    fn read_secret(&self, reference: &SecretReference) -> Result<String, SecretBackendError> {
        validate_secret_reference(self.backend_id(), reference)?;
        system_keyring_get_password(&self.service, reference)
    }

    fn delete_secret(&self, reference: &SecretReference) -> Result<(), SecretBackendError> {
        validate_secret_reference(self.backend_id(), reference)?;
        system_keyring_delete_password(&self.service, reference)
    }
}

pub fn validate_secret_reference(
    expected_backend: &str,
    reference: &SecretReference,
) -> Result<(), SecretBackendError> {
    if reference.backend == expected_backend {
        Ok(())
    } else {
        Err(SecretBackendError::InvalidReference {
            expected_backend: expected_backend.to_string(),
            actual_backend: reference.backend.clone(),
            key: reference.key.clone(),
        })
    }
}

fn system_keyring_set_password(
    service: &str,
    reference: &SecretReference,
    value: &str,
) -> Result<(), SecretBackendError> {
    let entry = keyring::Entry::new(service, &reference.key)
        .map_err(|error| keyring_operation_error("open", &reference.key, error))?;
    entry
        .set_password(value)
        .map_err(|error| keyring_operation_error("write", &reference.key, error))
}

fn system_keyring_get_password(
    service: &str,
    reference: &SecretReference,
) -> Result<String, SecretBackendError> {
    let entry = keyring::Entry::new(service, &reference.key)
        .map_err(|error| keyring_operation_error("open", &reference.key, error))?;
    entry
        .get_password()
        .map_err(|error| keyring_operation_error("read", &reference.key, error))
}

fn system_keyring_delete_password(
    service: &str,
    reference: &SecretReference,
) -> Result<(), SecretBackendError> {
    let entry = keyring::Entry::new(service, &reference.key)
        .map_err(|error| keyring_operation_error("open", &reference.key, error))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_operation_error("delete", &reference.key, error)),
    }
}

fn keyring_operation_error(
    operation: &'static str,
    key: &str,
    error: keyring::Error,
) -> SecretBackendError {
    if matches!(error, keyring::Error::NoEntry) {
        SecretBackendError::SecretNotFound {
            key: key.to_string(),
        }
    } else {
        SecretBackendError::OperationFailed {
            operation,
            key: key.to_string(),
            message: format!("{error:?}"),
        }
    }
}

pub fn secret_key_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.trim().as_bytes() {
        let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@');
        if allowed {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("_x{byte:02X}_"));
        }
    }
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

pub fn account_password_secret_key(alias: &str) -> String {
    format!("account:{}:password", secret_key_component(alias))
}

pub fn account_cookie_secret_key(alias: &str, cookie_name: &str) -> String {
    format!(
        "account:{}:cookie:{}",
        secret_key_component(alias),
        secret_key_component(cookie_name)
    )
}

pub fn account_git_token_secret_key(alias: &str) -> String {
    format!("account:{}:git_token", secret_key_component(alias))
}

pub fn card_secret_fingerprint(index: usize) -> String {
    format!("idx{}", index + 1)
}

pub fn card_number_secret_key(fingerprint: &str) -> String {
    format!("card:{}:number", secret_key_component(fingerprint))
}

pub fn card_cvc_secret_key(fingerprint: &str) -> String {
    format!("card:{}:cvc", secret_key_component(fingerprint))
}

pub fn current_secret_storage_status() -> SecretStorageStatus {
    SecretStorageStatus::ref_secret_backend()
}

pub fn internal_ref_secret_fields() -> Vec<SecretFieldSpec> {
    vec![
        SecretFieldSpec {
            store: "accounts.json",
            json_path: "$.accounts.*.password",
            category: "password",
            target_store: SecretStorageStatus::ref_backend_id(),
            target_key_template: "account:{alias}:password",
            ref_json_field: "password_ref",
        },
        SecretFieldSpec {
            store: "accounts.json",
            json_path: "$.accounts.*.cookies.*",
            category: "cookie",
            target_store: SecretStorageStatus::ref_backend_id(),
            target_key_template: "account:{alias}:cookie:{cookie_name}",
            ref_json_field: "cookie_ref",
        },
        SecretFieldSpec {
            store: "accounts.json",
            json_path: "$.accounts.*.git_token",
            category: "git_token",
            target_store: SecretStorageStatus::ref_backend_id(),
            target_key_template: "account:{alias}:git_token",
            ref_json_field: "git_token_ref",
        },
        SecretFieldSpec {
            store: "data/cards.json",
            json_path: "$.cards[].number",
            category: "card_number",
            target_store: SecretStorageStatus::ref_backend_id(),
            target_key_template: "card:{fingerprint}:number",
            ref_json_field: "number_ref",
        },
        SecretFieldSpec {
            store: "data/cards.json",
            json_path: "$.cards[].cvc",
            category: "card_cvc",
            target_store: SecretStorageStatus::ref_backend_id(),
            target_key_template: "card:{fingerprint}:cvc",
            ref_json_field: "cvc_ref",
        },
    ]
}
