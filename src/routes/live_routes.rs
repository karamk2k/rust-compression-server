use axum::body::Body;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::services::live_stream_service::LiveStreamService;

const STREAMS_PAGE_HTML: &str = include_str!("../../web/admin/streams.html");
const WATCH_PAGE_HTML: &str = include_str!("../../web/live/watch.html");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/streams", get(streams_page))
        .route("/api/live/streams", get(list_streams).post(create_stream))
        .route(
            "/api/live/streams/:id/publish-key",
            post(rotate_publish_key),
        )
        .route("/api/live/streams/:id/enabled", patch(set_stream_enabled))
        .route(
            "/api/live/streams/:id/viewer-keys",
            get(list_viewer_keys).post(create_viewer_key),
        )
        .route("/api/live/viewer-keys/:id/revoke", post(revoke_viewer_key))
        .route("/internal/rtmp/publish", post(authorize_publish))
        .route("/internal/rtmp/publish-done", post(publish_done))
        .route("/internal/live/authorize", get(authorize_hls))
        .route("/watch/:public_id", get(watch_stream))
        .route("/hls/:file_name", get(serve_local_hls))
}

#[derive(Deserialize)]
struct CreateStreamRequest {
    title: String,
}

#[derive(Deserialize)]
struct SetEnabledRequest {
    enabled: bool,
}

#[derive(Deserialize)]
struct CreateViewerKeyRequest {
    label: String,
    expires_hours: Option<i64>,
}

#[derive(Deserialize)]
struct RtmpPublishForm {
    name: String,
    key: Option<String>,
    args: Option<String>,
    addr: Option<String>,
    app: Option<String>,
}

#[derive(Deserialize)]
struct HlsAuthQuery {
    stream: String,
}

#[derive(Deserialize)]
struct WatchQuery {
    key: Option<String>,
}

async fn streams_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return Redirect::to("/admin/login").into_response();
    }
    Html(STREAMS_PAGE_HTML.to_string()).into_response()
}

