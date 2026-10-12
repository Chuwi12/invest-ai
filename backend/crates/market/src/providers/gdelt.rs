//! GDELT DOC 2.0. Sin clave. Noticias globales en tiempo casi real.
//! Límite oficial: una petición cada 5 s. Cuando se supera, GDELT responde 200 con un texto
//! plano («Please limit requests...») en vez de JSON: se trata como `RateLimited`.

use async_trait::async_trait;
use chrono::{NaiveDateTime, Utc};
use domain::{AssetClass, NewsItem};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, text};
use crate::{
    http::{send_ok, snippet},
    news::{brand, item_id},
};

const BASE: &str = "https://api.gdeltproject.org/api/v2/doc/doc";

pub struct Gdelt {
    http: Client,
    info: ProviderInfo,
}

impl Gdelt {
    pub fn new(http: Client) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "gdelt",
                requires_key: false,
                has_key: false,
                capabilities: vec![Capability::News],
                classes: AssetClass::ALL.to_vec(),
                // Una cada 5 s, sin ráfaga.
                rate_per_sec: 0.2,
                burst: 1,
            },
        }
    }
}

/// Consulta GDELT: marca/ticker entre comillas y filtro de idioma. Solo texto saneado.
pub fn build_query(q: &NewsQuery) -> String {
    let lang = " sourcelang:english";
    let Some(id) = &q.id else {
        return format!("(\"stock market\" OR \"financial markets\" OR \"Wall Street\"){lang}");
    };
    let name = q
        .name
        .as_deref()
        .map(brand)
        .filter(|b| b.chars().count() >= 3);
    let ticker = q
        .ticker()
        .filter(|t| t.len() >= 3 && t.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|t| t.to_ascii_uppercase());
    let mut terms: Vec<String> = Vec::new();
    if let Some(n) = &name {
        terms.push(format!("\"{n}\""));
    }
    if let Some(t) = &ticker {
        // Un ticker suelto es ambiguo (p. ej. «ARM»): se acompaña del contexto bursátil.
        terms.push(if id.class == AssetClass::Crypto {
            t.clone()
        } else {
            format!("\"{t} stock\"")
        });
    }
    if terms.is_empty() {
        terms.push("\"stock market\"".into());
    }
    format!("({}){lang}", terms.join(" OR "))
}

#[async_trait]
impl Provider for Gdelt {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("query", &build_query(q))
            .append_pair("mode", "artlist")
            .append_pair("format", "json")
            .append_pair("sort", "datedesc")
            .append_pair("timespan", "3d")
            .append_pair("maxrecords", &q.limit.clamp(5, 75).to_string());
        let text = send_ok(self.http.get(url)).await?;
        parse_artlist(&text)
    }
}

pub fn parse_artlist(text: &str) -> PResult<Vec<NewsItem>> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("Please limit requests") {
        return Err(ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(5)),
        });
    }
    if !trimmed.starts_with('{') {
        // Errores de consulta (palabra demasiado corta, etc.) llegan como texto plano.
        return Err(ProviderError::Upstream(format!(
            "GDELT: {}",
            snippet(trimmed, 100)
        )));
    }
    let v: Value = serde_json::from_str(trimmed)
        .map_err(|e| ProviderError::Upstream(format!("GDELT JSON no válido: {e}")))?;
    // Sin resultados GDELT devuelve `{}`.
    let now = Utc::now();
    let mut out = Vec::new();
    for a in v["articles"].as_array().into_iter().flatten() {
        let (Some(url), Some(title), Some(seen)) = (
            text_of(&a["url"]),
            text_of(&a["title"]),
            text_of(&a["seendate"]),
        ) else {
            continue;
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        let Ok(naive) = NaiveDateTime::parse_from_str(&seen, "%Y%m%dT%H%M%SZ") else {
            continue;
        };
        let published = naive.and_utc();
        if published > now + chrono::Duration::days(1) {
            continue;
        }
        out.push(NewsItem {
            id: item_id(&url),
            title,
            summary: None,
            url,
            publisher: text_of(&a["domain"]),
            published_at: published,
            image_url: text_of(&a["socialimage"]).filter(|u| u.starts_with("http")),
            tickers: Vec::new(),
            sentiment: None,
            provider: "gdelt".into(),
            language: text_of(&a["language"]).map(|l| language_code(&l)),
        });
    }
    Ok(out)
}

fn text_of(v: &Value) -> Option<String> {
    text(v)
}

fn language_code(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "english" => "en",
        "spanish" => "es",
        "french" => "fr",
        "german" => "de",
        "portuguese" => "pt",
        "italian" => "it",
        other => return other.to_string(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::AssetId;

    /// Fixture real: la respuesta de GDELT cuando se supera el límite de 1 petición / 5 s.
    const RATE_LIMITED: &str = include_str!("../../tests/fixtures/gdelt_artlist_apple.json");

    #[test]
    fn rate_limit_text_is_rate_limited_error() {
        assert!(matches!(
            parse_artlist(RATE_LIMITED),
            Err(ProviderError::RateLimited { .. })
        ));
    }

    #[test]
    fn parses_documented_artlist_format() {
        // Formato documentado de `mode=artlist&format=json` (el fixture real capturado es el
        // aviso de límite; los campos son los de la documentación oficial).
        let body = r#"{"articles":[
          {"url":"https://www.example.com/a","url_mobile":"","title":"Apple lanza algo","seendate":"20261011T120000Z","socialimage":"https://img.example.com/a.jpg","domain":"example.com","language":"English","sourcecountry":"United States"},
          {"url":"javascript:alert(1)","title":"malo","seendate":"20261011T120000Z"},
          {"url":"https://www.example.com/b","title":"sin fecha"}
        ]}"#;
        let items = parse_artlist(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].published_at.to_rfc3339(),
            "2026-10-11T12:00:00+00:00"
        );
        assert_eq!(items[0].publisher.as_deref(), Some("example.com"));
        assert_eq!(items[0].language.as_deref(), Some("en"));
        assert_eq!(parse_artlist("{}").unwrap().len(), 0);
    }

    #[test]
    fn plain_text_error_is_upstream_error() {
        assert!(matches!(
            parse_artlist("The specified phrase is too short."),
            Err(ProviderError::Upstream(_))
        ));
    }

    #[test]
    fn query_is_sanitized() {
        let q = NewsQuery {
            id: Some(AssetId::new(AssetClass::Stock, "AAPL")),
            name: Some("Apple Inc.".into()),
            limit: 10,
        };
        assert_eq!(
            build_query(&q),
            "(\"Apple\" OR \"AAPL stock\") sourcelang:english"
        );
        let evil = NewsQuery {
            id: Some(AssetId::new(AssetClass::Stock, "AAPL")),
            name: Some("x\") OR domain:evil.com (\"".into()),
            limit: 10,
        };
        let s = build_query(&evil);
        assert!(
            !s.contains("domain:evil") || !s.contains(':') || s.matches(':').count() == 1,
            "{s}"
        );
        assert_eq!(s.matches('"').count() % 2, 0, "{s}");
    }
}
