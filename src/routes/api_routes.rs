use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio_util::io::ReaderStream;
use anyhow::{anyhow, Result as AnyResult};
use tracing::error;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::db;
use crate::models::file_model::FileRecord;
use crate::models::folder_model::NewFolderRecord;
use crate::models::upload_job_model::{NewUploadJobRecord, UploadJobStatus};
use crate::services::auth_service::AuthService;
use crate::services::file_service::FileService;
use crate::services::r2_storage_service::R2StorageService;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(get_me))
        .route("/api/upload", post(upload_file))
        .route("/api/jobs/:id", get(get_upload_job))
        .route("/api/folders", get(list_folders).post(create_folder))
        .route("/api/folders/tree", get(list_folders_tree))
        .route("/api/folders/:id", patch(update_folder).delete(delete_folder))
        .route("/api/folders/path", get(get_folder_path))
        .route("/api/files", get(list_files))
        .route("/api/files/summary", get(get_file_summary))
        .route("/api/files/move-batch", patch(move_files_batch))
        .route("/api/files/delete-batch", post(delete_files_batch))
        .route("/api/files/:id/move", patch(move_file_to_folder))
        .route("/api/files/:id/thumb", get(file_thumbnail))
        .route("/api/files/:id/view", get(view_file))
        .route("/api/files/:id/download", get(download_file))
        .route("/api/files/:id", delete(delete_file))
        // Allow large multipart uploads (up to 10 GB).
        .layer(DefaultBodyLimit::max(10usize * 1024 * 1024 * 1024))
}

#[derive(Debug, Deserialize)]
struct FilesQuery {
    folder_id: Option<i64>,
    cursor: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FoldersQuery {
    parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FolderPathQuery {
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateFolderRequest {
    name: String,
    parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpdateFolderRequest {
    name: Option<String>,
    #[serde(default)]
    parent_id: Option<Option<i64>>,
}

#[derive(Debug, Deserialize)]
struct MoveFileRequest {
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MoveFilesBatchRequest {
    ids: Vec<i64>,
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DeleteFilesBatchRequest {
    ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
struct FileSummaryResponse {
    total_files: i64,
    total_original_size: i64,
    total_stored_size: i64,
}

#[derive(Debug, Serialize)]
struct CurrentUserResponse {
    id: i64,
    username: String,
}

#[derive(Debug, Serialize)]
struct PaginatedFilesResponse {
    items: Vec<FileRecord>,
    next_cursor: Option<i64>,
    has_more: bool,
}

async fn get_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let auth_service = AuthService::new(state.db.clone());
    match auth_service.username_by_id(user_id).await {
        Ok(Some(username)) => Json(CurrentUserResponse { id: user_id, username }).into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "user not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load current user: {}", error) }),
        ),
    }
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

    let mut selected_folder_id: Option<i64> = None;
    let mut pending_file: Option<(String, std::path::PathBuf)> = None;

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": format!("invalid multipart body: {}", error) }),
                );
            }
        };

        match field.name() {
            Some("folder_id") => {
                let raw_value = match field.text().await {
                    Ok(value) => value,
                    Err(error) => {
                        return json_response(
                            StatusCode::BAD_REQUEST,
                            json!({ "error": format!("invalid folder_id field: {}", error) }),
                        );
                    }
                };
                match parse_optional_i64(raw_value.trim()) {
                    Ok(parsed) => selected_folder_id = parsed,
                    Err(message) => {
                        return json_response(StatusCode::BAD_REQUEST, json!({ "error": message }));
                    }
                }
            }
            Some("file") => {
                if pending_file.is_some() {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": "only one file per upload request is supported" }),
                    );
                }

                let original_name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "upload.bin".to_string());

                let temp_dir = state.upload_dir.join(".upload_jobs");
                if let Err(error) = tokio::fs::create_dir_all(&temp_dir).await {
                    return json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": format!("failed to prepare upload temp dir: {}", error) }),
                    );
                }

                let temp_name = format!(
                    "{}_{}",
                    Uuid::new_v4(),
                    sanitize_upload_name_for_temp(&original_name)
                );
                let temp_path = temp_dir.join(temp_name);
                let mut output = match File::create(&temp_path).await {
                    Ok(file) => file,
                    Err(error) => {
                        return json_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            json!({ "error": format!("failed to allocate upload temp file: {}", error) }),
                        );
                    }
                };

                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            if let Err(error) = output.write_all(&chunk).await {
                                let _ = tokio::fs::remove_file(&temp_path).await;
                                return json_response(
                                    StatusCode::BAD_REQUEST,
                                    json!({ "error": format!("failed to stream upload data: {}", error) }),
                                );
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = tokio::fs::remove_file(&temp_path).await;
                            return json_response(
                                StatusCode::BAD_REQUEST,
                                json!({ "error": format!("invalid multipart chunk: {}", error) }),
                            );
                        }
                    }
                }

                if let Err(error) = output.flush().await {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": format!("failed to finalize upload file: {}", error) }),
                    );
                }

                pending_file = Some((original_name, temp_path));
            }
            _ => {}
        }
    }

    let Some((original_name, temp_path)) = pending_file else {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "missing file field" }));
    };

    if let Some(folder_id) = selected_folder_id {
        match db::folder_exists_for_user(&state.db, folder_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_response(StatusCode::BAD_REQUEST, json!({ "error": "invalid folder_id" }));
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate folder: {}", error) }),
                );
            }
        }
    }

    let temp_path_str = temp_path.to_string_lossy().to_string();
    let job_id = match db::insert_upload_job(
        &state.db,
        &NewUploadJobRecord {
            original_name: &original_name,
            temp_path: &temp_path_str,
            uploaded_by: user_id,
            folder_id: selected_folder_id,
        },
    )
    .await
    {
        Ok(job_id) => job_id,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            error!(error = ?error, "failed to create upload job");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "failed to queue upload job" }),
            );
        }
    };

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "job_id": job_id,
            "status": "pending",
            "folder_id": selected_folder_id,
            "original_name": original_name
        })),
    )
        .into_response()
}

