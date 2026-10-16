//! Finnhub (`FINNHUB_API_KEY`). Cotización de acciones/ETF de EE. UU., perfil y métricas,
//! recomendaciones de analistas, noticias por empresa y WebSocket de trades en tiempo real.
//! La clave viaja en la cabecera `X-Finnhub-Token`, nunca en la URL de REST.

use std::{collections::HashSet, sync::Mutex};

use async_trait::async_trait;
use chrono::{Duration, NaiveDate, Utc};
use domain::{AssetClass, AssetId, Fundamentals, IpoEntry, IpoStatus, MarketState, NewsItem, Quote};
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
    base: String,
    /// Recursos (`ruta|símbolo`) que el plan gratuito deniega con 403: no se vuelven a pedir.
    denied: Mutex<HashSet<String>>,
}

/// Tope de recursos denegados recordados (acota la memoria con símbolos del usuario).
const MAX_DENIED: usize = 512;

impl Finnhub {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self::with_base(http, key, BASE)
    }

    /// Igual que `new` con otra URL base (pruebas con un servidor local).
    pub fn with_base(http: Client, key: Option<Secret>, base: &str) -> Self {
        Self {
            http,
            base: base.to_string(),
            denied: Mutex::new(HashSet::new()),
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
        let tag = format!(
            "{path}|{}",
            query.iter().find(|(k, _)| *k == "symbol").map_or("", |(_, v)| v.as_str())
        );
        if self.denied.lock().unwrap_or_else(|p| p.into_inner()).contains(&tag) {
            return Err(ProviderError::Unsupported);
        }
        let mut url = Url::parse(&self.base).map_err(|e| ProviderError::upstream(e.to_string()))?;
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
        // 403 = el recurso no entra en el plan (candles, forex, símbolos no estadounidenses...):
        // se desactiva esa capacidad para ese recurso sin tratarlo como fallo del proveedor.
        // Una clave inválida da 401, que sí es un error.
        if body.status == reqwest::StatusCode::FORBIDDEN {
            let mut d = self.denied.lock().unwrap_or_else(|p| p.into_inner());
            if d.len() >= MAX_DENIED {
                d.clear();
            }
            d.insert(tag);
            return Err(ProviderError::Unsupported);
        }
        check_status(&body)?;
        let v: Value = parse_json(&body.text)?;
        if let Some(e) = v["error"].as_str() {
            return Err(ProviderError::Upstream(format!("Finnhub: {e}")));
        }
        Ok(v)
    }
}

/// Tope de filas que devuelve `calendar/ipo` por petición (medido: una ventana de un año
/// devuelve exactamente 200, las más recientes); si se alcanza, la ventana se parte en dos.
pub const IPO_PAGE_CAP: usize = 200;

impl Finnhub {
    /// Calendario de OPV entre dos fechas (incluidas). Disponible en el plan gratuito.
    pub async fn ipo_calendar(&self, from: NaiveDate, to: NaiveDate) -> PResult<Vec<IpoEntry>> {
        if self.key.is_none() {
            return Err(ProviderError::Unsupported);
        }
        let mut out = Vec::new();
        let mut pending = vec![(from, to, 0u8)];
        let now = Utc::now();
        while let Some((a, b, depth)) = pending.pop() {
            let v = self
                .get("calendar/ipo", &[("from", a.to_string()), ("to", b.to_string())])
                .await?;
            let rows = parse_ipo_calendar(&v, now);
            if rows.len() >= IPO_PAGE_CAP && a < b && depth < 8 {
                let mid = a + Duration::days((b - a).num_days() / 2);
                pending.push((a, mid, depth + 1));
                pending.push((mid + Duration::days(1), b, depth + 1));
                continue;
            }
            out.extend(rows);
        }
        out.sort_by(|x, y| y.date.cmp(&x.date).then_with(|| x.name.cmp(&y.name)));
        out.dedup_by(|x, y| x.date == y.date && x.name == y.name && x.status == y.status);
        Ok(out)
    }
}

