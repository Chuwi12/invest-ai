//! CoinGecko (API pública; la clave «demo» es opcional y solo sube el límite de peticiones).
//! Búsqueda, metadatos, oferta circulante/máxima, máximo histórico, cotización y OHLC.
//! Las marcas de tiempo son las de CoinGecko (`last_updated_at`, `market_data.last_updated`).

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{
    AssetClass, AssetId, AssetSummary, Candle, Fundamentals, Interval, MarketState, Quote, Range,
};
use moka::future::Cache;
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, PResult, Provider, ProviderError, ProviderInfo, SearchHit, from_secs, num,
    strip_html,
};
use crate::{
    http::{check_status, parse_json, send},
    secret::Secret,
};

const BASE: &str = "https://api.coingecko.com";

/// Ids de CoinGecko de las monedas más habituales: evita una búsqueda por cada consulta.
const KNOWN: &[(&str, &str)] = &[
    ("BTC", "bitcoin"),
    ("ETH", "ethereum"),
    ("USDT", "tether"),
    ("BNB", "binancecoin"),
    ("SOL", "solana"),
    ("XRP", "ripple"),
    ("USDC", "usd-coin"),
    ("DOGE", "dogecoin"),
    ("ADA", "cardano"),
    ("TRX", "tron"),
    ("AVAX", "avalanche-2"),
    ("LINK", "chainlink"),
    ("DOT", "polkadot"),
    ("LTC", "litecoin"),
    ("BCH", "bitcoin-cash"),
    ("XLM", "stellar"),
    ("ATOM", "cosmos"),
    ("XMR", "monero"),
    ("UNI", "uniswap"),
    ("NEAR", "near"),
];

/// Id de CoinGecko de una moneda conocida (sin red).
pub fn known_coin_id(symbol: &str) -> Option<&'static str> {
    let up = symbol.trim().to_ascii_uppercase();
    let up = up.strip_suffix("-USD").unwrap_or(&up);
    KNOWN.iter().find(|(s, _)| *s == up).map(|(_, id)| *id)
}

pub struct CoinGecko {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
    base: String,
    /// símbolo (mayúsculas) → id de CoinGecko, resuelto por búsqueda.
    ids: Cache<String, Arc<str>>,
}

impl CoinGecko {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        let has_key = key.is_some();
        Self {
            http,
            key,
            base: BASE.into(),
            ids: Cache::builder().max_capacity(2_000).time_to_live(Duration::from_secs(86_400)).build(),
            info: ProviderInfo {
                name: "coingecko",
                requires_key: false,
                has_key,
                capabilities: vec![
                    Capability::Search,
                    Capability::Quote,
                    Capability::History,
                    Capability::Fundamentals,
                ],
                classes: vec![AssetClass::Crypto],
                // Sin clave el plan público ronda las 5–15 peticiones/min.
                rate_per_sec: if has_key { 0.4 } else { 0.15 },
                burst: 3,
            },
        }
    }

    fn url(&self, path: &[&str]) -> PResult<Url> {
        let mut u = Url::parse(&self.base).map_err(|e| ProviderError::upstream(e.to_string()))?;
        u.path_segments_mut()
            .map_err(|()| ProviderError::upstream("URL base no válida"))?
            .extend(["api", "v3"])
            .extend(path);
        Ok(u)
    }

    async fn get(&self, url: Url, query: &[(&str, String)]) -> PResult<String> {
        let mut req = self.http.get(url).query(query);
        if let Some(k) = &self.key {
            req = req.header("x-cg-demo-api-key", k.expose());
        }
        let body = send(req).await?;
        // CoinGecko usa 404 con `{"error":"coin not found"}`.
        check_status(&body)?;
        Ok(body.text)
    }

    /// Id de CoinGecko para un símbolo, buscando si no es de los conocidos.
    async fn coin_id(&self, id: &AssetId) -> PResult<Arc<str>> {
        if id.class != AssetClass::Crypto {
            return Err(ProviderError::Unsupported);
        }
        if let Some(k) = known_coin_id(&id.symbol) {
            return Ok(Arc::from(k));
        }
        let sym = id.symbol.trim().to_ascii_uppercase();
        let sym = sym.strip_suffix("-USD").unwrap_or(&sym).to_string();
        if let Some(hit) = self.ids.get(&sym).await {
            return Ok(hit);
        }
        let found = parse_search_ids(&self.search_raw(&sym).await?, &sym);
        match found {
            Some(coin) => {
                let arc: Arc<str> = Arc::from(coin);
                self.ids.insert(sym, arc.clone()).await;
                Ok(arc)
            }
            None => Err(ProviderError::NotFound(format!("{sym} no está en CoinGecko"))),
        }
    }

    async fn search_raw(&self, q: &str) -> PResult<String> {
        let url = self.url(&["search"])?;
        self.get(url, &[("query", q.to_string())]).await
    }
}

