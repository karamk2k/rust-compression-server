
use axum::{routing::{get, post}, Router, Json, extract::Path};
use serde::{Serialize, Deserialize};
use std::net::SocketAddr;
use crate::file_compressor::FileCompressor;
use tokio::net::TcpListener;
use axum::serve;
use crate::routes;
#[derive(Debug, Serialize, Deserialize)]
struct CompressRequest {
    input: String,
    output: String,
}

pub struct Server {
    pub compressor: FileCompressor,
}

impl Server {
    pub fn new(compressor: FileCompressor) -> Self {
        Self { compressor }
    }

    pub async fn run(self) {
        let app = Router::new()
            .route("/", get(|| async { "Rust Compression Server 🚀" }))
            .merge(routes::router());

        let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
        println!("Server running at http://{}", addr);

        let listener = TcpListener::bind(addr).await.unwrap();
        serve(listener, app).await.unwrap();
    }

    // async fn compress(Json(payload): Json<CompressRequest>) -> Json<String> {
    //     let compressor = FileCompressor::default();
    //     match compressor.compress_file(&payload.input, &payload.output) {
    //         Ok(_) => Json(format!("Compressed {} -> {}", payload.input, payload.output)),
    //         Err(e) => Json(format!("Error: {}", e)),
    //     }
    // }


}