/// `calendar/ipo`: `{"ipoCalendar":[{date,exchange,name,numberOfShares,price,status,symbol,totalSharesValue}]}`.
/// `price` es texto: «14.00» (definitivo), «17.00-19.00» (rango) o null.
pub fn parse_ipo_calendar(v: &Value, now: chrono::DateTime<Utc>) -> Vec<IpoEntry> {
    v["ipoCalendar"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let date = NaiveDate::parse_from_str(r["date"].as_str()?, "%Y-%m-%d").ok()?;
            let name = text(&r["name"])?;
            let status = match r["status"].as_str().unwrap_or("") {
                "expected" => IpoStatus::Expected,
                "priced" => IpoStatus::Priced,
                "filed" => IpoStatus::Filed,
                "withdrawn" => IpoStatus::Withdrawn,
                _ => IpoStatus::Unknown,
            };
            let (price_low, price_high) = match r["price"].as_str().map(str::trim) {
                Some(p) if p.contains('-') => {
                    let (a, b) = p.split_once('-').unwrap_or((p, p));
                    (a.trim().parse::<f64>().ok(), b.trim().parse::<f64>().ok())
                }
                Some(p) => {
                    let x = p.parse::<f64>().ok();
                    (x, x)
                }
                None => (num(&r["price"]), num(&r["price"])),
            };
            let symbol = text(&r["symbol"]);
            let spac_likely = is_spac_likely(&name, symbol.as_deref(), price_low, price_high);
            Some(IpoEntry {
                date,
                symbol,
                name,
                exchange: text(&r["exchange"]),
                status,
                price_low,
                price_high,
                shares: r["numberOfShares"].as_u64().filter(|n| *n > 0),
                total_value: num(&r["totalSharesValue"]).filter(|x| *x > 0.0),
                spac_likely,
                source: "finnhub".into(),
                source_url: None,
                as_of: now,
            })
        })
        .collect()
}

/// Indicio (no certeza) de sociedad vehículo (SPAC) a partir del nombre, el símbolo y el precio.
/// Las SPAC salen a 10,00 USD (valor del fideicomiso), así que el precio exacto es la condición
/// necesaria de las reglas nuevas; además debe darse una de: (a) símbolo de 5 letras acabado en
/// `U` (unidades), (b) palabra típica en el nombre, (c) nombre acabado en numeral de serie
/// («Capital Corp XIII», «Corp 4», «Capital Four»). Se mantiene la regla antigua («acquisition»
/// con precio 10 o símbolo de unidades). Heurística: puede dejar pasar alguna SPAC y, con precio
/// 10 y «Holdings» o «Special», marcar alguna empresa operativa. Una conversión de caja de ahorros
/// a 10 USD («Hoyne Bancorp, Inc.») no se marca.
pub fn is_spac_likely(name: &str, symbol: Option<&str>, low: Option<f64>, high: Option<f64>) -> bool {
    let lower = name.to_ascii_lowercase();
    let units = symbol.is_some_and(|s| s.len() == 5 && s.ends_with('U') && s.chars().all(|c| c.is_ascii_uppercase()));
    if lower.contains("acquisition") && (low == Some(10.0) || units) {
        return true;
    }
    if low != Some(10.0) || high != Some(10.0) {
        return false;
    }
    const WORDS: &[&str] = &[
        "acquisition", "merger", "spac", "blank check", "capital corp", "capital partners", "equity partners", "holdings",
        "investment corp", "opportunit", "liquidity", "growth corp", "special",
    ];
    units || WORDS.iter().any(|w| lower.contains(w)) || ends_in_series_number(&lower)
}

