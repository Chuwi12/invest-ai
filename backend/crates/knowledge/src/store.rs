//! Persistencia en SQLite (consultas dinámicas de sqlx; sin macros que exijan DATABASE_URL).
//! No depende de `PRAGMA foreign_keys`: los borrados en cascada se hacen explícitamente.

use std::collections::HashMap;

use chrono::DateTime;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{
    embed::{from_blob, to_blob},
    error::{KnowledgeError, Result},
    model::{DocKind, DocumentInfo},
    text::parse_hash128,
};

/// Esquema (misma fuente que la migración del servidor: no hay dos copias).
pub const SCHEMA_SQL: &str = include_str!("../../server/migrations/0004_knowledge.sql");

pub async fn check_schema(pool: &SqlitePool) -> Result<()> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('documents','document_assets','chunks','embeddings')",
    )
    .fetch_one(pool)
    .await?;
    if n == 4 { Ok(()) } else { Err(KnowledgeError::SchemaMissing) }
}

pub struct ChunkRow {
    pub ordinal: i64,
    pub section: Option<String>,
    pub text: String,
    pub text_hash: String,
    pub tokens: i64,
}

pub struct InsertDoc<'a> {
    pub doc_key: &'a str,
    pub content_hash: &'a str,
    pub kind: DocKind,
    pub subtype: Option<&'a str>,
    pub source: &'a str,
    pub publisher: Option<&'a str>,
    pub title: &'a str,
    pub url: Option<&'a str>,
    pub issuer: Option<&'a str>,
    pub language: Option<&'a str>,
    pub published_ms: i64,
    pub ingested_ms: i64,
    pub chunk_count: i64,
    pub meta: &'a str,
}

pub async fn insert_document(tx: &mut Transaction<'_, Sqlite>, d: &InsertDoc<'_>) -> Result<i64> {
    let r = sqlx::query(
        "INSERT INTO documents (doc_key, content_hash, kind, subtype, source, publisher, title, url, issuer, language,
                                published_at, ingested_at, chunk_count, meta)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(d.doc_key)
    .bind(d.content_hash)
    .bind(d.kind.as_str())
    .bind(d.subtype)
    .bind(d.source)
    .bind(d.publisher)
    .bind(d.title)
    .bind(d.url)
    .bind(d.issuer)
    .bind(d.language)
    .bind(d.published_ms)
    .bind(d.ingested_ms)
    .bind(d.chunk_count)
    .bind(d.meta)
    .execute(&mut **tx)
    .await?;
    Ok(r.last_insert_rowid())
}

pub async fn insert_chunk(tx: &mut Transaction<'_, Sqlite>, doc_id: i64, c: &ChunkRow) -> Result<i64> {
    let r = sqlx::query("INSERT INTO chunks (document_id, ordinal, section, text, text_hash, token_count) VALUES (?,?,?,?,?,?)")
        .bind(doc_id)
        .bind(c.ordinal)
        .bind(&c.section)
        .bind(&c.text)
        .bind(&c.text_hash)
        .bind(c.tokens)
        .execute(&mut **tx)
        .await?;
    Ok(r.last_insert_rowid())
}

pub async fn insert_embedding(tx: &mut Transaction<'_, Sqlite>, chunk_id: i64, model: &str, vec: &[f32], now_ms: i64) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO embeddings (chunk_id, model, dim, vec, created_at) VALUES (?,?,?,?,?)")
        .bind(chunk_id)
        .bind(model)
        .bind(vec.len() as i64)
        .bind(to_blob(vec))
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn add_assets(tx: &mut Transaction<'_, Sqlite>, doc_id: i64, assets: &[String]) -> Result<()> {
    for a in assets {
        sqlx::query("INSERT OR IGNORE INTO document_assets (document_id, asset_id) VALUES (?,?)").bind(doc_id).bind(a).execute(&mut **tx).await?;
    }
    Ok(())
}

pub async fn assets_of(pool: &SqlitePool, doc_id: i64) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT asset_id FROM document_assets WHERE document_id = ? ORDER BY asset_id").bind(doc_id).fetch_all(pool).await?)
}

