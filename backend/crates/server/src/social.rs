//! Redes sociales (Reddit y X) en el servidor: consulta con caché por activo, guardado con fecha
//! real, métricas deterministas y presupuesto mensual de X persistente. Diseño y riesgos en
//! `docs/notes/social.md`.
//!
//! Es OPINIÓN, no hechos, y no entra en la puntuación ni en los pesos. Todo está APAGADO por
//! defecto: sin credenciales `SocialService::from_config` devuelve `None`, no se cablea nada y no
//! se hace ninguna petición. Un fallo de redes nunca rompe un análisis ni el arranque.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

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
/// Con el presupuesto mensual de X agotado tampoco: cada intento escribiría una fila en `social_runs`
/// sin poder conseguir nada (B3). Se reintenta pasada una hora (o tras reiniciar con más presupuesto).
const BUDGET_COOLDOWN: Duration = Duration::from_secs(60 * 60);
/// Las publicaciones se conservan más que los 30 días de la línea base + el día actual.
const POSTS_RETENTION_DAYS: i64 = 45;
/// `social_runs` dice qué periodos están observados; `stored_digest` mira 40 días atrás.
const RUNS_RETENTION_DAYS: i64 = 45;
/// Las instantáneas de `social_metrics` son el material que `learning` evaluará algún día: se
/// guardan un año (con el tope de una por activo y hora son como mucho 8 760 filas por activo).
const METRICS_RETENTION_DAYS: i64 = 365;
/// Como mucho una instantánea de métricas por activo en este intervalo (B3).
const METRICS_MIN_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Ventana de recuento de activos distintos consultados en X.
const X_ASSETS_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Activos DISTINTOS que se consultan en X por hora si no se define `X_MAX_ASSETS_PER_HOUR`.
/// Es la segunda barrera (tras el control de origen de `guard.rs`) contra quien intente agotar el
/// presupuesto mensual con símbolos distintos. Los activos de la lista de seguimiento NO cuentan para
/// él (ver `SocialService::x_slot`), así que el uso interactivo dispone de los 30 huecos aunque la
/// pasada diaria esté en curso.
pub const DEFAULT_X_MAX_ASSETS_PER_HOUR: u32 = 30;
const MAX_X_MAX_ASSETS_PER_HOUR: u32 = 1000;
/// A partir de este número de activos se limpian los cerrojos y esperas que ya nadie usa.
const PRUNE_AT: usize = 512;

fn chrono_dur(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or_default()
}

/// `X_MAX_ASSETS_PER_HOUR`: entero entre 1 y 1000; sin definir, vacío o inválido, el valor por defecto.
pub fn parse_x_max_assets_per_hour(raw: Option<&str>) -> u32 {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => DEFAULT_X_MAX_ASSETS_PER_HOUR,
        Some(v) => match v.parse::<u32>() {
            Ok(n) if n >= 1 => n.min(MAX_X_MAX_ASSETS_PER_HOUR),
            _ => {
                tracing::warn!(valor = v, "X_MAX_ASSETS_PER_HOUR no es un entero positivo: se usa {DEFAULT_X_MAX_ASSETS_PER_HOUR}");
                DEFAULT_X_MAX_ASSETS_PER_HOUR
            }
        },
    }
}

/// Ids de los activos de la lista de seguimiento (`WATCHLIST`, o la de por defecto) si está activada: los
/// que no cuentan para `X_MAX_ASSETS_PER_HOUR`. Lee las mismas variables que `Config::from_env`, que ya
/// habrá fallado al arrancar si son inválidas (aquí un error equivale a «sin lista»).
fn watchlist_assets() -> HashSet<String> {
    match crate::watchlist::WatchlistConfig::from_env() {
        Ok(c) if c.enabled => c.assets.iter().map(ToString::to_string).collect(),
        _ => HashSet::new(),
    }
}

