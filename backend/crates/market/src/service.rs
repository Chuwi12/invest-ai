//! `MarketService`: implementa `domain::MarketData` sobre todos los proveedores.
//!
//! - **Enrutado** por clase de activo y capacidad, con fallback ordenado (`order_for`).
//! - **Caché** moka con TTL por tipo de dato y **single-flight** (`try_get_with` agrupa las
//!   peticiones concurrentes de la misma clave en una sola llamada al proveedor).
//! - **Rate limiting** por proveedor (token bucket) y pausa automática tras un 429.
//! - **Estado** real de cada proveedor (último éxito/error, latencia).
//! - **Hub de tiempo real** (ver `realtime`).

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{
    AssetClass, AssetId, AssetSnapshot, AssetSummary, Candle, DomainError, Fundamentals,
    Indicators, Interval, MacroContext, MarketData, NewsItem, PriceTick, ProviderStatus, Quote,
    Range,
};
use futures::{FutureExt, StreamExt, stream::BoxStream};
use moka::future::Cache;
use tokio_util::sync::CancellationToken;

use crate::{
    config::MarketConfig,
    http::build_client,
    indicators,
    news::{self, Relevance},
    options::OptionsChain,
    providers::{
        Capability, NewsQuery, PResult, Provider, ProviderError, SearchHit,
        alphavantage::AlphaVantage,
        binance::{self, Binance, BinanceFeed},
        coingecko::CoinGecko,
        cryptopanic::CryptoPanic,
        finnhub::{Finnhub, FinnhubFeed},
        fmp::Fmp,
        fred::Fred,
        gdelt::Gdelt,
        google_news::GoogleNews,
        marketaux::Marketaux,
        newsapi::NewsApi,
        plain_ticker,
        sec::Sec,
        twelvedata::TwelveData,
        us_symbol,
        yahoo::Yahoo,
    },
    ratelimit::RateLimiter,
    realtime::{Hub, HubBuilder, QuoteFn},
    secret::{Secret, redact},
    symbol,
};

/// Espera máxima por un token del rate limiter antes de pasar al siguiente proveedor.
const MAX_LIMITER_WAIT: Duration = Duration::from_secs(3);
/// Tope por operación de proveedor (una operación puede encadenar varias peticiones HTTP).
const OP_TIMEOUT: Duration = Duration::from_secs(25);
/// Pausa mínima de un proveedor tras un 429 sin `Retry-After`.
const DEFAULT_BLOCK: Duration = Duration::from_secs(30);
const MAX_BLOCK: Duration = Duration::from_secs(300);
const MAX_QUERY_LEN: usize = 64;

// ---------------------------------------------------------------------------------------------
// Proveedor con límite de ritmo y estado
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct SlotState {
    last_ok: Option<DateTime<Utc>>,
    last_error: Option<String>,
    last_latency_ms: Option<u64>,
    blocked_until: Option<Instant>,
}

struct Slot {
    provider: Arc<dyn Provider>,
    limiter: RateLimiter,
    state: Mutex<SlotState>,
    secrets: Arc<Vec<Secret>>,
}

impl Slot {
    fn new(provider: Arc<dyn Provider>, secrets: Arc<Vec<Secret>>) -> Self {
        let info = provider.info();
        Self {
            limiter: RateLimiter::new(info.rate_per_sec, info.burst),
            provider,
            state: Mutex::new(SlotState::default()),
            secrets,
        }
    }

    fn name(&self) -> &'static str {
        self.provider.info().name
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Ejecuta una operación respetando bloqueo, límite de ritmo y timeout, y anota el resultado.
    async fn run<T, F, Fut>(&self, f: &F) -> PResult<T>
    where
        F: Fn(Arc<dyn Provider>) -> Fut,
        Fut: Future<Output = PResult<T>>,
    {
        {
            let st = self.state();
            if let Some(until) = st.blocked_until {
                let now = Instant::now();
                if until > now {
                    return Err(ProviderError::RateLimited {
                        retry_after: Some(until - now),
                    });
                }
            }
        }
        if self.limiter.acquire(MAX_LIMITER_WAIT).await.is_err() {
            return Err(ProviderError::RateLimited { retry_after: None });
        }
        let t0 = Instant::now();
        let res = match tokio::time::timeout(OP_TIMEOUT, f(self.provider.clone())).await {
            Ok(r) => r,
            Err(_) => Err(ProviderError::upstream("timeout")),
        };
        let ms = t0.elapsed().as_millis() as u64;
        let mut st = self.state();
        match &res {
            Ok(_) | Err(ProviderError::NotFound(_)) => {
                st.last_ok = Some(Utc::now());
                st.last_error = None;
                st.last_latency_ms = Some(ms);
            }
            Err(ProviderError::Unsupported) => {}
            Err(ProviderError::RateLimited { retry_after }) => {
                st.last_error = Some("límite de peticiones alcanzado".into());
                st.blocked_until =
                    Some(Instant::now() + retry_after.unwrap_or(DEFAULT_BLOCK).min(MAX_BLOCK));
            }
            Err(ProviderError::Upstream(m)) => {
                st.last_error = Some(redact(m, &self.secrets));
                st.last_latency_ms = Some(ms);
            }
        }
        res
    }
}

