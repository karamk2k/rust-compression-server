use anyhow::Result;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::rngs::OsRng;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::file_model::{FileRecord, NewFileRecord};
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
        "INSERT INTO files (original_name, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(record.original_name)
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

pub async fn list_files(pool: &SqlitePool) -> Result<Vec<FileRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileRecord>(
        "SELECT id, original_name, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by, created_at \
         FROM files ORDER BY id DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<Option<FileRecord>, sqlx::Error> {
    sqlx::query_as::<_, FileRecord>(
        "SELECT id, original_name, stored_path, compressed_path, is_compressed, original_size, compressed_size, uploaded_by, created_at \
         FROM files WHERE id = ?1 LIMIT 1",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM files WHERE id = ?1")
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
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
