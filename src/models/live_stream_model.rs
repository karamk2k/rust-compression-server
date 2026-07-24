use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct LiveStream {
    pub id: i64,
    pub public_id: String,
    pub owner_id: i64,
    pub title: String,
    pub enabled: bool,
    pub is_live: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ViewerKey {
    pub id: i64,
    pub stream_id: i64,
    pub label: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}
