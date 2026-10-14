use anyhow::{Result as AnyResult, anyhow};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::db;
use crate::events::{
    dispatch_to_user,
    drive::{FilesDeleted, FilesMoved},
};
use crate::models::file_model::FileRecord;
use crate::services::file_service::FileService;
use crate::services::r2_storage_service::R2StorageService;

#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    folder_id: Option<i64>,
    cursor: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct MoveFileRequest {
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MoveFilesBatchRequest {
    ids: Vec<i64>,
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteFilesBatchRequest {
    ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
struct FileSummaryResponse {
    total_files: i64,
    total_original_size: i64,
    total_stored_size: i64,
}

#[derive(Debug, Serialize)]
struct PaginatedFilesResponse {
    items: Vec<FileRecord>,
    next_cursor: Option<i64>,
    has_more: bool,
}

pub async fn list_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FilesQuery>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    if let Some(folder_id) = query.folder_id {
        match db::folder_exists_for_user(&state.db, folder_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid folder_id" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate folder: {}", error) }),
                );
            }
        }
    }

    if query.limit.is_some() || query.cursor.is_some() {
        let limit = query.limit.unwrap_or(30).clamp(1, 200) as i64;
        let files = match db::list_files_for_user_and_folder_page(
            &state.db,
            user_id,
            query.folder_id,
            query.cursor,
            limit + 1,
        )
        .await
        {
            Ok(files) => files,
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to query files: {}", error) }),
                );
            }
        };

        let has_more = files.len() as i64 > limit;
        let mut items = files;
        if has_more {
            items.pop();
        }
        let next_cursor = items.last().map(|file| file.id);

        return Json(PaginatedFilesResponse {
            items,
            next_cursor,
            has_more,
        })
        .into_response();
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    match file_service
        .list_files_for_user(user_id, query.folder_id)
        .await
    {
        Ok(files) => Json(files).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query files: {}", error) }),
        ),
    }
}

pub async fn get_file_summary(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    match db::summarize_files_for_user(&state.db, user_id).await {
        Ok((total_files, total_original_size, total_stored_size)) => Json(FileSummaryResponse {
            total_files,
            total_original_size,
            total_stored_size,
        })
        .into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to summarize files: {}", error) }),
        ),
    }
}

pub async fn move_file_to_folder(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MoveFileRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let target_folder_id = payload.folder_id;

    let file = match db::get_file_by_id(&state.db, id).await {
        Ok(Some(file)) if file.uploaded_by == user_id => file,
        Ok(Some(_)) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
        Ok(None) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to load file: {}", error) }),
            );
        }
    };

    if let Some(folder_id) = target_folder_id {
        match db::folder_exists_for_user(&state.db, folder_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid target folder" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate target folder: {}", error) }),
                );
            }
        }
    }

    if file.folder_id == target_folder_id {
        return json_response(
            StatusCode::OK,
            json!({ "moved": true, "id": id, "folder_id": target_folder_id }),
        );
    }

    match db::move_file_to_folder_for_user(&state.db, id, user_id, target_folder_id).await {
        Ok(true) => {
            dispatch_to_user(
                &state.websocket_hub,
                user_id,
                &FilesMoved {
                    file_ids: &[id],
                    folder_id: target_folder_id,
                },
            )
            .await;
            json_response(
                StatusCode::OK,
                json!({ "moved": true, "id": id, "folder_id": target_folder_id }),
            )
        }
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to move file: {}", error) }),
        ),
    }
}

