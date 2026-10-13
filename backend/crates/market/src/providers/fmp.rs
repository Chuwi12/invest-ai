//! Financial Modeling Prep (`FMP_API_KEY`), API «stable». Respaldo de cotización y de
//! fundamentales/ratios (perfil + ratios TTM + métricas TTM). Plan gratuito: 250 peticiones/día.
//! Los nombres de campo siguen la documentación de la API `stable`; sin clave no se ha podido
//! contrastar contra respuestas reales (ver notas).

use async_trait::async_trait;
use domain::{AssetClass, AssetId, Fundamentals, MarketState, Quote};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, PResult, Provider, ProviderError, ProviderInfo, from_secs, num, text, us_symbol,
};
use crate::{
    http::{check_status, parse_json, send},
    secret::Secret,
};

const BASE: &str = "https://financialmodelingprep.com/stable";

pub struct Fmp {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl Fmp {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "fmp",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::Quote, Capability::Fundamentals],
                classes: vec![AssetClass::Stock, AssetClass::Etf],
                rate_per_sec: 2.0,
                burst: 5,
            },
            key,
        }
    }

    async fn get(&self, path: &str, symbol: &str) -> PResult<Value> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .push(path);
        url.query_pairs_mut()
            .append_pair("symbol", symbol)
            .append_pair("apikey", key.expose());
        let body = send(self.http.get(url)).await?;
        check_status(&body)?;
        let v: Value = parse_json(&body.text)?;
        if let Some(e) = v["Error Message"].as_str() {
            let low = e.to_ascii_lowercase();
            return Err(if low.contains("limit") {
                ProviderError::RateLimited { retry_after: None }
            } else {
                ProviderError::Upstream("FMP: error de acceso".into())
            });
        }
        Ok(v)
    }
}

#[async_trait]
impl Provider for Fmp {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        quote_from_json(&self.get("quote", &sym).await?, id)
    }

    async fn fundamentals(&self, id: &AssetId) -> PResult<Option<Fundamentals>> {
        let sym = us_symbol(id).ok_or(ProviderError::Unsupported)?;
        let (profile, ratios, metrics) = futures::join!(
            self.get("profile", &sym),
            self.get("ratios-ttm", &sym),
            self.get("key-metrics-ttm", &sym)
        );
        let profile = profile?;
        let Some(p) = profile.get(0) else {
            return Ok(None);
        };
        Ok(Some(fundamentals_from_json(
            p,
            ratios.ok().as_ref().and_then(|r| r.get(0)),
            metrics.ok().as_ref().and_then(|r| r.get(0)),
        )))
    }
}

pub fn quote_from_json(v: &Value, id: &AssetId) -> PResult<Quote> {
    let q = v
        .get(0)
        .ok_or_else(|| ProviderError::NotFound("sin cotización".into()))?;
    let price = num(&q["price"])
        .filter(|p| *p > 0.0)
        .ok_or_else(|| ProviderError::NotFound("sin precio".into()))?;
    let ts = q["timestamp"]
        .as_i64()
        .and_then(from_secs)
        .ok_or_else(|| ProviderError::upstream("sin marca de tiempo"))?;
    Ok(Quote {
        id: id.clone(),
        price,
        change: num(&q["change"]),
        change_pct: num(&q["changePercentage"]),
        open: num(&q["open"]),
        high: num(&q["dayHigh"]),
        low: num(&q["dayLow"]),
        prev_close: num(&q["previousClose"]),
        volume: num(&q["volume"]),
        market_cap: num(&q["marketCap"]),
        currency: Some("USD".into()),
        market_state: MarketState::Unknown,
        timestamp: ts,
        source: "fmp".into(),
        delay_secs: None,
        source_url: None,
        timestamp_origin: Some(domain::TimestampOrigin::Provider),
    })
}

pub fn fundamentals_from_json(
    p: &Value,
    ratios: Option<&Value>,
    metrics: Option<&Value>,
) -> Fundamentals {
    let r = ratios.unwrap_or(&Value::Null);
    let m = metrics.unwrap_or(&Value::Null);
    Fundamentals {
        description: text(&p["description"]),
        sector: text(&p["sector"]),
        industry: text(&p["industry"]),
        website: text(&p["website"]),
        employees: text(&p["fullTimeEmployees"])
            .and_then(|e| e.parse().ok())
            .or_else(|| p["fullTimeEmployees"].as_u64()),
        country: text(&p["country"]),
        currency: text(&p["currency"]),
        market_cap: num(&p["marketCap"]),
        enterprise_value: num(&m["enterpriseValueTTM"]),
        pe_trailing: num(&r["priceToEarningsRatioTTM"]),
        peg: num(&r["priceToEarningsGrowthRatioTTM"]),
        price_to_book: num(&r["priceToBookRatioTTM"]),
        price_to_sales: num(&r["priceToSalesRatioTTM"]),
        ev_to_ebitda: num(&m["evToEBITDATTM"]),
        gross_margin: num(&r["grossProfitMarginTTM"]),
        operating_margin: num(&r["operatingProfitMarginTTM"]),
        profit_margin: num(&r["netProfitMarginTTM"]),
        roe: num(&m["returnOnEquityTTM"]),
        roa: num(&m["returnOnAssetsTTM"]),
        debt_to_equity: num(&r["debtToEquityRatioTTM"]),
        current_ratio: num(&r["currentRatioTTM"]).or_else(|| num(&m["currentRatioTTM"])),
        dividend_yield: num(&r["dividendYieldTTM"]),
        payout_ratio: num(&r["dividendPayoutRatioTTM"]),
        beta: num(&p["beta"]),
        sources: vec!["fmp".into()],
        // El perfil no trae fecha de cálculo; se deja sin `as_of` en lugar de inventarla.
        as_of: None,
        ..Fundamentals::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quote_requires_timestamp_and_price() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let v = json!([{"symbol":"AAPL","price":261.74,"changePercentage":-0.42,"change":-1.1,"volume":38000000,"dayLow":260.68,"dayHigh":263.31,"marketCap":3.9e12,"open":261.07,"previousClose":262.84,"timestamp":1760040000}]);
        let q = quote_from_json(&v, &id).unwrap();
        assert_eq!(q.price, 261.74);
        assert_eq!(q.timestamp.timestamp(), 1_760_040_000);
        assert!(quote_from_json(&json!([]), &id).is_err());
        assert!(quote_from_json(&json!([{"price":1.0}]), &id).is_err());
    }

    #[test]
    fn fundamentals_do_not_invent_as_of() {
        let f = fundamentals_from_json(
            &json!({"sector":"Technology","currency":"USD","marketCap":1.0e12,"beta":1.2,"fullTimeEmployees":"164000"}),
            Some(&json!({"priceToEarningsRatioTTM":30.0,"netProfitMarginTTM":0.25})),
            None,
        );
        assert_eq!(f.employees, Some(164_000));
        assert_eq!(f.currency.as_deref(), Some("USD"));
        assert_eq!(f.pe_trailing, Some(30.0));
        assert_eq!(f.profit_margin, Some(0.25));
        assert!(f.as_of.is_none());
    }
}
