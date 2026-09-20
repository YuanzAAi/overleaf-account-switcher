use overleaf_core::registration_trial_plan_code;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod project_api;
pub mod session;

pub const REGISTERED_EMAIL_MESSAGE: &str =
    "this email address is already associated with a different overleaf account";
pub const OVERLEAF_SESSION_COOKIE: &str = "overleaf_session2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionState {
    Trial,
    Pro,
    Subscription,
    Free,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionStatus {
    pub trial_expiry: Option<i64>,
    pub state: SubscriptionState,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialEligibility {
    Eligible,
    ActiveTrial,
    Ineligible,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialPlanAvailability {
    Available,
    ExistingSubscription,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialEligibilityStatus {
    pub eligibility: TrialEligibility,
    pub subscription: SubscriptionStatus,
    pub plan_availability: TrialPlanAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTokenPageState {
    pub visible_token: Option<String>,
    pub expiry: Option<i64>,
    pub has_existing_masked_token: bool,
    pub can_generate_token: bool,
    pub can_add_another_token: bool,
}

pub fn parse_cookie_string(cookie_str: &str) -> BTreeMap<String, String> {
    let cookie_str = cookie_str.trim();
    if cookie_str.is_empty() {
        return BTreeMap::new();
    }

    if looks_like_overleaf_session_value(cookie_str) {
        return BTreeMap::from([(OVERLEAF_SESSION_COOKIE.to_string(), cookie_str.to_string())]);
    }

    let mut cookies = BTreeMap::new();
    for item in cookie_str.split(';') {
        let item = item.trim();
        if let Some((key, value)) = item.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                cookies.insert(key.to_string(), value.trim().to_string());
            }
        }
    }

    if cookies.is_empty() {
        cookies.insert(OVERLEAF_SESSION_COOKIE.to_string(), cookie_str.to_string());
    }

    cookies
}

pub fn overleaf_session_cookie(cookies: &BTreeMap<String, String>) -> Option<&str> {
    cookies
        .get(OVERLEAF_SESSION_COOKIE)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
}

pub fn format_cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) fn header_map_from_strings(
    headers: &BTreeMap<String, String>,
) -> Result<HeaderMap, String> {
    headers
        .iter()
        .map(|(name, value)| {
            Ok((
                HeaderName::from_bytes(name.as_bytes()).map_err(|e| e.to_string())?,
                HeaderValue::from_str(value).map_err(|e| e.to_string())?,
            ))
        })
        .collect()
}

pub(crate) fn body_preview(body: &str) -> Option<String> {
    let body = body.trim();
    (!body.is_empty()).then(|| body.chars().take(200).collect())
}

pub fn extract_git_auth_token(page_html: &str) -> Option<String> {
    // 元数据里的 accessTokenPartial 只是脱敏前缀。
    let text = page_text(page_html);
    let re = Regex::new(r"\bolp_[A-Za-z0-9_-]+\b").unwrap();
    for matched in re.find_iter(&text) {
        if text[matched.end()..].starts_with('*') {
            continue;
        }
        return Some(matched.as_str().to_string());
    }
    None
}

pub fn parse_git_token_page_state(page_html: &str, now_unix: i64) -> GitTokenPageState {
    let text = page_text(page_html);
    let lower = text.to_ascii_lowercase();
    let has_existing_masked_token = Regex::new(r"olp_[A-Za-z0-9_-]*\*+")
        .unwrap()
        .is_match(&text);

    GitTokenPageState {
        visible_token: extract_git_auth_token(page_html),
        expiry: select_git_token_expiry(parse_git_token_expiry_candidates(&text), now_unix),
        has_existing_masked_token,
        can_generate_token: lower.contains("generate token"),
        can_add_another_token: lower.contains("add another token"),
    }
}

pub fn parse_git_token_expiry_candidates(text: &str) -> Vec<i64> {
    let mut candidates = Vec::new();
    let date_patterns = [
        r"\b\d{1,2}(?:st|nd|rd|th)?\s+[A-Za-z]+\.?\s+\d{4}\b",
        r"\b[A-Za-z]+\.?\s+\d{1,2}(?:st|nd|rd|th)?,?\s+\d{4}\b",
    ];

    for pattern in date_patterns {
        let re = Regex::new(pattern).unwrap();
        for matched in re.find_iter(text) {
            if let Some(timestamp) = parse_overleaf_datetime(matched.as_str()) {
                candidates.push(timestamp);
            }
        }
    }

    candidates
}

pub fn select_git_token_expiry(candidates: Vec<i64>, now_unix: i64) -> Option<i64> {
    let future = candidates
        .iter()
        .copied()
        .filter(|timestamp| *timestamp > now_unix)
        .max();
    future.or_else(|| candidates.into_iter().max())
}

pub fn has_registered_email_error(page_text: &str) -> bool {
    page_text
        .to_ascii_lowercase()
        .contains(REGISTERED_EMAIL_MESSAGE)
}

pub fn page_text(page_html: &str) -> String {
    let mut text = String::with_capacity(page_html.len());
    let mut in_tag = false;

    for ch in page_html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }

    unescape_html(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn extract_csrf_token(page_html: &str) -> Option<String> {
    extract_meta_content(page_html, "ol-csrfToken")
}

pub fn extract_user_json(page_html: &str) -> Option<serde_json::Value> {
    let raw = extract_meta_content(page_html, "ol-user")?;
    serde_json::from_str(&raw).ok()
}

pub fn meta_boolean(page_html: &str, expected_name: &str) -> Option<bool> {
    static CONTENT_ATTRIBUTE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?i)\scontent(?:\s|/|$)").unwrap());
    for chunk in page_html.split('<') {
        let tag = chunk.split('>').next().unwrap_or_default();
        if !tag.trim_start().to_ascii_lowercase().starts_with("meta")
            || attr_value(tag, "name").as_deref() != Some(expected_name)
        {
            continue;
        }
        return Some(match attr_value(tag, "content") {
            None => CONTENT_ATTRIBUTE.is_match(tag),
            Some(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            ),
        });
    }
    None
}

pub fn parse_overleaf_datetime(date_str: &str) -> Option<i64> {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(date_str.trim()) {
        return Some(date.timestamp());
    }
    let stripped = strip_ordinals(date_str);
    let without_utc = Regex::new(r"(?i)\s+UTC\s*$")
        .unwrap()
        .replace(stripped.trim(), "");
    let clean = without_utc.replace(',', " ");
    let parts: Vec<_> = clean.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let (year, month, day, rest) = if let Some(month) = month_number(parts[0]) {
        (
            parts.get(2)?.parse::<i32>().ok()?,
            month,
            parts.get(1)?.parse::<u32>().ok()?,
            &parts[3..],
        )
    } else if let Some(month) = parts.get(1).and_then(|value| month_number(value)) {
        (
            parts.get(2)?.parse::<i32>().ok()?,
            month,
            parts.first()?.parse::<u32>().ok()?,
            &parts[3..],
        )
    } else {
        return None;
    };

    let (hour, minute) = parse_time(rest)?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60)
}

