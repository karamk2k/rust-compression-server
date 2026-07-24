use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::services::auth_service::AuthService;

#[derive(Debug, Serialize)]
struct CurrentUserResponse {
    id: i64,
    username: String,
}

pub async fn get_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let auth_service = AuthService::new(state.db.clone());
    match auth_service.username_by_id(user_id).await {
        Ok(Some(username)) => Json(CurrentUserResponse {
            id: user_id,
            username,
        })
        .into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "user not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load current user: {}", error) }),
        ),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}
