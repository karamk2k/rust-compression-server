use axum::Router;

use crate::app_state::AppState;

pub mod admin_routes;
pub mod api_routes;
pub mod file_routes;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(file_routes::router())
        .merge(api_routes::router())
        .merge(admin_routes::router())
}
