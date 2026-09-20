#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionState {
    Trial,
    Pro,
    Free,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub alias: String,
    pub email: String,
    pub subscription_state: SubscriptionState,
}

pub fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

pub fn make_alias(input_alias: Option<&str>, email: &str) -> String {
    let alias = input_alias.unwrap_or_default().trim();
    if !alias.is_empty() {
        return alias.to_string();
    }

    let email = email.trim();
    match email.split_once('@') {
        Some((name, _)) if !name.trim().is_empty() => name.trim().to_string(),
        _ if !email.is_empty() => email.to_string(),
        _ => "account".to_string(),
    }
}
