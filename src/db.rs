use anyhow::Result;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::rngs::OsRng;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::file_model::{FileRecord, NewFileRecord};
use crate::models::folder_model::{FolderListItem, FolderRecord, NewFolderRecord};
use crate::models::upload_job_model::{NewUploadJobRecord, UploadJobRecord};
use crate::models::user_model::LoginUser;

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn ensure_admin_user(pool: &SqlitePool, username: &str, password: &str) -> Result<()> {
    let existing = sqlx::query_as::<_, (i64,)>("SELECT COUNT(1) FROM users WHERE username = ?1")
        .bind(username)
        .fetch_one(pool)
        .await?;

    if existing.0 == 0 {
        let password_hash = hash_password(password)?;
        sqlx::query("INSERT INTO users (username, password_hash) VALUES (?1, ?2)")
            .bind(username)
            .bind(password_hash)
            .execute(pool)
            .await?;
    }

    Ok(())
}

pub async fn verify_credentials(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let user = sqlx::query_as::<_, LoginUser>(
        "SELECT id, password_hash FROM users WHERE username = ?1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(user.and_then(|u| {
        if verify_password(&u.password_hash, password) {
            Some(u.id)
        } else {
            None
        }
    }))
}

pub async fn create_session(pool: &SqlitePool, user_id: i64) -> Result<String, sqlx::Error> {
    let session_id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at) VALUES (?1, ?2, datetime('now', '+1 day'))",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(session_id)
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn user_id_from_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    // Keep the sessions table clean and reject stale sessions.
    sqlx::query("DELETE FROM sessions WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?;

    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT user_id FROM sessions WHERE id = ?1 AND expires_at > datetime('now') LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|x| x.0))
}

