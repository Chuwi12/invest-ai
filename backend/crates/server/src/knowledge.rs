//! Cableado del archivo de conocimiento local (RAG, crate `knowledge`). Diseño en `docs/notes/rag.md`.
//!
//! Todo es opcional y **nunca impide arrancar**: con `RAG_ENABLED=0` (valor por defecto) o con
//! `INVEST_AI_FAKE=1` no se crea nada y la app es idéntica a la que no conoce el RAG; si abrir el
//! archivo falla, se registra con `tracing::error!` y la app sigue sin RAG.
//!
//! Piezas:
//! - [`TeeMarket`] / [`TeeIpo`]: decoradores que archivan lo que la app YA descarga (noticias,
//!   folletos de OPV) sin ninguna petición extra y sin retrasar la respuesta (colas acotadas).
//! - [`Activity`] + [`TrackedAnalyst`] + [`GatedEmbedder`]: el embedder compite por la VRAM con el
//!   LLM, así que mientras haya un análisis o un chat en curso (de cualquier origen: HTTP, SSE,
//!   lista de seguimiento) NO se embeben documentos. Las consultas (`EmbedKind::Query`) sí pasan:
//!   sin ellas el propio análisis no podría recuperar nada.
//! - [`run_maintenance`]: `embed_pending` en tandas pequeñas y `purge` cada 6 h.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, Utc};
use domain::{
    Analysis, AnalysisDepth, AnalysisEvent, Analyst, AssetId, AssetSnapshot, AssetSummary, Candle, ChatEvent, ChatRequest, Fundamentals,
    Indicators, Interval, IpoBaseRate, IpoData, IpoEntry, IpoPerformance, LlmStatus, MacroContext, MarketData, NewsItem, PriceTick,
    ProviderStatus, Prospectus, Quote, Range,
};
use futures::{StreamExt, stream::BoxStream};
use knowledge::{
    DocKind, EmbedError, EmbedKind, Embedder, KnowledgeBase, KnowledgeConfig, KnowledgeSearch, NewDocument, NewsSink, OllamaEmbedder, Section,
    spawn_news_worker,
};
use sqlx::SqlitePool;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Cada cuánto se purga el archivo (retención por edad y tope de fragmentos).
pub const PURGE_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Cada cuánto se miran los fragmentos pendientes de embedding.
pub const MAINTENANCE_TICK: Duration = Duration::from_secs(120);
/// Fragmentos por tanda de `embed_pending` (pequeñas: se comparte GPU con el LLM).
pub const PENDING_BATCH: usize = 64;
const FILING_QUEUE: usize = 16;
const FILINGS_SEEN_CAP: usize = 1000;
const GATE_POLL: Duration = Duration::from_millis(500);

// ------------------------------------------------------------------------------------ actividad

/// Contador de generaciones del LLM en curso (análisis y chats).
#[derive(Clone, Default, Debug)]
pub struct Activity(Arc<AtomicUsize>);

/// Marca una generación en curso mientras viva.
#[derive(Debug)]
pub struct ActivityGuard(Arc<AtomicUsize>);

impl Activity {
    pub fn busy(&self) -> bool {
        self.0.load(Ordering::SeqCst) > 0
    }

    pub fn begin(&self) -> ActivityGuard {
        self.0.fetch_add(1, Ordering::SeqCst);
        ActivityGuard(self.0.clone())
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Decorador de `Analyst` que cuenta las generaciones en curso. Cubre todas las vías (rutas HTTP,
/// SSE, chat y lista de seguimiento) porque envuelve al analista, no a las rutas.
pub struct TrackedAnalyst {
    inner: Arc<dyn Analyst>,
    activity: Activity,
}

impl TrackedAnalyst {
    pub fn new(inner: Arc<dyn Analyst>, activity: Activity) -> Self {
        Self { inner, activity }
    }
}

#[async_trait]
impl Analyst for TrackedAnalyst {
    async fn analyze(&self, snapshot: &AssetSnapshot) -> domain::Result<Analysis> {
        let _g = self.activity.begin();
        self.inner.analyze(snapshot).await
    }

    async fn analyze_with(&self, snapshot: &AssetSnapshot, depth: Option<AnalysisDepth>) -> domain::Result<Analysis> {
        let _g = self.activity.begin();
        self.inner.analyze_with(snapshot, depth).await
    }

    fn analyze_stream(self: Arc<Self>, snapshot: AssetSnapshot, depth: Option<AnalysisDepth>) -> BoxStream<'static, AnalysisEvent> {
        let guard = self.activity.begin();
        self.inner.clone().analyze_stream(snapshot, depth).map(move |ev| {
            let _keep = &guard;
            ev
        }).boxed()
    }

    fn chat(&self, req: ChatRequest) -> BoxStream<'static, ChatEvent> {
        let guard = self.activity.begin();
        self.inner.chat(req).map(move |ev| {
            let _keep = &guard;
            ev
        }).boxed()
    }

    async fn status(&self) -> LlmStatus {
        self.inner.status().await
    }
}

/// Embedder que no calcula embeddings de DOCUMENTOS mientras hay una generación en curso.
pub struct GatedEmbedder {
    inner: Arc<dyn Embedder>,
    activity: Activity,
    shutdown: CancellationToken,
    poll: Duration,
}

impl GatedEmbedder {
    pub fn new(inner: Arc<dyn Embedder>, activity: Activity, shutdown: CancellationToken) -> Self {
        Self { inner, activity, shutdown, poll: GATE_POLL }
    }

    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }
}

