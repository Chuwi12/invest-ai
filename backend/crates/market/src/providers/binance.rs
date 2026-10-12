//! Binance (REST público + WebSocket `miniTicker`). Sin clave. Solo cripto frente a USDT.
//!
//! Se usan los hosts de «datos de mercado» (`data-api.binance.vision`, `data-stream.binance.vision`):
//! sirven los mismos endpoints públicos que `api.binance.com` y no aplican el bloqueo geográfico
//! de la API de trading. Las marcas de tiempo son las del proveedor (`closeTime`, `E`).

use async_trait::async_trait;
use chrono::{Datelike, Utc};
use domain::{AssetClass, AssetId, Candle, Interval, MarketState, Quote, Range};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{Capability, PResult, Provider, ProviderError, ProviderInfo, from_ms, num};
use crate::{
    http::{check_status, parse_json, send},
    realtime::{RawTick, WsFeed},
};

pub const REST_BASE: &str = "https://data-api.binance.vision";
pub const WS_BASE: &str = "wss://data-stream.binance.vision";

pub struct Binance {
    http: Client,
    info: ProviderInfo,
    base: String,
}

impl Binance {
    pub fn new(http: Client) -> Self {
        Self::with_base(http, REST_BASE.into())
    }

    pub fn with_base(http: Client, base: String) -> Self {
        Self {
            http,
            base,
            info: ProviderInfo {
                name: "binance",
                requires_key: false,
                has_key: false,
                capabilities: vec![Capability::Quote, Capability::Realtime, Capability::History],
                classes: vec![AssetClass::Crypto],
                // Peso de 24hr = 2 por símbolo; el límite de Binance es 6000 de peso/min.
                rate_per_sec: 10.0,
                burst: 20,
            },
        }
    }

    fn url(&self, path: &[&str]) -> PResult<Url> {
        let mut u = Url::parse(&self.base).map_err(|e| ProviderError::upstream(e.to_string()))?;
        u.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .extend(path);
        Ok(u)
    }

    /// GET interpretando el error `{"code":-1121,"msg":"Invalid symbol."}` como `NotFound`.
    async fn get(&self, url: Url, query: &[(&str, String)]) -> PResult<String> {
        let body = send(self.http.get(url).query(query)).await?;
        if body.status == reqwest::StatusCode::BAD_REQUEST
            && let Ok(v) = serde_json::from_str::<Value>(&body.text)
            && v["code"].as_i64() == Some(-1121)
        {
            return Err(ProviderError::NotFound("par no listado en Binance".into()));
        }
        check_status(&body)?;
        Ok(body.text)
    }
}

/// `BTC` → `BTCUSDT`. `None` si el símbolo no puede ser un activo base de Binance.
pub fn pair(id: &AssetId) -> Option<String> {
    if id.class != AssetClass::Crypto {
        return None;
    }
    let s = id.symbol.trim().to_ascii_uppercase();
    let base = s
        .strip_suffix("-USD")
        .or_else(|| s.strip_suffix("USDT"))
        .unwrap_or(&s);
    let ok = (2..=12).contains(&base.len()) && base.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| format!("{base}USDT"))
}

/// `BTCUSDT` → `BTC`.
pub fn base_of(pair: &str) -> Option<&str> {
    pair.strip_suffix("USDT").filter(|b| !b.is_empty())
}

fn binance_interval(i: Interval) -> &'static str {
    match i {
        Interval::W1 => "1w",
        Interval::Mo1 => "1M",
        other => other.as_str(),
    }
}

fn range_start_ms(range: Range, now: chrono::DateTime<Utc>) -> Option<i64> {
    let days = match range {
        Range::D1 => 1,
        Range::D5 => 5,
        Range::M1 => 31,
        Range::M3 => 92,
        Range::M6 => 183,
        Range::Y1 => 366,
        Range::Y2 => 731,
        Range::Y5 => 1827,
        Range::Y10 => 3653,
        Range::Ytd => {
            let jan1 = chrono::NaiveDate::from_ymd_opt(now.year(), 1, 1)?;
            return Some(jan1.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis());
        }
        Range::Max => return None,
    };
    Some((now - chrono::Duration::days(days)).timestamp_millis())
}

// ---------------------------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------------------------

