//! Twelve Data (`TWELVEDATA_API_KEY`). Respaldo de cotización e histórico de acciones, ETF,
//! divisas y cripto. Clave en la cabecera `Authorization: apikey ...`. Horas pedidas en UTC.

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveDateTime, Utc};
use domain::{AssetClass, AssetId, Candle, Interval, MarketState, Quote, Range};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, PResult, Provider, ProviderError, ProviderInfo, from_secs, num, us_symbol,
};
use crate::{
    http::{check_status, parse_json, send},
    secret::Secret,
};

const BASE: &str = "https://api.twelvedata.com";

pub struct TwelveData {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl TwelveData {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "twelvedata",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::Quote, Capability::History],
                classes: vec![
                    AssetClass::Stock,
                    AssetClass::Etf,
                    AssetClass::Forex,
                    AssetClass::Crypto,
                ],
                // Plan gratuito: 8 peticiones/min.
                rate_per_sec: 0.13,
                burst: 2,
            },
            key,
        }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> PResult<Value> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .push(path);
        let body = send(
            self.http
                .get(url)
                .query(query)
                .header("Authorization", format!("apikey {}", key.expose())),
        )
        .await?;
        // Twelve Data devuelve errores con HTTP 200 y `{"status":"error","code":...}`.
        if let Ok(v) = serde_json::from_str::<Value>(&body.text) {
            if v["status"].as_str() == Some("error") {
                let msg = v["message"].as_str().unwrap_or("error");
                return Err(match v["code"].as_i64() {
                    Some(429) => ProviderError::RateLimited {
                        retry_after: Some(std::time::Duration::from_secs(30)),
                    },
                    Some(404) => ProviderError::NotFound(msg.chars().take(80).collect()),
                    _ => ProviderError::Upstream(format!(
                        "Twelve Data: {}",
                        msg.chars().take(120).collect::<String>()
                    )),
                });
            }
            check_status(&body)?;
            return Ok(v);
        }
        check_status(&body)?;
        parse_json(&body.text)
    }
}

/// Símbolo de Twelve Data: `EUR/USD`, `BTC/USD`, `AAPL`.
pub fn td_symbol(id: &AssetId) -> Option<String> {
    match id.class {
        AssetClass::Stock | AssetClass::Etf => us_symbol(id).map(|s| s.replace('.', "-")),
        AssetClass::Crypto => {
            let s = id.symbol.trim().to_ascii_uppercase();
            let base = s.strip_suffix("-USD").unwrap_or(&s);
            (!base.is_empty() && base.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| format!("{base}/USD"))
        }
        AssetClass::Forex => {
            let t = id.symbol.replace(['/', '='], "").to_ascii_uppercase();
            let t = t
                .strip_suffix('X')
                .filter(|_| t.len() == 7)
                .unwrap_or(&t)
                .to_string();
            (t.len() == 6 && t.chars().all(|c| c.is_ascii_alphabetic()))
                .then(|| format!("{}/{}", &t[..3], &t[3..]))
        }
        _ => None,
    }
}

fn td_interval(i: Interval) -> &'static str {
    match i {
        Interval::M1 => "1min",
        Interval::M5 => "5min",
        Interval::M15 => "15min",
        Interval::M30 => "30min",
        Interval::H1 => "1h",
        Interval::D1 => "1day",
        Interval::W1 => "1week",
        Interval::Mo1 => "1month",
    }
}

#[async_trait]
impl Provider for TwelveData {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let sym = td_symbol(id).ok_or(ProviderError::Unsupported)?;
        let v = self
            .get("quote", &[("symbol", sym), ("timezone", "UTC".into())])
            .await?;
        quote_from_json(&v, id)
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> PResult<Vec<Candle>> {
        let sym = td_symbol(id).ok_or(ProviderError::Unsupported)?;
        let interval = super::coerce_interval(range, interval);
        let days = match range {
            Range::D1 => 2,
            Range::D5 => 7,
            Range::M1 => 31,
            Range::M3 => 92,
            Range::M6 => 183,
            Range::Ytd | Range::Y1 => 366,
            Range::Y2 => 731,
            Range::Y5 => 1827,
            Range::Y10 => 3653,
            Range::Max => 7300,
        };
        let start = (Utc::now() - chrono::Duration::days(days))
            .format("%Y-%m-%d")
            .to_string();
        let v = self
            .get(
                "time_series",
                &[
                    ("symbol", sym),
                    ("interval", td_interval(interval).into()),
                    ("start_date", start),
                    ("outputsize", "5000".into()),
                    ("order", "asc".into()),
                    ("timezone", "UTC".into()),
                ],
            )
            .await?;
        parse_time_series(&v)
    }
}

