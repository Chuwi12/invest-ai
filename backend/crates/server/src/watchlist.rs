//! Lista de seguimiento: análisis diarios, automáticos y de uno en uno de una lista fija de
//! activos, para construir un historial de aciertos medido fuera de muestra (`docs/notes/learning.md`).
//!
//! - **Programador** ([`run_scheduler`]): una pasada al día a `WATCHLIST_TIME` (hora local). Si el
//!   servidor arranca cuando ya pasó la hora y ese día aún faltan activos, la recupera de inmediato
//!   (`trigger = recovery`; se anota la hora real de inicio junto a la prevista).
//! - **Ejecución** ([`run_round`]): los activos de uno en uno (una sola GPU) por el mismo camino que
//!   la API (`routes::analysis::run_to_completion`: single-flight, límite de generaciones, guardado
//!   en BD con `origin = 'watchlist'`). Cada intento y cada fallo queda en `watchlist_results`.
//! - **Fallos**: un activo que falla no detiene la lista. Los fallos transitorios (Ollama caído,
//!   límites, red) se reintentan con espera; si fallan varios activos seguidos se aborta la pasada
//!   (no tiene sentido recorrer 20 activos contra un Ollama muerto) y se reintenta el resto más tarde
//!   el mismo día. Nada de esto bloquea el servidor: corre en su propia tarea.
//! - **Reloj** ([`Clock`]): inyectable. En producción [`SystemClock`] (zona local del sistema);
//!   en pruebas [`ManualClock`].

use std::{sync::Arc, time::Duration};

