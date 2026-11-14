//! Informes periódicos de las empresas en el RAG: al pedir el snapshot de una acción se archivan, en
//! segundo plano y solo con `RAG_ARCHIVE=1`, el último 10-K, el último 10-Q y la nota de resultados
//! (un documento por familia), como mucho una descarga por empresa cada 7 días. Embedder simulado y
//! `CompanyDocs` falso: sin red.

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::{NaiveDate, Utc};
use common::TestApp;
use domain::{AssetId, CompanyDocs, DomainError, FundDocs, IpoData, MarketData, Prospectus, ProspectusSection};
use knowledge::{DocKind, FakeEmbedder, KnowledgeBase, KnowledgeConfig, RagModes};
use server::{
    company_docs::{CompanyArchive, company_report_to_document},
    knowledge::{Rag, open_with},
};
use tokio_util::sync::CancellationToken;

fn id(s: &str) -> AssetId {
    s.parse().unwrap()
}

fn days_ago(n: i64) -> NaiveDate {
    Utc::now().date_naive() - chrono::Duration::days(n)
}

fn section(key: &str, text: &str, parent: Option<&str>) -> ProspectusSection {
    ProspectusSection { key: key.into(), title: format!("título de {key}"), text: text.into(), parent: parent.map(String::from), truncated: false, original_chars: text.len() }
}

/// Informe de EDGAR de prueba; el texto de cada sección lleva la accesión para que cambie con ella.
fn report(form: &str, accession: &str, filed_days_ago: i64, keys: &[&str]) -> Prospectus {
    Prospectus {
        issuer: "Apple Inc.".into(),
        cik: 320193,
        ticker: Some("AAPL".into()),
        form: form.into(),
        preliminary: false,
        filed_on: days_ago(filed_days_ago),
        accession: accession.into(),
        url: format!("https://www.sec.gov/Archives/edgar/data/320193/{accession}/doc.htm"),
        index_url: format!("https://www.sec.gov/Archives/edgar/data/320193/{accession}/{accession}-index.htm"),
        offer_price: None,
        sections: keys.iter().map(|k| section(k, &format!("Texto de {k} del informe {accession}: ingresos por servicios y riesgos de la cadena de suministro."), None)).collect(),
        missing: vec![],
        document_chars: 1000,
        source: "sec".into(),
        fetched_at: Utc::now(),
    }
}

fn annual(accession: &str) -> Prospectus {
    report("10-K", accession, 40, &["risk_factors", "mdna", "business"])
}
fn quarterly(accession: &str) -> Prospectus {
    report("10-Q", accession, 10, &["mdna", "risk_factors"])
}
fn earnings(accession: &str, filed_days_ago: i64) -> Prospectus {
    report("8-K", accession, filed_days_ago, &["earnings_release"])
}

enum Reply {
    Reports(Vec<Prospectus>),
    Fail,
}

/// Fuente de informes de prueba: cuenta las llamadas, responde lo que se le diga y puede tardar.
struct StubDocs {
    calls: AtomicUsize,
    delay: Duration,
    reply: Mutex<Reply>,
}

impl StubDocs {
    fn new(reply: Reply) -> Arc<Self> {
        Self::slow(reply, Duration::ZERO)
    }

    fn slow(reply: Reply, delay: Duration) -> Arc<Self> {
        Arc::new(Self { calls: AtomicUsize::new(0), delay, reply: Mutex::new(reply) })
    }

