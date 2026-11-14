//! Informes periódicos de las empresas en el archivo de conocimiento (RAG). Diseño en `docs/notes/rag.md`.
//!
//! Cuando se pide el snapshot de una acción, `TeeMarket` ofrece su identificador a [`CompanySink`].
//! Un trabajador en segundo plano llama a `CompanyDocs::company_reports` (EDGAR: último 10-K / 20-F /
//! 40-F, último 10-Q si es posterior y nota de resultados del último 8-K) y archiva cada informe como
//! `Filing`, con sus secciones clave.
//!
//! - La respuesta al usuario nunca espera: `offer` es síncrono y no bloquea (cola acotada).
//! - Solo existe con `RAG_ARCHIVE=1` (sin archivado no se crea nada y no se pide nada a EDGAR).
//! - Un documento por empresa y familia (`sec:<TICKER>:annual|quarterly|earnings`): el informe nuevo
//!   sustituye al anterior de su familia. Nunca se borra por mera ausencia en la respuesta; solo con
//!   prueba de que lo archivado quedó anticuado (ver `superseded`). Con `Err` no se borra nada.
//! - Como mucho una descarga por empresa cada [`REFRESH_DAYS`] días, también entre reinicios (se mira
//!   la fecha de lo ya archivado y enlazado al activo) y un reintento en memoria cada [`crate::doc_queue::RETRY`] tras un fallo.

use std::{collections::HashMap, sync::Arc};

use chrono::{NaiveTime, Utc};
use domain::{AssetClass, AssetId, CompanyDocs, Prospectus};
use knowledge::{DocKind, KnowledgeBase, NewDocument, Section};
use sqlx::SqlitePool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::doc_queue::{DocArchive, DocSink, FETCH_TIMEOUT, ingest_checked, linked_age_ms};

/// Días entre descargas de una misma empresa.
pub const REFRESH_DAYS: i64 = 7;
/// Una nota de resultados más vieja que esto ya no es lo último de la empresa: no se archiva y, si estaba archivada, se borra.
pub const EARNINGS_MAX_AGE_DAYS: i64 = 120;
const QUEUE: usize = 32;
const DAY_MS: i64 = 24 * 3600 * 1000;
/// Familias de informe, una por documento archivado.
const FAMILIES: [&str; 3] = ["annual", "quarterly", "earnings"];

/// Cola de acciones cuyos informes conviene archivar. `offer` nunca espera.
pub type CompanySink = DocSink;

fn is_stock(id: &AssetId) -> bool {
    id.class == AssetClass::Stock
}

/// Cola + trabajador. El trabajador arranca con [`CompanyArchive::start`], cuando ya hay un `CompanyDocs`.
pub struct CompanyArchive(DocArchive);

impl CompanyArchive {
    pub fn new(kb: Arc<KnowledgeBase>, pool: SqlitePool, shutdown: CancellationToken) -> Self {
        Self(DocArchive::new(kb, pool, shutdown, QUEUE, is_stock))
    }

    pub fn sink(&self) -> CompanySink {
        self.0.sink()
    }

    /// Arranca el trabajador (una sola vez; las siguientes llamadas no hacen nada).
    pub fn start(&self, docs: Arc<dyn CompanyDocs>) {
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

/// Familia de un formulario: anual (10-K, 20-F, 40-F y enmiendas), trimestral (10-Q) o resultados (8-K).
fn family_of(form: &str) -> Option<&'static str> {
    let form = form.trim().to_ascii_uppercase();
    let is = |prefixes: &[&str]| prefixes.iter().any(|p| form.starts_with(p));
    if is(&["10-K", "20-F", "40-F"]) {
        Some(FAMILIES[0])
    } else if is(&["10-Q"]) {
        Some(FAMILIES[1])
    } else if is(&["8-K", "6-K"]) {
        Some(FAMILIES[2])
    } else {
        None
    }
}

/// Clave lógica del informe de una familia: una por ticker y familia, así el nuevo sustituye al anterior.
fn doc_key(id: &AssetId, family: &str) -> String {
    format!("sec:{}:{family}", id.symbol.to_ascii_uppercase())
}

async fn archive_one(kb: &KnowledgeBase, pool: &SqlitePool, docs: &dyn CompanyDocs, id: &AssetId) {
    // Lo más reciente de lo ya archivado de esta empresa, por activo y no por clave: en las acciones de dos
    // clases (GOOG/GOOGL) el informe sale `Duplicate` del de la otra y solo queda enlazado, sin clave propia.
    if linked_age_ms(pool, &id.to_string(), "sec:").await.is_some_and(|age| age < REFRESH_DAYS * DAY_MS) {
        tracing::debug!(%id, "informes de la empresa ya archivados hace menos de {REFRESH_DAYS} días");
        return;
    }
    let reports = match tokio::time::timeout(FETCH_TIMEOUT, docs.company_reports(id)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            // Con error no se borra nada: lo ya archivado sigue siendo lo mejor que hay.
            tracing::debug!(%id, error = %e, "sin informes de empresa que archivar");
            return;
        }
        Err(_) => {
            tracing::warn!(%id, "descarga de informes de empresa: tiempo agotado");
            return;
        }
    };
    // El más reciente de cada familia.
    let today = Utc::now().date_naive();
    let mut latest: HashMap<&'static str, Prospectus> = HashMap::new();
    for p in reports {
        let Some(family) = family_of(&p.form) else {
            tracing::debug!(%id, form = %p.form, "formulario sin familia conocida: se ignora");
            continue;
        };
        if family == FAMILIES[2] && (today - p.filed_on).num_days() > EARNINGS_MAX_AGE_DAYS {
            continue;
        }
        if latest.get(family).is_none_or(|b| p.filed_on > b.filed_on) {
            latest.insert(family, p);
        }
    }
    for family in FAMILIES {
        let Some(p) = latest.get(family) else {
            // Nunca se borra por mera ausencia (un `Ok` incompleto, un cambio de CIK…): solo con prueba de que
            // lo archivado quedó anticuado.
            if superseded(pool, &doc_key(id, family), family, &latest).await {
                match kb.delete_by_key(&doc_key(id, family)).await {
                    Ok(true) => tracing::debug!(%id, family, "informe anticuado borrado del archivo"),
                    Ok(false) => {}
                    Err(e) => tracing::warn!(%id, family, error = %e, "no se pudo borrar el informe anticuado"),
                }
            }
            continue;
        };
        let Some(doc) = company_report_to_document(p, id) else {
            tracing::debug!(%id, form = %p.form, "el informe de la empresa no tiene secciones");
            continue;
        };
        match ingest_checked(kb, doc).await {
            Ok(r) => tracing::debug!(%id, form = %p.form, accession = %p.accession, outcome = ?r.outcome, chunks = r.chunks, pending = r.pending, "informe de empresa archivado"),
            Err(e) => tracing::warn!(%id, form = %p.form, error = %e, "no se pudo archivar el informe de la empresa"),
        }
    }
}

