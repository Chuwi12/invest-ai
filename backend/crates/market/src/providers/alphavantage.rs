//! Alpha Vantage (`ALPHAVANTAGE_API_KEY`). Respaldo de cotización, histórico diario,
//! fundamentales (OVERVIEW) y noticias con sentimiento (NEWS_SENTIMENT).
//! Plan gratuito: 5 peticiones/min y ~25/día. Los avisos de límite llegan con HTTP 200 en las
//! claves `Note`/`Information`. La clave va en la consulta (la API no admite otra vía) y no se
//! registra: los errores pasan por `http::map_reqwest`, que elimina la URL.

use async_trait::async_trait;
use chrono::{NaiveDateTime, Utc};
use domain::{
    AssetClass, AssetId, Candle, Fundamentals, Interval, MarketState, NewsItem, Quote, Range,
};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, from_ymd, num,
    sentiment_from_score, text, us_symbol,
};
use crate::{
    http::{check_status, parse_json, send},
    news::item_id,
    secret::Secret,
};

const BASE: &str = "https://www.alphavantage.co/query";

pub struct AlphaVantage {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl AlphaVantage {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "alphavantage",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![
                    Capability::Quote,
                    Capability::History,
                    Capability::Fundamentals,
                    Capability::News,
                ],
                classes: vec![AssetClass::Stock, AssetClass::Etf],
                rate_per_sec: 0.08,
                burst: 1,
            },
            key,
        }
    }

    async fn get(&self, params: &[(&str, String)]) -> PResult<Value> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())))
            .append_pair("apikey", key.expose());
        let body = send(self.http.get(url)).await?;
        check_status(&body)?;
        let v: Value = parse_json(&body.text)?;
        check_envelope(&v)?;
        Ok(v)
    }
}

/// Avisos de límite/errores que Alpha Vantage devuelve con HTTP 200.
pub fn check_envelope(v: &Value) -> PResult<()> {
    if v["Note"].is_string() || v["Information"].is_string() {
        return Err(ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(60)),
        });
    }
    if v["Error Message"].is_string() {
        return Err(ProviderError::NotFound(
            "símbolo desconocido en Alpha Vantage".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl Provider for AlphaVantage {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let v = self
            .get(&[("function", "GLOBAL_QUOTE".into()), ("symbol", sym)])
            .await?;
        quote_from_json(&v, id)
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        interval: Interval,
    ) -> PResult<Vec<Candle>> {
        // Solo velas diarias (las intradía y el histórico completo exigen planes de pago).
        if interval != Interval::D1 {
            return Err(ProviderError::Unsupported);
        }
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let full = !matches!(range, Range::D1 | Range::D5 | Range::M1 | Range::M3);
        let v = self
            .get(&[
                ("function", "TIME_SERIES_DAILY".into()),
                ("symbol", sym),
                ("outputsize", if full { "full" } else { "compact" }.into()),
            ])
            .await?;
        let mut c = parse_daily(&v)?;
        let days = match range {
            Range::D1 => 3,
            Range::D5 => 8,
            Range::M1 => 32,
            Range::M3 => 93,
            Range::M6 => 184,
            Range::Ytd | Range::Y1 => 367,
            Range::Y2 => 732,
            Range::Y5 => 1828,
            Range::Y10 | Range::Max => 36_500,
        };
        let from = (Utc::now() - chrono::Duration::days(days)).timestamp();
        c.retain(|k| k.t >= from);
        Ok(c)
    }

    async fn fundamentals(&self, id: &AssetId) -> PResult<Option<Fundamentals>> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let v = self
            .get(&[("function", "OVERVIEW".into()), ("symbol", sym)])
            .await?;
        if v.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(None);
        }
        Ok(Some(parse_overview(&v)))
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let mut p = vec![
            ("function", "NEWS_SENTIMENT".to_string()),
            ("sort", "LATEST".into()),
            ("limit", q.limit.clamp(5, 50).to_string()),
        ];
        if let Some(id) = &q.id {
            p.push(("tickers", us_symbol(id).ok_or(ProviderError::Unsupported)?));
        } else {
            p.push(("topics", "financial_markets".into()));
        }
        parse_news(&self.get(&p).await?)
    }
}

