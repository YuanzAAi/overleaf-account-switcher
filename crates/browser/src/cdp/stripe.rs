use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cdp_session::{CdpSession, CdpSessionError};
use crate::cdp_ws::CdpWebSocketTransport;

const REQUIRED_FIELD_READY_TIMEOUT_MS: u64 = 5_000;
const TARGET_CANDIDATE_READY_TIMEOUT_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeAddressInput {
    pub name: String,
    pub line1: String,
    pub city: String,
    pub state: String,
    pub zip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripePaymentInput {
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StripeFrameActionResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub filled: Vec<String>,
    #[serde(default)]
    pub optional_missing: Vec<String>,
    #[serde(default)]
    pub target_reload: bool,
}

impl StripeFrameActionResult {
    pub(crate) fn empty_success() -> Self {
        Self {
            ok: true,
            missing: None,
            filled: Vec::new(),
            optional_missing: Vec::new(),
            target_reload: false,
        }
    }

    pub(crate) fn merge(&mut self, source: Self) {
        self.ok &= source.ok;
        if source.missing.is_some() {
            self.missing = source.missing;
        }
        self.filled.extend(source.filled);
        self.optional_missing.extend(source.optional_missing);
        self.target_reload |= source.target_reload;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripePaymentFramesResult {
    pub address: StripeFrameActionResult,
    pub payment: StripeFrameActionResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripeOopifError {
    InvalidTargetList {
        message: String,
    },
    MissingTarget {
        component: &'static str,
    },
    Connect {
        component: &'static str,
        message: String,
    },
    Session {
        component: &'static str,
        message: String,
    },
    MissingField {
        component: &'static str,
        field: String,
    },
    TargetReload {
        component: &'static str,
    },
}

#[derive(Debug, Deserialize)]
struct DebuggerTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StripePaymentField {
    Number,
    Expiry,
    Cvc,
}

impl StripePaymentField {
    pub(crate) const ALL: [Self; 3] = [Self::Number, Self::Expiry, Self::Cvc];

    fn selector(self) -> &'static str {
        match self {
            Self::Number => "input[name='number']",
            Self::Expiry => "input[name='expiry']",
            Self::Cvc => "input[name='cvc']",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Expiry => "expiry",
            Self::Cvc => "cvc",
        }
    }
}

impl fmt::Display for StripeOopifError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTargetList { message } => {
                write!(formatter, "invalid Chrome target list: {message}")
            }
            Self::MissingTarget { component } => {
                write!(formatter, "Stripe {component} OOPIF target not found")
            }
            Self::Connect { component, message } => {
                write!(formatter, "connect Stripe {component} OOPIF: {message}")
            }
            Self::Session { component, message } => {
                write!(formatter, "operate Stripe {component} OOPIF: {message}")
            }
            Self::MissingField { component, field } => {
                write!(formatter, "Stripe {component} field unavailable: {field}")
            }
            Self::TargetReload { component } => {
                write!(formatter, "Stripe {component} OOPIF target reloading")
            }
        }
    }
}

impl std::error::Error for StripeOopifError {}

pub fn split_stripe_name(full_name: &str) -> (String, String) {
    let parts: Vec<&str> = full_name.split_whitespace().collect();
    match parts.as_slice() {
        [] => (String::new(), String::new()),
        [first] => ((*first).to_string(), String::new()),
        [first, rest @ ..] => ((*first).to_string(), rest.join(" ")),
    }
}

pub fn stripe_payment_expiry_value(exp_month: &str, exp_year: &str) -> String {
    let month = exp_month.trim();
    let year = exp_year.trim();
    let suffix = year
        .char_indices()
        .rev()
        .nth(1)
        .map(|(index, _)| &year[index..])
        .unwrap_or(year);
    format!("{month}/{suffix}")
}

pub async fn fill_stripe_oopif_frames(
    targets_json: &str,
    address: &StripeAddressInput,
    payment: &StripePaymentInput,
) -> Result<StripePaymentFramesResult, StripeOopifError> {
    let address = fill_stripe_address_oopif_frame(targets_json, address).await?;
    let mut payment_result = StripeFrameActionResult::empty_success();
    for field in StripePaymentField::ALL {
        payment_result.merge(fill_stripe_payment_oopif_field(targets_json, payment, field).await?);
    }
    Ok(StripePaymentFramesResult {
        address,
        payment: payment_result,
    })
}