/// Nombre acabado en numeral romano (II..XV), cifra o número en letra, ignorando el sufijo societario.
fn ends_in_series_number(lower: &str) -> bool {
    let mut toks: Vec<&str> = lower.split_whitespace().map(|t| t.trim_matches(|c: char| c == ',' || c == '.')).filter(|t| !t.is_empty()).collect();
    if toks.last().is_some_and(|t| matches!(*t, "inc" | "corp" | "corporation" | "ltd" | "co" | "llc" | "limited")) {
        toks.pop();
    }
    toks.last().is_some_and(|t| {
        matches!(
            *t,
            "ii" | "iii" | "iv" | "v" | "vi" | "vii" | "viii" | "ix" | "x" | "xi" | "xii" | "xiii" | "xiv" | "xv"
                | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine" | "ten"
        ) || (t.len() <= 2 && t.chars().all(|c| c.is_ascii_digit()) && *t != "0" && *t != "1")
    })
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
        let today = Utc::now().date_naive();
        let window = [
            ("symbol", sym.clone()),
            ("from", today.to_string()),
            ("to", (today + Duration::days(120)).to_string()),
        ];
        let (profile, metric, rec, cal) = futures::join!(
            self.get("stock/profile2", &by_symbol),
            self.get("stock/metric", &with_metric),
            self.get("stock/recommendation", &by_symbol),
            self.get("calendar/earnings", &window),
        );
        // El perfil decide si el activo existe; métricas y recomendaciones son complementos.
        let profile = profile?;
        if profile.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(None);
        }
        let mut f = fundamentals_from_json(&profile, metric.ok().as_ref(), rec.ok().as_ref());
        f.next_earnings_date = cal.ok().and_then(|c| next_earnings(&c, today));
        Ok(Some(f))
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