pub fn quote_from_json(v: &Value, id: &AssetId) -> PResult<Quote> {
    let g = &v["Global Quote"];
    let price = num(&g["05. price"])
        .filter(|p| *p > 0.0)
        .ok_or_else(|| ProviderError::NotFound("sin cotización".into()))?;
    // Solo da el día de la última sesión (sin hora): se usa esa fecha a las 00:00 UTC.
    let ts = g["07. latest trading day"]
        .as_str()
        .and_then(from_ymd)
        .ok_or_else(|| ProviderError::upstream("sin fecha de sesión"))?;
    Ok(Quote {
        id: id.clone(),
        price,
        change: num(&g["09. change"]),
        change_pct: g["10. change percent"]
            .as_str()
            .and_then(|s| s.trim_end_matches('%').parse().ok()),
        open: num(&g["02. open"]),
        high: num(&g["03. high"]),
        low: num(&g["04. low"]),
        prev_close: num(&g["08. previous close"]),
        volume: num(&g["06. volume"]),
        market_cap: None,
        currency: Some("USD".into()),
        market_state: MarketState::Unknown,
        timestamp: ts,
        source: "alphavantage".into(),
        delay_secs: None,
        source_url: None,
        timestamp_origin: Some(domain::TimestampOrigin::Provider),
    })
}

pub fn parse_daily(v: &Value) -> PResult<Vec<Candle>> {
    let ts = v["Time Series (Daily)"]
        .as_object()
        .ok_or_else(|| ProviderError::NotFound("sin serie diaria".into()))?;
    let mut out: Vec<Candle> = ts
        .iter()
        .filter_map(|(d, r)| {
            Some(Candle {
                t: from_ymd(d)?.timestamp(),
                o: num(&r["1. open"])?,
                h: num(&r["2. high"])?,
                l: num(&r["3. low"])?,
                c: num(&r["4. close"])?,
                v: num(&r["5. volume"]),
            })
        })
        .collect();
    out.sort_by_key(|c| c.t);
    Ok(out)
}

/// Alpha Vantage devuelve `"None"` o `"-"` para datos ausentes: `num` ya los descarta.
pub fn parse_overview(v: &Value) -> Fundamentals {
    let mut f = Fundamentals {
        description: text(&v["Description"]),
        sector: text(&v["Sector"]),
        industry: text(&v["Industry"]),
        country: text(&v["Country"]),
        currency: text(&v["Currency"]),
        market_cap: num(&v["MarketCapitalization"]),
        enterprise_value: None,
        pe_trailing: num(&v["PERatio"]),
        pe_forward: num(&v["ForwardPE"]),
        peg: num(&v["PEGRatio"]),
        price_to_book: num(&v["PriceToBookRatio"]),
        price_to_sales: num(&v["PriceToSalesRatioTTM"]),
        ev_to_ebitda: num(&v["EVToEBITDA"]),
        eps_trailing: num(&v["EPS"]),
        revenue_ttm: num(&v["RevenueTTM"]),
        revenue_growth_yoy: num(&v["QuarterlyRevenueGrowthYOY"]),
        earnings_growth_yoy: num(&v["QuarterlyEarningsGrowthYOY"]),
        operating_margin: num(&v["OperatingMarginTTM"]),
        profit_margin: num(&v["ProfitMargin"]),
        roe: num(&v["ReturnOnEquityTTM"]),
        roa: num(&v["ReturnOnAssetsTTM"]),
        dividend_yield: num(&v["DividendYield"]),
        beta: num(&v["Beta"]),
        shares_outstanding: num(&v["SharesOutstanding"]),
        analyst_target_mean: num(&v["AnalystTargetPrice"]),
        sources: vec!["alphavantage".into()],
        as_of: v["LatestQuarter"].as_str().and_then(from_ymd),
        ..Fundamentals::default()
    };
    if let Some(x) = num(&v["52WeekHigh"]) {
        f.extra.insert("high52w".into(), x.into());
    }
    if let Some(x) = num(&v["52WeekLow"]) {
        f.extra.insert("low52w".into(), x.into());
    }
    f
}

