//! Filtro de entidad al archivar noticias (`TeeMarket`): las de un activo solo se archivan enlazadas
//! a él si su título o resumen lo nombran; lo que recibe el llamador no cambia. Sin red ni GPU.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ai::fixture::{self, FakeMarket};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use domain::{
    AssetId, AssetSnapshot, AssetSummary, Candle, Fundamentals, Indicators, Interval, MacroContext, MarketData, NewsItem, PriceTick, ProviderStatus,
    Quote, Range,
};
use futures::stream::BoxStream;
use knowledge::{FakeEmbedder, KnowledgeConfig};
use server::{
    db,
    fake::{FakeAnalyst, FakeIpo},
    knowledge::{Rag, open_with},
};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

/// `NewsItem` no implementa `PartialEq`: se compara por su JSON.
fn json(items: &[NewsItem]) -> serde_json::Value {
    serde_json::to_value(items).unwrap()
}

fn ko() -> AssetId {
    "stock:KO".parse().unwrap()
}

fn item(id: &str, title: &str, summary: Option<&str>, tickers: &[&str]) -> NewsItem {
    NewsItem {
        id: id.into(),
        title: title.into(),
        summary: summary.map(String::from),
        url: format!("https://news.example.com/{id}"),
        publisher: Some("Wire".into()),
        published_at: Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
        image_url: None,
        tickers: tickers.iter().map(|t| t.to_string()).collect(),
        sentiment: None,
        provider: "finnhub".into(),
        language: Some("en".into()),
    }
}

/// Lo que Finnhub devuelve en el snapshot de KO: etiquetado con el símbolo aunque hable de otras empresas.
fn snapshot_news() -> Vec<NewsItem> {
    vec![
        item("k1", "Costco and Kraft Heinz post strong quarterly sales", Some("Consumer staples lead the market higher."), &["KO"]),
        item("k2", "Coca-Cola lifts its full-year outlook as volumes grow", Some("The beverage maker beat estimates."), &["KO"]),
        item("k3", "Beverage maker beats estimates on sparkling volumes", Some("Coca-Cola said unit case volume rose 3 percent."), &["KO"]),
        item("k4", "PepsiCo and Mondelez cut their forecasts", Some("Snack makers warn about weak demand."), &["KO", "PEP"]),
    ]
}

/// Lo que devuelve `news(KO)` en otra consulta (distintos ids, para que el archivo no los dé por vistos).
fn later_news() -> Vec<NewsItem> {
    vec![
        item("n1", "Walmart and Target report strong holiday traffic", None, &["KO"]),
        item("n2", "Coca-Cola names a new finance chief", None, &["KO"]),
    ]
}

/// Mercado de prueba: el snapshot de KO sale del fixture y `news` devuelve `later_news`. Con `degraded`, el
/// snapshot trae el símbolo como nombre (así lo rellena `market` cuando falla la ficha).
struct Scripted(FakeMarket, Arc<AtomicBool>);

#[async_trait]
impl MarketData for Scripted {
    async fn search(&self, query: &str, limit: usize) -> domain::Result<Vec<AssetSummary>> {
        self.0.search(query, limit).await
    }
    async fn quote(&self, id: &AssetId) -> domain::Result<Quote> {
        self.0.quote(id).await
    }
    async fn quotes(&self, ids: &[AssetId]) -> domain::Result<Vec<Quote>> {
        self.0.quotes(ids).await
    }
    async fn history(&self, id: &AssetId, range: Range, interval: Interval) -> domain::Result<Vec<Candle>> {
        self.0.history(id, range, interval).await
    }
    async fn fundamentals(&self, id: &AssetId) -> domain::Result<Option<Fundamentals>> {
        self.0.fundamentals(id).await
    }
    async fn indicators(&self, id: &AssetId) -> domain::Result<Indicators> {
        self.0.indicators(id).await
    }
    async fn news(&self, id: Option<&AssetId>, _limit: usize) -> domain::Result<Vec<NewsItem>> {
        Ok(if id.is_some() { later_news() } else { vec![item("g1", "Stocks close higher as yields ease", None, &[]), item("g2", "Oil slips on demand worries", None, &[])] })
    }
    async fn macro_context(&self) -> domain::Result<MacroContext> {
        self.0.macro_context().await
    }
    async fn snapshot(&self, id: &AssetId) -> domain::Result<AssetSnapshot> {
        let mut s = self.0.snapshot(id).await?;
        if self.1.load(Ordering::SeqCst) {
            s.summary.name = s.summary.symbol.clone();
        }
        Ok(s)
    }
    fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        self.0.subscribe(ids)
    }
    fn providers_status(&self) -> Vec<ProviderStatus> {
        self.0.providers_status()
    }
}

