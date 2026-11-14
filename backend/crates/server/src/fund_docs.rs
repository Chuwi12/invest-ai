//! Folleto resumido de los ETF en el archivo de conocimiento (RAG). Diseño en `docs/notes/rag.md`.
//!
//! Cuando se pide el snapshot (o, por tanto, el análisis) de un ETF o fondo, `TeeMarket` ofrece su
//! identificador a [`FundSink`]. Un trabajador en segundo plano descarga el folleto resumido
//! (497K, o 485BPOS) de EDGAR y lo archiva como `Filing`, con las secciones clave (objetivo,
//! comisiones, estrategia, riesgos y rotación).
//!
//! - La respuesta al usuario nunca espera: `offer` es síncrono y no bloquea (cola acotada).
//! - Solo existe con `RAG_ARCHIVE=1` (sin archivado no se crea nada y no se pide nada a EDGAR).
//! - Como mucho una descarga por ETF cada [`REFRESH_DAYS`] días, también entre reinicios (se mira
//!   la fecha del documento ya archivado) y un reintento en memoria cada [`RETRY`] tras un fallo.

use std::sync::Arc;

use chrono::{NaiveTime, Utc};
use domain::{AssetClass, AssetId, FundDocs, Prospectus};
use knowledge::{DocKind, KnowledgeBase, NewDocument, Section};
use sqlx::SqlitePool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::doc_queue::{DocArchive, DocSink, FETCH_TIMEOUT, archived_age_ms, ingest_checked};
pub use crate::doc_queue::RETRY;

/// Días entre refrescos de un mismo folleto.
pub const REFRESH_DAYS: i64 = 30;
const QUEUE: usize = 8;

/// Cola de ETF cuyo folleto conviene archivar. `offer` nunca espera.
pub type FundSink = DocSink;

fn is_fund(id: &AssetId) -> bool {
    matches!(id.class, AssetClass::Etf | AssetClass::Fund)
}

/// Cola + trabajador. El trabajador arranca con [`FundArchive::start`], cuando ya hay un `FundDocs`.
pub struct FundArchive(DocArchive);

impl FundArchive {
    pub fn new(kb: Arc<KnowledgeBase>, pool: SqlitePool, shutdown: CancellationToken) -> Self {
        Self(DocArchive::new(kb, pool, shutdown, QUEUE, is_fund))
    }

    pub fn sink(&self) -> FundSink {
        self.0.sink()
    }

    /// Arranca el trabajador (una sola vez; las siguientes llamadas no hacen nada).
    pub fn start(&self, docs: Arc<dyn FundDocs>) {
        self.0.start(move |kb, pool, id| {
            let docs = docs.clone();
            async move { archive_one(&kb, &pool, docs.as_ref(), &id).await }
        });
    }

    /// Trabajadores lanzados, para que el apagado los espere.
    pub fn take_handles(&self) -> Vec<JoinHandle<()>> {
        self.0.take_handles()
    }
}

/// Clave lógica del folleto de un ETF: una por ticker, así el de cada año sustituye al anterior.
pub fn doc_key(id: &AssetId) -> String {
    format!("etf:{}", id.symbol.to_ascii_uppercase())
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
    match ingest_checked(kb, doc).await {
        Ok(r) => tracing::debug!(%id, accession = %p.accession, outcome = ?r.outcome, chunks = r.chunks, pending = r.pending, "folleto de ETF archivado"),
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
