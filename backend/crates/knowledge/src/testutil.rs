//! Ayudas para tests (BD en memoria con el esquema real). Pública para los tests de integración.
#![doc(hidden)]

use std::str::FromStr;

use sqlx::{SqlitePool, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};

use crate::store::SCHEMA_SQL;

/// Base en memoria compartida entre las conexiones del pool, con el esquema de la migración 0004.
pub async fn memory_pool() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap().foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(4).min_connections(1).idle_timeout(None).max_lifetime(None).connect_with(opts).await.unwrap();
    sqlx::raw_sql(SCHEMA_SQL).execute(&pool).await.unwrap();
    pool
}

/// Base en un fichero (para probar la persistencia entre aperturas).
pub async fn file_pool(path: &std::path::Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(2).connect_with(opts).await.unwrap();
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'documents'").fetch_one(&pool).await.unwrap();
    if exists == 0 {
        sqlx::raw_sql(SCHEMA_SQL).execute(&pool).await.unwrap();
    }
    pool
}
