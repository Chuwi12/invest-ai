//! Yahoo Finance (API no oficial). Es el proveedor con más cobertura y no necesita clave.
//!
//! - `search`, `chart` (quote e histórico) y noticias no necesitan sesión.
//! - `quote` en lote, `quoteSummary` y `options` exigen cookie + crumb: la cookie se obtiene en
//!   `fc.yahoo.com`, el crumb en `/v1/test/getcrumb`; el par se cachea y se renueva ante un 401.
//!   La cookie se envía a mano (cabecera `Cookie`) para poder usar el cliente HTTP compartido
//!   sin un almacén de cookies global que se mezclaría con otros proveedores.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{
    AssetClass, AssetId, AssetSummary, Candle, Fundamentals, Holding, Interval, MacroIndicator,
    MarketState, NewsItem, Quote, Range,
};
use futures::{StreamExt, stream};
use reqwest::{Client, header};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use url::Url;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, SearchHit,
    coerce_interval, plain_ticker,
};
use crate::{
    http::{self, Body, check_status, map_reqwest, parse_json, send},
    news::item_id,
    options::{OptionContract, OptionsChain},
};

const BASE: &str = "https://query1.finance.yahoo.com";
const COOKIE_URL: &str = "https://fc.yahoo.com";

/// Símbolos de Yahoo que alimentan el contexto macro cuando no hay FRED (o lo complementan).
const MACRO_SYMBOLS: &[(&str, &str, &str, &str)] = &[
    // (clave, nombre, símbolo Yahoo, unidad)
    (
        "us10y",
        "Rendimiento del bono de EE. UU. a 10 años",
        "^TNX",
        "%",
    ),
    (
        "us5y",
        "Rendimiento del bono de EE. UU. a 5 años",
        "^FVX",
        "%",
    ),
    (
        "us30y",
        "Rendimiento del bono de EE. UU. a 30 años",
        "^TYX",
        "%",
    ),
    (
        "us3m",
        "Rendimiento de la letra de EE. UU. a 3 meses",
        "^IRX",
        "%",
    ),
    ("vix", "Índice de volatilidad VIX", "^VIX", "puntos"),
    ("dxy", "Índice del dólar (DXY)", "DX-Y.NYB", "puntos"),
    ("eurusd", "Euro / dólar", "EURUSD=X", "USD"),
    ("gold", "Oro (futuro)", "GC=F", "USD/oz"),
    ("wti", "Petróleo WTI (futuro)", "CL=F", "USD/barril"),
];

pub struct Yahoo {
    http: Client,
    info: ProviderInfo,
    session: Mutex<Option<Arc<Session>>>,
}

struct Session {
    cookie: String,
    crumb: String,
}

impl Yahoo {
    pub fn new(http: Client) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "yahoo",
                requires_key: false,
                has_key: false,
                capabilities: vec![
                    Capability::Search,
                    Capability::Quote,
                    Capability::History,
                    Capability::Fundamentals,
                    Capability::News,
                    Capability::Options,
                    Capability::Macro,
                ],
                classes: vec![
                    AssetClass::Stock,
                    AssetClass::Etf,
                    AssetClass::Fund,
                    AssetClass::Index,
                    AssetClass::Crypto,
                    AssetClass::Forex,
                    AssetClass::Commodity,
                    AssetClass::Bond,
                    AssetClass::Future,
                    AssetClass::Option,
                ],
                // Yahoo castiga las ráfagas: ~4 peticiones/s sostenidas.
                rate_per_sec: 4.0,
                burst: 8,
            },
            session: Mutex::new(None),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Sesión (cookie + crumb)
    // -----------------------------------------------------------------------------------------

    async fn session(&self) -> PResult<Arc<Session>> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let fresh = Arc::new(self.fetch_session().await?);
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    /// Descarta la sesión si sigue siendo la que falló (otro hilo pudo renovarla ya).
    async fn invalidate(&self, failed_crumb: &str) {
        let mut guard = self.session.lock().await;
        if guard.as_ref().is_some_and(|s| s.crumb == failed_crumb) {
            *guard = None;
        }
    }

    async fn fetch_session(&self) -> PResult<Session> {
        // fc.yahoo.com responde 404, pero con la cookie de sesión en `Set-Cookie`.
        let resp = self
            .http
            .get(COOKIE_URL)
            .send()
            .await
            .map_err(map_reqwest)?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited { retry_after: None });
        }
        let cookie = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .map(str::trim)
            .filter(|kv| kv.contains('='))
            .collect::<Vec<_>>()
            .join("; ");
        if cookie.is_empty() {
            return Err(ProviderError::upstream("Yahoo no entregó cookie de sesión"));
        }
        let body = send(
            self.http
                .get(format!("{BASE}/v1/test/getcrumb"))
                .header(header::COOKIE, &cookie),
        )
        .await?;
        check_status(&body)?;
        let crumb = body.text.trim().to_string();
        if crumb.is_empty() || crumb.len() > 64 || crumb.contains('<') || crumb.contains(' ') {
            return Err(ProviderError::upstream(format!(
                "crumb de Yahoo no válido: {}",
                http::snippet(&crumb, 60)
            )));
        }
        tracing::debug!("yahoo: sesión renovada");
        Ok(Session { cookie, crumb })
    }

    /// GET con crumb; si Yahoo contesta 401 / «Invalid Crumb» renueva la sesión y reintenta una vez.
    async fn get_with_crumb(&self, url: Url, query: &[(&str, String)]) -> PResult<String> {
        let mut last: Option<Body> = None;
        for _ in 0..2 {
            let s = self.session().await?;
            let mut q: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
            q.push(("crumb", &s.crumb));
            let body = send(
                self.http
                    .get(url.clone())
                    .query(&q)
                    .header(header::COOKIE, &s.cookie),
            )
            .await?;
            let bad_crumb = body.status == reqwest::StatusCode::UNAUTHORIZED
                || body.text.contains("Invalid Crumb")
                || body.text.contains("Invalid Cookie");
            if bad_crumb {
                self.invalidate(&s.crumb).await;
                last = Some(body);
                continue;
            }
            check_status(&body)?;
            return Ok(body.text);
        }
        let detail = last
            .map(|b| http::snippet(&b.text, 100))
            .unwrap_or_default();
        Err(ProviderError::upstream(format!(
            "sesión de Yahoo rechazada: {detail}"
        )))
    }

    async fn get_plain(&self, url: Url, query: &[(&str, String)]) -> PResult<String> {
        let q: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        http::send_ok(self.http.get(url).query(&q)).await
    }

    fn url(segments: &[&str]) -> PResult<Url> {
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .extend(segments);
        Ok(url)
    }

    // -----------------------------------------------------------------------------------------
    // Operaciones sueltas (usadas por el trait y por la API pública del servicio)
    // -----------------------------------------------------------------------------------------

    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> PResult<ChartData> {
        let url = Self::url(&["v8", "finance", "chart", symbol])?;
        let res = self
            .get_plain(
                url,
                &[
                    ("range", range.to_string()),
                    ("interval", interval.to_string()),
                    ("includePrePost", "false".into()),
                ],
            )
            .await;
        match res {
            // Para símbolos inexistentes Yahoo responde 404 con el JSON de error en el cuerpo.
            Err(ProviderError::NotFound(_)) => Err(ProviderError::NotFound(format!(
                "símbolo {symbol} no encontrado"
            ))),
            other => parse_chart(&other?),
        }
    }

    async fn chart_quote(&self, id: &AssetId, symbol: &str) -> PResult<Quote> {
        let data = self.chart(symbol, "1d", "1d").await?;
        Ok(quote_from_chart(id, &data, Utc::now()))
    }

    async fn batch_quotes(&self, ids: &[AssetId]) -> PResult<Vec<Quote>> {
        let mut by_symbol: HashMap<String, Vec<&AssetId>> = HashMap::new();
        for id in ids {
            if let Some(sym) = to_yahoo_symbol(id) {
                by_symbol.entry(sym).or_default().push(id);
            }
        }
        if by_symbol.is_empty() {
            return Err(ProviderError::Unsupported);
        }
        let symbols: Vec<String> = by_symbol.keys().cloned().collect();
        let mut out = Vec::new();
        for chunk in symbols.chunks(50) {
            let url = Self::url(&["v7", "finance", "quote"])?;
            let text = self
                .get_with_crumb(url, &[("symbols", chunk.join(","))])
                .await?;
            out.extend(parse_quotes(&text, &by_symbol)?);
        }
        Ok(out)
    }

    /// Cadena de opciones. Sin `expiration` devuelve el vencimiento más próximo.
    pub async fn options_chain(
        &self,
        id: &AssetId,
        expiration: Option<i64>,
    ) -> PResult<OptionsChain> {
        let symbol = to_yahoo_symbol(id).ok_or(ProviderError::Unsupported)?;
        let url = Self::url(&["v7", "finance", "options", &symbol])?;
        let mut q = Vec::new();
        if let Some(d) = expiration {
            q.push(("date", d.to_string()));
        }
        let text = self.get_with_crumb(url, &q).await?;
        parse_options(&text)
    }
}

