//! Ajustes clave-valor (tabla `settings`). Implementa `domain::SettingsStore` para que el crate
//! `ai` guarde el modelo activo sin conocer SQLite.

use async_trait::async_trait;
use domain::{DomainError, SettingsStore};
use sqlx::SqlitePool;

use crate::db::now_ms;

#[derive(Clone)]
pub struct SqliteSettings {
    db: SqlitePool,
}

impl SqliteSettings {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

fn internal(e: sqlx::Error) -> DomainError {
    DomainError::Internal(format!("ajustes: {e}"))
}

#[async_trait]
impl SettingsStore for SqliteSettings {
    async fn get(&self, key: &str) -> domain::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.db)
            .await
            .map_err(internal)?;
        Ok(row.map(|(v,)| v))
    }

    async fn set(&self, key: &str, value: &str) -> domain::Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(now_ms())
        .execute(&self.db)
        .await
        .map_err(internal)?;
        Ok(())
    }
}