/// Resultado de un intento de refresco de un activo.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refresh {
    /// Último error (fallo de red, presupuesto agotado, tope de activos de X).
    pub error: Option<String>,
    /// Alguna red respondió y se guardó lo recibido. Es informativo: la instantánea de métricas ya no
    /// depende de este valor en memoria (se decide en SQL, ver `record_metrics`).
    pub refreshed: bool,
}

/// Activos distintos consultados en X en la última hora.
#[derive(Default)]
struct XWindow {
    seen: HashMap<String, DateTime<Utc>>,
    /// Última vez que se avisó en el log de que se alcanzó el tope (un aviso por ventana).
    warned_at: Option<DateTime<Utc>>,
}

enum XSlot {
    Granted,
    /// `first`: es la primera denegación de esta ventana (para avisar una sola vez).
    Denied { first: bool },
}

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
    /// Hasta cuándo no se vuelve a intentar un activo y red (fallo o presupuesto agotado).
    cooldown: std::sync::Mutex<HashMap<(String, SocialSource), DateTime<Utc>>>,
    /// Tope de activos distintos consultados en X por hora (`X_MAX_ASSETS_PER_HOUR`).
    x_max_assets: u32,
    /// Activos que no cuentan para ese tope: los de la lista de seguimiento (un conjunto fijo que
    /// define el usuario). El presupuesto mensual de lecturas sí los cuenta siempre.
    x_exempt: HashSet<String>,
    x_window: std::sync::Mutex<XWindow>,
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
        let x_max_assets = parse_x_max_assets_per_hour(std::env::var("X_MAX_ASSETS_PER_HOUR").ok().as_deref());
        let x_exempt = watchlist_assets();
        if cfg.x.is_some() {
            tracing::info!(activos_por_hora = x_max_assets, exentos = x_exempt.len(), "X: tope de activos distintos consultados por hora (los de la lista de seguimiento no cuentan)");
        }
        Some(Self::build(pool, cfg, providers, budget, x_max_assets, x_exempt))
    }

    /// Con proveedores a medida (tests) y el tope de activos de X por defecto.
    pub fn with_providers(cfg: SocialConfig, pool: SqlitePool, providers: Vec<Arc<dyn SocialProvider>>, budget: Arc<SqliteBudget>) -> Arc<Self> {
        Self::build(pool, cfg, providers, budget, DEFAULT_X_MAX_ASSETS_PER_HOUR, HashSet::new())
    }

    /// Como `with_providers`, con otro tope de activos distintos por hora en X (tests).
    pub fn with_providers_and_x_cap(
        cfg: SocialConfig,
        pool: SqlitePool,
        providers: Vec<Arc<dyn SocialProvider>>,
        budget: Arc<SqliteBudget>,
        x_max_assets_per_hour: u32,
    ) -> Arc<Self> {
        Self::build(pool, cfg, providers, budget, x_max_assets_per_hour.max(1), HashSet::new())
    }

    /// Como `with_providers_and_x_cap`, y además con los ids de activo (`stock:AAPL`) que no cuentan para
    /// el tope: los de la lista de seguimiento (tests).
    pub fn with_providers_and_x_limits(
        cfg: SocialConfig,
        pool: SqlitePool,
        providers: Vec<Arc<dyn SocialProvider>>,
        budget: Arc<SqliteBudget>,
        x_max_assets_per_hour: u32,
        x_exempt: impl IntoIterator<Item = String>,
    ) -> Arc<Self> {
        Self::build(pool, cfg, providers, budget, x_max_assets_per_hour.max(1), x_exempt.into_iter().collect())
    }

    fn build(
        pool: SqlitePool,
        cfg: SocialConfig,
        providers: Vec<Arc<dyn SocialProvider>>,
        budget: Arc<SqliteBudget>,
        x_max_assets: u32,
        x_exempt: HashSet<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            providers,
            budget,
            locks: Default::default(),
            cooldown: Default::default(),
            x_max_assets,
            x_exempt,
            x_window: Default::default(),
        })
    }

    pub fn config(&self) -> &SocialConfig {
        &self.cfg
    }

    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|p| p.into_inner());
        if locks.len() >= PRUNE_AT {
            // Sin más dueño que el mapa: nadie está refrescando ese activo. Evita que símbolos
            // inventados hagan crecer el mapa sin límite.
            locks.retain(|_, l| Arc::strong_count(l) > 1);
        }
        locks.entry(key.to_string()).or_default().clone()
    }

    /// No se vuelve a intentar `(activo, red)` hasta `until`. Poda de paso las esperas vencidas.
    fn set_cooldown(&self, key: (String, SocialSource), until: DateTime<Utc>, now: DateTime<Utc>) {
        let mut cd = self.cooldown.lock().unwrap_or_else(|p| p.into_inner());
        if cd.len() >= PRUNE_AT {
            cd.retain(|_, u| *u > now);
        }
        cd.insert(key, until);
    }

    /// Reserva un hueco para consultar `asset` en X: como mucho `x_max_assets` activos distintos en
    /// la última hora. Un activo ya contado sigue entrando (no ocupa un hueco nuevo).
    ///
    /// Los de la lista de seguimiento no ocupan hueco ni quedan fuera por falta de él: la pasada diaria
    /// gasta tantos huecos como activos tiene la lista, y si el uso interactivo los agotaba el resto de
    /// la lista se analizaba sin X, sesgando la serie que mide `learning` según lo que se hubiera abierto
    /// ese día. No hay abuso posible: es un conjunto fijo por configuración (no crece con símbolos
    /// inventados) y cada activo sigue limitado por `SOCIAL_REFRESH_MINUTES` y por el presupuesto
    /// MENSUAL de lecturas, que cuenta siempre. (Desde `SocialData::digest` no se conoce quién llama:
    /// el análisis corre en una tarea aparte, así que se exime por activo y no por origen.)
    fn x_slot(&self, asset: &str, now: DateTime<Utc>) -> XSlot {
        if self.x_exempt.contains(asset) {
            return XSlot::Granted;
        }
        let mut w = self.x_window.lock().unwrap_or_else(|p| p.into_inner());
        let since = now - chrono_dur(X_ASSETS_WINDOW);
        w.seen.retain(|_, t| *t > since);
        let max = usize::try_from(self.x_max_assets).unwrap_or(usize::MAX);
        if w.seen.contains_key(asset) || w.seen.len() < max {
            w.seen.insert(asset.to_string(), now);
            return XSlot::Granted;
        }
        let first = w.warned_at.is_none_or(|t| t <= since);
        if first {
            w.warned_at = Some(now);
        }
        XSlot::Denied { first }
    }

    /// Estado del tope de activos distintos por hora en X, para `GET /api/social/status`.
    fn x_assets_status(&self, now: DateTime<Utc>) -> Value {
        let w = self.x_window.lock().unwrap_or_else(|p| p.into_inner());
        let since = now - chrono_dur(X_ASSETS_WINDOW);
        let used = w.seen.values().filter(|t| **t > since).count();
        let max = usize::try_from(self.x_max_assets).unwrap_or(usize::MAX);
        let full = used >= max;
        // Cuándo se libera un hueco: cuando el activo contado más antiguo salga de la ventana.
        let next_slot_at = full.then(|| w.seen.values().filter(|t| **t > since).min().map(|t| *t + chrono_dur(X_ASSETS_WINDOW))).flatten();
        json!({
            "limit": self.x_max_assets,
            "used": used,
            "full": full,
            "windowMinutes": X_ASSETS_WINDOW.as_secs() / 60,
            "exemptAssets": self.x_exempt.len(),
            "nextSlotAt": next_slot_at,
        })
    }

    /// Consulta las redes si la última consulta correcta de ese activo y red es más antigua que
    /// `refresh_minutes`. Nunca falla: devuelve el último error (si lo hubo) como texto.
    pub async fn refresh_if_stale(&self, asset: &AssetSummary) -> Option<String> {
        self.refresh(asset).await.error
    }

    /// Como `refresh_if_stale`, y además dice si hubo un refresco real (para anotar métricas).
    pub async fn refresh(&self, asset: &AssetSummary) -> Refresh {
        let key = asset.id.to_string();
        let lock = self.lock_for(&key);
        let _g = lock.lock().await;
        let mut last_err = None;
        let mut refreshed = false;
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
            let in_cooldown = self.cooldown.lock().unwrap_or_else(|p| p.into_inner()).get(&cd_key).is_some_and(|until| now < *until);
            if in_cooldown {
                continue;
            }
            // Segunda barrera del presupuesto de X: activos DISTINTOS por hora. Va después de la
            // caché y de la espera, así que solo cuentan consultas que de verdad iban a salir.
            if source == SocialSource::X
                && let XSlot::Denied { first } = self.x_slot(&key, now)
            {
                if first {
                    tracing::warn!(
                        tope = self.x_max_assets,
                        "X: tope de activos distintos consultados por hora alcanzado (X_MAX_ASSETS_PER_HOUR); no se consulta X hasta que baje"
                    );
                }
                last_err = Some("tope de activos distintos por hora en X alcanzado".to_string());
                continue;
            }
            let q = SocialQuery { class: asset.class, symbol: asset.id.symbol.clone(), name: Some(asset.name.clone()), days: self.cfg.days };
            match p.fetch(&q).await {
                Ok(batch) => {
                    let window_start = now - chrono::Duration::days(i64::from(self.cfg.days));
                    let ok = !batch.budget_exhausted;
                    match self.store(&key, source, &batch.posts, now, window_start, ok, batch.truncated, batch.reads, batch.budget_exhausted).await {
                        Ok(()) => refreshed |= ok,
                        Err(e) => {
                            tracing::warn!(error = %e, "no se pudieron guardar las publicaciones sociales");
                            last_err = Some(e.to_string());
                            self.set_cooldown(cd_key.clone(), now + chrono_dur(FAILURE_COOLDOWN), now);
                        }
                    }
                    if batch.budget_exhausted {
                        last_err = Some("presupuesto mensual de lecturas de X agotado".into());
                        // Sin esto, cada GET repetiría la consulta y escribiría otra fila en `social_runs`.
                        self.set_cooldown(cd_key, now + chrono_dur(BUDGET_COOLDOWN), now);
                    }
                }
                Err(e) => {
                    let msg = market::secret::redact(&e.to_string(), &self.cfg.secrets());
                    tracing::warn!(activo = %key, red = source.as_str(), error = %msg, "redes sociales: la consulta falló");
                    self.set_cooldown(cd_key, now + chrono_dur(FAILURE_COOLDOWN), now);
                    let _ = sqlx::query("INSERT INTO social_runs (asset_id, source, fetched_at, window_start, ok, error) VALUES (?1, ?2, ?3, ?4, 0, ?5)")
                        .bind(&key)
                        .bind(source.as_str())
                        .bind(now.timestamp_millis())
                        .bind((now - chrono::Duration::days(i64::from(self.cfg.days))).timestamp_millis())
                        .bind(&msg)
                        .execute(&self.pool)
                        .await;
                    self.purge(now).await;
                    last_err = Some(msg);
                }
            }
        }
        Refresh { error: last_err, refreshed }
    }

    /// Retención de las tablas de redes (también cuando solo hay fallos y `store` no llega a correr).
    async fn purge(&self, now: DateTime<Utc>) {
        match self.pool.acquire().await {
            Ok(mut conn) => {
                if let Err(e) = purge_old(&mut conn, now).await {
                    tracing::warn!(error = %e, "no se pudo aplicar la retención de las tablas de redes sociales");
                }
            }
            Err(e) => tracing::warn!(error = %e, "no se pudo aplicar la retención de las tablas de redes sociales"),
        }
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
        purge_old(&mut tx, now).await?;
        tx.commit().await
    }

    /// Lo guardado para el activo, sin consultar a nadie. `None` si no hay publicaciones recientes.
    /// Con `record` intenta añadir una instantánea de métricas; `record_metrics` lo decide en SQL: solo
    /// si hay una ejecución correcta más reciente que la última instantánea (un refresco real aún no
    /// anotado) y ninguna instantánea del activo en la última hora (`METRICS_MIN_INTERVAL`). Es
    /// idempotente y barato, así que `digest_for` lo pide siempre.
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

    /// Añade una instantánea (nunca actualiza una anterior). La decisión va en la propia sentencia
    /// `INSERT … SELECT … WHERE`, no en una variable en memoria, así que resiste cancelaciones y
    /// reinicios (p. ej. el análisis corta `digest()` a los 20 s después de guardar el refresco y antes
    /// de anotar la instantánea: la siguiente lectura la anota). Solo se inserta si:
    /// - no hay otra instantánea del activo en la última hora (`?18`), y
    /// - existe una ejecución correcta (`ok = 1`) más reciente que la última instantánea: hubo un
    ///   refresco real que ninguna instantánea recoge todavía. Una lectura de lo ya guardado, un
    ///   fallo o el presupuesto agotado (`ok = 0`) no la crean.
    ///
    /// Comprobación e inserción van en una sola sentencia: sin carreras.
    async fn record_metrics(&self, key: &str, m: &domain::SocialMetrics) -> Result<(), sqlx::Error> {
        let recent_after = m.as_of - chrono_dur(METRICS_MIN_INTERVAL);
        sqlx::query(
            "INSERT INTO social_metrics (asset_id, as_of, window_days, mentions, authors, bullish, bearish, neutral, mentions_window, bullish_window, bearish_window, baseline_mean, baseline_days, ratio, z, truncated, by_source)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
             WHERE NOT EXISTS (SELECT 1 FROM social_metrics WHERE asset_id = ?1 AND as_of > ?18)
               AND EXISTS (SELECT 1 FROM social_runs WHERE asset_id = ?1 AND ok = 1
                           AND fetched_at > COALESCE((SELECT MAX(as_of) FROM social_metrics WHERE asset_id = ?1), 0))",
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
        .bind(recent_after.timestamp_millis())
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// Refresca si toca y devuelve lo guardado. La instantánea de métricas solo se anota si hay un
    /// refresco real que ninguna instantánea recoge todavía (no por cada lectura de lo ya guardado) y
    /// como mucho una por activo y hora. Se pide en cada llamada porque la decisión es de SQL: si una
    /// petición se cancela entre guardar el refresco y anotarla, la siguiente lectura la anota.
    pub async fn digest_for(&self, asset: &AssetSummary) -> Result<Option<SocialDigest>, DomainError> {
        let _ = self.refresh(asset).await;
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
                    "assetsPerHour": self.x_assets_status(now),
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

/// Retención de las tres tablas de redes que crecen con el uso (el presupuesto mensual es una
/// fila por mes). Se ejecuta tras cada refresco y tras cada fallo registrado.
async fn purge_old(conn: &mut sqlx::SqliteConnection, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    let cutoff = |days: i64| (now - chrono::Duration::days(days)).timestamp_millis();
    sqlx::query("DELETE FROM social_posts WHERE created_at < ?1").bind(cutoff(POSTS_RETENTION_DAYS)).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM social_runs WHERE fetched_at < ?1").bind(cutoff(RUNS_RETENTION_DAYS)).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM social_metrics WHERE as_of < ?1").bind(cutoff(METRICS_RETENTION_DAYS)).execute(&mut *conn).await?;
    Ok(())
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
