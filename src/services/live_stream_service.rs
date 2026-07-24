use anyhow::{Result, anyhow};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::live_stream_model::{LiveStream, ViewerKey};

#[derive(Clone)]
pub struct LiveStreamService {
    db: SqlitePool,
}

impl LiveStreamService {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn create(&self, owner_id: i64, title: &str) -> Result<(LiveStream, String)> {
        let title = title.trim();
        if title.is_empty() || title.len() > 120 {
            return Err(anyhow!("title must contain 1 to 120 characters"));
        }
        let public_id = Uuid::new_v4().simple().to_string();
        let publish_key = random_token();
        let result = sqlx::query(
            "INSERT INTO live_streams (public_id, owner_id, title, publish_key_hash) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&public_id)
        .bind(owner_id)
        .bind(title)
        .bind(token_hash(&publish_key))
        .execute(&self.db)
        .await?;
        let stream = self
            .get_owned(result.last_insert_rowid(), owner_id)
            .await?
            .ok_or_else(|| anyhow!("created stream could not be loaded"))?;
        Ok((stream, publish_key))
    }

    pub async fn list_owned(&self, owner_id: i64) -> Result<Vec<LiveStream>> {
        Ok(sqlx::query_as(
            "SELECT id, public_id, owner_id, title, enabled, is_live, created_at, updated_at \
             FROM live_streams WHERE owner_id = ?1 ORDER BY id DESC",
        )
        .bind(owner_id)
        .fetch_all(&self.db)
        .await?)
    }

    pub async fn get_owned(&self, id: i64, owner_id: i64) -> Result<Option<LiveStream>> {
        Ok(sqlx::query_as(
            "SELECT id, public_id, owner_id, title, enabled, is_live, created_at, updated_at \
             FROM live_streams WHERE id = ?1 AND owner_id = ?2 LIMIT 1",
        )
        .bind(id)
        .bind(owner_id)
        .fetch_optional(&self.db)
        .await?)
    }

    pub async fn rotate_publish_key(&self, id: i64, owner_id: i64) -> Result<Option<String>> {
        let token = random_token();
        let result = sqlx::query(
            "UPDATE live_streams SET publish_key_hash = ?1, updated_at = CURRENT_TIMESTAMP \
             WHERE id = ?2 AND owner_id = ?3",
        )
        .bind(token_hash(&token))
        .bind(id)
        .bind(owner_id)
        .execute(&self.db)
        .await?;
        Ok((result.rows_affected() == 1).then_some(token))
    }

    pub async fn set_enabled(&self, id: i64, owner_id: i64, enabled: bool) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE live_streams SET enabled = ?1, updated_at = CURRENT_TIMESTAMP \
             WHERE id = ?2 AND owner_id = ?3",
        )
        .bind(enabled)
        .bind(id)
        .bind(owner_id)
        .execute(&self.db)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn authorize_publish(&self, public_id: &str, key: &str) -> Result<bool> {
        let expected = sqlx::query_scalar::<_, String>(
            "SELECT publish_key_hash FROM live_streams WHERE public_id = ?1 AND enabled = 1 LIMIT 1",
        )
        .bind(public_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(expected.is_some_and(|hash| constant_time_eq(&hash, &token_hash(key))))
    }

    pub async fn mark_live(&self, public_id: &str, is_live: bool) -> Result<()> {
        sqlx::query(
            "UPDATE live_streams SET is_live = ?1, updated_at = CURRENT_TIMESTAMP WHERE public_id = ?2",
        )
        .bind(is_live)
        .bind(public_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn create_viewer_key(
        &self,
        stream_id: i64,
        owner_id: i64,
        label: &str,
        expires_hours: Option<i64>,
    ) -> Result<Option<(ViewerKey, String)>> {
        if self.get_owned(stream_id, owner_id).await?.is_none() {
            return Ok(None);
        }
        let label = label.trim();
        if label.is_empty() || label.len() > 80 {
            return Err(anyhow!("label must contain 1 to 80 characters"));
        }
        let hours = expires_hours.unwrap_or(168).clamp(1, 24 * 365);
        let token = random_token();
        let result = sqlx::query(
            "INSERT INTO live_viewer_keys (stream_id, label, token_hash, expires_at) \
             VALUES (?1, ?2, ?3, datetime('now', '+' || ?4 || ' hours'))",
        )
        .bind(stream_id)
        .bind(label)
        .bind(token_hash(&token))
        .bind(hours)
        .execute(&self.db)
        .await?;
        let key = sqlx::query_as(
            "SELECT id, stream_id, label, expires_at, revoked_at, created_at \
             FROM live_viewer_keys WHERE id = ?1",
        )
        .bind(result.last_insert_rowid())
        .fetch_one(&self.db)
        .await?;
        Ok(Some((key, token)))
    }

    pub async fn list_viewer_keys(
        &self,
        stream_id: i64,
        owner_id: i64,
    ) -> Result<Option<Vec<ViewerKey>>> {
        if self.get_owned(stream_id, owner_id).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(
            sqlx::query_as(
                "SELECT id, stream_id, label, expires_at, revoked_at, created_at \
             FROM live_viewer_keys WHERE stream_id = ?1 ORDER BY id DESC",
            )
            .bind(stream_id)
            .fetch_all(&self.db)
            .await?,
        ))
    }

    pub async fn revoke_viewer_key(&self, key_id: i64, owner_id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE live_viewer_keys SET revoked_at = CURRENT_TIMESTAMP \
             WHERE id = ?1 AND stream_id IN (SELECT id FROM live_streams WHERE owner_id = ?2)",
        )
        .bind(key_id)
        .bind(owner_id)
        .execute(&self.db)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn authorize_viewer(&self, public_id: &str, token: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(1) FROM live_viewer_keys vk \
             JOIN live_streams s ON s.id = vk.stream_id \
             WHERE s.public_id = ?1 AND s.enabled = 1 AND vk.token_hash = ?2 \
               AND vk.revoked_at IS NULL AND (vk.expires_at IS NULL OR vk.expires_at > datetime('now'))",
        )
        .bind(public_id)
        .bind(token_hash(token))
        .fetch_one(&self.db)
        .await?;
        Ok(count == 1)
    }

    pub async fn public_stream(&self, public_id: &str) -> Result<Option<LiveStream>> {
        Ok(sqlx::query_as(
            "SELECT id, public_id, owner_id, title, enabled, is_live, created_at, updated_at \
             FROM live_streams WHERE public_id = ?1 AND enabled = 1 LIMIT 1",
        )
        .bind(public_id)
        .fetch_optional(&self.db)
        .await?)
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
