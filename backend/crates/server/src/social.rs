//! Redes sociales (Reddit y X) en el servidor: consulta con caché por activo, guardado con fecha
//! real, métricas deterministas y presupuesto mensual de X persistente. Diseño y riesgos en
//! `docs/notes/social.md`.
//!
//! Es OPINIÓN, no hechos, y no entra en la puntuación ni en los pesos. Todo está APAGADO por
//! defecto: sin credenciales `SocialService::from_config` devuelve `None`, no se cablea nada y no
//! se hace ninguna petición. Un fallo de redes nunca rompe un análisis ni el arranque.

use std::{collections::HashMap, sync::Arc, time::Duration};

use ai::social::{CoverageRun, compute_metrics, pick_excerpts};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetSummary, DomainError, SocialData, SocialDigest, SocialPost, SocialSource};
use market::{
    providers::{reddit::Reddit, x::X},
    social::{ReadBudget, SocialConfig, SocialProvider, SocialQuery},
};
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::db::{dt_from_ms, now_ms};

/// Tras un fallo no se vuelve a intentar el mismo activo y red durante este tiempo.
const FAILURE_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Las publicaciones se conservan más que los 30 días de la línea base + el día actual.
const POSTS_RETENTION_DAYS: i64 = 45;

// ---------------------------------------------------------------------------------------------
// Presupuesto de X en SQLite
// ---------------------------------------------------------------------------------------------

/// Contador mensual de posts leídos de X. La reserva es atómica (mutex + transacción): dos
/// consultas concurrentes no pueden pasarse del tope, y el contador sobrevive a los reinicios.
pub struct SqliteBudget {
    pool: SqlitePool,
    lock: tokio::sync::Mutex<()>,
}

impl SqliteBudget {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool, lock: tokio::sync::Mutex::new(()) }
    }
}

#[async_trait]
impl ReadBudget for SqliteBudget {
    async fn reserve(&self, month: &str, want: u32, cap: u32) -> Result<u32, String> {
        let _g = self.lock.lock().await;
        let e = |e: sqlx::Error| format!("presupuesto de X: {e}");
        let mut tx = self.pool.begin().await.map_err(e)?;
        sqlx::query("INSERT INTO social_x_budget (month, reads, updated_at) VALUES (?1, 0, ?2) ON CONFLICT(month) DO NOTHING")
            .bind(month)
            .bind(now_ms())
            .execute(&mut *tx)
            .await
            .map_err(e)?;
        let used: i64 = sqlx::query("SELECT reads FROM social_x_budget WHERE month = ?1").bind(month).fetch_one(&mut *tx).await.map_err(e)?.get(0);
        let used = u32::try_from(used.max(0)).unwrap_or(u32::MAX);
        let grant = want.min(cap.saturating_sub(used));
        if grant > 0 {
            sqlx::query("UPDATE social_x_budget SET reads = reads + ?2, updated_at = ?3 WHERE month = ?1")
                .bind(month)
                .bind(i64::from(grant))
                .bind(now_ms())
                .execute(&mut *tx)
                .await
                .map_err(e)?;
        }
        tx.commit().await.map_err(e)?;
        Ok(grant)
    }

    async fn settle(&self, month: &str, reserved: u32, actual: u32) {
        let _g = self.lock.lock().await;
        let refund = i64::from(reserved.saturating_sub(actual));
        if refund == 0 {
            return;
        }
        if let Err(e) = sqlx::query("UPDATE social_x_budget SET reads = MAX(0, reads - ?2), updated_at = ?3 WHERE month = ?1")
            .bind(month)
            .bind(refund)
            .bind(now_ms())
            .execute(&self.pool)
            .await
        {
            // Lado seguro: si no se devuelve, el contador queda más alto (gasta menos, no más).
            tracing::warn!(error = %e, "no se pudo ajustar el contador de lecturas de X");
        }
    }

    async fn used(&self, month: &str) -> u32 {
        sqlx::query("SELECT reads FROM social_x_budget WHERE month = ?1")
            .bind(month)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map_or(0, |r| u32::try_from(r.get::<i64, _>(0).max(0)).unwrap_or(u32::MAX))
    }
}

// ---------------------------------------------------------------------------------------------
// Servicio
// ---------------------------------------------------------------------------------------------

