//! Implementaciones de mentira de `MarketData` y `Analyst`, deterministas y sin red.
//!
//! Sirven para los tests de integración y para arrancar el servidor sin proveedores ni Ollama
//! (`INVEST_AI_FAKE=1`), p. ej. mientras se desarrolla el frontend.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use domain::{
    Analysis, Analyst, AssetClass, AssetId, AssetSnapshot, AssetSummary, Candle, ChatEvent, ChatRequest,
    DomainError, Fundamentals, Horizon, Indicators, IpoEntry, IpoPerformance, Prospectus, Interval, KeyLevels, LlmStatus, MacroContext,
    MacroIndicator, MarketData, MarketState, NewsItem, PriceTick, ProviderStatus, Quote, Range,
    Result, ScoreBreakdown, Sentiment, SentimentLabel, SourceRef, Trend, Verdict,
};
use futures::{
    StreamExt,
    stream::{self, BoxStream},
};
use tokio_stream::wrappers::ReceiverStream;

/// (id, nombre, mercado, divisa, precio base)
const CATALOG: &[(&str, &str, &str, &str, f64)] = &[
    ("stock:AAPL", "Apple Inc.", "NASDAQ", "USD", 190.0),
    ("stock:MSFT", "Microsoft Corporation", "NASDAQ", "USD", 420.0),
    ("stock:TSLA", "Tesla, Inc.", "NASDAQ", "USD", 250.0),
    ("stock:SAN.MC", "Banco Santander", "BME", "EUR", 4.5),
    ("etf:SPY", "SPDR S&P 500 ETF Trust", "NYSE Arca", "USD", 520.0),
    ("crypto:BTC", "Bitcoin", "", "USD", 65000.0),
    ("crypto:ETH", "Ethereum", "", "USD", 3200.0),
    ("index:^GSPC", "S&P 500", "SNP", "USD", 5200.0),
    ("index:^IXIC", "NASDAQ Composite", "NASDAQ", "USD", 16500.0),
    ("index:^STOXX50E", "EURO STOXX 50", "STOXX", "EUR", 4900.0),
    ("index:^IBEX", "IBEX 35", "BME", "EUR", 11000.0),
    ("index:^VIX", "CBOE Volatility Index", "CBOE", "USD", 14.0),
    ("commodity:GC=F", "Oro (futuros)", "COMEX", "USD", 2300.0),
    ("commodity:CL=F", "Petróleo WTI (futuros)", "NYMEX", "USD", 80.0),
    ("forex:EURUSD=X", "EUR/USD", "", "USD", 1.08),
    ("bond:^TNX", "Rendimiento del bono a 10 años de EE. UU.", "", "USD", 4.2),
];

fn catalog_entry(id: &AssetId) -> Option<&'static (&'static str, &'static str, &'static str, &'static str, f64)> {
    let key = id.to_string();
    CATALOG.iter().find(|e| e.0 == key)
}

fn summary_of(e: &(&str, &str, &str, &str, f64)) -> AssetSummary {
    let id: AssetId = e.0.parse().expect("id del catálogo válido");
    AssetSummary {
        symbol: id.symbol.clone(),
        class: id.class,
        id,
        name: e.1.to_string(),
        exchange: (!e.2.is_empty()).then(|| e.2.to_string()),
        currency: Some(e.3.to_string()),
        country: None,
        source: "fake".into(),
        source_url: None,
    }
}

/// Fecha fija para que las respuestas sean deterministas.
/// Perfil de fondo INVENTADO del modo simulado (fuente `fake`): solo sirve para ver la pantalla.
fn fake_fund_profile() -> domain::FundProfile {
    let w = |name: &str, weight: f64| domain::WeightedItem { name: name.into(), weight };
    let h = |symbol: &str, name: &str, weight: f64| domain::Holding { symbol: Some(symbol.into()), name: name.into(), weight };
    domain::FundProfile {
        family: Some("Gestora simulada".into()),
        category: Some("Large Blend".into()),
        legal_type: Some("Exchange Traded Fund".into()),
        expense_ratio: Some(0.0009),
        total_assets: Some(5.0e11),
        dividend_yield: Some(0.012),
        turnover: Some(0.03),
        beta_3y: Some(1.0),
        top_holdings: vec![h("AAA", "Alfa Corp", 0.08), h("BBB", "Beta Inc", 0.07), h("CCC", "Gamma SA", 0.05)],
        sector_weights: vec![w("technology", 0.32), w("financial_services", 0.14), w("healthcare", 0.11)],
        asset_mix: vec![w("stock", 0.99), w("cash", 0.01)],
        equity: Some(domain::FundEquityStats { price_to_earnings: Some(23.5), price_to_book: Some(4.9), price_to_sales: Some(3.1), ..Default::default() }),
        source: "fake".into(),
        as_of: Some(fixed_time()),
        ..domain::FundProfile::default()
    }
}

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 1, 2, 15, 30, 0).unwrap()
}

pub struct FakeMarket {
    /// Cada cuánto emite ticks `subscribe`.
    pub tick_interval: Duration,
    /// Hace fallar `quote`/`quotes` con `Unavailable` (para probar fallos parciales).
    pub fail_quotes: AtomicBool,
    /// Hace fallar `news` con `Upstream`.
    pub fail_news: AtomicBool,
    /// Retraso artificial de `snapshot` (para probar el single-flight).
    pub snapshot_delay: Mutex<Duration>,
    pub snapshot_calls: AtomicUsize,
    pub search_calls: AtomicUsize,
    /// Conjuntos de ids con los que se llamó a `subscribe`, en orden.
    pub subscriptions: Mutex<Vec<Vec<AssetId>>>,
}

