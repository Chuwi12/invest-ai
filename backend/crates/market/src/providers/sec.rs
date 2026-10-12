//! SEC EDGAR. Sin clave, pero la SEC exige un `User-Agent` con contacto y un máximo de 10
//! peticiones por segundo. Se exponen los últimos filings relevantes (10-K, 10-Q, 8-K...) como
//! noticias del proveedor `sec`. Solo empresas con ticker en EE. UU.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetClass, AssetId, NewsItem};
use moka::future::Cache;
use reqwest::{Client, header};
use serde_json::Value;

use super::{Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, from_ymd, text as json_text};
use crate::http::{parse_json, send_ok};

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";

/// Formularios que interesan a un inversor (se descarta el ruido de Form 4, 144, etc.).
const FORMS: &[&str] = &[
    "10-K", "10-K/A", "10-Q", "10-Q/A", "8-K", "8-K/A", "20-F", "40-F", "6-K", "S-1", "S-3",
    "424B5", "DEF 14A", "SC 13D", "SC 13G",
];

#[derive(Debug, Clone)]
pub struct Company {
    pub cik: u64,
    pub name: String,
}

pub struct Sec {
    http: Client,
    info: ProviderInfo,
    user_agent: String,
    tickers: Cache<(), Arc<HashMap<String, Company>>>,
}

impl Sec {
    /// `user_agent` debe llevar un contacto (`"invest-ai contacto@dominio.com"`).
    pub fn new(http: Client, user_agent: String) -> Self {
        Self {
            http,
            user_agent,
            tickers: Cache::builder().time_to_live(Duration::from_secs(86_400)).max_capacity(1).build(),
            info: ProviderInfo {
                name: "sec",
                requires_key: false,
                has_key: false,
                capabilities: vec![Capability::News],
                classes: vec![AssetClass::Stock],
                // El límite oficial es 10/s; se deja margen.
                rate_per_sec: 8.0,
                burst: 8,
            },
        }
    }

    async fn get(&self, url: String) -> PResult<String> {
        send_ok(self.http.get(url).header(header::USER_AGENT, &self.user_agent)).await
    }

    async fn company(&self, id: &AssetId) -> PResult<Company> {
        let map = self
            .tickers
            .try_get_with((), async {
                let text = self.get(TICKERS_URL.into()).await?;
                parse_tickers(&text).map(Arc::new)
            })
            .await
            .map_err(|e: Arc<ProviderError>| match &*e {
                ProviderError::RateLimited { retry_after } => ProviderError::RateLimited { retry_after: *retry_after },
                other => ProviderError::Upstream(other.to_string()),
            })?;
        map.get(&id.symbol.to_ascii_uppercase())
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(format!("{} no figura en la SEC", id.symbol)))
    }

    /// Últimos filings (más recientes primero).
    pub async fn filings(&self, id: &AssetId, limit: usize) -> PResult<Vec<NewsItem>> {
        if id.class != AssetClass::Stock {
            return Err(ProviderError::Unsupported);
        }
        let co = self.company(id).await?;
        let text = self.get(format!("https://data.sec.gov/submissions/CIK{:010}.json", co.cik)).await?;
        parse_submissions(&text, limit, Utc::now())
    }
}

#[async_trait]
impl Provider for Sec {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        match &q.id {
            Some(id) => self.filings(id, q.limit.clamp(1, 20)).await,
            None => Err(ProviderError::Unsupported),
        }
    }
}

pub fn parse_tickers(text: &str) -> PResult<HashMap<String, Company>> {
    let v: Value = parse_json(text)?;
    let obj = v.as_object().ok_or_else(|| ProviderError::upstream("company_tickers no es un objeto"))?;
    let mut out = HashMap::with_capacity(obj.len());
    for e in obj.values() {
        let (Some(cik), Some(t)) = (e["cik_str"].as_u64(), e["ticker"].as_str()) else { continue };
        out.entry(t.to_ascii_uppercase())
            .or_insert_with(|| Company { cik, name: e["title"].as_str().unwrap_or(t).to_string() });
    }
    Ok(out)
}

fn safe_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 200
        && !s.contains("..")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