/// Salud de un proveedor (más detalle que `ProviderStatus`, que fija el dominio).
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub name: &'static str,
    pub enabled: bool,
    pub last_ok: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_latency_ms: Option<u64>,
    /// Segundos que quedan de pausa tras un 429.
    pub blocked_secs: Option<u64>,
}

// ---------------------------------------------------------------------------------------------
// Enrutado
// ---------------------------------------------------------------------------------------------

/// Proveedores a probar, por orden, para una clase y capacidad.
pub fn order_for(class: AssetClass, cap: Capability) -> &'static [&'static str] {
    use AssetClass as C;
    use Capability as K;
    match (cap, class) {
        (K::Quote, C::Crypto) => &["binance", "coingecko", "yahoo", "twelvedata"],
        (K::Quote, C::Stock | C::Etf) => &["yahoo", "finnhub", "twelvedata", "fmp", "alphavantage"],
        (K::Quote, C::Forex) => &["yahoo", "twelvedata"],
        (K::Quote, C::Bond) => &["fred", "yahoo"],
        (K::Quote, _) => &["yahoo"],
        (K::History, C::Crypto) => &["binance", "yahoo", "twelvedata", "coingecko"],
        (K::History, C::Stock | C::Etf) => &["yahoo", "twelvedata", "alphavantage"],
        (K::History, C::Forex) => &["yahoo", "twelvedata"],
        (K::History, C::Bond) => &["yahoo", "fred"],
        (K::History, _) => &["yahoo"],
        (K::Fundamentals, C::Crypto) => &["coingecko"],
        (K::Fundamentals, C::Stock | C::Etf) => &["yahoo", "finnhub", "fmp", "alphavantage"],
        (K::Fundamentals, _) => &["yahoo"],
        (K::News, C::Stock) => &[
            "google_news",
            "gdelt",
            "yahoo",
            "sec",
            "finnhub",
            "marketaux",
            "alphavantage",
            "newsapi",
        ],
        (K::News, C::Crypto) => &[
            "google_news",
            "gdelt",
            "yahoo",
            "cryptopanic",
            "marketaux",
            "newsapi",
        ],
        (K::News, _) => &["google_news", "gdelt", "yahoo", "newsapi"],
        _ => &[],
    }
}

pub(crate) fn clone_err(e: &DomainError) -> DomainError {
    match e {
        DomainError::NotFound(m) => DomainError::NotFound(m.clone()),
        DomainError::InvalidInput(m) => DomainError::InvalidInput(m.clone()),
        DomainError::Upstream { provider, message } => DomainError::Upstream {
            provider: provider.clone(),
            message: message.clone(),
        },
        DomainError::RateLimited { provider } => DomainError::RateLimited {
            provider: provider.clone(),
        },
        DomainError::Unavailable(m) => DomainError::Unavailable(m.clone()),
        DomainError::Internal(m) => DomainError::Internal(m.clone()),
    }
}

async fn cached<T, Fut>(cache: &Cache<String, T>, key: String, fut: Fut) -> domain::Result<T>
where
    T: Clone + Send + Sync + 'static,
    Fut: Future<Output = domain::Result<T>>,
{
    cache
        .try_get_with(key, fut)
        .await
        .map_err(|e: Arc<DomainError>| clone_err(&e))
}

fn cache<T: Clone + Send + Sync + 'static>(ttl: Duration, cap: u64) -> Cache<String, T> {
    Cache::builder().time_to_live(ttl).max_capacity(cap).build()
}

// ---------------------------------------------------------------------------------------------
// Núcleo
// ---------------------------------------------------------------------------------------------