pub async fn move_files_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MoveFilesBatchRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let ids = normalize_ids(&payload.ids);
    if ids.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "ids must not be empty" }),
        );
    }
    if ids.len() > 500 {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "too many ids (max 500)" }),
        );
    }

    if let Some(folder_id) = payload.folder_id {
        match db::folder_exists_for_user(&state.db, folder_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid target folder" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate target folder: {}", error) }),
                );
            }
        }
    }

    let mut moved_ids = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();

    for file_id in ids {
        let file = match db::get_file_by_id(&state.db, file_id).await {
            Ok(Some(file)) if file.uploaded_by == user_id => file,
            Ok(Some(_)) | Ok(None) => {
                failed.push(json!({ "id": file_id, "error": "file not found" }));
                continue;
            }
            Err(error) => {
                failed.push(
                    json!({ "id": file_id, "error": format!("failed to load file: {}", error) }),
                );
                continue;
            }
        };

        if file.folder_id == payload.folder_id {
            moved_ids.push(file_id);
            continue;
        }

        match db::move_file_to_folder_for_user(&state.db, file_id, user_id, payload.folder_id).await
        {
            Ok(true) => moved_ids.push(file_id),
            Ok(false) => failed.push(json!({ "id": file_id, "error": "file not found" })),
            Err(error) => {
                failed.push(
                    json!({ "id": file_id, "error": format!("failed to move file: {}", error) }),
                );
            }
        }
    }

    if !moved_ids.is_empty() {
        dispatch_to_user(
            &state.websocket_hub,
            user_id,
            &FilesMoved {
                file_ids: &moved_ids,
                folder_id: payload.folder_id,
            },
        )
        .await;
    }

    json_response(
        StatusCode::OK,
        json!({
            "moved_ids": moved_ids,
            "failed": failed
        }),
    )
}

pub async fn delete_files_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<DeleteFilesBatchRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let ids = normalize_ids(&payload.ids);
    if ids.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "ids must not be empty" }),
        );
    }
    if ids.len() > 500 {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "too many ids (max 500)" }),
        );
    }

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let mut deleted_ids = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();

    for file_id in ids {
        match file_service.delete_file_by_id(file_id, user_id).await {
            Ok(true) => deleted_ids.push(file_id),
            Ok(false) => failed.push(json!({ "id": file_id, "error": "file not found" })),
            Err(error) => {
                failed.push(json!({ "id": file_id, "error": format!("delete failed: {}", error) }))
            }
        }
    }

    if !deleted_ids.is_empty() {
        dispatch_to_user(
            &state.websocket_hub,
            user_id,
            &FilesDeleted {
                file_ids: &deleted_ids,
            },
        )
        .await;
    }

    json_response(
        StatusCode::OK,
        json!({
            "deleted_ids": deleted_ids,
            "failed": failed
        }),
    )
}

pub async fn file_thumbnail(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let file = match db::get_file_by_id(&state.db, id).await {
        Ok(Some(file)) if file.uploaded_by == user_id => file,
        Ok(Some(_)) | Ok(None) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to load file: {}", error) }),
            );
        }
    };

    if is_image_name(&file.original_name) {
        return Redirect::to(&format!("/api/files/{}/view", id)).into_response();
    }
    if let Some(thumbnail_path) = file.thumbnail_path.as_deref() {
        let result = if R2StorageService::is_r2_path(thumbnail_path) {
            stream_r2_object_response(
                &state,
                thumbnail_path,
                "thumbnail.jpg",
                "image/jpeg",
                false,
                None,
            )
            .await
            .map_err(|error| error.to_string())
        } else {
            stream_uncompressed_file_response(
                thumbnail_path,
                "thumbnail.jpg",
                "image/jpeg",
                false,
                None,
            )
            .await
            .map_err(|error| error.to_string())
        };

        return match result {
            Ok(mut response) => {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("private, max-age=86400"),
                );
                response
            }
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("thumbnail failed: {}", error) }),
            ),
        };
    }
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "error": "thumbnail not available for this file type" }),
    )
}

pub async fn view_file(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

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
        Ok(Some(file)) if file.uploaded_by == user_id => file,
        Ok(Some(_)) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
        Ok(None) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
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
            Ok(Some((file_name, bytes))) => bytes_response(
                &file_name,
                detected_content_type(&bytes).unwrap_or(content_type),
                bytes,
                false,
                range_header.as_deref(),
            ),
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

pub async fn download_file(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let file = match file_service.get_file_by_id(id).await {
        Ok(Some(file)) if file.uploaded_by == user_id => file,
        Ok(Some(_)) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
        Ok(None) => {
            return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" }));
        }
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
            Ok(Some((file_name, bytes))) => bytes_response(
                &file_name,
                detected_content_type(&bytes).unwrap_or(content_type),
                bytes,
                true,
                None,
            ),
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

pub async fn delete_file(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    match file_service.delete_file_by_id(id, user_id).await {
        Ok(true) => {
            dispatch_to_user(
                &state.websocket_hub,
                user_id,
                &FilesDeleted { file_ids: &[id] },
            )
            .await;
            json_response(StatusCode::OK, json!({ "deleted": true, "id": id }))
        }
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("delete failed: {}", error) }),
        ),
    }
}

