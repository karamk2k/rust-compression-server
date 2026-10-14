use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::error;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::auth::authenticated_user_id;
use crate::db;
use crate::events::{dispatch_to_user, upload::UploadQueued};
use crate::models::upload_job_model::{NewUploadJobRecord, UploadJobStatus};

pub async fn upload_file(
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
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "missing file field" }),
        );
    };

    if let Some(folder_id) = selected_folder_id {
        match db::folder_exists_for_user(&state.db, folder_id, user_id).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "invalid folder_id" }),
                );
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

    dispatch_to_user(
        &state.websocket_hub,
        user_id,
        &UploadQueued {
            job_id,
            original_name: &original_name,
            folder_id: selected_folder_id,
        },
    )
    .await;

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

pub async fn get_upload_job(
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

fn parse_optional_i64(raw: &str) -> Result<Option<i64>, &'static str> {
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }

    match raw.parse::<i64>() {
        Ok(value) if value > 0 => Ok(Some(value)),
        _ => Err("folder_id must be a positive integer"),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}