struct Inner {
    slots: Vec<Slot>,
    yahoo: Arc<Yahoo>,
    sec: Arc<Sec>,
    quotes: Cache<String, Quote>,
    history_intraday: Cache<String, Arc<Vec<Candle>>>,
    history_daily: Cache<String, Arc<Vec<Candle>>>,
    indicators: Cache<String, Indicators>,
    fundamentals: Cache<String, Option<Fundamentals>>,
    news: Cache<String, Arc<Vec<NewsItem>>>,
    macros: Cache<String, MacroContext>,
    search: Cache<String, Arc<Vec<AssetSummary>>>,
    summaries: Cache<String, AssetSummary>,
    options: Cache<String, Arc<OptionsChain>>,
}

impl Inner {
    fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.name() == name)
    }

    fn to_domain(&self, name: &str, e: ProviderError) -> DomainError {
        let secrets = self.slot(name).map(|s| s.secrets.clone());
        match e {
            ProviderError::NotFound(m) => DomainError::NotFound(m),
            ProviderError::RateLimited { .. } => DomainError::RateLimited {
                provider: name.into(),
            },
            ProviderError::Unsupported => DomainError::Unavailable(format!("{name}: no soportado")),
            ProviderError::Upstream(m) => DomainError::Upstream {
                provider: name.into(),
                message: secrets.map_or(m.clone(), |s| redact(&m, &s)),
            },
        }
    }

    /// Prueba los proveedores en orden y devuelve el primer resultado aceptable.
    async fn route<T, F, Fut>(
        &self,
        what: &str,
        names: &[&str],
        f: F,
        accept: impl Fn(&T) -> bool,
    ) -> domain::Result<T>
    where
        F: Fn(Arc<dyn Provider>) -> Fut,
        Fut: Future<Output = PResult<T>>,
    {
        let mut not_found = false;
        let mut last: Option<DomainError> = None;
        for name in names {
            let Some(slot) = self.slot(name) else {
                continue;
            };
            if !slot.provider.info().enabled() {
                continue;
            }
            match slot.run(&f).await {
                Ok(v) if accept(&v) => return Ok(v),
                Ok(_) | Err(ProviderError::NotFound(_)) => not_found = true,
                Err(ProviderError::Unsupported) => {}
                Err(e) => {
                    tracing::debug!(provider = name, what, error = %e, "fallback al siguiente proveedor");
                    last = Some(self.to_domain(name, e));
                }
            }
        }
        if not_found {
            return Err(DomainError::NotFound(what.to_string()));
        }
        Err(last.unwrap_or_else(|| {
            DomainError::Unavailable(format!("ningún proveedor disponible para {what}"))
        }))
    }

    // ---- quotes -----------------------------------------------------------------------

    async fn quote(&self, id: &AssetId) -> domain::Result<Quote> {
        symbol::validate(id)?;
        cached(&self.quotes, id.to_string(), async {
            let id = id.clone();
            self.route(
                &format!("cotización de {id}"),
                order_for(id.class, Capability::Quote),
                |p| {
                    let id = id.clone();
                    async move { p.quote(&id).await }
                },
                |q: &Quote| q.price.is_finite(),
            )
            .await
        })
        .await
    }

    async fn quotes(&self, ids: &[AssetId]) -> Vec<Quote> {
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<&AssetId> = ids
            .iter()
            .filter(|i| symbol::validate(i).is_ok() && seen.insert((*i).clone()))
            .take(100)
            .collect();
        let mut found: HashMap<AssetId, Quote> = HashMap::new();
        let mut missing: Vec<AssetId> = Vec::new();
        for id in ids {
            match self.quotes.get(&id.to_string()).await {
                Some(q) => {
                    found.insert(id.clone(), q);
                }
                None => missing.push(id.clone()),
            }
        }
        // Un lote por proveedor: Binance para cripto, Yahoo para el resto.
        let (crypto, other): (Vec<AssetId>, Vec<AssetId>) = missing
            .iter()
            .cloned()
            .partition(|i| i.class == AssetClass::Crypto);
        for (provider, group) in [("binance", crypto), ("yahoo", other)] {
            if group.is_empty() {
                continue;
            }
            let Some(slot) = self.slot(provider) else {
                continue;
            };
            let batch = group.clone();
            if let Ok(qs) = slot
                .run(&|p: Arc<dyn Provider>| {
                    let batch = batch.clone();
                    async move { p.quotes(&batch).await }
                })
                .await
            {
                for q in qs {
                    if q.price.is_finite() && group.iter().any(|g| g == &q.id) {
                        self.quotes.insert(q.id.to_string(), q.clone()).await;
                        found.insert(q.id.clone(), q);
                    }
                }
            }
        }
        // Lo que el lote no resolvió, uno a uno (con fallback de proveedor) y concurrencia acotada.
        let rest: Vec<AssetId> = missing
            .into_iter()
            .filter(|m| !found.contains_key(m))
            .collect();
        let extra: Vec<Quote> = futures::stream::iter(rest)
            .map(|id| async move { self.quote(&id).await.ok() })
            .buffer_unordered(4)
            .filter_map(|q| async move { q })
            .collect()
            .await;
        for q in extra {
            found.insert(q.id.clone(), q);
        }
        found.into_values().collect()
    }

    // ---- historia / indicadores -------------------------------------------------------

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> domain::Result<Vec<Candle>> {
        symbol::validate(id)?;
        let key = format!("{id}|{}|{}", range.as_str(), interval.as_str());
        let cache = if interval.seconds() < Interval::D1.seconds() {
            &self.history_intraday
        } else {
            &self.history_daily
        };
        let arc = cached(cache, key, async {
            let id = id.clone();
            self.route(
                &format!("histórico de {id}"),
                order_for(id.class, Capability::History),
                |p| {
                    let id = id.clone();
                    async move { p.history(&id, range, interval).await }
                },
                |c: &Vec<Candle>| !c.is_empty(),
            )
            .await
            .map(Arc::new)
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn indicators(&self, id: &AssetId) -> domain::Result<Indicators> {
        symbol::validate(id)?;
        cached(&self.indicators, id.to_string(), async {
            let candles = self.history(id, Range::Y2, Interval::D1).await?;
            if candles.len() < 2 {
                return Err(DomainError::NotFound(format!(
                    "histórico insuficiente de {id}"
                )));
            }
            Ok(indicators::compute(&candles))
        })
        .await
    }

    // ---- fundamentales ----------------------------------------------------------------

    async fn fundamentals(&self, id: &AssetId) -> domain::Result<Option<Fundamentals>> {
        symbol::validate(id)?;
        if !matches!(
            id.class,
            AssetClass::Stock | AssetClass::Etf | AssetClass::Fund | AssetClass::Crypto
        ) {
            return Ok(None);
        }
        cached(&self.fundamentals, id.to_string(), async {
            let id = id.clone();
            let r = self
                .route(
                    &format!("fundamentales de {id}"),
                    order_for(id.class, Capability::Fundamentals),
                    |p| {
                        let id = id.clone();
                        async move { p.fundamentals(&id).await }
                    },
                    |f: &Option<Fundamentals>| f.is_some(),
                )
                .await;
            match r {
                Ok(f) => Ok(f),
                // Sin fundamentales no es un error: simplemente no los hay.
                Err(DomainError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }

    // ---- búsqueda y resumen -----------------------------------------------------------

    async fn search(&self, query: &str, limit: usize) -> domain::Result<Vec<AssetSummary>> {
        let q = query.trim();
        if q.is_empty() || q.chars().count() > MAX_QUERY_LEN || q.chars().any(char::is_control) {
            return Err(DomainError::InvalidInput(
                "consulta de búsqueda no válida".into(),
            ));
        }
        let limit = limit.clamp(1, 50);
        let key = format!("{}|{limit}", q.to_lowercase());
        let arc = cached(&self.search, key, async {
            let fetch = |name: &'static str| {
                let q = q.to_string();
                async move {
                    let Some(slot) = self.slot(name) else {
                        return Err(ProviderError::Unsupported);
                    };
                    if !slot.provider.info().enabled() {
                        return Err(ProviderError::Unsupported);
                    }
                    slot.run(&|p: Arc<dyn Provider>| {
                        let q = q.clone();
                        async move { p.search(&q, limit).await }
                    })
                    .await
                }
            };
            let (y, c) = futures::join!(fetch("yahoo"), fetch("coingecko"));
            let mut err = None;
            let mut best: HashMap<AssetId, SearchHit> = HashMap::new();
            for (name, r) in [("yahoo", y), ("coingecko", c)] {
                match r {
                    Ok(hits) => {
                        for h in hits {
                            match best.get(&h.summary.id) {
                                Some(old) if old.liquidity >= h.liquidity => {}
                                _ => {
                                    best.insert(h.summary.id.clone(), h);
                                }
                            }
                        }
                    }
                    Err(ProviderError::Unsupported | ProviderError::NotFound(_)) => {}
                    Err(e) => err = Some(self.to_domain(name, e)),
                }
            }
            if best.is_empty()
                && let Some(e) = err
            {
                return Err(e);
            }
            let up = q.to_ascii_uppercase();
            let mut hits: Vec<SearchHit> = best.into_values().collect();
            // Una coincidencia exacta de símbolo sube al principio.
            for h in &mut hits {
                if h.summary.symbol.to_ascii_uppercase() == up {
                    h.liquidity += 0.5;
                }
            }
            hits.sort_by(|a, b| {
                b.liquidity
                    .partial_cmp(&a.liquidity)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.summary.id.cmp(&b.summary.id))
            });
            hits.truncate(limit);
            Ok(Arc::new(
                hits.into_iter().map(|h| h.summary).collect::<Vec<_>>(),
            ))
        })
        .await?;
        Ok((*arc).clone())
    }

    /// Ficha del activo (nombre, mercado...). `NotFound` si ningún proveedor lo conoce.
    async fn summary(&self, id: &AssetId, quote: Option<&Quote>) -> domain::Result<AssetSummary> {
        symbol::validate(id)?;
        cached(&self.summaries, id.to_string(), async {
            let hits = self.search(&id.symbol, 10).await.unwrap_or_default();
            let found = hits.into_iter().find(|h| {
                h.id.class == id.class && h.symbol.eq_ignore_ascii_case(&id.symbol)
                    || h.id.class != id.class
                        && matches!(h.id.class, AssetClass::Stock | AssetClass::Etf)
                        && matches!(id.class, AssetClass::Stock | AssetClass::Etf)
                        && h.symbol.eq_ignore_ascii_case(&id.symbol)
            });
            if let Some(mut s) = found {
                s.id = id.clone();
                s.class = id.class;
                return Ok(s);
            }
            // La búsqueda no lo lista, pero si hay cotización el activo existe.
            let q = match quote {
                Some(q) => q.clone(),
                None => self.quote(id).await?,
            };
            Ok(AssetSummary {
                id: id.clone(),
                symbol: id.symbol.clone(),
                name: id.symbol.clone(),
                class: id.class,
                exchange: None,
                currency: q.currency,
                country: None,
                source: q.source,
            })
        })
        .await
    }

    // ---- noticias ---------------------------------------------------------------------

    async fn news(&self, id: Option<&AssetId>, limit: usize) -> domain::Result<Vec<NewsItem>> {
        if let Some(id) = id {
            symbol::validate(id)?;
        }
        let limit = limit.clamp(1, 50);
        let key = id.map_or_else(|| "general".to_string(), ToString::to_string);
        let arc = cached(&self.news, key, async {
            let name = match id {
                Some(id) => tokio::time::timeout(Duration::from_secs(6), self.summary(id, None))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(|s| s.name)
                    .filter(|n| !n.eq_ignore_ascii_case(&id.symbol)),
                None => None,
            };
            let q = NewsQuery {
                id: id.cloned(),
                name: name.clone(),
                limit: 50,
            };
            let class = id.map_or(AssetClass::Other, |i| i.class);
            let names = order_for(class, Capability::News);
            let futs = names
                .iter()
                .filter_map(|n| self.slot(n))
                .filter(|s| s.provider.info().enabled())
                .map(|slot| {
                    let q = q.clone();
                    async move {
                        let r = slot
                            .run(&|p: Arc<dyn Provider>| {
                                let q = q.clone();
                                async move { p.news(&q).await }
                            })
                            .await;
                        (slot.name(), r)
                    }
                });
            let results = futures::future::join_all(futs).await;
            let mut lists = Vec::new();
            let mut err = None;
            let mut answered = false;
            for (name, r) in results {
                match r {
                    Ok(v) => {
                        answered = true;
                        lists.push(v);
                    }
                    Err(ProviderError::Unsupported | ProviderError::NotFound(_)) => {}
                    Err(e) => err = Some(self.to_domain(name, e)),
                }
            }
            if !answered && let Some(e) = err {
                return Err(e);
            }
            let rel = id.map(|i| Relevance::new(i, name.as_deref()));
            Ok(Arc::new(news::merge(lists, rel.as_ref(), 50, Utc::now())))
        })
        .await?;
        Ok(arc.iter().take(limit).cloned().collect())
    }

    // ---- macro ------------------------------------------------------------------------

    async fn macro_context(&self) -> domain::Result<MacroContext> {
        cached(&self.macros, "macro".into(), async {
            let run = |name: &'static str| async move {
                let slot = self
                    .slot(name)
                    .filter(|s| s.provider.info().enabled())
                    .ok_or(ProviderError::Unsupported)?;
                slot.run(&|p: Arc<dyn Provider>| async move { p.macro_indicators().await })
                    .await
            };
            let (fred, yahoo) = futures::join!(run("fred"), run("yahoo"));
            let mut indicators = Vec::new();
            let mut err = None;
            for (name, r) in [("fred", fred), ("yahoo", yahoo)] {
                match r {
                    Ok(list) => {
                        for i in list {
                            // FRED va primero: es la fuente oficial; Yahoo solo completa.
                            if !indicators
                                .iter()
                                .any(|x: &domain::MacroIndicator| x.key == i.key)
                            {
                                indicators.push(i);
                            }
                        }
                    }
                    Err(ProviderError::Unsupported) => {}
                    Err(e) => err = Some(self.to_domain(name, e)),
                }
            }
            if indicators.is_empty() {
                return Err(
                    err.unwrap_or_else(|| DomainError::Unavailable("sin datos macro".into()))
                );
            }
            let as_of = indicators.iter().map(|i| i.as_of).max();
            Ok(MacroContext { indicators, as_of })
        })
        .await
    }

    // ---- snapshot ---------------------------------------------------------------------

    async fn snapshot(&self, id: &AssetId) -> domain::Result<AssetSnapshot> {
        symbol::validate(id)?;
        async fn part<T>(
            secs: u64,
            f: impl Future<Output = domain::Result<T>>,
        ) -> domain::Result<T> {
            tokio::time::timeout(Duration::from_secs(secs), f)
                .await
                .unwrap_or_else(|_| {
                    Err(DomainError::Unavailable("tiempo de espera agotado".into()))
                })
        }
        let (quote, indicators, fundamentals, news, macro_ctx, summary) = futures::join!(
            part(8, self.quote(id)),
            part(12, self.indicators(id)),
            part(12, self.fundamentals(id)),
            part(12, self.news(Some(id), 15)),
            part(8, self.macro_context()),
            part(8, self.summary(id, None)),
        );
        let mut missing = Vec::new();
        let mut note = |what: &str, e: &DomainError| missing.push(format!("{what}: {e}"));

        let summary = match (summary, &quote) {
            (Ok(s), _) => s,
            (Err(DomainError::NotFound(m)), Err(DomainError::NotFound(_))) => {
                return Err(DomainError::NotFound(format!(
                    "el activo {id} no existe ({m})"
                )));
            }
            (Err(e), q) => {
                note("ficha del activo", &e);
                AssetSummary {
                    id: id.clone(),
                    symbol: id.symbol.clone(),
                    name: id.symbol.clone(),
                    class: id.class,
                    exchange: None,
                    currency: q.as_ref().ok().and_then(|q| q.currency.clone()),
                    country: None,
                    source: "derived".into(),
                }
            }
        };
        let quote = quote.map_err(|e| note("cotización", &e)).ok();
        let indicators = indicators
            .map_err(|e| note("indicadores técnicos", &e))
            .ok();
        let fundamentals = match fundamentals {
            Ok(Some(f)) => Some(f),
            Ok(None) => {
                if matches!(
                    id.class,
                    AssetClass::Stock | AssetClass::Etf | AssetClass::Fund | AssetClass::Crypto
                ) {
                    note(
                        "fundamentales",
                        &DomainError::NotFound("ningún proveedor los tiene".into()),
                    );
                }
                None
            }
            Err(e) => {
                note("fundamentales", &e);
                None
            }
        };
        let news = news.map_err(|e| note("noticias", &e)).unwrap_or_default();
        let macro_context = macro_ctx.map_err(|e| note("contexto macro", &e)).ok();
        Ok(AssetSnapshot {
            summary,
            quote,
            fundamentals,
            indicators,
            news,
            macro_context,
            missing,
            generated_at: Utc::now(),
        })
    }

    // ---- extras fuera del contrato ----------------------------------------------------

    async fn options_chain(
        &self,
        id: &AssetId,
        expiration: Option<i64>,
    ) -> domain::Result<OptionsChain> {
        symbol::validate(id)?;
        let key = format!("{id}|{}", expiration.unwrap_or(0));
        let arc = cached(&self.options, key, async {
            let slot = self
                .slot("yahoo")
                .ok_or_else(|| DomainError::Unavailable("Yahoo no disponible".into()))?;
            let y = self.yahoo.clone();
            let id2 = id.clone();
            let _ = &y;
            slot.run(&|_p: Arc<dyn Provider>| {
                let y = y.clone();
                let id = id2.clone();
                async move { y.options_chain(&id, expiration).await }
            })
            .await
            .map(Arc::new)
            .map_err(|e| self.to_domain("yahoo", e))
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn filings(&self, id: &AssetId, limit: usize) -> domain::Result<Vec<NewsItem>> {
        symbol::validate(id)?;
        let slot = self
            .slot("sec")
            .ok_or_else(|| DomainError::Unavailable("SEC no disponible".into()))?;
        let sec = self.sec.clone();
        slot.run(&|_p: Arc<dyn Provider>| {
            let sec = sec.clone();
            let id = id.clone();
            async move { sec.filings(&id, limit.clamp(1, 20)).await }
        })
        .await
        .map_err(|e| self.to_domain("sec", e))
    }
}

// ---------------------------------------------------------------------------------------------
// Servicio público
// ---------------------------------------------------------------------------------------------

pub struct MarketService {
    inner: Arc<Inner>,
    hub: Hub,
    cancel: CancellationToken,
}

impl Drop for MarketService {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl MarketService {
    /// Crea el servicio. **Debe llamarse dentro de un runtime de Tokio** (arranca las tareas del
    /// hub de tiempo real).
    pub fn new(config: MarketConfig) -> anyhow::Result<Arc<Self>> {
        let http = build_client(config.request_timeout).map_err(|e| anyhow::anyhow!("{e}"))?;
        let secrets = Arc::new(config.secrets());
        if config.uses_default_sec_contact() {
            tracing::warn!(
                "SEC_USER_AGENT sin configurar: la SEC puede bloquear un contacto de ejemplo"
            );
        }
        let yahoo = Arc::new(Yahoo::new(http.clone()));
        let sec = Arc::new(Sec::new(http.clone(), config.sec_user_agent.clone()));
        let providers: Vec<Arc<dyn Provider>> = vec![
            yahoo.clone(),
            Arc::new(Binance::with_base(
                http.clone(),
                config.binance_rest_base.clone(),
            )),
            Arc::new(CoinGecko::new(http.clone(), config.coingecko_key.clone())),
            Arc::new(GoogleNews::new(http.clone(), config.news_languages.clone())),
            Arc::new(Gdelt::new(http.clone())),
            sec.clone(),
            Arc::new(Fred::new(http.clone(), config.fred_key.clone())),
            Arc::new(Finnhub::new(http.clone(), config.finnhub_key.clone())),
            Arc::new(AlphaVantage::new(
                http.clone(),
                config.alphavantage_key.clone(),
            )),
            Arc::new(Fmp::new(http.clone(), config.fmp_key.clone())),
            Arc::new(TwelveData::new(http.clone(), config.twelvedata_key.clone())),
            Arc::new(NewsApi::new(http.clone(), config.newsapi_key.clone())),
            Arc::new(Marketaux::new(http.clone(), config.marketaux_key.clone())),
            Arc::new(CryptoPanic::new(http, config.cryptopanic_key.clone())),
        ];
        let slots: Vec<Slot> = providers
            .into_iter()
            .map(|p| Slot::new(p, secrets.clone()))
            .collect();
        for s in &slots {
            let i = s.provider.info();
            if !i.enabled() {
                tracing::info!(provider = i.name, "proveedor desactivado (falta la clave)");
            }
        }
        let inner = Arc::new(Inner {
            slots,
            yahoo,
            sec,
            quotes: cache(Duration::from_secs(3), 5_000),
            history_intraday: cache(Duration::from_secs(60), 500),
            history_daily: cache(Duration::from_secs(15 * 60), 500),
            indicators: cache(Duration::from_secs(15 * 60), 1_000),
            fundamentals: cache(Duration::from_secs(6 * 3600), 1_000),
            news: cache(Duration::from_secs(3 * 60), 500),
            macros: cache(Duration::from_secs(3600), 4),
            search: cache(Duration::from_secs(10 * 60), 1_000),
            summaries: cache(Duration::from_secs(24 * 3600), 5_000),
            options: cache(Duration::from_secs(30), 100),
        });

        let cancel = CancellationToken::new();
        let quote_fn: QuoteFn = {
            let inner = inner.clone();
            Arc::new(move |id: AssetId| {
                let inner = inner.clone();
                async move { inner.quote(&id).await.ok() }.boxed()
            })
        };
        let mut builder = HubBuilder::new();
        if config.realtime {
            builder = builder.feed(
                BinanceFeed {
                    ws_base: config.binance_ws_base.clone(),
                },
                |id: &AssetId| binance::pair(id),
                |sym: &str| {
                    binance::base_of(sym)
                        .map(|b| vec![AssetId::new(AssetClass::Crypto, b)])
                        .unwrap_or_default()
                },
            );
            if let Some(key) = config.finnhub_key.clone() {
                builder = builder.feed(
                    FinnhubFeed::new(key),
                    |id: &AssetId| us_symbol(id),
                    |sym: &str| {
                        let s = sym.replace('.', "-");
                        vec![
                            AssetId::new(AssetClass::Stock, s.clone()),
                            AssetId::new(AssetClass::Etf, s),
                        ]
                    },
                );
            }
        }
        let hub = builder.build(quote_fn, cancel.clone());
        Ok(Arc::new(Self { inner, hub, cancel }))
    }

    /// Cadena de opciones (Yahoo). Fuera del contrato `MarketData`.
    pub async fn options_chain(
        &self,
        id: &AssetId,
        expiration: Option<i64>,
    ) -> domain::Result<OptionsChain> {
        self.inner.options_chain(id, expiration).await
    }

    /// Últimos filings de la SEC (10-K, 10-Q, 8-K...) de una acción de EE. UU.
    pub async fn filings(&self, id: &AssetId, limit: usize) -> domain::Result<Vec<NewsItem>> {
        self.inner.filings(id, limit).await
    }

    /// Ficha del activo (nombre, mercado...).
    pub async fn summary(&self, id: &AssetId) -> domain::Result<AssetSummary> {
        self.inner.summary(id, None).await
    }

    /// Salud detallada de los proveedores (latencia, pausa por 429...).
    pub fn provider_health(&self) -> Vec<ProviderHealth> {
        self.inner
            .slots
            .iter()
            .map(|s| {
                let st = s.state();
                ProviderHealth {
                    name: s.name(),
                    enabled: s.provider.info().enabled(),
                    last_ok: st.last_ok,
                    last_error: st.last_error.clone(),
                    last_latency_ms: st.last_latency_ms,
                    blocked_secs: st
                        .blocked_until
                        .and_then(|u| u.checked_duration_since(Instant::now()))
                        .map(|d| d.as_secs()),
                }
            })
            .collect()
    }

    /// Estado de los WebSockets (nombre, conectado).
    pub fn realtime_feeds(&self) -> Vec<(&'static str, bool)> {
        self.hub.feed_status()
    }

    /// Página pública para verificar el dato de un proveedor sobre un activo.
    pub fn source_url(provider: &str, id: &AssetId) -> Option<String> {
        symbol::verify_url(provider, id)
    }

    /// Ticker sin sufijos de bolsa (`SAN.MC` → `SAN`).
    pub fn plain_ticker(symbol: &str) -> String {
        plain_ticker(symbol)
    }
}

#[async_trait]
impl MarketData for MarketService {
    async fn search(&self, query: &str, limit: usize) -> domain::Result<Vec<AssetSummary>> {
        self.inner.search(query, limit).await
    }

    async fn quote(&self, id: &AssetId) -> domain::Result<Quote> {
        self.inner.quote(id).await
    }

    async fn quotes(&self, ids: &[AssetId]) -> domain::Result<Vec<Quote>> {
        Ok(self.inner.quotes(ids).await)
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> domain::Result<Vec<Candle>> {
        self.inner.history(id, range, interval).await
    }

    async fn fundamentals(&self, id: &AssetId) -> domain::Result<Option<Fundamentals>> {
        self.inner.fundamentals(id).await
    }

    async fn indicators(&self, id: &AssetId) -> domain::Result<Indicators> {
        self.inner.indicators(id).await
    }

    async fn news(&self, id: Option<&AssetId>, limit: usize) -> domain::Result<Vec<NewsItem>> {
        self.inner.news(id, limit).await
    }

    async fn macro_context(&self) -> domain::Result<MacroContext> {
        self.inner.macro_context().await
    }

    async fn snapshot(&self, id: &AssetId) -> domain::Result<AssetSnapshot> {
        self.inner.snapshot(id).await
    }

    fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        let valid: Vec<AssetId> = ids
            .iter()
            .filter(|i| symbol::validate(i).is_ok())
            .cloned()
            .collect();
        self.hub.subscribe(&valid)
    }

    fn providers_status(&self) -> Vec<ProviderStatus> {
        self.inner
            .slots
            .iter()
            .map(|s| {
                let info = s.provider.info();
                let st = s.state();
                ProviderStatus {
                    name: info.name.to_string(),
                    enabled: info.enabled(),
                    requires_key: info.requires_key,
                    has_key: info.has_key,
                    capabilities: info
                        .capabilities
                        .iter()
                        .map(|c| c.as_str().to_string())
                        .collect(),
                    asset_classes: info.classes.clone(),
                    last_error: st.last_error.clone(),
                    last_ok: st.last_ok,
                }
            })
            .collect()
    }
}
