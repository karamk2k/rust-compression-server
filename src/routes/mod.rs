use axum::Router;

use crate::app_state::AppState;

pub mod admin_routes;
pub mod api_routes;
pub mod channels_routes;
pub mod file_routes;
pub mod folder_routes;
pub mod live_routes;
pub mod ws_routes;
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(file_routes::router())
        .merge(folder_routes::router())
        .merge(api_routes::router())
        .merge(admin_routes::router())
        .merge(live_routes::router())
        .merge(ws_routes::router())
}