fn parse_stripe_target_urls(
    input: &str,
    component: &'static str,
) -> Result<Vec<String>, StripeOopifError> {
    let targets: Vec<DebuggerTarget> =
        serde_json::from_str(input).map_err(|error| StripeOopifError::InvalidTargetList {
            message: error.to_string(),
        })?;
    let mut urls = Vec::new();
    for target in targets {
        if target.target_type != "iframe" || !target_matches_component(&target.url, component) {
            continue;
        }
        let Some(url) = target.websocket_url.as_deref().map(str::trim) else {
            continue;
        };
        if !url.is_empty() && !urls.iter().any(|candidate| candidate == url) {
            urls.push(url.to_string());
        }
    }
    if urls.is_empty() {
        return Err(StripeOopifError::MissingTarget { component });
    }
    Ok(urls)
}

fn target_matches_component(url: &str, component: &'static str) -> bool {
    if url.contains(&format!("componentName={component}")) {
        return true;
    }
    if url.contains("componentName=") {
        return false;
    }
    // 重挂载时 URL 会变化，以字段可见性确认支付 iframe。
    component == "payment"
}

pub(crate) async fn fill_stripe_address_oopif_frame(
    targets_json: &str,
    input: &StripeAddressInput,
) -> Result<StripeFrameActionResult, StripeOopifError> {
    let targets = parse_stripe_target_urls(targets_json, "address")?;
    let mut preferred_error = None;
    for websocket_url in targets {
        match fill_address_target(&websocket_url, input).await {
            Ok(result) => return Ok(result),
            Err(error) => retain_candidate_error(&mut preferred_error, error),
        }
    }
    Err(preferred_error.unwrap_or(StripeOopifError::MissingTarget {
        component: "address",
    }))
}

pub(crate) async fn fill_stripe_payment_oopif_field(
    targets_json: &str,
    input: &StripePaymentInput,
    field: StripePaymentField,
) -> Result<StripeFrameActionResult, StripeOopifError> {
    let targets = parse_stripe_target_urls(targets_json, "payment")?;
    let mut preferred_error = None;
    for websocket_url in targets {
        match fill_payment_field_target(&websocket_url, input, field).await {
            Ok(result) => return Ok(result),
            Err(error) => retain_candidate_error(&mut preferred_error, error),
        }
    }
    Err(preferred_error.unwrap_or(StripeOopifError::MissingTarget {
        component: "payment",
    }))
}

fn retain_candidate_error(slot: &mut Option<StripeOopifError>, error: StripeOopifError) {
    let preferred = matches!(
        error,
        StripeOopifError::MissingField { .. } | StripeOopifError::TargetReload { .. }
    );
    if preferred || slot.is_none() {
        *slot = Some(error);
    }
}

async fn connect_target(
    component: &'static str,
    websocket_url: &str,
) -> Result<CdpSession<CdpWebSocketTransport>, StripeOopifError> {
    let transport = CdpWebSocketTransport::connect(websocket_url)
        .await
        .map_err(|error| StripeOopifError::Connect {
            component,
            message: error.message,
        })?;
    Ok(CdpSession::new(transport))
}

async fn fill_address_target(
    websocket_url: &str,
    input: &StripeAddressInput,
) -> Result<StripeFrameActionResult, StripeOopifError> {
    let component = "address";
    let mut session = connect_target(component, websocket_url).await?;
    let prepared = evaluate_action(
        &mut session,
        component,
        prepare_address_expression(&input.state),
        true,
    )
    .await?;
    if prepared.target_reload {
        return Err(StripeOopifError::TargetReload { component });
    }
    ensure_action_ok(component, prepared)?;

    let (first_name, last_name) = split_stripe_name(&input.name);
    let first_name = if first_name.is_empty() {
        input.name.trim()
    } else {
        first_name.as_str()
    };
    let last_name = if last_name.is_empty() {
        input.name.trim()
    } else {
        last_name.as_str()
    };
    let mut result = StripeFrameActionResult {
        ok: true,
        missing: None,
        filled: vec!["country".to_string(), "administrativeArea".to_string()],
        optional_missing: Vec::new(),
        target_reload: false,
    };
    type_input(
        &mut session,
        component,
        "input[name='lastName']",
        last_name,
        "lastName",
        false,
        &mut result,
    )
    .await?;
    type_input(
        &mut session,
        component,
        "input[name='firstName']",
        first_name,
        "firstName",
        false,
        &mut result,
    )
    .await?;
    for (selector, value, label) in [
        (
            "input[name='addressLine1']",
            input.line1.as_str(),
            "addressLine1",
        ),
        ("input[name='locality']", input.city.as_str(), "locality"),
        ("input[name='postalCode']", input.zip.as_str(), "postalCode"),
    ] {
        type_input(
            &mut session,
            component,
            selector,
            value,
            label,
            true,
            &mut result,
        )
        .await?;
    }
    Ok(result)
}

