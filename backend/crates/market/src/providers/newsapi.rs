//! NewsAPI.org (`NEWSAPI_API_KEY`). Clave en la cabecera `X-Api-Key`.
//! Aviso: el plan gratuito retrasa los artículos 24 h y limita a 100 peticiones/día.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetClass, NewsItem};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, text};
use crate::{
    http::{check_status, parse_json, send},
    news::{brand, item_id},
    secret::Secret,
};

const BASE: &str = "https://newsapi.org/v2";

pub struct NewsApi {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

impl NewsApi {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "newsapi",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::News],
                classes: AssetClass::ALL.to_vec(),
                rate_per_sec: 0.5,
                burst: 3,
            },
            key,
        }
    }
}

#[async_trait]
impl Provider for NewsApi {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let size = q.limit.clamp(5, 50).to_string();
        let mut url;
        match &q.id {
            Some(_) => {
                let name = q.name.as_deref().map(brand).filter(|b| !b.is_empty());
                let term = name
                    .or_else(|| q.ticker())
                    .ok_or(ProviderError::Unsupported)?;
                url = Url::parse(&format!("{BASE}/everything"))
                    .map_err(|e| ProviderError::upstream(e.to_string()))?;
                url.query_pairs_mut()
                    .append_pair("q", &format!("\"{term}\""))
                    .append_pair("sortBy", "publishedAt")
                    .append_pair("language", "en")
                    .append_pair("pageSize", &size);
            }
            None => {
                url = Url::parse(&format!("{BASE}/top-headlines"))
                    .map_err(|e| ProviderError::upstream(e.to_string()))?;
                url.query_pairs_mut()
                    .append_pair("category", "business")
                    .append_pair("language", "en")
                    .append_pair("pageSize", &size);
            }
        }
        let body = send(self.http.get(url).header("X-Api-Key", key.expose())).await?;
        check_status(&body)?;
        parse_articles(&body.text)
    }
}

pub fn parse_articles(text_body: &str) -> PResult<Vec<NewsItem>> {
    let v: Value = parse_json(text_body)?;
    if v["status"].as_str() == Some("error") {
        return Err(if v["code"].as_str() == Some("rateLimited") {
            ProviderError::RateLimited { retry_after: None }
        } else {
            ProviderError::Upstream(format!(
                "NewsAPI: {}",
                v["code"].as_str().unwrap_or("error")
            ))
        });
    }
    let now = Utc::now();
    Ok(v["articles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let title = text(&a["title"]).filter(|t| t != "[Removed]")?;
            let url = text(&a["url"]).filter(|u| u.starts_with("http"))?;
            let ts = DateTime::parse_from_rfc3339(a["publishedAt"].as_str()?)
                .ok()?
                .with_timezone(&Utc);
            (ts <= now + chrono::Duration::days(1)).then(|| NewsItem {
                id: item_id(&url),
                title,
                summary: text(&a["description"]),
                url,
                publisher: text(&a["source"]["name"]),
                published_at: ts,
                image_url: text(&a["urlToImage"]).filter(|u| u.starts_with("http")),
                tickers: Vec::new(),
                sentiment: None,
                provider: "newsapi".into(),
                language: Some("en".into()),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_articles_and_errors() {
        let ok = r#"{"status":"ok","totalResults":2,"articles":[
          {"source":{"id":null,"name":"Reuters"},"title":"Apple sube","description":"d","url":"https://r.com/a","urlToImage":null,"publishedAt":"2026-10-09T12:00:00Z"},
          {"source":{"name":"X"},"title":"[Removed]","url":"https://removed.com","publishedAt":"2026-10-09T12:00:00Z"}]}"#;
        let n = parse_articles(ok).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].publisher.as_deref(), Some("Reuters"));
        assert!(matches!(
            parse_articles(r#"{"status":"error","code":"rateLimited"}"#),
            Err(ProviderError::RateLimited { .. })
        ));
        assert!(matches!(
            parse_articles(r#"{"status":"error","code":"apiKeyInvalid"}"#),
            Err(ProviderError::Upstream(_))
        ));
    }
}
