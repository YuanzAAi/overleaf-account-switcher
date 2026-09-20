use overleaf_core::{Card, CardStatus};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::secrets::SecretReference;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CardsDocument {
    #[serde(default)]
    pub cards: Vec<CardRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub number: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_ref: Option<SecretReference>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub number_bin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub number_last_four: String,
    pub exp_month: String,
    pub exp_year: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cvc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvc_ref: Option<SecretReference>,
    #[serde(default = "default_card_status")]
    pub status: String,
    #[serde(default)]
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardAddOutcome {
    Added,
    Duplicate,
}

#[derive(Debug, Clone)]
pub struct CardStore {
    path: PathBuf,
}

fn default_card_status() -> String {
    "new".to_string()
}

impl CardsDocument {
    pub fn from_json_str(input: &str) -> serde_json::Result<Self> {
        serde_json::from_str(input)
    }

    /// 检查是否含明文卡片敏感字段。
    pub fn contains_plaintext_secrets(&self) -> bool {
        self.cards
            .iter()
            .any(|card| !card.number.is_empty() || !card.cvc.is_empty())
    }

    pub fn contains_number(&self, number: &str) -> bool {
        self.cards.iter().any(|card| card.number == number)
    }

    pub fn add_card(&mut self, record: CardRecord) -> CardAddOutcome {
        if self.contains_number(&record.number) {
            return CardAddOutcome::Duplicate;
        }

        self.cards.push(record);
        CardAddOutcome::Added
    }

    pub fn mark_used(&mut self, number: &str) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.number == number) else {
            return false;
        };

        card.status = "used".to_string();
        true
    }

    pub fn mark_failed(&mut self, number: &str, error: &str) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.number == number) else {
            return false;
        };

        card.status = "failed".to_string();
        card.last_error = error.to_string();
        true
    }

    pub fn remove_by_number_or_suffix(&mut self, number_or_suffix: &str) -> bool {
        let Some(index) = self.cards.iter().position(|card| {
            card.number == number_or_suffix || card.number.ends_with(number_or_suffix)
        }) else {
            return false;
        };

        self.cards.remove(index);
        true
    }
}

impl CardRecord {
    pub fn to_domain(&self) -> Card {
        Card {
            number: self.number.clone(),
            exp_month: self.exp_month.clone(),
            exp_year: self.exp_year.clone(),
            cvc: self.cvc.clone(),
            status: self.status_as_domain(),
            last_error: (!self.last_error.is_empty()).then_some(self.last_error.clone()),
        }
    }

    pub fn status_as_domain(&self) -> CardStatus {
        match self.status.trim().to_ascii_lowercase().as_str() {
            "used" => CardStatus::Used,
            "failed" => CardStatus::Failed,
            _ => CardStatus::New,
        }
    }
}

pub fn parse_card_line(input: &str) -> Result<CardRecord, String> {
    let normalized = input.replace("----", "|").replace('/', "|");
    let parts: Vec<_> = normalized.split('|').map(str::trim).collect();
    if parts.len() != 4 || parts.iter().any(|part| part.is_empty()) {
        return Err("银行卡格式应为：卡号----月份/年份----CVC".to_string());
    }
    if !parts[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("卡号只能包含数字".to_string());
    }

    Ok(CardRecord {
        number: parts[0].to_string(),
        number_ref: None,
        number_bin: first_six(parts[0]).to_string(),
        number_last_four: last_four(parts[0]).to_string(),
        exp_month: parts[1].to_string(),
        exp_year: parts[2].to_string(),
        cvc: parts[3].to_string(),
        cvc_ref: None,
        status: default_card_status(),
        last_error: String::new(),
    })
}

fn last_four(number: &str) -> &str {
    let len = number.len();
    if len <= 4 {
        number
    } else {
        &number[len - 4..]
    }
}

fn first_six(number: &str) -> &str {
    let len = number.len();
    if len <= 6 {
        number
    } else {
        &number[..6]
    }
}

impl CardStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<CardsDocument> {
        load_cards_document(&self.path)
    }

    pub fn save(&self, document: &CardsDocument) -> io::Result<()> {
        save_cards_document(&self.path, document)
    }
}

pub fn load_cards_document(path: impl AsRef<Path>) -> io::Result<CardsDocument> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(CardsDocument::default());
    }

    let text = fs::read_to_string(path)?;
    let document = CardsDocument::from_json_str(&text).map_err(invalid_data)?;
    ensure_internal_secret_boundary(&document)?;
    Ok(document)
}

pub fn save_cards_document(path: impl AsRef<Path>, document: &CardsDocument) -> io::Result<()> {
    ensure_internal_secret_boundary(document)?;
    crate::save_json_atomic(path, document)
}

fn ensure_internal_secret_boundary(document: &CardsDocument) -> io::Result<()> {
    if document.contains_plaintext_secrets() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "internal cards document contains plaintext secrets; use explicit card input flow",
        ));
    }
    Ok(())
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