pub async fn delete_documents(tx: &mut Transaction<'_, Sqlite>, ids: &[i64], fts: bool) -> Result<usize> {
    let mut chunks = 0usize;
    for id in ids {
        if fts {
            fts_delete_doc(tx, *id).await?;
        }
        sqlx::query("DELETE FROM embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id = ?)").bind(id).execute(&mut **tx).await?;
        chunks += sqlx::query("DELETE FROM chunks WHERE document_id = ?").bind(id).execute(&mut **tx).await?.rows_affected() as usize;
        sqlx::query("DELETE FROM document_assets WHERE document_id = ?").bind(id).execute(&mut **tx).await?;
        sqlx::query("DELETE FROM documents WHERE id = ?").bind(id).execute(&mut **tx).await?;
    }
    Ok(chunks)
}

// ----------------------------------------------------------------------------- FTS5 (perezoso)
//
// La tabla FTS5 NO está en ninguna migración: se crea al abrir el archivo y, si falla, el archivo
// sigue funcionando solo con vectores. Se mantiene con borrados e inserciones EXPLÍCITOS en las
// mismas transacciones (no con triggers): un trigger sobre `chunks` quedaría en la BD aunque FTS5
// dejara de estar disponible y haría fallar cada alta o baja de fragmentos.

/// Tabla sin contenido propio (el texto vive en `chunks`; no se duplica): `rowid` = `chunks.id`.
/// `unicode61 remove_diacritics 2`: parte por cualquier no alfanumérico y quita tildes, válido
/// para español e inglés; no se usa `porter` (solo inglés).
const FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text, title, section, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2')";
/// Pesos de bm25 por columna: texto 1,0, título 1,5, sección 1,5 (el nombre de la empresa o «Risk Factors»
/// pesan algo más que una mención suelta). Heurística razonada, sin calibrar.
const FTS_WEIGHTS: &str = "1.0, 1.5, 1.5";

/// Crea (si falta) y sondea la tabla; borra huérfanos de una sesión anterior en la que FTS5 falló a mitad.
pub async fn fts_init(pool: &SqlitePool) -> Result<()> {
    sqlx::query(FTS_DDL).execute(pool).await?;
    sqlx::query("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH '\"sonda\"' LIMIT 1").fetch_optional(pool).await?;
    sqlx::query("DELETE FROM chunks_fts WHERE rowid NOT IN (SELECT id FROM chunks)").execute(pool).await?;
    Ok(())
}

pub async fn fts_count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM chunks_fts").fetch_one(pool).await?)
}

pub async fn fts_insert(tx: &mut Transaction<'_, Sqlite>, chunk_id: i64, text: &str, title: &str, section: Option<&str>) -> Result<()> {
    sqlx::query("INSERT INTO chunks_fts (rowid, text, title, section) VALUES (?,?,?,?)")
        .bind(chunk_id)
        .bind(text)
        .bind(title)
        .bind(section.unwrap_or(""))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn fts_delete_doc(tx: &mut Transaction<'_, Sqlite>, doc_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?)").bind(doc_id).execute(&mut **tx).await?;
    Ok(())
}

pub struct BackfillRow {
    pub chunk_id: i64,
    pub text: String,
    pub title: String,
    pub section: Option<String>,
}

/// Siguiente tanda de fragmentos con `id > after`, por id (indexado por clave, sin releer lo hecho).
pub async fn chunks_after(pool: &SqlitePool, after: i64, limit: i64) -> Result<Vec<BackfillRow>> {
    let rows = sqlx::query("SELECT c.id, c.text, d.title, c.section FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id > ? ORDER BY c.id LIMIT ?")
        .bind(after)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| BackfillRow { chunk_id: r.get(0), text: r.get(1), title: r.get(2), section: r.get(3) }).collect())
}