impl Default for FakeMarket {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_millis(100),
            fail_quotes: AtomicBool::new(false),
            fail_news: AtomicBool::new(false),
            snapshot_delay: Mutex::new(Duration::ZERO),
            snapshot_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
            subscriptions: Mutex::new(Vec::new()),
        }
    }
}

impl FakeMarket {
    fn quote_of(e: &(&str, &str, &str, &str, f64)) -> Quote {
        let id: AssetId = e.0.parse().expect("id del catálogo válido");
        let price = e.4;
        let prev = price / 1.01;
        Quote {
            id,
            price,
            change: Some(price - prev),
            change_pct: Some(1.0),
            open: Some(prev),
            high: Some(price * 1.01),
            low: Some(prev * 0.99),
            prev_close: Some(prev),
            volume: Some(1_000_000.0),
            market_cap: None,
            currency: Some(e.3.to_string()),
            market_state: if e.0.starts_with("crypto:") { MarketState::Continuous } else { MarketState::Regular },
            timestamp: fixed_time(),
            source: "fake".into(),
            delay_secs: Some(0),
            source_url: None,
            timestamp_origin: None,
        }
    }

    fn news_for(id: Option<&AssetId>, limit: usize) -> Vec<NewsItem> {
        let subject = id.map_or("los mercados".to_string(), |i| i.symbol.clone());
        (1..=5)
            .map(|n| NewsItem {
                id: format!("news-{}-{n}", id.map_or("general".into(), |i| i.to_string())),
                title: format!("Noticia {n} sobre {subject}"),
                summary: Some(format!("Resumen de la noticia {n}.")),
                url: format!("https://example.com/news/{n}"),
                publisher: Some("Fake Wire".into()),
                published_at: fixed_time() - chrono::Duration::hours(i64::from(n)),
                image_url: None,
                tickers: id.map(|i| vec![i.symbol.clone()]).unwrap_or_default(),
                sentiment: None,
                provider: "fake".into(),
                language: Some("es".into()),
            })
            .take(limit)
            .collect()
    }
}

#[async_trait]
impl MarketData for FakeMarket {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<AssetSummary>> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        let q = query.to_lowercase();
        Ok(CATALOG
            .iter()
            .filter(|e| e.0.to_lowercase().contains(&q) || e.1.to_lowercase().contains(&q))
            .take(limit)
            .map(summary_of)
            .collect())
    }

    async fn quote(&self, id: &AssetId) -> Result<Quote> {
        if self.fail_quotes.load(Ordering::SeqCst) {
            return Err(DomainError::Unavailable("proveedor de cotizaciones caído".into()));
        }
        catalog_entry(id)
            .map(Self::quote_of)
            .ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))
    }

    async fn quotes(&self, ids: &[AssetId]) -> Result<Vec<Quote>> {
        if self.fail_quotes.load(Ordering::SeqCst) {
            return Err(DomainError::Unavailable("proveedor de cotizaciones caído".into()));
        }
        Ok(ids.iter().filter_map(catalog_entry).map(Self::quote_of).collect())
    }

    async fn history(&self, id: &AssetId, _range: Range, interval: Interval) -> Result<Vec<Candle>> {
        let e = catalog_entry(id).ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))?;
        Ok((0..24)
            .map(|i| {
                let base = e.4 * (1.0 + 0.001 * f64::from(i));
                Candle {
                    t: 1_735_689_600 + i64::from(i) * interval.seconds(),
                    o: base,
                    h: base * 1.01,
                    l: base * 0.99,
                    c: base * 1.002,
                    v: Some(1000.0 + f64::from(i)),
                }
            })
            .collect())
    }

    async fn fundamentals(&self, id: &AssetId) -> Result<Option<Fundamentals>> {
        catalog_entry(id).ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))?;
        if !matches!(id.class, AssetClass::Stock | AssetClass::Etf) {
            return Ok(None);
        }
        Ok(Some(Fundamentals {
            description: Some(format!("Descripción simulada de {}.", id.symbol)),
            sector: Some("Technology".into()),
            pe_trailing: Some(28.5),
            pe_forward: Some(25.0),
            profit_margin: Some(0.25),
            dividend_yield: Some(0.005),
            fund_profile: (id.class == AssetClass::Etf).then(fake_fund_profile),
            sources: vec!["fake".into()],
            as_of: Some(fixed_time()),
            ..Fundamentals::default()
        }))
    }

    async fn indicators(&self, id: &AssetId) -> Result<Indicators> {
        let e = catalog_entry(id).ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))?;
        Ok(Indicators {
            as_of: Some(fixed_time()),
            last_close: Some(e.4),
            sma50: Some(e.4 * 0.97),
            sma200: Some(e.4 * 0.9),
            rsi14: Some(55.0),
            trend: Some(Trend::Up),
            supports: vec![e.4 * 0.95],
            resistances: vec![e.4 * 1.05],
            ..Indicators::default()
        })
    }

    async fn news(&self, id: Option<&AssetId>, limit: usize) -> Result<Vec<NewsItem>> {
        if self.fail_news.load(Ordering::SeqCst) {
            return Err(DomainError::Upstream { provider: "fake".into(), message: "noticias caídas".into() });
        }
        if let Some(id) = id {
            catalog_entry(id).ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))?;
        }
        Ok(Self::news_for(id, limit))
    }

    async fn macro_context(&self) -> Result<MacroContext> {
        Ok(MacroContext {
            indicators: vec![MacroIndicator {
                key: "fed_funds".into(),
                name: "Tipo de la Fed".into(),
                value: 5.25,
                change: Some(0.0),
                unit: "%".into(),
                as_of: fixed_time(),
                source: "fake".into(),
                source_url: None,
                timestamp_origin: None,
            }],
            as_of: Some(fixed_time()),
        })
    }

    async fn snapshot(&self, id: &AssetId) -> Result<AssetSnapshot> {
        self.snapshot_calls.fetch_add(1, Ordering::SeqCst);
        let delay = *self.snapshot_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let e = catalog_entry(id).ok_or_else(|| DomainError::NotFound(format!("activo desconocido: {id}")))?;
        Ok(AssetSnapshot {
            summary: summary_of(e),
            quote: Some(Self::quote_of(e)),
            fundamentals: self.fundamentals(id).await?,
            indicators: Some(self.indicators(id).await?),
            news: Self::news_for(Some(id), 5),
            macro_context: Some(self.macro_context().await?),
            missing: Vec::new(),
            generated_at: fixed_time(),
        })
    }

    fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        self.subscriptions.lock().unwrap().push(ids.to_vec());
        let ids: Vec<AssetId> = ids.iter().filter(|id| catalog_entry(id).is_some()).cloned().collect();
        let interval = tokio::time::interval(self.tick_interval);
        stream::unfold((interval, ids, 0u32), |(mut interval, ids, n)| async move {
            interval.tick().await;
            let ticks: Vec<PriceTick> = ids
                .iter()
                .map(|id| {
                    let base = catalog_entry(id).map_or(100.0, |e| e.4);
                    PriceTick {
                        id: id.clone(),
                        price: base + f64::from(n) * 0.01,
                        change_pct: Some(1.0),
                        volume: None,
                        ts: Utc::now(),
                        source: "fake".into(),
                    }
                })
                .collect();
            Some((ticks, (interval, ids, n + 1)))
        })
        .flat_map(stream::iter)
        .boxed()
    }

    fn providers_status(&self) -> Vec<ProviderStatus> {
        vec![ProviderStatus {
            name: "fake".into(),
            enabled: true,
            requires_key: false,
            has_key: false,
            capabilities: vec!["search".into(), "quote".into(), "realtime".into()],
            asset_classes: AssetClass::ALL.to_vec(),
            last_error: None,
            last_ok: Some(fixed_time()),
            blocked_secs: None,
            latency_ms: None,
        }]
    }
}