pub fn parse_submissions(text: &str, limit: usize, now: DateTime<Utc>) -> PResult<Vec<NewsItem>> {
    let v: Value = parse_json(text)?;
    let name = v["name"].as_str().unwrap_or("").to_string();
    let cik: u64 = v["cik"]
        .as_str()
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| ProviderError::upstream("submissions sin cik"))?;
    let r = &v["filings"]["recent"];
    let col = |k: &str| r[k].as_array();
    let (Some(acc), Some(date), Some(form)) = (col("accessionNumber"), col("filingDate"), col("form"))
    else {
        return Err(ProviderError::upstream("submissions sin filings recientes"));
    };
    let mut out = Vec::new();
    for i in 0..acc.len() {
        let (Some(acc_no), Some(form), Some(date_s)) =
            (acc[i].as_str(), form.get(i).and_then(Value::as_str), date.get(i).and_then(Value::as_str))
        else {
            continue;
        };
        if !FORMS.contains(&form) {
            continue;
        }
        let get = |k: &str| col(k).and_then(|a| a.get(i)).and_then(json_text);
        let Some(doc) = get("primaryDocument").filter(|d| safe_path(d)) else { continue };
        let acc_clean = acc_no.replace('-', "");
        if !acc_clean.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let published = get("acceptanceDateTime")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc))
            .or_else(|| from_ymd(date_s));
        let Some(published) = published.filter(|p| *p <= now + chrono::Duration::days(1)) else { continue };
        let url = format!("https://www.sec.gov/Archives/edgar/data/{cik}/{acc_clean}/{doc}");
        let desc = get("primaryDocDescription").unwrap_or_else(|| form.to_string());
        let items = get("items");
        let summary = match items {
            Some(it) => format!("{desc} (apartados {it}). Presentado el {date_s}."),
            None => format!("{desc}. Presentado el {date_s}."),
        };
        out.push(NewsItem {
            id: crate::news::item_id(&url),
            title: format!("{name}: formulario {form} ante la SEC"),
            summary: Some(summary),
            url,
            publisher: Some("SEC EDGAR".into()),
            published_at: published,
            image_url: None,
            tickers: v["tickers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect(),
            sentiment: None,
            provider: "sec".into(),
            language: Some("en".into()),
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKERS: &str = include_str!("../../tests/fixtures/sec_company_tickers.json");
    const SUBS: &str = include_str!("../../tests/fixtures/sec_submissions_aapl.json");

    #[test]
    fn parses_ticker_map() {
        let m = parse_tickers(TICKERS).unwrap();
        let aapl = &m["AAPL"];
        assert_eq!(aapl.cik, 320_193);
        assert_eq!(aapl.name, "Apple Inc.");
        assert!(!m.contains_key("ZZZZZ"));
        assert!(parse_tickers("[]").is_err());
    }

    #[test]
    fn parses_filings_with_acceptance_time() {
        let now = DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z").unwrap().with_timezone(&Utc);
        let items = parse_submissions(SUBS, 10, now).unwrap();
        assert!(!items.is_empty() && items.len() <= 10);
        for n in &items {
            assert!(n.url.starts_with("https://www.sec.gov/Archives/edgar/data/320193/"), "{}", n.url);
            assert_eq!(n.provider, "sec");
            assert!(n.title.starts_with("Apple Inc."));
            assert!(n.tickers.contains(&"AAPL".to_string()));
        }
        // Más recientes primero (como en el JSON de la SEC).
        assert!(items.windows(2).all(|w| w[0].published_at >= w[1].published_at));
        // Se filtra el ruido: el fixture empieza por formularios 4.
        assert!(!items.iter().any(|n| n.title.contains("formulario 4 ")));
    }

    #[test]
    fn rejects_hostile_paths() {
        assert!(!safe_path("../x"));
        assert!(!safe_path("a b"));
        assert!(!safe_path("x?y=1"));
        assert!(safe_path("xslF345X06/form4.xml"));
    }

    #[test]
    fn missing_sections_are_errors() {
        assert!(parse_submissions("{}", 5, Utc::now()).is_err());
    }
}
