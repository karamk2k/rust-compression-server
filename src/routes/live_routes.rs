use axum::{
    Router,
    routing::{get, patch, post},
};

use crate::app_state::AppState;
use crate::controllers::live_controller;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/streams", get(live_controller::streams_page))
        .route(
            "/api/live/streams",
            get(live_controller::list_streams).post(live_controller::create_stream),
        )
        .route(
            "/api/live/streams/:id/publish-key",
            post(live_controller::rotate_publish_key),
        )
        .route(
            "/api/live/streams/:id/enabled",
            patch(live_controller::set_stream_enabled),
        )
        .route(
            "/api/live/streams/:id/viewer-keys",
            get(live_controller::list_viewer_keys).post(live_controller::create_viewer_key),
        )
        .route(
            "/api/live/viewer-keys/:id/revoke",
            post(live_controller::revoke_viewer_key),
        )
        .route(
            "/internal/rtmp/publish",
            post(live_controller::authorize_publish),
        )
        .route(
            "/internal/rtmp/publish-done",
            post(live_controller::publish_done),
        )
        .route(
            "/internal/live/authorize",
            get(live_controller::authorize_hls),
        )
        .route("/watch/:public_id", get(live_controller::watch_stream))
        .route("/hls/:file_name", get(live_controller::serve_local_hls))
}
