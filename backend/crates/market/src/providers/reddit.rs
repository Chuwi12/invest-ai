//! Reddit (API oficial, OAuth de solo aplicación). Ver `docs/notes/social.md`.
//!
//! Solo la API autenticada: el acceso sin autenticar (`.json`) devuelve 403 desde mayo de 2026 y
//! NO se usa. Credenciales: `REDDIT_CLIENT_ID`, `REDDIT_CLIENT_SECRET`, `REDDIT_USER_AGENT`.
//! Las publicaciones son OPINIÓN de usuarios anónimos: el texto se recorta y el autor se guarda
//! como seudónimo (hash).

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetClass, SocialPost, SocialSource};
use moka::future::Cache;
use reqwest::{Client, header};
use serde_json::Value;
use tokio::time::Instant;

use super::{PResult, ProviderError, from_secs, text};
use crate::{
    http::{check_status, parse_json, snippet},
    ratelimit::RateLimiter,
    secret::Secret,
    social::{
        Matcher, RedditConfig, SUBS_CRYPTO, SUBS_ES, SUBS_ETF, SUBS_GENERAL, SocialBatch, SocialProvider, SocialQuery, clip, hash_author,
        send_with_backoff,
    },
};

const MAX_TEXT_CHARS: usize = 500;
const MAX_TITLE_CHARS: usize = 300;
const PAGE_LIMIT: usize = 100;
const SUBS_PER_REQUEST: usize = 10;

pub struct Reddit {
    http: Client,
    cfg: RedditConfig,
    /// Reddit permite ~100 consultas/min por cliente: se usa la mitad.
    limiter: RateLimiter,
    token: tokio::sync::Mutex<Option<(Secret, Instant)>>,
    cache: Cache<String, Arc<SocialBatch>>,
    backoff: Duration,
}

impl Reddit {
    pub fn new(http: Client, cfg: RedditConfig) -> Self {
        Self {
            http,
            cfg,
            limiter: RateLimiter::new(0.8, 4),
            token: tokio::sync::Mutex::new(None),
            cache: Cache::builder().time_to_live(Duration::from_secs(10 * 60)).max_capacity(500).build(),
            backoff: Duration::from_secs(2),
        }
    }

    /// Espera base entre reintentos de un 429 sin `Retry-After` (tests).
    #[must_use]
    pub fn with_backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }

    /// Subreddits a consultar según la clase del activo.
    pub fn subreddits_for(&self, class: AssetClass) -> Vec<String> {
        if let Some(v) = &self.cfg.subreddits {
            return v.clone();
        }
        let sets: &[&[&str]] = if class == AssetClass::Crypto { &[SUBS_CRYPTO, SUBS_ES] } else { &[SUBS_GENERAL, SUBS_ETF, SUBS_ES] };
        sets.iter().flat_map(|s| s.iter()).map(|s| (*s).to_string()).collect()
    }

    async fn token(&self, force: bool) -> PResult<Secret> {
        let mut guard = self.token.lock().await;
        if !force && let Some((t, exp)) = guard.as_ref() && *exp > Instant::now() {
            return Ok(t.clone());
        }
        self.limiter.acquire(Duration::from_secs(30)).await.map_err(|_| ProviderError::RateLimited { retry_after: None })?;
        let url = format!("{}/api/v1/access_token", self.cfg.auth_base.trim_end_matches('/'));
        let body = send_with_backoff(
            || {
                self.http
                    .post(&url)
                    .header(header::USER_AGENT, &self.cfg.user_agent)
                    .basic_auth(self.cfg.client_id.expose(), Some(self.cfg.client_secret.expose()))
                    .form(&[("grant_type", "client_credentials")])
            },
            self.backoff,
            2,
        )
        .await?;
        if !body.status.is_success() {
            *guard = None;
            return Err(ProviderError::Upstream(format!("Reddit rechazó la autenticación (HTTP {})", body.status.as_u16())));
        }
        let v: Value = parse_json(&body.text)?;
        let tok = v["access_token"].as_str().and_then(|t| Secret::new(t.to_string())).ok_or_else(|| {
            // `error` es un código corto de Reddit (invalid_grant...), nunca la clave.
            ProviderError::Upstream(format!("Reddit no dio token: {}", snippet(v["error"].as_str().unwrap_or("respuesta sin access_token"), 40)))
        })?;
        let ttl = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60).max(30);
        *guard = Some((tok.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(tok)
    }

    async fn search(&self, subs: &str, q: &str, days: u32) -> PResult<(Vec<Value>, bool, u32)> {
        let t = match days {
            0..=1 => "day",
            2..=7 => "week",
            _ => "month",
        };
        let url = format!("{}/r/{}/search", self.cfg.api_base.trim_end_matches('/'), subs);
        let mut requests = 0;
        let mut force = false;
        loop {
            let token = self.token(force).await?;
            self.limiter.acquire(Duration::from_secs(30)).await.map_err(|_| ProviderError::RateLimited { retry_after: None })?;
            requests += 1;
            let body = send_with_backoff(
                || {
                    self.http
                        .get(&url)
                        .header(header::USER_AGENT, &self.cfg.user_agent)
                        .bearer_auth(token.expose())
                        .query(&[
                            ("q", q),
                            ("restrict_sr", "1"),
                            ("sort", "new"),
                            ("t", t),
                            ("type", "link"),
                            ("limit", "100"),
                            ("raw_json", "1"),
                        ])
                },
                self.backoff,
                2,
            )
            .await?;
            if body.status == reqwest::StatusCode::UNAUTHORIZED && !force {
                force = true; // token caducado o revocado: se renueva una vez
                continue;
            }
            check_status(&body)?;
            let v: Value = parse_json(&body.text)?;
            let children = v["data"]["children"].as_array().ok_or_else(|| ProviderError::upstream("Reddit sin `data.children`"))?.clone();
            let more = children.len() >= PAGE_LIMIT && v["data"]["after"].is_string();
            return Ok((children, more, requests));
        }
    }
}

