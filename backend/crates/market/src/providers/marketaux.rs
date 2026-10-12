//! Marketaux (`MARKETAUX_API_KEY`). Noticias con entidades y sentimiento por símbolo.
//! El plan gratuito devuelve pocos artículos por petición (3) y 100 peticiones/día.
//! La clave va en la consulta (`api_token`), única vía que ofrece la API.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetClass, AssetId, NewsItem};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, num,
    sentiment_from_score, text, us_symbol,
};
use crate::{
    http::{check_status, parse_json, send},
    news::item_id,
    secret::Secret,
};

const BASE: &str = "https://api.marketaux.com/v1/news/all";

pub struct Marketaux {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl Marketaux {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "marketaux",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::News],
                classes: vec![AssetClass::Stock, AssetClass::Crypto],
                rate_per_sec: 0.2,
                burst: 2,
            },
            key,
        }
    }
}

fn symbol_for(id: &AssetId) -> Option<String> {
    match id.class {
        AssetClass::Crypto => {
            let s = id.symbol.to_ascii_uppercase();
            let b = s.strip_suffix("-USD").unwrap_or(&s);
            (!b.is_empty() && b.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| format!("{b}USD"))
        }
        _ => us_symbol(id),
    }
}

#[async_trait]
impl Provider for Marketaux {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("api_token", key.expose())
                .append_pair("language", "en")
                .append_pair("limit", &q.limit.clamp(3, 50).to_string())
                .append_pair("filter_entities", "true");
            if let Some(id) = &q.id {
                qp.append_pair(
                    "symbols",
                    &symbol_for(id).ok_or(ProviderError::Unsupported)?,
                );
            }
        }
        let body = send(self.http.get(url)).await?;
        if body.status == reqwest::StatusCode::TOO_MANY_REQUESTS || body.status.as_u16() == 402 {
            return Err(ProviderError::RateLimited { retry_after: None });
        }
        check_status(&body)?;
        parse_news(&body.text, q.id.as_ref().and_then(symbol_for).as_deref())
    }
}

pub fn parse_news(body: &str, wanted: Option<&str>) -> PResult<Vec<NewsItem>> {
    let v: Value = parse_json(body)?;
    let data = v["data"]
        .as_array()
        .ok_or_else(|| ProviderError::upstream("Marketaux sin `data`"))?;
    let now = Utc::now();
    Ok(data
        .iter()
        .filter_map(|n| {
            let title = text(&n["title"])?;
            let url = text(&n["url"]).filter(|u| u.starts_with("http"))?;
            let ts = DateTime::parse_from_rfc3339(n["published_at"].as_str()?)
                .ok()?
                .with_timezone(&Utc);
            if ts > now + chrono::Duration::days(1) {
                return None;
            }
            let entities = n["entities"].as_array();
            let score = entities.and_then(|es| {
                es.iter()
                    .find(|e| {
                        wanted.is_none_or(|w| {
                            e["symbol"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(w))
                        })
                    })
                    .and_then(|e| num(&e["sentiment_score"]))
            });
            Some(NewsItem {
                id: item_id(&url),
                title,
                summary: text(&n["description"]).or_else(|| text(&n["snippet"])),
                url,
                publisher: text(&n["source"]),
                published_at: ts,
                image_url: text(&n["image_url"]).filter(|u| u.starts_with("http")),
                tickers: entities
                    .into_iter()
                    .flatten()
                    .filter_map(|e| text(&e["symbol"]))
                    .collect(),
                sentiment: score.map(|s| sentiment_from_score(s as f32, "provider:marketaux")),
                provider: "marketaux".into(),
                language: text(&n["language"]),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entities_sentiment() {
        let b = r#"{"meta":{"found":1},"data":[{"uuid":"1","title":"Tesla cae","description":"d","url":"https://x.com/a","image_url":"https://x.com/i.png","language":"en","published_at":"2026-10-09T12:00:00.000000Z","source":"x.com","entities":[{"symbol":"TSLA","sentiment_score":-0.6},{"symbol":"F","sentiment_score":0.9}]}]}"#;
        let n = parse_news(b, Some("TSLA")).unwrap();
        assert_eq!(n[0].tickers, ["TSLA", "F"]);
        let s = n[0].sentiment.as_ref().unwrap();
        assert_eq!(s.label, domain::SentimentLabel::VeryNegative);
        assert_eq!(s.method, "provider:marketaux");
        assert!(parse_news("{}", None).is_err());
    }

    #[test]
    fn symbol_mapping() {
        assert_eq!(
            symbol_for(&AssetId::new(AssetClass::Crypto, "btc")).as_deref(),
            Some("BTCUSD")
        );
        assert_eq!(
            symbol_for(&AssetId::new(AssetClass::Stock, "AAPL")).as_deref(),
            Some("AAPL")
        );
    }
}
