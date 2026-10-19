//! X (API oficial v2, búsqueda reciente). Pago por uso: sin nivel gratuito y ~0,005 USD por post
//! leído, así que CADA lectura pasa por un presupuesto mensual persistente (`ReadBudget`).
//! Prohibido el scraping. Credencial: `X_BEARER_TOKEN`. Ver `docs/notes/social.md`.
//!
//! Garantía de gasto: antes de cada petición se RESERVA `max_results` lecturas en el contador;
//! X nunca devuelve más de `max_results`, así que el consumo real no puede superar lo reservado, y
//! lo reservado nunca supera el tope mensual. Tras la respuesta se ajusta al número real de posts.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{SocialPost, SocialSource};
use moka::future::Cache;
use reqwest::{Client, header};
use serde_json::Value;

use super::{PResult, ProviderError, text};
use crate::{
    http::{check_status, parse_json},
    ratelimit::RateLimiter,
    social::{
        ReadBudget, SocialBatch, SocialProvider, SocialQuery, X_MAX_DAYS, X_MIN_RESULTS, XConfig, clip, hash_author, send_with_backoff,
    },
};

const MAX_TEXT_CHARS: usize = 500;
/// Longitud máxima (conservadora) de la consulta: la API admite 512 caracteres.
const MAX_QUERY_CHARS: usize = 420;

pub struct X {
    http: Client,
    cfg: XConfig,
    budget: Arc<dyn ReadBudget>,
    limiter: RateLimiter,
    cache: Cache<String, Arc<SocialBatch>>,
    backoff: Duration,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl X {
    pub fn new(http: Client, cfg: XConfig, budget: Arc<dyn ReadBudget>) -> Self {
        Self {
            http,
            cfg,
            budget,
            limiter: RateLimiter::new(0.2, 2),
            // Una consulta repetida no vuelve a gastar presupuesto durante 30 min.
            cache: Cache::builder().time_to_live(Duration::from_secs(30 * 60)).max_capacity(500).build(),
            backoff: Duration::from_secs(5),
            clock: Arc::new(Utc::now),
        }
    }

    #[must_use]
    pub fn with_backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }

    #[must_use]
    pub fn with_clock(mut self, c: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>) -> Self {
        self.clock = c;
        self
    }

    pub fn config(&self) -> &XConfig {
        &self.cfg
    }

    /// Consultas de búsqueda para un cashtag: una por grupo de cuentas (`from:`) y, si se pidió,
    /// una general. Siempre sin retuits.
    pub fn queries(&self, ticker: &str) -> Vec<String> {
        let tail = " -is:retweet".to_string();
        let lang = if self.cfg.langs.is_empty() {
            String::new()
        } else if self.cfg.langs.len() == 1 {
            format!(" lang:{}", self.cfg.langs[0])
        } else {
            format!(" ({})", self.cfg.langs.iter().map(|l| format!("lang:{l}")).collect::<Vec<_>>().join(" OR "))
        };
        let head = format!("${ticker}");
        let mut out = Vec::new();
        let mut group: Vec<String> = Vec::new();
        let fits = |g: &[String], extra: &str| head.len() + tail.len() + 8 + g.iter().map(|a| a.len() + 9).sum::<usize>() + extra.len() + 9 <= MAX_QUERY_CHARS;
        for a in &self.cfg.accounts {
            if !group.is_empty() && !fits(&group, a) {
                out.push(Self::account_query(&head, &group, &tail));
                group.clear();
            }
            group.push(a.clone());
        }
        if !group.is_empty() {
            out.push(Self::account_query(&head, &group, &tail));
        }
        if self.cfg.search_all {
            out.push(format!("{head}{tail}{lang}"));
        }
        out
    }

    fn account_query(head: &str, accounts: &[String], tail: &str) -> String {
        format!("{head} ({}){tail}", accounts.iter().map(|a| format!("from:{a}")).collect::<Vec<_>>().join(" OR "))
    }
}

#[async_trait]
impl SocialProvider for X {
    fn source(&self) -> SocialSource {
        SocialSource::X
    }

