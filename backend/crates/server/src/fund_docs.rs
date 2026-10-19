//! Folleto resumido de los ETF en el archivo de conocimiento (RAG). Diseño en `docs/notes/rag.md`.
//!
//! Cuando se pide el snapshot (o, por tanto, el análisis) de un ETF o fondo, `TeeMarket` ofrece su
//! identificador a [`FundSink`]. Un trabajador en segundo plano descarga el folleto resumido
//! (497K, o 485BPOS) de EDGAR y lo archiva como `Filing`, con las secciones clave (objetivo,
//! comisiones, estrategia, riesgos y rotación).
//!
//! - La respuesta al usuario nunca espera: `offer` es síncrono y no bloquea (cola acotada).
//! - Solo existe con `RAG_ENABLED=1` (sin RAG no se crea nada y no se pide nada a EDGAR).
//! - Como mucho una descarga por ETF cada [`REFRESH_DAYS`] días, también entre reinicios (se mira
//!   la fecha del documento ya archivado) y un reintento en memoria cada [`RETRY`] tras un fallo.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{NaiveTime, Utc};
use domain::{AssetClass, AssetId, FundDocs, Prospectus};
use knowledge::{DocKind, KnowledgeBase, NewDocument, Section};
use sqlx::{Row, SqlitePool};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// Días entre refrescos de un mismo folleto.
pub const REFRESH_DAYS: i64 = 30;
/// Espera en memoria antes de volver a intentar un ETF (con éxito, fallo o sin folleto).
pub const RETRY: Duration = Duration::from_secs(6 * 3600);
/// Tiempo máximo de una descarga completa (varias peticiones a EDGAR).
const FETCH_TIMEOUT: Duration = Duration::from_secs(150);
const QUEUE: usize = 8;
const SEEN_CAP: usize = 1000;

/// Cola de ETF cuyo folleto conviene archivar. `offer` nunca espera.
#[derive(Clone)]
pub struct FundSink {
    tx: mpsc::Sender<AssetId>,
    seen: Arc<Mutex<HashMap<String, Instant>>>,
}

