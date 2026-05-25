use chrono::{DateTime, Duration, Utc};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::services::file_service::FileService;
use crate::services::file_share_service::FileShareService;

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    expires_at: Option<DateTime<Utc>>,
    expires_in_hours: Option<i64>,
}

#[derive(Debug, Serialize)]
struct CreateShareResponse {
    id: i64,
    uuid: String,
    file_id: i64,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    url: String,
}

pub async fn create_file_share(
    Path(file_id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateShareRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let file_share_service = FileShareService::new(state.db.clone());
    let expires_at = payload.expires_at.or_else(|| {
        payload
            .expires_in_hours
            .map(|hours| Utc::now() + Duration::hours(hours.clamp(1, 24 * 30)))
    });

    match file_share_service.create_share(file_id, user_id, expires_at).await {
        Ok(Some(share)) => Json(CreateShareResponse {
            id: share.id,
            uuid: share.uuid.clone(),
            file_id: share.file_id,
            expires_at: share.expires_at,
            created_at: share.created_at,
            url: format!("/share/{}", share.uuid),
        })
        .into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to create share: {}", error) }),
        ),
    }
}

pub async fn get_file_share(Path(uuid): Path<String>, State(state): State<AppState>) -> Response {
    let file_share_service = FileShareService::new(state.db.clone());
    match file_share_service.get_share_by_uuid(&uuid).await {
        Ok(Some(share)) => Json(share).into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "share not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load share: {}", error) }),
        ),
    }
}

pub async fn view_shared_file(Path(uuid): Path<String>, State(state): State<AppState>) -> Response {
    let file_share_service = FileShareService::new(state.db.clone());
    let share = match file_share_service.get_share_by_uuid(&uuid).await {
        Ok(Some(share)) => share,
        Ok(None) => return share_not_found_page(),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to load share: {}", error) }),
            );
        }
    };

    if share.expires_at <= Utc::now() {
        return share_not_found_page();
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    match file_service.view_file_bytes(share.file_id).await {
        Ok(Some((file_name, bytes))) => bytes_response(&file_name, content_type_for_name(&file_name), bytes),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to view share: {}", error) }),
        ),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}

fn share_not_found_page() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(
            r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Share Not Found</title>
  <style>
    :root { color-scheme: light dark; }
    body {
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #f6f7fb;
      color: #111827;
    }
    main {
      width: min(520px, calc(100vw - 32px));
      text-align: center;
    }
    h1 {
      margin: 0 0 10px;
      font-size: 32px;
      font-weight: 750;
    }
    p {
      margin: 0;
      color: #5b6472;
      font-size: 16px;
      line-height: 1.5;
    }
    @media (prefers-color-scheme: dark) {
      body { background: #0f172a; color: #f8fafc; }
      p { color: #cbd5e1; }
    }
  </style>
</head>
<body>
  <main>
    <h1>Page not found</h1>
    <p>This share link does not exist or is no longer available.</p>
  </main>
</body>
</html>"#,
        ),
    )
        .into_response()
}

fn bytes_response(file_name: &str, content_type: &'static str, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&format!("inline; filename=\"{}\"", file_name.replace('"', "_"))) {
        response.headers_mut().insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn content_type_for_name(file_name: &str) -> &'static str {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" | "md" | "json" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
