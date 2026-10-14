//! Twelve Data (`TWELVEDATA_API_KEY`). Respaldo de cotización e histórico de acciones, ETF,
//! divisas y cripto. Clave en la cabecera `Authorization: apikey ...`. Horas pedidas en UTC.

use std::sync::Mutex;

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

/// Plan gratuito: 800 créditos al día (1 por petición) con reinicio a las 00:00 UTC. Se deja
/// margen para no llegar al tope: el 429 de Twelve Data no distingue en la cabecera si es el
/// límite por minuto o el diario.
const DAILY_BUDGET: u32 = 780;

pub struct TwelveData {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
    base: String,
    /// Día UTC y peticiones hechas ese día.
    daily: Mutex<(NaiveDate, u32)>,
}

impl TwelveData {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self::with_base(http, key, BASE)
    }

    /// Igual que `new` con otra URL base (pruebas con un servidor local).
    pub fn with_base(http: Client, key: Option<Secret>, base: &str) -> Self {
        Self {
            http,
            base: base.to_string(),
            daily: Mutex::new((Utc::now().date_naive(), 0)),
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
        self.spend_credit()?;
        let mut url = Url::parse(&self.base).map_err(|e| ProviderError::upstream(e.to_string()))?;
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

impl TwelveData {
    /// Cuenta la petición contra el presupuesto diario; agotado, falla sin salir a la red.
    fn spend_credit(&self) -> PResult<()> {
        let now = Utc::now();
        let mut d = self.daily.lock().unwrap_or_else(|p| p.into_inner());
        if d.0 != now.date_naive() {
            *d = (now.date_naive(), 0);
        }
        if d.1 >= DAILY_BUDGET {
            let midnight = now.date_naive().succ_opt().and_then(|n| n.and_hms_opt(0, 0, 0));
            let wait = midnight.map_or(3600, |m| (m.and_utc() - now).num_seconds().max(60));
            return Err(ProviderError::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(wait as u64)),
            });
        }
        d.1 += 1;
        Ok(())
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
    // `timestamp` es el INICIO de la barra diaria (apertura de la sesión, o las 00:00 UTC en
    // cripto/forex), no la hora de la cotización: AAPL llegaba con 7 h menos de las reales.
    // `last_quote_at` es la hora de la última cotización.
    let ts = v["last_quote_at"]
        .as_i64()
        .filter(|t| *t > 0)
        .or_else(|| v["timestamp"].as_i64())
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
        // Forex y cripto no traen `currency`: es la divisa de cotización del par (`EUR/USD` -> USD).
        currency: v["currency"].as_str().map(str::to_string).or_else(|| {
            let (_, quote) = v["symbol"].as_str()?.split_once('/')?;
            Some(quote.to_string())
        }),
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

    // Respuestas REALES de Twelve Data (plan gratuito) capturadas el 14-oct-2026 con
    // `timezone=UTC`; ver `tests/fixtures/twelvedata_*.json`.
    fn fx(name: &str) -> Value {
        let p = format!("{}/tests/fixtures/twelvedata_{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn real_quote_uses_last_quote_time_not_bar_start() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let q = quote_from_json(&fx("quote_aapl"), &id).unwrap();
        assert_eq!(q.price, 341.26999);
        // `timestamp` = 13:30 UTC (apertura); `last_quote_at` = 19:59 UTC (última cotización).
        assert_eq!(q.timestamp.to_rfc3339(), "2026-10-13T19:59:00+00:00");
        assert_eq!(q.currency.as_deref(), Some("USD"));
        assert_eq!(q.market_state, MarketState::Closed);
        assert_eq!(q.prev_close, Some(340.97));
        assert_eq!(q.volume, Some(38_946_900.0));
        // `percent_change` ya viene en puntos porcentuales (0,088 %).
        assert!((q.change_pct.unwrap() - 0.0879807).abs() < 1e-6);
        let calc = (q.price / q.prev_close.unwrap() - 1.0) * 100.0;
        assert!((q.change_pct.unwrap() - calc).abs() < 1e-3);
        assert_eq!(q.timestamp_origin, Some(domain::TimestampOrigin::Provider));
        assert!(q.source_url.is_none() && q.delay_secs.is_none());
    }

    #[test]
    fn real_forex_and_crypto_quotes_get_quote_currency() {
        let eur = quote_from_json(&fx("quote_eurusd"), &AssetId::new(AssetClass::Forex, "EURUSD=X")).unwrap();
        assert_eq!(eur.price, 1.12008);
        assert_eq!(eur.currency.as_deref(), Some("USD"));
        assert_eq!(eur.market_state, MarketState::Regular);
        assert!(eur.volume.is_none());
        assert_eq!(eur.timestamp.timestamp(), 1_791_964_440);
        let btc = quote_from_json(&fx("quote_btcusd"), &AssetId::new(AssetClass::Crypto, "BTC")).unwrap();
        assert_eq!(btc.price, 83499.99);
        assert_eq!(btc.currency.as_deref(), Some("USD"));
        // 330,99 / 83169 = 0,398 % respecto al cierre previo (no el cambio rodante de 24 h).
        assert!((btc.change_pct.unwrap() - 0.39797280).abs() < 1e-6);
    }

    #[test]
    fn real_unknown_symbol_is_not_found() {
        // El cuerpo de error real (HTTP 404 + `status: error`) no es una cotización.
        let v = fx("quote_notfound");
        assert_eq!(v["code"], 404);
        assert!(quote_from_json(&v, &AssetId::new(AssetClass::Stock, "ZZZZNOPE")).is_err());
    }

    #[test]
    fn real_series_daily_and_intraday_in_utc() {
        let d = parse_time_series(&fx("series_aapl_1day")).unwrap();
        assert!(d.len() >= 10 && d.windows(2).all(|w| w[0].t < w[1].t));
        assert_eq!(chrono::DateTime::from_timestamp(d[0].t, 0).unwrap().to_rfc3339(), "2026-09-25T00:00:00+00:00");
        assert!(d[0].v.is_some() && d[0].h >= d[0].l);
        // Intradía con `timezone=UTC`: la primera barra de AAPL (9:30 ET) es 13:30 UTC.
        let h = parse_time_series(&fx("series_aapl_1h")).unwrap();
        assert_eq!(chrono::DateTime::from_timestamp(h[0].t, 0).unwrap().to_rfc3339(), "2026-10-13T13:30:00+00:00");
        assert_eq!(h.len(), 7);
        // Forex: sin volumen.
        let f = parse_time_series(&fx("series_eurusd_1h")).unwrap();
        assert_eq!(f.len(), 12);
        assert!(f.iter().all(|c| c.v.is_none()));
    }

    #[test]
    fn parses_series_edge_cases() {
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

    /// Servidor HTTP local: devuelve (estado, cuerpo) y guarda la última petición cruda.
    async fn serve(
        status: u16,
        body: &'static str,
    ) -> (String, std::sync::Arc<Mutex<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let last = std::sync::Arc::new(Mutex::new(String::new()));
        let l2 = last.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                let l2 = l2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let k = c.read(&mut buf).await.unwrap_or(0);
                    *l2.lock().unwrap() = String::from_utf8_lossy(&buf[..k]).to_string();
                    let _ = c
                        .write_all(format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes())
                        .await;
                });
            }
        });
        (format!("http://{addr}"), last)
    }

    #[tokio::test]
    async fn key_goes_in_authorization_header_never_in_the_url() {
        let (base, last) = serve(200, include_str!("../../tests/fixtures/twelvedata_quote_aapl.json")).await;
        let td = TwelveData::with_base(Client::new(), Some(Secret::new("k3y-secret").unwrap()), &base);
        let q = td.quote(&AssetId::new(AssetClass::Stock, "AAPL")).await.unwrap();
        assert_eq!(q.price, 341.26999);
        let req = last.lock().unwrap().clone();
        let line = req.lines().next().unwrap();
        assert!(line.starts_with("GET /quote?symbol=AAPL&timezone=UTC"), "{line}");
        assert!(!line.contains("k3y-secret"));
        assert!(req.to_ascii_lowercase().contains("authorization: apikey k3y-secret"));
    }

    #[tokio::test]
    async fn real_429_body_becomes_rate_limited() {
        let (base, _) = serve(429, include_str!("../../tests/fixtures/twelvedata_rate_limited.json")).await;
        let td = TwelveData::with_base(Client::new(), Some(Secret::new("k3y").unwrap()), &base);
        let r = td.quote(&AssetId::new(AssetClass::Forex, "EURUSD=X")).await;
        assert!(matches!(r, Err(ProviderError::RateLimited { .. })), "{r:?}");
        // El 404 real (símbolo inexistente) es `NotFound`, no un fallo del proveedor.
        let (base, _) = serve(404, include_str!("../../tests/fixtures/twelvedata_quote_notfound.json")).await;
        let td = TwelveData::with_base(Client::new(), Some(Secret::new("k3y").unwrap()), &base);
        let r = td.quote(&AssetId::new(AssetClass::Stock, "ZZZZNOPE")).await;
        assert!(matches!(r, Err(ProviderError::NotFound(_))), "{r:?}");
    }

    #[tokio::test]
    async fn daily_budget_stops_requests_without_touching_the_network() {
        let (base, last) = serve(200, "{}").await;
        let td = TwelveData::with_base(Client::new(), Some(Secret::new("k3y").unwrap()), &base);
        td.daily.lock().unwrap().1 = DAILY_BUDGET;
        let r = td.quote(&AssetId::new(AssetClass::Stock, "AAPL")).await;
        match r {
            Err(ProviderError::RateLimited { retry_after: Some(d) }) => assert!(d.as_secs() >= 60),
            o => panic!("{o:?}"),
        }
        assert!(last.lock().unwrap().is_empty(), "no debe salir ninguna petición");
    }
}
