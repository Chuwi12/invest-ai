//! Finnhub (`FINNHUB_API_KEY`). Cotización de acciones/ETF de EE. UU., perfil y métricas,
//! recomendaciones de analistas, noticias por empresa y WebSocket de trades en tiempo real.
//! La clave viaja en la cabecera `X-Finnhub-Token`, nunca en la URL de REST.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use domain::{AssetClass, AssetId, Fundamentals, MarketState, NewsItem, Quote};
use reqwest::Client;
use serde_json::{Value, json};
use url::Url;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, from_ms, from_secs, num,
    text, us_symbol,
};
use crate::{
    http::{check_status, parse_json, send},
    news::item_id,
    realtime::{RawTick, WsFeed},
    secret::Secret,
};

const BASE: &str = "https://finnhub.io/api/v1";
pub const WS_URL: &str = "wss://ws.finnhub.io";

pub struct Finnhub {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl Finnhub {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "finnhub",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![
                    Capability::Quote,
                    Capability::Realtime,
                    Capability::Fundamentals,
                    Capability::News,
                ],
                classes: vec![AssetClass::Stock, AssetClass::Etf],
                // Plan gratuito: 60 peticiones/min.
                rate_per_sec: 1.0,
                burst: 10,
            },
            key,
        }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> PResult<Value> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .extend(path.split('/'));
        let body = send(
            self.http
                .get(url)
                .query(query)
                .header("X-Finnhub-Token", key.expose()),
        )
        .await?;
        check_status(&body)?;
        let v: Value = parse_json(&body.text)?;
        if let Some(e) = v["error"].as_str() {
            return Err(ProviderError::Upstream(format!("Finnhub: {e}")));
        }
        Ok(v)
    }
}

#[async_trait]
impl Provider for Finnhub {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let v = self.get("quote", &[("symbol", sym)]).await?;
        quote_from_json(&v, id)
    }

    async fn fundamentals(&self, id: &AssetId) -> PResult<Option<Fundamentals>> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let by_symbol = [("symbol", sym.clone())];
        let with_metric = [("symbol", sym.clone()), ("metric", "all".to_string())];
        let (profile, metric, rec) = futures::join!(
            self.get("stock/profile2", &by_symbol),
            self.get("stock/metric", &with_metric),
            self.get("stock/recommendation", &by_symbol),
        );
        // El perfil decide si el activo existe; métricas y recomendaciones son complementos.
        let profile = profile?;
        if profile.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(None);
        }
        Ok(Some(fundamentals_from_json(
            &profile,
            metric.ok().as_ref(),
            rec.ok().as_ref(),
        )))
    }

    async fn news(&self, qy: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let v = match &qy.id {
            Some(id) => {
                let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
                let to = Utc::now().date_naive();
                let from = to - Duration::days(7);
                self.get(
                    "company-news",
                    &[
                        ("symbol", sym),
                        ("from", from.to_string()),
                        ("to", to.to_string()),
                    ],
                )
                .await?
            }
            None => self.get("news", &[("category", "general".into())]).await?,
        };
        let mut items = parse_news(&v, qy.id.as_ref().and_then(us_symbol).as_deref());
        items.truncate(qy.limit.clamp(1, 100));
        Ok(items)
    }
}

pub fn quote_from_json(v: &Value, id: &AssetId) -> PResult<Quote> {
    let price = num(&v["c"]).filter(|p| *p > 0.0);
    let ts = v["t"].as_i64().filter(|t| *t > 0).and_then(from_secs);
    // Finnhub responde ceros para símbolos que no conoce.
    let (Some(price), Some(ts)) = (price, ts) else {
        return Err(ProviderError::NotFound(format!(
            "{} sin cotización en Finnhub",
            id.symbol
        )));
    };
    Ok(Quote {
        id: id.clone(),
        price,
        change: num(&v["d"]),
        change_pct: num(&v["dp"]),
        open: num(&v["o"]).filter(|x| *x > 0.0),
        high: num(&v["h"]).filter(|x| *x > 0.0),
        low: num(&v["l"]).filter(|x| *x > 0.0),
        prev_close: num(&v["pc"]).filter(|x| *x > 0.0),
        volume: None,
        market_cap: None,
        currency: Some("USD".into()),
        market_state: MarketState::Unknown,
        timestamp: ts,
        source: "finnhub".into(),
        delay_secs: None,
        source_url: None,
        timestamp_origin: Some(domain::TimestampOrigin::Provider),
    })
}