/// Id de la moneda cuyo símbolo coincide exactamente, con mejor ranking de capitalización.
pub fn parse_search_ids(text: &str, symbol: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    v["coins"]
        .as_array()?
        .iter()
        .filter(|c| c["symbol"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(symbol)))
        .min_by_key(|c| c["market_cap_rank"].as_u64().unwrap_or(u64::MAX))
        .and_then(|c| c["id"].as_str().map(str::to_string))
}

#[async_trait]
impl Provider for CoinGecko {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn search(&self, query: &str, limit: usize) -> PResult<Vec<SearchHit>> {
        let text = self.search_raw(query).await?;
        let mut hits = parse_search(&text)?;
        hits.truncate(limit.max(1));
        for h in &hits {
            // Aprovecha para recordar ids sin coste adicional.
            if let Some(coin) = parse_search_ids(&text, &h.summary.symbol) {
                self.ids.insert(h.summary.symbol.to_ascii_uppercase(), Arc::from(coin)).await;
            }
        }
        Ok(hits)
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        let coin = self.coin_id(id).await?;
        let url = self.url(&["simple", "price"])?;
        let text = self
            .get(
                url,
                &[
                    ("ids", coin.to_string()),
                    ("vs_currencies", "usd".into()),
                    ("include_market_cap", "true".into()),
                    ("include_24hr_vol", "true".into()),
                    ("include_24hr_change", "true".into()),
                    ("include_last_updated_at", "true".into()),
                ],
            )
            .await?;
        quote_from_simple(&text, &coin, id)
    }

    async fn history(&self, id: &AssetId, range: Range, _interval: Interval) -> PResult<Vec<Candle>> {
        let coin = self.coin_id(id).await?;
        // OHLC de CoinGecko: la granularidad la decide `days` (30 min / 4 h / 4 d), no el intervalo.
        let days = match range {
            Range::D1 => "1",
            Range::D5 => "7",
            Range::M1 => "30",
            Range::M3 => "90",
            Range::M6 => "180",
            Range::Ytd | Range::Y1 => "365",
            // Más de un año solo con plan de pago: se pide el máximo permitido.
            Range::Y2 | Range::Y5 | Range::Y10 | Range::Max => "365",
        };
        let url = self.url(&["coins", &coin, "ohlc"])?;
        let text = self.get(url, &[("vs_currency", "usd".into()), ("days", days.into())]).await?;
        parse_ohlc(&text)
    }

    async fn fundamentals(&self, id: &AssetId) -> PResult<Option<Fundamentals>> {
        let coin = self.coin_id(id).await?;
        let url = self.url(&["coins", &coin])?;
        let text = self
            .get(
                url,
                &[
                    ("localization", "false".into()),
                    ("tickers", "false".into()),
                    ("community_data", "false".into()),
                    ("developer_data", "false".into()),
                    ("sparkline", "false".into()),
                ],
            )
            .await?;
        parse_coin(&text).map(Some)
    }
}

// ---------------------------------------------------------------------------------------------
// Parseo
// ---------------------------------------------------------------------------------------------

