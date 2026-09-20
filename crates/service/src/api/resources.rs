use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CardAddRequest {
    line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CardStatusRequest {
    number_or_suffix: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CardRemoveRequest {
    number_or_suffix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AddressSelectRequest {
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AddressFallbackRequest {
    #[serde(default)]
    selector: usize,
}

pub(super) fn cards_response(state: &ApiState) -> ApiResponse {
    let store = CardStore::new(state.config.cards_path());
    match store.load() {
        Ok(document) => json_response(200, &list_card_summaries(&document)),
        Err(error) => io_error_response(error),
    }
}

pub(super) fn addresses_response(state: &ApiState) -> ApiResponse {
    let store = AddressStore::new(state.config.addresses_path());
    match store.load() {
        Ok(document) => json_response(200, &list_address_summaries(&document)),
        Err(error) => io_error_response(error),
    }
}

pub(super) fn fetch_address_response(state: &ApiState, now_unix: i64) -> ApiResponse {
    let store = AddressStore::new(state.config.addresses_path());
    let selector = usize::try_from(now_unix).unwrap_or_default();
    match block_on_api(fetch_address_into_store(
        state.address_provider.as_ref(),
        &store,
        selector,
    )) {
        Ok(Ok(report)) => {
            let source = match report.source {
                AddressFetchSource::Api => RegistrationAddressSource::Api,
                AddressFetchSource::Fallback => RegistrationAddressSource::Fallback,
            };
            state.set_prefetched_registration_address(RegistrationAddressSelection {
                source,
                fallback_index: (source == RegistrationAddressSource::Fallback)
                    .then_some(report.index),
                address: report.address.clone(),
                api_error: report.api_error.clone(),
            });
            json_response(200, &report)
        }
        Ok(Err(error)) => address_action_error_response(error),
        Err(response) => response,
    }
}

pub(super) fn select_address_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: AddressSelectRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let store = AddressStore::new(state.config.addresses_path());
    match store.load() {
        Ok(document) => match select_address_by_index(&document, request.index) {
            Ok(selection) => selected_address_response(state, selection),
            Err(error) => address_action_error_response(error),
        },
        Err(error) => io_error_response(error),
    }
}

pub(super) fn select_fallback_address_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: AddressFallbackRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let store = AddressStore::new(state.config.addresses_path());
    match select_fallback_address_in_store(&store, request.selector) {
        Ok(selection) => selected_address_response(state, selection),
        Err(error) => address_action_error_response(error),
    }
}

fn selected_address_response(state: &ApiState, selection: crate::AddressSelection) -> ApiResponse {
    state.set_prefetched_registration_address(RegistrationAddressSelection {
        source: RegistrationAddressSource::Fallback,
        fallback_index: Some(selection.index),
        address: selection.address.clone(),
        api_error: None,
    });
    json_response(200, &selection)
}

pub(super) fn add_card_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: CardAddRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let store = CardStore::new(state.config.cards_path());
    match add_card_from_line_in_store_with_backend(
        &store,
        &request.line,
        state.secret_backend.as_ref(),
    ) {
        Ok(report) => json_response(200, &report),
        Err(error) => card_action_error_response(error),
    }
}

pub(super) fn export_cards_response(state: &ApiState, body: &str) -> ApiResponse {
    #[derive(Deserialize)]
    struct Request {
        ids: Vec<String>,
    }
    let request: Request = match serde_json::from_str(body) {
        Ok(request) => request,
        Err(_) => {
            return json_response(
                400,
                &ApiErrorBody {
                    error: "请选择要导出的银行卡".to_string(),
                },
            )
        }
    };
    let store = CardStore::new(state.config.cards_path());
    let document = match store.load() {
        Ok(document) => document,
        Err(error) => return io_error_response(error),
    };
    match crate::card_actions::export_cards_text(
        &document,
        &request.ids,
        state.secret_backend.as_ref(),
    ) {
        Ok(body) => ApiResponse {
            status_code: 200,
            content_type: "text/plain; charset=utf-8",
            body,
        },
        Err(error) => card_action_error_response(error),
    }
}

pub(super) fn update_card_status_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: CardStatusRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let store = CardStore::new(state.config.cards_path());
    match request.status.trim().to_ascii_lowercase().as_str() {
        "used" => match mark_card_used_in_store_with_backend(
            &store,
            &request.number_or_suffix,
            state.secret_backend.as_ref(),
        ) {
            Ok(report) => json_response(200, &report),
            Err(error) => card_action_error_response(error),
        },
        "failed" => match mark_card_failed_in_store_with_backend(
            &store,
            &request.number_or_suffix,
            request.error.as_deref().unwrap_or_default(),
            state.secret_backend.as_ref(),
        ) {
            Ok(report) => json_response(200, &report),
            Err(error) => card_action_error_response(error),
        },
        _ => json_response(
            400,
            &ApiErrorBody {
                error: "card status must be used or failed".to_string(),
            },
        ),
    }
}

pub(super) fn remove_card_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: CardRemoveRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let store = CardStore::new(state.config.cards_path());
    match remove_card_in_store_with_backend(
        &store,
        &request.number_or_suffix,
        state.secret_backend.as_ref(),
    ) {
        Ok(report) => json_response(200, &report),
        Err(error) => card_action_error_response(error),
    }
}