#[async_trait]
impl Embedder for GatedEmbedder {
    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        if kind == EmbedKind::Document {
            // Espera (no falla): un error transitorio activaría la pausa de 30 s también para las consultas.
            while self.activity.busy() {
                tokio::select! {
                    () = self.shutdown.cancelled() => return Err(EmbedError::Unavailable("apagando".into())),
                    () = tokio::time::sleep(self.poll) => {}
                }
            }
        }
        self.inner.embed(texts, kind).await
    }
}

// ------------------------------------------------------------------------------------- mercado

/// `MarketData` que ofrece al archivo las noticias que ya devuelve, sin peticiones extra.
pub struct TeeMarket {
    inner: Arc<dyn MarketData>,
    sink: NewsSink,
    /// Cola de folletos de ETF (ver `fund_docs`): solo con RAG y un `FundDocs` cableado.
    funds: Option<crate::fund_docs::FundSink>,
}

impl TeeMarket {
    pub fn new(inner: Arc<dyn MarketData>, sink: NewsSink) -> Self {
        Self { inner, sink, funds: None }
    }

    /// Ofrece al archivo los ETF/fondos de cada snapshot para archivar su folleto resumido.
    #[must_use]
    pub fn with_funds(mut self, funds: crate::fund_docs::FundSink) -> Self {
        self.funds = Some(funds);
        self
    }
}

#[async_trait]
impl MarketData for TeeMarket {
    async fn search(&self, query: &str, limit: usize) -> domain::Result<Vec<AssetSummary>> {
        self.inner.search(query, limit).await
    }
    async fn quote(&self, id: &AssetId) -> domain::Result<Quote> {
        self.inner.quote(id).await
    }
    async fn quotes(&self, ids: &[AssetId]) -> domain::Result<Vec<Quote>> {
        self.inner.quotes(ids).await
    }
    async fn history(&self, id: &AssetId, range: Range, interval: Interval) -> domain::Result<Vec<Candle>> {
        self.inner.history(id, range, interval).await
    }
    async fn fundamentals(&self, id: &AssetId) -> domain::Result<Option<Fundamentals>> {
        self.inner.fundamentals(id).await
    }
    async fn indicators(&self, id: &AssetId) -> domain::Result<Indicators> {
        self.inner.indicators(id).await
    }
    async fn news(&self, id: Option<&AssetId>, limit: usize) -> domain::Result<Vec<NewsItem>> {
        let r = self.inner.news(id, limit).await?;
        self.sink.offer(id, &r);
        Ok(r)
    }
    async fn macro_context(&self) -> domain::Result<MacroContext> {
        self.inner.macro_context().await
    }
    async fn snapshot(&self, id: &AssetId) -> domain::Result<AssetSnapshot> {
        let s = self.inner.snapshot(id).await?;
        self.sink.offer(Some(id), &s.news);
        if let Some(funds) = &self.funds {
            funds.offer(&s.summary.id);
        }
        Ok(s)
    }
    fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        self.inner.subscribe(ids)
    }
    fn providers_status(&self) -> Vec<ProviderStatus> {
        self.inner.providers_status()
    }
}

// --------------------------------------------------------------------------------------- OPV

/// Cola de folletos de OPV. `offer` es síncrono y nunca espera.
#[derive(Clone)]
pub struct FilingSink {
    tx: mpsc::Sender<(Prospectus, String)>,
    seen: Arc<Mutex<HashSet<String>>>,
}

impl FilingSink {
    /// Ofrece un folleto recién obtenido. `requested` es lo que se pidió (símbolo), por si el folleto no trae ticker.
    pub fn offer(&self, p: &Prospectus, requested: &str) {
        let key = format!("{}|{}", p.accession, p.sections.iter().map(|s| s.text.len()).sum::<usize>());
        {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() > FILINGS_SEEN_CAP {
                seen.clear();
            }
            if !seen.insert(key.clone()) {
                return;
            }
        }
        if self.tx.try_send((p.clone(), requested.to_string())).is_err() {
            // Cola llena: se olvida para que la próxima petición lo reintente.
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        }
    }
}

