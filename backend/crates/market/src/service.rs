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
    IpoEntry, IpoPerformance, IpoStatus, Prospectus, Indicators, Interval, MacroContext, MarketData, NewsItem, PriceTick, ProviderStatus, Quote,
    Range, TimestampOrigin,
};
use futures::{FutureExt, StreamExt, stream::BoxStream};
use moka::future::Cache;
use tokio_util::sync::CancellationToken;

use crate::{
    config::MarketConfig,
    http::{build_client, build_slow_client},
    indicators,
    news::{self, Relevance},
    options::OptionsChain,
    provenance,
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
        sec::{ProspectusPick, Sec},
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
/// Timeouts consecutivos tras los que se aparta a un proveedor (sin esto, uno caído añade su
/// timeout completo a cada petición en frío: GDELT medía 6 s por noticia).
const TIMEOUT_STRIKES: u32 = 2;
/// Una operación cancelada tras tanto tiempo se considera un timeout.
const SLOW_CANCEL: Duration = Duration::from_millis(1000);
const TIMEOUT_BLOCK: Duration = Duration::from_secs(60);
/// Tras la primera lista de noticias válida, cuánto se espera al resto de proveedores.
const NEWS_GRACE: Duration = Duration::from_millis(1500);
/// Proveedores de noticias demasiado lentos para esperarlos (GDELT: 8-10 s solo en el apretón de
/// manos TLS, 11-15 s por petición, medido el 13-oct-2026). No cuentan para la latencia de la
/// respuesta: se lanzan en segundo plano, terminan solos y sus noticias se fusionan en las lecturas
/// siguientes de la misma clave (`late_news`).
const BACKGROUND_NEWS: &[&str] = &["gdelt"];
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
    /// Timeouts seguidos; al llegar a `TIMEOUT_STRIKES` se aparta al proveedor un rato.
    timeouts_in_a_row: u32,
}

impl SlotState {
    /// Anota un timeout; al llegar a `TIMEOUT_STRIKES` seguidos aparta al proveedor un rato.
    fn strike(&mut self) {
        self.timeouts_in_a_row += 1;
        if self.timeouts_in_a_row >= TIMEOUT_STRIKES {
            self.blocked_until = Some(Instant::now() + TIMEOUT_BLOCK);
        }
    }
}

