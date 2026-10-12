//! Google News RSS. Sin clave. Titulares por consulta (`q=`), en inglés y/o español.
//! La fecha es la `pubDate` que publica Google; el enlace es una redirección de Google News al
//! artículo original (estable y verificable).

use async_trait::async_trait;
use chrono::Utc;
use domain::{AssetClass, AssetId, NewsItem};
use reqwest::Client;
use url::Url;

use super::{Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo};
use crate::{
    http::{send_ok, snippet},
    news::{brand, item_id},
};

const BASE: &str = "https://news.google.com/rss/search";

pub struct GoogleNews {
    http: Client,
    info: ProviderInfo,
    /// Códigos de idioma: `en`, `es`. Una petición por idioma.
    languages: Vec<String>,
}

impl GoogleNews {
    pub fn new(http: Client, languages: Vec<String>) -> Self {
        let mut languages: Vec<String> =
            languages.into_iter().map(|l| l.to_ascii_lowercase()).filter(|l| edition(l).is_some()).collect();
        if languages.is_empty() {
            languages.push("en".into());
        }
        languages.dedup();
        Self {
            http,
            languages,
            info: ProviderInfo {
                name: "google_news",
                requires_key: false,
                has_key: false,
                capabilities: vec![Capability::News],
                classes: AssetClass::ALL.to_vec(),
                rate_per_sec: 1.0,
                burst: 3,
            },
        }
    }
}

/// (hl, gl, ceid) de cada edición soportada.
fn edition(lang: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match lang {
        "en" => Some(("en-US", "US", "US:en")),
        "es" => Some(("es", "ES", "ES:es")),
        _ => None,
    }
}

/// Consulta de texto para el activo. Solo contiene nombre comercial limpio y ticker (sin
/// caracteres de operador), nunca texto libre del usuario.
pub fn build_query(q: &NewsQuery, lang: &str) -> String {
    let Some(id) = &q.id else {
        return if lang == "es" { "mercados financieros bolsa" } else { "stock market" }.to_string();
    };
    let ticker = q.ticker().unwrap_or_default();
    let name = q.name.as_deref().map(brand).filter(|b| !b.is_empty());
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = &name {
        parts.push(n.clone());
    }
    if !ticker.is_empty()
        && ticker.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && (name.is_none() || id.class != AssetClass::Stock || ticker.len() >= 3 || name.is_some())
    {
        let t = ticker.to_ascii_uppercase();
        if name.as_deref().is_none_or(|n| !n.eq_ignore_ascii_case(&t)) {
            parts.push(t);
        }
    }
    if name.is_none() {
        parts.push(
            match id.class {
                AssetClass::Crypto => "crypto",
                AssetClass::Forex => "forex",
                _ => "stock",
            }
            .to_string(),
        );
    }
    parts.push("when:7d".into());
    parts.join(" ")
}

#[async_trait]
impl Provider for GoogleNews {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let futs = self.languages.iter().map(|lang| async move {
            let (hl, gl, ceid) = edition(lang).ok_or(ProviderError::Unsupported)?;
            let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
            url.query_pairs_mut()
                .append_pair("q", &build_query(q, lang))
                .append_pair("hl", hl)
                .append_pair("gl", gl)
                .append_pair("ceid", ceid);
            let text = send_ok(self.http.get(url)).await?;
            parse_rss(&text, lang)
        });
        let results = futures::future::join_all(futs).await;
        let mut out = Vec::new();
        let mut last_err = None;
        for r in results {
            match r {
                Ok(v) => out.extend(v),
                Err(e) => last_err = Some(e),
            }
        }
        match (out.is_empty(), last_err) {
            (true, Some(e)) => Err(e),
            _ => Ok(out),
        }
    }
}

/// «Titular - Medio» → («Titular», Some(«Medio»)). Google añade siempre el medio al título.
pub fn split_publisher(title: &str) -> (String, Option<String>) {
    match title.rsplit_once(" - ") {
        Some((head, tail)) if !head.trim().is_empty() && !tail.trim().is_empty() && tail.len() <= 60 => {
            (head.trim().to_string(), Some(tail.trim().to_string()))
        }
        _ => (title.trim().to_string(), None),
    }
}