/// Reindexa una tanda (borrar + insertar: idempotente).
pub async fn fts_reindex(pool: &SqlitePool, rows: &[BackfillRow]) -> Result<()> {
    let mut tx = pool.begin().await?;
    for r in rows {
        sqlx::query("DELETE FROM chunks_fts WHERE rowid = ?").bind(r.chunk_id).execute(&mut *tx).await?;
        fts_insert(&mut tx, r.chunk_id, &r.text, &r.title, r.section.as_deref()).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub struct LexRow {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub text_hash: String,
}

pub struct LexQuery<'a> {
    pub expr: &'a str,
    pub kinds: &'a [DocKind],
    pub assets: &'a [String],
    pub include_general: bool,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: i64,
}

/// Mejores fragmentos por BM25 (de mejor a peor) con los filtros de tipo, fechas y activos aplicados
/// en SQL (si no, con un activo concreto el top global podría no contener ninguno suyo).
pub async fn lexical_search(pool: &SqlitePool, q: &LexQuery<'_>) -> Result<Vec<LexRow>> {
    let mut sql = String::from(
        "SELECT chunks_fts.rowid, c.document_id, c.text_hash FROM chunks_fts \
         JOIN chunks c ON c.id = chunks_fts.rowid JOIN documents d ON d.id = c.document_id WHERE chunks_fts MATCH ?",
    );
    if !q.kinds.is_empty() {
        sql.push_str(&format!(" AND d.kind IN ({})", vec!["?"; q.kinds.len()].join(",")));
    }
    if q.since_ms.is_some() {
        sql.push_str(" AND d.published_at >= ?");
    }
    if q.until_ms.is_some() {
        sql.push_str(" AND d.published_at <= ?");
    }
    if !q.assets.is_empty() {
        sql.push_str(&format!(" AND (d.id IN (SELECT document_id FROM document_assets WHERE asset_id IN ({}))", vec!["?"; q.assets.len()].join(",")));
        if q.include_general {
            sql.push_str(" OR NOT EXISTS (SELECT 1 FROM document_assets x WHERE x.document_id = d.id)");
        }
        sql.push(')');
    }
    sql.push_str(&format!(" ORDER BY bm25(chunks_fts, {FTS_WEIGHTS}), chunks_fts.rowid LIMIT ?"));
    let mut query = sqlx::query(&sql).bind(q.expr);
    for k in q.kinds {
        query = query.bind(k.as_str());
    }
    if let Some(s) = q.since_ms {
        query = query.bind(s);
    }
    if let Some(u) = q.until_ms {
        query = query.bind(u);
    }
    for a in q.assets {
        query = query.bind(a);
    }
    let rows = query.bind(q.limit).fetch_all(pool).await?;
    Ok(rows.iter().map(|r| LexRow { chunk_id: r.get(0), doc_id: r.get(1), text_hash: r.get(2) }).collect())
}

pub struct Existing {
    pub id: i64,
    pub content_hash: String,
}

pub async fn find_by_key(pool: &SqlitePool, key: &str) -> Result<Option<Existing>> {
    let row = sqlx::query("SELECT id, content_hash FROM documents WHERE doc_key = ?").bind(key).fetch_optional(pool).await?;
    Ok(row.map(|r| Existing { id: r.get(0), content_hash: r.get(1) }))
}

pub async fn find_by_hash(pool: &SqlitePool, hash: &str) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM documents WHERE content_hash = ?").bind(hash).fetch_optional(pool).await?)
}

