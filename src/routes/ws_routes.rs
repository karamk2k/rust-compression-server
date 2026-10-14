use crate::app_state::AppState;
use crate::auth::{authenticated_user_id, bearer_token_from_headers};
use axum::{
    Json, Router,
    extract::{State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::websocket::{channels::Channel, message::WsMessage};

const MAX_EVENT_NAME_LENGTH: usize = 128;

#[derive(Debug, Deserialize)]
struct PublishEventRequest {
    channel: String,
    event: String,
    #[serde(default)]
    payload: Value,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ws", get(websocket_handler))
        .route("/api/realtime/events", post(publish_event))
}

async fn publish_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PublishEventRequest>,
) -> Response {
    // Publishing is a server-to-server operation. Browser session cookies and
    // session bearer tokens may connect and subscribe, but cannot forge events.
    let valid_api_token = bearer_token_from_headers(&headers)
        .zip(state.api_token.as_deref())
        .is_some_and(|(provided, expected)| provided == expected);
    let Some(user_id) = valid_api_token.then_some(state.api_token_user_id).flatten() else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };

    if !Channel::exists(&request.channel) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown_channel" })),
        )
            .into_response();
    }

    if !Channel::can_join(&request.channel, user_id) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "channel_forbidden" })),
        )
            .into_response();
    }

    if request.event.is_empty() || request.event.len() > MAX_EVENT_NAME_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_event",
                "message": "Event names must contain between 1 and 128 bytes"
            })),
        )
            .into_response();
    }

    let delivered = state
        .websocket_hub
        .broadcast(
            &request.channel,
            WsMessage::channel(request.event, &request.channel, request.payload),
        )
        .await;

    Json(json!({ "delivered": delivered })).into_response()
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !has_allowed_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    ws.on_upgrade(move |socket| async move {
        crate::websocket::connection::handle_connection(socket, state, user_id).await;
    })
}

fn has_allowed_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        // Non-browser clients do not necessarily send Origin.
        return true;
    };
    let Some(host) = headers.get("host") else {
        return false;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(host) = host.to_str() else {
        return false;
    };

    origin
        .split_once("://")
        .map(|(_, authority)| authority.trim_end_matches('/'))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::has_allowed_origin;

    #[test]
    fn websocket_origin_must_match_host() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("app.example:8080"));
        headers.insert(
            "origin",
            HeaderValue::from_static("https://app.example:8080"),
        );
        assert!(has_allowed_origin(&headers));

        headers.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!has_allowed_origin(&headers));
    }
}
