use axum::{
    Router,
    routing::{get, patch},
};

use crate::app_state::AppState;
use crate::controllers::folder_controller;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/folders",
            get(folder_controller::list_folders).post(folder_controller::create_folder),
        )
        .route(
            "/api/folders/tree",
            get(folder_controller::list_folders_tree),
        )
        .route(
            "/api/folders/:id",
            patch(folder_controller::update_folder).delete(folder_controller::delete_folder),
        )
        .route("/api/folders/path", get(folder_controller::get_folder_path))
}
