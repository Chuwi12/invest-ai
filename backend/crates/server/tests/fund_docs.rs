//! Folleto resumido de ETF en el RAG: se archiva en segundo plano la primera vez que se pide el
//! snapshot, como mucho cada 30 días, sin retrasar la respuesta y solo con RAG. Embedder simulado;
//! el folleto sale de un 497K REAL de iShares (TLT) procesado con el extractor de `market`.

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::NaiveDate;
use common::TestApp;
use domain::{AssetId, FundDocs, Prospectus};
use knowledge::{DocKind, FakeEmbedder, KnowledgeBase, KnowledgeConfig, SearchRequest};
use server::{
    fund_docs::{FundArchive, doc_key, fund_prospectus_to_document},
    knowledge::{Rag, open_with},
};
use tokio_util::sync::CancellationToken;

const TLT_HTML: &str = include_str!("../../market/tests/fixtures/sec_fund_497k_tlt.htm");

fn tlt_prospectus() -> Prospectus {
    let ex = market::fund_prospectus::extract(TLT_HTML, "TLT", 6000);
    Prospectus {
        issuer: ex.fund_name.clone().unwrap(),
        cik: 1100663,
        ticker: Some("TLT".into()),
        form: "497K".into(),
        preliminary: false,
        filed_on: NaiveDate::from_ymd_opt(2026, 6, 29).unwrap(),
        accession: "0001193125-26-287948".into(),
        url: "https://www.sec.gov/Archives/edgar/data/1100663/000119312526287948/d128154d497k.htm".into(),
        index_url: "https://www.sec.gov/Archives/edgar/data/1100663/000119312526287948/0001193125-26-287948-index.htm".into(),
        offer_price: None,
        sections: ex.sections,
        missing: ex.missing,
        document_chars: ex.document_chars,
        source: "sec".into(),
        fetched_at: chrono::Utc::now(),
    }
}

/// Fuente de folletos de prueba: cuenta las llamadas y puede tardar o fallar.
struct StubDocs {
    calls: AtomicUsize,
    delay: Duration,
    fail: bool,
}

impl StubDocs {
    fn new(delay: Duration, fail: bool) -> Arc<Self> {
        Arc::new(Self { calls: AtomicUsize::new(0), delay, fail })
    }
}

#[async_trait]
impl FundDocs for StubDocs {
    async fn fund_prospectus(&self, _id: &AssetId) -> domain::Result<Prospectus> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        if self.fail { Err(domain::DomainError::NotFound("sin folleto".into())) } else { Ok(tlt_prospectus()) }
    }
}