    fn set(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CompanyDocs for StubDocs {
    async fn company_reports(&self, _id: &AssetId) -> domain::Result<Vec<Prospectus>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        match &*self.reply.lock().unwrap() {
            Reply::Reports(r) => Ok(r.clone()),
            Reply::Fail => Err(DomainError::NotFound("sin informes".into())),
        }
    }
}

/// Los ETF no son lo que se prueba aquí: su fuente siempre falla.
struct NoFunds;

#[async_trait]
impl FundDocs for NoFunds {
    async fn fund_prospectus(&self, _id: &AssetId) -> domain::Result<Prospectus> {
        Err(DomainError::NotFound("sin folleto".into()))
    }
}

async fn app_with_companies(docs: Arc<StubDocs>) -> (TestApp, Arc<KnowledgeBase>, Rag, CancellationToken) {
    app_with_modes(docs, RagModes::default()).await
}

async fn app_with_modes(docs: Arc<StubDocs>, modes: RagModes) -> (TestApp, Arc<KnowledgeBase>, Rag, CancellationToken) {
    let mut app = TestApp::new().await;
    let shutdown = CancellationToken::new();
    let cfg = KnowledgeConfig { modes, ..KnowledgeConfig::default() };
    let rag = open_with(cfg, app.state.db.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.expect("archivo abierto");
    let kb = rag.kb.clone();
    let (market, ipo, analyst) = rag.wrap_with_docs(app.state.market.clone(), app.state.ipo.clone(), app.state.analyst.clone(), Arc::new(NoFunds), docs);
    let mut state = app.state.clone().with_knowledge(Some(kb.clone()));
    (state.market, state.ipo, state.analyst) = (market, ipo, analyst);
    app.router = server::app(state.clone(), None);
    app.state = state;
    (app, kb, rag, shutdown)
}

async fn wait_for(mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("la condición no se cumplió a tiempo");
}

/// (doc_key, subtype, accesión) de los informes archivados, por clave.
async fn archived(app: &TestApp) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT doc_key, subtype, json_extract(meta, '$.accession') FROM documents WHERE doc_key LIKE 'sec:%' ORDER BY doc_key")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

fn row(key: &str, form: &str, accession: &str) -> (String, String, String) {
    (key.into(), form.into(), accession.into())
}

/// «Pasan 8 días»: los informes archivados envejecen (y se olvida cuándo se comprobaron).
async fn age_documents(app: &TestApp, days: i64) {
    let old = Utc::now().timestamp_millis() - days * 24 * 3600 * 1000;
    sqlx::query("UPDATE documents SET ingested_at = ?, meta = json_remove(meta, '$.checkedAt') WHERE doc_key LIKE 'sec:%'").bind(old).execute(&app.state.db).await.unwrap();
}

/// «Reinicio»: otra cola y otro trabajador sobre el mismo archivo; ofrece `symbol` y espera a que se procese.
async fn restart_and_offer(app: &TestApp, kb: &Arc<KnowledgeBase>, shutdown: &CancellationToken, docs: Arc<StubDocs>, symbol: &str) {
    let archive = CompanyArchive::new(kb.clone(), app.state.db.clone(), shutdown.clone());
    archive.start(docs);
    archive.sink().offer(&id(symbol));
    tokio::time::sleep(Duration::from_millis(250)).await;
}

#[tokio::test]
async fn first_stock_snapshot_archives_the_three_families_once() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, _kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    let (s, snap) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    wait_for(async || archived(&app).await.len() == 3).await;
    assert_eq!(
        archived(&app).await,
        [row("sec:AAPL:annual", "10-K", "A1"), row("sec:AAPL:earnings", "8-K", "E1"), row("sec:AAPL:quarterly", "10-Q", "Q1")]
    );

    // Metadatos del documento anual.
    let (title, source, publisher, url, language, kind, issuer): (String, String, String, String, String, String, String) =
        sqlx::query_as("SELECT title, source, publisher, url, language, kind, issuer FROM documents WHERE doc_key = 'sec:AAPL:annual'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(title, format!("Apple Inc. (AAPL) — 10-K {}", days_ago(40)));
    assert_eq!((source.as_str(), publisher.as_str(), language.as_str(), kind.as_str(), issuer.as_str()), ("sec", "SEC EDGAR", "en", "filing", "Apple Inc."));
    assert!(url.contains("sec.gov/Archives") && url.contains("A1"), "{url}");
    let (cik, index_url, ticker, checked): (i64, String, String, i64) =
        sqlx::query_as("SELECT json_extract(meta,'$.cik'), json_extract(meta,'$.indexUrl'), json_extract(meta,'$.ticker'), json_extract(meta,'$.checkedAt') FROM documents WHERE doc_key = 'sec:AAPL:annual'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!((cik, ticker.as_str()), (320193, "AAPL"));
    assert!(index_url.ends_with("-index.htm") && checked > 0);
    // Todos enlazados al activo, con las secciones por su nombre legible.
    let assets: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT asset_id FROM document_assets").fetch_all(&app.state.db).await.unwrap();
    assert_eq!(assets, [("stock:AAPL".to_string(),)]);
    let heads = |key: &str| {
        let key = key.to_string();
        let db = app.state.db.clone();
        async move {
            let r: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT c.section FROM chunks c JOIN documents d ON d.id = c.document_id WHERE d.doc_key = ? ORDER BY c.section").bind(key).fetch_all(&db).await.unwrap();
            r.into_iter().map(|r| r.0).collect::<Vec<_>>()
        }
    };
    assert_eq!(heads("sec:AAPL:annual").await, ["Business", "Management's Discussion and Analysis", "Risk Factors"]);
    assert_eq!(heads("sec:AAPL:earnings").await, ["Earnings Release"]);

    // La segunda vez (misma sesión) no vuelve a pedirlo.
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(docs.calls(), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn a_new_accession_replaces_the_previous_report_of_its_family() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    app.get("/api/assets/stock:AAPL/snapshot").await;
    wait_for(async || archived(&app).await.len() == 3).await;

    // Una semana después hay un 10-K nuevo; el 10-Q y la nota siguen siendo los mismos.
    age_documents(&app, 8).await;
    let before = Utc::now().timestamp_millis();
    let docs2 = StubDocs::new(Reply::Reports(vec![annual("A2"), quarterly("Q1"), earnings("E1", 5)]));
    restart_and_offer(&app, &kb, &shutdown, docs2.clone(), "stock:AAPL").await;
    assert_eq!(docs2.calls(), 1);
    assert_eq!(
        archived(&app).await,
        [row("sec:AAPL:annual", "10-K", "A2"), row("sec:AAPL:earnings", "8-K", "E1"), row("sec:AAPL:quarterly", "10-Q", "Q1")],
        "sigue habiendo un documento por familia"
    );
    let old_text: i64 = sqlx::query_scalar("SELECT count(*) FROM chunks WHERE text LIKE '%informe A1%'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(old_text, 0, "el texto del 10-K anterior desapareció");
    // Lo que no cambió también se marca como comprobado (así no se vuelve a descargar en 7 días).
    let checked: i64 = sqlx::query_scalar("SELECT json_extract(meta,'$.checkedAt') FROM documents WHERE doc_key = 'sec:AAPL:quarterly'").fetch_one(&app.state.db).await.unwrap();
    assert!(checked >= before, "checkedAt refrescado");
    shutdown.cancel();
}

/// Fecha de presentación de un informe ya archivado (`published_at`), para envejecerlo sin tocar nada más.
async fn set_published(app: &TestApp, key: &str, days: i64) {
    let ms = Utc::now().timestamp_millis() - days * 24 * 3600 * 1000;
    sqlx::query("UPDATE documents SET published_at = ? WHERE doc_key = ?").bind(ms).bind(key).execute(&app.state.db).await.unwrap();
}

/// Archiva 10-K (hace 40 días), 10-Q (hace 10) y nota de resultados (hace 5) y deja pasar 8 días.
async fn archived_three(docs: Arc<StubDocs>) -> (TestApp, Arc<KnowledgeBase>, CancellationToken) {
    let (app, kb, _rag, shutdown) = app_with_companies(docs).await;
    app.get("/api/assets/stock:AAPL/snapshot").await;
    wait_for(async || archived(&app).await.len() == 3).await;
    age_documents(&app, 8).await;
    (app, kb, shutdown)
}

const THREE: [(&str, &str, &str); 3] = [("sec:AAPL:annual", "10-K", "A1"), ("sec:AAPL:earnings", "8-K", "E1"), ("sec:AAPL:quarterly", "10-Q", "Q1")];

fn three() -> Vec<(String, String, String)> {
    THREE.iter().map(|(k, f, a)| row(k, f, a)).collect()
}

#[tokio::test]
async fn absence_alone_never_deletes_an_archived_report() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, kb, shutdown) = archived_three(docs).await;

    // Un `Ok` incompleto (p. ej. un cambio de CIK que esconde el 10-K, o un JSON degradado) no borra nada.
    let d2 = StubDocs::new(Reply::Reports(vec![quarterly("Q1")]));
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(d2.calls(), 1);
    assert_eq!(archived(&app).await, three(), "falta el 10-K y la nota en la respuesta, pero nada indica que estén anticuados");

    // Ni siquiera con la respuesta vacía.
    age_documents(&app, 8).await;
    let d3 = StubDocs::new(Reply::Reports(vec![]));
    restart_and_offer(&app, &kb, &shutdown, d3.clone(), "stock:AAPL").await;
    assert_eq!(d3.calls(), 1);
    assert_eq!(archived(&app).await, three());

    // Con error, tampoco.
    age_documents(&app, 8).await;
    let d4 = StubDocs::new(Reply::Fail);
    restart_and_offer(&app, &kb, &shutdown, d4.clone(), "stock:AAPL").await;
    assert_eq!(d4.calls(), 1);
    assert_eq!(archived(&app).await, three());
    shutdown.cancel();
}

#[tokio::test]
async fn a_newer_annual_filing_deletes_the_older_quarterly_report_and_only_that() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, kb, shutdown) = archived_three(docs).await;

    // Un 10-K presentado ANTES que el 10-Q archivado (hace 40 días frente a hace 10) no lo deja anticuado.
    let d2 = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(archived(&app).await, three());

    // Un 10-K nuevo, posterior al 10-Q: el 10-Q se borra (con sus fragmentos). La nota de hace 5 días se queda.
    age_documents(&app, 8).await;
    let d3 = StubDocs::new(Reply::Reports(vec![report("10-K", "A2", 2, &["risk_factors", "mdna", "business"])]));
    restart_and_offer(&app, &kb, &shutdown, d3.clone(), "stock:AAPL").await;
    assert_eq!(d3.calls(), 1);
    assert_eq!(archived(&app).await, [row("sec:AAPL:annual", "10-K", "A2"), row("sec:AAPL:earnings", "8-K", "E1")]);
    let orphans: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM chunks c LEFT JOIN documents d ON d.id = c.document_id WHERE d.id IS NULL) + (SELECT count(*) FROM chunks WHERE text LIKE '%informe Q1%')")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(orphans, 0, "sin fragmentos del documento borrado");
    shutdown.cancel();
}