pub struct SocialService {
    pool: SqlitePool,
    cfg: SocialConfig,
    providers: Vec<Arc<dyn SocialProvider>>,
    budget: Arc<SqliteBudget>,
    /// Un refresco a la vez por activo.
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    cooldown: std::sync::Mutex<HashMap<(String, SocialSource), DateTime<Utc>>>,
}

impl SocialService {
    /// `None` si no hay credenciales válidas (lo normal): la app es idéntica a la que no conoce las redes.
    pub fn from_config(cfg: SocialConfig, pool: SqlitePool) -> Option<Arc<Self>> {
        for n in &cfg.notes {
            tracing::warn!("redes sociales: {n}");
        }
        if !cfg.enabled() {
            return None;
        }
        let http = match market::http::build_client(Duration::from_secs(15)) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "redes sociales: no se pudo crear el cliente HTTP; quedan desactivadas");
                return None;
            }
        };
        let budget = Arc::new(SqliteBudget::new(pool.clone()));
        let mut providers: Vec<Arc<dyn SocialProvider>> = Vec::new();
        if let Some(r) = &cfg.reddit {
            providers.push(Arc::new(Reddit::new(http.clone(), r.clone())));
        }
        if let Some(x) = &cfg.x {
            providers.push(Arc::new(X::new(http, x.clone(), budget.clone())));
            tracing::info!(
                presupuesto_mensual = x.monthly_read_budget,
                coste_max_usd = f64::from(x.monthly_read_budget) * x.cost_per_read_usd,
                "X activado: pago por uso con tope mensual de lecturas"
            );
        }
        tracing::info!(reddit = cfg.reddit.is_some(), x = cfg.x.is_some(), "redes sociales activadas (opinión, no entra en la puntuación)");
        Some(Arc::new(Self { pool, cfg, providers, budget, locks: Default::default(), cooldown: Default::default() }))
    }

    /// Con proveedores a medida (tests).
    pub fn with_providers(cfg: SocialConfig, pool: SqlitePool, providers: Vec<Arc<dyn SocialProvider>>, budget: Arc<SqliteBudget>) -> Arc<Self> {
        Arc::new(Self { pool, cfg, providers, budget, locks: Default::default(), cooldown: Default::default() })
    }

    pub fn config(&self) -> &SocialConfig {
        &self.cfg
    }

    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap_or_else(|p| p.into_inner()).entry(key.to_string()).or_default().clone()
    }

    /// Consulta las redes si la última consulta correcta de ese activo y red es más antigua que
    /// `refresh_minutes`. Nunca falla: devuelve el último error (si lo hubo) como texto.
    pub async fn refresh_if_stale(&self, asset: &AssetSummary) -> Option<String> {
        let key = asset.id.to_string();
        let lock = self.lock_for(&key);
        let _g = lock.lock().await;
        let mut last_err = None;
        for p in &self.providers {
            let source = p.source();
            let now = Utc::now();
            let fresh_after = now - chrono::Duration::minutes(i64::try_from(self.cfg.refresh_minutes).unwrap_or(360));
            let last_ok: Option<i64> = sqlx::query("SELECT MAX(fetched_at) FROM social_runs WHERE asset_id = ?1 AND source = ?2 AND ok = 1")
                .bind(&key)
                .bind(source.as_str())
                .fetch_one(&self.pool)
                .await
                .ok()
                .and_then(|r| r.get(0));
            if last_ok.is_some_and(|t| dt_from_ms(t) > fresh_after) {
                continue;
            }
            let cd_key = (key.clone(), source);
            let in_cooldown = self
                .cooldown
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&cd_key)
                .is_some_and(|t| now - *t < chrono::Duration::from_std(FAILURE_COOLDOWN).unwrap_or_default());
            if in_cooldown {
                continue;
            }
            let q = SocialQuery { class: asset.class, symbol: asset.id.symbol.clone(), name: Some(asset.name.clone()), days: self.cfg.days };
            match p.fetch(&q).await {
                Ok(batch) => {
                    let window_start = now - chrono::Duration::days(i64::from(self.cfg.days));
                    let ok = !batch.budget_exhausted;
                    if let Err(e) = self.store(&key, source, &batch.posts, now, window_start, ok, batch.truncated, batch.reads, batch.budget_exhausted).await {
                        tracing::warn!(error = %e, "no se pudieron guardar las publicaciones sociales");
                        last_err = Some(e.to_string());
                    }
                    if batch.budget_exhausted {
                        last_err = Some("presupuesto mensual de lecturas de X agotado".into());
                    }
                }
                Err(e) => {
                    let msg = market::secret::redact(&e.to_string(), &self.cfg.secrets());
                    tracing::warn!(activo = %key, red = source.as_str(), error = %msg, "redes sociales: la consulta falló");
                    self.cooldown.lock().unwrap_or_else(|p| p.into_inner()).insert(cd_key, now);
                    let _ = sqlx::query("INSERT INTO social_runs (asset_id, source, fetched_at, window_start, ok, error) VALUES (?1, ?2, ?3, ?4, 0, ?5)")
                        .bind(&key)
                        .bind(source.as_str())
                        .bind(now.timestamp_millis())
                        .bind((now - chrono::Duration::days(i64::from(self.cfg.days))).timestamp_millis())
                        .bind(&msg)
                        .execute(&self.pool)
                        .await;
                    last_err = Some(msg);
                }
            }
        }
        last_err
    }

    #[allow(clippy::too_many_arguments)]
    async fn store(
        &self,
        asset: &str,
        source: SocialSource,
        posts: &[SocialPost],
        now: DateTime<Utc>,
        window_start: DateTime<Utc>,
        ok: bool,
        truncated: bool,
        reads: u32,
        budget_exhausted: bool,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let t = now.timestamp_millis();
        for p in posts {
            sqlx::query(
                "INSERT INTO social_posts (asset_id, post_id, source, community, author, title, body, score, comments, created_at, url, first_seen_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                 ON CONFLICT(asset_id, post_id) DO UPDATE SET score = excluded.score, comments = excluded.comments, last_seen_at = excluded.last_seen_at",
            )
            .bind(asset)
            .bind(&p.id)
            .bind(p.source.as_str())
            .bind(&p.community)
            .bind(&p.author)
            .bind(&p.title)
            .bind(&p.text)
            .bind(p.score)
            .bind(i64::from(p.comments))
            .bind(p.created_at.timestamp_millis())
            .bind(&p.url)
            .bind(t)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("INSERT INTO social_runs (asset_id, source, fetched_at, window_start, ok, posts, reads, truncated, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .bind(asset)
            .bind(source.as_str())
            .bind(t)
            .bind(window_start.timestamp_millis())
            .bind(ok)
            .bind(i64::try_from(posts.len()).unwrap_or(i64::MAX))
            .bind(i64::from(reads))
            .bind(truncated)
            .bind(budget_exhausted.then_some("presupuesto agotado"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM social_posts WHERE created_at < ?1").bind((now - chrono::Duration::days(POSTS_RETENTION_DAYS)).timestamp_millis()).execute(&mut *tx).await?;
        tx.commit().await
    }

    /// Lo guardado para el activo, sin consultar a nadie. `None` si no hay publicaciones recientes.
    pub async fn stored_digest(&self, asset: &AssetSummary, record: bool) -> Result<Option<SocialDigest>, sqlx::Error> {
        let key = asset.id.to_string();
        let now = Utc::now();
        let rows = sqlx::query(
            "SELECT post_id, source, community, author, title, body, score, comments, created_at, url FROM social_posts WHERE asset_id = ?1 AND created_at > ?2 AND created_at <= ?3",
        )
        .bind(&key)
        .bind((now - chrono::Duration::days(31)).timestamp_millis())
        .bind(now.timestamp_millis())
        .fetch_all(&self.pool)
        .await?;
        let posts: Vec<SocialPost> = rows
            .iter()
            .filter_map(|r| {
                Some(SocialPost {
                    id: r.get(0),
                    source: r.get::<String, _>(1).parse().ok()?,
                    community: r.get(2),
                    author: r.get(3),
                    title: r.get(4),
                    text: r.get(5),
                    score: r.get(6),
                    comments: u32::try_from(r.get::<i64, _>(7)).unwrap_or(0),
                    created_at: dt_from_ms(r.get(8)),
                    url: r.get(9),
                })
            })
            .collect();
        if posts.is_empty() {
            return Ok(None);
        }
        let runs: Vec<CoverageRun> = sqlx::query("SELECT fetched_at, window_start FROM social_runs WHERE asset_id = ?1 AND ok = 1 AND fetched_at > ?2")
            .bind(&key)
            .bind((now - chrono::Duration::days(40)).timestamp_millis())
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| CoverageRun { fetched_at: dt_from_ms(r.get(0)), window_start: dt_from_ms(r.get(1)) })
            .collect();
        let truncated: bool = sqlx::query("SELECT COALESCE(MAX(truncated), 0) FROM social_runs WHERE asset_id = ?1 AND ok = 1 AND fetched_at > ?2")
            .bind(&key)
            .bind((now - chrono::Duration::days(i64::from(self.cfg.days))).timestamp_millis())
            .fetch_one(&self.pool)
            .await?
            .get::<i64, _>(0)
            != 0;
        let metrics = compute_metrics(now, self.cfg.days, &posts, &runs, truncated);
        let excerpts = pick_excerpts(now, self.cfg.days, &posts, self.cfg.top_n);
        if record {
            self.record_metrics(&key, &metrics).await?;
        }
        Ok(Some(SocialDigest { asset: asset.id.clone(), metrics, excerpts }))
    }

    /// Añade una instantánea (nunca actualiza una anterior).
    async fn record_metrics(&self, key: &str, m: &domain::SocialMetrics) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO social_metrics (asset_id, as_of, window_days, mentions, authors, bullish, bearish, neutral, mentions_window, bullish_window, bearish_window, baseline_mean, baseline_days, ratio, z, truncated, by_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        )
        .bind(key)
        .bind(m.as_of.timestamp_millis())
        .bind(i64::from(m.window_days))
        .bind(i64::from(m.mentions))
        .bind(i64::from(m.authors))
        .bind(i64::from(m.bullish))
        .bind(i64::from(m.bearish))
        .bind(i64::from(m.neutral))
        .bind(i64::from(m.mentions_window))
        .bind(i64::from(m.bullish_window))
        .bind(i64::from(m.bearish_window))
        .bind(m.baseline_mean)
        .bind(i64::from(m.baseline_days))
        .bind(m.ratio)
        .bind(m.z)
        .bind(m.truncated)
        .bind(serde_json::to_string(&m.by_source).unwrap_or_else(|_| "[]".into()))
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// Refresca si toca y devuelve lo guardado (con instantánea de métricas).
    pub async fn digest_for(&self, asset: &AssetSummary) -> Result<Option<SocialDigest>, DomainError> {
        let _ = self.refresh_if_stale(asset).await;
        self.stored_digest(asset, true).await.map_err(|e| DomainError::Internal(format!("redes sociales: {e}")))
    }

    /// Ruta `GET /api/social/status`.
    pub async fn status(&self) -> Value {
        let now = Utc::now();
        let month = now.format("%Y-%m").to_string();
        let x = match &self.cfg.x {
            Some(x) => {
                let used = self.budget.used(&month).await;
                let remaining = x.monthly_read_budget.saturating_sub(used);
                json!({
                    "enabled": true,
                    "month": month,
                    "monthlyReadBudget": x.monthly_read_budget,
                    "used": used,
                    "remaining": remaining,
                    "costPerReadUsd": x.cost_per_read_usd,
                    "estimatedCostUsd": round4(f64::from(used) * x.cost_per_read_usd),
                    "maxMonthlyCostUsd": round4(f64::from(x.monthly_read_budget) * x.cost_per_read_usd),
                    "exhausted": remaining < market::social::X_MIN_RESULTS,
                    "accounts": x.accounts.len(),
                    "searchAll": x.search_all,
                })
            }
            None => json!({ "enabled": false }),
        };
        let last_error = sqlx::query("SELECT fetched_at, source, error FROM social_runs WHERE ok = 0 AND error IS NOT NULL ORDER BY id DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(|r| json!({ "at": dt_from_ms(r.get(0)), "source": r.get::<String, _>(1), "message": r.get::<String, _>(2) }));
        json!({
            "enabled": true,
            "reddit": { "enabled": self.cfg.reddit.is_some() },
            "x": x,
            "days": self.cfg.days,
            "refreshMinutes": self.cfg.refresh_minutes,
            "lastError": last_error,
            "notes": self.cfg.notes,
        })
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Estado cuando no hay redes cableadas.
pub fn disabled_status(cfg: &SocialConfig) -> Value {
    json!({ "enabled": false, "reddit": { "enabled": false }, "x": { "enabled": false }, "lastError": null, "notes": cfg.notes })
}

#[async_trait]
impl SocialData for SocialService {
    async fn digest(&self, asset: &AssetSummary) -> domain::Result<Option<SocialDigest>> {
        self.digest_for(asset).await
    }
}
