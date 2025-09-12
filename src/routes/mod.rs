pub mod file_routes;

pub fn router() -> axum::Router {
    file_routes::router()
}