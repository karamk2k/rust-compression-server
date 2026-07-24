use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::pages::share_not_found_page;
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

    match file_share_service
        .create_share(file_id, user_id, expires_at)
        .await
    {
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
        Ok(Some((file_name, bytes))) => {
            bytes_response(&file_name, content_type_for_name(&file_name), bytes)
        }
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

fn bytes_response(file_name: &str, content_type: &'static str, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&format!(
        "inline; filename=\"{}\"",
        file_name.replace('"', "_")
    )) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn content_type_for_name(file_name: &str) -> &'static str {
    let ext = file_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
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
