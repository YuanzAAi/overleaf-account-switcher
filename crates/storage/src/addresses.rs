use overleaf_core::Address;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AddressesDocument {
    #[serde(default)]
    pub addresses: Vec<AddressRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressRecord {
    pub name: String,
    pub line1: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    #[serde(default)]
    pub phone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressAddOutcome {
    Added,
    Duplicate,
    MissingRequiredFields,
}

#[derive(Debug, Clone)]
pub struct AddressStore {
    path: PathBuf,
}

impl AddressesDocument {
    pub fn from_json_str(input: &str) -> serde_json::Result<Self> {
        serde_json::from_str(input)
    }

    pub fn to_pretty_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn add_address(&mut self, record: AddressRecord) -> AddressAddOutcome {
        if !record.has_required_fields() {
            return AddressAddOutcome::MissingRequiredFields;
        }

        if self.contains_address(&record) {
            return AddressAddOutcome::Duplicate;
        }

        self.addresses.push(record);
        AddressAddOutcome::Added
    }

    pub fn contains_address(&self, record: &AddressRecord) -> bool {
        self.addresses
            .iter()
            .any(|address| same_address(address, record))
    }
}

impl AddressRecord {
    pub fn to_domain(&self) -> Address {
        Address {
            name: self.name.clone(),
            line1: self.line1.clone(),
            city: self.city.clone(),
            state: self.state.clone(),
            zip: self.zip.clone(),
            phone: self.phone.clone().filter(|value| !value.trim().is_empty()),
        }
    }

    pub fn has_required_fields(&self) -> bool {
        self.to_domain().has_required_fields()
    }
}

impl AddressStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<AddressesDocument> {
        load_addresses_document(&self.path)
    }

    pub fn save(&self, document: &AddressesDocument) -> io::Result<()> {
        save_addresses_document(&self.path, document)
    }
}

pub fn load_addresses_document(path: impl AsRef<Path>) -> io::Result<AddressesDocument> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(AddressesDocument::default());
    }

    let text = fs::read_to_string(path)?;
    AddressesDocument::from_json_str(&text).map_err(invalid_data)
}

pub fn save_addresses_document(
    path: impl AsRef<Path>,
    document: &AddressesDocument,
) -> io::Result<()> {
    crate::save_json_atomic(path, document)
}

fn same_address(left: &AddressRecord, right: &AddressRecord) -> bool {
    normalized(&left.line1) == normalized(&right.line1)
        && normalized(&left.city) == normalized(&right.city)
        && normalized(&left.state) == normalized(&right.state)
        && normalized(&left.zip) == normalized(&right.zip)
}

fn normalized(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
