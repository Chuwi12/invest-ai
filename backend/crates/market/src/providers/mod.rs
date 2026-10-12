//! Proveedores de datos. Cada módulo implementa solo las capacidades que ofrece su API; el resto
//! devuelve `Unsupported` y el enrutador pasa al siguiente proveedor.

use std::time::Duration;

use async_trait::async_trait;
use domain::{
    AssetClass, AssetId, AssetSummary, Candle, Fundamentals, Interval, MacroIndicator, NewsItem,
    Quote, Range,
};

pub mod alphavantage;
pub mod binance;
pub mod coingecko;
pub mod cryptopanic;
pub mod finnhub;
pub mod fmp;
pub mod fred;
pub mod gdelt;
pub mod google_news;
pub mod marketaux;
pub mod newsapi;
pub mod sec;
pub mod twelvedata;
pub mod yahoo;

// ---------------------------------------------------------------------------------------------
// Errores de proveedor
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// El proveedor no cubre esta operación o clase de activo (no cuenta como fallo).
    #[error("operación no soportada")]
    Unsupported,
    #[error("no encontrado: {0}")]
    NotFound(String),
    #[error("límite de peticiones alcanzado")]
    RateLimited { retry_after: Option<Duration> },
    #[error("{0}")]
    Upstream(String),
}

pub type PResult<T> = Result<T, ProviderError>;

impl ProviderError {
    pub fn upstream(msg: impl Into<String>) -> Self {
        Self::Upstream(msg.into())
    }
}

// ---------------------------------------------------------------------------------------------
// Capacidades e información del proveedor
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Search,
    Quote,
    Realtime,
    History,
    Fundamentals,
    News,
    Macro,
    Options,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Quote => "quote",
            Self::Realtime => "realtime",
            Self::History => "history",
            Self::Fundamentals => "fundamentals",
            Self::News => "news",
            Self::Macro => "macro",
            Self::Options => "options",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: &'static str,
    pub requires_key: bool,
    pub has_key: bool,
    pub capabilities: Vec<Capability>,
    pub classes: Vec<AssetClass>,
    /// Peticiones por segundo sostenidas y ráfaga permitida (token bucket).
    pub rate_per_sec: f64,
    pub burst: u32,
}

impl ProviderInfo {
    pub fn enabled(&self) -> bool {
        !self.requires_key || self.has_key
    }
}

// ---------------------------------------------------------------------------------------------
// Tipos auxiliares de las consultas
// ---------------------------------------------------------------------------------------------

/// Resultado de búsqueda con una pista de liquidez/relevancia comparable entre proveedores
/// (0 = nada líquido, 1 = muy líquido) que se usa al fusionar resultados.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub summary: AssetSummary,
    pub liquidity: f64,
}

/// Qué noticias se piden. `symbol`/`name` vacíos = noticias generales de mercado.
#[derive(Debug, Clone, Default)]
pub struct NewsQuery {
    pub id: Option<AssetId>,
    pub name: Option<String>,
    pub limit: usize,
}

impl NewsQuery {
    pub fn class(&self) -> Option<AssetClass> {
        self.id.as_ref().map(|i| i.class)
    }

    /// Ticker «limpio» para consultas de texto: `SAN.MC` → `SAN`, `BTC` → `BTC`.
    pub fn ticker(&self) -> Option<String> {
        self.id.as_ref().map(|i| plain_ticker(&i.symbol))
    }

    /// Clave estable de caché.
    pub fn cache_key(&self) -> String {
        match &self.id {
            Some(id) => format!("{}|{}", id, self.limit),
            None => format!("general|{}", self.limit),
        }
    }
}

