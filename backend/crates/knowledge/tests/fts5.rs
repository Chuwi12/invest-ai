//! ¿El SQLite que empaqueta `sqlx` (libsqlite3-sys con la característica `bundled`) incluye FTS5?
//! Solo comprueba la capacidad: la búsqueda léxica híbrida NO está implementada (ver docs/notes/rag.md).

use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

#[tokio::test]
async fn bundled_sqlite_has_fts5() {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let version: String = sqlx::query_scalar("SELECT sqlite_version()").fetch_one(&pool).await.unwrap();
    println!("sqlite_version = {version}");
    sqlx::query("CREATE VIRTUAL TABLE t USING fts5(body, tokenize = 'unicode61 remove_diacritics 2')").execute(&pool).await.expect("FTS5 no disponible");
    sqlx::query("INSERT INTO t(body) VALUES ('Item 1A. Risk Factors del folleto de AAPL'), ('otra cosa')").execute(&pool).await.unwrap();
    let hits: Vec<String> = sqlx::query_scalar("SELECT body FROM t WHERE t MATCH 'aapl' ORDER BY bm25(t)").fetch_all(&pool).await.unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    let opts: Vec<String> = sqlx::query_scalar("SELECT compile_options FROM pragma_compile_options WHERE compile_options LIKE '%FTS%'").fetch_all(&pool).await.unwrap();
    println!("compile_options FTS = {opts:?}");
}