#[async_trait]
impl SocialProvider for Reddit {
    fn source(&self) -> SocialSource {
        SocialSource::Reddit
    }

    async fn fetch(&self, q: &SocialQuery) -> PResult<SocialBatch> {
        let m = q.matcher();
        let Some(query) = m.reddit_query() else { return Ok(SocialBatch::default()) };
        let subs = self.subreddits_for(q.class);
        let key = format!("{}|{}|{}|{}", m.ticker, m.name.as_deref().unwrap_or(""), q.days, subs.join("+"));
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(SocialBatch { requests: 0, ..(*hit).clone() });
        }
        let since = Utc::now() - chrono::Duration::days(i64::from(q.days));
        let mut batch = SocialBatch::default();
        let mut seen = std::collections::HashSet::new();
        let mut last_err = None;
        let mut any_ok = false;
        for chunk in subs.chunks(SUBS_PER_REQUEST) {
            match self.search(&chunk.join("+"), &query, q.days).await {
                Ok((children, more, n)) => {
                    any_ok = true;
                    batch.requests += n;
                    batch.truncated |= more;
                    for c in &children {
                        if let Some(p) = parse_post(c, &m, since)
                            && seen.insert(p.id.clone())
                        {
                            batch.posts.push(p);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Reddit: falló un grupo de subreddits");
                    last_err = Some(e);
                }
            }
        }
        if !any_ok && let Some(e) = last_err {
            return Err(e);
        }
        self.cache.insert(key, Arc::new(batch.clone())).await;
        Ok(batch)
    }
}

/// Convierte un hijo del listado de Reddit (`kind: t3`). `None` si no es una publicación válida,
/// es muy antigua, está borrada, es NSFW o no menciona de verdad el activo.
pub fn parse_post(child: &Value, m: &Matcher, since: DateTime<Utc>) -> Option<SocialPost> {
    if child["kind"].as_str()? != "t3" {
        return None;
    }
    let d = &child["data"];
    let id = text(&d["id"])?;
    let created_at = from_secs(d["created_utc"].as_f64()? as i64)?;
    if created_at < since || created_at > Utc::now() + chrono::Duration::days(1) || d["over_18"].as_bool() == Some(true) {
        return None;
    }
    let title = text(&d["title"])?;
    let selftext = d["selftext"].as_str().unwrap_or("");
    if matches!(selftext.trim(), "[removed]" | "[deleted]") {
        return None;
    }
    if !m.matches(&format!("{title}\n{selftext}")) {
        return None;
    }
    let permalink = text(&d["permalink"]).filter(|p| p.starts_with('/'))?;
    let author = d["author"].as_str().filter(|a| !a.is_empty() && *a != "[deleted]").map(|a| hash_author(SocialSource::Reddit, a)).unwrap_or_default();
    Some(SocialPost {
        id: format!("reddit:{id}"),
        source: SocialSource::Reddit,
        community: text(&d["subreddit"]).unwrap_or_default(),
        author,
        title: Some(clip(&title, MAX_TITLE_CHARS)),
        text: clip(selftext, MAX_TEXT_CHARS),
        score: d["score"].as_i64().unwrap_or(0),
        comments: d["num_comments"].as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32,
        created_at,
        url: format!("https://www.reddit.com{permalink}"),
    })
}
