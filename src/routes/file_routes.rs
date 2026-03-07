use axum::{routing::get, Router, Json, extract::Path};
use crate::file_compressor::FileCompressor;
use crate::app_state::AppState;
use serde_json::json;
use base64::{engine::general_purpose, Engine as _};
use tower_http::cors::CorsLayer;
use tracing::info;


pub fn router() -> Router<AppState> {
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/decompress/:file", get(decompress))
        .route("/test", get(test))
        .layer(cors)
}

async fn decompress(Path(file): Path<String>) -> Json<serde_json::Value> {
    let compressor = FileCompressor::default();
    info!(file = %file, "received decompress request");
    match compressor.decompress_file_to_bytes(&file) {
        Ok(bytes) => {
            let b64 = general_purpose::STANDARD.encode(&bytes);
            info!(file = %file, size = bytes.len(), "decompressed file");
            Json(json!({
                "file": file,
                "data": b64,
                "encoding": "base64"
            }))
        }
        Err(e) => Json(json!({
            "file": file,
            "error": e.to_string()
        })),
    }
}

async fn test() -> Json<&'static str> {
    Json("Hello from test 🚀")
}
