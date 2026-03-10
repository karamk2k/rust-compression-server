use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UploadJobRecord {
    pub id: i64,
    pub original_name: String,
    pub temp_path: String,
    pub uploaded_by: i64,
    pub folder_id: Option<i64>,
    pub status: String,
    pub file_id: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub updated_at: String,
}

pub struct NewUploadJobRecord<'a> {
    pub original_name: &'a str,
    pub temp_path: &'a str,
    pub uploaded_by: i64,
    pub folder_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadJobStatus {
    pub id: i64,
    pub original_name: String,
    pub status: String,
    pub file_id: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub updated_at: String,
}

impl From<UploadJobRecord> for UploadJobStatus {
    fn from(value: UploadJobRecord) -> Self {
        Self {
            id: value.id,
            original_name: value.original_name,
            status: value.status,
            file_id: value.file_id,
            error: value.error,
            created_at: value.created_at,
            started_at: value.started_at,
            finished_at: value.finished_at,
            updated_at: value.updated_at,
        }
    }
}