/// Folleto -> documento del archivo (tipo `Filing`, una sección por sección del folleto; las
/// subsecciones, cuyo texto ya contiene el padre, no se indexan dos veces).
pub fn prospectus_to_document(p: &Prospectus, requested: &str) -> Option<NewDocument> {
    let sections: Vec<Section> = p
        .sections
        .iter()
        .filter(|s| s.parent.is_none() && !s.text.trim().is_empty())
        .map(|s| Section { heading: Some(s.title.clone()).filter(|t| !t.trim().is_empty()), text: s.text.clone() })
        .collect();
    if sections.is_empty() || p.accession.trim().is_empty() {
        return None;
    }
    let ticker = p.ticker.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| requested.to_string());
    let assets: Vec<String> = format!("stock:{}", ticker.trim().to_ascii_uppercase()).parse::<AssetId>().map(|id| vec![id.to_string()]).unwrap_or_default();
    Some(NewDocument {
        doc_key: format!("filing:{}", p.accession),
        kind: DocKind::Filing,
        subtype: Some(p.form.clone()),
        source: "sec".into(),
        publisher: Some("SEC EDGAR".into()),
        title: format!("{} {}", p.issuer, p.form),
        url: Some(p.url.clone()),
        issuer: Some(p.issuer.clone()),
        language: Some("en".into()),
        published_at: p.filed_on.and_time(NaiveTime::MIN).and_utc(),
        assets,
        sections,
        meta: serde_json::json!({ "cik": p.cik, "accession": p.accession, "preliminary": p.preliminary, "indexUrl": p.index_url, "offerPrice": p.offer_price }),
    })
}

/// `IpoData` que archiva cada folleto que obtiene (por `/api/ipo/{symbol}/prospectus` y `/risk-profile`).
pub struct TeeIpo {
    inner: Arc<dyn IpoData>,
    sink: FilingSink,
}

impl TeeIpo {
    pub fn new(inner: Arc<dyn IpoData>, sink: FilingSink) -> Self {
        Self { inner, sink }
    }
}

#[async_trait]
impl IpoData for TeeIpo {
    async fn calendar(&self, from: NaiveDate, to: NaiveDate) -> domain::Result<Vec<IpoEntry>> {
        self.inner.calendar(from, to).await
    }
    async fn prospectus(&self, issuer: &str) -> domain::Result<Prospectus> {
        let p = self.inner.prospectus(issuer).await?;
        self.sink.offer(&p, issuer);
        Ok(p)
    }
    async fn performance(&self, symbol: &str, ipo_date: NaiveDate, offer_price: Option<f64>, benchmark: &str) -> domain::Result<IpoPerformance> {
        self.inner.performance(symbol, ipo_date, offer_price, benchmark).await
    }
    async fn base_rate(&self, months: u32) -> domain::Result<IpoBaseRate> {
        self.inner.base_rate(months).await
    }
}

fn spawn_filing_worker(kb: Arc<KnowledgeBase>, shutdown: CancellationToken) -> (FilingSink, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<(Prospectus, String)>(FILING_QUEUE);
    let sink = FilingSink { tx, seen: Arc::default() };
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                item = rx.recv() => {
                    let Some((p, requested)) = item else { break };
                    let Some(doc) = prospectus_to_document(&p, &requested) else { continue };
                    match kb.ingest(doc).await {
                        Ok(r) => tracing::debug!(accession = %p.accession, outcome = ?r.outcome, chunks = r.chunks, pending = r.pending, "folleto archivado"),
                        Err(e) => tracing::warn!(accession = %p.accession, error = %e, "no se pudo archivar el folleto"),
                    }
                }
            }
        }
    });
    (sink, handle)
}

// -------------------------------------------------------------------------------------- cableado

/// Archivo de conocimiento abierto y sus colas de ingesta.
pub struct Rag {
    pub kb: Arc<KnowledgeBase>,
    pub activity: Activity,
    news: NewsSink,
    filings: FilingSink,
    funds: crate::fund_docs::FundArchive,
    workers: Vec<JoinHandle<()>>,
}

/// Abre el archivo con `OllamaEmbedder`. `None` (y `tracing::error!`) si algo falla: la app sigue sin RAG.
pub async fn open(cfg: KnowledgeConfig, pool: SqlitePool, shutdown: CancellationToken) -> Option<Rag> {
    let embedder = match OllamaEmbedder::new(cfg.embed.clone()) {
        Ok(e) => Arc::new(e) as Arc<dyn Embedder>,
        Err(e) => {
            tracing::error!(error = %e, "RAG desactivado: no se pudo crear el cliente de embeddings");
            return None;
        }
    };
    open_with(cfg, pool, embedder, shutdown).await
}