async fn get_upload_job(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    match db::get_upload_job_by_id_for_user(&state.db, id, user_id).await {
        Ok(Some(job)) => Json(UploadJobStatus::from(job)).into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "job not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query job: {}", error) }),
        ),
    }
}

async fn list_folders(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FoldersQuery>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    if let Some(parent_id) = query.parent_id {
        match db::folder_exists_for_user(&state.db, parent_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid parent folder" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate parent folder: {}", error) }),
                );
            }
        }
    }

    match db::list_folders_for_user_and_parent(&state.db, user_id, query.parent_id).await {
        Ok(folders) => Json(folders).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query folders: {}", error) }),
        ),
    }
}

async fn list_folders_tree(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    match db::list_all_folders_with_counts_for_user(&state.db, user_id).await {
        Ok(folders) => Json(folders).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query folder tree: {}", error) }),
        ),
    }
}

async fn create_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateFolderRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    let folder_name = sanitize_folder_name(&payload.name);
    if folder_name.is_empty() {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "folder name is required" }));
    }

    if let Some(parent_id) = payload.parent_id {
        match db::folder_exists_for_user(&state.db, parent_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid parent folder" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate parent folder: {}", error) }),
                );
            }
        }
    }

    match db::folder_name_exists_for_user_and_parent(&state.db, user_id, payload.parent_id, &folder_name).await {
        Ok(true) => {
            return json_response(
                StatusCode::CONFLICT,
                json!({ "error": "folder with same name already exists in this location" }),
            );
        }
        Ok(false) => {}
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder name: {}", error) }),
            );
        }
    }

    let folder_id = match db::insert_folder(
        &state.db,
        &NewFolderRecord {
            name: &folder_name,
            parent_id: payload.parent_id,
            created_by: user_id,
        },
    )
    .await
    {
        Ok(folder_id) => folder_id,
        Err(error) => {
            let message = error.to_string();
            if message.contains("UNIQUE constraint failed") {
                return json_response(
                    StatusCode::CONFLICT,
                    json!({ "error": "folder with same name already exists in this location" }),
                );
            }
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to create folder: {}", error) }),
            );
        }
    };

    match db::get_folder_by_id_for_user(&state.db, folder_id, user_id).await {
        Ok(Some(folder)) => (StatusCode::CREATED, Json(folder)).into_response(),
        Ok(None) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "folder was created but could not be loaded" }),
        ),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load folder: {}", error) }),
        ),
    }
}