/// Si una operación descartable (`run_discardable`: `collect_with_grace` descarta al proveedor lento) se cancela tras
/// llevar mucho tiempo en vuelo, cuenta como timeout: si no, el lento nunca acumularía fallos.
struct CancelGuard<'a> {
    slot: &'a Slot,
    t0: Instant,
    armed: bool,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if self.armed && self.t0.elapsed() >= SLOW_CANCEL {
            self.slot.state().strike();
        }
    }
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
    /// Si el futuro se suelta a medias (el cliente abortó la petición) NO cuenta como timeout.
    async fn run<T, F, Fut>(&self, f: &F) -> PResult<T>
    where
        F: Fn(Arc<dyn Provider>) -> Fut,
        Fut: Future<Output = PResult<T>>,
    {
        self.run_inner(f, false).await
    }

    /// Como `run`, para operaciones que `collect_with_grace` puede descartar por lentas: si se
    /// sueltan tras mucho tiempo en vuelo cuentan como timeout (si no, el lento nunca acumularía
    /// fallos). Solo ahí es seguro suponer que la cancelación es culpa del proveedor.
    async fn run_discardable<T, F, Fut>(&self, f: &F) -> PResult<T>
    where
        F: Fn(Arc<dyn Provider>) -> Fut,
        Fut: Future<Output = PResult<T>>,
    {
        self.run_inner(f, true).await
    }

    async fn run_inner<T, F, Fut>(&self, f: &F, strike_on_cancel: bool) -> PResult<T>
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
        let mut guard = CancelGuard { slot: self, t0, armed: strike_on_cancel };
        let res = match tokio::time::timeout(OP_TIMEOUT, f(self.provider.clone())).await {
            Ok(r) => r,
            Err(_) => Err(ProviderError::upstream("timeout")),
        };
        guard.armed = false;
        let ms = t0.elapsed().as_millis() as u64;
        let mut st = self.state();
        match &res {
            Ok(_) | Err(ProviderError::NotFound(_)) => {
                st.timeouts_in_a_row = 0;
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
                if m == "timeout" {
                    st.strike();
                } else {
                    st.timeouts_in_a_row = 0;
                }
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

/// Rellena la URL de verificación de una cotización a partir de su proveedor.
fn with_quote_url(mut q: Quote) -> Quote {
    if q.source_url.is_none() {
        q.source_url = symbol::verify_url(&q.source, &q.id);
    }
    q
}

/// Rellena la URL de verificación y el origen de la fecha de unos fundamentales.
fn with_fundamentals_origin(mut f: Fundamentals, id: &AssetId) -> Fundamentals {
    let first = f.sources.first().cloned();
    if f.source_url.is_none() {
        f.source_url = first.as_deref().and_then(|p| symbol::verify_url(p, id));
    }
    if f.timestamp_origin.is_none() && f.as_of.is_some() {
        // Yahoo y Finnhub no fechan el cálculo: `as_of` es la hora de la consulta.
        f.timestamp_origin = Some(match first.as_deref() {
            Some("yahoo" | "finnhub") | None => TimestampOrigin::Local,
            Some(_) => TimestampOrigin::Provider,
        });
    }
    f
}

fn with_summary_url(mut s: AssetSummary) -> AssetSummary {
    if s.source_url.is_none() {
        s.source_url = symbol::verify_url(&s.source, &s.id);
    }
    s
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

/// Recoge los resultados de varios proveedores a la vez. En cuanto uno responde bien, los demás
/// tienen `grace` para llegar; los que no, se descartan (se cancelan al soltar el futuro). Así un
/// proveedor lento no fija la latencia de toda la respuesta.
async fn collect_with_grace<T, F>(futs: impl IntoIterator<Item = F>, grace: Duration) -> Vec<(&'static str, PResult<T>)>
where
    F: Future<Output = (&'static str, PResult<T>)>,
{
    use futures::stream::FuturesUnordered;
    let mut pending: FuturesUnordered<F> = futs.into_iter().collect();
    let mut out = Vec::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let next = match deadline {
            Some(d) => match tokio::time::timeout_at(d, pending.next()).await {
                Ok(n) => n,
                Err(_) => break,
            },
            None => pending.next().await,
        };
        let Some(item) = next else { break };
        if item.1.is_ok() && deadline.is_none() {
            deadline = Some(tokio::time::Instant::now() + grace);
        }
        out.push(item);
    }
    out
}

fn cache<T: Clone + Send + Sync + 'static>(ttl: Duration, cap: u64) -> Cache<String, T> {
    Cache::builder().time_to_live(ttl).max_capacity(cap).build()
}

// ---------------------------------------------------------------------------------------------
// Núcleo
// ---------------------------------------------------------------------------------------------

/// Velas y el proveedor que las sirvió.
struct HistEntry {
    source: String,
    candles: Vec<Candle>,
}

struct Inner {
    slots: Vec<Arc<Slot>>,
    yahoo: Arc<Yahoo>,
    sec: Arc<Sec>,
    finnhub: Arc<Finnhub>,
    ipo_calendar: Cache<String, Arc<Vec<IpoEntry>>>,
    prospectuses: Cache<String, Arc<Prospectus>>,
    ipo_perf: Cache<String, Arc<IpoPerformance>>,
    quotes: Cache<String, Quote>,
    /// Lotes de cotizaciones en vuelo (vida de 200 ms): peticiones iguales y simultáneas comparten una.
    quote_batches: Cache<String, Arc<Vec<Quote>>>,
    history_intraday: Cache<String, Arc<HistEntry>>,
    history_daily: Cache<String, Arc<HistEntry>>,
    indicators: Cache<String, Indicators>,
    fundamentals: Cache<String, Option<Fundamentals>>,
    news: Cache<String, Arc<Vec<NewsItem>>>,
    /// Noticias de los proveedores en segundo plano, ya filtradas por relevancia, por clave de `news`.
    late_news: Cache<String, Arc<Vec<NewsItem>>>,
    macros: Cache<String, MacroContext>,
    search: Cache<String, Arc<Vec<AssetSummary>>>,
    summaries: Cache<String, AssetSummary>,
    options: Cache<String, Arc<OptionsChain>>,
}

impl Inner {
    fn slot(&self, name: &str) -> Option<&Arc<Slot>> {
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
        self.route_named(what, names, f, accept).await.map(|(_, v)| v)
    }

    /// Como `route`, y dice qué proveedor respondió.
    async fn route_named<T, F, Fut>(
        &self,
        what: &str,
        names: &[&str],
        f: F,
        accept: impl Fn(&T) -> bool,
    ) -> domain::Result<(String, T)>
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
                Ok(v) if accept(&v) => return Ok(((*name).to_string(), v)),
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
            .map(with_quote_url)
        })
        .await
    }

    /// Cotizaciones de varios activos; las llamadas simultáneas con el mismo conjunto de ids
    /// comparten una sola ejecución (single-flight), como las cachés de un solo recurso.
    async fn quotes(&self, ids: &[AssetId]) -> Vec<Quote> {
        let mut key: Vec<String> = ids.iter().map(ToString::to_string).collect();
        key.sort_unstable();
        key.dedup();
        let batch = self.quote_batches.get_with(key.join(","), async { Arc::new(self.quotes_uncached(ids).await) }).await;
        (*batch).clone()
    }

    async fn quotes_uncached(&self, ids: &[AssetId]) -> Vec<Quote> {
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
                        let q = with_quote_url(q);
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
        Ok(self.history_src(id, range, interval).await?.1)
    }

    /// Histórico y proveedor que lo sirvió (queda anotado en `provenance`).
    async fn history_src(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> domain::Result<(String, Vec<Candle>)> {
        symbol::validate(id)?;
        let key = format!("{id}|{}|{}", range.as_str(), interval.as_str());
        let cache = if interval.seconds() < Interval::D1.seconds() {
            &self.history_intraday
        } else {
            &self.history_daily
        };
        let entry = cached(cache, key, async {
            let id = id.clone();
            self.route_named(
                &format!("histórico de {id}"),
                order_for(id.class, Capability::History),
                |p| {
                    let id = id.clone();
                    async move { p.history(&id, range, interval).await }
                },
                |c: &Vec<Candle>| !c.is_empty(),
            )
            .await
            .map(|(source, candles)| Arc::new(HistEntry { source, candles }))
        })
        .await?;
        provenance::record_history(
            id,
            range,
            interval,
            provenance::HistoryOrigin {
                source: entry.source.clone(),
                source_url: symbol::verify_url(&entry.source, id),
                as_of: entry
                    .candles
                    .last()
                    .and_then(|c| DateTime::<Utc>::from_timestamp(c.t, 0)),
                interval: provenance::infer_interval(&entry.candles),
            },
        );
        Ok((entry.source.clone(), entry.candles.clone()))
    }

    async fn indicators(&self, id: &AssetId) -> domain::Result<Indicators> {
        symbol::validate(id)?;
        cached(&self.indicators, id.to_string(), Box::pin(async {
            let (source, candles) = self.history_src(id, Range::Y2, Interval::D1).await?;
            if candles.len() < 2 {
                return Err(DomainError::NotFound(format!(
                    "histórico insuficiente de {id}"
                )));
            }
            let mut ind = indicators::compute(&candles);
            ind.source_url = symbol::verify_url(&source, id);
            ind.source = Some(source);
            Ok(ind)
        }))
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
        cached(&self.fundamentals, id.to_string(), Box::pin(async {
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
                Ok(f) => Ok(f.map(|f| with_fundamentals_origin(f, &id))),
                // Sin fundamentales no es un error: simplemente no los hay.
                Err(DomainError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            }
        }))
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
        let arc = cached(&self.search, key, Box::pin(async {
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
                hits.into_iter()
                    .map(|h| with_summary_url(h.summary))
                    .collect::<Vec<_>>(),
            ))
        }))
        .await?;
        Ok((*arc).clone())
    }

    /// Ficha del activo (nombre, mercado...). `NotFound` si ningún proveedor lo conoce.
    async fn summary(&self, id: &AssetId, quote: Option<&Quote>) -> domain::Result<AssetSummary> {
        symbol::validate(id)?;
        cached(&self.summaries, id.to_string(), Box::pin(async {
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
                return Ok(with_summary_url(s));
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
                source_url: symbol::verify_url(&q.source, id),
                source: q.source,
            })
        }))
        .await
    }

    // ---- noticias ---------------------------------------------------------------------

    async fn news(&self, id: Option<&AssetId>, limit: usize) -> domain::Result<Vec<NewsItem>> {
        if let Some(id) = id {
            symbol::validate(id)?;
        }
        let limit = limit.clamp(1, 50);
        let key = id.map_or_else(|| "general".to_string(), ToString::to_string);
        let arc = cached(&self.news, key.clone(), Box::pin(async {
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
            let rel = id.map(|i| Relevance::new(i, name.as_deref()));
            let (background, foreground): (Vec<&Arc<Slot>>, Vec<&Arc<Slot>>) = order_for(class, Capability::News)
                .iter()
                .filter_map(|n| self.slot(n))
                .filter(|s| s.provider.info().enabled())
                .partition(|s| BACKGROUND_NEWS.contains(&s.name()));
            // Los lentos corren por su cuenta: no retrasan esta respuesta ni se cancelan al
            // cerrarse la ventana de gracia (si no, nunca llegarían a contestar).
            for slot in background {
                self.spawn_late_news(slot.clone(), q.clone(), key.clone(), rel.clone());
            }
            let futs = foreground.into_iter().map(|slot| {
                let q = q.clone();
                async move {
                    let r = slot
                        .run_discardable(&|p: Arc<dyn Provider>| {
                            let q = q.clone();
                            async move { p.news(&q).await }
                        })
                        .await;
                    (slot.name(), r)
                }
            });
            let results = collect_with_grace(futs, NEWS_GRACE).await;
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
            Ok(Arc::new(news::merge(lists, rel.as_ref(), 50, Utc::now())))
        }))
        .await;
        let arc = match arc {
            Ok(arc) => arc,
            // Si fallan todos los de primer plano pero un proveedor en segundo plano ya trajo noticias de
            // esta clave, se sirven esas en vez de un error.
            Err(e) => match self.late_news.get(&key).await {
                Some(late) if !late.is_empty() => return Ok(late.iter().take(limit).cloned().collect()),
                _ => return Err(e),
            },
        };
        // Noticias de los proveedores en segundo plano que ya han llegado (ya filtradas).
        let list = match self.late_news.get(&key).await {
            Some(late) => Arc::new(news::merge(vec![arc.to_vec(), late.to_vec()], None, 50, Utc::now())),
            None => arc,
        };
        Ok(list.iter().take(limit).cloned().collect())
    }

    /// Lanza a un proveedor lento en una tarea propia: al terminar con éxito deja sus noticias en
    /// `late_news` (fusionadas con las que ya hubiera de otros proveedores en segundo plano). El
    /// fallo o el timeout los anota el propio `Slot` (circuit breaker), igual que en primer plano.
    fn spawn_late_news(&self, slot: Arc<Slot>, q: NewsQuery, key: String, rel: Option<Relevance>) {
        let late = self.late_news.clone();
        tokio::spawn(async move {
            let r = slot
                .run(&|p: Arc<dyn Provider>| {
                    let q = q.clone();
                    async move { p.news(&q).await }
                })
                .await;
            match r {
                Ok(list) => {
                    let mut lists = vec![list];
                    if let Some(prev) = late.get(&key).await {
                        lists.push(prev.to_vec());
                    }
                    let kept = news::merge(lists, rel.as_ref(), 50, Utc::now());
                    tracing::debug!(provider = slot.name(), items = kept.len(), "noticias del proveedor lento");
                    if !kept.is_empty() {
                        late.insert(key, Arc::new(kept)).await;
                    }
                }
                Err(e) => tracing::debug!(provider = slot.name(), error = %e, "proveedor lento sin noticias"),
            }
        });
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
        // Cada parte va en el montón (`Box::pin`): sin eso, el estado de las seis (206 KB en debug,
        // 54 KB solo `news`) viaja por valor dentro del `join!`, del `timeout` y del futuro
        // encajado por `async_trait`, y en debug cada traslado es una copia en la pila. Con los
        // 2 MiB de un worker de Tokio el servidor abortaba por desbordamiento de pila.
        let (quote, indicators, fundamentals, news, macro_ctx, summary) = futures::join!(
            part(8, Box::pin(self.quote(id))),
            part(12, Box::pin(self.indicators(id))),
            part(12, Box::pin(self.fundamentals(id))),
            part(12, Box::pin(self.news(Some(id), 15))),
            part(8, Box::pin(self.macro_context())),
            part(8, Box::pin(self.summary(id, None))),
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
                    source_url: None,
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


    // ---- OPV (IPO) ---------------------------------------------------------------------

    /// Calendario de OPV: Finnhub (plan gratuito) y, si no hay clave o falla, folletos
    /// definitivos recientes del índice de EDGAR (sin precio ni símbolo).
    async fn ipo_calendar(&self, from: chrono::NaiveDate, to: chrono::NaiveDate) -> domain::Result<Vec<IpoEntry>> {
        if from > to || (to - from).num_days() > 730 {
            return Err(DomainError::InvalidInput("rango de fechas no válido (máx. 2 años)".into()));
        }
        let key = format!("{from}|{to}");
        let arc = cached(&self.ipo_calendar, key, async {
            let fh = self.slot("finnhub").filter(|s| s.provider.info().enabled());
            let mut last: Option<DomainError> = None;
            if let Some(slot) = fh {
                let f = self.finnhub.clone();
                match slot.run(&|_p: Arc<dyn Provider>| { let f = f.clone(); async move { f.ipo_calendar(from, to).await } }).await {
                    Ok(v) => return Ok(Arc::new(v)),
                    Err(e) => last = Some(self.to_domain("finnhub", e)),
                }
            }
            let slot = self.slot("sec").ok_or_else(|| DomainError::Unavailable("SEC no disponible".into()))?;
            let sec = self.sec.clone();
            match slot.run(&|_p: Arc<dyn Provider>| { let s = sec.clone(); async move { s.recent_final_prospectuses(from, to).await } }).await {
                Ok(v) => Ok(Arc::new(v)),
                Err(e) => Err(last.unwrap_or_else(|| self.to_domain("sec", e))),
            }
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn ipo_prospectus(&self, issuer: &str, pick: ProspectusPick, max_chars: usize) -> domain::Result<Prospectus> {
        let issuer = issuer.trim();
        if issuer.is_empty() || issuer.len() > 80 || issuer.chars().any(char::is_control) {
            return Err(DomainError::InvalidInput("emisor no válido".into()));
        }
        let key = format!("{}|{pick:?}|{max_chars}", issuer.to_ascii_lowercase());
        let arc = cached(&self.prospectuses, key, async {
            let slot = self.slot("sec").ok_or_else(|| DomainError::Unavailable("SEC no disponible".into()))?;
            let sec = self.sec.clone();
            let issuer = issuer.to_string();
            slot.run(&|_p: Arc<dyn Provider>| { let s = sec.clone(); let i = issuer.clone(); async move { s.prospectus(&i, pick, max_chars).await } })
                .await
                .map(Arc::new)
                .map_err(|e| self.to_domain("sec", e))
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn daily_series(&self, symbol: &str, from: i64, to: i64) -> domain::Result<crate::providers::yahoo::DailySeries> {
        let slot = self.slot("yahoo").ok_or_else(|| DomainError::Unavailable("Yahoo no disponible".into()))?;
        let y = self.yahoo.clone();
        let sym = symbol.to_string();
        slot.run(&|_p: Arc<dyn Provider>| { let y = y.clone(); let s = sym.clone(); async move { y.daily_between(&s, from, to).await } })
            .await
            .map_err(|e| self.to_domain("yahoo", e))
    }

    async fn ipo_performance(
        &self,
        symbol: &str,
        ipo_date: chrono::NaiveDate,
        offer_price: Option<f64>,
        benchmark: &str,
    ) -> domain::Result<IpoPerformance> {
        let symbol = symbol.trim().to_ascii_uppercase();
        let bench = benchmark.trim().to_ascii_uppercase();
        if !crate::ipo::BENCHMARKS.contains(&bench.as_str()) {
            return Err(DomainError::InvalidInput(format!("índice no admitido (usa {:?})", crate::ipo::BENCHMARKS)));
        }
        symbol::validate(&AssetId::new(AssetClass::Stock, symbol.clone()))?;
        let key = format!("{symbol}|{ipo_date}|{offer_price:?}|{bench}");
        let arc = cached(&self.ipo_perf, key, async {
            // Precio de salida: el dado, si no el del calendario y, si no, el de la portada del folleto.
            let (offer, src) = match offer_price {
                Some(p) => (p, "manual"),
                None => match self.calendar_price(&symbol, ipo_date).await {
                    Some(p) => (p, "calendar"),
                    None => {
                        let pr = self.ipo_prospectus(&symbol, ProspectusPick::Initial, 1).await?;
                        (pr.offer_price.ok_or_else(|| DomainError::NotFound(format!("{symbol}: sin precio de OPV en el calendario ni en el folleto")))?, "prospectus")
                    }
                },
            };
            let from = ipo_date.and_hms_opt(0, 0, 0).map_or(0, |d| d.and_utc().timestamp());
            let to = (Utc::now() + chrono::Duration::days(2)).timestamp();
            let (s, b) = tokio::join!(
                self.daily_series(&symbol, from - 86_400, to),
                self.daily_series(&bench, from - 12 * 86_400, to)
            );
            let (s, b) = (s?, b?);
            crate::ipo::compute(&crate::ipo::PerfInput {
                symbol: &symbol,
                ipo_date,
                offer_price: offer,
                offer_price_source: src,
                series: &s,
                bench: &b,
                bench_name: &bench,
                source_url: symbol::verify_url("yahoo", &AssetId::new(AssetClass::Stock, symbol.clone())),
                now: Utc::now(),
            })
            .map(Arc::new)
            .map_err(DomainError::InvalidInput)
        })
        .await?;
        Ok((*arc).clone())
    }

    /// Precio definitivo del calendario para un símbolo cuya OPV cae a ±5 días de `date`.
    async fn calendar_price(&self, symbol: &str, date: chrono::NaiveDate) -> Option<f64> {
        let rows = self.ipo_calendar(date - chrono::Duration::days(5), date + chrono::Duration::days(5)).await.ok()?;
        rows.iter()
            .find(|e| e.status == IpoStatus::Priced && e.symbol.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(symbol)))
            .and_then(|e| e.price_low.filter(|p| Some(*p) == e.price_high))
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
    /// Tasa base de OPV recientes (caché de 6 h y cálculo único en vuelo).
    pub(crate) ipo_base: crate::ipo_risk::BaseRates,
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
        // Por debajo de `OP_TIMEOUT` (25 s): si no, el tope por operación cortaría antes que el cliente.
        let slow_http = build_slow_client(Duration::from_secs(24)).map_err(|e| anyhow::anyhow!("{e}"))?;
        let secrets = Arc::new(config.secrets());
        if config.uses_default_sec_contact() {
            tracing::warn!(
                "SEC_USER_AGENT sin configurar: la SEC puede bloquear un contacto de ejemplo"
            );
        }
        let yahoo = Arc::new(Yahoo::new(http.clone()));
        let sec = Arc::new(Sec::new(http.clone(), config.sec_user_agent.clone()));
        let finnhub = Arc::new(Finnhub::new(http.clone(), config.finnhub_key.clone()));
        let providers: Vec<Arc<dyn Provider>> = vec![
            yahoo.clone(),
            Arc::new(Binance::with_base(
                http.clone(),
                config.binance_rest_base.clone(),
            )),
            Arc::new(CoinGecko::new(http.clone(), config.coingecko_key.clone())),
            Arc::new(GoogleNews::new(http.clone(), config.news_languages.clone())),
            sec.clone(),
            Arc::new(Fred::new(http.clone(), config.fred_key.clone())),
            finnhub.clone(),
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
        let mut providers = providers;
        if config.gdelt {
            providers.push(Arc::new(Gdelt::new(slow_http)));
        } else {
            tracing::info!(provider = "gdelt", "proveedor desactivado (MARKET_GDELT=1 para activarlo)");
        }
        let slots: Vec<Arc<Slot>> = providers
            .into_iter()
            .map(|p| Arc::new(Slot::new(p, secrets.clone())))
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
            finnhub,
            ipo_calendar: cache(Duration::from_secs(30 * 60), 64),
            prospectuses: cache(Duration::from_secs(6 * 3600), 16),
            ipo_perf: cache(Duration::from_secs(3600), 256),
            quotes: cache(Duration::from_secs(3), 5_000),
            quote_batches: cache(Duration::from_millis(200), 200),
            history_intraday: cache(Duration::from_secs(60), 500),
            history_daily: cache(Duration::from_secs(15 * 60), 500),
            indicators: cache(Duration::from_secs(15 * 60), 1_000),
            fundamentals: cache(Duration::from_secs(6 * 3600), 1_000),
            news: cache(Duration::from_secs(3 * 60), 500),
            late_news: cache(Duration::from_secs(3 * 60), 500),
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
        Ok(Arc::new(Self { inner, hub, cancel, ipo_base: Default::default() }))
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

    /// Calendario de OPV de EE. UU. entre dos fechas (Finnhub; respaldo: índice de EDGAR).
    pub async fn ipo_calendar(&self, from: chrono::NaiveDate, to: chrono::NaiveDate) -> domain::Result<Vec<IpoEntry>> {
        self.inner.ipo_calendar(from, to).await
    }

    /// Folleto de la OPV de un emisor (CIK, ticker o nombre) con sus secciones clave en texto limpio.
    /// `latest = true` toma el folleto definitivo más reciente en vez del de la OPV.
    /// `max_chars` acota cada sección (0 = sin tope).
    pub async fn ipo_prospectus(&self, issuer: &str, latest: bool, max_chars: usize) -> domain::Result<Prospectus> {
        let pick = if latest { ProspectusPick::Latest } else { ProspectusPick::Initial };
        self.inner.ipo_prospectus(issuer, pick, max_chars).await
    }

    /// Rentabilidad posterior a la OPV (primer día, 1/3/6/12 meses) frente a `SPY`, `IWM` o `QQQ`.
    /// Sin `offer_price` se toma el del calendario y, si falta, el de la portada del folleto.
    pub async fn ipo_performance(
        &self,
        symbol: &str,
        ipo_date: chrono::NaiveDate,
        offer_price: Option<f64>,
        benchmark: &str,
    ) -> domain::Result<IpoPerformance> {
        self.inner.ipo_performance(symbol, ipo_date, offer_price, benchmark).await
    }

    /// Historial de resultados de las OPV con precio definitivo de los últimos `months` meses
    /// (hasta `limit`, en el orden del calendario). Las que fallan se omiten.
    pub async fn recent_ipo_performance(&self, months: u32, limit: usize, benchmark: &str) -> domain::Result<Vec<IpoPerformance>> {
        let wanted: Vec<IpoEntry> = self.priced_ipos(months).await?.into_iter().take(limit.clamp(1, 50)).collect();
        Ok(self.performances_of(wanted, benchmark).await.into_iter().map(|x| x.perf).collect())
    }

    /// Como `recent_ipo_performance`, pero con la muestra repartida de forma uniforme por toda la
    /// ventana (en vez de las primeras): así incluye OPV lo bastante antiguas para tener ya el
    /// dato de 3 o 6 meses. Es la base de la tasa base de `ipo_risk`.
    pub async fn sampled_ipo_performance(&self, months: u32, limit: usize, benchmark: &str) -> domain::Result<Vec<crate::ipo_risk::SizedPerf>> {
        let all = self.priced_ipos(months).await?;
        let limit = limit.clamp(1, 80);
        let wanted: Vec<IpoEntry> = if all.len() <= limit {
            all
        } else {
            (0..limit).map(|k| all[k * all.len() / limit].clone()).collect()
        };
        Ok(self.performances_of(wanted, benchmark).await)
    }

    /// OPV con precio definitivo (sin SPAC) de los últimos `months` meses, según el calendario.
    async fn priced_ipos(&self, months: u32) -> domain::Result<Vec<IpoEntry>> {
        let to = Utc::now().date_naive();
        let from = to.checked_sub_months(chrono::Months::new(months.clamp(1, 24))).unwrap_or(to);
        let cal = self.inner.ipo_calendar(from, to).await?;
        Ok(cal
            .into_iter()
            .filter(|e| e.status == IpoStatus::Priced && !e.spac_likely && e.symbol.is_some() && e.price_low.is_some() && e.price_low == e.price_high)
            .collect())
    }

    /// Rendimiento de varias OPV, cuatro a la vez (cada una son 2-3 peticiones); conserva el
    /// orden y omite las que fallan.
    async fn performances_of(&self, wanted: Vec<IpoEntry>, benchmark: &str) -> Vec<crate::ipo_risk::SizedPerf> {
        futures::stream::iter(wanted)
            .map(|e| async move {
                let sym = e.symbol.clone().unwrap_or_default();
                let size_usd = crate::ipo_risk::offer_size_usd(&e);
                self.inner.ipo_performance(&sym, e.date, e.price_low, benchmark).await.ok().map(|perf| crate::ipo_risk::SizedPerf { size_usd, perf })
            })
            .buffered(4)
            .filter_map(|p| async move { p })
            .collect::<Vec<_>>()
            .await
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
                    blocked_secs: blocked_secs(st.blocked_until),
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
                    latency_ms: st.last_latency_ms,
                    blocked_secs: blocked_secs(st.blocked_until),
                }
            })
            .collect()
    }
}

fn blocked_secs(until: Option<Instant>) -> Option<u64> {
    until
        .and_then(|u| u.checked_duration_since(Instant::now()))
        .map(|d| d.as_secs().max(1))
}

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use domain::MarketState;

    fn quote(source: &str, id: &AssetId) -> Quote {
        Quote {
            id: id.clone(),
            price: 1.0,
            change: None,
            change_pct: None,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            volume: None,
            market_cap: None,
            currency: None,
            market_state: MarketState::Unknown,
            timestamp: Utc::now(),
            source: source.into(),
            delay_secs: None,
            source_url: None,
            timestamp_origin: None,
        }
    }

    #[test]
    fn quote_gets_verification_url_from_its_provider() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let q = with_quote_url(quote("yahoo", &id));
        assert_eq!(q.source_url.as_deref(), Some("https://finance.yahoo.com/quote/AAPL"));
        // Sin página pública por activo no se inventa ninguna.
        assert!(with_quote_url(quote("alphavantage", &id)).source_url.is_none());
    }

    #[test]
    fn fundamentals_origin_distinguishes_local_and_provider_dates() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let f = |src: &str, as_of: Option<DateTime<Utc>>| Fundamentals {
            sources: vec![src.into()],
            as_of,
            ..Default::default()
        };
        let y = with_fundamentals_origin(f("yahoo", Some(Utc::now())), &id);
        assert_eq!(y.timestamp_origin, Some(TimestampOrigin::Local));
        assert!(y.source_url.is_some());
        let av = with_fundamentals_origin(f("alphavantage", Some(Utc::now())), &id);
        assert_eq!(av.timestamp_origin, Some(TimestampOrigin::Provider));
        let none = with_fundamentals_origin(f("fmp", None), &id);
        assert_eq!(none.timestamp_origin, None);
    }

    #[test]
    fn blocked_secs_only_while_blocked() {
        assert_eq!(blocked_secs(None), None);
        assert_eq!(blocked_secs(Some(Instant::now() - Duration::from_secs(1))), None);
        assert!(blocked_secs(Some(Instant::now() + Duration::from_secs(30))).unwrap() >= 29);
    }
}

#[cfg(test)]
mod latency_tests {
    use super::*;
    use crate::providers::{ProviderInfo, SearchHit};
    use domain::MacroIndicator;

    struct Dummy(ProviderInfo);

    #[async_trait]
    impl Provider for Dummy {
        fn info(&self) -> &ProviderInfo {
            &self.0
        }
    }

    fn slot() -> Slot {
        let info = ProviderInfo {
            name: "dummy",
            requires_key: false,
            has_key: false,
            capabilities: vec![],
            classes: vec![],
            rate_per_sec: 1000.0,
            burst: 1000,
        };
        Slot::new(Arc::new(Dummy(info)), Arc::new(Vec::new()))
    }

    async fn timing_out(s: &Slot) -> PResult<()> {
        s.run(&|_p: Arc<dyn Provider>| async { Err::<(), _>(ProviderError::upstream("timeout")) }).await
    }

    #[tokio::test]
    async fn repeated_timeouts_bench_the_provider() {
        let s = slot();
        assert!(matches!(timing_out(&s).await, Err(ProviderError::Upstream(_))));
        // Un solo timeout no aparta al proveedor.
        assert!(s.state().blocked_until.is_none());
        assert!(matches!(timing_out(&s).await, Err(ProviderError::Upstream(_))));
        assert!(s.state().blocked_until.is_some());
        // Apartado: responde al instante sin llamar al proveedor.
        assert!(matches!(timing_out(&s).await, Err(ProviderError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn a_success_resets_the_timeout_streak() {
        let s = slot();
        let _ = timing_out(&s).await;
        s.run(&|_p: Arc<dyn Provider>| async { Ok::<_, ProviderError>(()) }).await.unwrap();
        let _ = timing_out(&s).await;
        assert!(s.state().blocked_until.is_none());
    }

    #[tokio::test]
    async fn cancelled_slow_operations_count_as_timeouts() {
        let s = slot();
        for _ in 0..TIMEOUT_STRIKES {
            let slow = s.run_discardable(&|_p: Arc<dyn Provider>| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<_, ProviderError>(())
            });
            // Se descarta tras 1,1 s, como hace `collect_with_grace`.
            let _ = tokio::time::timeout(Duration::from_millis(1100), slow).await;
        }
        assert!(s.state().blocked_until.is_some());
    }

    /// Una petición del cliente que se aborta (cambio de vista, cierre de pestaña) suelta el futuro
    /// de la operación en vuelo. No es un timeout del proveedor: dos así seguidas apartaban a Yahoo
    /// 60 s para TODOS los usuarios.
    #[tokio::test]
    async fn a_client_abort_does_not_bench_the_provider() {
        let s = slot();
        for _ in 0..(TIMEOUT_STRIKES + 1) {
            let slow = s.run(&|_p: Arc<dyn Provider>| async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<_, ProviderError>(())
            });
            let _ = tokio::time::timeout(Duration::from_millis(1100), slow).await;
        }
        assert!(s.state().blocked_until.is_none(), "un abort del cliente no es un timeout del proveedor");
    }

    /// Proveedor "binance" falso que cuenta las llamadas al lote de cotizaciones.
    struct CountingBinance {
        info: ProviderInfo,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl Provider for CountingBinance {
        fn info(&self) -> &ProviderInfo {
            &self.info
        }

        async fn quotes(&self, ids: &[AssetId]) -> PResult<Vec<Quote>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(ids
                .iter()
                .map(|id| Quote {
                    id: id.clone(),
                    price: 1.0,
                    change: None,
                    change_pct: None,
                    open: None,
                    high: None,
                    low: None,
                    prev_close: None,
                    volume: None,
                    market_cap: None,
                    currency: None,
                    market_state: domain::MarketState::Unknown,
                    timestamp: Utc::now(),
                    source: "binance".into(),
                    delay_secs: None,
                    source_url: None,
                    timestamp_origin: None,
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn simultaneous_identical_batches_hit_the_provider_once() {
        let http = build_client(Duration::from_secs(5)).unwrap();
        let info = ProviderInfo {
            name: "binance",
            requires_key: false,
            has_key: false,
            capabilities: vec![],
            classes: vec![],
            rate_per_sec: 1000.0,
            burst: 1000,
        };
        let prov = Arc::new(CountingBinance { info, calls: Default::default() });
        let inner = Arc::new(Inner {
            slots: vec![Arc::new(Slot::new(prov.clone(), Arc::new(Vec::new())))],
            yahoo: Arc::new(Yahoo::new(http.clone())),
            sec: Arc::new(Sec::new(http.clone(), "test test@example.com".into())),
            finnhub: Arc::new(Finnhub::new(http, None)),
            ipo_calendar: cache(Duration::from_secs(60), 10),
            prospectuses: cache(Duration::from_secs(60), 10),
            ipo_perf: cache(Duration::from_secs(60), 10),
            quotes: cache(Duration::from_secs(3), 100),
            quote_batches: cache(Duration::from_millis(200), 10),
            history_intraday: cache(Duration::from_secs(60), 10),
            history_daily: cache(Duration::from_secs(60), 10),
            indicators: cache(Duration::from_secs(60), 10),
            fundamentals: cache(Duration::from_secs(60), 10),
            news: cache(Duration::from_secs(60), 10),
            late_news: cache(Duration::from_secs(60), 10),
            macros: cache(Duration::from_secs(60), 4),
            search: cache(Duration::from_secs(60), 10),
            summaries: cache(Duration::from_secs(60), 10),
            options: cache(Duration::from_secs(60), 10),
        });
        let ids = vec![AssetId::new(AssetClass::Crypto, "BTC"), AssetId::new(AssetClass::Crypto, "ETH")];
        let reordered = vec![ids[1].clone(), ids[0].clone()];
        let tasks: Vec<_> = (0..30)
            .map(|i| {
                let inner = inner.clone();
                let ids = if i % 2 == 0 { ids.clone() } else { reordered.clone() };
                tokio::spawn(async move { inner.quotes(&ids).await.len() })
            })
            .collect();
        for t in tasks {
            assert_eq!(t.await.unwrap(), 2);
        }
        assert_eq!(prov.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    fn test_inner(slots: Vec<Arc<Slot>>) -> Arc<Inner> {
        let http = build_client(Duration::from_secs(5)).unwrap();
        Arc::new(Inner {
            slots,
            yahoo: Arc::new(Yahoo::new(http.clone())),
            sec: Arc::new(Sec::new(http.clone(), "test test@example.com".into())),
            finnhub: Arc::new(Finnhub::new(http, None)),
            ipo_calendar: cache(Duration::from_secs(60), 10),
            prospectuses: cache(Duration::from_secs(60), 10),
            ipo_perf: cache(Duration::from_secs(60), 10),
            quotes: cache(Duration::from_secs(3), 100),
            quote_batches: cache(Duration::from_millis(200), 10),
            history_intraday: cache(Duration::from_secs(60), 10),
            history_daily: cache(Duration::from_secs(60), 10),
            indicators: cache(Duration::from_secs(60), 10),
            fundamentals: cache(Duration::from_secs(60), 10),
            news: cache(Duration::from_secs(60), 10),
            late_news: cache(Duration::from_secs(60), 10),
            macros: cache(Duration::from_secs(60), 4),
            search: cache(Duration::from_secs(60), 10),
            summaries: cache(Duration::from_secs(60), 10),
            options: cache(Duration::from_secs(60), 10),
        })
    }

    /// Proveedor de noticias falso: responde tras `delay` con sus noticias (o con un fallo).
    struct FakeNews {
        info: ProviderInfo,
        delay: Duration,
        items: Option<Vec<NewsItem>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl Provider for FakeNews {
        fn info(&self) -> &ProviderInfo {
            &self.info
        }

        async fn news(&self, _q: &NewsQuery) -> PResult<Vec<NewsItem>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.items.clone().ok_or_else(|| ProviderError::upstream("boom"))
        }
    }

    fn news_item(title: &str, provider: &str, age_min: i64) -> NewsItem {
        let url = format!("https://example.com/{provider}/{}", title.replace(' ', "-"));
        NewsItem {
            id: news::item_id(&url),
            title: title.into(),
            summary: None,
            url,
            publisher: None,
            published_at: Utc::now() - chrono::Duration::minutes(age_min),
            image_url: None,
            tickers: vec![],
            sentiment: None,
            provider: provider.into(),
            language: None,
        }
    }

    fn fake_news(name: &'static str, delay_ms: u64, items: Option<Vec<NewsItem>>) -> Arc<FakeNews> {
        Arc::new(FakeNews {
            info: ProviderInfo {
                name,
                requires_key: false,
                has_key: false,
                capabilities: vec![],
                classes: vec![],
                rate_per_sec: 1000.0,
                burst: 1000,
            },
            delay: Duration::from_millis(delay_ms),
            items,
            calls: Default::default(),
        })
    }

    fn providers_of(items: &[NewsItem]) -> Vec<&str> {
        let mut p: Vec<&str> = items.iter().map(|n| n.provider.as_str()).collect();
        p.sort_unstable();
        p.dedup();
        p
    }

    /// GDELT tarda 11-15 s por petición (8-10 s de TLS). No debe retrasar la respuesta, pero si al
    /// final contesta sus noticias SÍ se fusionan en la siguiente lectura (sin volver a pedirlas).
    #[tokio::test(start_paused = true)]
    async fn slow_background_provider_does_not_delay_news_and_is_merged_once_it_answers() {
        let fast = fake_news("google_news", 50, Some(vec![news_item("alfa", "google_news", 5), news_item("beta", "google_news", 20)]));
        let slow = fake_news("gdelt", 12_000, Some(vec![news_item("gamma", "gdelt", 10)]));
        let inner = test_inner(vec![
            Arc::new(Slot::new(fast.clone(), Arc::new(Vec::new()))),
            Arc::new(Slot::new(slow.clone(), Arc::new(Vec::new()))),
        ]);
        let t0 = tokio::time::Instant::now();
        let first = inner.news(None, 50).await.unwrap();
        assert!(t0.elapsed() < Duration::from_millis(500), "la respuesta esperó al lento: {:?}", t0.elapsed());
        assert_eq!(providers_of(&first), ["google_news"]);
        // Mientras GDELT sigue en vuelo, otra lectura no lo ve ni lo vuelve a pedir.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(providers_of(&inner.news(None, 50).await.unwrap()), ["google_news"]);
        // Llega a los ~12 s: ahora se fusiona, ordenado por fecha, sin nuevas peticiones.
        tokio::time::sleep(Duration::from_secs(8)).await;
        let merged = inner.news(None, 50).await.unwrap();
        assert_eq!(providers_of(&merged), ["gdelt", "google_news"]);
        assert_eq!(merged.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["alfa", "gamma", "beta"]);
        assert_eq!(slow.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fast.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Si fallan todos los de primer plano pero el de segundo plano ya trajo noticias, se sirven esas
    /// en lugar de un error (antes `news()` devolvía el error aunque `late_news` tuviera la lista).
    #[tokio::test(start_paused = true)]
    async fn late_news_are_served_when_every_foreground_provider_fails() {
        let bad = fake_news("google_news", 50, None);
        let slow = fake_news("gdelt", 12_000, Some(vec![news_item("gamma", "gdelt", 10)]));
        let inner = test_inner(vec![
            Arc::new(Slot::new(bad, Arc::new(Vec::new()))),
            Arc::new(Slot::new(slow, Arc::new(Vec::new()))),
        ]);
        // Sin nada todavía: error, como antes.
        assert!(inner.news(None, 50).await.is_err());
        // GDELT contesta a los ~12 s; el primer plano sigue fallando.
        tokio::time::sleep(Duration::from_secs(13)).await;
        let served = inner.news(None, 50).await.expect("debería servir las noticias tardías");
        assert_eq!(providers_of(&served), ["gdelt"]);
    }

    #[tokio::test(start_paused = true)]
    async fn failing_background_provider_leaves_news_intact_and_is_accounted() {
        let fast = fake_news("google_news", 50, Some(vec![news_item("alfa", "google_news", 5)]));
        let bad = fake_news("gdelt", 100, None);
        let bad_slot = Arc::new(Slot::new(bad.clone(), Arc::new(Vec::new())));
        let inner = test_inner(vec![Arc::new(Slot::new(fast, Arc::new(Vec::new()))), bad_slot.clone()]);
        let first = inner.news(None, 50).await.unwrap();
        assert_eq!(providers_of(&first), ["google_news"]);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(providers_of(&inner.news(None, 50).await.unwrap()), ["google_news"]);
        // El fallo del proveedor en segundo plano queda anotado como el de cualquier otro.
        assert!(bad_slot.state().last_error.is_some());
    }

    /// Proveedor falso que responde a todo con datos mínimos pero válidos (para recorrer el
    /// camino completo de `snapshot` sin red).
    struct FakeAll {
        info: ProviderInfo,
    }

    #[async_trait]
    impl Provider for FakeAll {
        fn info(&self) -> &ProviderInfo {
            &self.info
        }

        async fn search(&self, query: &str, _limit: usize) -> PResult<Vec<SearchHit>> {
            let id = AssetId::new(AssetClass::Stock, query);
            Ok(vec![SearchHit {
                summary: AssetSummary {
                    id,
                    symbol: query.into(),
                    name: format!("{query} Inc."),
                    class: AssetClass::Stock,
                    exchange: Some("NASDAQ".into()),
                    currency: Some("USD".into()),
                    country: None,
                    source: self.info.name.into(),
                    source_url: None,
                },
                liquidity: 1.0,
            }])
        }

        async fn quote(&self, id: &AssetId) -> PResult<Quote> {
            Ok(Quote {
                id: id.clone(),
                price: 100.0,
                change: None,
                change_pct: None,
                open: None,
                high: None,
                low: None,
                prev_close: Some(99.0),
                volume: None,
                market_cap: None,
                currency: Some("USD".into()),
                market_state: domain::MarketState::Regular,
                timestamp: Utc::now(),
                source: self.info.name.into(),
                delay_secs: None,
                source_url: None,
                timestamp_origin: None,
            })
        }

        async fn history(&self, _id: &AssetId, _r: Range, _i: Interval) -> PResult<Vec<Candle>> {
            let now = Utc::now().timestamp();
            Ok((0..300)
                .map(|i| {
                    let p = 100.0 + f64::from(i % 17);
                    Candle { t: now - (300 - i64::from(i)) * 86_400, o: p, h: p + 1.0, l: p - 1.0, c: p + 0.5, v: Some(1e6) }
                })
                .collect())
        }

        async fn fundamentals(&self, _id: &AssetId) -> PResult<Option<Fundamentals>> {
            Ok(Some(Fundamentals::default()))
        }

        async fn news(&self, _q: &NewsQuery) -> PResult<Vec<NewsItem>> {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(vec![news_item("titular", self.info.name, 3)])
        }

        async fn macro_indicators(&self) -> PResult<Vec<MacroIndicator>> {
            Ok(vec![])
        }
    }

    fn fake_all(name: &'static str) -> Arc<Slot> {
        let info = ProviderInfo {
            name,
            requires_key: false,
            has_key: false,
            capabilities: vec![],
            classes: vec![],
            rate_per_sec: 1000.0,
            burst: 1000,
        };
        Arc::new(Slot::new(Arc::new(FakeAll { info }), Arc::new(Vec::new())))
    }

    /// Regresión (13-oct-2026): `GET /assets/{id}/snapshot` abortaba el servidor DEBUG con
    /// «thread 'tokio-rt-worker' has overflowed its stack». Los workers de Tokio tienen 2 MiB de
    /// pila y en debug cada futuro anidado se copia entero en el marco del que lo sondea; el
    /// `join!` de `snapshot` con las partes sin encajar superaba el límite. Se ejecuta en un hilo
    /// de 2 MiB con un runtime de un solo hilo (como un worker) y proveedores simulados.
    #[test]
    fn snapshot_runs_in_a_2_mib_stack() {
        let worker = std::thread::Builder::new()
            .name("tokio-rt-worker-like".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(async {
                    let inner = test_inner(vec![fake_all("yahoo"), fake_all("google_news"), fake_all("gdelt")]);
                    let id = AssetId::new(AssetClass::Stock, "AAPL");
                    // El futuro de `snapshot` (y el de `news`, el mayor de sus partes) debe ser
                    // pequeño: antes 206 KB y 54 KB en debug, ahora van en el montón.
                    let (snap_size, news_size) = (std::mem::size_of_val(&inner.snapshot(&id)), std::mem::size_of_val(&inner.news(Some(&id), 15)));
                    println!("SIZE snapshot={snap_size} news={news_size}");
                    assert!(snap_size < 16 * 1024, "el futuro de snapshot ocupa {snap_size} B");
                    assert!(news_size < 16 * 1024, "el futuro de news ocupa {news_size} B");
                    let snap = inner.snapshot(&id).await.expect("snapshot");
                    assert!(snap.quote.is_some() && snap.indicators.is_some() && !snap.news.is_empty(), "{:?}", snap.missing);
                });
            })
            .unwrap();
        worker.join().expect("el hilo no debe entrar en pánico");
    }

    type Fut = std::pin::Pin<Box<dyn Future<Output = (&'static str, PResult<u8>)> + Send>>;

    fn after(ms: u64, name: &'static str, ok: bool) -> Fut {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            (name, if ok { Ok(1) } else { Err(ProviderError::upstream("x")) })
        })
    }

    #[tokio::test(start_paused = true)]
    async fn slow_provider_is_dropped_after_the_grace_period() {
        let t0 = tokio::time::Instant::now();
        let out = collect_with_grace(vec![after(300, "fast", true), after(6000, "slow", true)], Duration::from_millis(1500)).await;
        assert_eq!(out.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["fast"]);
        assert!(t0.elapsed() < Duration::from_millis(2000), "tardó {:?}", t0.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_everyone_when_nobody_has_answered_yet() {
        // Un fallo rápido no abre la ventana de gracia: se espera al que sí responde.
        let out = collect_with_grace(vec![after(100, "bad", false), after(5000, "slow", true)], Duration::from_millis(1500)).await;
        assert_eq!(out.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn providers_arriving_within_grace_are_kept() {
        let out = collect_with_grace(vec![after(100, "a", true), after(900, "b", true)], Duration::from_millis(1500)).await;
        assert_eq!(out.len(), 2);
    }
}
