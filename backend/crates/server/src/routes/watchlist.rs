//! `GET /watchlist`: la lista, la próxima ejecución y la última con su resultado por activo.

use axum::{Json, extract::State};
use serde::Serialize;

use crate::{
    error::ApiResult,
    repo::watchlist::{self as repo, ResultRow, RunRow},
    state::AppState,
    watchlist::next_due,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistAsset {
    pub id: String,
    /// Último intento (de cualquier pasada); `None` si nunca se ha intentado.
    pub last: Option<ResultRow>,
    /// Fin del último análisis válido.
    pub last_ok_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistStatus {
    pub enabled: bool,
    /// `true` = lista por defecto (decidida a priori); `false` = la del usuario en `WATCHLIST`.
    pub default_list: bool,
    /// Hora local de la pasada diaria (`HH:MM`).
    pub time: String,
    pub depth: String,
    /// Próxima pasada programada; `None` si está desactivada.
    pub next_run: Option<chrono::DateTime<chrono::Utc>>,
    /// Hay una pasada en marcha ahora mismo.
    pub running: bool,
    pub assets: Vec<WatchlistAsset>,
    pub last_run: Option<RunRow>,
    /// Resultado por activo de la última pasada.
    pub last_run_results: Vec<ResultRow>,
    /// Pasadas recientes (la más reciente primero).
    pub recent_runs: Vec<RunRow>,
    /// Análisis guardados con origen `watchlist`.
    pub analyses_total: i64,
}

pub async fn status(State(state): State<AppState>) -> ApiResult<Json<WatchlistStatus>> {
    let cfg = &state.watchlist;
    let last_results = repo::last_result_per_asset(&state.db).await?;
    let last_ok = repo::last_ok_per_asset(&state.db).await?;
    let assets = cfg
        .assets
        .iter()
        .map(|a| {
            let id = a.to_string();
            WatchlistAsset {
                last: last_results.iter().find(|r| r.asset_id == id).cloned(),
                last_ok_at: last_ok.get(&id).map(|ms| crate::db::dt_from_ms(*ms)),
                id,
            }
        })
        .collect();
    let last_run = repo::last_run(&state.db).await?;
    let last_run_results = match &last_run {
        Some(r) => repo::results_of_run(&state.db, r.id).await?,
        None => Vec::new(),
    };
    let next_run = cfg.enabled.then(|| next_due(state.clock.as_ref(), state.clock.now(), cfg.time));
    Ok(Json(WatchlistStatus {
        enabled: cfg.enabled,
        default_list: cfg.default_list,
        time: cfg.time.format("%H:%M").to_string(),
        depth: serde_json::to_value(cfg.depth).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
        next_run,
        running: repo::running_run(&state.db).await?.is_some(),
        assets,
        last_run,
        last_run_results,
        recent_runs: repo::recent_runs(&state.db, 7).await?,
        analyses_total: repo::count_watchlist_analyses(&state.db).await?,
    }))
}