pub fn quote_from_json(v: &Value, id: &AssetId) -> PResult<Quote> {
    let price = num(&v["close"])
        .filter(|p| *p > 0.0)
        .ok_or_else(|| ProviderError::NotFound("sin cotización".into()))?;
    let ts = v["timestamp"]
        .as_i64()
        .and_then(from_secs)
        .ok_or_else(|| ProviderError::upstream("cotización sin marca de tiempo"))?;
    Ok(Quote {
        id: id.clone(),
        price,
        change: num(&v["change"]),
        change_pct: num(&v["percent_change"]),
        open: num(&v["open"]),
        high: num(&v["high"]),
        low: num(&v["low"]),
        prev_close: num(&v["previous_close"]),
        volume: num(&v["volume"]),
        market_cap: None,
        currency: v["currency"].as_str().map(str::to_string),
        market_state: match v["is_market_open"].as_bool() {
            Some(true) => MarketState::Regular,
            Some(false) => MarketState::Closed,
            None => MarketState::Unknown,
        },
        timestamp: ts,
        source: "twelvedata".into(),
        delay_secs: None,
        source_url: None,
        timestamp_origin: Some(domain::TimestampOrigin::Provider),
    })
}

pub fn parse_time_series(v: &Value) -> PResult<Vec<Candle>> {
    let vals = v["values"]
        .as_array()
        .ok_or_else(|| ProviderError::NotFound("serie vacía".into()))?;
    let mut out: Vec<Candle> = Vec::with_capacity(vals.len());
    for r in vals {
        let Some(dt) = r["datetime"].as_str() else {
            continue;
        };
        let t = NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|d| d.and_utc().timestamp())
            .or_else(|| {
                NaiveDate::parse_from_str(dt, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp()))
            });
        let (Some(t), Some(o), Some(h), Some(l), Some(c)) = (
            t,
            num(&r["open"]),
            num(&r["high"]),
            num(&r["low"]),
            num(&r["close"]),
        ) else {
            continue;
        };
        out.push(Candle {
            t,
            o,
            h,
            l,
            c,
            v: num(&r["volume"]),
        });
    }
    out.sort_by_key(|c| c.t);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Formas documentadas en twelvedata.com/docs (la API exige clave: no hay fixture real).
    #[test]
    fn parses_quote() {
        let v = json!({"symbol":"AAPL","currency":"USD","timestamp":1760040000,"open":"261.07","high":"263.31","low":"260.68","close":"261.74","volume":"38000000","previous_close":"262.84","change":"-1.10","percent_change":"-0.42","is_market_open":false});
        let q = quote_from_json(&v, &AssetId::new(AssetClass::Stock, "AAPL")).unwrap();
        assert_eq!(q.price, 261.74);
        assert_eq!(q.market_state, MarketState::Closed);
        assert_eq!(q.timestamp.timestamp(), 1_760_040_000);
    }

    #[test]
    fn parses_series_daily_and_intraday() {
        let v = json!({"values":[
            {"datetime":"2026-10-09","open":"1","high":"3","low":"0.5","close":"2","volume":"10"},
            {"datetime":"2026-10-08 14:30:00","open":"1","high":"3","low":"0.5","close":"2"},
            {"datetime":"basura","open":"1","high":"3","low":"0.5","close":"2"}]});
        let c = parse_time_series(&v).unwrap();
        assert_eq!(c.len(), 2);
        assert!(c[0].t < c[1].t);
        assert!(parse_time_series(&json!({})).is_err());
    }

    #[test]
    fn symbols() {
        let s = |c, x: &str| td_symbol(&AssetId::new(c, x));
        assert_eq!(s(AssetClass::Forex, "EURUSD=X").as_deref(), Some("EUR/USD"));
        assert_eq!(s(AssetClass::Forex, "EUR/USD").as_deref(), Some("EUR/USD"));
        assert_eq!(s(AssetClass::Crypto, "btc").as_deref(), Some("BTC/USD"));
        assert_eq!(s(AssetClass::Stock, "AAPL").as_deref(), Some("AAPL"));
        assert!(s(AssetClass::Stock, "SAN.MC").is_none());
        assert!(s(AssetClass::Index, "^GSPC").is_none());
    }
}