/// ¿Hay prueba de que el informe archivado de esta familia quedó anticuado? Solo dos casos: el 10-Q cuando la
/// respuesta trae un anual presentado después de él, y la nota de resultados con más de
/// [`EARNINGS_MAX_AGE_DAYS`] días según su propia fecha (la respuesta no trae una nota que la sustituya).
/// El anual nunca caduca por ausencia.
async fn superseded(pool: &SqlitePool, key: &str, family: &str, latest: &HashMap<&'static str, Prospectus>) -> bool {
    let published: Option<i64> = sqlx::query_scalar("SELECT published_at FROM documents WHERE doc_key = ?").bind(key).fetch_optional(pool).await.ok().flatten();
    let Some(published) = published else { return false };
    match family {
        "quarterly" => latest.get("annual").is_some_and(|a| a.filed_on.and_time(NaiveTime::MIN).and_utc().timestamp_millis() > published),
        "earnings" => Utc::now().timestamp_millis() - published > EARNINGS_MAX_AGE_DAYS * DAY_MS,
        _ => false,
    }
}

fn heading(key: &str, fallback: &str) -> String {
    match key {
        "risk_factors" => "Risk Factors",
        "mdna" => "Management's Discussion and Analysis",
        "market_risk" => "Quantitative and Qualitative Disclosures About Market Risk",
        "business" => "Business",
        "operating_review" => "Operating and Financial Review and Prospects",
        "earnings_release" => "Earnings Release",
        _ => fallback,
    }
    .to_string()
}

/// Informe de una empresa -> documento del archivo (`Filing`, una sección por sección clave; las
/// subsecciones, cuyo texto ya contiene el padre, no se indexan dos veces). `None` si no hay texto,
/// falta el identificador del documento o el formulario no es de una familia conocida.
pub fn company_report_to_document(p: &Prospectus, id: &AssetId) -> Option<NewDocument> {
    let family = family_of(&p.form)?;
    let sections: Vec<Section> = p
        .sections
        .iter()
        .filter(|s| s.parent.is_none() && !s.text.trim().is_empty())
        .map(|s| Section { heading: Some(heading(&s.key, &s.title)).filter(|h| !h.trim().is_empty()), text: s.text.clone() })
        .collect();
    if sections.is_empty() || p.accession.trim().is_empty() {
        return None;
    }
    let ticker = id.symbol.to_ascii_uppercase();
    Some(NewDocument {
        doc_key: doc_key(id, family),
        kind: DocKind::Filing,
        subtype: Some(p.form.clone()),
        source: "sec".into(),
        publisher: Some("SEC EDGAR".into()),
        title: format!("{} ({ticker}) — {} {}", p.issuer, p.form, p.filed_on),
        url: Some(p.url.clone()),
        issuer: Some(p.issuer.clone()),
        language: Some("en".into()),
        published_at: p.filed_on.and_time(NaiveTime::MIN).and_utc(),
        assets: vec![id.to_string()],
        sections,
        meta: serde_json::json!({
            "cik": p.cik, "accession": p.accession, "indexUrl": p.index_url, "ticker": ticker,
            "missing": p.missing, "checkedAt": Utc::now().timestamp_millis(),
        }),
    })
}