use anyhow::{Context, bail};
use async_trait::async_trait;
use chrono::{DateTime, Days, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use domain::{AnalysisDepth, AssetId, DomainError};
use futures::FutureExt;
use tokio::sync::watch;

use crate::{
    repo::watchlist::{self as repo, DayState, NewResult},
    routes::analysis::run_to_completion,
    state::AppState,
};

/// Lista por defecto, decidida A PRIORI (el porqué está en `docs/notes/learning.md`). Cambiarla
/// según los resultados invalida la medición: no la toques mirando el rendimiento.
pub const DEFAULT_WATCHLIST: [&str; 20] = [
    // Grandes de EE. UU. de ocho sectores.
    "stock:AAPL", "stock:MSFT", "stock:NVDA", "stock:AMZN", "stock:TSLA", "stock:JPM", "stock:XOM", "stock:JNJ",
    "stock:KO", "stock:WMT", "stock:CAT", "stock:NEE",
    // ADR europeos (los valores con sufijo de bolsa extranjera no se resuelven).
    "stock:ASML", "stock:SAP",
    // ETF: renta variable de EE. UU., pequeñas empresas, oro y bonos largos.
    "etf:QQQ", "etf:IWM", "etf:GLD", "etf:TLT",
    // Cripto (benchmark BTC).
    "crypto:ETH", "crypto:SOL",
];

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Intentos por activo en una pasada (1 = sin reintentos).
    pub attempts: u32,
    /// Espera antes del intento 2, 3…; si faltan, se repite la última.
    pub backoff: Vec<Duration>,
    /// Fallos transitorios seguidos (tras agotar los reintentos) que abortan la pasada.
    pub abort_after: u32,
    /// Espera entre rondas del mismo día cuando quedan activos por hacer.
    pub round_cooldown: Duration,
    /// Rondas máximas por día (la primera cuenta).
    pub max_rounds: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            backoff: vec![Duration::from_secs(60), Duration::from_secs(300)],
            abort_after: 2,
            round_cooldown: Duration::from_secs(30 * 60),
            max_rounds: 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatchlistConfig {
    pub enabled: bool,
    pub assets: Vec<AssetId>,
    /// `true` si `WATCHLIST` estaba vacío y se usa la lista a priori.
    pub default_list: bool,
    /// Hora local de la pasada diaria.
    pub time: NaiveTime,
    pub depth: AnalysisDepth,
    pub retry: RetryPolicy,
    /// Máximo que se aplaza la pasada si el equipo va con batería (`WATCHLIST_BATTERY_MAX_DELAY_HOURS`;
    /// 0 = no aplazar nunca).
    pub battery_max_delay: Duration,
    /// Cada cuánto se vuelve a mirar si ya está enchufado.
    pub power_poll: Duration,
}

impl Default for WatchlistConfig {
    fn default() -> Self {
        Self::build(false, "", "23:00", "standard").expect("valores por defecto válidos")
    }
}

impl WatchlistConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str, d: &str| match std::env::var(k) {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => d.to_string(),
        };
        // Activada por defecto: sin historial no hay nada que medir.
        let enabled = !matches!(get("WATCHLIST_ENABLED", "1").to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
        let mut cfg = Self::build(
            enabled,
            &std::env::var("WATCHLIST").unwrap_or_default(),
            &get("WATCHLIST_TIME", "23:00"),
            &get("WATCHLIST_DEPTH", "standard"),
        )?;
        let hours: f64 = get("WATCHLIST_BATTERY_MAX_DELAY_HOURS", "3")
            .replace(',', ".")
            .parse()
            .ok()
            .filter(|h: &f64| h.is_finite() && *h >= 0.0)
            .context("WATCHLIST_BATTERY_MAX_DELAY_HOURS debe ser un número de horas >= 0")?;
        cfg.battery_max_delay = Duration::from_secs_f64(hours * 3600.0);
        Ok(cfg)
    }

    pub fn build(enabled: bool, list: &str, time: &str, depth: &str) -> anyhow::Result<Self> {
        let default_list = list.trim().is_empty();
        let raw: Vec<&str> = if default_list {
            DEFAULT_WATCHLIST.to_vec()
        } else {
            list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()
        };
        let mut assets: Vec<AssetId> = Vec::new();
        for r in raw {
            let id: AssetId =
                r.parse().map_err(|e| anyhow::anyhow!("WATCHLIST: «{r}» no es un id de activo válido (p. ej. stock:AAPL): {e}"))?;
            if !assets.contains(&id) {
                assets.push(id);
            }
        }
        let time = NaiveTime::parse_from_str(time.trim(), "%H:%M")
            .with_context(|| format!("WATCHLIST_TIME «{time}» no es una hora HH:MM válida"))?;
        let depth: AnalysisDepth = depth.parse().map_err(|e| anyhow::anyhow!("WATCHLIST_DEPTH: {e}"))?;
        if enabled && assets.is_empty() {
            bail!("WATCHLIST vacía");
        }
        Ok(Self {
            enabled,
            assets,
            default_list,
            time,
            depth,
            retry: RetryPolicy::default(),
            battery_max_delay: Duration::from_secs(3 * 3600),
            power_poll: Duration::from_secs(300),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Reloj
// ---------------------------------------------------------------------------------------------

#[async_trait]
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
    /// Hora local correspondiente a un instante.
    fn utc_to_local(&self, t: DateTime<Utc>) -> NaiveDateTime;
    /// Instante de una hora local (en saltos de horario de verano, el siguiente válido).
    fn local_to_utc(&self, l: NaiveDateTime) -> DateTime<Utc>;
    async fn sleep_until(&self, t: DateTime<Utc>);
}

/// Reloj real con la zona horaria local del sistema.
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn utc_to_local(&self, t: DateTime<Utc>) -> NaiveDateTime {
        t.with_timezone(&Local).naive_local()
    }

    fn local_to_utc(&self, l: NaiveDateTime) -> DateTime<Utc> {
        let resolve = |l: NaiveDateTime| Local.from_local_datetime(&l).earliest();
        resolve(l)
            .or_else(|| resolve(l + chrono::Duration::hours(1))) // hueco del cambio de hora
            .map_or_else(|| Utc.from_utc_datetime(&l), |d| d.with_timezone(&Utc))
    }

    /// Duerme en tramos cortos y vuelve a mirar el reloj de pared: el temporizador monótono de
    /// tokio no avanza con el equipo suspendido y la pasada saldría tarde al despertar.
    async fn sleep_until(&self, t: DateTime<Utc>) {
        loop {
            let left = (t - Utc::now()).to_std().unwrap_or_default();
            if left.is_zero() {
                return;
            }
            tokio::time::sleep(left.min(Duration::from_secs(30))).await;
        }
    }
}

/// Reloj controlable para pruebas. Con un `horizon`, las esperas que no lo superan saltan en el
/// acto (el tiempo avanza solo hasta ahí); las que lo superan esperan a que la prueba mueva el reloj.
pub struct ManualClock {
    offset: FixedOffset,
    tx: watch::Sender<(DateTime<Utc>, DateTime<Utc>)>,
}

impl ManualClock {
    /// `offset_secs`: desfase de la zona local respecto a UTC. Sin avance automático.
    pub fn new(start: DateTime<Utc>, offset_secs: i32) -> Arc<Self> {
        let (tx, _) = watch::channel((start, start));
        Arc::new(Self { offset: FixedOffset::east_opt(offset_secs).expect("desfase válido"), tx })
    }

    /// Mueve el reloj a `t` (y deja pasar las esperas que venzan).
    pub fn set(&self, t: DateTime<Utc>) {
        self.tx.send_modify(|(now, horizon)| {
            *now = t;
            *horizon = (*horizon).max(t);
        });
    }

    /// Las esperas que acaben antes de `t` saltan solas; las posteriores esperan.
    pub fn auto_advance_until(&self, t: DateTime<Utc>) {
        self.tx.send_modify(|(_, horizon)| *horizon = t);
    }
}

#[async_trait]
impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        self.tx.borrow().0
    }

    fn utc_to_local(&self, t: DateTime<Utc>) -> NaiveDateTime {
        t.with_timezone(&self.offset).naive_local()
    }

    fn local_to_utc(&self, l: NaiveDateTime) -> DateTime<Utc> {
        self.offset.from_local_datetime(&l).unwrap().with_timezone(&Utc)
    }

    async fn sleep_until(&self, t: DateTime<Utc>) {
        let mut rx = self.tx.subscribe();
        loop {
            let reached = {
                let mut reached = false;
                self.tx.send_modify(|(now, horizon)| {
                    if *now >= t {
                        reached = true;
                    } else if t <= *horizon {
                        *now = t; // salto
                        reached = true;
                    }
                });
                reached
            };
            if reached {
                tokio::task::yield_now().await;
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Energía
// ---------------------------------------------------------------------------------------------

/// ¿Va el equipo con batería? Con batería la GPU de un portátil queda limitada y cada análisis
/// tarda el doble, así que la pasada diaria espera (con tope) a que se enchufe.
pub trait PowerProbe: Send + Sync + 'static {
    /// `Some(true)` = con batería; `Some(false)` = con corriente; `None` = no se puede saber.
    fn on_battery(&self) -> Option<bool>;
}

/// Lee `/sys/class/power_supply/*`: batería si hay una `Battery`, alguna fuente externa (`Mains`,
/// `USB*`) y ninguna está conectada (`online = 0`). Sin batería o sin fuentes externas listadas
/// (sobremesa, VM) = desconocido: un sobremesa con puertos USB-C listados y desconectados no espera.
pub struct SystemPower {
    root: std::path::PathBuf,
}

impl Default for SystemPower {
    fn default() -> Self {
        Self { root: "/sys/class/power_supply".into() }
    }
}

impl SystemPower {
    pub fn at(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl PowerProbe for SystemPower {
    fn on_battery(&self) -> Option<bool> {
        let mut supplies = 0;
        let mut online = false;
        let mut battery = false;
        for entry in std::fs::read_dir(&self.root).ok()?.flatten() {
            let dir = entry.path();
            let kind = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
            let kind = kind.trim();
            if kind == "Battery" {
                battery = true;
                continue;
            }
            if kind != "Mains" && !kind.starts_with("USB") {
                continue;
            }
            let Some(on) = std::fs::read_to_string(dir.join("online")).ok().map(|v| v.trim() == "1") else {
                continue;
            };
            supplies += 1;
            online |= on;
        }
        (battery && supplies > 0).then_some(!online)
    }
}

/// Sonda de pruebas: devuelve lo que se le diga.
#[derive(Default)]
pub struct ManualPower(pub std::sync::Mutex<Option<bool>>);

impl PowerProbe for ManualPower {
    fn on_battery(&self) -> Option<bool> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Espera (hasta `battery_max_delay`) a que el equipo se enchufe. Devuelve la etiqueta de energía
/// con la que empezar y los segundos esperados; `None` = apagado.
async fn wait_for_power(state: &AppState, cfg: &WatchlistConfig, clock: &dyn Clock) -> Option<(&'static str, i64)> {
    let start = clock.now();
    let mut announced = false;
    loop {
        let on_battery = state.power.on_battery();
        let waited = clock.now() - start;
        let label = match on_battery {
            Some(true) => "battery",
            Some(false) => "ac",
            None => "unknown",
        };
        let max = chrono::Duration::from_std(cfg.battery_max_delay).unwrap_or_default();
        if on_battery != Some(true) || waited >= max {
            if on_battery == Some(true) {
                tracing::warn!(horas = waited.num_minutes() as f64 / 60.0, "lista de seguimiento: sigue con batería; se hace la pasada igualmente (tope de espera)");
            } else if announced {
                tracing::info!(minutos = waited.num_minutes(), "lista de seguimiento: equipo enchufado; empieza la pasada");
            }
            return Some((label, waited.num_seconds()));
        }
        if !announced {
            tracing::warn!(max = ?cfg.battery_max_delay, "lista de seguimiento: el equipo va con batería; se aplaza la pasada hasta que se enchufe");
            announced = true;
        }
        let poll = chrono::Duration::from_std(cfg.power_poll).unwrap_or(chrono::Duration::minutes(5));
        let wake = (clock.now() + poll).min(start + max);
        if !wait_until(state, clock, wake.max(clock.now() + chrono::Duration::seconds(1))).await {
            return None;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fechas
// ---------------------------------------------------------------------------------------------

fn local_day(clock: &dyn Clock, t: DateTime<Utc>) -> NaiveDate {
    clock.utc_to_local(t).date()
}

fn due_of(clock: &dyn Clock, day: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    clock.local_to_utc(day.and_time(time))
}

/// Próxima hora programada estrictamente posterior a `now` si hoy ya pasó, o la de hoy si no.
pub fn next_due(clock: &dyn Clock, now: DateTime<Utc>, time: NaiveTime) -> DateTime<Utc> {
    let day = local_day(clock, now);
    let today = due_of(clock, day, time);
    if now < today { today } else { due_of(clock, day + Days::new(1), time) }
}

// ---------------------------------------------------------------------------------------------
// Programador
// ---------------------------------------------------------------------------------------------

/// Espera cancelable por el apagado. `false` = hay que salir.
async fn wait_until(state: &AppState, clock: &dyn Clock, t: DateTime<Utc>) -> bool {
    tokio::select! {
        () = state.shutdown.cancelled() => false,
        () = clock.sleep_until(t) => !state.shutdown.is_cancelled(),
    }
}

/// Pasada tardía: si empieza más de esto después de la hora prevista, cuenta como recuperación.
const LATE_AFTER: chrono::Duration = chrono::Duration::minutes(10);

pub async fn run_scheduler(state: AppState, cfg: Arc<WatchlistConfig>, clock: Arc<dyn Clock>) {
    if !cfg.enabled {
        return;
    }
    match repo::mark_interrupted(&state.db, clock.now().timestamp_millis()).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(n, "lista de seguimiento: había pasadas sin terminar de una ejecución anterior (marcadas como interrumpidas)"),
        Err(e) => tracing::error!(error = %e, "lista de seguimiento: no se pudo revisar el registro"),
    }
    tracing::info!(activos = cfg.assets.len(), hora = %cfg.time, profundidad = ?cfg.depth, "lista de seguimiento activa");
    let mut first = true;
    // Día en el que ya se agotaron las rondas: no insistir hasta el siguiente.
    let mut gave_up: Option<NaiveDate> = None;
    loop {
        if state.shutdown.is_cancelled() {
            return;
        }
        let now = clock.now();
        let day = local_day(clock.as_ref(), now);
        let due = due_of(clock.as_ref(), day, cfg.time);
        if now < due {
            if !wait_until(&state, clock.as_ref(), due).await {
                return;
            }
            first = false;
            continue;
        }
        let date = day.to_string();
        let pending = match pending_assets(&state, &cfg, &date).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "lista de seguimiento: no se pudo leer el registro; se reintenta en 5 min");
                if !wait_until(&state, clock.as_ref(), now + chrono::Duration::minutes(5)).await {
                    return;
                }
                continue;
            }
        };
        if pending.is_empty() || gave_up == Some(day) {
            first = false;
            let tomorrow = due_of(clock.as_ref(), day + Days::new(1), cfg.time);
            if !wait_until(&state, clock.as_ref(), tomorrow).await {
                return;
            }
            continue;
        }
        let mut trigger = if (first && now > due) || now - due > LATE_AFTER { "recovery" } else { "schedule" };
        first = false;
        let mut round = 1;
        let mut power = match wait_for_power(&state, &cfg, clock.as_ref()).await {
            Some(p) => p,
            None => return,
        };
        loop {
            let fut = run_round(&state, &cfg, clock.as_ref(), day, due, trigger, power);
            match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
                Ok(Ok(RoundEnd::Shutdown)) => return,
                Ok(Ok(RoundEnd::Finished)) => {}
                Ok(Err(e)) => tracing::error!(error = %e, "lista de seguimiento: la pasada falló"),
                Err(_) => {
                    tracing::error!("lista de seguimiento: la pasada hizo panic; se registra como interrumpida");
                    let _ = repo::mark_interrupted(&state.db, clock.now().timestamp_millis()).await;
                }
            }
            let left = pending_assets(&state, &cfg, &date).await.map(|p| p.len()).unwrap_or(0);
            if left == 0 {
                break;
            }
            if round >= cfg.retry.max_rounds {
                tracing::warn!(left, "lista de seguimiento: se agotaron las rondas de hoy; quedan activos sin analizar");
                gave_up = Some(day);
                break;
            }
            tracing::warn!(left, espera = ?cfg.retry.round_cooldown, "lista de seguimiento: quedan activos por analizar; otra ronda más tarde");
            let resume = clock.now() + chrono::Duration::from_std(cfg.retry.round_cooldown).unwrap_or_default();
            if !wait_until(&state, clock.as_ref(), resume).await {
                return;
            }
            if local_day(clock.as_ref(), clock.now()) != day {
                break; // cambió el día: la pasada de mañana empieza de cero
            }
            round += 1;
            trigger = "retry";
            power = (match state.power.on_battery() {
                Some(true) => "battery",
                Some(false) => "ac",
                None => "unknown",
            }, 0);
        }
    }
}

/// Activos de la lista que ese día aún necesitan un análisis válido.
async fn pending_assets(state: &AppState, cfg: &WatchlistConfig, date: &str) -> Result<Vec<AssetId>, crate::error::ApiError> {
    let states = repo::day_states(&state.db, date).await?;
    Ok(cfg
        .assets
        .iter()
        .filter(|a| matches!(states.get(&a.to_string()).copied(), None | Some(DayState::Pending)))
        .cloned()
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub enum RoundEnd {
    Finished,
    Shutdown,
}

/// Se considera transitorio cualquier fallo salvo «no existe» y «entrada no válida».
fn is_transient(e: &DomainError) -> bool {
    !matches!(e, DomainError::NotFound(_) | DomainError::InvalidInput(_))
}

enum Attempt {
    Ok(Box<domain::Analysis>, u32),
    Failed { error: DomainError, attempts: u32 },
    Shutdown,
}

async fn analyze_with_retries(
    state: &AppState,
    cfg: &WatchlistConfig,
    clock: &dyn Clock,
    id: &AssetId,
) -> Attempt {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let res = tokio::select! {
            () = state.shutdown.cancelled() => return Attempt::Shutdown,
            r = run_to_completion(state, id, Some(cfg.depth), "watchlist") => r,
        };
        match res {
            Ok(a) => return Attempt::Ok(Box::new(a), attempts),
            Err(_) if state.shutdown.is_cancelled() => return Attempt::Shutdown,
            Err(error) => {
                tracing::warn!(%id, attempts, %error, "lista de seguimiento: análisis fallido");
                if !is_transient(&error) || attempts >= cfg.retry.attempts {
                    return Attempt::Failed { error, attempts };
                }
                let idx = (attempts as usize - 1).min(cfg.retry.backoff.len().saturating_sub(1));
                let wait = cfg.retry.backoff.get(idx).copied().unwrap_or(Duration::from_secs(60));
                let until = clock.now() + chrono::Duration::from_std(wait).unwrap_or_default();
                if !wait_until(state, clock, until).await {
                    return Attempt::Shutdown;
                }
            }
        }
    }
}

/// Una ronda: los activos pendientes del día, de uno en uno. Registra todo en BD.
pub async fn run_round(
    state: &AppState,
    cfg: &WatchlistConfig,
    clock: &dyn Clock,
    day: NaiveDate,
    due: DateTime<Utc>,
    trigger: &str,
    power: (&str, i64),
) -> Result<RoundEnd, crate::error::ApiError> {
    let date = day.to_string();
    let pending = pending_assets(state, cfg, &date).await?;
    let depth = serde_json::to_value(cfg.depth).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    let run_id =
        repo::start_run(
            &state.db,
            repo::NewRun {
                local_date: &date,
                scheduled_for: due.timestamp_millis(),
                started_at: clock.now().timestamp_millis(),
                trigger,
                depth: &depth,
                planned: pending.len(),
                power: power.0,
                power_wait_secs: power.1,
            },
        )
        .await?;
    tracing::info!(run_id, trigger, planned = pending.len(), "lista de seguimiento: empieza la pasada");

    let mut transient_streak = 0;
    let mut aborted = false;
    for id in &pending {
        let started = clock.now().timestamp_millis();
        if aborted {
            repo::add_result(
                &state.db,
                NewResult {
                    run_id, local_date: &date, asset: id, status: "skipped", transient: true, attempts: 0,
                    analysis_id: None, verdict: None, error_code: Some("aborted"),
                    error: Some("pasada abortada por fallos seguidos; se reintenta más tarde"),
                    started_at: started, finished_at: started,
                },
            )
            .await?;
            continue;
        }
        match analyze_with_retries(state, cfg, clock, id).await {
            Attempt::Shutdown => {
                repo::finish_run(&state.db, run_id, "interrupted", clock.now().timestamp_millis()).await?;
                return Ok(RoundEnd::Shutdown);
            }
            Attempt::Ok(a, attempts) => {
                transient_streak = 0;
                let verdict = serde_json::to_value(a.verdict).ok().and_then(|v| v.as_str().map(str::to_owned));
                repo::set_run_model(&state.db, run_id, &a.model).await?;
                repo::add_result(
                    &state.db,
                    NewResult {
                        run_id, local_date: &date, asset: id, status: "ok", transient: false, attempts,
                        analysis_id: Some(&a.id), verdict: verdict.as_deref(), error_code: None, error: None,
                        started_at: started, finished_at: clock.now().timestamp_millis(),
                    },
                )
                .await?;
            }
            Attempt::Failed { error, attempts } => {
                let transient = is_transient(&error);
                transient_streak = if transient { transient_streak + 1 } else { 0 };
                repo::add_result(
                    &state.db,
                    NewResult {
                        run_id, local_date: &date, asset: id, status: "failed", transient, attempts,
                        analysis_id: None, verdict: None, error_code: Some(error.code()), error: Some(&error.to_string()),
                        started_at: started, finished_at: clock.now().timestamp_millis(),
                    },
                )
                .await?;
                if transient_streak >= cfg.retry.abort_after {
                    tracing::warn!(run_id, "lista de seguimiento: fallos seguidos, se aborta la pasada");
                    aborted = true;
                }
            }
        }
    }
    let status = if aborted { "aborted" } else { "completed" };
    repo::finish_run(&state.db, run_id, status, clock.now().timestamp_millis()).await?;
    tracing::info!(run_id, status, "lista de seguimiento: pasada terminada");
    Ok(RoundEnd::Finished)
}

// ---------------------------------------------------------------------------------------------
// Ciclo de aprendizaje periódico
// ---------------------------------------------------------------------------------------------

/// Cada cuánto resuelve resultados maduros y evalúa la puerta de pesos.
pub const LEARNING_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Retardo tras arrancar (no compite con el arranque ni con el primer análisis).
pub const LEARNING_FIRST_DELAY: Duration = Duration::from_secs(120);

/// Instala los pesos aprendidos vigentes. El aprendizaje es opcional: ante cualquier fallo (tabla
/// ilegible, consulta caída) se registra y se siguen usando los pesos por defecto; nunca impide arrancar.
pub async fn activate_learned_weights(db: &sqlx::SqlitePool) -> Option<learning::WeightVersion> {
    match learning::activate(db).await {
        Ok(Some(v)) => {
            tracing::info!(version = v.version, kind = %v.kind, "pesos aprendidos activos");
            Some(v)
        }
        Ok(None) => {
            tracing::info!("pesos de puntuación por defecto (sin versión aprendida)");
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "no se pudieron activar los pesos aprendidos: se usan los de por defecto");
            ai::scoring::set_learned_multipliers(None);
            None
        }
    }
}

/// Un fallo (o un panic) de un ciclo se registra y se reintenta en el siguiente: nunca mata el bucle.
pub async fn run_learning_loop(state: AppState, every: Duration, first_delay: Duration) {
    let mut delay = first_delay;
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
        delay = every;
        let cycle = async {
            let model = state.models.list().await.active;
            let cfg = learning::LearningConfig { active_model: Some(model), ..Default::default() };
            learning::run_cycle(&state.db, state.market.as_ref(), Utc::now(), &cfg).await
        };
        match std::panic::AssertUnwindSafe(cycle).catch_unwind().await {
            Ok(Ok(r)) => tracing::info!(resueltos = r.resolve.resolved, "aprendizaje: ciclo completado"),
            Ok(Err(e)) => tracing::error!(error = %e, "aprendizaje: el ciclo falló; se reintenta en el siguiente"),
            Err(_) => tracing::error!("aprendizaje: el ciclo hizo panic; se reintenta en el siguiente"),
        }
    }
}
