use serde::Serialize;
use sqlx::FromRow;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FileShareRecord {
pub id: i64,
pub file_id: i64,
pub uuid: String,
pub expires_at: DateTime<Utc>,
pub created_at: DateTime<Utc>,
}


pub struct NewFileShareRecord {
pub file_id: i64,
pub uuid: String,
pub expires_at: DateTime<Utc>,
}

pub struct UpdateFileShareRecord {
pub expires_at: DateTime<Utc>,
}
