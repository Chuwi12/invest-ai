//! Pool SQLite, migraciones embebidas y utilidades de fechas.

use std::{str::FromStr, time::Duration};

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

/// Abre (y crea si falta) la base de datos y aplica las migraciones.
pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("DATABASE_URL no válida: {database_url}"))?
        .create_if_missing(true)
        // WAL: lecturas concurrentes con una escritura; NORMAL es seguro con WAL y evita un fsync
        // por transacción.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("temp_store", "memory")
        .optimize_on_close(true, None);

    // sqlx no crea carpetas: `data/` no existe en una instalación limpia.
    if !database_url.contains(":memory:")
        && let Some(parent) = opts.get_filename().parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await
        .with_context(|| format!("no se pudo abrir la base de datos {database_url}"))?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Base en memoria compartida entre las conexiones del pool (para tests).
pub async fn connect_memory() -> anyhow::Result<SqlitePool> {
    // `shared_cache` hace que todas las conexiones vean la misma base; sin conexiones abiertas
    // la base se destruye, así que el pool no debe cerrar las inactivas.
    let opts = SqliteConnectOptions::new()
        .in_memory(true)
        .shared_cache(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(opts)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await.context("fallo al aplicar las migraciones")
}

/// Uuid v7 en texto: ordenable por fecha de creación.
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn dt_from_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}