    async fn fetch(&self, q: &SocialQuery) -> PResult<SocialBatch> {
        let m = q.matcher();
        // En X siempre se busca por cashtag (`$AAPL`): no hay ambigüedad entre «F» y la palabra «f».
        if m.ticker.is_empty() || !m.ticker.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok(SocialBatch::default());
        }
        let days = q.days.clamp(1, X_MAX_DAYS);
        let queries = self.queries(&m.ticker);
        let key = format!("{}|{days}|{}", m.ticker, queries.join("§"));
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(SocialBatch { requests: 0, reads: 0, ..(*hit).clone() });
        }
        let now = (self.clock)();
        let month = now.format("%Y-%m").to_string();
        let start = (now - chrono::Duration::days(i64::from(days))).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut batch = SocialBatch::default();
        let mut first_err = None;
        for query in &queries {
            let want = self.cfg.results_per_request;
            let granted = self.budget.reserve(&month, want, self.cfg.monthly_read_budget).await.map_err(ProviderError::Upstream)?;
            if granted < X_MIN_RESULTS {
                // La API no admite pedir menos de 10: con menos saldo se corta aquí.
                self.budget.settle(&month, granted, 0).await;
                if !batch.budget_exhausted {
                    tracing::warn!(
                        presupuesto = self.cfg.monthly_read_budget,
                        mes = %month,
                        "X: presupuesto mensual de lecturas agotado (o con menos de 10 restantes): no se hacen más consultas este mes"
                    );
                }
                batch.budget_exhausted = true;
                break;
            }
            if self.limiter.acquire(Duration::from_secs(30)).await.is_err() {
                self.budget.settle(&month, granted, 0).await;
                first_err.get_or_insert(ProviderError::RateLimited { retry_after: None });
                break;
            }
            let url = format!("{}/2/tweets/search/recent", self.cfg.api_base.trim_end_matches('/'));
            let max_results = granted.to_string();
            let sent = send_with_backoff(
                || {
                    self.http
                        .get(&url)
                        .header(header::AUTHORIZATION, format!("Bearer {}", self.cfg.bearer.expose()))
                        .query(&[
                            ("query", query.as_str()),
                            ("max_results", max_results.as_str()),
                            ("start_time", start.as_str()),
                            ("tweet.fields", "created_at,public_metrics,author_id,lang"),
                            ("expansions", "author_id"),
                            ("user.fields", "username"),
                        ])
                },
                self.backoff,
                2,
            )
            .await;
            batch.requests += 1;
            let body = match sent {
                Ok(b) => b,
                Err(e) => {
                    // Sin respuesta: no se leyó nada.
                    self.budget.settle(&month, granted, 0).await;
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            if let Err(e) = check_status(&body) {
                self.budget.settle(&month, granted, 0).await;
                first_err.get_or_insert(e);
                continue;
            }
            let parsed: PResult<Value> = parse_json(&body.text);
            let v = match parsed {
                Ok(v) => v,
                Err(e) => {
                    // Respuesta 200 ilegible: pudo facturarse. Se cuenta lo reservado (lado seguro).
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            let data_len = v["data"].as_array().map_or(0, Vec::len);
            let actual = u32::try_from(data_len).unwrap_or(u32::MAX).min(granted);
            self.budget.settle(&month, granted, actual).await;
            batch.reads += actual;
            batch.truncated |= v["meta"]["next_token"].is_string();
            batch.posts.extend(parse_posts(&v, &m.ticker));
        }
        let mut seen = std::collections::HashSet::new();
        batch.posts.retain(|p| seen.insert(p.id.clone()));
        if batch.posts.is_empty() && !batch.budget_exhausted && batch.reads == 0 && let Some(e) = first_err {
            return Err(e);
        }
        if let Some(e) = first_err {
            tracing::warn!(error = %e, "X: alguna consulta falló");
        }
        if !batch.budget_exhausted {
            self.cache.insert(key, Arc::new(batch.clone())).await;
        }
        Ok(batch)
    }
}

/// Posts de una respuesta de búsqueda (`data`, `includes.users`). Descarta los que no llevan el
/// cashtag (el operador puede ser difuso) y los de fecha ilegible.
pub fn parse_posts(v: &Value, ticker: &str) -> Vec<SocialPost> {
    let users: HashMap<&str, &str> = v["includes"]["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| Some((u["id"].as_str()?, u["username"].as_str()?)))
        .collect();
    let matcher = crate::social::Matcher { ticker: ticker.to_string(), ambiguous: true, name: None };
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let id = text(&p["id"])?;
            let body = text(&p["text"])?;
            if !matcher.has_cashtag(&body) {
                return None;
            }
            let created_at = DateTime::parse_from_rfc3339(p["created_at"].as_str()?).ok()?.with_timezone(&Utc);
            let author_id = p["author_id"].as_str().unwrap_or("");
            let url = match users.get(author_id) {
                Some(u) if u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => format!("https://x.com/{u}/status/{id}"),
                _ => format!("https://x.com/i/web/status/{id}"),
            };
            let pm = &p["public_metrics"];
            Some(SocialPost {
                id: format!("x:{id}"),
                source: SocialSource::X,
                community: "x".into(),
                author: if author_id.is_empty() { String::new() } else { hash_author(SocialSource::X, author_id) },
                title: None,
                text: clip(&body, MAX_TEXT_CHARS),
                score: pm["like_count"].as_i64().unwrap_or(0),
                comments: pm["reply_count"].as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32,
                created_at,
                url,
            })
        })
        .collect()
}
