use axum::{routing::get, Router, Json, extract::Path};
use crate::file_compressor::FileCompressor;
use serde_json::json;
use base64::{engine::general_purpose, Engine as _};
use tower_http::cors::CorsLayer;


pub fn router() -> Router {
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/decompress/:file", get(decompress))
        .route("/test", get(test))
        .layer(cors)
}

async fn decompress(Path(file): Path<String>) -> Json<serde_json::Value> {
    let compressor = FileCompressor::default();
    println!("Received request to decompress file: {}", file);
    match compressor.decompress_file_to_bytes(&file) {
        Ok(bytes) => {
            let b64 = base64::encode(&bytes);
            println!("Decompressed file: {}, size: {} bytes", file, bytes.len());
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
