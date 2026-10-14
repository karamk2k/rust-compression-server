use axum::{
    Router,
    routing::{get, post},
};

use crate::app_state::AppState;
use crate::controllers::admin_controller;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/login",
            get(admin_controller::get_login).post(admin_controller::post_login),
        )
        .route("/admin/files", get(admin_controller::get_files))
        .route("/admin/logs/view", get(admin_controller::view_logs))
        .route("/admin/logs/download", get(admin_controller::download_logs))
        .route("/admin/logs/clear", post(admin_controller::clear_logs))
        .route("/admin/logout", post(admin_controller::post_logout))
}
