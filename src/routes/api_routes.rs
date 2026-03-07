use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;
use anyhow::{anyhow, Result as AnyResult};
use tracing::error;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::services::file_service::FileService;
use crate::services::r2_storage_service::R2StorageService;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/files", get(list_files))
        .route("/api/files/:id/view", get(view_file))
        .route("/api/files/:id/download", get(download_file))
        .route("/api/files/:id", delete(delete_file))
        // Allow large multipart uploads (up to 10 GB).
        .layer(DefaultBodyLimit::max(10usize * 1024 * 1024 * 1024))
}

async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => {
                return json_response(StatusCode::BAD_REQUEST, json!({ "error": "missing file field" }));
            }
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": format!("invalid multipart body: {}", error) }),
                );
            }
        };

        if field.name() != Some("file") {
            continue;
        }

        let original_name = field
            .file_name()
            .map(str::to_string);

        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": format!("failed to read file bytes: {}", error) }),
                );
            }
        };

        let file_service = FileService::new(
            state.db.clone(),
            state.compressor.clone(),
            state.media_transcoder.clone(),
            state.storage_backend.clone(),
            state.r2_storage.clone(),
            state.upload_dir.clone(),
        );

        return match file_service
            .upload_and_compress(original_name.as_deref(), &bytes, user_id)
            .await
        {
            Ok(upload_result) => (StatusCode::CREATED, Json(upload_result)).into_response(),
            Err(error) => {
                error!(error = ?error, "upload pipeline failed");
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("upload failed: {:#}", error) }),
                )
            }
        };
    }
}

async fn list_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    match file_service.list_files().await {
        Ok(files) => Json(files).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query files: {}", error) }),
        ),
    }
}

async fn view_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    }

    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let file = match file_service.get_file_by_id(id).await {
        Ok(Some(file)) => file,
        Ok(None) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("view failed: {}", error) }),
            );
        }
    };

    let content_type = content_type_for_name(&file.original_name);
    if file.is_compressed {
        return match file_service.view_file_bytes(id).await {
            Ok(Some((file_name, bytes))) => bytes_response(&file_name, content_type, bytes, false),
            Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("view failed: {}", error) }),
            ),
        };
    }

    if R2StorageService::is_r2_path(&file.stored_path) {
        return match stream_r2_object_response(
            &state,
            &file.stored_path,
            &file.original_name,
            content_type,
            false,
            range_header.as_deref(),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("view failed: {}", error) }),
            ),
        };
    }

    match stream_uncompressed_file_response(
        &file.stored_path,
        &file.original_name,
        content_type,
        false,
        range_header.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("view failed: {}", error) }),
        ),
    }
}

async fn download_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let file = match file_service.get_file_by_id(id).await {
        Ok(Some(file)) => file,
        Ok(None) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("download failed: {}", error) }),
            );
        }
    };

    let content_type = content_type_for_name(&file.original_name);
    if file.is_compressed {
        return match file_service.view_file_bytes(id).await {
            Ok(Some((file_name, bytes))) => bytes_response(&file_name, content_type, bytes, true),
            Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("download failed: {}", error) }),
            ),
        };
    }

    if R2StorageService::is_r2_path(&file.stored_path) {
        return match stream_r2_object_response(
            &state,
            &file.stored_path,
            &file.original_name,
            content_type,
            true,
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("download failed: {}", error) }),
            ),
        };
    }

    match stream_uncompressed_file_response(
        &file.stored_path,
        &file.original_name,
        content_type,
        true,
        None,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("download failed: {}", error) }),
        ),
    }
}

async fn delete_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    match file_service.delete_file_by_id(id).await {
        Ok(true) => json_response(StatusCode::OK, json!({ "deleted": true, "id": id })),
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("delete failed: {}", error) }),
        ),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}

