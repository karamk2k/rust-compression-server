use axum::{routing::get, Router, Json, extract::Path};
use crate::file_compressor::FileCompressor;
use serde_json::json;
use base64::{engine::general_purpose, Engine as _};

pub fn router() -> Router {
    Router::new()
        .route("/decompress/:file", get(decompress))
        .route("/test", get(test))
}

async fn decompress(Path(file): Path<String>) -> Json<serde_json::Value> {
    let compressor = FileCompressor::default();

    match compressor.decompress_file_to_bytes(&file) {
        Ok(bytes) => {
            let b64 = base64::encode(&bytes); // keep it safe for JSON
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