pub async fn username_by_id(pool: &SqlitePool, user_id: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id = ?1 LIMIT 1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

pub async fn insert_file(pool: &SqlitePool, record: &NewFileRecord<'_>) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO files (original_name, folder_id, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(record.original_name)
    .bind(record.folder_id)
    .bind(record.stored_path)
    .bind(record.compressed_path)
    .bind(record.is_compressed)
    .bind(record.original_size)
    .bind(record.compressed_size)
    .bind(record.uploaded_by)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn list_files_for_user_and_folder(
    pool: &SqlitePool,
    user_id: i64,
    folder_id: Option<i64>,
) -> Result<Vec<FileRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileRecord>(
        "SELECT id, original_name, folder_id, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by, created_at \
         FROM files \
         WHERE uploaded_by = ?1 \
           AND ((?2 IS NULL AND folder_id IS NULL) OR folder_id = ?2) \
         ORDER BY id DESC LIMIT 200",
    )
    .bind(user_id)
    .bind(folder_id)
    .fetch_all(pool)
    .await
}

pub async fn list_files_for_user_and_folder_page(
    pool: &SqlitePool,
    user_id: i64,
    folder_id: Option<i64>,
    cursor: Option<i64>,
    limit: i64,
) -> Result<Vec<FileRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileRecord>(
        "SELECT id, original_name, folder_id, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by, created_at \
         FROM files \
         WHERE uploaded_by = ?1 \
           AND ((?2 IS NULL AND folder_id IS NULL) OR folder_id = ?2) \
           AND (?3 IS NULL OR id < ?3) \
         ORDER BY id DESC LIMIT ?4",
    )
    .bind(user_id)
    .bind(folder_id)
    .bind(cursor)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn summarize_files_for_user(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<(i64, i64, i64), sqlx::Error> {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT COUNT(1), COALESCE(SUM(original_size), 0), COALESCE(SUM(compressed_size), 0) \
         FROM files WHERE uploaded_by = ?1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn get_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<Option<FileRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileRecord>(
        "SELECT id, original_name, folder_id, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by, created_at \
         FROM files WHERE id = ?1 LIMIT 1",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_file_by_id_for_user(
    pool: &SqlitePool,
    file_id: i64,
    user_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM files WHERE id = ?1 AND uploaded_by = ?2")
        .bind(file_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn insert_folder(
    pool: &SqlitePool,
    record: &NewFolderRecord<'_>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO folders (name, parent_id, created_by) VALUES (?1, ?2, ?3)",
    )
    .bind(record.name)
    .bind(record.parent_id)
    .bind(record.created_by)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn get_folder_by_id_for_user(
    pool: &SqlitePool,
    folder_id: i64,
    user_id: i64,
) -> Result<Option<FolderRecord>, sqlx::Error> {
    sqlx::query_as::<_, FolderRecord>(
        "SELECT id, name, parent_id, created_by, created_at \
         FROM folders WHERE id = ?1 AND created_by = ?2 LIMIT 1",
    )
    .bind(folder_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

pub async fn list_folders_for_user_and_parent(
    pool: &SqlitePool,
    user_id: i64,
    parent_id: Option<i64>,
) -> Result<Vec<FolderListItem>, sqlx::Error> {
    sqlx::query_as::<_, FolderListItem>(
        "SELECT f.id, f.name, f.parent_id, f.created_by, f.created_at, \
            (SELECT COUNT(1) FROM files fl WHERE fl.uploaded_by = f.created_by AND fl.folder_id = f.id) AS file_count \
         FROM folders f \
         WHERE f.created_by = ?1 \
           AND ((?2 IS NULL AND f.parent_id IS NULL) OR f.parent_id = ?2) \
         ORDER BY f.name COLLATE NOCASE ASC",
    )
    .bind(user_id)
    .bind(parent_id)
    .fetch_all(pool)
    .await
}

pub async fn list_all_folders_with_counts_for_user(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<FolderListItem>, sqlx::Error> {
    sqlx::query_as::<_, FolderListItem>(
        "SELECT f.id, f.name, f.parent_id, f.created_by, f.created_at, \
            (SELECT COUNT(1) FROM files fl WHERE fl.uploaded_by = f.created_by AND fl.folder_id = f.id) AS file_count \
         FROM folders f \
         WHERE f.created_by = ?1 \
         ORDER BY f.name COLLATE NOCASE ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn folder_exists_for_user(
    pool: &SqlitePool,
    folder_id: i64,
    user_id: i64,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM folders WHERE id = ?1 AND created_by = ?2",
    )
    .bind(folder_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(exists > 0)
}

pub async fn folder_name_exists_for_user_and_parent(
    pool: &SqlitePool,
    user_id: i64,
    parent_id: Option<i64>,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM folders \
         WHERE created_by = ?1 \
           AND ((?2 IS NULL AND parent_id IS NULL) OR parent_id = ?2) \
           AND lower(name) = lower(?3)",
    )
    .bind(user_id)
    .bind(parent_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(exists > 0)
}

pub async fn folder_name_exists_for_user_and_parent_except(
    pool: &SqlitePool,
    user_id: i64,
    parent_id: Option<i64>,
    name: &str,
    exclude_folder_id: i64,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM folders \
         WHERE created_by = ?1 \
           AND ((?2 IS NULL AND parent_id IS NULL) OR parent_id = ?2) \
           AND lower(name) = lower(?3) \
           AND id != ?4",
    )
    .bind(user_id)
    .bind(parent_id)
    .bind(name)
    .bind(exclude_folder_id)
    .fetch_one(pool)
    .await?;
    Ok(exists > 0)
}

pub async fn update_folder_by_id_for_user(
    pool: &SqlitePool,
    folder_id: i64,
    user_id: i64,
    name: &str,
    parent_id: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE folders \
         SET name = ?1, parent_id = ?2 \
         WHERE id = ?3 AND created_by = ?4",
    )
    .bind(name)
    .bind(parent_id)
    .bind(folder_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_folder_by_id_for_user(
    pool: &SqlitePool,
    folder_id: i64,
    user_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM folders WHERE id = ?1 AND created_by = ?2")
        .bind(folder_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn count_direct_subfolders_for_user(
    pool: &SqlitePool,
    user_id: i64,
    parent_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM folders WHERE created_by = ?1 AND parent_id = ?2",
    )
    .bind(user_id)
    .bind(parent_id)
    .fetch_one(pool)
    .await
}

pub async fn count_files_for_user_in_folder(
    pool: &SqlitePool,
    user_id: i64,
    folder_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM files WHERE uploaded_by = ?1 AND folder_id = ?2",
    )
    .bind(user_id)
    .bind(folder_id)
    .fetch_one(pool)
    .await
}

pub async fn count_active_upload_jobs_for_user_in_folder(
    pool: &SqlitePool,
    user_id: i64,
    folder_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(1) FROM upload_jobs \
         WHERE uploaded_by = ?1 AND folder_id = ?2 AND status IN ('pending', 'processing')",
    )
    .bind(user_id)
    .bind(folder_id)
    .fetch_one(pool)
    .await
}

pub async fn move_file_to_folder_for_user(
    pool: &SqlitePool,
    file_id: i64,
    user_id: i64,
    folder_id: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE files SET folder_id = ?1 WHERE id = ?2 AND uploaded_by = ?3",
    )
    .bind(folder_id)
    .bind(file_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn clear_upload_job_file_refs(
    pool: &SqlitePool,
    file_id: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE upload_jobs \
         SET file_id = NULL, updated_at = datetime('now') \
         WHERE file_id = ?1",
    )
    .bind(file_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn list_folder_path_for_user(
    pool: &SqlitePool,
    folder_id: i64,
    user_id: i64,
) -> Result<Vec<FolderRecord>, sqlx::Error> {
    let mut path = Vec::new();
    let mut cursor = Some(folder_id);
    let mut guard = 0usize;

    while let Some(current_id) = cursor {
        if guard > 64 {
            break;
        }
        let row = get_folder_by_id_for_user(pool, current_id, user_id).await?;
        let Some(folder) = row else {
            break;
        };
        cursor = folder.parent_id;
        path.push(folder);
        guard += 1;
    }

    path.reverse();
    Ok(path)
}

pub async fn insert_upload_job(
    pool: &SqlitePool,
    record: &NewUploadJobRecord<'_>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO upload_jobs (original_name, temp_path, uploaded_by, folder_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'pending', datetime('now'))",
    )
    .bind(record.original_name)
    .bind(record.temp_path)
    .bind(record.uploaded_by)
    .bind(record.folder_id)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn get_upload_job_by_id_for_user(
    pool: &SqlitePool,
    job_id: i64,
    user_id: i64,
) -> Result<Option<UploadJobRecord>, sqlx::Error> {
    sqlx::query_as::<_, UploadJobRecord>(
        "SELECT id, original_name, temp_path, uploaded_by, folder_id, status, file_id, error, created_at, started_at, finished_at, updated_at \
         FROM upload_jobs WHERE id = ?1 AND uploaded_by = ?2 LIMIT 1",
    )
    .bind(job_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

pub async fn reset_processing_upload_jobs(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE upload_jobs \
         SET status = 'pending', error = NULL, started_at = NULL, updated_at = datetime('now') \
         WHERE status = 'processing'",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn claim_next_pending_upload_job(
    pool: &SqlitePool,
) -> Result<Option<UploadJobRecord>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let next_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM upload_jobs WHERE status = 'pending' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(job_id) = next_id else {
        tx.commit().await?;
        return Ok(None);
    };

    let updated = sqlx::query(
        "UPDATE upload_jobs \
         SET status = 'processing', started_at = COALESCE(started_at, datetime('now')), error = NULL, updated_at = datetime('now') \
         WHERE id = ?1 AND status = 'pending'",
    )
    .bind(job_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if updated == 0 {
        tx.commit().await?;
        return Ok(None);
    }

    let job = sqlx::query_as::<_, UploadJobRecord>(
        "SELECT id, original_name, temp_path, uploaded_by, folder_id, status, file_id, error, created_at, started_at, finished_at, updated_at \
         FROM upload_jobs WHERE id = ?1 LIMIT 1",
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

pub async fn mark_upload_job_done(
    pool: &SqlitePool,
    job_id: i64,
    file_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE upload_jobs \
         SET status = 'done', file_id = ?2, error = NULL, finished_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ?1",
    )
    .bind(job_id)
    .bind(file_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_upload_job_failed(
    pool: &SqlitePool,
    job_id: i64,
    error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE upload_jobs \
         SET status = 'failed', error = ?2, finished_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ?1",
    )
    .bind(job_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(hash.to_string())
}

fn verify_password(password_hash: &str, password: &str) -> bool {
    let parsed = match PasswordHash::new(password_hash) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