#[tokio::test]
async fn an_earnings_note_older_than_120_days_is_deleted_unless_a_new_one_replaces_it() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, kb, shutdown) = archived_three(docs).await;
    // La nota archivada cumple 130 días según su propia fecha.
    set_published(&app, "sec:AAPL:earnings", 130).await;

    // Con error no se borra nada.
    let d2 = StubDocs::new(Reply::Fail);
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(archived(&app).await, three());

    // Una nota nueva la sustituye (no se borra, se actualiza).
    age_documents(&app, 8).await;
    let d3 = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E2", 3)]));
    restart_and_offer(&app, &kb, &shutdown, d3.clone(), "stock:AAPL").await;
    assert_eq!(archived(&app).await, [row("sec:AAPL:annual", "10-K", "A1"), row("sec:AAPL:earnings", "8-K", "E2"), row("sec:AAPL:quarterly", "10-Q", "Q1")]);

    // Sin nota nueva en la respuesta (o con otra de más de 120 días, que no se archiva), la anticuada se borra.
    set_published(&app, "sec:AAPL:earnings", 130).await;
    age_documents(&app, 8).await;
    let d4 = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E3", 125)]));
    restart_and_offer(&app, &kb, &shutdown, d4.clone(), "stock:AAPL").await;
    assert_eq!(d4.calls(), 1);
    assert_eq!(archived(&app).await, [row("sec:AAPL:annual", "10-K", "A1"), row("sec:AAPL:quarterly", "10-Q", "Q1")]);

    // El anual no caduca nunca por ausencia, ni aunque sea viejo.
    set_published(&app, "sec:AAPL:annual", 900).await;
    age_documents(&app, 8).await;
    let d5 = StubDocs::new(Reply::Reports(vec![]));
    restart_and_offer(&app, &kb, &shutdown, d5.clone(), "stock:AAPL").await;
    assert_eq!(d5.calls(), 1);
    assert_eq!(archived(&app).await, [row("sec:AAPL:annual", "10-K", "A1"), row("sec:AAPL:quarterly", "10-Q", "Q1")]);
    shutdown.cancel();
}