async fn update_folder(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateFolderRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    let existing = match db::get_folder_by_id_for_user(&state.db, id, user_id).await {
        Ok(Some(folder)) => folder,
        Ok(None) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" })),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to load folder: {}", error) }),
            );
        }
    };

    let target_name = match payload.name {
        Some(name) => sanitize_folder_name(&name),
        None => existing.name.clone(),
    };
    if target_name.is_empty() {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "folder name is required" }));
    }

    let target_parent_id = payload.parent_id.unwrap_or(existing.parent_id);
    if target_parent_id == Some(id) {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "folder cannot be its own parent" }),
        );
    }

    if let Some(parent_id) = target_parent_id {
        match db::get_folder_by_id_for_user(&state.db, parent_id, user_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid parent folder" }),
                );
            }
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate parent folder: {}", error) }),
                );
            }
        }

        let path = match db::list_folder_path_for_user(&state.db, parent_id, user_id).await {
            Ok(path) => path,
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate folder move: {}", error) }),
                );
            }
        };
        if path.iter().any(|folder| folder.id == id) {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "cannot move folder into itself or its descendants" }),
            );
        }
    }

    if existing.name == target_name && existing.parent_id == target_parent_id {
        return Json(existing).into_response();
    }

    match db::folder_name_exists_for_user_and_parent_except(
        &state.db,
        user_id,
        target_parent_id,
        &target_name,
        id,
    )
    .await
    {
        Ok(true) => {
            return json_response(
                StatusCode::CONFLICT,
                json!({ "error": "folder with same name already exists in this location" }),
            );
        }
        Ok(false) => {}
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder name: {}", error) }),
            );
        }
    }

    match db::update_folder_by_id_for_user(&state.db, id, user_id, &target_name, target_parent_id).await {
        Ok(true) => {}
        Ok(false) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" })),
        Err(error) => {
            let message = error.to_string();
            if message.contains("UNIQUE constraint failed") {
                return json_response(
                    StatusCode::CONFLICT,
                    json!({ "error": "folder with same name already exists in this location" }),
                );
            }
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to update folder: {}", error) }),
            );
        }
    }

    match db::get_folder_by_id_for_user(&state.db, id, user_id).await {
        Ok(Some(folder)) => Json(folder).into_response(),
        Ok(None) => json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load folder: {}", error) }),
        ),
    }
}

async fn delete_folder(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    let exists = match db::folder_exists_for_user(&state.db, id, user_id).await {
        Ok(exists) => exists,
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder: {}", error) }),
            );
        }
    };
    if !exists {
        return json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" }));
    }

    let direct_subfolders = match db::count_direct_subfolders_for_user(&state.db, user_id, id).await {
        Ok(count) => count,
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to check subfolders: {}", error) }),
            );
        }
    };
    if direct_subfolders > 0 {
        return json_response(
            StatusCode::CONFLICT,
            json!({ "error": "folder is not empty (contains subfolders)" }),
        );
    }

    let direct_files = match db::count_files_for_user_in_folder(&state.db, user_id, id).await {
        Ok(count) => count,
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to check files: {}", error) }),
            );
        }
    };
    if direct_files > 0 {
        return json_response(
            StatusCode::CONFLICT,
            json!({ "error": "folder is not empty (contains files)" }),
        );
    }

    let active_jobs = match db::count_active_upload_jobs_for_user_in_folder(&state.db, user_id, id).await {
        Ok(count) => count,
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to check upload jobs: {}", error) }),
            );
        }
    };
    if active_jobs > 0 {
        return json_response(
            StatusCode::CONFLICT,
            json!({ "error": "folder has active upload jobs; wait for them to finish" }),
        );
    }

    match db::delete_folder_by_id_for_user(&state.db, id, user_id).await {
        Ok(true) => json_response(StatusCode::OK, json!({ "deleted": true, "id": id })),
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to delete folder: {}", error) }),
        ),
    }
}