/// Anotador de sentimiento de mentira (sustituye a `ai::annotate_sentiment`).
pub fn annotate_fake(items: &mut [domain::NewsItem]) {
    for item in items {
        item.sentiment = Some(Sentiment { score: 0.5, label: SentimentLabel::Positive, method: "fake".into() });
    }
}

// ---------------------------------------------------------------------------------------------
// Analista
// ---------------------------------------------------------------------------------------------

/// Lo que vio el analista en una petición de chat (para afirmar en los tests).
#[derive(Debug, Clone)]
pub struct SeenChat {
    pub history_len: usize,
    pub user_message: String,
    pub focus: Vec<AssetId>,
    pub language: String,
}

#[derive(Default)]
pub struct FakeAnalyst {
    /// Retraso artificial de `analyze`.
    pub analyze_delay: Mutex<Duration>,
    pub analyze_calls: AtomicUsize,
    /// Pausa entre tokens del chat.
    pub token_delay: Mutex<Duration>,
    pub seen: Mutex<Vec<SeenChat>>,
    /// Profundidad pedida en cada `analyze_stream` (para comprobar qué análisis se lanzó).
    pub seen_depths: Mutex<Vec<Option<domain::AnalysisDepth>>>,
    /// Si es `true`, `analyze_stream` hace panic (simula un fallo inesperado del analista).
    pub panic_on_analyze: std::sync::atomic::AtomicBool,
    /// Fallos simulados por símbolo: `(intentos que aún fallan, código de error)`; `u32::MAX` = siempre.
    pub fail_symbols: Mutex<std::collections::HashMap<String, (u32, String)>>,
    /// Hace fallar con `unavailable` las N primeras generaciones (Ollama caído que luego vuelve).
    pub fail_first_calls: AtomicUsize,
    /// Activos en el orden en que empezó su análisis.
    pub started: Mutex<Vec<AssetId>>,
    /// Análisis en curso ahora mismo y máximo simultáneo observado (para comprobar «de uno en uno»).
    pub running_now: AtomicUsize,
    pub max_running: AtomicUsize,
}