// ---------------------------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------------------------

#[async_trait]
impl Provider for Yahoo {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn search(&self, query: &str, limit: usize) -> PResult<Vec<SearchHit>> {
        let url = Self::url(&["v1", "finance", "search"])?;
        let text = self
            .get_plain(
                url,
                &[
                    ("q", query.to_string()),
                    ("quotesCount", limit.clamp(1, 50).to_string()),
                    ("newsCount", "0".into()),
                    ("listsCount", "0".into()),
                    ("enableFuzzyQuery", "false".into()),
                ],
            )
            .await?;
        Ok(parse_search(&text)?.0)
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let symbol = to_yahoo_symbol(id).ok_or(ProviderError::Unsupported)?;
        // Primero el endpoint en lote (más completo: capitalización, estado del mercado...). Si falla
        // por sesión o límite, el chart no necesita crumb y sirve de respaldo.
        match self.batch_quotes(std::slice::from_ref(id)).await {
            Ok(mut v) if !v.is_empty() => Ok(v.swap_remove(0)),
            Ok(_) => Err(ProviderError::NotFound(format!(
                "símbolo {symbol} no encontrado"
            ))),
            Err(ProviderError::Upstream(_) | ProviderError::RateLimited { .. }) => {
                self.chart_quote(id, &symbol).await
            }
            Err(e) => Err(e),
        }
    }

