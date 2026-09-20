use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use overleaf_core::card::looks_like_payment_card_number;

use overleaf_storage::{
    card_cvc_secret_key, card_number_secret_key, card_secret_fingerprint, parse_card_line,
    save_cards_document, CardRecord, CardStore, CardsDocument, SecretBackend, SecretReference,
};
use serde::{Deserialize, Serialize};

const HIDDEN_CARD_ERROR_VALUE: &str = "[hidden]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardSummary {
    pub id: String,
    pub index: usize,
    pub bin: String,
    pub masked_number: String,
    pub last_four: String,
    pub exp_month: String,
    pub exp_year: String,
    pub status: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardPaymentSecret {
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardAddDecisionStatus {
    Added,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardAddReport {
    pub status: CardAddDecisionStatus,
    pub summary: Option<CardSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardMutationReport {
    pub changed: bool,
    pub summary: Option<CardSummary>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardSelectionStrategy {
    #[default]
    NewThenUsedThenFailed,
    NewOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardActionError {
    Io { message: String },
    InvalidFormat { message: String },
    CardNotFound { number_or_suffix: String },
    AmbiguousCardSuffix { suffix: String, matches: usize },
    SecretReadFailed { field: &'static str },
    SecretWriteFailed { field: &'static str },
}

pub fn list_card_summaries(document: &CardsDocument) -> Vec<CardSummary> {
    document
        .cards
        .iter()
        .enumerate()
        .map(|(index, card)| card_summary(index, card))
        .collect()
}

pub(crate) fn export_cards_text(
    document: &CardsDocument,
    ids: &[String],
    backend: &dyn SecretBackend,
) -> Result<String, CardActionError> {
    if ids.is_empty() {
        return Err(CardActionError::InvalidFormat {
            message: "请先选择银行卡".to_string(),
        });
    }
    let mut lines = Vec::new();
    let mut seen = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(CardActionError::InvalidFormat {
                message: "银行卡编号不能为空".into(),
            });
        }
        if !seen.insert(id) {
            continue;
        }
        let card = document
            .cards
            .iter()
            .find(|card| card_id(card) == *id)
            .ok_or_else(|| CardActionError::CardNotFound {
                number_or_suffix: id.clone(),
            })?;
        let secret = payment_secret_with_backend(card, backend)?;
        lines.push(format!(
            "{}----{}/{}----{}",
            secret.number, secret.exp_month, secret.exp_year, secret.cvc
        ));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn card_id(card: &CardRecord) -> String {
    card.number_ref
        .as_ref()
        .map(|reference| reference.key.clone())
        .unwrap_or_default()
}

pub fn add_card_from_line_in_store_with_backend(
    store: &CardStore,
    input: &str,
    backend: &dyn SecretBackend,
) -> Result<CardAddReport, CardActionError> {
    let mut document = store.load().map_err(CardActionError::from_io)?;
    let mut record =
        parse_card_line(input).map_err(|message| CardActionError::InvalidFormat { message })?;
    let number = record.number.clone();

    match find_card_index_with_backend(&document, &number, backend) {
        Ok(index) => {
            return Ok(CardAddReport {
                status: CardAddDecisionStatus::Duplicate,
                summary: Some(card_summary(index, &document.cards[index])),
            })
        }
        Err(CardActionError::CardNotFound { .. }) => {}
        Err(error) => return Err(error),
    }

    let (number_ref, cvc_ref) = next_card_secret_refs(&document);
    backend
        .write_secret(&number_ref, &record.number)
        .map_err(|_| CardActionError::SecretWriteFailed {
            field: "card_number",
        })?;
    if let Err(_error) = backend.write_secret(&cvc_ref, &record.cvc) {
        let _ = backend.delete_secret(&number_ref);
        return Err(CardActionError::SecretWriteFailed { field: "card_cvc" });
    }

    record.number.clear();
    record.number_ref = Some(number_ref);
    record.cvc.clear();
    record.cvc_ref = Some(cvc_ref);
    let written_number_ref = record.number_ref.clone();
    let written_cvc_ref = record.cvc_ref.clone();
    document.cards.push(record);
    if let Err(error) = save_cards_document(store.path(), &document) {
        if let Some(number_ref) = &written_number_ref {
            let _ = backend.delete_secret(number_ref);
        }
        if let Some(cvc_ref) = &written_cvc_ref {
            let _ = backend.delete_secret(cvc_ref);
        }
        return Err(CardActionError::from_io(error));
    }
    let index = document.cards.len() - 1;
    Ok(CardAddReport {
        status: CardAddDecisionStatus::Added,
        summary: Some(card_summary(index, &document.cards[index])),
    })
}

pub fn remove_card_in_store_with_backend(
    store: &CardStore,
    number_or_suffix: &str,
    backend: &dyn SecretBackend,
) -> Result<CardMutationReport, CardActionError> {
    let mut document = store.load().map_err(CardActionError::from_io)?;
    let index = find_card_index_with_backend(&document, number_or_suffix, backend)?;
    let removed = document.cards.remove(index);
    let report = CardMutationReport {
        changed: true,
        summary: Some(card_summary(index, &removed)),
    };
    save_cards_document(store.path(), &document).map_err(CardActionError::from_io)?;
    if let Some(reference) = &removed.number_ref {
        let _ = backend.delete_secret(reference);
    }
    if let Some(reference) = &removed.cvc_ref {
        let _ = backend.delete_secret(reference);
    }
    Ok(report)
}

pub fn card_for_payment_with_backend(
    document: &CardsDocument,
    number_or_suffix: &str,
    backend: &dyn SecretBackend,
) -> Result<CardPaymentSecret, CardActionError> {
    let index = find_card_index_with_backend(document, number_or_suffix, backend)?;
    payment_secret_with_backend(&document.cards[index], backend)
}

pub fn select_payment_card_with_strategy_with_backend(
    document: &CardsDocument,
    attempted_numbers: &BTreeSet<String>,
    strategy: CardSelectionStrategy,
    backend: &dyn SecretBackend,
) -> Result<Option<CardPaymentSecret>, CardActionError> {
    select_payment_card_with_strategy_and_bin_with_backend(
        document,
        attempted_numbers,
        strategy,
        None,
        backend,
    )
}

pub fn select_payment_card_with_strategy_and_bin_with_backend(
    document: &CardsDocument,
    attempted_numbers: &BTreeSet<String>,
    strategy: CardSelectionStrategy,
    card_bin: Option<&str>,
    backend: &dyn SecretBackend,
) -> Result<Option<CardPaymentSecret>, CardActionError> {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default();
    select_payment_card_with_strategy_and_bin_with_backend_at(
        document,
        attempted_numbers,
        strategy,
        card_bin,
        backend,
        now_unix,
    )
}

pub fn select_payment_card_with_strategy_with_backend_at(
    document: &CardsDocument,
    attempted_numbers: &BTreeSet<String>,
    strategy: CardSelectionStrategy,
    backend: &dyn SecretBackend,
    now_unix: i64,
) -> Result<Option<CardPaymentSecret>, CardActionError> {
    select_payment_card_with_strategy_and_bin_with_backend_at(
        document,
        attempted_numbers,
        strategy,
        None,
        backend,
        now_unix,
    )
}

pub fn select_payment_card_with_strategy_and_bin_with_backend_at(
    document: &CardsDocument,
    attempted_numbers: &BTreeSet<String>,
    strategy: CardSelectionStrategy,
    card_bin: Option<&str>,
    backend: &dyn SecretBackend,
    now_unix: i64,
) -> Result<Option<CardPaymentSecret>, CardActionError> {
    let mut candidates: Vec<_> = document
        .cards
        .iter()
        .enumerate()
        .filter(|(_, card)| card_bin.is_none_or(|bin| card_display_bin(card) == bin))
        .filter(|(_, card)| payment_card_is_unexpired_at(card, now_unix))
        .filter(|(_, card)| match strategy {
            CardSelectionStrategy::NewThenUsedThenFailed => true,
            CardSelectionStrategy::NewOnly => normalize_status(&card.status) == "new",
        })
        .collect();
    candidates.sort_by_key(|(_, card)| card_status_priority(&card.status));

    for (_, card) in candidates {
        let secret = payment_secret_with_backend(card, backend)?;
        if !attempted_numbers.contains(&secret.number) {
            return Ok(Some(secret));
        }
    }

    Ok(None)
}

fn payment_card_is_unexpired_at(card: &CardRecord, now_unix: i64) -> bool {
    payment_card_expiry_end_unix(&card.exp_month, &card.exp_year)
        .is_some_and(|expiry_end| now_unix < expiry_end)
}

fn payment_card_expiry_end_unix(exp_month: &str, exp_year: &str) -> Option<i64> {
    let month = exp_month.trim().parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let parsed_year = exp_year.trim().parse::<i64>().ok()?;
    let year = if (0..=99).contains(&parsed_year) {
        2000 + parsed_year
    } else {
        parsed_year
    };
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let (next_year, next_month) = if month == 12 {
        (year.checked_add(1)?, 1)
    } else {
        (year, month + 1)
    };
    days_from_civil(next_year, next_month).checked_mul(86_400)
}

fn days_from_civil(year: i64, month: u32) -> i64 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month as i64 + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn mark_card_used_in_store_with_backend(
    store: &CardStore,
    number_or_suffix: &str,
    backend: &dyn SecretBackend,
) -> Result<CardMutationReport, CardActionError> {
    let mut document = store.load().map_err(CardActionError::from_io)?;
    let report = mark_card_used_with_backend(&mut document, number_or_suffix, backend)?;
    save_cards_document(store.path(), &document).map_err(CardActionError::from_io)?;
    Ok(report)
}

pub fn mark_card_failed_in_store_with_backend(
    store: &CardStore,
    number_or_suffix: &str,
    error: &str,
    backend: &dyn SecretBackend,
) -> Result<CardMutationReport, CardActionError> {
    let mut document = store.load().map_err(CardActionError::from_io)?;
    let report = mark_card_failed_with_backend(&mut document, number_or_suffix, error, backend)?;
    save_cards_document(store.path(), &document).map_err(CardActionError::from_io)?;
    Ok(report)
}

fn find_card_index_with_backend(
    document: &CardsDocument,
    number_or_suffix: &str,
    backend: &dyn SecretBackend,
) -> Result<usize, CardActionError> {
    let value = number_or_suffix.trim();
    if !value.is_empty() {
        if let Some(index) = document
            .cards
            .iter()
            .position(|card| card_id(card) == value)
        {
            return Ok(index);
        }
    }
    let mut matches = Vec::new();

    for (index, card) in document.cards.iter().enumerate() {
        let number = card_number_for_payment(card, backend)?;
        if number == value || (!value.is_empty() && number.ends_with(value)) {
            matches.push(index);
        }
    }

    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(CardActionError::CardNotFound {
            number_or_suffix: safe_card_lookup_value(value),
        }),
        _ => Err(CardActionError::AmbiguousCardSuffix {
            suffix: safe_card_lookup_value(value),
            matches: matches.len(),
        }),
    }
}

fn card_summary(index: usize, card: &CardRecord) -> CardSummary {
    let bin = card_display_bin(card).to_string();
    let last_four = card_display_last_four(card).to_string();
    let last_error = sanitize_card_error(&card.last_error);
    CardSummary {
        id: card_id(card),
        index,
        bin: bin.clone(),
        masked_number: masked_number_from_bin_and_last_four(&bin, &last_four),
        last_four,
        exp_month: card.exp_month.clone(),
        exp_year: card.exp_year.clone(),
        status: normalize_status(&card.status).to_string(),
        last_error: (!last_error.trim().is_empty()).then_some(last_error),
    }
}

fn payment_secret_with_backend(
    card: &CardRecord,
    backend: &dyn SecretBackend,
) -> Result<CardPaymentSecret, CardActionError> {
    Ok(CardPaymentSecret {
        number: card_number_for_payment(card, backend)?,
        exp_month: card.exp_month.clone(),
        exp_year: card.exp_year.clone(),
        cvc: card_cvc_for_payment(card, backend)?,
    })
}

fn card_number_for_payment(
    card: &CardRecord,
    backend: &dyn SecretBackend,
) -> Result<String, CardActionError> {
    secret_field_for_payment(card.number_ref.as_ref(), backend, "card_number")
}

fn card_cvc_for_payment(
    card: &CardRecord,
    backend: &dyn SecretBackend,
) -> Result<String, CardActionError> {
    secret_field_for_payment(card.cvc_ref.as_ref(), backend, "card_cvc")
}

fn secret_field_for_payment(
    reference: Option<&SecretReference>,
    backend: &dyn SecretBackend,
    field: &'static str,
) -> Result<String, CardActionError> {
    if let Some(reference) = reference {
        return backend
            .read_secret(reference)
            .map_err(|_| CardActionError::SecretReadFailed { field });
    }
    Err(CardActionError::SecretReadFailed { field })
}

fn next_card_secret_refs(document: &CardsDocument) -> (SecretReference, SecretReference) {
    let mut index = document.cards.len();
    loop {
        let fingerprint = card_secret_fingerprint(index);
        let number_ref = SecretReference::new("card_number", card_number_secret_key(&fingerprint));
        let cvc_ref = SecretReference::new("card_cvc", card_cvc_secret_key(&fingerprint));
        if !card_secret_ref_exists(document, &number_ref)
            && !card_secret_ref_exists(document, &cvc_ref)
        {
            return (number_ref, cvc_ref);
        }
        index += 1;
    }
}

fn card_secret_ref_exists(document: &CardsDocument, reference: &SecretReference) -> bool {
    document.cards.iter().any(|card| {
        card.number_ref.as_ref() == Some(reference) || card.cvc_ref.as_ref() == Some(reference)
    })
}

fn mark_card_used_with_backend(
    document: &mut CardsDocument,
    number_or_suffix: &str,
    backend: &dyn SecretBackend,
) -> Result<CardMutationReport, CardActionError> {
    let index = find_card_index_with_backend(document, number_or_suffix, backend)?;
    document.cards[index].status = "used".to_string();
    document.cards[index].last_error.clear();
    Ok(CardMutationReport {
        changed: true,
        summary: Some(card_summary(index, &document.cards[index])),
    })
}

fn mark_card_failed_with_backend(
    document: &mut CardsDocument,
    number_or_suffix: &str,
    error: &str,
    backend: &dyn SecretBackend,
) -> Result<CardMutationReport, CardActionError> {
    let index = find_card_index_with_backend(document, number_or_suffix, backend)?;
    document.cards[index].status = "failed".to_string();
    document.cards[index].last_error = sanitize_card_error(error);
    Ok(CardMutationReport {
        changed: true,
        summary: Some(card_summary(index, &document.cards[index])),
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

fn card_display_last_four(card: &CardRecord) -> &str {
    if !card.number_last_four.trim().is_empty() {
        card.number_last_four.trim()
    } else {
        last_four(&card.number)
    }
}

fn card_display_bin(card: &CardRecord) -> &str {
    if !card.number_bin.trim().is_empty() {
        card.number_bin.trim()
    } else {
        first_six(&card.number)
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

fn masked_number_from_bin_and_last_four(bin: &str, last_four: &str) -> String {
    match (bin.trim().is_empty(), last_four.trim().is_empty()) {
        (true, true) => "**** **** ****".to_string(),
        (true, false) => format!("**** **** **** {last_four}"),
        (false, true) => format!("{bin} ****** ****"),
        (false, false) => format!("{bin} ****** {last_four}"),
    }
}

fn normalize_status(status: &str) -> &str {
    match status.trim().to_ascii_lowercase().as_str() {
        "used" => "used",
        "failed" => "failed",
        _ => "new",
    }
}

fn card_status_priority(status: &str) -> u8 {
    match normalize_status(status) {
        "new" => 0,
        "used" => 1,
        "failed" => 2,
        _ => 3,
    }
}

fn safe_card_lookup_value(value: &str) -> String {
    let trimmed = value.trim();
    let digits: String = trimmed
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if digits.len() >= 8 {
        let suffix = if digits.len() <= 4 {
            digits.as_str()
        } else {
            &digits[digits.len() - 4..]
        };
        return format!("****{suffix}");
    }
    trimmed.to_string()
}

fn sanitize_card_error(error: &str) -> String {
    let tokens: Vec<&str> = error.split_whitespace().collect();
    if tokens.is_empty() {
        return error.to_string();
    }

    let mut changed = false;
    let mut redact_next = false;
    let mut pending_assignment_key = false;
    let mut sanitized = Vec::with_capacity(tokens.len());
    for token in tokens {
        if redact_next {
            if is_card_error_separator_token(token) {
                sanitized.push(token.to_string());
                pending_assignment_key = false;
                continue;
            }
            sanitized.push(HIDDEN_CARD_ERROR_VALUE.to_string());
            changed = true;
            redact_next = false;
            pending_assignment_key = false;
            continue;
        }

        if pending_assignment_key {
            if is_card_error_separator_token(token) {
                sanitized.push(token.to_string());
                redact_next = true;
                pending_assignment_key = false;
                continue;
            }
            pending_assignment_key = false;
        }

        let (safe_token, should_redact_next) = sanitize_card_error_token(token);
        changed |= safe_token != token || should_redact_next;
        sanitized.push(safe_token);
        if should_redact_next {
            redact_next = true;
        } else if is_sensitive_card_error_key(token) {
            pending_assignment_key = true;
        }
    }

    if changed {
        sanitized.join(" ")
    } else {
        error.to_string()
    }
}

fn sanitize_card_error_token(token: &str) -> (String, bool) {
    if let Some((safe_token, redact_next)) = sanitize_card_error_assignment(token) {
        return (safe_token, redact_next);
    }
    if looks_like_payment_card_number(token) {
        return (HIDDEN_CARD_ERROR_VALUE.to_string(), false);
    }
    if is_sensitive_card_error_key(token) {
        return (token.to_string(), true);
    }
    (token.to_string(), false)
}

fn is_card_error_separator_token(token: &str) -> bool {
    matches!(token.trim(), "=" | ":")
}

fn sanitize_card_error_assignment(token: &str) -> Option<(String, bool)> {
    for separator in ['=', ':'] {
        let Some(index) = token.find(separator) else {
            continue;
        };
        let key = &token[..index];
        if !is_sensitive_card_error_key(key) {
            continue;
        }

        let value_start = index + separator.len_utf8();
        if token[value_start..].is_empty() {
            return Some((token.to_string(), true));
        }
        return Some((
            format!("{}{}", &token[..value_start], HIDDEN_CARD_ERROR_VALUE),
            false,
        ));
    }
    None
}

fn is_sensitive_card_error_key(key: &str) -> bool {
    let normalized = key
        .trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '-'
        })
        .to_ascii_lowercase()
        .replace('-', "_");
    matches!(
        normalized.as_str(),
        "card" | "card_number" | "number" | "pan" | "cvc" | "cvv" | "security_code" | "card_cvc"
    )
}

impl CardActionError {
    fn from_io(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}
