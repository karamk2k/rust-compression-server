use sqlx::SqlitePool;

use crate::models::file_shares::{FileShareRecord, NewFileShareRecord};

pub async fn find_by_uuid(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<FileShareRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileShareRecord>(
        "SELECT id, uuid, file_id, expires_at, created_at
         FROM file_shares
         WHERE uuid = ?1
         LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await
}

pub async fn insert(
    pool: &SqlitePool,
    record: &NewFileShareRecord,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO file_shares (file_id, uuid, expires_at)
         VALUES (?1, ?2, ?3)",
    )
    .bind(record.file_id)
    .bind(&record.uuid)
    .bind(record.expires_at)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}