    async fn quotes(&self, ids: &[AssetId]) -> PResult<Vec<Quote>> {
        match self.batch_quotes(ids).await {
            Ok(v) => Ok(v),
            Err(ProviderError::Upstream(_) | ProviderError::RateLimited { .. }) => {
                // Respaldo: un chart por activo, con paralelismo acotado.
                let quotes = stream::iter(ids.iter().cloned())
                    .map(|id| async move {
                        let sym = to_yahoo_symbol(&id)?;
                        self.chart_quote(&id, &sym).await.ok()
                    })
                    .buffer_unordered(4)
                    .filter_map(|q| async move { q })
                    .collect::<Vec<_>>()
                    .await;
                if quotes.is_empty() {
                    Err(ProviderError::upstream(
                        "Yahoo no devolvió ninguna cotización",
                    ))
                } else {
                    Ok(quotes)
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> PResult<Vec<Candle>> {
        let symbol = to_yahoo_symbol(id).ok_or(ProviderError::Unsupported)?;
        let interval = coerce_interval(range, interval);
        let mut data = self
            .chart(&symbol, range.as_str(), yahoo_interval(interval))
            .await?;
        fold_live_point(&mut data.candles, interval);
        Ok(data.candles)
    }

    async fn fundamentals(&self, id: &AssetId) -> PResult<Option<Fundamentals>> {
        let modules = match id.class {
            AssetClass::Stock => {
                "price,summaryDetail,defaultKeyStatistics,financialData,assetProfile,calendarEvents"
            }
            AssetClass::Etf | AssetClass::Fund => {
                "price,summaryDetail,defaultKeyStatistics,topHoldings,fundProfile,assetProfile"
            }
            // Índices, divisas, futuros...: no hay fundamentales con sentido.
            _ => return Ok(None),
        };
        let symbol = to_yahoo_symbol(id).ok_or(ProviderError::Unsupported)?;
        let url = Self::url(&["v10", "finance", "quoteSummary", &symbol])?;
        let text = self
            .get_with_crumb(url, &[("modules", modules.to_string())])
            .await?;
        parse_quote_summary(&text, Utc::now()).map(Some)
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let query = match &q.id {
            Some(id) => to_yahoo_symbol(id).unwrap_or_else(|| plain_ticker(&id.symbol)),
            None => "stock market".to_string(),
        };
        let url = Self::url(&["v1", "finance", "search"])?;
        let text = self
            .get_plain(
                url,
                &[
                    ("q", query),
                    ("quotesCount", "0".into()),
                    ("newsCount", q.limit.clamp(5, 50).to_string()),
                    ("listsCount", "0".into()),
                ],
            )
            .await?;
        let mut items = parse_search(&text)?.1;
        if let Some(id) = &q.id {
            // Solo titulares que Yahoo asocia al símbolo (la búsqueda trae también ruido).
            let sym = to_yahoo_symbol(id).unwrap_or_default().to_ascii_uppercase();
            items.retain(|n| n.tickers.iter().any(|t| t.to_ascii_uppercase() == sym));
        }
        Ok(items)
    }

    async fn macro_indicators(&self) -> PResult<Vec<MacroIndicator>> {
        let ids: Vec<AssetId> = MACRO_SYMBOLS
            .iter()
            .map(|(_, _, sym, _)| AssetId::new(macro_class(sym), *sym))
            .collect();
        let quotes = self.batch_quotes(&ids).await?;
        Ok(macro_from_quotes(&quotes))
    }
}

fn macro_class(symbol: &str) -> AssetClass {
    if symbol.starts_with('^') {
        AssetClass::Index
    } else if symbol.ends_with("=X") {
        AssetClass::Forex
    } else {
        AssetClass::Future
    }
}

/// Convierte cotizaciones de los símbolos de `MACRO_SYMBOLS` en indicadores macro.
pub fn macro_from_quotes(quotes: &[Quote]) -> Vec<MacroIndicator> {
    MACRO_SYMBOLS
        .iter()
        .filter_map(|(key, name, sym, unit)| {
            let q = quotes.iter().find(|q| q.id.symbol == *sym)?;
            Some(MacroIndicator {
                key: (*key).into(),
                name: (*name).into(),
                value: q.price,
                change: q.change,
                unit: (*unit).into(),
                as_of: q.timestamp,
                source: "yahoo".into(),
                source_url: crate::symbol::verify_url("yahoo", &q.id),
                timestamp_origin: q.timestamp_origin,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Traducción de símbolos y tipos
// ---------------------------------------------------------------------------------------------

/// Símbolo de Yahoo para un `AssetId` (`None` si Yahoo no lo cubre).
pub fn to_yahoo_symbol(id: &AssetId) -> Option<String> {
    let s = id.symbol.trim();
    if s.is_empty() {
        return None;
    }
    match id.class {
        AssetClass::Crypto => Some(format!("{}-USD", s.to_ascii_uppercase())),
        AssetClass::Bond => {
            // Series de FRED con equivalente en Yahoo; si ya es un símbolo de Yahoo (^TNX) se usa tal cual.
            let mapped = match s.to_ascii_uppercase().as_str() {
                "DGS10" => "^TNX",
                "DGS5" => "^FVX",
                "DGS30" => "^TYX",
                "DGS3MO" => "^IRX",
                _ if s.starts_with('^') || s.contains('=') || s.contains('.') => s,
                _ => return None,
            };
            Some(mapped.to_string())
        }
        AssetClass::Forex => {
            let t = s.replace('/', "").to_ascii_uppercase();
            if t.ends_with("=X") {
                Some(t)
            } else if t.len() == 6 && t.chars().all(|c| c.is_ascii_alphabetic()) {
                Some(format!("{t}=X"))
            } else {
                Some(t)
            }
        }
        _ => Some(s.to_string()),
    }
}

/// Clase de dominio a partir del `quoteType` de Yahoo.
pub fn class_from_quote_type(quote_type: &str) -> AssetClass {
    match quote_type.to_ascii_uppercase().as_str() {
        "EQUITY" => AssetClass::Stock,
        "ETF" => AssetClass::Etf,
        "MUTUALFUND" => AssetClass::Fund,
        "INDEX" => AssetClass::Index,
        "CRYPTOCURRENCY" => AssetClass::Crypto,
        "CURRENCY" => AssetClass::Forex,
        "FUTURE" => AssetClass::Future,
        "OPTION" => AssetClass::Option,
        _ => AssetClass::Other,
    }
}

fn yahoo_interval(i: Interval) -> &'static str {
    match i {
        Interval::H1 => "60m",
        other => other.as_str(),
    }
}

/// `BTC-USD` → `Some("BTC")`; otras cotizaciones (`BTC-EUR`) → `None` para no duplicar activos.
fn crypto_base(symbol: &str) -> Option<String> {
    symbol.strip_suffix("-USD").map(str::to_ascii_uppercase)
}

/// Peso por mercado: los grandes mercados líquidos pesan más que los OTC.
fn exchange_tier(exchange: &str) -> f64 {
    match exchange {
        "NMS" | "NYQ" | "NGM" | "NCM" | "ASE" | "PCX" | "BTS" | "NAS" | "NYS" | "MCE" | "LSE"
        | "GER" | "FRA" | "PAR" | "AMS" | "MIL" | "EBS" | "STO" | "TOR" | "JPX" | "HKG" | "SNP"
        | "CCC" | "CME" | "CMX" | "NYM" | "CBT" | "CCY" | "WCB" => 1.0,
        "PNK" | "OQB" | "OQX" | "OEM" | "OTC" | "YHD" => 0.3,
        _ => 0.7,
    }
}

// ---------------------------------------------------------------------------------------------
// Búsqueda
// ---------------------------------------------------------------------------------------------

/// Devuelve resultados de activos y noticias de una respuesta de `/v1/finance/search`.
pub fn parse_search(text: &str) -> PResult<(Vec<SearchHit>, Vec<NewsItem>)> {
    let v: Value = parse_json(text)?;
    let mut hits = Vec::new();
    for q in v["quotes"].as_array().into_iter().flatten() {
        let (Some(symbol), Some(qt)) = (q["symbol"].as_str(), q["quoteType"].as_str()) else {
            continue;
        };
        let class = class_from_quote_type(qt);
        let sym = if class == AssetClass::Crypto {
            match crypto_base(symbol) {
                Some(base) => base,
                None => continue,
            }
        } else {
            symbol.to_string()
        };
        let name = q["longname"]
            .as_str()
            .or_else(|| q["shortname"].as_str())
            .unwrap_or(symbol)
            .to_string();
        let exchange = q["exchDisp"]
            .as_str()
            .or_else(|| q["exchange"].as_str())
            .map(str::to_string);
        let score = q["score"].as_f64().unwrap_or(0.0).max(0.0);
        let relevance = ((1.0 + score).ln() / 1_000_001f64.ln()).clamp(0.0, 1.0);
        let tier = exchange_tier(q["exchange"].as_str().unwrap_or(""));
        hits.push(SearchHit {
            summary: AssetSummary {
                id: AssetId::new(class, sym.clone()),
                symbol: sym,
                name,
                class,
                exchange,
                currency: None,
                country: None,
                source: "yahoo".into(),
                source_url: None,
            },
            liquidity: relevance * tier,
        });
    }
    let news = v["news"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(news_from_value)
        .collect();
    Ok((hits, news))
}

fn news_from_value(n: &Value) -> Option<NewsItem> {
    let title = n["title"].as_str()?.trim();
    let url = n["link"].as_str()?;
    let ts = n["providerPublishTime"].as_i64()?;
    if title.is_empty() {
        return None;
    }
    Some(NewsItem {
        id: item_id(url),
        title: title.to_string(),
        summary: None,
        url: url.to_string(),
        publisher: n["publisher"].as_str().map(str::to_string),
        published_at: DateTime::<Utc>::from_timestamp(ts, 0)?,
        image_url: n["thumbnail"]["resolutions"]
            .as_array()
            .and_then(|r| r.first())
            .and_then(|r| r["url"].as_str())
            .map(str::to_string),
        tickers: n["relatedTickers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect(),
        sentiment: None,
        provider: "yahoo".into(),
        language: Some("en".into()),
    })
}

// ---------------------------------------------------------------------------------------------
// Chart (quote ligera + histórico)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChartEnvelope {
    chart: ChartInner,
}

#[derive(Debug, Deserialize)]
struct ChartInner {
    result: Option<Vec<ChartResult>>,
    error: Option<YahooError>,
}

#[derive(Debug, Deserialize)]
struct YahooError {
    code: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChartResult {
    meta: ChartMeta,
    #[serde(default)]
    timestamp: Vec<i64>,
    #[serde(default)]
    indicators: ChartIndicators,
}

#[derive(Debug, Default, Deserialize)]
struct ChartIndicators {
    #[serde(default)]
    quote: Vec<ChartQuote>,
}

#[derive(Debug, Default, Deserialize)]
struct ChartQuote {
    #[serde(default)]
    open: Vec<Option<f64>>,
    #[serde(default)]
    high: Vec<Option<f64>>,
    #[serde(default)]
    low: Vec<Option<f64>>,
    #[serde(default)]
    close: Vec<Option<f64>>,
    #[serde(default)]
    volume: Vec<Option<f64>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartMeta {
    currency: Option<String>,
    instrument_type: Option<String>,
    regular_market_time: Option<i64>,
    regular_market_price: Option<f64>,
    regular_market_day_high: Option<f64>,
    regular_market_day_low: Option<f64>,
    regular_market_volume: Option<f64>,
    chart_previous_close: Option<f64>,
    previous_close: Option<f64>,
    current_trading_period: Option<TradingPeriods>,
}

#[derive(Debug, Deserialize)]
struct TradingPeriods {
    pre: Option<Period>,
    regular: Option<Period>,
    post: Option<Period>,
}

#[derive(Debug, Deserialize)]
struct Period {
    start: i64,
    end: i64,
}

#[derive(Debug)]
pub struct ChartData {
    meta: ChartMeta,
    pub candles: Vec<Candle>,
}

pub fn parse_chart(text: &str) -> PResult<ChartData> {
    let env: ChartEnvelope = parse_json(text)?;
    if let Some(err) = env.chart.error {
        let msg = err.description.unwrap_or_default();
        return Err(match err.code.as_deref() {
            Some("Not Found") => ProviderError::NotFound(msg),
            _ => ProviderError::Upstream(format!("Yahoo chart: {msg}")),
        });
    }
    let result = env
        .chart
        .result
        .and_then(|mut r| {
            if r.is_empty() {
                None
            } else {
                Some(r.swap_remove(0))
            }
        })
        .ok_or_else(|| ProviderError::NotFound("Yahoo no devolvió datos".into()))?;
    let q = result.indicators.quote.first();
    let get = |v: Option<&Vec<Option<f64>>>, i: usize| v.and_then(|v| v.get(i).copied().flatten());
    let mut candles = Vec::with_capacity(result.timestamp.len());
    for (i, &t) in result.timestamp.iter().enumerate() {
        // Una vela sin cierre es un hueco (mercado cerrado/halt): se descarta.
        let Some(c) = get(q.map(|q| &q.close), i).filter(|c| c.is_finite()) else {
            continue;
        };
        let o = get(q.map(|q| &q.open), i).unwrap_or(c);
        let h = get(q.map(|q| &q.high), i).unwrap_or(c).max(o).max(c);
        let l = get(q.map(|q| &q.low), i).unwrap_or(c).min(o).min(c);
        candles.push(Candle {
            t,
            o,
            h,
            l,
            c,
            v: get(q.map(|q| &q.volume), i),
        });
    }
    Ok(ChartData {
        meta: result.meta,
        candles,
    })
}

/// Con la sesión abierta, Yahoo añade al final de las series intradía un punto «en vivo» que no
/// es una vela: su hora es la del último trade (`regularMarketTime`, p. ej. 15:47:31 en una
/// serie de 5 min), `o=h=l=c` y volumen 0. Medido con el mercado abierto (13-oct-2026): sin
/// tratarlo, el gráfico recibe una vela fantasma desalineada dentro del tramo en curso.
///
/// Se reconoce porque su separación con la vela anterior no es múltiplo del intervalo (una vela
/// real, aunque haya un hueco, siempre cae en una frontera). Se pliega en el tramo que le
/// corresponde: si ese tramo ya tiene vela, actualiza su cierre/máximo/mínimo (el volumen no se
/// toca: el punto no trae ninguno); si no, pasa a ser la vela de ese tramo, alineada a su inicio.
/// Los intervalos diarios o mayores no llevan este punto.
pub fn fold_live_point(candles: &mut Vec<Candle>, interval: Interval) {
    let step = interval.seconds();
    if step >= Interval::D1.seconds() || candles.len() < 2 {
        return;
    }
    let (prev, last) = (candles[candles.len() - 2], candles[candles.len() - 1]);
    let delta = last.t - prev.t;
    if delta <= 0 || delta % step == 0 {
        return;
    }
    candles.pop();
    let bucket = prev.t + (delta / step) * step;
    if bucket == prev.t {
        if let Some(p) = candles.last_mut() {
            p.c = last.c;
            p.h = p.h.max(last.h);
            p.l = p.l.min(last.l);
        }
    } else {
        candles.push(Candle { t: bucket, ..last });
    }
}

fn state_from_periods(now_ts: i64, p: &TradingPeriods) -> MarketState {
    let within = |p: &Option<Period>| {
        p.as_ref()
            .is_some_and(|p| now_ts >= p.start && now_ts < p.end)
    };
    if within(&p.regular) {
        MarketState::Regular
    } else if within(&p.pre) {
        MarketState::Pre
    } else if within(&p.post) {
        MarketState::Post
    } else {
        MarketState::Closed
    }
}

/// Quote construida con la meta del chart (no necesita crumb; menos campos que `/v7/quote`).
pub fn quote_from_chart(id: &AssetId, data: &ChartData, now: DateTime<Utc>) -> Quote {
    let m = &data.meta;
    let last = data.candles.last();
    let price = m
        .regular_market_price
        .or(last.map(|c| c.c))
        .unwrap_or(f64::NAN);
    let prev = m
        .chart_previous_close
        .or(m.previous_close)
        .filter(|p| *p > 0.0);
    let state = if id.class == AssetClass::Crypto
        || m.instrument_type.as_deref() == Some("CRYPTOCURRENCY")
    {
        MarketState::Continuous
    } else {
        m.current_trading_period
            .as_ref()
            .map(|p| state_from_periods(now.timestamp(), p))
            .unwrap_or(MarketState::Unknown)
    };
    Quote {
        id: id.clone(),
        price,
        change: prev.map(|p| price - p),
        change_pct: prev.map(|p| (price / p - 1.0) * 100.0),
        open: last.map(|c| c.o),
        high: m.regular_market_day_high.or(last.map(|c| c.h)),
        low: m.regular_market_day_low.or(last.map(|c| c.l)),
        prev_close: prev,
        volume: m.regular_market_volume.or(last.and_then(|c| c.v)),
        market_cap: None,
        currency: m.currency.clone(),
        market_state: state,
        timestamp: m
            .regular_market_time
            .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
            .unwrap_or(now),
        source: "yahoo".into(),
        delay_secs: None,
        source_url: None,
        timestamp_origin: Some(if m.regular_market_time.is_some() {
            domain::TimestampOrigin::Provider
        } else {
            domain::TimestampOrigin::Local
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Quote en lote (/v7/finance/quote)
// ---------------------------------------------------------------------------------------------

fn market_state_from_str(s: &str) -> MarketState {
    match s {
        "PRE" | "PREPRE" => MarketState::Pre,
        "REGULAR" => MarketState::Regular,
        "POST" | "POSTPOST" => MarketState::Post,
        "CLOSED" => MarketState::Closed,
        _ => MarketState::Unknown,
    }
}

/// Retraso EFECTIVO del feed (segundos) de una cotización de `/v7/finance/quote`.
///
/// `exchangeDataDelayedBy` viene en **minutos**, no en segundos (medido el 13-oct-2026 con la bolsa abierta: GC=F y
/// CL=F declaran 10 y su `regularMarketTime` va ~605-640 s por detrás; ^RUT, ^VIX y ^TNX declaran 15 y van ~905-915 s
/// por detrás; ver `docs/notes/market.md` y el test `delay_is_in_minutes_and_real_time_labels_win`). Es el retraso
/// ESTÁNDAR de la bolsa para quien no paga su licencia (Nasdaq 15, NYSE 20, TSX 15, CME 10, Cboe 15), no el de Yahoo:
/// donde Yahoo sirve el dato en tiempo real el campo sigue valiendo 15/20 (AAPL: 15 con 3-11 s de antigüedad real).
/// Esos casos los delata `quoteSourceName` («Nasdaq Real Time Price», «Free Realtime Quote»), y entonces el retraso
/// es 0. Sin etiqueta de tiempo real se usa el campo (×60); sin campo, `None` (desconocido).
fn effective_delay_secs(r: &Value) -> Option<u32> {
    let label = r["quoteSourceName"].as_str().unwrap_or("").to_ascii_lowercase();
    if label.contains("real time") || label.contains("realtime") {
        return Some(0);
    }
    let minutes = r["exchangeDataDelayedBy"].as_f64().filter(|m| m.is_finite() && *m >= 0.0)?;
    Some((minutes * 60.0).round().min(f64::from(u32::MAX)) as u32)
}

/// Convierte la respuesta en cotizaciones usando el mapa símbolo de Yahoo → ids pedidos.
pub fn parse_quotes(text: &str, requested: &HashMap<String, Vec<&AssetId>>) -> PResult<Vec<Quote>> {
    let v: Value = parse_json(text)?;
    if let Some(err) = v["quoteResponse"]["error"].as_object() {
        let msg = err
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("error desconocido");
        return Err(ProviderError::Upstream(format!("Yahoo quote: {msg}")));
    }
    let mut out = Vec::new();
    for r in v["quoteResponse"]["result"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(sym) = r["symbol"].as_str() else {
            continue;
        };
        let Some(price) = r["regularMarketPrice"].as_f64().filter(|p| p.is_finite()) else {
            continue;
        };
        let ids = requested
            .get(sym)
            .or_else(|| requested.get(&sym.to_ascii_uppercase()));
        let Some(ids) = ids else { continue };
        let is_crypto = r["quoteType"].as_str() == Some("CRYPTOCURRENCY");
        let ts = r["regularMarketTime"]
            .as_i64()
            .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0));
        let origin = if ts.is_some() { domain::TimestampOrigin::Provider } else { domain::TimestampOrigin::Local };
        let ts = ts.unwrap_or_else(Utc::now);
        for id in ids {
            out.push(Quote {
                id: (*id).clone(),
                price,
                change: r["regularMarketChange"].as_f64(),
                change_pct: r["regularMarketChangePercent"].as_f64(),
                open: r["regularMarketOpen"].as_f64(),
                high: r["regularMarketDayHigh"].as_f64(),
                low: r["regularMarketDayLow"].as_f64(),
                prev_close: r["regularMarketPreviousClose"].as_f64(),
                volume: r["regularMarketVolume"].as_f64(),
                market_cap: r["marketCap"].as_f64(),
                currency: r["currency"].as_str().map(str::to_string),
                market_state: if is_crypto || id.class == AssetClass::Crypto {
                    MarketState::Continuous
                } else {
                    market_state_from_str(r["marketState"].as_str().unwrap_or(""))
                },
                timestamp: ts,
                source: "yahoo".into(),
                delay_secs: effective_delay_secs(r),
                source_url: None,
                timestamp_origin: Some(origin),
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// quoteSummary → Fundamentals
// ---------------------------------------------------------------------------------------------

/// Número de un campo de Yahoo, que llega como `{"raw": 1.2, "fmt": "1.20"}` (o `{}` si no hay).
fn raw(v: &Value) -> Option<f64> {
    match v {
        Value::Object(_) => v["raw"].as_f64().filter(|x| x.is_finite()),
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        _ => None,
    }
}

fn raw_date(v: &Value) -> Option<NaiveDate> {
    raw(v)
        .and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0))
        .map(|d| d.date_naive())
}

fn first_some(vals: &[Option<f64>]) -> Option<f64> {
    vals.iter().copied().flatten().next()
}

pub fn parse_quote_summary(text: &str, now: DateTime<Utc>) -> PResult<Fundamentals> {
    let v: Value = parse_json(text)?;
    if let Some(err) = v["quoteSummary"]["error"].as_object() {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("");
        let msg = err
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("error desconocido");
        return Err(if code == "Not Found" {
            ProviderError::NotFound(msg.into())
        } else {
            ProviderError::Upstream(format!("Yahoo quoteSummary: {msg}"))
        });
    }
    let r = v["quoteSummary"]["result"]
        .get(0)
        .ok_or_else(|| ProviderError::NotFound("Yahoo no devolvió fundamentales".into()))?;
    let (price, sd, ks, fd, ap) = (
        &r["price"],
        &r["summaryDetail"],
        &r["defaultKeyStatistics"],
        &r["financialData"],
        &r["assetProfile"],
    );
    let fund = &r["fundProfile"];

    let mut f = Fundamentals {
        description: ap["longBusinessSummary"].as_str().map(str::to_string),
        sector: ap["sector"].as_str().map(str::to_string),
        industry: ap["industry"].as_str().map(str::to_string),
        website: ap["website"].as_str().map(str::to_string),
        employees: ap["fullTimeEmployees"].as_u64(),
        country: ap["country"].as_str().map(str::to_string),
        // Moneda de cotización: la de la capitalización y los objetivos de analistas.
        currency: price["currency"]
            .as_str()
            .or_else(|| sd["currency"].as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string),

        market_cap: first_some(&[raw(&price["marketCap"]), raw(&sd["marketCap"])]),
        enterprise_value: raw(&ks["enterpriseValue"]),
        pe_trailing: raw(&sd["trailingPE"]),
        pe_forward: first_some(&[raw(&sd["forwardPE"]), raw(&ks["forwardPE"])]),
        peg: raw(&ks["pegRatio"]),
        price_to_book: raw(&ks["priceToBook"]),
        price_to_sales: raw(&sd["priceToSalesTrailing12Months"])
            .or_else(|| raw(&ks["priceToSalesTrailing12Months"])),
        ev_to_ebitda: raw(&ks["enterpriseToEbitda"]),

        eps_trailing: raw(&ks["trailingEps"]),
        eps_forward: raw(&ks["forwardEps"]),
        revenue_ttm: raw(&fd["totalRevenue"]),
        revenue_growth_yoy: raw(&fd["revenueGrowth"]),
        earnings_growth_yoy: raw(&fd["earningsGrowth"]),
        gross_margin: raw(&fd["grossMargins"]),
        operating_margin: raw(&fd["operatingMargins"]),
        profit_margin: first_some(&[raw(&fd["profitMargins"]), raw(&ks["profitMargins"])]),
        roe: raw(&fd["returnOnEquity"]),
        roa: raw(&fd["returnOnAssets"]),
        free_cash_flow: raw(&fd["freeCashflow"]),
        total_cash: raw(&fd["totalCash"]),
        total_debt: raw(&fd["totalDebt"]),
        // Yahoo da deuda/capital en porcentaje (78.4 = 0.784×); el dominio usa ratio.
        debt_to_equity: raw(&fd["debtToEquity"]).map(|d| d / 100.0),
        current_ratio: raw(&fd["currentRatio"]),

        dividend_yield: first_some(&[
            raw(&sd["dividendYield"]),
            // En ETFs/fondos el rendimiento por distribución viene en `yield`.
            raw(&ks["yield"]),
            raw(&sd["trailingAnnualDividendYield"]),
        ]),
        payout_ratio: raw(&sd["payoutRatio"]),
        beta: first_some(&[raw(&sd["beta"]), raw(&ks["beta"])]),
        shares_outstanding: raw(&ks["sharesOutstanding"]),
        short_pct_float: raw(&ks["shortPercentOfFloat"]),
        insider_pct: raw(&ks["heldPercentInsiders"]),
        institutional_pct: raw(&ks["heldPercentInstitutions"]),

        analyst_target_mean: raw(&fd["targetMeanPrice"]),
        analyst_target_high: raw(&fd["targetHighPrice"]),
        analyst_target_low: raw(&fd["targetLowPrice"]),
        analyst_rating_mean: raw(&fd["recommendationMean"]),
        analyst_count: raw(&fd["numberOfAnalystOpinions"]).map(|n| n as u32),
        next_earnings_date: r["calendarEvents"]["earnings"]["earningsDate"]
            .get(0)
            .and_then(raw_date),

        expense_ratio: first_some(&[
            raw(&fund["feesExpensesInvestment"]["annualReportExpenseRatio"]),
            raw(&fund["feesExpensesInvestment"]["netExpRatio"]),
            raw(&ks["annualReportExpenseRatio"]),
        ]),
        total_assets: first_some(&[raw(&ks["totalAssets"]), raw(&sd["totalAssets"])]),
        top_holdings: r["topHoldings"]["holdings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some(Holding {
                    symbol: h["symbol"].as_str().map(str::to_string),
                    name: h["holdingName"].as_str()?.to_string(),
                    weight: raw(&h["holdingPercent"])?,
                })
            })
            .collect(),
        sources: vec!["yahoo".into()],
        as_of: Some(now),
        ..Fundamentals::default()
    };

    // Datos que no encajan en campos tipados.
    let mut extra = |k: &str, val: Option<Value>| {
        if let Some(val) = val.filter(|v| !v.is_null()) {
            f.extra.insert(k.to_string(), val);
        }
    };
    extra(
        "recommendationKey",
        fd["recommendationKey"].as_str().map(|s| json!(s)),
    );
    extra("ebitda", raw(&fd["ebitda"]).map(|x| json!(x)));
    extra(
        "operatingCashflow",
        raw(&fd["operatingCashflow"]).map(|x| json!(x)),
    );
    extra("quickRatio", raw(&fd["quickRatio"]).map(|x| json!(x)));
    extra("bookValue", raw(&ks["bookValue"]).map(|x| json!(x)));
    extra(
        "exDividendDate",
        raw_date(&sd["exDividendDate"]).map(|d| json!(d)),
    );
    extra("averageVolume", raw(&sd["averageVolume"]).map(|x| json!(x)));
    extra(
        "financialCurrency",
        fd["financialCurrency"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(|s| json!(s)),
    );
    extra(
        "category",
        fund["categoryName"]
            .as_str()
            .or(ks["category"].as_str())
            .map(|s| json!(s)),
    );
    extra(
        "fundFamily",
        fund["family"]
            .as_str()
            .or(ks["fundFamily"].as_str())
            .map(|s| json!(s)),
    );
    extra("ytdReturn", raw(&ks["ytdReturn"]).map(|x| json!(x)));
    extra(
        "threeYearAverageReturn",
        raw(&ks["threeYearAverageReturn"]).map(|x| json!(x)),
    );
    extra(
        "fiveYearAverageReturn",
        raw(&ks["fiveYearAverageReturn"]).map(|x| json!(x)),
    );
    Ok(f)
}

// ---------------------------------------------------------------------------------------------
// Opciones
// ---------------------------------------------------------------------------------------------

pub fn parse_options(text: &str) -> PResult<OptionsChain> {
    let v: Value = parse_json(text)?;
    if let Some(err) = v["optionChain"]["error"].as_object() {
        let msg = err
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("error desconocido");
        return Err(ProviderError::Upstream(format!("Yahoo options: {msg}")));
    }
    let r = v["optionChain"]["result"]
        .get(0)
        .ok_or_else(|| ProviderError::NotFound("sin cadena de opciones".into()))?;
    let contracts = |key: &str, set: &Value| -> Vec<OptionContract> {
        set[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                Some(OptionContract {
                    symbol: c["contractSymbol"].as_str()?.to_string(),
                    strike: c["strike"].as_f64()?,
                    last_price: c["lastPrice"].as_f64(),
                    bid: c["bid"].as_f64(),
                    ask: c["ask"].as_f64(),
                    change: c["change"].as_f64(),
                    change_pct: c["percentChange"].as_f64(),
                    volume: c["volume"].as_f64(),
                    open_interest: c["openInterest"].as_f64(),
                    implied_volatility: c["impliedVolatility"].as_f64(),
                    in_the_money: c["inTheMoney"].as_bool(),
                    last_trade: c["lastTradeDate"]
                        .as_i64()
                        .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0)),
                })
            })
            .collect()
    };
    let set = &r["options"][0];
    Ok(OptionsChain {
        underlying: r["underlyingSymbol"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        spot: r["quote"]["regularMarketPrice"].as_f64(),
        expirations: r["expirationDates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect(),
        expiration: set["expirationDate"].as_i64(),
        calls: contracts("calls", set),
        puts: contracts("puts", set),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../tests/fixtures/yahoo_search_apple.json");
    const CHART_1Y: &str = include_str!("../../tests/fixtures/yahoo_chart_aapl_1y.json");
    const CHART_1D: &str = include_str!("../../tests/fixtures/yahoo_chart_aapl_1d.json");
    const CHART_404: &str = include_str!("../../tests/fixtures/yahoo_chart_notfound.json");
    const QUOTES: &str = include_str!("../../tests/fixtures/yahoo_quote_batch.json");
    /// 10 cotizaciones reales de `finance.yahoo.com/quote/AAPL` (mismos campos que `/v7/finance/quote`, con los
    /// `{raw, fmt}` aplanados a `raw`), capturadas el 13-oct-2026 a las 17:15:38 UTC con la bolsa de EE. UU. abierta.
    const QUOTES_DELAY: &str = include_str!("../../tests/fixtures/yahoo_quote_delay_units.json");
    /// Cabecera `Date` de esa captura (13-oct-2026 17:15:38 UTC).
    const QUOTES_DELAY_CAPTURED_AT: i64 = 1_791_911_738;
    const QUOTES_EMPTY: &str = include_str!("../../tests/fixtures/yahoo_quote_notfound.json");
    const SUMMARY_AAPL: &str = include_str!("../../tests/fixtures/yahoo_quotesummary_aapl.json");
    const SUMMARY_SPY: &str = include_str!("../../tests/fixtures/yahoo_quotesummary_spy.json");
    const OPTIONS: &str = include_str!("../../tests/fixtures/yahoo_options_aapl.json");

    #[test]
    fn maps_quote_types() {
        use AssetClass::*;
        for (qt, class) in [
            ("EQUITY", Stock),
            ("ETF", Etf),
            ("MUTUALFUND", Fund),
            ("INDEX", Index),
            ("CRYPTOCURRENCY", Crypto),
            ("CURRENCY", Forex),
            ("FUTURE", Future),
            ("OPTION", Option),
            ("MONEYMARKET", Other),
        ] {
            assert_eq!(class_from_quote_type(qt), class, "{qt}");
        }
    }

    #[test]
    fn translates_symbols() {
        let s = |c, sym: &str| to_yahoo_symbol(&AssetId::new(c, sym));
        assert_eq!(s(AssetClass::Crypto, "btc").as_deref(), Some("BTC-USD"));
        assert_eq!(s(AssetClass::Forex, "EUR/USD").as_deref(), Some("EURUSD=X"));
        assert_eq!(
            s(AssetClass::Forex, "EURUSD=X").as_deref(),
            Some("EURUSD=X")
        );
        assert_eq!(s(AssetClass::Bond, "DGS10").as_deref(), Some("^TNX"));
        assert_eq!(s(AssetClass::Bond, "T10Y2Y"), None);
        assert_eq!(s(AssetClass::Index, "^GSPC").as_deref(), Some("^GSPC"));
        assert_eq!(s(AssetClass::Stock, "SAN.MC").as_deref(), Some("SAN.MC"));
    }

    #[test]
    fn parses_search_with_news() {
        let (hits, news) = parse_search(SEARCH).unwrap();
        let aapl = hits
            .iter()
            .find(|h| h.summary.symbol == "AAPL")
            .expect("AAPL");
        assert_eq!(aapl.summary.class, AssetClass::Stock);
        assert_eq!(aapl.summary.name, "Apple Inc.");
        assert_eq!(aapl.summary.exchange.as_deref(), Some("NASDAQ"));
        assert_eq!(aapl.summary.id.to_string(), "stock:AAPL");
        // El futuro sobre acciones debe quedar como `future` y menos líquido que la acción.
        let fut = hits
            .iter()
            .find(|h| h.summary.symbol == "SAAPL=F")
            .expect("futuro");
        assert_eq!(fut.summary.class, AssetClass::Future);
        assert!(aapl.liquidity > fut.liquidity);
        assert!(!news.is_empty());
        let n = &news[0];
        assert_eq!(n.provider, "yahoo");
        assert_eq!(n.tickers, vec!["AAPL"]);
        assert_eq!(n.id.len(), 24);
    }

    #[test]
    fn crypto_search_hits_collapse_to_base_ticker() {
        let json = r#"{"quotes":[
            {"symbol":"BTC-USD","quoteType":"CRYPTOCURRENCY","shortname":"Bitcoin USD","exchange":"CCC","score":5000.0},
            {"symbol":"BTC-EUR","quoteType":"CRYPTOCURRENCY","shortname":"Bitcoin EUR","exchange":"CCC","score":4000.0}],
            "news":[]}"#;
        let (hits, _) = parse_search(json).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].summary.id.to_string(), "crypto:BTC");
    }

    #[test]
    fn parses_daily_history() {
        let d = parse_chart(CHART_1Y).unwrap();
        assert!(
            d.candles.len() > 240 && d.candles.len() < 260,
            "{}",
            d.candles.len()
        );
        assert!(d.candles.windows(2).all(|w| w[0].t < w[1].t));
        assert!(
            d.candles
                .iter()
                .all(|c| c.l <= c.o && c.o <= c.h && c.l <= c.c && c.c <= c.h)
        );
        assert!(d.candles.iter().all(|c| c.v.is_some()));
    }

    const CHART_5M_LIVE: &str = include_str!("../../tests/fixtures/yahoo_chart_aapl_5m_live.json");

    fn candle(t: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Candle {
        Candle { t, o, h, l, c, v: Some(v) }
    }

    #[test]
    fn live_point_of_an_open_session_is_folded_into_its_bucket() {
        // Respuesta real de Yahoo con la sesión abierta (13-oct-2026): la última «vela» es el punto
        // en vivo de las 15:47:36 con volumen 0, dentro del tramo de las 15:45.
        let mut d = parse_chart(CHART_5M_LIVE).unwrap();
        let n = d.candles.len();
        let [.., prev, live] = d.candles[..] else { panic!("faltan velas") };
        assert_eq!(live.v, Some(0.0));
        assert_ne!((live.t - prev.t) % 300, 0, "el punto en vivo no está alineado");
        fold_live_point(&mut d.candles, Interval::M5);
        assert_eq!(d.candles.len(), n - 1);
        let last = d.candles[n - 2];
        assert_eq!(last.t, prev.t);
        assert_eq!(last.c, live.c, "el cierre pasa a ser el último precio");
        assert_eq!(last.v, prev.v, "el volumen no se toca");
        assert!(last.h >= prev.h && last.l <= prev.l && last.l <= last.c && last.c <= last.h);
        assert!(d.candles.windows(2).all(|w| (w[1].t - w[0].t) % 300 == 0), "todas las velas alineadas");
    }

    #[test]
    fn live_point_in_a_new_bucket_becomes_that_buckets_candle() {
        // 1 min: la vela de las 15:46 aún no existe; el punto de las 15:46:41 la abre, alineado.
        let mut c = vec![
            candle(1_791_906_240, 1.0, 2.0, 0.5, 1.5, 100.0), // 15:44:00
            candle(1_791_906_300, 1.5, 3.0, 1.0, 2.5, 80.0),  // 15:45:00
            candle(1_791_906_401, 2.6, 2.6, 2.6, 2.6, 0.0),   // 15:46:41
        ];
        fold_live_point(&mut c, Interval::M1);
        assert_eq!(c.len(), 3);
        assert_eq!(c[2], candle(1_791_906_360, 2.6, 2.6, 2.6, 2.6, 0.0)); // 15:46:00
    }

    #[test]
    fn aligned_or_daily_series_are_left_alone() {
        // Con huecos (halt) pero todo en fronteras: nada que plegar.
        let gap = vec![
            candle(0, 1.0, 1.0, 1.0, 1.0, 5.0),
            candle(300, 1.0, 1.0, 1.0, 1.0, 5.0),
            candle(1200, 1.0, 1.0, 1.0, 1.0, 0.0),
        ];
        let mut c = gap.clone();
        fold_live_point(&mut c, Interval::M5);
        assert_eq!(c, gap);
        // Diario: nunca se toca, aunque la separación no sea múltiplo.
        let mut d = vec![candle(0, 1.0, 1.0, 1.0, 1.0, 5.0), candle(100_000, 2.0, 2.0, 2.0, 2.0, 5.0)];
        let before = d.clone();
        fold_live_point(&mut d, Interval::D1);
        assert_eq!(d, before);
        // Menos de 2 velas: no hay con qué comparar.
        let mut one = vec![candle(7, 1.0, 1.0, 1.0, 1.0, 0.0)];
        fold_live_point(&mut one, Interval::M5);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn hourly_series_with_half_hour_offset_folds_the_live_point() {
        // 60 min con inicio a :30 (apertura de EE. UU.): 15:30 + punto a las 15:46:47.
        let mut c = vec![
            candle(1_791_901_800, 1.0, 2.0, 0.5, 1.5, 100.0), // 14:30:00
            candle(1_791_905_400, 1.5, 3.0, 1.0, 2.5, 80.0),  // 15:30:00
            candle(1_791_906_407, 2.0, 2.0, 2.0, 2.0, 0.0),   // 15:46:47
        ];
        fold_live_point(&mut c, Interval::H1);
        assert_eq!(c.len(), 2);
        assert_eq!(c[1], candle(1_791_905_400, 1.5, 3.0, 1.0, 2.0, 80.0));
    }

    #[test]
    fn quote_from_chart_meta() {
        let d = parse_chart(CHART_1D).unwrap();
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        // Instante dentro de la sesión regular del fixture (regular: 1791552600..1791576000).
        let now = DateTime::<Utc>::from_timestamp(1_791_560_000, 0).unwrap();
        let q = quote_from_chart(&id, &d, now);
        assert_eq!(q.price, 336.64);
        assert_eq!(q.prev_close, Some(340.42));
        assert!((q.change.unwrap() - (336.64 - 340.42)).abs() < 1e-9);
        assert!((q.change_pct.unwrap() - (336.64 / 340.42 - 1.0) * 100.0).abs() < 1e-9);
        assert_eq!(q.market_state, MarketState::Regular);
        assert_eq!(q.currency.as_deref(), Some("USD"));
        assert_eq!(q.high, Some(338.61));
        let closed = DateTime::<Utc>::from_timestamp(1_791_700_000, 0).unwrap();
        assert_eq!(
            quote_from_chart(&id, &d, closed).market_state,
            MarketState::Closed
        );
    }

    #[test]
    fn chart_not_found_is_not_found() {
        assert!(matches!(
            parse_chart(CHART_404),
            Err(ProviderError::NotFound(_))
        ));
    }

    #[test]
    fn parses_batch_quotes() {
        let ids = [
            AssetId::new(AssetClass::Stock, "AAPL"),
            AssetId::new(AssetClass::Stock, "SAN.MC"),
            AssetId::new(AssetClass::Index, "^GSPC"),
            AssetId::new(AssetClass::Crypto, "BTC"),
            AssetId::new(AssetClass::Forex, "EURUSD=X"),
        ];
        let mut req: HashMap<String, Vec<&AssetId>> = HashMap::new();
        for id in &ids {
            req.entry(to_yahoo_symbol(id).unwrap())
                .or_default()
                .push(id);
        }
        let qs = parse_quotes(QUOTES, &req).unwrap();
        assert_eq!(qs.len(), 5);
        let get = |s: &str| qs.iter().find(|q| q.id.symbol == s).unwrap();
        let aapl = get("AAPL");
        assert_eq!(aapl.price, 336.64);
        assert_eq!(aapl.market_state, MarketState::Closed);
        // `exchangeDataDelayedBy: 15` (minutos, estándar de Nasdaq) pero «Nasdaq Real Time Price»: tiempo real.
        assert_eq!(aapl.delay_secs, Some(0));
        // «Delayed Quote» con `exchangeDataDelayedBy: 15`: 15 min = 900 s (antes se leía como 15 s).
        assert_eq!(get("SAN.MC").delay_secs, Some(900));
        assert_eq!(get("^GSPC").delay_secs, Some(0));
        assert!(aapl.market_cap.unwrap() > 4e12);
        // La cripto se devuelve con el id pedido (crypto:BTC) aunque Yahoo la llame BTC-USD.
        let btc = get("BTC");
        assert_eq!(btc.id.class, AssetClass::Crypto);
        assert_eq!(btc.market_state, MarketState::Continuous);
        assert_eq!(get("SAN.MC").currency.as_deref(), Some("EUR"));
    }

    /// Evidencia de la unidad de `exchangeDataDelayedBy`: con la sesión abierta, el desfase REAL entre la captura y
    /// `regularMarketTime` de los feeds «Delayed Quote» coincide con el campo tomado en MINUTOS (10 -> ~10 min,
    /// 15 -> ~15 min), y los feeds etiquetados de tiempo real van con segundos de antigüedad aunque el campo valga
    /// 15 o 20 (retraso estándar de la bolsa, no el del feed).
    #[test]
    fn delay_is_in_minutes_and_real_time_labels_win() {
        let ids: Vec<(String, AssetId)> = [
            ("AAPL", AssetClass::Stock),
            ("JPM", AssetClass::Stock),
            ("CNQ.TO", AssetClass::Stock),
            ("^GSPC", AssetClass::Index),
            ("BTC-USD", AssetClass::Crypto),
            ("GC=F", AssetClass::Future),
            ("CL=F", AssetClass::Future),
            ("^RUT", AssetClass::Index),
            ("^VIX", AssetClass::Index),
            ("^TNX", AssetClass::Index),
        ]
        .into_iter()
        .map(|(sym, class)| (sym.to_string(), AssetId::new(class, sym)))
        .collect();
        let mut req: HashMap<String, Vec<&AssetId>> = HashMap::new();
        for (sym, id) in &ids {
            req.entry(sym.clone()).or_default().push(id);
        }
        let qs = parse_quotes(QUOTES_DELAY, &req).unwrap();
        assert_eq!(qs.len(), 10);
        let raw: Value = serde_json::from_str(QUOTES_DELAY).unwrap();
        let field = |sym: &str, key: &str| {
            raw["quoteResponse"]["result"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["symbol"] == sym)
                .map(|r| r[key].clone())
                .unwrap_or_else(|| panic!("{sym} no está en el fixture"))
        };
        let lag = |q: &Quote| QUOTES_DELAY_CAPTURED_AT - q.timestamp.timestamp();
        for q in &qs {
            let sym = q.id.symbol.as_str();
            let declared_min = field(sym, "exchangeDataDelayedBy").as_f64().unwrap();
            let label = field(sym, "quoteSourceName");
            let real_time = label.as_str().is_some_and(|l| l.to_lowercase().contains("real"));
            if real_time || declared_min == 0.0 {
                // Tiempo real: segundos de antigüedad, también cuando el campo dice 15 o 20.
                assert_eq!(q.delay_secs, Some(0), "{sym}");
                assert!(lag(q) < 120, "{sym}: antigüedad {} s", lag(q));
            } else {
                // Feed diferido: el campo son MINUTOS y la antigüedad medida los confirma (±2 min).
                assert_eq!(q.delay_secs, Some(declared_min as u32 * 60), "{sym}");
                let expect = i64::from(q.delay_secs.unwrap());
                assert!((lag(q) - expect).abs() < 120, "{sym}: declarado {expect} s, antigüedad {} s", lag(q));
            }
        }
        let by = |s: &str| qs.iter().find(|q| q.id.symbol == s).unwrap();
        assert_eq!(by("GC=F").delay_secs, Some(600)); // COMEX: 10 min
        assert_eq!(by("CL=F").delay_secs, Some(600));
        assert_eq!(by("^VIX").delay_secs, Some(900)); // Cboe, sin `quoteSourceName`: 15 min
        assert_eq!(by("^RUT").delay_secs, Some(900)); // «Delayed Quote»: 15 min
        assert_eq!(by("JPM").delay_secs, Some(0)); // NYSE declara 20, pero Yahoo lo sirve en tiempo real
        assert_eq!(by("CNQ.TO").delay_secs, Some(0)); // TSX declara 15, «Free Realtime Quote»
        assert_eq!(by("AAPL").delay_secs, Some(0));
    }

    #[test]
    fn delay_edge_cases() {
        let d = |s: &str| effective_delay_secs(&serde_json::from_str::<Value>(s).unwrap());
        assert_eq!(d(r#"{}"#), None, "sin campo ni etiqueta: desconocido");
        assert_eq!(d(r#"{"exchangeDataDelayedBy":0}"#), Some(0));
        assert_eq!(d(r#"{"exchangeDataDelayedBy":20,"quoteSourceName":"Delayed Quote"}"#), Some(1200));
        assert_eq!(d(r#"{"exchangeDataDelayedBy":15.0}"#), Some(900), "también si llega como decimal");
        assert_eq!(d(r#"{"exchangeDataDelayedBy":-5}"#), None, "negativo = dato inválido");
        assert_eq!(d(r#"{"exchangeDataDelayedBy":"15"}"#), None, "texto = dato inválido");
        assert_eq!(d(r#"{"quoteSourceName":"Free Realtime Quote"}"#), Some(0), "etiqueta de tiempo real sin campo");
        assert_eq!(d(r#"{"exchangeDataDelayedBy":1e12}"#), Some(u32::MAX), "sin desbordar");
    }

    #[test]
    fn empty_quote_result_gives_no_quotes() {
        let id = AssetId::new(AssetClass::Stock, "ZZZZNOTREAL");
        let req = HashMap::from([("ZZZZNOTREAL".to_string(), vec![&id])]);
        assert!(parse_quotes(QUOTES_EMPTY, &req).unwrap().is_empty());
    }

    #[test]
    fn parses_stock_fundamentals() {
        let f = parse_quote_summary(SUMMARY_AAPL, Utc::now()).unwrap();
        assert_eq!(f.sector.as_deref(), Some("Technology"));
        assert_eq!(f.industry.as_deref(), Some("Consumer Electronics"));
        assert!(f.description.as_deref().unwrap().starts_with("Apple Inc."));
        assert!((f.pe_trailing.unwrap() - 38.605507).abs() < 1e-6);
        assert_eq!(f.revenue_ttm, Some(466_822_987_776.0));
        assert!((f.dividend_yield.unwrap() - 0.0032).abs() < 1e-9);
        assert!((f.debt_to_equity.unwrap() - 0.78445).abs() < 1e-9);
        assert!((f.roe.unwrap() - 1.4875101).abs() < 1e-9);
        assert_eq!(f.analyst_count, Some(39));
        assert_eq!(f.analyst_target_mean.map(|x| x.round()), Some(328.0));
        assert_eq!(f.next_earnings_date, NaiveDate::from_ymd_opt(2026, 11, 2));
        assert_eq!(f.extra.get("recommendationKey"), Some(&json!("buy")));
        assert_eq!(f.sources, vec!["yahoo"]);
        assert!(f.top_holdings.is_empty());
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.extra.get("financialCurrency"), Some(&json!("USD")));
    }

    #[test]
    fn fundamentals_currency_distinguishes_trading_and_financial() {
        // ADR: cotiza en USD pero publica cuentas en TWD.
        let t = r#"{"quoteSummary":{"error":null,"result":[{"price":{"currency":"USD","marketCap":{"raw":5.0e11}},"financialData":{"financialCurrency":"TWD","totalRevenue":{"raw":2.0e12}}}]}}"#;
        let f = parse_quote_summary(t, Utc::now()).unwrap();
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.extra.get("financialCurrency"), Some(&json!("TWD")));
        // Sin moneda en la respuesta: no se inventa.
        let t = r#"{"quoteSummary":{"error":null,"result":[{"price":{"marketCap":{"raw":5.0e11}}}]}}"#;
        let f = parse_quote_summary(t, Utc::now()).unwrap();
        assert_eq!(f.currency, None);
        assert!(!f.extra.contains_key("financialCurrency"));
    }

    #[test]
    fn parses_etf_fundamentals() {
        let f = parse_quote_summary(SUMMARY_SPY, Utc::now()).unwrap();
        assert_eq!(f.top_holdings.len(), 10);
        assert_eq!(f.top_holdings[0].symbol.as_deref(), Some("NVDA"));
        assert!((f.top_holdings[0].weight - 0.0836162).abs() < 1e-9);
        assert!((f.expense_ratio.unwrap() - 0.000945).abs() < 1e-9);
        assert_eq!(f.total_assets, Some(809_216_114_688.0));
        assert!((f.dividend_yield.unwrap() - 0.0099).abs() < 1e-9);
        assert_eq!(f.extra.get("category"), Some(&json!("Large Blend")));
    }

    #[test]
    fn parses_options_chain() {
        let o = parse_options(OPTIONS).unwrap();
        assert_eq!(o.underlying, "AAPL");
        assert_eq!(o.expirations.len(), 21);
        assert_eq!(o.expiration, Some(1_791_763_200));
        assert!(!o.calls.is_empty() && !o.puts.is_empty());
        let c = &o.calls[0];
        assert_eq!(c.symbol, "AAPL261012C00250000");
        assert_eq!(c.strike, 250.0);
        assert_eq!(c.in_the_money, Some(true));
        assert!(c.implied_volatility.unwrap() > 0.0);
    }

    #[test]
    fn macro_indicators_from_quotes() {
        let mk = |sym: &str, price: f64| Quote {
            id: AssetId::new(macro_class(sym), sym),
            price,
            change: Some(0.1),
            change_pct: None,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            volume: None,
            market_cap: None,
            currency: None,
            market_state: MarketState::Closed,
            timestamp: Utc::now(),
            source: "yahoo".into(),
            delay_secs: None,
            source_url: None,
            timestamp_origin: None,
        };
        let ind = macro_from_quotes(&[mk("^TNX", 4.2), mk("^VIX", 17.5)]);
        let keys: Vec<_> = ind.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["us10y", "vix"]);
        assert_eq!(ind[0].value, 4.2);
    }
}
