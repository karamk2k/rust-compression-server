use axum::serve;
use axum::{routing::get, Router};
use crate::app_state::AppState;
use crate::routes;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;
use tracing::info;

pub struct Server {
    bind_target: String,
    state: AppState,
}

impl Server {
    pub fn new(host: String, port: u16, state: AppState) -> Self {
        Self {
            bind_target: format!("{}:{}", host, port),
            state,
        }
    }

    pub async fn run(self) {
        let app = Router::new()
            .route("/", get(|| async { "Rust Compression Server 🚀" }))
            .nest_service("/assets", ServeDir::new("web/assets"))
            .merge(routes::router())
            .with_state(self.state);

        let listener = TcpListener::bind(&self.bind_target).await.unwrap();
        let addr = listener.local_addr().unwrap();
        info!("server running at http://{}", addr);
        serve(listener, app).await.unwrap();
    }
}
