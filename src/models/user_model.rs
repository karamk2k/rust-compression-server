use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct LoginUser {
    pub id: i64,
    pub password_hash: String,
}
