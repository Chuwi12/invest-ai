//! Ayudas para tests (BD con el esquema real). Pública para los tests de integración.
#![doc(hidden)]

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::store::SCHEMA_SQL;

/// Directorio temporal que se borra al soltarse.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Pool de pruebas abierto como el de producción (`server/src/db.rs`): fichero SQLite en WAL, `busy_timeout` de 5 s y
/// varias conexiones, sin esquema. NO sirve `sqlite::memory:`: su caché compartida da `SQLITE_LOCKED_SHAREDCACHE`
/// entre conexiones concurrentes (en `prepare` no se espera) y FTS5 lo sufre («vtable constructor failed»).
/// El fichero se borra cuando se suelta el último clon del pool (el directorio vive en una de sus opciones).
pub async fn temp_wal_pool() -> sqlx::Result<SqlitePool> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = TempDir(std::env::temp_dir().join(format!("invest-ai-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))));
    // `create` (no `create_dir_all`) y 0700: falla si alguien la dejó preparada en el /tmp compartido.
    std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).create(&dir.0)?;
    let opts = SqliteConnectOptions::new()
        .filename(dir.0.join("test.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(4)
        .after_connect(move |_, _| {
            let _owner = &dir;
            Box::pin(async { Ok(()) })
        })
        .connect_with(opts)
        .await
}

/// Base temporal (ver [`temp_wal_pool`]) con el esquema de la migración 0004. El nombre es histórico: ya no es en memoria.
pub async fn memory_pool() -> SqlitePool {
    let pool = temp_wal_pool().await.unwrap();
    sqlx::raw_sql(SCHEMA_SQL).execute(&pool).await.unwrap();
    pool
}

/// Base en un fichero (para probar la persistencia entre aperturas).
pub async fn file_pool(path: &std::path::Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true).journal_mode(SqliteJournalMode::Wal).foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(2).connect_with(opts).await.unwrap();
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'documents'").fetch_one(&pool).await.unwrap();
    if exists == 0 {
        sqlx::raw_sql(SCHEMA_SQL).execute(&pool).await.unwrap();
    }
    pool
}