#[tokio::test]
async fn a_second_share_class_that_dedupes_into_the_first_is_not_downloaded_again() {
    // GOOG y GOOGL presentan el mismo 10-K: el de GOOG sale `Duplicate` (mismo contenido) y solo se enlaza al
    // documento de GOOGL; nunca existe `sec:GOOG:*`. La edad para refrescar tiene que mirar el activo enlazado.
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    let (app, kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    restart_and_offer(&app, &kb, &shutdown, docs.clone(), "stock:GOOGL").await;
    assert_eq!(archived(&app).await.len(), 3);

    let d2 = StubDocs::new(Reply::Reports(vec![annual("A1"), quarterly("Q1"), earnings("E1", 5)]));
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:GOOG").await;
    assert_eq!(d2.calls(), 1, "la primera vez sí se descarga");
    let keys: Vec<String> = archived(&app).await.into_iter().map(|r| r.0).collect();
    assert!(keys.iter().all(|k| k.starts_with("sec:GOOGL:")) && keys.len() == 3, "no hay sec:GOOG:*: {keys:?}");
    let linked: i64 = sqlx::query_scalar("SELECT count(*) FROM document_assets WHERE asset_id = 'stock:GOOG'").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(linked, 3, "los tres documentos quedaron enlazados también a GOOG");

    // Reinicio y de nuevo GOOG: archivado hace segundos, no se vuelve a descargar.
    let d3 = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    restart_and_offer(&app, &kb, &shutdown, d3.clone(), "stock:GOOG").await;
    assert_eq!(d3.calls(), 0, "enlazado y reciente");
    // A los 8 días, sí.
    age_documents(&app, 8).await;
    restart_and_offer(&app, &kb, &shutdown, d3.clone(), "stock:GOOG").await;
    assert_eq!(d3.calls(), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn the_latest_report_of_a_family_wins_and_unknown_forms_are_ignored() {
    let mut amendment = report("10-K/A", "A1-bis", 3, &["risk_factors"]);
    amendment.filed_on = days_ago(3);
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1"), amendment, report("S-1", "S1", 1, &["risk_factors"])]));
    let (app, _kb, _rag, shutdown) = app_with_companies(docs).await;
    app.get("/api/assets/stock:AAPL/snapshot").await;
    wait_for(async || !archived(&app).await.is_empty()).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(archived(&app).await, [row("sec:AAPL:annual", "10-K/A", "A1-bis")]);
    shutdown.cancel();
}

#[tokio::test]
async fn a_company_is_not_downloaded_again_within_7_days_not_even_after_a_restart() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    let (app, kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    app.get("/api/assets/stock:AAPL/snapshot").await;
    wait_for(async || archived(&app).await.len() == 1).await;
    assert_eq!(docs.calls(), 1);

    // «Reinicio» con lo archivado hace segundos: no se pide nada.
    let d2 = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(d2.calls(), 0, "archivado hace menos de 7 días");

    // A los 6 días tampoco; a los 8, sí.
    age_documents(&app, 6).await;
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(d2.calls(), 0, "6 días: todavía no");
    age_documents(&app, 8).await;
    restart_and_offer(&app, &kb, &shutdown, d2.clone(), "stock:AAPL").await;
    assert_eq!(d2.calls(), 1, "8 días: se refresca");
    shutdown.cancel();
}

#[tokio::test]
async fn only_stocks_trigger_a_company_download() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    let (app, kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    for path in ["etf:SPY", "crypto:BTC", "index:^GSPC", "forex:EURUSD=X"] {
        let (s, _) = app.get(&format!("/api/assets/{path}/snapshot")).await;
        assert_eq!(s, StatusCode::OK, "{path}");
    }
    // Y directamente en la cola: un ETF o una cripto ni se encolan.
    let archive = CompanyArchive::new(kb.clone(), app.state.db.clone(), shutdown.clone());
    archive.start(docs.clone());
    archive.sink().offer(&id("etf:SPY"));
    archive.sink().offer(&id("crypto:BTC"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(docs.calls(), 0);
    assert!(archived(&app).await.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn without_rag_archive_nothing_is_requested() {
    // RAG_ARCHIVE=0 con recuperación: el archivo se abre de solo lectura, sin decoradores ni colas.
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    let modes = RagModes { archive: false, analysis: true, chat: false };
    let (app, _kb, rag, shutdown) = app_with_modes(docs.clone(), modes).await;
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(docs.calls(), 0);
    assert!(archived(&app).await.is_empty());
    // Mismos objetos: ni TeeMarket ni cola.
    let market: Arc<dyn MarketData> = app.state.market.clone();
    let ipo: Arc<dyn IpoData> = app.state.ipo.clone();
    let analyst = app.state.analyst.clone();
    let (m2, i2, a2) = rag.wrap_with_docs(market.clone(), ipo.clone(), analyst.clone(), Arc::new(NoFunds), docs.clone());
    assert!(std::ptr::addr_eq(Arc::as_ptr(&m2), Arc::as_ptr(&market)));
    assert!(std::ptr::addr_eq(Arc::as_ptr(&i2), Arc::as_ptr(&ipo)));
    assert!(std::ptr::addr_eq(Arc::as_ptr(&a2), Arc::as_ptr(&analyst)));
    shutdown.cancel();
}

#[tokio::test]
async fn without_rag_at_all_nothing_is_requested() {
    let docs = StubDocs::new(Reply::Reports(vec![annual("A1")]));
    let app = TestApp::new().await;
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(docs.calls(), 0);
    assert!(app.state.knowledge.is_none());
}

#[tokio::test]
async fn a_failing_source_breaks_nothing_and_is_not_retried_until_6_hours_pass() {
    let docs = StubDocs::new(Reply::Fail);
    let (app, kb, _rag, shutdown) = app_with_companies(docs.clone()).await;
    let (s, snap) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    wait_for(async || docs.calls() == 1).await;
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(docs.calls(), 1, "el fallo tampoco se reintenta de inmediato");
    assert!(archived(&app).await.is_empty());
    // El fallo no dejó nada raro: con la fuente recuperada (otra cola) se archiva.
    docs.set(Reply::Reports(vec![annual("A1")]));
    restart_and_offer(&app, &kb, &shutdown, docs.clone(), "stock:AAPL").await;
    assert_eq!(archived(&app).await.len(), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn a_slow_download_never_delays_the_answer() {
    let docs = StubDocs::slow(Reply::Reports(vec![annual("A1")]), Duration::from_secs(5));
    let (app, _kb, _rag, shutdown) = app_with_companies(docs).await;
    let t = Instant::now();
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    assert!(t.elapsed() < Duration::from_secs(2), "la respuesta esperó a los informes: {:?}", t.elapsed());
    shutdown.cancel();
}

#[test]
fn document_has_readable_headings_skips_subsections_and_keys_by_family() {
    let mut p = annual("A1");
    p.issuer = "The Coca-Cola Company".into();
    p.sections = vec![
        section("risk_factors", "riesgos", None),
        section("mdna", "análisis de la dirección", None),
        section("market_risk", "riesgo de mercado", Some("mdna")),
        section("operating_review", "revisión operativa", None),
        section("otra", "texto sin clave conocida", None),
        section("vacia", "   ", None),
    ];
    let doc = company_report_to_document(&p, &id("stock:KO")).expect("documento");
    assert_eq!((doc.doc_key.as_str(), doc.kind, doc.subtype.as_deref(), doc.source.as_str()), ("sec:KO:annual", DocKind::Filing, Some("10-K"), "sec"));
    assert_eq!(doc.title, format!("The Coca-Cola Company (KO) — 10-K {}", days_ago(40)));
    assert_eq!(doc.assets, vec!["stock:KO".to_string()]);
    assert_eq!((doc.publisher.as_deref(), doc.language.as_deref(), doc.issuer.as_deref()), (Some("SEC EDGAR"), Some("en"), Some("The Coca-Cola Company")));
    let heads: Vec<_> = doc.sections.iter().map(|s| s.heading.clone().unwrap()).collect();
    assert_eq!(heads, ["Risk Factors", "Management's Discussion and Analysis", "Operating and Financial Review and Prospects", "título de otra"]);
    for k in ["cik", "accession", "indexUrl", "ticker", "missing", "checkedAt"] {
        assert!(doc.meta.get(k).is_some(), "falta meta.{k}: {}", doc.meta);
    }

    // Familia por formulario.
    for (form, key) in [("10-K/A", "sec:KO:annual"), ("20-F", "sec:KO:annual"), ("40-F", "sec:KO:annual"), ("10-Q", "sec:KO:quarterly"), ("8-K", "sec:KO:earnings")] {
        let mut p = quarterly("Q1");
        p.form = form.into();
        assert_eq!(company_report_to_document(&p, &id("stock:KO")).map(|d| d.doc_key).as_deref(), Some(key), "{form}");
    }
    // Sin familia conocida, sin secciones o sin accesión no hay documento.
    let mut p = quarterly("Q1");
    p.form = "S-1".into();
    assert!(company_report_to_document(&p, &id("stock:KO")).is_none());
    let mut p = quarterly("Q1");
    p.sections.clear();
    assert!(company_report_to_document(&p, &id("stock:KO")).is_none());
    let mut p = quarterly("Q1");
    p.accession = " ".into();
    assert!(company_report_to_document(&p, &id("stock:KO")).is_none());
}