fn bytes_response(
    file_name: &str,
    content_type: &'static str,
    bytes: Vec<u8>,
    as_attachment: bool,
) -> Response {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

async fn stream_r2_object_response(
    state: &AppState,
    db_path: &str,
    file_name: &str,
    content_type: &'static str,
    as_attachment: bool,
    range_header: Option<&str>,
) -> AnyResult<Response> {
    let r2_storage = state
        .r2_storage
        .as_ref()
        .ok_or_else(|| anyhow!("R2 storage is not configured"))?;

    let object_stream = r2_storage.get_object_stream(db_path, range_header).await?;
    let status = if object_stream.content_range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    let stream = ReaderStream::new(object_stream.body.into_async_read());
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;

    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    if let Some(content_length) = object_stream.content_length {
        if content_length >= 0 {
            if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_LENGTH, value);
            }
        }
    }

    if let Some(content_range) = object_stream.content_range {
        if let Ok(value) = HeaderValue::from_str(&content_range) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }

    if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }

    Ok(response)
}

async fn stream_uncompressed_file_response(
    file_path: &str,
    file_name: &str,
    content_type: &'static str,
    as_attachment: bool,
    range_header: Option<&str>,
) -> std::io::Result<Response> {
    let total_size = tokio::fs::metadata(file_path).await?.len();

    if total_size == 0 {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::OK;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        response
            .headers_mut()
            .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
        return Ok(response);
    }

    let parsed_range = parse_range_header(range_header, total_size);
    if matches!(parsed_range, ParsedRange::Invalid) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        response
            .headers_mut()
            .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total_size}")) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
        if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
        return Ok(response);
    }

    let mut file = File::open(file_path).await?;
    let (status, body, content_length, content_range) = match parsed_range {
        ParsedRange::Full => {
            let stream = ReaderStream::new(file);
            (StatusCode::OK, Body::from_stream(stream), total_size, None)
        }
        ParsedRange::Partial { start, end } => {
            let chunk_len = end - start + 1;
            file.seek(SeekFrom::Start(start)).await?;
            let stream = ReaderStream::new(file.take(chunk_len));
            (
                StatusCode::PARTIAL_CONTENT,
                Body::from_stream(stream),
                chunk_len,
                Some(format!("bytes {start}-{end}/{total_size}")),
            )
        }
        ParsedRange::Invalid => unreachable!(),
    };

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, value);
    }
    if let Some(content_range) = content_range {
        if let Ok(value) = HeaderValue::from_str(&content_range) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
    if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }

    Ok(response)
}

fn content_disposition_value(file_name: &str, as_attachment: bool) -> String {
    let disposition_kind = if as_attachment { "attachment" } else { "inline" };
    format!("{disposition_kind}; filename=\"{}\"", file_name.replace('"', "_"))
}

enum ParsedRange {
    Full,
    Partial { start: u64, end: u64 },
    Invalid,
}

fn parse_range_header(range_header: Option<&str>, total_size: u64) -> ParsedRange {
    let Some(range_header) = range_header else {
        return ParsedRange::Full;
    };

    let Some(specifier) = range_header.strip_prefix("bytes=") else {
        return ParsedRange::Invalid;
    };

    let first = specifier.split(',').next().unwrap_or("").trim();
    if first.is_empty() {
        return ParsedRange::Invalid;
    }

    let Some((start_raw, end_raw)) = first.split_once('-') else {
        return ParsedRange::Invalid;
    };

    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        let Ok(suffix_len) = end_raw.parse::<u64>() else {
            return ParsedRange::Invalid;
        };
        if suffix_len == 0 {
            return ParsedRange::Invalid;
        }
        let start = total_size.saturating_sub(suffix_len);
        let end = total_size.saturating_sub(1);
        return ParsedRange::Partial { start, end };
    }

    let Ok(start) = start_raw.parse::<u64>() else {
        return ParsedRange::Invalid;
    };
    if start >= total_size {
        return ParsedRange::Invalid;
    }

    let end = if end_raw.is_empty() {
        total_size.saturating_sub(1)
    } else {
        let Ok(parsed_end) = end_raw.parse::<u64>() else {
            return ParsedRange::Invalid;
        };
        parsed_end.min(total_size.saturating_sub(1))
    };

    if end < start {
        return ParsedRange::Invalid;
    }

    ParsedRange::Partial { start, end }
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
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "txt" | "md" | "rs" | "json" | "toml" | "yaml" | "yml" | "csv" => {
            "text/plain; charset=utf-8"
        }
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}
