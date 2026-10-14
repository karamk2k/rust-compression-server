use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::db;
use crate::events::{
    dispatch_to_user,
    drive::{FolderCreated, FolderDeleted, FolderMoved, FolderRenamed},
};
use crate::models::folder_model::{FolderRecord, NewFolderRecord};

#[derive(Debug, Deserialize)]
pub struct FoldersQuery {
    parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FolderPathQuery {
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    name: String,
    parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFolderRequest {
    name: Option<String>,
    #[serde(default)]
    parent_id: Option<Option<i64>>,
}

pub async fn list_folders(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FoldersQuery>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
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

pub async fn list_folders_tree(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
    };

    match db::list_all_folders_with_counts_for_user(&state.db, user_id).await {
        Ok(folders) => Json(folders).into_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to query folder tree: {}", error) }),
        ),
    }
}

pub async fn create_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateFolderRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
    };

    let folder_name = sanitize_folder_name(&payload.name);
    if folder_name.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "folder name is required" }),
        );
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

    match db::folder_name_exists_for_user_and_parent(
        &state.db,
        user_id,
        payload.parent_id,
        &folder_name,
    )
    .await
    {
        Ok(true) => return duplicate_folder_response(),
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
        Err(error) if is_unique_constraint_error(&error) => return duplicate_folder_response(),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to create folder: {}", error) }),
            );
        }
    };

    match db::get_folder_by_id_for_user(&state.db, folder_id, user_id).await {
        Ok(Some(folder)) => {
            dispatch_to_user(
                &state.websocket_hub,
                user_id,
                &FolderCreated {
                    folder_id,
                    parent_id: folder.parent_id,
                },
            )
            .await;
            (StatusCode::CREATED, Json(folder)).into_response()
        }
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

pub async fn update_folder(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateFolderRequest>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
    };

    let existing = match db::get_folder_by_id_for_user(&state.db, id, user_id).await {
        Ok(Some(folder)) => folder,
        Ok(None) => return folder_not_found_response(),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to load folder: {}", error) }),
            );
        }
    };

    let target_name = payload
        .name
        .map(|name| sanitize_folder_name(&name))
        .unwrap_or_else(|| existing.name.clone());
    if target_name.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "folder name is required" }),
        );
    }

    let target_parent_id = payload.parent_id.unwrap_or(existing.parent_id);
    if target_parent_id == Some(id) {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "folder cannot be its own parent" }),
        );
    }

    if let Some(parent_id) = target_parent_id {
        if let Err(response) = validate_parent_folder(&state, user_id, id, parent_id).await {
            return response;
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
        Ok(true) => return duplicate_folder_response(),
        Ok(false) => {}
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder name: {}", error) }),
            );
        }
    }

    match db::update_folder_by_id_for_user(&state.db, id, user_id, &target_name, target_parent_id)
        .await
    {
        Ok(true) => {}
        Ok(false) => return folder_not_found_response(),
        Err(error) if is_unique_constraint_error(&error) => return duplicate_folder_response(),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to update folder: {}", error) }),
            );
        }
    }

    match db::get_folder_by_id_for_user(&state.db, id, user_id).await {
        Ok(Some(folder)) => {
            dispatch_folder_changes(&state, user_id, &existing, &folder).await;
            Json(folder).into_response()
        }
        Ok(None) => folder_not_found_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load folder: {}", error) }),
        ),
    }
}

pub async fn delete_folder(
    Path(id): Path<i64>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
    };

    match db::folder_exists_for_user(&state.db, id, user_id).await {
        Ok(true) => {}
        Ok(false) => return folder_not_found_response(),
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder: {}", error) }),
            );
        }
    }

    let direct_subfolders = match db::count_direct_subfolders_for_user(&state.db, user_id, id).await
    {
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

    let active_jobs =
        match db::count_active_upload_jobs_for_user_in_folder(&state.db, user_id, id).await {
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
        Ok(true) => {
            dispatch_to_user(
                &state.websocket_hub,
                user_id,
                &FolderDeleted { folder_id: id },
            )
            .await;
            json_response(StatusCode::OK, json!({ "deleted": true, "id": id }))
        }
        Ok(false) => folder_not_found_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to delete folder: {}", error) }),
        ),
    }
}

pub async fn get_folder_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FolderPathQuery>,
) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return unauthorized_response(),
    };

    let Some(folder_id) = query.folder_id else {
        return Json(Vec::<FolderRecord>::new()).into_response();
    };

    match db::list_folder_path_for_user(&state.db, folder_id, user_id).await {
        Ok(path) if path.last().is_some_and(|folder| folder.id == folder_id) => {
            Json(path).into_response()
        }
        Ok(_) => folder_not_found_response(),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("failed to load folder path: {}", error) }),
        ),
    }
}

async fn validate_parent_folder(
    state: &AppState,
    user_id: i64,
    folder_id: i64,
    parent_id: i64,
) -> Result<(), Response> {
    match db::get_folder_by_id_for_user(&state.db, parent_id, user_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid parent folder" }),
            ));
        }
        Err(error) => {
            return Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate parent folder: {}", error) }),
            ));
        }
    }

    let path = db::list_folder_path_for_user(&state.db, parent_id, user_id)
        .await
        .map_err(|error| {
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("failed to validate folder move: {}", error) }),
            )
        })?;

    if path.iter().any(|folder| folder.id == folder_id) {
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "cannot move folder into itself or its descendants" }),
        ));
    }

    Ok(())
}

async fn dispatch_folder_changes(
    state: &AppState,
    user_id: i64,
    existing: &FolderRecord,
    updated: &FolderRecord,
) {
    if existing.name != updated.name {
        dispatch_to_user(
            &state.websocket_hub,
            user_id,
            &FolderRenamed {
                folder_id: updated.id,
                name: updated.name.clone(),
            },
        )
        .await;
    }

    if existing.parent_id != updated.parent_id {
        dispatch_to_user(
            &state.websocket_hub,
            user_id,
            &FolderMoved {
                folder_id: updated.id,
                parent_id: updated.parent_id,
            },
        )
        .await;
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
        .collect()
}

fn is_unique_constraint_error(error: &sqlx::Error) -> bool {
    error.to_string().contains("UNIQUE constraint failed")
}

fn unauthorized_response() -> Response {
    json_response(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }))
}

fn folder_not_found_response() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "error": "folder not found" }),
    )
}

fn duplicate_folder_response() -> Response {
    json_response(
        StatusCode::CONFLICT,
        json!({ "error": "folder with same name already exists in this location" }),
    )
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}