impl FakeAnalyst {
    pub fn build_analysis(snapshot: &AssetSnapshot) -> Analysis {
        let price = snapshot.quote.as_ref().map(|q| q.price);
        Analysis {
            id: uuid::Uuid::now_v7().to_string(),
            asset: snapshot.summary.id.clone(),
            verdict: Verdict::Accumulate,
            confidence: 0.7,
            horizon: Horizon::Medium,
            scores: ScoreBreakdown {
                overall: 68.0,
                technical: Some(70.0),
                momentum: Some(65.0),
                fundamental: Some(72.0),
                valuation: Some(55.0),
                sentiment: Some(60.0),
                macro_score: Some(50.0),
                risk: Some(40.0),
            },
            summary: format!("Análisis simulado de {}.", snapshot.summary.name),
            thesis: "Tesis simulada.".into(),
            bull_case: vec!["Tendencia alcista".into()],
            bear_case: vec!["Valoración exigente".into()],
            risks: vec!["Riesgo macro".into()],
            catalysts: Vec::new(),
            key_levels: KeyLevels {
                supports: price.map(|p| vec![p * 0.95]).unwrap_or_default(),
                resistances: price.map(|p| vec![p * 1.05]).unwrap_or_default(),
                entry_zone: price.map(|p| (p * 0.97, p)),
                stop_loss: price.map(|p| p * 0.9),
                targets: price.map(|p| vec![p * 1.15]).unwrap_or_default(),
            },
            strategy: Some("Entradas escalonadas.".into()),
            data_warnings: snapshot.missing.clone(),
            sources: vec![SourceRef {
                title: "Fake Wire".into(),
                url: Some("https://example.com/news/1".into()),
                provider: "fake".into(),
                published_at: Some(fixed_time()),
            }],
            price_at_analysis: price,
            model: "fake-model".into(),
            generated_at: Utc::now(),
            levels_note: Some("Niveles simulados.".into()),
            scenarios: None,
            expected_return_pct: None,
            would_change_if: Some(vec!["El precio cierra bajo el stop.".into()]),
            horizon_note: Some("Horizonte simulado.".into()),
            position_sizing: None,
            method: Some("Modelo simulado.".into()),
            quality_notes: Some(Vec::new()),
            depth: None,
            duration_ms: None,
            llm_calls: None,
            prompt_tokens: None,
            completion_tokens: None,
        }
    }
}

#[async_trait]
impl Analyst for FakeAnalyst {
    async fn analyze(&self, snapshot: &AssetSnapshot) -> Result<Analysis> {
        self.analyze_calls.fetch_add(1, Ordering::SeqCst);
        let delay = *self.analyze_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(Self::build_analysis(snapshot))
    }

    fn analyze_stream(
        self: std::sync::Arc<Self>,
        snapshot: AssetSnapshot,
        depth: Option<domain::AnalysisDepth>,
    ) -> BoxStream<'static, domain::AnalysisEvent> {
        use domain::AnalysisEvent;
        self.seen_depths.lock().unwrap().push(depth);
        self.started.lock().unwrap().push(snapshot.summary.id.clone());
        assert!(!self.panic_on_analyze.load(Ordering::SeqCst), "panic simulado del analista");
        const STAGES: [(&str, &str); 4] = [
            ("snapshot", "Preparando datos, puntuación y niveles"),
            ("technical", "Análisis técnico (simulado)"),
            ("judge", "Veredicto del juez (simulado)"),
            ("verify", "Verificando cifras y redacción"),
        ];
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            self.analyze_calls.fetch_add(1, Ordering::SeqCst);
            let now = self.running_now.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_running.fetch_max(now, Ordering::SeqCst);
            // El contador baja ANTES de emitir el evento final: quien lo recibe puede lanzar otro.
            struct Running(std::sync::Arc<FakeAnalyst>);
            impl Drop for Running {
                fn drop(&mut self) {
                    self.0.running_now.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let running = Running(self.clone());
            let planned_failure = {
                let first = self
                    .fail_first_calls
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok();
                let mut map = self.fail_symbols.lock().unwrap();
                let by_symbol = map.get_mut(&snapshot.summary.id.to_string()).and_then(|(left, code)| {
                    (*left > 0).then(|| {
                        if *left != u32::MAX {
                            *left -= 1;
                        }
                        code.clone()
                    })
                });
                by_symbol.or_else(|| first.then(|| "unavailable".to_string()))
            };
            if let Some(code) = planned_failure {
                drop(running);
                let _ = tx.send(AnalysisEvent::Error { code, message: "fallo simulado del analista".into() });
                return;
            }
            let delay = *self.analyze_delay.lock().unwrap() / STAGES.len() as u32;
            for (i, (stage, label)) in STAGES.iter().enumerate() {
                let ev = AnalysisEvent::Stage {
                    stage: (*stage).into(),
                    label: (*label).into(),
                    index: i as u32 + 1,
                    total: STAGES.len() as u32,
                };
                if tx.send(ev).is_err() {
                    return;
                }
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
            drop(running);
            let _ = tx.send(AnalysisEvent::Done { analysis: Self::build_analysis(&snapshot) });
        });
        tokio_stream::wrappers::UnboundedReceiverStream::new(rx).boxed()
    }

    fn chat(&self, req: ChatRequest) -> BoxStream<'static, ChatEvent> {
        self.seen.lock().unwrap().push(SeenChat {
            history_len: req.history.len(),
            user_message: req.user_message.clone(),
            focus: req.focus.clone(),
            language: req.language.clone(),
        });
        let delay = *self.token_delay.lock().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            // Si el receptor se suelta, `send` falla y la tarea termina.
            macro_rules! emit {
                ($ev:expr) => {
                    if tx.send($ev).await.is_err() {
                        return;
                    }
                };
            }
            let message_id = uuid::Uuid::now_v7().to_string();
            emit!(ChatEvent::Start { message_id: message_id.clone() });
            emit!(ChatEvent::Thinking { text: "Revisando los datos disponibles.".into() });

