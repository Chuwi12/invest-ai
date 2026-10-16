//! Registro de las pasadas de la lista de seguimiento (`watchlist_runs` / `watchlist_results`).

use std::collections::HashMap;

use domain::AssetId;
use serde::{Serialize, Serializer};
use sqlx::SqlitePool;

use crate::error::ApiResult;

/// Las columnas guardan ms UTC; la API las entrega como fecha ISO-8601 (como el resto del contrato).
fn ser_ms<S: Serializer>(ms: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crate::db::dt_from_ms(*ms).to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn ser_ms_opt<S: Serializer>(ms: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match ms {
        Some(ms) => ser_ms(ms, s),
        None => s.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub id: i64,
    pub local_date: String,
    /// Cuándo tocaba ejecutarla según WATCHLIST_TIME.
    #[serde(serialize_with = "ser_ms")]
    pub scheduled_for: i64,
    /// Cuándo empezó de verdad (si es muy posterior a `scheduledFor`, fue una recuperación).
    #[serde(serialize_with = "ser_ms")]
    pub started_at: i64,
    #[serde(serialize_with = "ser_ms_opt")]
    pub finished_at: Option<i64>,
    pub trigger: String,
    pub status: String,
    pub depth: String,
    pub model: Option<String>,
    pub planned: i64,
    /// `ac`, `battery` o `unknown` al empezar la pasada.
    pub power: String,
    /// Segundos que se aplazó la pasada esperando a que se enchufara el equipo.
    pub power_wait_secs: i64,
    pub ok: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ResultRow {
    pub id: i64,
    pub run_id: i64,
    pub local_date: String,
    pub asset_id: String,
    pub status: String,
    pub transient: bool,
    pub attempts: i64,
    pub analysis_id: Option<String>,
    pub verdict: Option<String>,
    pub error_code: Option<String>,
    pub error: Option<String>,
    #[serde(serialize_with = "ser_ms")]
    pub started_at: i64,
    #[serde(serialize_with = "ser_ms")]
    pub finished_at: i64,
}

pub struct NewResult<'a> {
    pub run_id: i64,
    pub local_date: &'a str,
    pub asset: &'a AssetId,
    pub status: &'a str,
    pub transient: bool,
    pub attempts: u32,
    pub analysis_id: Option<&'a str>,
    pub verdict: Option<&'a str>,
    pub error_code: Option<&'a str>,
    pub error: Option<&'a str>,
    pub started_at: i64,
    pub finished_at: i64,
}

const RUN_COLS: &str =
    "id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, model, planned, power, power_wait_secs, ok, failed";

pub struct NewRun<'a> {
    pub local_date: &'a str,
    pub scheduled_for: i64,
    pub started_at: i64,
    pub trigger: &'a str,
    pub depth: &'a str,
    pub planned: usize,
    pub power: &'a str,
    pub power_wait_secs: i64,
}

pub async fn start_run(db: &SqlitePool, r: NewRun<'_>) -> ApiResult<i64> {
    let res = sqlx::query(
        "INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned, power, power_wait_secs)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7, ?8)",
    )
    .bind(r.local_date)
    .bind(r.scheduled_for)
    .bind(r.started_at)
    .bind(r.trigger)
    .bind(r.depth)
    .bind(r.planned as i64)
    .bind(r.power)
    .bind(r.power_wait_secs)
    .execute(db)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn set_run_model(db: &SqlitePool, run_id: i64, model: &str) -> ApiResult<()> {
    sqlx::query("UPDATE watchlist_runs SET model = ?2 WHERE id = ?1 AND model IS NULL")
        .bind(run_id)
        .bind(model)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn finish_run(db: &SqlitePool, run_id: i64, status: &str, finished_at: i64) -> ApiResult<()> {
    sqlx::query(
        "UPDATE watchlist_runs SET status = ?2, finished_at = ?3,
            ok = (SELECT COUNT(*) FROM watchlist_results WHERE run_id = ?1 AND status = 'ok'),
            failed = (SELECT COUNT(*) FROM watchlist_results WHERE run_id = ?1 AND status <> 'ok')
          WHERE id = ?1",
    )
    .bind(run_id)
    .bind(status)
    .bind(finished_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Al arrancar: una pasada que quedó `running` es de una ejecución anterior que cayó o se apagó.
pub async fn mark_interrupted(db: &SqlitePool, now_ms: i64) -> ApiResult<u64> {
    let res = sqlx::query(
        "UPDATE watchlist_runs SET status = 'interrupted', finished_at = ?1,
            ok = (SELECT COUNT(*) FROM watchlist_results r WHERE r.run_id = watchlist_runs.id AND r.status = 'ok'),
            failed = (SELECT COUNT(*) FROM watchlist_results r WHERE r.run_id = watchlist_runs.id AND r.status <> 'ok')
          WHERE status = 'running'",
    )
    .bind(now_ms)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

pub async fn add_result(db: &SqlitePool, r: NewResult<'_>) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO watchlist_results (run_id, local_date, asset_id, status, transient, attempts, analysis_id,
             verdict, error_code, error, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(r.run_id)
    .bind(r.local_date)
    .bind(r.asset.to_string())
    .bind(r.status)
    .bind(r.transient)
    .bind(i64::from(r.attempts))
    .bind(r.analysis_id)
    .bind(r.verdict)
    .bind(r.error_code)
    .bind(r.error)
    .bind(r.started_at)
    .bind(r.finished_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Estado de cada activo en un día local, según sus resultados más recientes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayState {
    /// Ya tiene un análisis válido ese día.
    Done,
    /// Falló de forma permanente (activo inexistente…): no se reintenta ese día.
    GaveUp,
    /// Sin intento, o el último fue un fallo transitorio / no se intentó (pasada abortada).
    Pending,
}

pub async fn day_states(db: &SqlitePool, local_date: &str) -> ApiResult<HashMap<String, DayState>> {
    let rows: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT asset_id, status, transient FROM watchlist_results WHERE local_date = ?1 ORDER BY id",
    )
    .bind(local_date)
    .fetch_all(db)
    .await?;
    let mut out: HashMap<String, DayState> = HashMap::new();
    for (asset, status, transient) in rows {
        let prev = out.get(&asset).copied();
        let next = match (status.as_str(), transient) {
            ("ok", _) => DayState::Done,
            _ if prev == Some(DayState::Done) => DayState::Done,
            (_, false) => DayState::GaveUp,
            _ => DayState::Pending,
        };
        out.insert(asset, next);
    }
    Ok(out)
}

pub async fn last_run(db: &SqlitePool) -> ApiResult<Option<RunRow>> {
    Ok(sqlx::query_as(&format!("SELECT {RUN_COLS} FROM watchlist_runs ORDER BY id DESC LIMIT 1"))
        .fetch_optional(db)
        .await?)
}

pub async fn running_run(db: &SqlitePool) -> ApiResult<Option<RunRow>> {
    Ok(sqlx::query_as(&format!("SELECT {RUN_COLS} FROM watchlist_runs WHERE status = 'running' ORDER BY id DESC LIMIT 1"))
        .fetch_optional(db)
        .await?)
}

pub async fn recent_runs(db: &SqlitePool, limit: i64) -> ApiResult<Vec<RunRow>> {
    Ok(sqlx::query_as(&format!("SELECT {RUN_COLS} FROM watchlist_runs ORDER BY id DESC LIMIT ?1"))
        .bind(limit)
        .fetch_all(db)
        .await?)
}

/// El último resultado de cada activo (de cualquier pasada).
pub async fn last_result_per_asset(db: &SqlitePool) -> ApiResult<Vec<ResultRow>> {
    Ok(sqlx::query_as(
        "SELECT id, run_id, local_date, asset_id, status, transient, attempts, analysis_id, verdict,
                error_code, error, started_at, finished_at
           FROM watchlist_results r
          WHERE id = (SELECT MAX(id) FROM watchlist_results WHERE asset_id = r.asset_id)",
    )
    .fetch_all(db)
    .await?)
}

/// Último resultado `ok` de cada activo: fecha (ms) del último análisis válido.
pub async fn last_ok_per_asset(db: &SqlitePool) -> ApiResult<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT asset_id, MAX(finished_at) FROM watchlist_results WHERE status = 'ok' GROUP BY asset_id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

pub async fn results_of_run(db: &SqlitePool, run_id: i64) -> ApiResult<Vec<ResultRow>> {
    Ok(sqlx::query_as(
        "SELECT id, run_id, local_date, asset_id, status, transient, attempts, analysis_id, verdict,
                error_code, error, started_at, finished_at
           FROM watchlist_results WHERE run_id = ?1 ORDER BY id",
    )
    .bind(run_id)
    .fetch_all(db)
    .await?)
}

pub async fn count_watchlist_analyses(db: &SqlitePool) -> ApiResult<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM analyses WHERE origin = 'watchlist'")
        .fetch_one(db)
        .await?;
    Ok(n)
}
