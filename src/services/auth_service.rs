use anyhow::Result;
use sqlx::SqlitePool;

use crate::db;

#[derive(Clone)]
pub struct AuthService {
    db: SqlitePool,
}

impl AuthService {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn ensure_admin_user(&self, username: &str, password: &str) -> Result<()> {
        db::ensure_admin_user(&self.db, username, password).await
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Option<String>> {
        let user_id = db::verify_credentials(&self.db, username, password).await?;

        match user_id {
            Some(user_id) => {
                let session_id = db::create_session(&self.db, user_id).await?;
                Ok(Some(session_id))
            }
            None => Ok(None),
        }
    }

    pub async fn logout(&self, session_id: &str) -> Result<()> {
        db::delete_session(&self.db, session_id).await?;
        Ok(())
    }

    pub async fn user_id_from_session(&self, session_id: &str) -> Result<Option<i64>> {
        let user_id = db::user_id_from_session(&self.db, session_id).await?;
        Ok(user_id)
    }

    pub async fn username_by_id(&self, user_id: i64) -> Result<Option<String>> {
        let username = db::username_by_id(&self.db, user_id).await?;
        Ok(username)
    }
}