async fn app_with_funds(docs: Arc<StubDocs>) -> (TestApp, Arc<KnowledgeBase>, Rag, CancellationToken) {
    let mut app = TestApp::new().await;
    let shutdown = CancellationToken::new();
    let rag = open_with(KnowledgeConfig::default(), app.state.db.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.expect("archivo abierto");
    let kb = rag.kb.clone();
    let (market, ipo, analyst) = rag.wrap_with_funds(app.state.market.clone(), app.state.ipo.clone(), app.state.analyst.clone(), docs);
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

async fn etf_docs(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM documents WHERE doc_key LIKE 'etf:%'").fetch_one(&app.state.db).await.unwrap()
}

fn id(s: &str) -> AssetId {
    s.parse().unwrap()
}

#[tokio::test]
async fn first_etf_snapshot_archives_its_prospectus_once() {
    let docs = StubDocs::new(Duration::ZERO, false);
    let (app, kb, _rag, shutdown) = app_with_funds(docs.clone()).await;
    let (s, snap) = app.get("/api/assets/etf:SPY/snapshot").await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    wait_for(async || etf_docs(&app).await == 1).await;
    // La segunda vez (misma sesión) no vuelve a pedirlo.
    let (s, _) = app.get("/api/assets/etf:SPY/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(docs.calls.load(Ordering::SeqCst), 1);
    let r = kb.search(&SearchRequest { kinds: vec![DocKind::Filing], k: 5, include_general: true, ..SearchRequest::new("investment objective of the fund") }).await.unwrap();
    let h = r.hits.first().expect("hay resultados");
    assert_eq!((h.doc.kind, h.doc.source.as_str(), h.doc.subtype.as_deref()), (DocKind::Filing, "sec", Some("497K")));
    assert!(h.doc.url.as_deref().is_some_and(|u| u.contains("sec.gov/Archives")), "{:?}", h.doc.url);
    shutdown.cancel();
}

#[tokio::test]
async fn stocks_do_not_trigger_a_fund_download() {
    let docs = StubDocs::new(Duration::ZERO, false);
    let (app, kb, _rag, shutdown) = app_with_funds(docs.clone()).await;
    let (s, _) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(docs.calls.load(Ordering::SeqCst), 0);
    assert!(kb.search(&SearchRequest { kinds: vec![DocKind::Filing], k: 5, include_general: true, ..SearchRequest::new("objective") }).await.unwrap().hits.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn a_slow_download_never_delays_the_answer() {
    let docs = StubDocs::new(Duration::from_secs(5), false);
    let (app, _kb, _rag, shutdown) = app_with_funds(docs.clone()).await;
    let t = Instant::now();
    let (s, _) = app.get("/api/assets/etf:SPY/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    assert!(t.elapsed() < Duration::from_secs(2), "la respuesta esperó al folleto: {:?}", t.elapsed());
    shutdown.cancel();
}

#[tokio::test]
async fn a_failed_download_archives_nothing_and_the_answer_is_normal() {
    let docs = StubDocs::new(Duration::ZERO, true);
    let (app, _kb, _rag, shutdown) = app_with_funds(docs.clone()).await;
    let (s, snap) = app.get("/api/assets/etf:SPY/snapshot").await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    wait_for(async || docs.calls.load(Ordering::SeqCst) == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(etf_docs(&app).await, 0);
    shutdown.cancel();
}

#[tokio::test]
async fn a_prospectus_archived_less_than_30_days_ago_is_not_downloaded_again_even_after_a_restart() {
    let docs = StubDocs::new(Duration::ZERO, false);
    let (app, kb, rag, shutdown) = app_with_funds(docs.clone()).await;
    app.get("/api/assets/etf:SPY/snapshot").await;
    wait_for(async || etf_docs(&app).await == 1).await;
    // «Reinicio»: otra cola y otro trabajador sobre el mismo archivo.
    let docs2 = StubDocs::new(Duration::ZERO, false);
    let archive = FundArchive::new(kb.clone(), app.state.db.clone(), shutdown.clone());
    archive.start(docs2.clone());
    archive.sink().offer(&id("etf:SPY"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(docs2.calls.load(Ordering::SeqCst), 0, "ya está archivado y es reciente");
    // Con el documento envejecido a 31 días sí se refresca.
    let old = chrono::Utc::now().timestamp_millis() - 31 * 24 * 3600 * 1000;
    sqlx::query("UPDATE documents SET ingested_at = ?, meta = '{}' WHERE doc_key = ?").bind(old).bind(doc_key(&id("etf:SPY"))).execute(&app.state.db).await.unwrap();
    let archive = FundArchive::new(kb.clone(), app.state.db.clone(), shutdown.clone());
    archive.start(docs2.clone());
    archive.sink().offer(&id("etf:SPY"));
    wait_for(async || docs2.calls.load(Ordering::SeqCst) == 1).await;
    drop(rag);
    shutdown.cancel();
}

#[tokio::test]
async fn without_rag_wiring_nothing_is_requested() {
    // `wrap` (sin `FundDocs`) y la app sin RAG: ni cola ni descargas.
    let docs = StubDocs::new(Duration::ZERO, false);
    let app = TestApp::new().await;
    let (s, _) = app.get("/api/assets/etf:SPY/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(docs.calls.load(Ordering::SeqCst), 0);
    assert!(app.state.knowledge.is_none());
}

#[test]
fn document_has_the_five_sections_with_standard_headings_and_the_etf_asset() {
    let doc = fund_prospectus_to_document(&tlt_prospectus(), &id("etf:TLT")).expect("documento");
    assert_eq!((doc.doc_key.as_str(), doc.kind, doc.subtype.as_deref()), ("etf:TLT", DocKind::Filing, Some("497K")));
    assert_eq!(doc.assets, vec!["etf:TLT".to_string()]);
    let heads: Vec<_> = doc.sections.iter().map(|s| s.heading.clone().unwrap()).collect();
    assert_eq!(heads, ["Investment Objective", "Fees and Expenses", "Principal Investment Strategies", "Principal Risks", "Portfolio Turnover"], "{heads:?}");
    assert!(doc.title.contains("(TLT)") && doc.meta["accession"] == "0001193125-26-287948" && doc.language.as_deref() == Some("en"));
    // Sin secciones o sin accesión no hay documento.
    let mut p = tlt_prospectus();
    p.sections.clear();
    assert!(fund_prospectus_to_document(&p, &id("etf:TLT")).is_none());
    let mut p = tlt_prospectus();
    p.accession = " ".into();
    assert!(fund_prospectus_to_document(&p, &id("etf:TLT")).is_none());
}