pub fn fundamentals_from_json(
    profile: &Value,
    metric: Option<&Value>,
    rec: Option<&Value>,
) -> Fundamentals {
    let m = metric.map_or(&Value::Null, |m| &m["metric"]);
    // Finnhub da márgenes, rentabilidades y crecimientos en porcentaje; el dominio usa fracción.
    let pct = |k: &str| num(&m[k]).map(|x| x / 100.0);
    let millions = |v: &Value| num(v).map(|x| x * 1.0e6);
    let mut f = Fundamentals {
        sector: text(&profile["finnhubIndustry"]),
        website: text(&profile["weburl"]),
        country: text(&profile["country"]),
        currency: text(&profile["currency"]),
        market_cap: millions(&profile["marketCapitalization"]),
        shares_outstanding: millions(&profile["shareOutstanding"]),
        pe_trailing: num(&m["peTTM"]).or_else(|| num(&m["peBasicExclExtraTTM"])),
        price_to_book: num(&m["pbQuarterly"]).or_else(|| num(&m["pbAnnual"])),
        price_to_sales: num(&m["psTTM"]),
        eps_trailing: num(&m["epsTTM"]),
        gross_margin: pct("grossMarginTTM"),
        operating_margin: pct("operatingMarginTTM"),
        profit_margin: pct("netProfitMarginTTM"),
        roe: pct("roeTTM"),
        roa: pct("roaTTM"),
        revenue_growth_yoy: pct("revenueGrowthTTMYoy"),
        earnings_growth_yoy: pct("epsGrowthTTMYoy"),
        current_ratio: num(&m["currentRatioQuarterly"]),
        dividend_yield: pct("dividendYieldIndicatedAnnual"),
        payout_ratio: pct("payoutRatioTTM"),
        beta: num(&m["beta"]),
        sources: vec!["finnhub".into()],
        as_of: Some(Utc::now()),
        ..Fundamentals::default()
    };
    if let Some(r) = rec.and_then(|r| r.get(0)) {
        let g = |k: &str| r[k].as_f64().unwrap_or(0.0);
        let n = g("strongBuy") + g("buy") + g("hold") + g("sell") + g("strongSell");
        if n > 0.0 {
            f.analyst_rating_mean = Some(
                (g("strongBuy")
                    + 2.0 * g("buy")
                    + 3.0 * g("hold")
                    + 4.0 * g("sell")
                    + 5.0 * g("strongSell"))
                    / n,
            );
            f.analyst_count = Some(n as u32);
            if let Some(p) = r["period"].as_str() {
                f.extra.insert("analystPeriod".into(), json!(p));
            }
        }
    }
    if let Some(h) = num(&m["52WeekHigh"]) {
        f.extra.insert("high52w".into(), json!(h));
    }
    if let Some(l) = num(&m["52WeekLow"]) {
        f.extra.insert("low52w".into(), json!(l));
    }
    if let Some(n) = text(&profile["name"]) {
        f.extra.insert("name".into(), json!(n));
    }
    f
}

pub fn parse_news(v: &Value, symbol: Option<&str>) -> Vec<NewsItem> {
    let now = Utc::now();
    let mut out: Vec<NewsItem> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let title = text(&n["headline"])?;
            let url = text(&n["url"]).filter(|u| u.starts_with("http"))?;
            let ts = n["datetime"]
                .as_i64()
                .and_then(from_secs)
                .filter(|t| *t <= now + Duration::days(1))?;
            let mut tickers: Vec<String> = text(&n["related"])
                .map(|r| {
                    r.split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if tickers.is_empty() {
                tickers.extend(symbol.map(str::to_string));
            }
            Some(NewsItem {
                id: item_id(&url),
                title,
                summary: text(&n["summary"]),
                url,
                publisher: text(&n["source"]),
                published_at: ts,
                image_url: text(&n["image"]).filter(|u| u.starts_with("http")),
                tickers,
                sentiment: None,
                provider: "finnhub".into(),
                language: Some("en".into()),
            })
        })
        .collect();
    out.sort_by_key(|n| std::cmp::Reverse(n.published_at));
    out
}

// ---------------------------------------------------------------------------------------------
// WebSocket de trades
// ---------------------------------------------------------------------------------------------

pub struct FinnhubFeed {
    key: Secret,
}

impl FinnhubFeed {
    pub fn new(key: Secret) -> Self {
        Self { key }
    }
}