async fn create_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateStreamRequest>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db)
        .create(user_id, &body.title)
        .await
    {
        Ok((stream, publish_key)) => Json(json!({
            "stream": stream,
            "publish_key": publish_key,
            "obs_stream_key": format!("{}?key={}", stream.public_id, publish_key)
        }))
        .into_response(),
        Err(error) => json_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn list_streams(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db).list_owned(user_id).await {
        Ok(streams) => Json(json!({ "streams": streams })).into_response(),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn rotate_publish_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let service = LiveStreamService::new(state.db);
    match service.rotate_publish_key(id, user_id).await {
        Ok(Some(publish_key)) => match service.get_owned(id, user_id).await {
            Ok(Some(stream)) => Json(json!({
                "publish_key": publish_key,
                "obs_stream_key": format!("{}?key={}", stream.public_id, publish_key)
            }))
            .into_response(),
            _ => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stream could not be loaded",
            ),
        },
        Ok(None) => json_error(StatusCode::NOT_FOUND, "stream not found"),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn set_stream_enabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<SetEnabledRequest>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db.clone())
        .set_enabled(id, user_id, body.enabled)
        .await
    {
        Ok(true) => Json(json!({ "enabled": body.enabled })).into_response(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "stream not found"),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn create_viewer_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<CreateViewerKeyRequest>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db.clone())
        .create_viewer_key(id, user_id, &body.label, body.expires_hours)
        .await
    {
        Ok(Some((viewer_key, token))) => Json(json!({
            "viewer_key": viewer_key,
            "token": token,
            "watch_path": format!("/watch/{}?key={}", stream_public_id(&state, id).await.unwrap_or_default(), token)
        }))
        .into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "stream not found"),
        Err(error) => json_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn stream_public_id(state: &AppState, id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT public_id FROM live_streams WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

async fn list_viewer_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db)
        .list_viewer_keys(id, user_id)
        .await
    {
        Ok(Some(keys)) => Json(json!({ "viewer_keys": keys })).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "stream not found"),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn revoke_viewer_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let Some(user_id) = authenticated_user_id(&state, &headers).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match LiveStreamService::new(state.db)
        .revoke_viewer_key(id, user_id)
        .await
    {
        Ok(true) => Json(json!({ "revoked": true })).into_response(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "viewer key not found"),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn authorize_publish(
    State(state): State<AppState>,
    Form(form): Form<RtmpPublishForm>,
) -> StatusCode {
    let stream_id = form.name.clone();
    let source_addr = form.addr.as_deref().unwrap_or("unknown").to_string();
    let application = form.app.as_deref().unwrap_or("unknown").to_string();
    let service = LiveStreamService::new(state.db);
    // nginx-rtmp normally expands stream query arguments into form fields, so
    // `stream-id?key=secret` arrives as `key=secret`. Accept `args` as well for
    // builds that forward the raw query string.
    let publish_key = form.key.or_else(|| {
        form.args.as_deref().and_then(|args| {
            url::form_urlencoded::parse(args.as_bytes())
                .find_map(|(name, value)| (name == "key").then(|| value.into_owned()))
        })
    });
    let allowed = match publish_key.as_deref() {
        Some(key) => service
            .authorize_publish(&form.name, key)
            .await
            .unwrap_or(false),
        None => false,
    };
    if allowed {
        let _ = service.mark_live(&form.name, true).await;
        info!(
            stream_id = %stream_id,
            source_addr = %source_addr,
            application = %application,
            "RTMP publish accepted"
        );
        StatusCode::OK
    } else {
        warn!(
            stream_id = %stream_id,
            source_addr = %source_addr,
            application = %application,
            key_supplied = publish_key.is_some(),
            "RTMP publish rejected"
        );
        StatusCode::FORBIDDEN
    }
}

async fn publish_done(
    State(state): State<AppState>,
    Form(form): Form<RtmpPublishForm>,
) -> StatusCode {
    let _ = LiveStreamService::new(state.db)
        .mark_live(&form.name, false)
        .await;
    info!(
        stream_id = %form.name,
        source_addr = %form.addr.as_deref().unwrap_or("unknown"),
        "RTMP publisher disconnected"
    );
    StatusCode::OK
}

async fn authorize_hls(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HlsAuthQuery>,
) -> StatusCode {
    let token = cookie_value(&headers, "live_viewer");
    match token {
        Some(token)
            if LiveStreamService::new(state.db)
                .authorize_viewer(&query.stream, &token)
                .await
                .unwrap_or(false) =>
        {
            StatusCode::NO_CONTENT
        }
        _ => StatusCode::UNAUTHORIZED,
    }
}

async fn watch_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(public_id): Path<String>,
    Query(query): Query<WatchQuery>,
) -> Response {
    let service = LiveStreamService::new(state.db);
    let Some(stream) = service.public_stream(&public_id).await.ok().flatten() else {
        return (StatusCode::NOT_FOUND, Html("Stream not found".to_string())).into_response();
    };

    if let Some(key) = query.key {
        if !service
            .authorize_viewer(&public_id, &key)
            .await
            .unwrap_or(false)
        {
            return (
                StatusCode::FORBIDDEN,
                Html("This viewing key is invalid or expired".to_string()),
            )
                .into_response();
        }
        let mut response = Redirect::to(&format!("/watch/{}", public_id)).into_response();
        let cookie = format!(
            "live_viewer={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400{}",
            key,
            if forwarded_https(&headers) {
                "; Secure"
            } else {
                ""
            }
        );
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
        return response;
    }

    let allowed = match cookie_value(&headers, "live_viewer") {
        Some(token) => service
            .authorize_viewer(&public_id, &token)
            .await
            .unwrap_or(false),
        None => false,
    };
    if !allowed {
        return (
            StatusCode::UNAUTHORIZED,
            Html("A valid viewing link is required".to_string()),
        )
            .into_response();
    }

    Html(
        WATCH_PAGE_HTML
            .replace("{{STREAM_TITLE}}", &escape_html(&stream.title))
            .replace("{{STREAM_ID}}", &escape_html(&stream.public_id)),
    )
    .into_response()
}

async fn serve_local_hls(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(file_name): Path<String>,
) -> Response {
    let Some(stream_id) = hls_stream_id(&file_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(token) = cookie_value(&headers, "live_viewer") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let service = LiveStreamService::new(state.db.clone());
    if !service
        .authorize_viewer(stream_id, &token)
        .await
        .unwrap_or(false)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let path = state.live_hls_dir.join(&file_name);
    let file = match File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(error) => {
            warn!(?error, file = %file_name, "failed to open local HLS file");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let content_length = file.metadata().await.ok().map(|metadata| metadata.len());
    let content_type = if file_name.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp2t"
    };
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(length) = content_length {
        if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
            response.headers_mut().insert(header::CONTENT_LENGTH, value);
        }
    }
    response
}

fn hls_stream_id(file_name: &str) -> Option<&str> {
    if !(file_name.ends_with(".m3u8") || file_name.ends_with(".ts")) {
        return None;
    }
    if !file_name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
    }) {
        return None;
    }
    let stream_id = file_name.split(['-', '.']).next()?;
    (stream_id.len() == 32 && stream_id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(stream_id)
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|cookie| {
        let (key, value) = cookie.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