/// `SAN.MC` → `SAN`, `BRK-B` → `BRK-B`, `^GSPC` → `GSPC`, `EURUSD=X` → `EURUSD`.
pub fn plain_ticker(symbol: &str) -> String {
    let s = symbol.trim_start_matches('^');
    let s = s.split('=').next().unwrap_or(s);
    match s.rsplit_once('.') {
        Some((base, suffix)) if suffix.len() <= 3 && !base.is_empty() => base.to_string(),
        _ => s.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------------------------

#[async_trait]
pub trait Provider: Send + Sync {
    fn info(&self) -> &ProviderInfo;

    async fn search(&self, _query: &str, _limit: usize) -> PResult<Vec<SearchHit>> {
        Err(ProviderError::Unsupported)
    }

    async fn quote(&self, _id: &AssetId) -> PResult<Quote> {
        Err(ProviderError::Unsupported)
    }

    /// Lote de cotizaciones. Los activos que el proveedor no conozca se omiten.
    async fn quotes(&self, _ids: &[AssetId]) -> PResult<Vec<Quote>> {
        Err(ProviderError::Unsupported)
    }

    async fn history(
        &self,
        _id: &AssetId,
        _range: Range,
        _interval: Interval,
    ) -> PResult<Vec<Candle>> {
        Err(ProviderError::Unsupported)
    }

    /// `Ok(None)` = el proveedor responde pero no tiene fundamentales de ese activo.
    async fn fundamentals(&self, _id: &AssetId) -> PResult<Option<Fundamentals>> {
        Err(ProviderError::Unsupported)
    }

    async fn news(&self, _q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        Err(ProviderError::Unsupported)
    }

    async fn macro_indicators(&self) -> PResult<Vec<MacroIndicator>> {
        Err(ProviderError::Unsupported)
    }
}

// ---------------------------------------------------------------------------------------------
// Utilidades comunes a los proveedores
// ---------------------------------------------------------------------------------------------

/// Los intervalos intradía tienen límites de rango en casi todos los proveedores. Si la
/// combinación pedida no es posible, sube al siguiente intervalo válido en vez de fallar.
pub fn coerce_interval(range: Range, interval: Interval) -> Interval {
    let max_days = |i: Interval| -> Option<i64> {
        match i {
            Interval::M1 => Some(7),
            Interval::M5 | Interval::M15 | Interval::M30 => Some(60),
            Interval::H1 => Some(730),
            _ => None,
        }
    };
    let range_days = match range {
        Range::D1 => 1,
        Range::D5 => 5,
        Range::M1 => 31,
        Range::M3 => 92,
        Range::M6 => 183,
        Range::Ytd | Range::Y1 => 366,
        Range::Y2 => 731,
        Range::Y5 => 1827,
        Range::Y10 => 3653,
        Range::Max => 36_500,
    };
    let order = [
        Interval::M1,
        Interval::M5,
        Interval::M15,
        Interval::M30,
        Interval::H1,
        Interval::D1,
    ];
    let Some(mut idx) = order.iter().position(|i| *i == interval) else {
        return interval; // semanal/mensual: sin límite
    };
    while idx < order.len() - 1 {
        match max_days(order[idx]) {
            Some(max) if range_days > max => idx += 1,
            _ => break,
        }
    }
    order[idx]
}

/// Instante desde milisegundos Unix (marca de tiempo que da el proveedor).
pub fn from_ms(ms: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
}

/// Instante desde segundos Unix.
pub fn from_secs(s: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(s, 0)
}

/// Fecha `YYYY-MM-DD` a medianoche UTC (series diarias de FRED, Alpha Vantage...).
pub fn from_ymd(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let d = chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc())
}

/// Número desde JSON aceptando también cadenas numéricas (`"123.4"`), habituales en APIs.
pub fn num(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

pub fn text(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Elimina etiquetas HTML y decodifica las entidades más comunes (resúmenes de RSS/APIs).
pub fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ticker_strips_suffixes() {
        assert_eq!(plain_ticker("SAN.MC"), "SAN");
        assert_eq!(plain_ticker("AAPL"), "AAPL");
        assert_eq!(plain_ticker("^GSPC"), "GSPC");
        assert_eq!(plain_ticker("EURUSD=X"), "EURUSD");
        assert_eq!(plain_ticker("BRK-B"), "BRK-B");
    }

    #[test]
    fn coerce_interval_respects_yahoo_limits() {
        assert_eq!(coerce_interval(Range::D5, Interval::M1), Interval::M1);
        assert_eq!(coerce_interval(Range::M1, Interval::M1), Interval::M5);
        assert_eq!(coerce_interval(Range::Y1, Interval::M5), Interval::H1);
        assert_eq!(coerce_interval(Range::Y5, Interval::H1), Interval::D1);
        assert_eq!(coerce_interval(Range::Y5, Interval::W1), Interval::W1);
    }

    #[test]
    fn strip_html_decodes_entities() {
        let s = strip_html(r#"<a href="x">Titular &amp; más</a>&nbsp;<font>Fuente</font>"#);
        assert_eq!(s, "Titular & más Fuente");
    }
}