pub fn parse_subscription_status(
    subscription_html: &str,
    project_html: Option<&str>,
) -> SubscriptionStatus {
    parse_subscription_status_with_now(subscription_html, project_html, current_unix_timestamp())
}

pub fn parse_subscription_status_with_now(
    subscription_html: &str,
    project_html: Option<&str>,
    now_unix: i64,
) -> SubscriptionStatus {
    let raw_html = subscription_html;
    if meta_boolean(raw_html, "ol-hasSubscription") == Some(false) {
        return SubscriptionStatus {
            trial_expiry: None,
            state: SubscriptionState::Free,
            label: Some("Free".to_string()),
        };
    }
    let metadata = parse_subscription_meta_status(raw_html, true);
    let text = page_text(raw_html);
    let lower = text.to_ascii_lowercase();
    let subscription_context = ["subscription", "subscribed", "trial", "plan"]
        .iter()
        .any(|marker| lower.contains(marker));

    let (plan_state, plan_label) = parse_current_plan(&text);

    if let Some(mut status) = metadata {
        status.trial_expiry = status.trial_expiry.or_else(|| {
            extract_termination_date(raw_html, &text)
                .and_then(|date| parse_overleaf_datetime(&date))
        });
        return status;
    }

    if subscription_context {
        if let Some(date_text) = extract_termination_date(raw_html, &text) {
            if let Some(timestamp) = parse_overleaf_datetime(&date_text) {
                let (state, label) = if let Some(state) = plan_state {
                    (state, plan_label)
                } else {
                    (SubscriptionState::Trial, Some("试用账号".to_string()))
                };
                return SubscriptionStatus {
                    trial_expiry: Some(timestamp),
                    state,
                    label,
                };
            }
        }
    }

    if let Some(project_html) = project_html {
        let project_text = page_text(project_html);
        let days_re = Regex::new(r"(?i)(\d+)\s+more\s+days?\s+on\s+your").unwrap();
        if let Some(captures) = days_re.captures(&project_text) {
            if let Some(days_left) = captures
                .get(1)
                .and_then(|value| value.as_str().parse::<i64>().ok())
            {
                return SubscriptionStatus {
                    trial_expiry: Some(now_unix + days_left * 86_400),
                    state: SubscriptionState::Trial,
                    label: Some("试用账号".to_string()),
                };
            }
        }
    }

    if let Some(state) = plan_state {
        SubscriptionStatus {
            trial_expiry: None,
            state,
            label: plan_label,
        }
    } else if is_pro_plan_text(&lower) {
        SubscriptionStatus {
            trial_expiry: None,
            state: SubscriptionState::Pro,
            label: Some("Pro/Professional".to_string()),
        }
    } else if lower.contains("cancel subscription") || lower.contains("manage subscription") {
        SubscriptionStatus {
            trial_expiry: None,
            state: SubscriptionState::Subscription,
            label: Some("已订阅账号".to_string()),
        }
    } else if lower.contains("free plan") || lower.contains("upgrade") {
        SubscriptionStatus {
            trial_expiry: None,
            state: SubscriptionState::Free,
            label: Some("Free".to_string()),
        }
    } else {
        SubscriptionStatus {
            trial_expiry: None,
            state: SubscriptionState::Unknown,
            label: None,
        }
    }
}