/// Fecha del próximo resultado (la más cercana desde hoy) del calendario de Finnhub.
pub fn next_earnings(v: &Value, today: NaiveDate) -> Option<NaiveDate> {
    v["earningsCalendar"]
        .as_array()?
        .iter()
        .filter_map(|e| e["date"].as_str()?.parse::<NaiveDate>().ok())
        .filter(|d| *d >= today)
        .min()
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
        format!("{WS_URL}/?token={}", self.key.expose())
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

    // Respuestas REALES de la API de Finnhub (plan gratuito) capturadas el 14-oct-2026 con la
    // bolsa de EE. UU. cerrada; ver `tests/fixtures/finnhub_*.json`.
    fn fx(name: &str) -> Value {
        let p = format!("{}/tests/fixtures/finnhub_{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn stock(s: &str) -> AssetId {
        AssetId::new(AssetClass::Stock, s)
    }

    #[test]
    fn real_quote_units_and_timestamp() {
        let q = quote_from_json(&fx("quote_aapl"), &stock("AAPL")).unwrap();
        assert_eq!(q.price, 341.27);
        // `t` = cierre de la sesión del 13-oct (16:00 ET = 20:00 UTC), en segundos.
        assert_eq!(q.timestamp.to_rfc3339(), "2026-10-13T20:00:00+00:00");
        assert_eq!(q.prev_close, Some(340.97));
        assert_eq!(q.currency.as_deref(), Some("USD"));
        assert_eq!(q.timestamp_origin, Some(domain::TimestampOrigin::Provider));
        // `dp` ya viene en puntos porcentuales (0,3 / 340,97 = 0,088 %), igual que `Quote.change_pct`.
        let dp = q.change_pct.unwrap();
        assert!((dp - 0.088).abs() < 1e-9);
        let calc = (q.price / q.prev_close.unwrap() - 1.0) * 100.0;
        assert!((dp - calc).abs() < 0.001, "dp={dp} calc={calc}");
        assert!((q.change.unwrap() - 0.3).abs() < 1e-9);
        let m = quote_from_json(&fx("quote_msft"), &stock("MSFT")).unwrap();
        assert_eq!(m.price, 543.45);
        assert!((m.change_pct.unwrap() - (543.45 / 541.15 - 1.0) * 100.0).abs() < 0.001);
    }

    #[test]
    fn real_unknown_symbol_is_not_found() {
        assert!(matches!(
            quote_from_json(&fx("quote_unknown"), &stock("ZZZZNOPE")),
            Err(ProviderError::NotFound(_))
        ));
    }

    #[test]
    fn real_fundamentals_units() {
        let f = fundamentals_from_json(
            &fx("profile2_aapl"),
            Some(&fx("metric_aapl")),
            Some(&fx("recommendation_aapl")),
        );
        // Capitalización y acciones vienen en millones.
        assert!((f.market_cap.unwrap() / 1.0e12 - 4.98).abs() < 0.01);
        assert!((f.shares_outstanding.unwrap() / 1.0e9 - 14.687).abs() < 0.001);
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.sector.as_deref(), Some("Technology"));
        // Márgenes, rentabilidades y rendimiento por dividendo vienen en % -> fracción.
        assert!((f.gross_margin.unwrap() - 0.4865).abs() < 1e-9);
        assert!((f.profit_margin.unwrap() - 0.2762).abs() < 1e-9);
        assert!((f.dividend_yield.unwrap() - 0.0050534).abs() < 1e-9);
        assert!((f.payout_ratio.unwrap() - 0.1213).abs() < 1e-9);
        assert!((f.pe_trailing.unwrap() - 38.8303).abs() < 1e-9);
        assert!((f.eps_trailing.unwrap() - 8.7233).abs() < 1e-3);
        assert!((f.beta.unwrap() - 1.106).abs() < 1e-3);
        // Recomendaciones: el primer elemento es el periodo más reciente (sep-2026).
        assert_eq!(f.analyst_count, Some(12 + 22 + 15 + 3 + 1));
        let mean = (12.0 + 44.0 + 45.0 + 12.0 + 5.0) / 53.0;
        assert!((f.analyst_rating_mean.unwrap() - mean).abs() < 1e-12);
        assert_eq!(f.extra["analystPeriod"], "2026-09-01");
        assert_eq!(f.extra["high52w"], 345.34);
        // Sin métricas ni recomendaciones solo quedan los datos del perfil.
        let f = fundamentals_from_json(&fx("profile2_aapl"), None, None);
        assert!(f.pe_trailing.is_none() && f.analyst_count.is_none());
    }

    #[test]
    fn real_earnings_calendar() {
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(next_earnings(&fx("earnings_aapl"), d(2026, 10, 14)), Some(d(2026, 11, 2)));
        // Un resultado ya pasado no cuenta; sin calendario, tampoco.
        assert_eq!(next_earnings(&fx("earnings_aapl"), d(2026, 11, 3)), None);
        assert_eq!(next_earnings(&json!({"earningsCalendar": []}), d(2026, 10, 14)), None);
    }

    #[test]
    fn real_company_news() {
        let v = fx("company_news_aapl");
        let n = parse_news(&v, Some("AAPL"));
        assert_eq!(n.len(), v.as_array().unwrap().len());
        assert!(n.windows(2).all(|w| w[0].published_at >= w[1].published_at));
        assert!(n.iter().all(|i| i.provider == "finnhub" && i.tickers == ["AAPL"]));
        // `datetime` en segundos: 1791954600 = 2026-10-14T04:30:00Z.
        assert!(n.iter().any(|i| i.published_at.timestamp() == 1_791_954_600));
        // El enlace es la redirección de Finnhub al artículo original; el editor va en `publisher`.
        assert!(n.iter().all(|i| i.url.starts_with("https://finnhub.io/api/news?id=")));
        assert!(n.iter().any(|i| i.publisher.as_deref() == Some("Yahoo")));
        assert!(parse_news(&fx("news_general"), None).len() >= 5);
    }

    #[test]
    fn news_filters_unsafe_urls() {
        let v = json!([
            {"datetime":1760040000,"headline":"javascript","url":"javascript:alert(1)"},
            {"datetime":1760050000,"headline":"Más nueva","url":"https://r.com/b"}
        ]);
        let n = parse_news(&v, Some("AAPL"));
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].tickers, ["AAPL"]);
    }

    #[test]
    fn real_ws_trades_and_subscriptions() {
        let f = FinnhubFeed::new(Secret::new("k3y-secret").unwrap());
        let v = fx("ws_trades");
        let mut ticks = Vec::new();
        for m in v["messages"].as_array().unwrap() {
            ticks.extend(f.parse(&m.to_string()));
        }
        // El último mensaje del fixture es un `ping` real: no produce ticks.
        assert!(f.parse(r#"{"type":"ping"}"#).is_empty());
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|t| t.symbol == "BINANCE:BTCUSDT" && t.price > 1000.0));
        // `t` en milisegundos: 1791963757789 = 2026-10-14T07:42:37.789Z.
        assert_eq!(ticks[0].ts.timestamp_millis(), 1_791_963_757_789);
        assert_eq!(ticks[0].price, 83431.55);
        assert_eq!(ticks[0].volume, Some(0.01633));
        let m = f.subscribe_msgs(&["AAPL".into()], true);
        assert_eq!(m[0], r#"{"symbol":"AAPL","type":"subscribe"}"#);
        // La URL lleva la clave, el Debug del feed no.
        assert!(f.url().ends_with("/?token=k3y-secret"));
    }

    #[test]
    fn us_symbols_only() {
        let s = |c, x: &str| us_symbol(&AssetId::new(c, x));
        assert_eq!(s(AssetClass::Stock, "BRK-B").as_deref(), Some("BRK.B"));
        assert_eq!(s(AssetClass::Etf, "spy").as_deref(), Some("SPY"));
        assert!(s(AssetClass::Stock, "SAN.MC").is_none());
        assert!(s(AssetClass::Crypto, "BTC").is_none());
    }

    /// Servidor HTTP local mínimo: contesta con `respond(ruta)` y cuenta peticiones.
    async fn serve(
        respond: fn(&str) -> (u16, &'static str),
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n2 = n.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                let n = n2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let k = c.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..k]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (code, body) = respond(&path);
                    let _ = c
                        .write_all(format!("HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes())
                        .await;
                });
            }
        });
        (format!("http://{addr}"), n)
    }

    #[tokio::test]
    async fn forbidden_resource_disables_only_that_capability() {
        let (base, hits) = serve(|path| {
            if path.starts_with("/quote") {
                (200, include_str!("../../tests/fixtures/finnhub_quote_aapl.json"))
            } else if path.starts_with("/stock/recommendation") {
                (403, r#"{"error":"You don't have access to this resource."}"#)
            } else {
                (200, "{}")
            }
        })
        .await;
        let f = Finnhub::with_base(Client::new(), Some(Secret::new("k3y").unwrap()), &base);
        let id = stock("AAPL");
        assert_eq!(f.quote(&id).await.unwrap().price, 341.27);
        // 403 -> `Unsupported` (no cuenta como fallo del proveedor) y no se repite la petición.
        let rec = f.get("stock/recommendation", &[("symbol", "AAPL".into())]).await;
        assert!(matches!(rec, Err(ProviderError::Unsupported)));
        let before = hits.load(std::sync::atomic::Ordering::SeqCst);
        let rec = f.get("stock/recommendation", &[("symbol", "AAPL".into())]).await;
        assert!(matches!(rec, Err(ProviderError::Unsupported)));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);
        // El resto de capacidades siguen funcionando.
        assert_eq!(f.quote(&id).await.unwrap().price, 341.27);
    }

    #[tokio::test]
    async fn invalid_key_is_an_error_not_unsupported() {
        let (base, _) = serve(|_| (401, r#"{"error":"Invalid API key."}"#)).await;
        let f = Finnhub::with_base(Client::new(), Some(Secret::new("k3y").unwrap()), &base);
        assert!(matches!(f.quote(&stock("AAPL")).await, Err(ProviderError::Upstream(_))));
    }
}