async fn get_folder_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FolderPathQuery>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => {
            return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
    };

    let Some(folder_id) = query.folder_id else {
        return Json(Vec::<crate::models::folder_model::FolderRecord>::new()).into_response();
    };

    match db::list_folder_path_for_user(&state.db, folder_id, user_id).await {
        Ok(path) => {
            let valid = path.last().map(|folder| folder.id == folder_id).unwrap_or(false);
            if !valid {
                return json_response(StatusCode::NOT_FOUND, json!({ "error": "folder not found" }));
            }
            Json(path).into_response()
        }
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load folder path: {}", error) }),
        ),
    }
}

async fn list_files(
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
            Ok(false) => return json_response(StatusCode::BAD_REQUEST, json!({ "error": "invalid folder_id" })),
            Err(error) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": format!("failed to validate folder: {}", error) }),
                );
            }
        }
    }

    if query.limit.is_some() || query.cursor.is_some() {
        let limit = query.limit.unwrap_or(60).clamp(1, 200) as i64;
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

    match file_service.list_files_for_user(user_id, query.folder_id).await {
        Ok(files) => Json(files).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query files: {}", error) }),
        ),
    }
}

async fn get_file_summary(State(state): State<AppState>, headers: HeaderMap) -> Response {
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

async fn move_file_to_folder(
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
        Ok(Some(_)) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Ok(None) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
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
        Ok(true) => json_response(
            StatusCode::OK,
            json!({ "moved": true, "id": id, "folder_id": target_folder_id }),
        ),
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to move file: {}", error) }),
        ),
    }
}

async fn move_files_batch(
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
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "ids must not be empty" }));
    }
    if ids.len() > 500 {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "too many ids (max 500)" }));
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
                failed.push(json!({ "id": file_id, "error": format!("failed to load file: {}", error) }));
                continue;
            }
        };

        if file.folder_id == payload.folder_id {
            moved_ids.push(file_id);
            continue;
        }

        match db::move_file_to_folder_for_user(&state.db, file_id, user_id, payload.folder_id).await {
            Ok(true) => moved_ids.push(file_id),
            Ok(false) => failed.push(json!({ "id": file_id, "error": "file not found" })),
            Err(error) => {
                failed.push(json!({ "id": file_id, "error": format!("failed to move file: {}", error) }));
            }
        }
    }

    json_response(
        StatusCode::OK,
        json!({
            "moved_ids": moved_ids,
            "failed": failed
        }),
    )
}

async fn delete_files_batch(
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
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "ids must not be empty" }));
    }
    if ids.len() > 500 {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": "too many ids (max 500)" }));
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
            Err(error) => failed.push(json!({ "id": file_id, "error": format!("delete failed: {}", error) })),
        }
    }

    json_response(
        StatusCode::OK,
        json!({
            "deleted_ids": deleted_ids,
            "failed": failed
        }),
    )
}

async fn file_thumbnail(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })),
    };

    let file = match db::get_file_by_id(&state.db, id).await {
        Ok(Some(file)) if file.uploaded_by == user_id => file,
        Ok(Some(_)) | Ok(None) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
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
    json_response(StatusCode::NOT_FOUND, json!({ "error": "thumbnail not available for this file type" }))
}

async fn view_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response {
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
        Ok(Some(_)) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
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
        Ok(Some(_)) => return json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
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
        Ok(true) => json_response(StatusCode::OK, json!({ "deleted": true, "id": id })),
        Ok(false) => json_response(StatusCode::NOT_FOUND, json!({ "error": "file not found" })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("delete failed: {}", error) }),
        ),
    }
}

fn sanitize_upload_name_for_temp(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '_',
        })
        .collect();

    if sanitized.is_empty() {
        "upload.bin".to_string()
    } else {
        sanitized
    }
}

fn sanitize_folder_name(name: &str) -> String {
    name.trim()
        .chars()
        .take(80)
        .map(|ch| match ch {
            '/' | '\\' => '_',
            _ => ch,
        })
        .collect::<String>()
}

fn parse_optional_i64(raw: &str) -> Result<Option<i64>, &'static str> {
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }

    match raw.parse::<i64>() {
        Ok(value) if value > 0 => Ok(Some(value)),
        _ => Err("folder_id must be a positive integer"),
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
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg")
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