async fn fill_payment_field_target(
    websocket_url: &str,
    input: &StripePaymentInput,
    field: StripePaymentField,
) -> Result<StripeFrameActionResult, StripeOopifError> {
    let component = "payment";
    let mut session = connect_target(component, websocket_url).await?;
    let prepared = evaluate_action(
        &mut session,
        component,
        wait_for_visible_field_expression(
            field.selector(),
            field.label(),
            TARGET_CANDIDATE_READY_TIMEOUT_MS,
        ),
        true,
    )
    .await?;
    ensure_action_ok(component, prepared)?;
    let expiry = stripe_payment_expiry_value(&input.exp_month, &input.exp_year);
    let value = match field {
        StripePaymentField::Number => input.number.as_str(),
        StripePaymentField::Expiry => expiry.as_str(),
        StripePaymentField::Cvc => input.cvc.as_str(),
    };
    let mut result = StripeFrameActionResult::empty_success();
    type_input(
        &mut session,
        component,
        field.selector(),
        value,
        field.label(),
        true,
        &mut result,
    )
    .await?;
    Ok(result)
}

async fn evaluate_action(
    session: &mut CdpSession<CdpWebSocketTransport>,
    component: &'static str,
    expression: String,
    await_promise: bool,
) -> Result<StripeFrameActionResult, StripeOopifError> {
    let raw = session
        .evaluate_string(expression, await_promise)
        .await
        .map_err(|error| session_error(component, error))?;
    serde_json::from_str(&raw).map_err(|error| StripeOopifError::Session {
        component,
        message: format!("parse action result: {error}"),
    })
}

fn ensure_action_ok(
    component: &'static str,
    result: StripeFrameActionResult,
) -> Result<(), StripeOopifError> {
    if result.ok {
        return Ok(());
    }
    Err(StripeOopifError::MissingField {
        component,
        field: result.missing.unwrap_or_else(|| "unknown".to_string()),
    })
}

#[allow(clippy::too_many_arguments)]
async fn type_input(
    session: &mut CdpSession<CdpWebSocketTransport>,
    component: &'static str,
    selector: &str,
    value: &str,
    label: &str,
    required: bool,
    result: &mut StripeFrameActionResult,
) -> Result<(), StripeOopifError> {
    if value.trim().is_empty() {
        if required {
            return Err(StripeOopifError::MissingField {
                component,
                field: label.to_string(),
            });
        }
        result.optional_missing.push(label.to_string());
        return Ok(());
    }
    let focus = evaluate_action(
        session,
        component,
        focus_and_clear_expression(
            selector,
            if required {
                REQUIRED_FIELD_READY_TIMEOUT_MS
            } else {
                0
            },
        ),
        true,
    )
    .await?;
    if !focus.ok {
        if required {
            return Err(StripeOopifError::MissingField {
                component,
                field: focus.missing.unwrap_or_else(|| label.to_string()),
            });
        }
        result.optional_missing.push(label.to_string());
        return Ok(());
    }
    session
        .insert_text(value)
        .await
        .map_err(|error| session_error(component, error))?;
    tokio::time::sleep(Duration::from_millis(75)).await;
    let validation = evaluate_action(
        session,
        component,
        validate_and_blur_expression(
            selector,
            label,
            if required {
                REQUIRED_FIELD_READY_TIMEOUT_MS
            } else {
                0
            },
        ),
        true,
    )
    .await?;
    ensure_action_ok(component, validation)?;
    result.filled.push(label.to_string());
    Ok(())
}

fn session_error(component: &'static str, error: CdpSessionError) -> StripeOopifError {
    StripeOopifError::Session {
        component,
        message: error.to_string(),
    }
}