#[async_trait]
impl Provider for Binance {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let symbol = pair(id).ok_or(ProviderError::Unsupported)?;
        let url = self.url(&["api", "v3", "ticker", "24hr"])?;
        let text = self.get(url, &[("symbol", symbol)]).await?;
        let v: Value = parse_json(&text)?;
        quote_from_ticker(&v, Some(id))
    }

    async fn quotes(&self, ids: &[AssetId]) -> PResult<Vec<Quote>> {
        let pairs: Vec<String> = ids.iter().filter_map(pair).collect();
        if pairs.is_empty() {
            return Err(ProviderError::Unsupported);
        }
        let mut out = Vec::new();
        for chunk in pairs.chunks(100) {
            let url = self.url(&["api", "v3", "ticker", "24hr"])?;
            let list =
                serde_json::to_string(chunk).map_err(|e| ProviderError::upstream(e.to_string()))?;
            match self.get(url, &[("symbols", list)]).await {
                Ok(text) => out.extend(parse_ticker_batch(&text)?),
                // Un par inexistente invalida todo el lote: se reintentan uno a uno.
                Err(ProviderError::NotFound(_)) => {
                    for p in chunk {
                        let id = AssetId::new(AssetClass::Crypto, base_of(p).unwrap_or(p));
                        if let Ok(q) = self.quote(&id).await {
                            out.push(q);
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> PResult<Vec<Candle>> {
        let symbol = pair(id).ok_or(ProviderError::Unsupported)?;
        let url = self.url(&["api", "v3", "klines"])?;
        let mut q = vec![
            ("symbol", symbol),
            ("interval", binance_interval(interval).to_string()),
            ("limit", "1000".to_string()),
        ];
        if let Some(start) = range_start_ms(range, Utc::now()) {
            q.push(("startTime", start.to_string()));
        }
        let text = self.get(url, &q).await?;
        parse_klines(&text)
    }
}

// ---------------------------------------------------------------------------------------------
// Parseo
// ---------------------------------------------------------------------------------------------

/// Quote desde un objeto de `/ticker/24hr`. `id` fuerza el identificador; sin él se deduce del par.
pub fn quote_from_ticker(v: &Value, id: Option<&AssetId>) -> PResult<Quote> {
    let sym = v["symbol"]
        .as_str()
        .ok_or_else(|| ProviderError::upstream("ticker sin símbolo"))?;
    let id = match id {
        Some(i) => i.clone(),
        None => AssetId::new(
            AssetClass::Crypto,
            base_of(sym).ok_or_else(|| ProviderError::upstream("par que no es USDT"))?,
        ),
    };
    let price = num(&v["lastPrice"])
        .filter(|p| *p > 0.0)
        .ok_or_else(|| ProviderError::upstream("ticker sin precio"))?;
    let provider_ts = v["closeTime"].as_i64().and_then(from_ms);
    let origin = if provider_ts.is_some() { domain::TimestampOrigin::Provider } else { domain::TimestampOrigin::Local };
    let ts = provider_ts.unwrap_or_else(Utc::now);
    Ok(Quote {
        id,
        price,
        change: num(&v["priceChange"]),
        change_pct: num(&v["priceChangePercent"]),
        open: num(&v["openPrice"]),
        high: num(&v["highPrice"]),
        low: num(&v["lowPrice"]),
        prev_close: num(&v["prevClosePrice"]),
        volume: num(&v["volume"]),
        market_cap: None,
        currency: Some("USDT".into()),
        market_state: MarketState::Continuous,
        timestamp: ts,
        source: "binance".into(),
        delay_secs: Some(0),
        source_url: None,
        timestamp_origin: Some(origin),
    })
}

pub fn parse_ticker_batch(text: &str) -> PResult<Vec<Quote>> {
    let v: Value = parse_json(text)?;
    let arr = v
        .as_array()
        .ok_or_else(|| ProviderError::upstream("se esperaba una lista de tickers"))?;
    Ok(arr
        .iter()
        .filter_map(|t| quote_from_ticker(t, None).ok())
        .collect())
}

pub fn parse_klines(text: &str) -> PResult<Vec<Candle>> {
    let v: Value = parse_json(text)?;
    let arr = v
        .as_array()
        .ok_or_else(|| ProviderError::upstream("se esperaba una lista de velas"))?;
    let mut out = Vec::with_capacity(arr.len());
    for k in arr {
        let Some(k) = k.as_array() else { continue };
        let (Some(t), Some(o), Some(h), Some(l), Some(c)) = (
            k.first().and_then(Value::as_i64),
            k.get(1).and_then(num),
            k.get(2).and_then(num),
            k.get(3).and_then(num),
            k.get(4).and_then(num),
        ) else {
            continue;
        };
        out.push(Candle {
            t: t / 1000,
            o,
            h,
            l,
            c,
            v: k.get(5).and_then(num),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

/// Feed de `@miniTicker` sobre una sola conexión (`/ws`) con suscripciones dinámicas.
pub struct BinanceFeed {
    pub ws_base: String,
}

impl Default for BinanceFeed {
    fn default() -> Self {
        Self {
            ws_base: WS_BASE.into(),
        }
    }
}

impl WsFeed for BinanceFeed {
    fn name(&self) -> &'static str {
        "binance"
    }

    fn url(&self) -> String {
        format!("{}/ws", self.ws_base)
    }

    fn subscribe_msgs(&self, symbols: &[String], on: bool) -> Vec<String> {
        // Binance admite hasta 1024 streams por conexión; se envían en bloques de 100.
        symbols
            .chunks(100)
            .map(|chunk| {
                let params: Vec<String> = chunk
                    .iter()
                    .map(|s| format!("{}@miniTicker", s.to_ascii_lowercase()))
                    .collect();
                serde_json::json!({
                    "method": if on { "SUBSCRIBE" } else { "UNSUBSCRIBE" },
                    "params": params,
                    "id": 1
                })
                .to_string()
            })
            .collect()
    }

    fn parse(&self, text: &str) -> Vec<RawTick> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        // En streams combinados el evento viene envuelto en `data`.
        let e = if v.get("data").is_some() {
            &v["data"]
        } else {
            &v
        };
        if e["e"].as_str() != Some("24hrMiniTicker") {
            return Vec::new();
        }
        let (Some(symbol), Some(price)) = (e["s"].as_str(), num(&e["c"])) else {
            return Vec::new();
        };
        if price <= 0.0 {
            return Vec::new();
        }
        vec![RawTick {
            symbol: symbol.to_string(),
            price,
            open24h: num(&e["o"]).filter(|o| *o > 0.0),
            volume: num(&e["v"]),
            ts: e["E"].as_i64().and_then(from_ms).unwrap_or_else(Utc::now),
        }]
    }
}

// ---------------------------------------------------------------------------------------------
// Tests (fixtures reales de Binance)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const KLINES: &str = include_str!("../../tests/fixtures/binance_klines_btcusdt_1d.json");
    const T24: &str = include_str!("../../tests/fixtures/binance_ticker24hr_btcusdt.json");
    const BATCH: &str = include_str!("../../tests/fixtures/binance_ticker24hr_batch.json");

    #[test]
    fn pair_mapping() {
        let id = |s: &str| AssetId::new(AssetClass::Crypto, s);
        assert_eq!(pair(&id("btc")).unwrap(), "BTCUSDT");
        assert_eq!(pair(&id("BTC-USD")).unwrap(), "BTCUSDT");
        assert_eq!(pair(&id("ETHUSDT")).unwrap(), "ETHUSDT");
        assert!(pair(&id("B")).is_none());
        assert!(pair(&id("BT C")).is_none());
        assert!(pair(&AssetId::new(AssetClass::Stock, "AAPL")).is_none());
    }

    #[test]
    fn parses_ticker_with_provider_time() {
        let v: Value = serde_json::from_str(T24).unwrap();
        let q = quote_from_ticker(&v, None).unwrap();
        assert_eq!(q.id.to_string(), "crypto:BTC");
        assert_eq!(q.price, 83047.0);
        assert!((q.change_pct.unwrap() - 0.201).abs() < 1e-9);
        assert_eq!(q.prev_close, Some(82880.01));
        assert_eq!(q.timestamp.timestamp_millis(), 1_791_710_295_013);
        assert_eq!(q.market_state, MarketState::Continuous);
        assert_eq!(q.source, "binance");
    }

    #[test]
    fn parses_batch() {
        let qs = parse_ticker_batch(BATCH).unwrap();
        assert!(qs.len() >= 2);
        assert!(qs.iter().any(|q| q.id.symbol == "ETH" && q.price > 0.0));
    }

    #[test]
    fn parses_klines() {
        let c = parse_klines(KLINES).unwrap();
        assert!(c.len() > 10);
        assert_eq!(c[0].t, 1_789_171_200);
        assert_eq!(c[0].o, 77225.71);
        assert_eq!(c[0].c, 77278.73);
        assert!(c.windows(2).all(|w| w[0].t < w[1].t));
        assert!(c.iter().all(|k| k.h >= k.l && k.v.is_some()));
    }

    #[test]
    fn garbage_is_error_not_panic() {
        assert!(parse_klines("{}").is_err());
        assert!(parse_klines("[[1,\"x\"]]").unwrap().is_empty());
        assert!(parse_ticker_batch("nope").is_err());
        let v: Value = serde_json::json!({"symbol":"BTCUSDT","lastPrice":"0"});
        assert!(quote_from_ticker(&v, None).is_err());
    }

    #[test]
    fn miniticker_ws_message() {
        let f = BinanceFeed::default();
        let msg = r#"{"e":"24hrMiniTicker","E":1791710295000,"s":"BTCUSDT","c":"83100.5","o":"82880.0","h":"83234","l":"82778","v":"6271.9","q":"520000000"}"#;
        let t = f.parse(msg);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].symbol, "BTCUSDT");
        assert_eq!(t[0].price, 83100.5);
        assert_eq!(t[0].open24h, Some(82880.0));
        assert_eq!(t[0].ts.timestamp_millis(), 1_791_710_295_000);
        // Respuesta a SUBSCRIBE y mensajes basura: sin ticks.
        assert!(f.parse(r#"{"result":null,"id":1}"#).is_empty());
        assert!(f.parse("basura").is_empty());
        let combined = format!(r#"{{"stream":"btcusdt@miniTicker","data":{msg}}}"#);
        assert_eq!(f.parse(&combined).len(), 1);
        let subs = f.subscribe_msgs(&["BTCUSDT".into(), "ETHUSDT".into()], true);
        assert_eq!(subs.len(), 1);
        assert!(subs[0].contains("btcusdt@miniTicker") && subs[0].contains("SUBSCRIBE"));
    }
}