fn ko_snapshot(name: &str) -> AssetSnapshot {
    let mut s = fixture::stock_snapshot();
    s.summary.id = ko();
    s.summary.symbol = "KO".into();
    s.summary.name = name.into();
    s.news = snapshot_news();
    s
}

struct Rig {
    rag: Rag,
    pool: SqlitePool,
    market: Arc<dyn MarketData>,
    /// Activa el snapshot con el símbolo por nombre.
    degraded: Arc<AtomicBool>,
    shutdown: CancellationToken,
}

async fn rig() -> Rig {
    rig_named("The Coca-Cola Company").await
}

/// `name` es el nombre que trae la ficha del activo en el snapshot.
async fn rig_named(name: &str) -> Rig {
    let pool = db::connect_memory().await.expect("BD en memoria");
    let shutdown = CancellationToken::new();
    let rag = open_with(KnowledgeConfig::default(), pool.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.expect("archivo abierto");
    let degraded = Arc::new(AtomicBool::new(false));
    let raw: Arc<dyn MarketData> = Arc::new(Scripted(FakeMarket::new().with_snapshot(ko_snapshot(name)), degraded.clone()));
    let (market, _, _) = rag.wrap(raw, Arc::new(FakeIpo::default()), Arc::new(FakeAnalyst::default()));
    Rig { rag, pool, market, degraded, shutdown }
}

/// (título, activos enlazados) de lo archivado, esperando a que haya al menos `expected` documentos y un margen para lo que no debería llegar.
async fn archived(rig: &Rig, expected: usize) -> Vec<(String, Option<String>)> {
    for _ in 0..200 {
        if rig.rag.kb.stats().await.unwrap().documents >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    sqlx::query_as("SELECT d.title, (SELECT group_concat(asset_id) FROM document_assets a WHERE a.document_id = d.id) FROM documents d ORDER BY d.title")
        .fetch_all(&rig.pool)
        .await
        .unwrap()
}

fn titles(docs: &[(String, Option<String>)]) -> Vec<&str> {
    docs.iter().map(|d| d.0.as_str()).collect()
}

#[tokio::test]
async fn the_snapshot_archives_only_the_news_that_name_the_asset_and_returns_everything() {
    let rig = rig().await;
    let s = rig.market.snapshot(&ko()).await.unwrap();
    // Al llamador le llega todo lo que dio el proveedor, en el mismo orden.
    assert_eq!(json(&s.news), json(&snapshot_news()), "el filtro solo afecta a lo que se archiva");

    let docs = archived(&rig, 2).await;
    let t = titles(&docs);
    assert_eq!(docs.len(), 2, "{t:?}");
    assert!(t.iter().any(|t| t.contains("lifts its full-year outlook")), "nombra a Coca-Cola en el título: {t:?}");
    assert!(t.iter().any(|t| t.contains("sparkling volumes")), "la nombra solo en el resumen: {t:?}");
    assert!(!t.iter().any(|t| t.contains("Costco") || t.contains("PepsiCo")), "etiquetadas KO pero hablan de otras empresas: {t:?}");
    assert!(docs.iter().all(|d| d.1.as_deref() == Some("stock:KO")), "las archivadas se enlazan a KO: {docs:?}");
    rig.shutdown.cancel();
}

#[tokio::test]
async fn news_filters_with_the_name_seen_in_the_snapshot_and_links_everything_without_it() {
    // Sin snapshot previo no se sabe el nombre: se enlazan todas, como antes.
    let rig1 = rig().await;
    assert_eq!(json(&rig1.market.news(Some(&ko()), 10).await.unwrap()), json(&later_news()));
    let docs = archived(&rig1, 2).await;
    assert_eq!(docs.len(), 2, "sin nombre conocido: todas, también la de Walmart: {:?}", titles(&docs));
    rig1.shutdown.cancel();

    // Con el nombre recordado de un snapshot, `news` filtra igual, y devuelve todo.
    let rig2 = rig().await;
    rig2.market.snapshot(&ko()).await.unwrap();
    assert_eq!(archived(&rig2, 2).await.len(), 2);
    assert_eq!(json(&rig2.market.news(Some(&ko()), 10).await.unwrap()), json(&later_news()), "al llamador, todo");
    let docs = archived(&rig2, 3).await;
    let t = titles(&docs);
    assert_eq!(docs.len(), 3, "{t:?}");
    assert!(t.iter().any(|t| t.contains("new finance chief")), "{t:?}");
    assert!(!t.iter().any(|t| t.contains("Walmart")), "{t:?}");
    rig2.shutdown.cancel();
}

#[tokio::test]
async fn general_news_are_archived_unchanged() {
    let rig = rig().await;
    assert_eq!(rig.market.news(None, 10).await.unwrap().len(), 2);
    let docs = archived(&rig, 2).await;
    assert_eq!(docs.len(), 2, "sin activo no se filtra nada: {:?}", titles(&docs));
    assert!(docs.iter().all(|d| d.1.is_none()), "y siguen siendo generales: {docs:?}");
    rig.shutdown.cancel();
}

#[tokio::test]
async fn a_name_equal_to_the_symbol_counts_as_unknown_and_links_everything() {
    // Cuando falla la ficha, `market` pone el símbolo como nombre: con eso el filtro tiraría casi todo.
    for name in ["KO", "ko", " Ko "] {
        let rig = rig_named(name).await;
        let s = rig.market.snapshot(&ko()).await.unwrap();
        assert_eq!(json(&s.news), json(&snapshot_news()), "{name:?}: al llamador, todo");
        let docs = archived(&rig, 4).await;
        assert_eq!(docs.len(), 4, "{name:?}: sin nombre real se enlazan todas, también las de Costco y PepsiCo: {:?}", titles(&docs));
        assert!(docs.iter().all(|d| d.1.as_deref() == Some("stock:KO")), "{docs:?}");

        // Y el símbolo no se recuerda como nombre: `news` tampoco filtra con él.
        assert_eq!(rig.market.news(Some(&ko()), 10).await.unwrap().len(), 2);
        let docs = archived(&rig, 6).await;
        assert_eq!(docs.len(), 6, "{name:?}: {:?}", titles(&docs));
        rig.shutdown.cancel();
    }
}

#[tokio::test]
async fn a_real_name_is_not_overwritten_by_a_later_snapshot_whose_name_is_the_symbol() {
    let rig = rig().await;
    rig.market.snapshot(&ko()).await.unwrap();
    assert_eq!(archived(&rig, 2).await.len(), 2);
    // La ficha cae: el snapshot trae «KO» por nombre. Lo recordado (el nombre bueno) se conserva...
    rig.degraded.store(true, Ordering::SeqCst);
    rig.market.snapshot(&ko()).await.unwrap();
    let before = archived(&rig, 2).await.len();
    // El propio snapshot degradado también filtra con el nombre recordado: no entran sus noticias ajenas.
    assert_eq!(before, 2, "el snapshot sin nombre no enlaza las noticias que no nombran a Coca-Cola");
    // ...y `news` sigue filtrando con él: Walmart fuera, el nuevo director financiero de Coca-Cola dentro.
    rig.market.news(Some(&ko()), 10).await.unwrap();
    let docs = archived(&rig, before + 1).await;
    let t = titles(&docs);
    assert!(t.iter().any(|t| t.contains("new finance chief")), "{t:?}");
    assert!(!t.iter().any(|t| t.contains("Walmart")), "{t:?}");
    rig.shutdown.cancel();
}
