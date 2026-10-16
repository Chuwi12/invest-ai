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
    DomainError, Fundamentals, Horizon, Indicators, Interval, KeyLevels, LlmStatus, MacroContext,
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
