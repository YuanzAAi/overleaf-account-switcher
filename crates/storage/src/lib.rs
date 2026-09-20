use std::env;
use std::path::{Path, PathBuf};

pub mod accounts;
pub mod addresses;
pub mod cards;
mod json_file;
pub mod secrets;

pub use json_file::save_json_atomic;

pub use accounts::{
    save_accounts_document, save_accounts_exchange_document, AccountImportOutcome, AccountRecord,
    AccountStore, AccountsDocument, SCHEMA_VERSION,
};
pub use addresses::{
    save_addresses_document, AddressAddOutcome, AddressRecord, AddressStore, AddressesDocument,
};
pub use cards::{
    parse_card_line, save_cards_document, CardAddOutcome, CardRecord, CardStore, CardsDocument,
};
pub use secrets::{
    account_cookie_secret_key, account_git_token_secret_key, account_password_secret_key,
    card_cvc_secret_key, card_number_secret_key, card_secret_fingerprint,
    current_secret_storage_status, validate_secret_reference, SecretBackend, SecretBackendError,
    SecretReference, SecretStorageStatus, SystemKeyringSecretBackend, SECRET_KEYRING_SERVICE,
    SECRET_REFERENCE_VERSION,
};

pub const DATA_DIR_ENV: &str = "OVERLEAF_SWITCHER_DATA_DIR";
pub const TMP_DIR_ENV: &str = "OVERLEAF_SWITCHER_TMP_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    pub data_dir: PathBuf,
    pub tmp_dir: PathBuf,
}

impl StorageLayout {
    pub fn from_environment() -> Self {
        Self::new(data_dir(), tmp_dir())
    }

    pub fn new(data_dir: impl Into<PathBuf>, tmp_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            tmp_dir: tmp_dir.into(),
        }
    }

    pub fn accounts_dir(&self) -> PathBuf {
        accounts_dir_in(&self.data_dir)
    }

    pub fn accounts_path(&self) -> PathBuf {
        self.accounts_dir().join("accounts.json")
    }

    pub fn cards_dir(&self) -> PathBuf {
        cards_dir_in(&self.data_dir)
    }

    pub fn cards_path(&self) -> PathBuf {
        self.cards_dir().join("cards.json")
    }

    pub fn addresses_dir(&self) -> PathBuf {
        addresses_dir_in(&self.data_dir)
    }

    pub fn addresses_path(&self) -> PathBuf {
        self.addresses_dir().join("addresses.json")
    }

    pub fn sqlite_dir(&self) -> PathBuf {
        sqlite_dir_in(&self.data_dir)
    }

    pub fn exports_dir(&self) -> PathBuf {
        exports_dir_in(&self.data_dir)
    }

    pub fn required_data_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.data_dir.clone(),
            self.accounts_dir(),
            self.cards_dir(),
            self.addresses_dir(),
            self.exports_dir(),
        ]
    }
}

pub fn data_dir() -> PathBuf {
    env::var_os(DATA_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_storage_root().join("data"))
}

pub fn tmp_dir() -> PathBuf {
    env::var_os(TMP_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_storage_root().join("tmp"))
}

pub fn default_storage_root() -> PathBuf {
    user_data_base_dir().join(".OverleafAccountSwitcher")
}

#[cfg(windows)]
fn user_data_base_dir() -> PathBuf {
    if let Some(path) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let (Some(drive), Some(path)) = (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        return PathBuf::from(drive).join(path);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

#[cfg(not(windows))]
fn user_data_base_dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(env::temp_dir)
}

pub fn accounts_json_path() -> PathBuf {
    StorageLayout::from_environment().accounts_path()
}

pub fn cards_json_path() -> PathBuf {
    StorageLayout::from_environment().cards_path()
}

pub fn addresses_json_path() -> PathBuf {
    StorageLayout::from_environment().addresses_path()
}

pub fn sqlite_dir_path() -> PathBuf {
    StorageLayout::from_environment().sqlite_dir()
}

pub fn exports_dir_path() -> PathBuf {
    StorageLayout::from_environment().exports_dir()
}

pub fn accounts_json_path_in(data_dir: impl AsRef<Path>) -> PathBuf {
    accounts_dir_in(data_dir).join("accounts.json")
}

pub fn cards_json_path_in(data_dir: impl AsRef<Path>) -> PathBuf {
    cards_dir_in(data_dir).join("cards.json")
}

pub fn addresses_json_path_in(data_dir: impl AsRef<Path>) -> PathBuf {
    addresses_dir_in(data_dir).join("addresses.json")
}

pub fn accounts_dir_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("accounts")
}

pub fn cards_dir_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("cards")
}

pub fn addresses_dir_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("addresses")
}

pub fn sqlite_dir_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("sqlite")
}

pub fn exports_dir_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("exports")
}
