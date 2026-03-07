use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::Response;

use crate::app_state::AppState;
use crate::services::auth_service::AuthService;

pub fn session_id_from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;

    raw.split(';').find_map(|cookie| {
        let cookie = cookie.trim();
        cookie
            .strip_prefix("session_id=")
            .map(|value| value.to_string())
    })
}

pub fn with_session_cookie(mut response: Response, session_id: &str) -> Response {
    let cookie = format!("session_id={}; HttpOnly; Path=/; SameSite=Lax", session_id);
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

pub fn clear_session_cookie(mut response: Response) -> Response {
    let cookie = "session_id=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax";
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

pub async fn authenticated_user_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    let session_id = session_id_from_headers(headers)?;
    let auth_service = AuthService::new(state.db.clone());
    auth_service.user_id_from_session(&session_id).await.ok().flatten()
}