pub fn parse_search(text: &str) -> PResult<Vec<SearchHit>> {
    let v: Value = parse_json(text)?;
    let coins = v["coins"].as_array().ok_or_else(|| ProviderError::upstream("búsqueda sin lista de monedas"))?;
    let mut out = Vec::new();
    for c in coins {
        let (Some(sym), Some(name)) = (c["symbol"].as_str(), c["name"].as_str()) else { continue };
        let sym = sym.to_ascii_uppercase();
        if crate::symbol::validate_symbol(&sym).is_err() {
            continue;
        }
        let rank = c["market_cap_rank"].as_f64();
        // Rank 1 → 1.0; rank 1000 → ~0.1. Sin ranking, casi nada líquido.
        let liquidity = rank.map_or(0.05, |r| (1.0 / (1.0 + (r.max(1.0)).ln())).clamp(0.05, 1.0));
        out.push(SearchHit {
            summary: AssetSummary {
                id: AssetId::new(AssetClass::Crypto, sym.clone()),
                symbol: sym,
                name: name.to_string(),
                class: AssetClass::Crypto,
                exchange: None,
                currency: Some("USD".into()),
                country: None,
                source: "coingecko".into(),
            },
            liquidity,
        });
    }
    Ok(out)
}

pub fn quote_from_simple(text: &str, coin: &str, id: &AssetId) -> PResult<Quote> {
    let v: Value = parse_json(text)?;
    let c = &v[coin];
    let price = num(&c["usd"]).filter(|p| *p > 0.0).ok_or_else(|| {
        ProviderError::NotFound(format!("CoinGecko sin precio para {coin}"))
    })?;
    let change_pct = num(&c["usd_24h_change"]);
    let ts = c["last_updated_at"].as_i64().and_then(from_secs).unwrap_or_else(Utc::now);
    Ok(Quote {
        id: id.clone(),
        price,
        change: change_pct.map(|p| price - price / (1.0 + p / 100.0)),
        change_pct,
        open: None,
        high: None,
        low: None,
        prev_close: change_pct.map(|p| price / (1.0 + p / 100.0)),
        volume: None,
        market_cap: num(&c["usd_market_cap"]),
        currency: Some("USD".into()),
        market_state: MarketState::Continuous,
        timestamp: ts,
        source: "coingecko".into(),
        delay_secs: None,
    })
}

pub fn parse_ohlc(text: &str) -> PResult<Vec<Candle>> {
    let v: Value = parse_json(text)?;
    let arr = v.as_array().ok_or_else(|| ProviderError::upstream("OHLC no es una lista"))?;
    let mut out: Vec<Candle> = Vec::with_capacity(arr.len());
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
        // El OHLC de CoinGecko no trae volumen.
        out.push(Candle { t: t / 1000, o, h, l, c, v: None });
    }
    Ok(out)
}

fn date_of(v: &Value) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|d| d.date_naive())
}

