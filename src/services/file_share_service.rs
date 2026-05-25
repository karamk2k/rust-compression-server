use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db;
use crate::models::file_shares::{FileShareRecord, NewFileShareRecord};
use crate::repositories::file_shares_repository;

#[derive(Clone)]
pub struct FileShareService {
    db: SqlitePool,
}

impl FileShareService {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn get_share_by_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<FileShareRecord>, sqlx::Error> {
        file_shares_repository::find_by_uuid(&self.db, uuid).await
    }

    pub async fn create_share(
        &self,
        file_id: i64,
        user_id: i64,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<Option<FileShareRecord>, sqlx::Error> {
        let Some(file) = db::get_file_by_id(&self.db, file_id).await? else {
            return Ok(None);
        };
        if file.uploaded_by != user_id {
            return Ok(None);
        }

        let uuid = Uuid::new_v4().to_string();
        let expires_at = expires_at.unwrap_or_else(|| Utc::now() + Duration::days(1));

        file_shares_repository::insert(
            &self.db,
            &NewFileShareRecord {
                file_id,
                uuid: uuid.clone(),
                expires_at,
            },
        )
        .await?;

        file_shares_repository::find_by_uuid(&self.db, &uuid).await
    }
}