            if let Some(id) = req.focus.first() {
                let call_id = "call-1".to_string();
                emit!(ChatEvent::ToolCall {
                    id: call_id.clone(),
                    name: "get_quote".into(),
                    arguments: serde_json::json!({ "id": id.to_string() }),
                });
                // Usa el `market` de la petición, como haría el bucle de tool calling real.
                let (ok, summary) = match req.market.quote(id).await {
                    Ok(q) => (true, format!("{}: {:.2}", id.symbol, q.price)),
                    Err(e) => (false, e.to_string()),
                };
                emit!(ChatEvent::ToolResult { id: call_id, name: "get_quote".into(), ok, summary });
            }

            if req.user_message.to_lowercase().contains("falla") {
                emit!(ChatEvent::Token { text: "Empiezo a responder".into() });
                emit!(ChatEvent::Error { message: "fallo simulado del modelo".into() });
                return;
            }

            let focus = req.focus.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let reply = format!("Respuesta simulada [{focus}] a: {}", req.user_message);
            let mut completion_tokens = 0u32;
            for word in reply.split_inclusive(' ') {
                emit!(ChatEvent::Token { text: word.to_string() });
                completion_tokens += 1;
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }

            if req.user_message.to_lowercase().contains("analiza")
                && let Some(id) = req.focus.first()
                && let Ok(snapshot) = req.market.snapshot(id).await
            {
                emit!(ChatEvent::Analysis { analysis: Box::new(FakeAnalyst::build_analysis(&snapshot)) });
            }

