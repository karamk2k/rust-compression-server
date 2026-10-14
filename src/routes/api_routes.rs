use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post},
};

use crate::app_state::AppState;
use crate::controllers::{auth_controller, file_controller, shares_controller, upload_controller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(auth_controller::get_me))
        .route("/api/upload", post(upload_controller::upload_file))
        .route("/api/jobs/:id", get(upload_controller::get_upload_job))
        .route("/api/files", get(file_controller::list_files))
        .route("/api/files/summary", get(file_controller::get_file_summary))
        .route(
            "/api/files/move-batch",
            patch(file_controller::move_files_batch),
        )
        .route(
            "/api/files/delete-batch",
            post(file_controller::delete_files_batch),
        )
        .route(
            "/api/files/:id/move",
            patch(file_controller::move_file_to_folder),
        )
        .route(
            "/api/files/:id/share",
            post(shares_controller::create_file_share),
        )
        .route("/api/files/:id/thumb", get(file_controller::file_thumbnail))
        .route("/api/files/:id/view", get(file_controller::view_file))
        .route(
            "/api/files/:id/download",
            get(file_controller::download_file),
        )
        .route("/api/files/:id", delete(file_controller::delete_file))
        .route("/api/shares/:uuid", get(shares_controller::get_file_share))
        .route("/share/:uuid", get(shares_controller::view_shared_file))
        // Allow large multipart uploads (up to 10 GB).
        .layer(DefaultBodyLimit::max(10usize * 1024 * 1024 * 1024))
}