pub(crate) fn parse_subscription_meta_status(
    page_html: &str,
    include_billing_period: bool,
) -> Option<SubscriptionStatus> {
    let raw = extract_meta_content(page_html, "ol-subscription")?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let object = value.as_object()?;
    if object.is_empty() {
        return None;
    }

    let plan_code = value
        .get("planCode")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .pointer("/plan/planCode")
                .and_then(serde_json::Value::as_str)
        });
    let plan_name = value
        .pointer("/plan/name")
        .and_then(serde_json::Value::as_str);
    let mut trial_expiry = ["/payment/trialEndsAt", "/payment/trialEndsAtFormatted"]
        .iter()
        .find_map(|path| {
            value
                .pointer(path)
                .and_then(serde_json::Value::as_str)
                .and_then(parse_overleaf_datetime)
        });
    if include_billing_period {
        trial_expiry = trial_expiry.max(
            value
                .pointer("/payment/periodEnd")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_overleaf_datetime),
        );
    }
    let payment_state = value
        .pointer("/payment/state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let plan_text = format!(
        "{} {}",
        plan_code.unwrap_or_default(),
        plan_name.unwrap_or_default()
    )
    .to_ascii_lowercase()
    .replace('-', " ");

    let state = if is_pro_plan_text(&plan_text) {
        SubscriptionState::Pro
    } else if plan_code.is_some() || !payment_state.trim().is_empty() {
        SubscriptionState::Subscription
    } else {
        return None;
    };
    let label = plan_name
        .map(ToOwned::to_owned)
        .or_else(|| plan_code.map(ToOwned::to_owned));

    Some(SubscriptionStatus {
        trial_expiry,
        state,
        label,
    })
}

pub fn classify_trial_eligibility(
    subscription: &SubscriptionStatus,
    plan_availability: TrialPlanAvailability,
    now_unix: i64,
) -> TrialEligibility {
    if subscription
        .trial_expiry
        .is_some_and(|expiry| expiry > now_unix)
    {
        return TrialEligibility::ActiveTrial;
    }

    match subscription.state {
        SubscriptionState::Pro | SubscriptionState::Subscription => TrialEligibility::Ineligible,
        SubscriptionState::Trial if subscription.trial_expiry.is_some() => {
            TrialEligibility::Ineligible
        }
        SubscriptionState::Free | SubscriptionState::Unknown => match plan_availability {
            TrialPlanAvailability::Available => TrialEligibility::Eligible,
            TrialPlanAvailability::ExistingSubscription => TrialEligibility::Ineligible,
            TrialPlanAvailability::Unknown => TrialEligibility::Unknown,
        },
        SubscriptionState::Trial => TrialEligibility::Unknown,
    }
}

