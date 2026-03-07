use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FileRecord {
    pub id: i64,
    pub original_name: String,
    pub stored_path: String,
    pub compressed_path: String,
    pub is_compressed: bool,
    pub original_size: i64,
    pub compressed_size: i64,
    pub uploaded_by: i64,
    pub created_at: String,
}

pub struct NewFileRecord<'a> {
    pub original_name: &'a str,
    pub stored_path: &'a str,
    pub compressed_path: &'a str,
    pub is_compressed: bool,
    pub original_size: i64,
    pub compressed_size: i64,
    pub uploaded_by: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadResult {
    pub id: i64,
    pub original_name: String,
    pub stored_path: String,
    pub compressed_path: String,
    pub is_compressed: bool,
    pub original_size: i64,
    pub compressed_size: i64,
}
