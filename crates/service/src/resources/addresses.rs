use std::time::Duration;

use overleaf_storage::{
    save_addresses_document, AddressAddOutcome, AddressRecord, AddressStore, AddressesDocument,
};
use serde::Serialize;
use serde_json::{json, Value};

pub const MEIGUODIZHI_API_URL: &str = "https://www.meiguodizhi.com/api/v1/dz";
const MEIGUODIZHI_TIMEOUT_SECONDS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressSummary {
    pub index: usize,
    pub name: String,
    pub line1: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    pub phone: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressSelection {
    pub index: usize,
    pub address: AddressRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressFetchSource {
    Api,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressFetchReport {
    pub source: AddressFetchSource,
    pub index: usize,
    pub address: AddressRecord,
    pub api_error: Option<AddressActionError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationAddressSource {
    Api,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistrationAddressSelection {
    pub source: RegistrationAddressSource,
    pub fallback_index: Option<usize>,
    pub address: AddressRecord,
    pub api_error: Option<AddressActionError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddressActionError {
    Io {
        message: String,
    },
    Json {
        message: String,
    },
    Network {
        message: String,
    },
    InvalidApiResponse,
    MissingRequiredFields,
    EmptyAddressBook,
    AddressIndexOutOfRange {
        index: usize,
        len: usize,
    },
    AddressUnavailable {
        api_message: String,
        fallback_message: String,
    },
}

#[async_trait::async_trait]
pub trait RegistrationAddressProvider {
    async fn fetch_address(&self) -> Result<AddressRecord, AddressActionError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestMeiguodizhiAddressProvider;

pub fn list_address_summaries(document: &AddressesDocument) -> Vec<AddressSummary> {
    document
        .addresses
        .iter()
        .enumerate()
        .map(|(index, address)| address_summary(index, address))
        .collect()
}

pub fn add_or_select_address_in_store(
    store: &AddressStore,
    record: AddressRecord,
) -> Result<AddressSelection, AddressActionError> {
    if !record.has_required_fields() {
        return Err(AddressActionError::MissingRequiredFields);
    }

    let mut document = store.load().map_err(AddressActionError::from_io)?;
    let outcome = document.add_address(record.clone());
    if outcome == AddressAddOutcome::Added {
        save_addresses_document(store.path(), &document).map_err(AddressActionError::from_io)?;
    }

    let Some(index) = document
        .addresses
        .iter()
        .position(|address| same_address(address, &record))
    else {
        return Err(AddressActionError::InvalidApiResponse);
    };

    Ok(AddressSelection {
        index,
        address: document.addresses[index].clone(),
    })
}

pub fn select_fallback_address(
    document: &AddressesDocument,
    selector: usize,
) -> Result<AddressSelection, AddressActionError> {
    if document.addresses.is_empty() {
        return Err(AddressActionError::EmptyAddressBook);
    }

    let index = selector % document.addresses.len();
    let address = document.addresses[index].clone();
    if !address.has_required_fields() {
        return Err(AddressActionError::MissingRequiredFields);
    }

    Ok(AddressSelection { index, address })
}

pub fn select_address_by_index(
    document: &AddressesDocument,
    index: usize,
) -> Result<AddressSelection, AddressActionError> {
    let Some(address) = document.addresses.get(index).cloned() else {
        return Err(AddressActionError::AddressIndexOutOfRange {
            index,
            len: document.addresses.len(),
        });
    };
    if !address.has_required_fields() {
        return Err(AddressActionError::MissingRequiredFields);
    }

    Ok(AddressSelection { index, address })
}

pub fn select_fallback_address_in_store(
    store: &AddressStore,
    selector: usize,
) -> Result<AddressSelection, AddressActionError> {
    let document = store.load().map_err(AddressActionError::from_io)?;
    select_fallback_address(&document, selector)
}

pub async fn select_registration_address(
    provider: &(dyn RegistrationAddressProvider + Send + Sync),
    fallback_store: &AddressStore,
    fallback_selector: usize,
) -> Result<RegistrationAddressSelection, AddressActionError> {
    match provider.fetch_address().await {
        Ok(address) => Ok(RegistrationAddressSelection {
            source: RegistrationAddressSource::Api,
            fallback_index: None,
            address,
            api_error: None,
        }),
        Err(api_error) => match select_fallback_address_in_store(fallback_store, fallback_selector)
        {
            Ok(selection) => Ok(RegistrationAddressSelection {
                source: RegistrationAddressSource::Fallback,
                fallback_index: Some(selection.index),
                address: selection.address,
                api_error: Some(api_error),
            }),
            Err(fallback_error) => Err(AddressActionError::AddressUnavailable {
                api_message: address_action_error_message(&api_error),
                fallback_message: address_action_error_message(&fallback_error),
            }),
        },
    }
}

pub async fn fetch_address_into_store(
    provider: &(dyn RegistrationAddressProvider + Send + Sync),
    fallback_store: &AddressStore,
    fallback_selector: usize,
) -> Result<AddressFetchReport, AddressActionError> {
    match provider.fetch_address().await {
        Ok(address) => {
            let selection = add_or_select_address_in_store(fallback_store, address)?;
            Ok(AddressFetchReport {
                source: AddressFetchSource::Api,
                index: selection.index,
                address: selection.address,
                api_error: None,
            })
        }
        Err(api_error) => match select_fallback_address_in_store(fallback_store, fallback_selector)
        {
            Ok(selection) => Ok(AddressFetchReport {
                source: AddressFetchSource::Fallback,
                index: selection.index,
                address: selection.address,
                api_error: Some(api_error),
            }),
            Err(fallback_error) => Err(AddressActionError::AddressUnavailable {
                api_message: address_action_error_message(&api_error),
                fallback_message: address_action_error_message(&fallback_error),
            }),
        },
    }
}

pub fn meiguodizhi_request_payload() -> Value {
    json!({
        "path": "/",
        "method": "address",
    })
}

#[async_trait::async_trait]
impl RegistrationAddressProvider for ReqwestMeiguodizhiAddressProvider {
    async fn fetch_address(&self) -> Result<AddressRecord, AddressActionError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(MEIGUODIZHI_TIMEOUT_SECONDS))
            .no_proxy()
            .build()
            .map_err(AddressActionError::from_network)?;
        let response = client
            .post(MEIGUODIZHI_API_URL)
            .header("Content-Type", "application/json")
            .header("Sec-CH-UA-Platform", "\"Windows\"")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36",
            )
            .header(
                "Sec-CH-UA",
                "\"Google Chrome\";v=\"149\", \"Chromium\";v=\"149\", \"Not)A;Brand\";v=\"24\"",
            )
            .header("Sec-CH-UA-Mobile", "?0")
            .header("Accept", "*/*")
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .header("Origin", "https://www.meiguodizhi.com")
            .header("Referer", "https://www.meiguodizhi.com/")
            .json(&meiguodizhi_request_payload())
            .send()
            .await
            .map_err(AddressActionError::from_network)?;

        if !response.status().is_success() {
            return Err(AddressActionError::Network {
                message: format!("meiguodizhi API returned HTTP {}", response.status()),
            });
        }

        let body = response
            .text()
            .await
            .map_err(AddressActionError::from_network)?;
        address_from_meiguodizhi_response(&body)
    }
}

pub fn address_from_meiguodizhi_response(input: &str) -> Result<AddressRecord, AddressActionError> {
    let value: Value = serde_json::from_str(input).map_err(AddressActionError::from_json)?;
    if value.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(AddressActionError::InvalidApiResponse);
    }

    let address = value
        .get("address")
        .and_then(Value::as_object)
        .ok_or(AddressActionError::InvalidApiResponse)?;
    let record = AddressRecord {
        name: field(address, "Full_Name"),
        line1: field(address, "Address"),
        city: field(address, "City"),
        state: field(address, "State"),
        zip: field(address, "Zip_Code"),
        phone: optional_field(address, "Telephone"),
    };

    if !record.has_required_fields() {
        return Err(AddressActionError::MissingRequiredFields);
    }

    Ok(record)
}

fn address_summary(index: usize, address: &AddressRecord) -> AddressSummary {
    AddressSummary {
        index,
        name: address.name.clone(),
        line1: address.line1.clone(),
        city: address.city.clone(),
        state: address.state.clone(),
        zip: address.zip.clone(),
        phone: address
            .phone
            .clone()
            .filter(|value| !value.trim().is_empty()),
        complete: address.has_required_fields(),
    }
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

fn field(address: &serde_json::Map<String, Value>, key: &str) -> String {
    address
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn optional_field(address: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    let value = field(address, key);
    (!value.is_empty()).then_some(value)
}

pub fn address_action_error_message(error: &AddressActionError) -> String {
    match error {
        AddressActionError::Io { message }
        | AddressActionError::Json { message }
        | AddressActionError::Network { message } => message.clone(),
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

impl AddressActionError {
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

    fn from_network(error: reqwest::Error) -> Self {
        Self::Network {
            message: error.to_string(),
        }
    }
}