            emit!(ChatEvent::Done {
                message_id,
                model: "fake-model".into(),
                prompt_tokens: Some(u32::try_from(req.history.len() * 10 + 10).unwrap_or(u32::MAX)),
                completion_tokens: Some(completion_tokens),
            });
        });
        ReceiverStream::new(rx).boxed()
    }

    async fn status(&self) -> LlmStatus {
        LlmStatus {
            available: true,
            base_url: "http://fake.invalid".into(),
            model: "fake-model".into(),
            installed_models: vec!["fake-model".into()],
            supports_tools: true,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// OPV simulada (INVEST_AI_FAKE=1): empresas inventadas, texto de folleto inventado
// ---------------------------------------------------------------------------------------------

/// Datos de OPV inventados pero verosímiles. El texto de los folletos usa el mismo formato que los
/// reales (tablas «Etiqueta | valor»), así que el perfil de riesgo lo calcula el extractor real.
/// Todo va marcado con `source = "fake"`; las URLs no apuntan a documentos reales.
#[derive(Clone)]
pub struct FakeIpo {
    /// «Hoy» de la simulación (el calendario se genera en torno a esta fecha).
    pub today: chrono::NaiveDate,
}

impl Default for FakeIpo {
    fn default() -> Self {
        Self { today: Utc::now().date_naive() }
    }
}

/// Ficha de una OPV inventada.
struct FakeSpec {
    symbol: &'static str,
    name: &'static str,
    exchange: &'static str,
    /// Días respecto a «hoy» (negativo = ya cotiza).
    offset: i64,
    status: domain::IpoStatus,
    /// Precio (o rango) en USD; `None` en presentaciones.
    price: Option<(f64, f64)>,
    new_shares: u64,
    sell_shares: u64,
    out_after: u64,
    spac: bool,
    lock_days: u32,
    debt_musd: Option<u32>,
    dual_class: bool,
    controlled_pct: Option<u32>,
    dilution: f64,
    /// (ingresos en miles de USD, resultado neto en miles) por periodo.
    fin: [(i64, i64); 3],
    underwriters: [&'static str; 3],
    /// Bandera de rendimiento: (día 1 %, 3 meses %) usados por la simulación.
    drift: (f64, f64),
}

const FAKE_IPOS: &[FakeSpec] = &[
    FakeSpec { symbol: "LHTX", name: "Lumen Harbor Therapeutics", exchange: "NASDAQ Global Select", offset: -12, status: domain::IpoStatus::Priced, price: Some((16.0, 16.0)), new_shares: 12_500_000, sell_shares: 0, out_after: 48_300_000, spac: false, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 9.12, fin: [(0, -41_200), (0, -63_900), (2_100, -78_400)], underwriters: ["Fictional Securities LLC", "Example Capital Markets", "Placeholder & Co."], drift: (31.0, 0.0) },
    FakeSpec { symbol: "ORFS", name: "Orbital Freight Systems", exchange: "NYSE", offset: -27, status: domain::IpoStatus::Priced, price: Some((24.0, 24.0)), new_shares: 15_000_000, sell_shares: 18_000_000, out_after: 212_000_000, spac: false, lock_days: 180, debt_musd: Some(240), dual_class: true, controlled_pct: Some(71), dilution: 21.4, fin: [(612_300, 18_400), (781_900, -22_700), (904_500, 31_200)], underwriters: ["Example Capital Markets", "Fictional Securities LLC", "Sample Partners LLC"], drift: (-6.0, -18.0) },
    FakeSpec { symbol: "CBME", name: "Cobalt Meridian Energy", exchange: "NYSE", offset: 9, status: domain::IpoStatus::Expected, price: Some((18.0, 20.0)), new_shares: 20_000_000, sell_shares: 0, out_after: 96_000_000, spac: false, lock_days: 180, debt_musd: Some(120), dual_class: false, controlled_pct: None, dilution: 14.8, fin: [(210_000, -12_000), (305_400, -4_300), (388_700, 9_800)], underwriters: ["Sample Partners LLC", "Example Capital Markets", "Placeholder & Co."], drift: (0.0, 0.0) },
    FakeSpec { symbol: "NRWF", name: "Nimbus Row Foods", exchange: "NASDAQ Global Market", offset: 23, status: domain::IpoStatus::Expected, price: Some((12.0, 14.0)), new_shares: 9_000_000, sell_shares: 3_000_000, out_after: 41_500_000, spac: false, lock_days: 90, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 8.3, fin: [(88_000, 1_900), (97_400, 3_100), (121_800, 6_700)], underwriters: ["Placeholder & Co.", "Fictional Securities LLC", "Sample Partners LLC"], drift: (0.0, 0.0) },
    FakeSpec { symbol: "HQACU", name: "Helix Quarry Acquisition Corp.", exchange: "NASDAQ Global", offset: -5, status: domain::IpoStatus::Priced, price: Some((10.0, 10.0)), new_shares: 20_000_000, sell_shares: 0, out_after: 25_000_000, spac: true, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 0.9, fin: [(0, -120), (0, -310), (0, -95)], underwriters: ["Example Capital Markets", "Fictional Securities LLC", "Sample Partners LLC"], drift: (0.5, 0.0) },
    FakeSpec { symbol: "VLPM", name: "Verdant Loop Mobility", exchange: "NASDAQ", offset: -3, status: domain::IpoStatus::Filed, price: None, new_shares: 0, sell_shares: 0, out_after: 0, spac: false, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 0.0, fin: [(0, 0), (0, 0), (0, 0)], underwriters: ["", "", ""], drift: (0.0, 0.0) },
    FakeSpec { symbol: "STRB", name: "Saffron Tide Robotics", exchange: "NASDAQ", offset: 15, status: domain::IpoStatus::Withdrawn, price: None, new_shares: 0, sell_shares: 0, out_after: 0, spac: false, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 0.0, fin: [(0, 0), (0, 0), (0, 0)], underwriters: ["", "", ""], drift: (0.0, 0.0) },
    // Más antiguas: solo alimentan la tasa base y el rendimiento posterior.
    FakeSpec { symbol: "GPKS", name: "Granite Peak Software", exchange: "NASDAQ Global Select", offset: -75, status: domain::IpoStatus::Priced, price: Some((20.0, 20.0)), new_shares: 14_000_000, sell_shares: 6_000_000, out_after: 88_000_000, spac: false, lock_days: 180, debt_musd: None, dual_class: true, controlled_pct: None, dilution: 15.2, fin: [(142_000, -9_800), (201_000, -4_100), (268_000, 2_300)], underwriters: ["Fictional Securities LLC", "Example Capital Markets", "Sample Partners LLC"], drift: (44.0, 22.0) },
    FakeSpec { symbol: "AMBR", name: "Amber Trail Biosciences", exchange: "NASDAQ Global Select", offset: -120, status: domain::IpoStatus::Priced, price: Some((15.0, 15.0)), new_shares: 10_000_000, sell_shares: 0, out_after: 39_000_000, spac: false, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 8.9, fin: [(0, -30_000), (0, -52_000), (0, -71_000)], underwriters: ["Example Capital Markets", "Placeholder & Co.", "Fictional Securities LLC"], drift: (12.0, -27.0) },
    FakeSpec { symbol: "KLRN", name: "Kilroy Nine Logistics", exchange: "NYSE", offset: -170, status: domain::IpoStatus::Priced, price: Some((28.0, 28.0)), new_shares: 18_000_000, sell_shares: 12_000_000, out_after: 150_000_000, spac: false, lock_days: 180, debt_musd: Some(300), dual_class: false, controlled_pct: Some(64), dilution: 25.1, fin: [(402_000, 21_000), (470_000, 33_400), (561_000, 40_100)], underwriters: ["Sample Partners LLC", "Fictional Securities LLC", "Example Capital Markets"], drift: (8.0, 5.0) },
    FakeSpec { symbol: "PNDL", name: "Pendulum Reach Health", exchange: "NASDAQ Global Select", offset: -230, status: domain::IpoStatus::Priced, price: Some((17.0, 17.0)), new_shares: 16_000_000, sell_shares: 0, out_after: 71_000_000, spac: false, lock_days: 180, debt_musd: None, dual_class: false, controlled_pct: None, dilution: 11.0, fin: [(55_000, -38_000), (79_000, -44_000), (96_000, -51_000)], underwriters: ["Placeholder & Co.", "Example Capital Markets", "Sample Partners LLC"], drift: (63.0, 41.0) },
];

/// Pseudoaleatorio estable por símbolo (no es criptografía: solo variación inventada).
fn fake_noise(sym: &str, salt: u32) -> f64 {
    let mut h: u32 = 2_166_136_261 ^ salt;
    for b in sym.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    f64::from(h % 10_000) / 10_000.0 - 0.5
}

fn group3(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn money_cell(v: i64) -> String {
    if v < 0 { format!("$({})", group3(v.unsigned_abs())) } else { format!("${}", group3(v as u64)) }
}

impl FakeIpo {
    fn entry(&self, s: &FakeSpec) -> IpoEntry {
        let (lo, hi) = s.price.unzip();
        let total = match (s.status, s.price) {
            (domain::IpoStatus::Priced, Some((p, _))) => Some(p * (s.new_shares + s.sell_shares) as f64),
            (domain::IpoStatus::Expected, Some((_, h))) => Some(h * (s.new_shares + s.sell_shares) as f64 * 1.15),
            _ => None,
        };
        IpoEntry {
            date: self.today + chrono::Duration::days(s.offset),
            symbol: Some(s.symbol.into()),
            name: s.name.into(),
            exchange: Some(s.exchange.into()),
            status: s.status,
            price_low: lo,
            price_high: hi,
            shares: (s.new_shares + s.sell_shares > 0).then_some(s.new_shares + s.sell_shares),
            total_value: total,
            spac_likely: s.spac,
            source: "fake".into(),
            source_url: None,
            as_of: fixed_time(),
        }
    }

    fn spec(symbol: &str) -> Option<&'static FakeSpec> {
        FAKE_IPOS.iter().find(|s| s.symbol.eq_ignore_ascii_case(symbol))
    }

    fn section(key: &str, title: &str, text: String) -> domain::ProspectusSection {
        let n = text.chars().count();
        domain::ProspectusSection { key: key.into(), title: title.into(), text, parent: None, truncated: false, original_chars: n }
    }

    /// Folleto inventado con el formato de los reales.
    fn fake_prospectus(&self, s: &FakeSpec) -> Prospectus {
        let price = s.price.map_or(0.0, |(_, h)| h);
        let total = s.new_shares + s.sell_shares;
        let mut offering = format!("Common stock offered by us | {} shares.\n", group3(s.new_shares));
        if s.sell_shares > 0 {
            offering.push_str(&format!("Common stock offered by the selling stockholders | {} shares.\n", group3(s.sell_shares)));
        }
        offering.push_str(&format!("Common stock to be outstanding immediately after this offering | {} shares.\n", group3(s.out_after)));
        if s.dual_class {
            offering.push_str("Voting rights | Following this offering we will have two classes of common stock. Our Class A common stock will be entitled to one vote per share, and our Class B common stock will be entitled to ten votes per share.\n");
        }
        let mut uop = format!(
            "We estimate that we will receive net proceeds of approximately ${:.1} million from the sale of shares in this offering, based on an assumed initial public offering price of ${price:.2} per share. This is simulated text.\n",
            price * s.new_shares as f64 * 0.93 / 1e6
        );
        if s.sell_shares > 0 {
            uop.push_str("We will not receive any proceeds from the sale of shares by the selling stockholders.\n");
        }
        match s.debt_musd {
            Some(d) => uop.push_str(&format!("We intend to use the net proceeds to repay approximately ${d} million of outstanding indebtedness under our term loan and for general corporate purposes.\n")),
            None => uop.push_str("We intend to use the net proceeds to fund clinical and product development, working capital and other general corporate purposes.\n"),
        }
        let mut summary = String::from("This is a simulated prospectus summary written for the INVEST_AI_FAKE mode. The company, its figures and its underwriters are invented.\n");
        if let Some(pct) = s.controlled_pct {
            summary.push_str(&format!("As a “controlled company” under the listing rules we may exempt ourselves from some governance requirements. Our sponsor will control approximately {pct}% of the voting power of our outstanding capital stock after this offering.\n"));
        }
        let lock = format!(
            "We, our directors, executive officers and the holders of substantially all of our equity securities have agreed with the underwriters that for a period of {} days after the date of this prospectus we will not sell or transfer any shares, subject to exceptions.\n",
            s.lock_days
        );
        let mut uw = String::from("The underwriters named below have severally agreed to purchase the shares set forth opposite their names. Simulated text.\nName | Number of Shares\n");
        let split = [total * 45 / 100, total * 35 / 100];
        let parts = [split[0], split[1], total - split[0] - split[1]];
        for (n, k) in s.underwriters.iter().zip(parts) {
            uw.push_str(&format!("{n} | {}\n", group3(k)));
        }
        uw.push_str(&format!("Total | {}\n", group3(total)));
        let dil = format!(
            "Purchasers of shares in this offering will experience immediate dilution.\nDilution per share to new investors in this offering | ${:.2}\n",
            s.dilution
        );
        let fin = format!(
            "(in thousands) | 2024 | 2025 | Six Months Ended June 30, 2026\nRevenue | {} | {} | {}\nNet loss | {} | {} | {}\n",
            money_cell(s.fin[0].0), money_cell(s.fin[1].0), money_cell(s.fin[2].0),
            money_cell(s.fin[0].1), money_cell(s.fin[1].1), money_cell(s.fin[2].1),
        );
        // «Net loss» con resultados positivos se ve como «Net income (loss)» en los folletos reales.
        let fin = if s.fin.iter().any(|f| f.1 > 0) { fin.replace("Net loss", "Net income (loss)") } else { fin };
        let sections = vec![
            Self::section("summary", "Prospectus Summary", summary),
            Self::section("offering", "The Offering", offering),
            Self::section("summary_financial_data", "Summary Financial Data", fin),
            Self::section("risk_factors", "Risk Factors", "Simulated risk factors. Investing in these shares involves a high degree of risk (invented text).".into()),
            Self::section("use_of_proceeds", "Use of Proceeds", uop),
            Self::section("dilution", "Dilution", dil),
            Self::section("lock_up", "Lock-Up Agreements", lock),
            Self::section("underwriting", "Underwriting", uw),
        ];
        let entry = self.entry(s);
        Prospectus {
            issuer: s.name.into(),
            cik: 9_000_000 + s.symbol.len() as u64,
            ticker: Some(s.symbol.into()),
            form: if s.status == domain::IpoStatus::Priced { "424B4".into() } else { "S-1/A".into() },
            preliminary: s.status != domain::IpoStatus::Priced,
            filed_on: entry.date - chrono::Duration::days(1),
            accession: "0000000000-00-000000".into(),
            url: "https://www.sec.gov/edgar/search/".into(),
            index_url: "https://www.sec.gov/edgar/search/".into(),
            offer_price: s.price.map(|(_, h)| h),
            missing: vec!["capitalization".into(), "dividend_policy".into(), "principal_stockholders".into(), "shares_eligible".into()],
            document_chars: sections.iter().map(|x| x.original_chars).sum(),
            sections,
            source: "fake".into(),
            fetched_at: fixed_time(),
        }
    }

    fn fake_performance(&self, s: &FakeSpec, date: chrono::NaiveDate, price: f64, bench: &str) -> IpoPerformance {
        let today = self.today;
        let days = (today - date).num_days().max(0);
        let d1 = s.drift.0 + fake_noise(s.symbol, 1) * 6.0;
        let long = s.drift.1;
        let ret = |months: i64| -> f64 { d1 + (long - d1) * (months as f64 / 3.0).min(2.0) + fake_noise(s.symbol, months as u32 + 7) * 8.0 };
        let mut points = Vec::new();
        for (h, m) in [("1m", 1i64), ("3m", 3), ("6m", 6), ("12m", 12)] {
            let target = date + chrono::Duration::days(m * 30);
            let reached = (today - target).num_days() >= 0;
            let r = reached.then(|| ret(m));
            let b = reached.then(|| 1.4 * m as f64 + fake_noise(s.symbol, 40 + m as u32) * 2.0);
            points.push(domain::IpoReturnPoint {
                horizon: h.into(),
                target_date: target,
                price_date: reached.then_some(target),
                price: r.map(|r| price * (1.0 + r / 100.0)),
                return_pct: r,
                since_first_close_pct: r.map(|r| ((1.0 + r / 100.0) / (1.0 + d1 / 100.0) - 1.0) * 100.0),
                benchmark_return_pct: b,
                excess_pct: r.zip(b).map(|(r, b)| r - b),
            });
        }
        let latest = if days >= 30 { ret((days / 30).min(12)) } else { d1 };
        IpoPerformance {
            symbol: s.symbol.into(),
            ipo_date: date,
            offer_price: price,
            offer_price_source: "calendar".into(),
            first_day_close: price * (1.0 + d1 / 100.0),
            first_day_return_pct: d1,
            first_day_high_pct: d1 + 4.0 + fake_noise(s.symbol, 3).abs() * 6.0,
            points,
            latest_price: price * (1.0 + latest / 100.0),
            latest_date: today,
            latest_return_pct: latest,
            max_drawdown_pct: -(8.0 + fake_noise(s.symbol, 5).abs() * 40.0),
            benchmark: bench.into(),
            source: "fake".into(),
            source_url: None,
            warnings: vec!["Datos simulados (INVEST_AI_FAKE).".into()],
            as_of: fixed_time(),
        }
    }
}

#[async_trait]
impl domain::IpoData for FakeIpo {
    async fn calendar(&self, from: chrono::NaiveDate, to: chrono::NaiveDate) -> Result<Vec<IpoEntry>> {
        Ok(FAKE_IPOS.iter().map(|s| self.entry(s)).filter(|e| e.date >= from && e.date <= to).collect())
    }

    async fn prospectus(&self, issuer: &str) -> Result<Prospectus> {
        let s = Self::spec(issuer).ok_or_else(|| DomainError::NotFound(format!("folleto no encontrado: {issuer}")))?;
        if s.price.is_none() || s.new_shares == 0 {
            return Err(DomainError::NotFound(format!("{issuer}: aún sin folleto con datos de la oferta")));
        }
        Ok(self.fake_prospectus(s))
    }

    async fn performance(&self, symbol: &str, ipo_date: chrono::NaiveDate, offer_price: Option<f64>, benchmark: &str) -> Result<IpoPerformance> {
        let s = Self::spec(symbol).ok_or_else(|| DomainError::NotFound(format!("{symbol}: OPV desconocida")))?;
        let e = self.entry(s);
        let price = offer_price.or(e.price_low).ok_or_else(|| DomainError::NotFound(format!("{symbol}: sin precio de OPV")))?;
        if e.status != domain::IpoStatus::Priced || e.date > self.today {
            return Err(DomainError::NotFound(format!("{symbol}: aún no cotiza")));
        }
        Ok(self.fake_performance(s, ipo_date, price, benchmark))
    }

    async fn base_rate(&self, months: u32) -> Result<domain::IpoBaseRate> {
        let cutoff = self.today - chrono::Duration::days(i64::from(months) * 30);
        let perfs: Vec<market::ipo_risk::SizedPerf> = FAKE_IPOS
            .iter()
            .filter(|s| s.status == domain::IpoStatus::Priced && !s.spac)
            .map(|s| (s, self.entry(s)))
            .filter(|(_, e)| e.date >= cutoff && e.date <= self.today)
            .map(|(s, e)| market::ipo_risk::SizedPerf {
                size_usd: market::ipo_risk::offer_size_usd(&e),
                perf: self.fake_performance(s, e.date, e.price_low.unwrap_or(10.0), "SPY"),
            })
            .collect();
        let mut b = market::ipo_risk::compute_base_rate_sized(&perfs, months, "SPY", fixed_time());
        b.source = "fake".into();
        b.caveats.insert(0, "Datos simulados (INVEST_AI_FAKE): no son OPV reales.".into());
        Ok(b)
    }
}