pub fn parse_coin(text: &str) -> PResult<Fundamentals> {
    let v: Value = parse_json(text)?;
    if v["id"].as_str().is_none() {
        return Err(ProviderError::NotFound("moneda desconocida".into()));
    }
    let m = &v["market_data"];
    let usd = |k: &str| num(&m[k]["usd"]);
    let mut f = Fundamentals {
        description: v["description"]["en"]
            .as_str()
            .map(strip_html)
            .map(|d| d.chars().take(1200).collect::<String>())
            .filter(|d| !d.is_empty()),
        website: v["links"]["homepage"]
            .as_array()
            .and_then(|a| a.iter().find_map(|h| h.as_str().filter(|s| !s.is_empty())))
            .map(str::to_string),
        country: v["country_origin"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        market_cap: usd("market_cap"),
        fully_diluted_valuation: usd("fully_diluted_valuation"),
        circulating_supply: num(&m["circulating_supply"]),
        total_supply: num(&m["total_supply"]),
        max_supply: num(&m["max_supply"]),
        all_time_high: usd("ath"),
        all_time_high_date: date_of(&m["ath_date"]["usd"]),
        sources: vec!["coingecko".into()],
        as_of: m["last_updated"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .or_else(|| v["last_updated"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc))),
        ..Fundamentals::default()
    };
    let mut put = |k: &str, val: Option<Value>| {
        if let Some(val) = val.filter(|x| !x.is_null()) {
            f.extra.insert(k.to_string(), val);
        }
    };
    put("marketCapRank", v["market_cap_rank"].as_u64().map(Into::into));
    put("hashingAlgorithm", v["hashing_algorithm"].as_str().map(Into::into));
    put("genesisDate", v["genesis_date"].as_str().map(Into::into));
    put("categories", v["categories"].as_array().map(|c| Value::Array(c.iter().take(8).cloned().collect())));
    put("athChangePct", num(&m["ath_change_percentage"]["usd"]).map(Into::into));
    put("priceChange7dPct", num(&m["price_change_percentage_7d"]).map(Into::into));
    put("priceChange30dPct", num(&m["price_change_percentage_30d"]).map(Into::into));
    put("priceChange1yPct", num(&m["price_change_percentage_1y"]).map(Into::into));
    put("totalVolume24h", usd("total_volume").map(Into::into));
    put("atl", usd("atl").map(Into::into));
    Ok(f)
}

/// Mapa símbolo→id para tests y precarga.
pub fn known_ids() -> HashMap<&'static str, &'static str> {
    KNOWN.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COIN: &str = include_str!("../../tests/fixtures/coingecko_coin_bitcoin.json");
    const SEARCH: &str = include_str!("../../tests/fixtures/coingecko_search_bitcoin.json");
    const SIMPLE: &str = include_str!("../../tests/fixtures/coingecko_simple_price.json");
    const OHLC: &str = include_str!("../../tests/fixtures/coingecko_ohlc_bitcoin.json");

    #[test]
    fn parses_search_with_liquidity_by_rank() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits[0].summary.id.to_string(), "crypto:BTC");
        assert_eq!(hits[0].summary.name, "Bitcoin");
        assert!(hits[0].liquidity > hits[1].liquidity);
        assert_eq!(parse_search_ids(SEARCH, "btc").as_deref(), Some("bitcoin"));
        assert_eq!(parse_search_ids(SEARCH, "BCH").as_deref(), Some("bitcoin-cash"));
        assert!(parse_search_ids(SEARCH, "NOPE").is_none());
    }

    #[test]
    fn parses_simple_price_with_provider_time() {
        let id = AssetId::new(AssetClass::Crypto, "BTC");
        let q = quote_from_simple(SIMPLE, "bitcoin", &id).unwrap();
        assert_eq!(q.price, 82967.0);
        assert_eq!(q.timestamp.timestamp(), 1_791_710_273);
        assert!((q.change_pct.unwrap() - 0.1788672378307378).abs() < 1e-9);
        assert!(q.market_cap.unwrap() > 1.0e12);
        // El cierre previo derivado es coherente con el precio y el cambio.
        let back = q.prev_close.unwrap() * (1.0 + q.change_pct.unwrap() / 100.0);
        assert!((back - q.price).abs() < 1e-6);
        assert!(quote_from_simple(SIMPLE, "dogecoin", &id).is_err());
    }

    #[test]
    fn parses_coin_metadata() {
        let f = parse_coin(COIN).unwrap();
        assert_eq!(f.max_supply, Some(21_000_000.0));
        assert_eq!(f.circulating_supply, Some(20_096_512.0));
        assert_eq!(f.all_time_high, Some(126_080.0));
        assert_eq!(f.all_time_high_date, NaiveDate::from_ymd_opt(2025, 10, 6));
        assert_eq!(f.website.as_deref(), Some("http://www.bitcoin.org"));
        assert!(f.description.as_deref().unwrap().starts_with("Bitcoin is"));
        assert_eq!(f.sources, ["coingecko"]);
        assert_eq!(f.as_of.unwrap().to_rfc3339(), "2026-10-11T09:16:49+00:00");
        assert_eq!(f.extra["marketCapRank"], 1);
    }

    #[test]
    fn parses_ohlc() {
        let c = parse_ohlc(OHLC).unwrap();
        assert!(c.len() > 20);
        assert_eq!(c[0].t, 1_789_128_000);
        assert!(c.iter().all(|k| k.h >= k.l && k.v.is_none()));
    }

    #[test]
    fn known_ids_resolve_without_network() {
        assert_eq!(known_coin_id("btc"), Some("bitcoin"));
        assert_eq!(known_coin_id("ETH-USD"), Some("ethereum"));
        assert_eq!(known_coin_id("ZZZZ"), None);
        assert!(known_ids().len() >= 20);
    }
}