fn prepare_address_expression(state: &str) -> String {
    let payload = json!({ "state": state.trim() }).to_string();
    format!(
        r#"new Promise(resolve => {{
  const input = {payload};
  const done = value => resolve(JSON.stringify(value));
  const visible = element => !!element && !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
  const setValue = (element, value) => {{
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value')
      || Object.getOwnPropertyDescriptor(window.HTMLSelectElement?.prototype || {{}}, 'value');
    if (descriptor?.set) descriptor.set.call(element, value);
    else element.value = value;
    element.dispatchEvent(new Event('input', {{ bubbles: true }}));
    element.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const deadline = Date.now() + 10000;
  const tick = () => {{
    const country = document.querySelector("select[name='country']");
    if (!visible(country) && Date.now() < deadline) return setTimeout(tick, 100);
    if (!visible(country)) return done({{ ok: false, missing: 'country' }});
    if (country.value !== 'US') {{
      setTimeout(() => setValue(country, 'US'), 0);
      return done({{ ok: true, target_reload: true }});
    }}
    const fields = {{
      addressLine1: document.querySelector("input[name='addressLine1']"),
      locality: document.querySelector("input[name='locality']"),
      administrativeArea: document.querySelector("select[name='administrativeArea']"),
      postalCode: document.querySelector("input[name='postalCode']")
    }};
    const missing = Object.keys(fields).find(key => !visible(fields[key]));
    if (missing && Date.now() < deadline) return setTimeout(tick, 100);
    if (missing) return done({{ ok: false, missing }});
    setValue(fields.administrativeArea, input.state);
    if (fields.administrativeArea.value !== input.state) {{
      if (Date.now() < deadline) return setTimeout(tick, 100);
      return done({{ ok: false, missing: 'administrativeArea value' }});
    }}
    done({{ ok: true, filled: ['country', 'administrativeArea'] }});
  }};
  tick();
}})"#
    )
}

fn wait_for_visible_field_expression(selector: &str, label: &str, wait_ms: u64) -> String {
    let selector = serde_json::to_string(selector).expect("selector should serialize");
    let label = serde_json::to_string(label).expect("label should serialize");
    format!(
        r#"new Promise(resolve => {{
  const done = value => resolve(JSON.stringify(value));
  const visible = element => !!element && !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
  const deadline = Date.now() + {wait_ms};
  const tick = () => {{
    const element = document.querySelector({selector});
    if (!visible(element) && Date.now() < deadline) return setTimeout(tick, 50);
    done({{ ok: visible(element), missing: visible(element) ? null : {label} }});
  }};
  tick();
}})"#
    )
}

fn focus_and_clear_expression(selector: &str, wait_ms: u64) -> String {
    let selector = serde_json::to_string(selector).expect("selector should serialize");
    format!(
        r#"new Promise(resolve => {{
  const done = value => resolve(JSON.stringify(value));
  const visible = element => !!element && !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
  const deadline = Date.now() + {wait_ms};
  const tick = () => {{
    const element = document.querySelector({selector});
    if (!visible(element) && Date.now() < deadline) return setTimeout(tick, 100);
    if (!visible(element)) return done({{ ok: false, missing: {selector} }});
    element.focus();
    const descriptor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value')
      || Object.getOwnPropertyDescriptor(window.HTMLInputElement?.prototype || {{}}, 'value');
    if (descriptor?.set) descriptor.set.call(element, '');
    else element.value = '';
    element.dispatchEvent(new Event('input', {{ bubbles: true }}));
    done({{ ok: true }});
  }};
  tick();
}})"#
    )
}

fn validate_and_blur_expression(selector: &str, label: &str, wait_ms: u64) -> String {
    let selector = serde_json::to_string(selector).expect("selector should serialize");
    let label = serde_json::to_string(label).expect("label should serialize");
    format!(
        r#"new Promise(resolve => {{
  const done = value => resolve(JSON.stringify(value));
  const visible = element => !!element && !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
  const deadline = Date.now() + {wait_ms};
  const tick = () => {{
    const element = document.querySelector({selector});
    if (!visible(element) && Date.now() < deadline) return setTimeout(tick, 100);
    if (!visible(element)) return done({{ ok: false, missing: {label} }});
    element.blur();
    const invalid = element.getAttribute('aria-invalid') === 'true';
    const empty = String(element.value || '').length === 0;
    done({{ ok: !invalid && !empty, missing: invalid || empty ? {label} : null }});
  }};
  tick();
}})"#
    )
}