pub fn parse_trial_plan_availability(
    status: u16,
    effective_url: Option<&str>,
    page_html: &str,
    trial_days: u32,
) -> TrialPlanAvailability {
    let Some(plan_code) = registration_trial_plan_code(trial_days) else {
        return TrialPlanAvailability::Unknown;
    };
    let effective_url = effective_url.unwrap_or_default().to_ascii_lowercase();
    if effective_url.contains("/user/subscription")
        && effective_url.contains("hassubscription=true")
    {
        return TrialPlanAvailability::ExistingSubscription;
    }
    if status != 200 || !effective_url.contains("/user/subscription/new") {
        return TrialPlanAvailability::Unknown;
    }

    let html = page_html.to_ascii_lowercase();
    let plan_code = plan_code.to_ascii_lowercase();
    let has_trial_plan =
        effective_url.contains(&format!("plancode={plan_code}")) || html.contains(&plan_code);
    if has_trial_plan {
        TrialPlanAvailability::Available
    } else {
        TrialPlanAvailability::Unknown
    }
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn looks_like_overleaf_session_value(input: &str) -> bool {
    input.starts_with("s%3A") || input.starts_with("s:")
}

fn parse_current_plan(text: &str) -> (Option<SubscriptionState>, Option<String>) {
    let plan_re = Regex::new(r"(?i)currently\s+subscribed\s+to\s+the\s+(.+?)\s+plan\b").unwrap();
    let Some(captures) = plan_re.captures(text) else {
        return (None, None);
    };

    let plan_name = captures
        .get(1)
        .map(|value| {
            value
                .as_str()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    if plan_name.to_ascii_lowercase().contains("pro")
        || plan_name.to_ascii_lowercase().contains("professional")
    {
        (Some(SubscriptionState::Pro), Some(plan_name))
    } else {
        (Some(SubscriptionState::Subscription), Some(plan_name))
    }
}

fn extract_termination_date(raw_html: &str, text: &str) -> Option<String> {
    let strong_re = Regex::new(r"(?is)terminate\s+on\s*<strong>\s*([^<]+?)\s*</strong>").unwrap();
    if let Some(date) = strong_re
        .captures(raw_html)
        .and_then(|captures| captures.get(1))
        .map(|value| page_text(value.as_str()))
    {
        return Some(date);
    }

    let date_pattern = r"((?:[A-Z][A-Za-z]+\.?\s+\d{1,2}(?:st|nd|rd|th)?,\s+\d{4}|\d{1,2}(?:st|nd|rd|th)?\s+[A-Z][A-Za-z]+\.?\s+\d{4})(?:\s+\d{1,2}:\d{2}\s+[AP]M)?(?:\s+UTC)?)";
    let text_re = Regex::new(&format!(
        r"(?i)(?:(?:will\s+)?(?:terminate|terminates?|ends?|expires?)|lose\s+access\s+to\s+all\s+premium\s+features)\s+(?:on\s+)?{date_pattern}"
    ))
    .unwrap();
    text_re
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
}

fn is_pro_plan_text(lower_text: &str) -> bool {
    let patterns = [
        r"\b(?:pro|professional)\s+(?:annual|monthly)\b",
        r"\b(?:annual|monthly)\s+(?:pro|professional)\b",
        r"\bcurrent\s+plan\b.{0,120}\b(?:pro|professional)\b",
        r"\byour\s+plan\b.{0,120}\b(?:pro|professional)\b",
        r"\bcurrently\s+subscribed\b.{0,120}\b(?:pro|professional)\b",
    ];
    patterns
        .iter()
        .any(|pattern| Regex::new(pattern).unwrap().is_match(lower_text))
}

pub(crate) fn extract_meta_content(page_html: &str, expected_name: &str) -> Option<String> {
    for chunk in page_html.split('<') {
        let tag = chunk.split('>').next().unwrap_or_default();
        if !tag.trim_start().to_ascii_lowercase().starts_with("meta") {
            continue;
        }
        if attr_value(tag, "name").as_deref() == Some(expected_name) {
            return attr_value(tag, "content").map(|value| unescape_html(&value));
        }
    }
    None
}

fn attr_value(tag: &str, attr_name: &str) -> Option<String> {
    let pattern = format!(
        r#"(?is)\b{}\s*=\s*("([^"]*)"|'([^']*)')"#,
        regex::escape(attr_name)
    );
    let re = Regex::new(&pattern).unwrap();
    let captures = re.captures(tag)?;
    captures
        .get(2)
        .or_else(|| captures.get(3))
        .map(|value| value.as_str().to_string())
}

fn unescape_html(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn strip_ordinals(input: &str) -> String {
    Regex::new(r"(?i)(\d+)(st|nd|rd|th)\b")
        .unwrap()
        .replace_all(input.trim(), "$1")
        .to_string()
}

fn month_number(input: &str) -> Option<u32> {
    let normalized = input.trim_end_matches('.').to_ascii_lowercase();
    match normalized.as_str() {
        "january" | "jan" => Some(1),
        "february" | "feb" => Some(2),
        "march" | "mar" => Some(3),
        "april" | "apr" => Some(4),
        "may" => Some(5),
        "june" | "jun" => Some(6),
        "july" | "jul" => Some(7),
        "august" | "aug" => Some(8),
        "september" | "sep" | "sept" => Some(9),
        "october" | "oct" => Some(10),
        "november" | "nov" => Some(11),
        "december" | "dec" => Some(12),
        _ => None,
    }
}

fn parse_time(parts: &[&str]) -> Option<(u32, u32)> {
    if parts.is_empty() {
        return Some((0, 0));
    }
    if parts.len() < 2 {
        return None;
    }

    let (hour_text, minute_text) = parts[0].split_once(':')?;
    let mut hour = hour_text.parse::<u32>().ok()?;
    let minute = minute_text.parse::<u32>().ok()?;
    let am_pm = parts[1].to_ascii_lowercase();

    if hour == 12 {
        hour = 0;
    }
    if am_pm == "pm" {
        hour += 12;
    } else if am_pm != "am" {
        return None;
    }

    Some((hour, minute))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    y -= (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