impl FundSink {
    /// Ofrece un activo; ignora lo que no sea ETF/fondo y lo ofrecido hace menos de [`RETRY`].
    pub fn offer(&self, id: &AssetId) {
        if !matches!(id.class, AssetClass::Etf | AssetClass::Fund) {
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

/// Cola + trabajador. El trabajador arranca con [`FundArchive::start`], cuando ya hay un `FundDocs`.
pub struct FundArchive {
    sink: FundSink,
    rx: Mutex<Option<mpsc::Receiver<AssetId>>>,
    kb: Arc<KnowledgeBase>,
    pool: SqlitePool,
    shutdown: CancellationToken,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl FundArchive {
    pub fn new(kb: Arc<KnowledgeBase>, pool: SqlitePool, shutdown: CancellationToken) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE);
        Self { sink: FundSink { tx, seen: Arc::default() }, rx: Mutex::new(Some(rx)), kb, pool, shutdown, handles: Mutex::default() }
    }

    pub fn sink(&self) -> FundSink {
        self.sink.clone()
    }

    /// Arranca el trabajador (una sola vez; las siguientes llamadas no hacen nada).
    pub fn start(&self, docs: Arc<dyn FundDocs>) {
        let Some(mut rx) = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let (kb, pool, shutdown) = (self.kb.clone(), self.pool.clone(), self.shutdown.clone());
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    item = rx.recv() => {
                        let Some(id) = item else { break };
                        archive_one(&kb, &pool, docs.as_ref(), &id).await;
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

/// Clave lógica del folleto de un ETF: una por ticker, así el de cada año sustituye al anterior.
pub fn doc_key(id: &AssetId) -> String {
    format!("etf:{}", id.symbol.to_ascii_uppercase())
}

/// Edad (ms) del folleto archivado: desde la última vez que se archivó o se comprobó.
async fn archived_age_ms(pool: &SqlitePool, key: &str) -> Option<i64> {
    let row = sqlx::query("SELECT ingested_at, meta FROM documents WHERE doc_key = ?").bind(key).fetch_optional(pool).await.ok()??;
    let ingested: i64 = row.try_get(0).ok()?;
    let meta: String = row.try_get(1).unwrap_or_default();
    let checked = serde_json::from_str::<serde_json::Value>(&meta).ok().and_then(|m| m["checkedAt"].as_i64()).unwrap_or(0);
    Some(Utc::now().timestamp_millis() - ingested.max(checked))
}

async fn archive_one(kb: &KnowledgeBase, pool: &SqlitePool, docs: &dyn FundDocs, id: &AssetId) {
    let key = doc_key(id);
    if archived_age_ms(pool, &key).await.is_some_and(|age| age < REFRESH_DAYS * 24 * 3600 * 1000) {
        tracing::debug!(%id, "folleto del ETF ya archivado hace menos de {REFRESH_DAYS} días");
        return;
    }
    let p = match tokio::time::timeout(FETCH_TIMEOUT, docs.fund_prospectus(id)).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            tracing::debug!(%id, error = %e, "sin folleto de ETF que archivar");
            return;
        }
        Err(_) => {
            tracing::warn!(%id, "descarga del folleto de ETF: tiempo agotado");
            return;
        }
    };
    let Some(doc) = fund_prospectus_to_document(&p, id) else {
        tracing::debug!(%id, "el folleto del ETF no tiene secciones");
        return;
    };
    match kb.ingest(doc).await {
        Ok(r) => {
            // Marca la comprobación: un documento sin cambios no mueve `ingested_at`.
            let _ = kb.merge_meta(r.document_id, serde_json::json!({ "checkedAt": Utc::now().timestamp_millis() })).await;
            tracing::debug!(%id, accession = %p.accession, outcome = ?r.outcome, chunks = r.chunks, pending = r.pending, "folleto de ETF archivado");
        }
        Err(e) => tracing::warn!(%id, error = %e, "no se pudo archivar el folleto del ETF"),
    }
}

fn heading(key: &str, fallback: &str) -> String {
    match key {
        "objective" => "Investment Objective",
        "fees" => "Fees and Expenses",
        "strategy" => "Principal Investment Strategies",
        "risks" => "Principal Risks",
        "turnover" => "Portfolio Turnover",
        _ => fallback,
    }
    .to_string()
}

/// Folleto de un ETF -> documento del archivo (`Filing`, una sección por sección clave). `None` si
/// no hay texto o falta el identificador del documento.
pub fn fund_prospectus_to_document(p: &Prospectus, id: &AssetId) -> Option<NewDocument> {
    let sections: Vec<Section> = p
        .sections
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| Section { heading: Some(heading(&s.key, &s.title)), text: s.text.clone() })
        .collect();
    if sections.is_empty() || p.accession.trim().is_empty() {
        return None;
    }
    let ticker = id.symbol.to_ascii_uppercase();
    Some(NewDocument {
        doc_key: doc_key(id),
        kind: DocKind::Filing,
        subtype: Some(p.form.clone()),
        source: "sec".into(),
        publisher: Some("SEC EDGAR".into()),
        title: format!("{} ({ticker}) — {}", p.issuer, p.form),
        url: Some(p.url.clone()),
        issuer: Some(p.issuer.clone()),
        language: Some("en".into()),
        published_at: p.filed_on.and_time(NaiveTime::MIN).and_utc(),
        assets: vec![id.to_string()],
        sections,
        meta: serde_json::json!({
            "cik": p.cik, "accession": p.accession, "indexUrl": p.index_url, "ticker": ticker, "etf": true,
            "missing": p.missing, "checkedAt": Utc::now().timestamp_millis(),
        }),
    })
}