pub fn parse_news(v: &Value) -> PResult<Vec<NewsItem>> {
    let feed = v["feed"]
        .as_array()
        .ok_or_else(|| ProviderError::NotFound("sin noticias".into()))?;
    let now = Utc::now();
    Ok(feed
        .iter()
        .filter_map(|n| {
            let title = text(&n["title"])?;
            let url = text(&n["url"]).filter(|u| u.starts_with("http"))?;
            let ts = NaiveDateTime::parse_from_str(n["time_published"].as_str()?, "%Y%m%dT%H%M%S")
                .ok()?
                .and_utc();
            if ts > now + chrono::Duration::days(1) {
                return None;
            }
            Some(NewsItem {
                id: item_id(&url),
                title,
                summary: text(&n["summary"]),
                url,
                publisher: text(&n["source"]),
                published_at: ts,
                image_url: text(&n["banner_image"]).filter(|u| u.starts_with("http")),
                tickers: n["ticker_sentiment"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| text(&t["ticker"]))
                    .collect(),
                sentiment: num(&n["overall_sentiment_score"])
                    .map(|s| sentiment_from_score(s as f32, "provider:alphavantage")),
                provider: "alphavantage".into(),
                language: Some("en".into()),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Formas documentadas en alphavantage.co/documentation (la API exige clave).
    #[test]
    fn limit_notice_is_rate_limited() {
        let v = json!({"Information":"We have detected your API key ... 25 requests per day"});
        assert!(matches!(
            check_envelope(&v),
            Err(ProviderError::RateLimited { .. })
        ));
        assert!(matches!(
            check_envelope(&json!({"Error Message":"Invalid API call"})),
            Err(ProviderError::NotFound(_))
        ));
    }

    #[test]
    fn global_quote() {
        let v = json!({"Global Quote":{"01. symbol":"AAPL","02. open":"261.07","03. high":"263.31","04. low":"260.68","05. price":"261.74","06. volume":"38000000","07. latest trading day":"2026-10-09","08. previous close":"262.84","09. change":"-1.1000","10. change percent":"-0.4185%"}});
        let q = quote_from_json(&v, &AssetId::new(AssetClass::Stock, "AAPL")).unwrap();
        assert_eq!(q.price, 261.74);
        assert!((q.change_pct.unwrap() + 0.4185).abs() < 1e-9);
        assert_eq!(q.timestamp.to_rfc3339(), "2026-10-09T00:00:00+00:00");
        assert!(quote_from_json(&json!({"Global Quote":{}}), &q.id).is_err());
    }

    #[test]
    fn daily_series_sorted() {
        let v = json!({"Time Series (Daily)":{
            "2026-10-09":{"1. open":"2","2. high":"3","3. low":"1","4. close":"2.5","5. volume":"100"},
            "2026-10-08":{"1. open":"1","2. high":"3","3. low":"1","4. close":"2","5. volume":"90"}}});
        let c = parse_daily(&v).unwrap();
        assert_eq!(c.len(), 2);
        assert!(c[0].t < c[1].t);
    }

    #[test]
    fn overview_ignores_none_strings() {
        let v = json!({"Symbol":"X","Description":"Una empresa","Sector":"TECHNOLOGY","Currency":"USD","MarketCapitalization":"1000000","PERatio":"None","Beta":"1.2","DividendYield":"0.005","LatestQuarter":"2026-06-30"});
        let f = parse_overview(&v);
        assert_eq!(f.market_cap, Some(1_000_000.0));
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.pe_trailing, None);
        assert_eq!(f.beta, Some(1.2));
        assert_eq!(f.as_of.unwrap().to_rfc3339(), "2026-06-30T00:00:00+00:00");
    }

    #[test]
    fn news_with_sentiment() {
        let v = json!({"feed":[{"title":"Apple sube","url":"https://x.com/a","time_published":"20261009T123000","summary":"s","source":"X","overall_sentiment_score":0.3,"ticker_sentiment":[{"ticker":"AAPL"}]},{"title":"sin url","time_published":"20261009T123000"}]});
        let n = parse_news(&v).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].published_at.to_rfc3339(), "2026-10-09T12:30:00+00:00");
        assert_eq!(n[0].tickers, ["AAPL"]);
        assert_eq!(
            n[0].sentiment.as_ref().unwrap().label,
            domain::SentimentLabel::Positive
        );
    }
}