fn normalize_ids(ids: &[i64]) -> Vec<i64> {
    let mut list: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    list.sort_unstable();
    list.dedup();
    list
}

fn is_image_name(file_name: &str) -> bool {
    let ext = file_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg"
    )
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}

fn bytes_response(
    file_name: &str,
    content_type: &'static str,
    bytes: Vec<u8>,
    as_attachment: bool,
    range_header: Option<&str>,
) -> Response {
    let total_size = bytes.len() as u64;
    let parsed_range = parse_range_header(range_header, total_size);
    if matches!(parsed_range, ParsedRange::Invalid) {
        return range_not_satisfiable_response(total_size, content_type, file_name, as_attachment);
    }

    let (status, body, content_length, content_range) = match parsed_range {
        ParsedRange::Full => (StatusCode::OK, Body::from(bytes), total_size, None),
        ParsedRange::Partial { start, end } => {
            let content_length = end - start + 1;
            let body = Body::from(bytes[start as usize..=end as usize].to_vec());
            (
                StatusCode::PARTIAL_CONTENT,
                body,
                content_length,
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
    apply_media_headers(
        &mut response,
        file_name,
        as_attachment,
        content_length,
        content_range.as_deref(),
    );
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
    apply_media_headers(
        &mut response,
        file_name,
        as_attachment,
        object_stream.content_length.unwrap_or_default().max(0) as u64,
        object_stream.content_range.as_deref(),
    );

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
        if let Ok(value) =
            HeaderValue::from_str(&content_disposition_value(file_name, as_attachment))
        {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
        return Ok(response);
    }

    let parsed_range = parse_range_header(range_header, total_size);
    if matches!(parsed_range, ParsedRange::Invalid) {
        return Ok(range_not_satisfiable_response(
            total_size,
            content_type,
            file_name,
            as_attachment,
        ));
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
    apply_media_headers(
        &mut response,
        file_name,
        as_attachment,
        content_length,
        content_range.as_deref(),
    );

    Ok(response)
}

fn content_disposition_value(file_name: &str, as_attachment: bool) -> String {
    let disposition_kind = if as_attachment {
        "attachment"
    } else {
        "inline"
    };
    let safe_name = file_name
        .chars()
        .map(|character| match character {
            '"' | '\r' | '\n' | '\\' => '_',
            _ => character,
        })
        .collect::<String>();
    format!("{disposition_kind}; filename=\"{safe_name}\"")
}

fn apply_media_headers(
    response: &mut Response,
    file_name: &str,
    as_attachment: bool,
    content_length: u64,
    content_range: Option<&str>,
) {
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    if let Some(content_range) = content_range
        && let Ok(value) = HeaderValue::from_str(content_range)
    {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&content_disposition_value(file_name, as_attachment)) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
}

fn range_not_satisfiable_response(
    total_size: u64,
    content_type: &'static str,
    file_name: &str,
    as_attachment: bool,
) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    apply_media_headers(&mut response, file_name, as_attachment, 0, None);
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total_size}")) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
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

    if specifier.contains(',') || total_size == 0 {
        return ParsedRange::Invalid;
    }
    let first = specifier.trim();
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

fn detected_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        Some("video/mp4")
    } else if bytes.starts_with(b"\x1A\x45\xDF\xA3") {
        Some("video/webm")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::{ParsedRange, parse_range_header};

    #[test]
    fn parses_bounded_open_and_suffix_ranges() {
        assert!(matches!(
            parse_range_header(Some("bytes=2-5"), 10),
            ParsedRange::Partial { start: 2, end: 5 }
        ));
        assert!(matches!(
            parse_range_header(Some("bytes=7-"), 10),
            ParsedRange::Partial { start: 7, end: 9 }
        ));
        assert!(matches!(
            parse_range_header(Some("bytes=-3"), 10),
            ParsedRange::Partial { start: 7, end: 9 }
        ));
    }

    #[test]
    fn rejects_malformed_unsatisfiable_and_multiple_ranges() {
        for value in [
            "items=0-1",
            "bytes=",
            "bytes=20-",
            "bytes=4-2",
            "bytes=0-1,4-5",
            "bytes=-0",
        ] {
            assert!(
                matches!(parse_range_header(Some(value), 10), ParsedRange::Invalid),
                "{value}"
            );
        }
    }
}