pub fn parse_rss(xml: &str, lang: &str) -> PResult<Vec<NewsItem>> {
    let feed = feed_rs::parser::parse(xml.as_bytes())
        .map_err(|e| ProviderError::Upstream(format!("RSS no válido: {e} ({})", snippet(xml, 40))))?;
    let now = Utc::now();
    let mut out = Vec::with_capacity(feed.entries.len());
    for e in feed.entries {
        let (Some(title), Some(link)) =
            (e.title.as_ref().map(|t| t.content.as_str()), e.links.first().map(|l| l.href.clone()))
        else {
            continue;
        };
        // Solo http(s): un feed manipulado no debe poder colar otros esquemas.
        if !(link.starts_with("https://") || link.starts_with("http://")) {
            continue;
        }
        let Some(published) = e.published.or(e.updated) else { continue };
        let (title, publisher) = split_publisher(title);
        if title.is_empty() || published > now + chrono::Duration::days(1) {
            continue;
        }
        out.push(NewsItem {
            id: item_id(&link),
            title,
            summary: None,
            url: link,
            publisher,
            published_at: published,
            image_url: None,
            tickers: Vec::new(),
            sentiment: None,
            provider: "google_news".into(),
            language: Some(lang.to_string()),
        });
    }
    Ok(out)
}

#[allow(dead_code)]
fn _id(_: &AssetId) {}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = include_str!("../../tests/fixtures/googlenews_apple_en.xml");
    const ES: &str = include_str!("../../tests/fixtures/googlenews_apple_es.xml");

    #[test]
    fn parses_english_feed() {
        let items = parse_rss(EN, "en").unwrap();
        assert!(items.len() >= 5);
        let first = &items[0];
        assert_eq!(first.title, "Why Apple (AAPL) Stock Is Trading Lower Today");
        assert_eq!(first.publisher.as_deref(), Some("Yahoo Finance"));
        assert_eq!(first.published_at.to_rfc3339(), "2026-10-09T23:15:10+00:00");
        assert!(first.url.starts_with("https://news.google.com/rss/articles/"));
        assert_eq!(first.provider, "google_news");
        assert_eq!(first.id.len(), 24);
    }

    #[test]
    fn parses_spanish_feed_and_unique_ids() {
        let items = parse_rss(ES, "es").unwrap();
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i.language.as_deref() == Some("es")));
        let ids: std::collections::HashSet<_> = items.iter().map(|i| &i.id).collect();
        assert_eq!(ids.len(), items.len());
    }

    #[test]
    fn rejects_html_error_page() {
        assert!(parse_rss("<html><body>429 Too Many Requests</body></html>", "en").is_err());
    }

    #[test]
    fn queries_are_clean() {
        let q = NewsQuery {
            id: Some(AssetId::new(AssetClass::Stock, "AAPL")),
            name: Some("Apple Inc.".into()),
            limit: 10,
        };
        assert_eq!(build_query(&q, "en"), "Apple AAPL when:7d");
        let evil = NewsQuery {
            id: Some(AssetId::new(AssetClass::Stock, "AAPL")),
            name: Some("Apple\" OR site:evil.com (".into()),
            limit: 10,
        };
        let qs = build_query(&evil, "en");
        assert!(!qs.contains('"') && !qs.contains('(') && !qs.contains("site:"), "{qs}");
        let btc = NewsQuery { id: Some(AssetId::new(AssetClass::Crypto, "BTC")), name: None, limit: 5 };
        assert_eq!(build_query(&btc, "en"), "BTC crypto when:7d");
        assert_eq!(build_query(&NewsQuery::default(), "en"), "stock market");
    }

    #[test]
    fn splits_publisher() {
        assert_eq!(split_publisher("A - B - Reuters"), ("A - B".into(), Some("Reuters".into())));
        assert_eq!(split_publisher("Sin medio"), ("Sin medio".into(), None));
    }
}