impl WsFeed for FinnhubFeed {
    fn name(&self) -> &'static str {
        "finnhub"
    }

    fn url(&self) -> String {
        // Finnhub solo admite la clave como parámetro de la URL del WebSocket. Esta URL no se
        // registra en ningún sitio (ver `realtime::ws_worker`).
        format!("{WS_URL}?token={}", self.key.expose())
    }

    fn subscribe_msgs(&self, symbols: &[String], on: bool) -> Vec<String> {
        symbols
            .iter()
            .map(|s| {
                json!({ "type": if on { "subscribe" } else { "unsubscribe" }, "symbol": s })
                    .to_string()
            })
            .collect()
    }

    fn parse(&self, text: &str) -> Vec<RawTick> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        if v["type"].as_str() != Some("trade") {
            return Vec::new();
        }
        v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let price = num(&t["p"]).filter(|p| *p > 0.0)?;
                Some(RawTick {
                    symbol: t["s"].as_str()?.to_string(),
                    price,
                    open24h: None,
                    volume: num(&t["v"]),
                    ts: t["t"].as_i64().and_then(from_ms).unwrap_or_else(Utc::now),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Las muestras siguen la forma documentada en finnhub.io/docs/api (no hay fixture real:
    // la API exige clave).
    #[test]
    fn quote_zero_means_not_found() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let v = json!({"c":0,"d":null,"dp":null,"h":0,"l":0,"o":0,"pc":0,"t":0});
        assert!(matches!(
            quote_from_json(&v, &id),
            Err(ProviderError::NotFound(_))
        ));
        let v = json!({"c":261.74,"d":-1.1,"dp":-0.42,"h":263.31,"l":260.68,"o":261.07,"pc":262.84,"t":1760040000});
        let q = quote_from_json(&v, &id).unwrap();
        assert_eq!(q.price, 261.74);
        assert_eq!(q.timestamp.timestamp(), 1_760_040_000);
        assert_eq!(q.prev_close, Some(262.84));
    }

    #[test]
    fn fundamentals_units() {
        let profile = json!({"country":"US","currency":"USD","finnhubIndustry":"Technology","marketCapitalization":3000000.0,"name":"Apple Inc","shareOutstanding":15000.0,"weburl":"https://www.apple.com/"});
        let metric = json!({"metric":{"peTTM":30.5,"netProfitMarginTTM":24.0,"roeTTM":150.0,"dividendYieldIndicatedAnnual":0.5,"52WeekHigh":280.0}});
        let rec = json!([{"buy":20,"hold":10,"period":"2026-10-01","sell":1,"strongBuy":10,"strongSell":0}]);
        let f = fundamentals_from_json(&profile, Some(&metric), Some(&rec));
        assert_eq!(f.market_cap, Some(3.0e12));
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.shares_outstanding, Some(1.5e10));
        assert!((f.profit_margin.unwrap() - 0.24).abs() < 1e-12);
        assert!((f.roe.unwrap() - 1.5).abs() < 1e-12);
        assert!((f.dividend_yield.unwrap() - 0.005).abs() < 1e-12);
        assert_eq!(f.analyst_count, Some(41));
        let mean = (10.0 + 40.0 + 30.0 + 4.0) / 41.0;
        assert!((f.analyst_rating_mean.unwrap() - mean).abs() < 1e-12);
        // Sin métricas ni recomendaciones solo quedan los datos del perfil.
        let f = fundamentals_from_json(&profile, None, None);
        assert!(f.pe_trailing.is_none() && f.analyst_count.is_none());
    }

    #[test]
    fn news_parse_and_filter() {
        let v = json!([
            {"category":"company","datetime":1760040000,"headline":"Apple hace algo","id":1,"image":"https://i.com/a.png","related":"AAPL","source":"Reuters","summary":"Resumen","url":"https://r.com/a"},
            {"datetime":1760040000,"headline":"javascript","url":"javascript:alert(1)"},
            {"datetime":1760050000,"headline":"Más nueva","url":"https://r.com/b"}
        ]);
        let n = parse_news(&v, Some("AAPL"));
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].title, "Más nueva");
        assert_eq!(n[0].tickers, ["AAPL"]);
        assert_eq!(n[1].tickers, ["AAPL"]);
    }

    #[test]
    fn ws_trades_and_subscriptions() {
        let f = FinnhubFeed::new(Secret::new("k3y-secret").unwrap());
        let t = f.parse(r#"{"data":[{"p":261.9,"s":"AAPL","t":1760040000123,"v":100},{"p":0,"s":"X","t":1,"v":1}],"type":"trade"}"#);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].symbol, "AAPL");
        assert_eq!(t[0].ts.timestamp_millis(), 1_760_040_000_123);
        assert!(f.parse(r#"{"type":"ping"}"#).is_empty());
        let m = f.subscribe_msgs(&["AAPL".into()], true);
        assert_eq!(m[0], r#"{"symbol":"AAPL","type":"subscribe"}"#);
    }

    #[test]
    fn us_symbols_only() {
        let s = |c, x: &str| us_symbol(&AssetId::new(c, x));
        assert_eq!(s(AssetClass::Stock, "BRK-B").as_deref(), Some("BRK.B"));
        assert_eq!(s(AssetClass::Etf, "spy").as_deref(), Some("SPY"));
        assert!(s(AssetClass::Stock, "SAN.MC").is_none());
        assert!(s(AssetClass::Crypto, "BTC").is_none());
    }
}