/// Embeddings ya calculados para estos textos (mismo `text_hash` y modelo): `hash -> vector`.
pub async fn embeddings_by_text_hash(pool: &SqlitePool, model: &str, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>> {
    let mut out = HashMap::new();
    for part in hashes.chunks(400) {
        let marks = vec!["?"; part.len()].join(",");
        let sql = format!(
            "SELECT c.text_hash, e.vec FROM chunks c JOIN embeddings e ON e.chunk_id = c.id AND e.model = ? WHERE c.text_hash IN ({marks}) GROUP BY c.text_hash"
        );
        let mut q = sqlx::query(&sql).bind(model);
        for h in part {
            q = q.bind(h);
        }
        for r in q.fetch_all(pool).await? {
            let h: String = r.get(0);
            let blob: Vec<u8> = r.get(1);
            if let Some(v) = from_blob(&blob) {
                out.insert(h, v);
            }
        }
    }
    Ok(out)
}

fn doc_info(r: &sqlx::sqlite::SqliteRow, assets: Vec<String>) -> Result<DocumentInfo> {
    let kind: String = r.get("kind");
    Ok(DocumentInfo {
        id: r.get("id"),
        doc_key: r.get("doc_key"),
        kind: DocKind::parse(&kind).ok_or_else(|| KnowledgeError::Internal(format!("tipo desconocido en BD: {kind}")))?,
        subtype: r.get("subtype"),
        source: r.get("source"),
        publisher: r.get("publisher"),
        title: r.get("title"),
        url: r.get("url"),
        issuer: r.get("issuer"),
        language: r.get("language"),
        published_at: DateTime::from_timestamp_millis(r.get("published_at")).unwrap_or_default(),
        assets,
        pinned: r.get::<i64, _>("pinned") == 1,
    })
}

/// Todos los documentos con sus activos (para arrancar el índice).
pub async fn load_documents(pool: &SqlitePool) -> Result<Vec<DocumentInfo>> {
    let mut assets: HashMap<i64, Vec<String>> = HashMap::new();
    for r in sqlx::query("SELECT document_id, asset_id FROM document_assets ORDER BY asset_id").fetch_all(pool).await? {
        assets.entry(r.get(0)).or_default().push(r.get(1));
    }
    let mut out = Vec::new();
    for r in sqlx::query("SELECT * FROM documents").fetch_all(pool).await? {
        let id: i64 = r.get("id");
        out.push(doc_info(&r, assets.remove(&id).unwrap_or_default())?);
    }
    Ok(out)
}

pub struct LoadedVector {
    pub chunk_id: i64,
    pub document_id: i64,
    pub text_hash: u128,
    pub vec: Vec<f32>,
}

pub async fn load_vectors(pool: &SqlitePool, model: &str) -> Result<Vec<LoadedVector>> {
    let mut out = Vec::new();
    let rows = sqlx::query("SELECT c.id, c.document_id, c.text_hash, e.dim, e.vec FROM embeddings e JOIN chunks c ON c.id = e.chunk_id WHERE e.model = ? ORDER BY c.id")
        .bind(model)
        .fetch_all(pool)
        .await?;
    for r in rows {
        let blob: Vec<u8> = r.get(4);
        let dim: i64 = r.get(3);
        let (Some(vec), Some(text_hash)) = (from_blob(&blob), parse_hash128(&r.get::<String, _>(2))) else { continue };
        if vec.len() as i64 != dim {
            continue;
        }
        out.push(LoadedVector { chunk_id: r.get(0), document_id: r.get(1), text_hash, vec });
    }
    Ok(out)
}

pub struct PendingChunk {
    pub chunk_id: i64,
    pub document_id: i64,
    pub kind: DocKind,
    pub title: String,
    pub section: Option<String>,
    pub text: String,
    pub text_hash: String,
}

pub async fn pending_chunks(pool: &SqlitePool, model: &str, limit: i64) -> Result<Vec<PendingChunk>> {
    let rows = sqlx::query(
        "SELECT c.id, c.document_id, d.kind, d.title, c.section, c.text, c.text_hash
         FROM chunks c JOIN documents d ON d.id = c.document_id
         WHERE NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.chunk_id = c.id AND e.model = ?)
         ORDER BY d.published_at DESC, c.id LIMIT ?",
    )
    .bind(model)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            let kind: String = r.get(2);
            Ok(PendingChunk {
                chunk_id: r.get(0),
                document_id: r.get(1),
                kind: DocKind::parse(&kind).ok_or_else(|| KnowledgeError::Internal("tipo desconocido".into()))?,
                title: r.get(3),
                section: r.get(4),
                text: r.get(5),
                text_hash: r.get(6),
            })
        })
        .collect()
}

