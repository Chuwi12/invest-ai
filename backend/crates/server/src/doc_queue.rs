//! Cola acotada de activos cuyo informe conviene archivar en segundo plano. La comparten el folleto
//! resumido de los ETF (`fund_docs`) y los informes de las empresas (`company_docs`). Diseño en
//! `docs/notes/rag.md`.
//!
//! - `offer` es síncrono y nunca bloquea: cola acotada con `try_send`, y si está llena se olvida el
//!   activo para que la próxima petición lo reintente.
//! - Un activo ofrecido hace menos de [`RETRY`] (con éxito o con fallo) no se vuelve a encolar.
//! - La edad del documento ya archivado (`ingested_at` / `meta.checkedAt`) la comprueba cada trabajo.

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use domain::AssetId;
use knowledge::{IngestReport, KnowledgeBase, NewDocument};
use sqlx::{Row, SqlitePool};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// Espera en memoria antes de volver a intentar un activo (con éxito, fallo o sin informe).
pub const RETRY: Duration = Duration::from_secs(6 * 3600);
/// Tiempo máximo de una descarga completa (varias peticiones a EDGAR).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(150);
const SEEN_CAP: usize = 1000;

/// Cola de activos cuyo informe conviene archivar. `offer` nunca espera.
#[derive(Clone)]
pub struct DocSink {
    tx: mpsc::Sender<AssetId>,
    seen: Arc<Mutex<HashMap<String, Instant>>>,
    accepts: fn(&AssetId) -> bool,
}

impl DocSink {
    /// Ofrece un activo; ignora los que la cola no acepta y los ofrecidos hace menos de [`RETRY`].
    pub fn offer(&self, id: &AssetId) {
        if !(self.accepts)(id) {
            return;
        }
        let key = id.to_string();
        {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() > SEEN_CAP {
                seen.clear();
            }
            if seen.get(&key).is_some_and(|t| t.elapsed() < RETRY) {
                return;
            }
            seen.insert(key.clone(), Instant::now());
        }
        if self.tx.try_send(id.clone()).is_err() {
            // Cola llena: se olvida para que la próxima petición lo reintente.
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        }
    }
}

/// Cola + trabajador. El trabajador arranca con [`DocArchive::start`], cuando ya hay una fuente de documentos.
pub struct DocArchive {
    sink: DocSink,
    rx: Mutex<Option<mpsc::Receiver<AssetId>>>,
    kb: Arc<KnowledgeBase>,
    pool: SqlitePool,
    shutdown: CancellationToken,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl DocArchive {
    /// `queue`: activos pendientes como máximo; `accepts`: qué clases de activo se encolan.
    pub fn new(kb: Arc<KnowledgeBase>, pool: SqlitePool, shutdown: CancellationToken, queue: usize, accepts: fn(&AssetId) -> bool) -> Self {
        let (tx, rx) = mpsc::channel(queue);
        Self {
            sink: DocSink { tx, seen: Arc::default(), accepts },
            rx: Mutex::new(Some(rx)),
            kb,
            pool,
            shutdown,
            handles: Mutex::default(),
        }
    }

    pub fn sink(&self) -> DocSink {
        self.sink.clone()
    }

    /// Arranca el trabajador (una sola vez; las siguientes llamadas no hacen nada). `work` recibe el
    /// archivo, la BD y el activo, y los procesa de uno en uno.
    pub fn start<F, Fut>(&self, work: F)
    where
        F: Fn(Arc<KnowledgeBase>, SqlitePool, AssetId) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let Some(mut rx) = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let (kb, pool, shutdown) = (self.kb.clone(), self.pool.clone(), self.shutdown.clone());
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    item = rx.recv() => {
                        let Some(id) = item else { break };
                        work(kb.clone(), pool.clone(), id).await;
                    }
                }
            }
        });
        self.handles.lock().unwrap_or_else(|e| e.into_inner()).push(handle);
    }

    /// Trabajadores lanzados, para que el apagado los espere.
    pub fn take_handles(&self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut *self.handles.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Edad (ms) de un documento archivado: desde la última vez que se archivó o se comprobó (`meta.checkedAt`).
fn age_ms(ingested_at: i64, meta: &str) -> i64 {
    let checked = serde_json::from_str::<serde_json::Value>(meta).ok().and_then(|m| m["checkedAt"].as_i64()).unwrap_or(0);
    Utc::now().timestamp_millis() - ingested_at.max(checked)
}

/// Edad (ms) del documento archivado con esa clave.
pub async fn archived_age_ms(pool: &SqlitePool, key: &str) -> Option<i64> {
    let row = sqlx::query("SELECT ingested_at, meta FROM documents WHERE doc_key = ?").bind(key).fetch_optional(pool).await.ok()??;
    Some(age_ms(row.try_get(0).ok()?, &row.try_get::<String, _>(1).unwrap_or_default()))
}

/// Edad (ms) del documento más reciente cuya clave empieza por `key_prefix` y que está enlazado al activo
/// `asset` (no importa con qué clave se archivó: un duplicado de otro ticker sigue enlazado a este).
pub async fn linked_age_ms(pool: &SqlitePool, asset: &str, key_prefix: &str) -> Option<i64> {
    let rows = sqlx::query("SELECT d.ingested_at, d.meta FROM documents d JOIN document_assets a ON a.document_id = d.id WHERE a.asset_id = ? AND substr(d.doc_key, 1, ?) = ?")
        .bind(asset)
        .bind(key_prefix.len() as i64)
        .bind(key_prefix)
        .fetch_all(pool)
        .await
        .ok()?;
    rows.iter().filter_map(|r| Some(age_ms(r.try_get(0).ok()?, &r.try_get::<String, _>(1).unwrap_or_default()))).min()
}

/// Archiva el documento y marca la comprobación (`meta.checkedAt`): un documento sin cambios no mueve `ingested_at`.
pub async fn ingest_checked(kb: &KnowledgeBase, doc: NewDocument) -> knowledge::Result<IngestReport> {
    let r = kb.ingest(doc).await?;
    let _ = kb.merge_meta(r.document_id, serde_json::json!({ "checkedAt": Utc::now().timestamp_millis() })).await;
    Ok(r)
}