/// Como [`open`], con un embedder dado (tests: embedder simulado, sin Ollama).
pub async fn open_with(cfg: KnowledgeConfig, pool: SqlitePool, embedder: Arc<dyn Embedder>, shutdown: CancellationToken) -> Option<Rag> {
    let activity = Activity::default();
    let gated = Arc::new(GatedEmbedder::new(embedder, activity.clone(), shutdown.clone()));
    let kb = match KnowledgeBase::open(pool.clone(), gated, cfg).await {
        Ok(kb) => kb,
        Err(e) => {
            tracing::error!(error = %e, "RAG desactivado: no se pudo abrir el archivo de conocimiento (la app sigue sin él)");
            return None;
        }
    };
    let (news, w1) = spawn_news_worker(kb.clone(), shutdown.clone());
    let (filings, w2) = spawn_filing_worker(kb.clone(), shutdown.clone());
    let funds = crate::fund_docs::FundArchive::new(kb.clone(), pool, shutdown);
    Some(Rag { kb, activity, news, filings, funds, workers: vec![w1, w2] })
}

impl Rag {
    /// Envuelve mercado, OPV y analista: archivan lo que pasa por ellos y marcan la actividad del LLM.
    pub fn wrap(&self, market: Arc<dyn MarketData>, ipo: Arc<dyn IpoData>, analyst: Arc<dyn Analyst>) -> (Arc<dyn MarketData>, Arc<dyn IpoData>, Arc<dyn Analyst>) {
        (
            Arc::new(TeeMarket::new(market, self.news.clone())),
            Arc::new(TeeIpo::new(ipo, self.filings.clone())),
            Arc::new(TrackedAnalyst::new(analyst, self.activity.clone())),
        )
    }

    /// Como [`Rag::wrap`], y además archiva el folleto resumido de los ETF cuyo snapshot se pide
    /// (primera vez y luego como mucho cada 30 días), descargado en segundo plano con `funds`.
    pub fn wrap_with_funds(
        &self,
        market: Arc<dyn MarketData>,
        ipo: Arc<dyn IpoData>,
        analyst: Arc<dyn Analyst>,
        funds: Arc<dyn domain::FundDocs>,
    ) -> (Arc<dyn MarketData>, Arc<dyn IpoData>, Arc<dyn Analyst>) {
        self.funds.start(funds);
        (
            Arc::new(TeeMarket::new(market, self.news.clone()).with_funds(self.funds.sink())),
            Arc::new(TeeIpo::new(ipo, self.filings.clone())),
            Arc::new(TrackedAnalyst::new(analyst, self.activity.clone())),
        )
    }

    /// Búsqueda para `ai`.
    pub fn search_handle(&self) -> Arc<dyn KnowledgeSearch> {
        self.kb.clone()
    }

    /// Lanza el mantenimiento y registra los trabajadores en `tasks` (el apagado los espera).
    pub fn spawn_background(self, tasks: &TaskTracker, shutdown: CancellationToken) {
        for w in self.workers.into_iter().chain(self.funds.take_handles()) {
            tasks.spawn(async move {
                let _ = w.await;
            });
        }
        tasks.spawn(run_maintenance(self.kb, self.activity, shutdown));
    }
}

/// `embed_pending` al arrancar y de forma periódica (en tandas pequeñas, solo sin generaciones en
/// curso) y `purge` cada [`PURGE_EVERY`]. Termina con `shutdown`.
pub async fn run_maintenance(kb: Arc<KnowledgeBase>, activity: Activity, shutdown: CancellationToken) {
    let mut last_purge: Option<Instant> = None;
    loop {
        if !activity.busy() && !shutdown.is_cancelled() {
            loop {
                match kb.embed_pending(PENDING_BATCH).await {
                    Ok(0) => break,
                    Ok(n) => tracing::debug!(n, "embeddings pendientes completados"),
                    Err(e) => {
                        tracing::warn!(error = %e, "embed_pending falló");
                        break;
                    }
                }
                if activity.busy() || shutdown.is_cancelled() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if last_purge.is_none_or(|t| t.elapsed() >= PURGE_EVERY) {
                match kb.purge(Utc::now()).await {
                    Ok(r) => tracing::info!(by_age = r.documents_by_age, by_cap = r.documents_by_cap, chunks = r.chunks_removed, "archivo de conocimiento purgado"),
                    Err(e) => tracing::warn!(error = %e, "purge del archivo de conocimiento falló"),
                }
                last_purge = Some(Instant::now());
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(MAINTENANCE_TICK) => {}
        }
    }
}