pub async fn count_pending(pool: &SqlitePool, model: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM chunks c WHERE NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.chunk_id = c.id AND e.model = ?)")
        .bind(model)
        .fetch_one(pool)
        .await?)
}

/// Fragmentos con su documento, por id (para componer los resultados).
pub async fn chunks_with_docs(pool: &SqlitePool, ids: &[i64]) -> Result<HashMap<i64, (Option<String>, String, DocumentInfo)>> {
    let mut out = HashMap::new();
    for part in ids.chunks(400) {
        let marks = vec!["?"; part.len()].join(",");
        let sql = format!("SELECT c.id AS chunk_id, c.section AS csection, c.text AS ctext, d.* FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id IN ({marks})");
        let mut q = sqlx::query(&sql);
        for i in part {
            q = q.bind(i);
        }
        for r in q.fetch_all(pool).await? {
            let did: i64 = r.get("id");
            let assets = assets_of(pool, did).await?;
            out.insert(r.get("chunk_id"), (r.get("csection"), r.get("ctext"), doc_info(&r, assets)?));
        }
    }
    Ok(out)
}

/// Documentos candidatos a borrar por antigüedad: `(id, chunk_count)` no fijados de un tipo anterior a `cutoff_ms`.
pub async fn expired(pool: &SqlitePool, kind: DocKind, cutoff_ms: i64) -> Result<Vec<(i64, i64)>> {
    let rows = sqlx::query("SELECT id, chunk_count FROM documents WHERE kind = ? AND pinned = 0 AND published_at < ?").bind(kind.as_str()).bind(cutoff_ms).fetch_all(pool).await?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

/// Candidatos a expulsar por tope, de menos a más valiosos: noticias antiguas primero, luego
/// folletos; nunca análisis ni fijados.
pub async fn eviction_order(pool: &SqlitePool) -> Result<Vec<(i64, i64)>> {
    let rows = sqlx::query(
        "SELECT id, chunk_count FROM documents WHERE pinned = 0 AND kind IN ('news','filing')
         ORDER BY CASE kind WHEN 'news' THEN 0 ELSE 1 END, published_at ASC, id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

pub async fn total_chunks(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM chunks").fetch_one(pool).await?)
}

pub async fn sizes(pool: &SqlitePool) -> Result<(i64, i64, i64)> {
    let docs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents").fetch_one(pool).await?;
    let text: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(LENGTH(CAST(text AS BLOB))),0) FROM chunks").fetch_one(pool).await?;
    let emb: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(LENGTH(vec)),0) FROM embeddings").fetch_one(pool).await?;
    Ok((docs, text, emb))
}

pub async fn set_pinned(pool: &SqlitePool, id: i64, pinned: bool) -> Result<bool> {
    Ok(sqlx::query("UPDATE documents SET pinned = ? WHERE id = ?").bind(i64::from(pinned)).bind(id).execute(pool).await?.rows_affected() > 0)
}

pub async fn get_meta(pool: &SqlitePool, id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT meta FROM documents WHERE id = ?").bind(id).fetch_optional(pool).await?)
}

pub async fn set_meta(pool: &SqlitePool, id: i64, meta: &str) -> Result<()> {
    sqlx::query("UPDATE documents SET meta = ? WHERE id = ?").bind(meta).bind(id).execute(pool).await?;
    Ok(())
}
