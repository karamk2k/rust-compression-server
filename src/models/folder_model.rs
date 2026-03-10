use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FolderRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FolderListItem {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
    pub file_count: i64,
}

pub struct NewFolderRecord<'a> {
    pub name: &'a str,
    pub parent_id: Option<i64>,
    pub created_by: i64,
}
