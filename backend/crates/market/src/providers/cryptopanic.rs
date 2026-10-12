//! CryptoPanic (`CRYPTOPANIC_API_KEY`). Noticias cripto con votos de la comunidad.
//! La clave va en la consulta (`auth_token`). Se usa la API `developer/v2`; no se ha podido
//! contrastar contra una respuesta real sin clave (ver notas).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetClass, NewsItem};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, sentiment_from_score,
    text,
};
use crate::{
    http::{check_status, parse_json, send},
    news::item_id,
    secret::Secret,
};

const BASE: &str = "https://cryptopanic.com/api/developer/v2/posts/";

pub struct CryptoPanic {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl CryptoPanic {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "cryptopanic",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::News],
                classes: vec![AssetClass::Crypto],
                rate_per_sec: 0.5,
                burst: 2,
            },
            key,
        }
    }
}

#[async_trait]
impl Provider for CryptoPanic {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("auth_token", key.expose())
                .append_pair("public", "true")
                .append_pair("kind", "news");
            if let Some(id) = &q.id {
                if id.class != AssetClass::Crypto {
                    return Err(ProviderError::Unsupported);
                }
                let s = id.symbol.to_ascii_uppercase();
                let b = s.strip_suffix("-USD").unwrap_or(&s);
                if b.is_empty() || !b.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(ProviderError::Unsupported);
                }
                qp.append_pair("currencies", b);
            }
        }
        let body = send(self.http.get(url)).await?;
        check_status(&body)?;
        let mut items = parse_posts(&body.text)?;
        items.truncate(q.limit.clamp(1, 100));
        Ok(items)
    }
}

pub fn parse_posts(body: &str) -> PResult<Vec<NewsItem>> {
    let v: Value = parse_json(body)?;
    let results = v["results"]
        .as_array()
        .ok_or_else(|| ProviderError::upstream("CryptoPanic sin `results`"))?;
    let now = Utc::now();
    Ok(results
        .iter()
        .filter_map(|p| {
            let title = text(&p["title"])?;
            let ts = DateTime::parse_from_rfc3339(p["published_at"].as_str()?)
                .ok()?
                .with_timezone(&Utc);
            if ts > now + chrono::Duration::days(1) {
                return None;
            }
            // Si la API no da la URL original se enlaza la página del post en CryptoPanic.
            let url = text(&p["original_url"])
                .or_else(|| text(&p["url"]))
                .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
                .or_else(|| {
                    let id = p["id"].as_u64()?;
                    let slug = text(&p["slug"])
                        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))?;
                    Some(format!("https://cryptopanic.com/news/{id}/{slug}"))
                })?;
            let votes = &p["votes"];
            let (pos, neg) = (
                votes["positive"].as_f64().unwrap_or(0.0),
                votes["negative"].as_f64().unwrap_or(0.0),
            );
            let sentiment = (pos + neg >= 3.0).then(|| {
                sentiment_from_score(((pos - neg) / (pos + neg)) as f32, "provider:cryptopanic")
            });
            let tickers = p["instruments"]
                .as_array()
                .or_else(|| p["currencies"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|c| text(&c["code"]))
                .collect();
            Some(NewsItem {
                id: item_id(&url),
                title,
                summary: text(&p["description"]),
                url,
                publisher: text(&p["source"]["title"]).or_else(|| text(&p["source"]["domain"])),
                published_at: ts,
                image_url: None,
                tickers,
                sentiment,
                provider: "cryptopanic".into(),
                language: Some("en".into()),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posts_with_votes() {
        let b = r#"{"results":[
          {"id":42,"slug":"btc-sube","title":"BTC sube","published_at":"2026-10-09T12:00:00Z","source":{"title":"CoinDesk"},"votes":{"positive":8,"negative":2},"instruments":[{"code":"BTC"}]},
          {"id":43,"title":"sin fecha"}]}"#;
        let n = parse_posts(b).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].url, "https://cryptopanic.com/news/42/btc-sube");
        assert_eq!(n[0].tickers, ["BTC"]);
        assert_eq!(
            n[0].sentiment.as_ref().unwrap().label,
            domain::SentimentLabel::VeryPositive
        );
        assert!(parse_posts("{}").is_err());
    }

    #[test]
    fn few_votes_no_sentiment() {
        let b = r#"{"results":[{"id":1,"slug":"x","title":"t","published_at":"2026-10-09T12:00:00Z","votes":{"positive":1,"negative":0}}]}"#;
        assert!(parse_posts(b).unwrap()[0].sentiment.is_none());
    }
}
