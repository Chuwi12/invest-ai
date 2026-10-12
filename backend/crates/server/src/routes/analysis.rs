//! Análisis de IA por activo: el último guardado, generar uno nuevo, histórico y snapshot.

use std::{sync::Arc, time::Duration};

use axum::{Json, extract::State};
use domain::{Analysis, AssetId, AssetSnapshot, DomainError};

use crate::{
    db::new_id,
    error::ApiResult,
    extract::AssetPath,
    repo,
    resolve::cache_summaries,
    state::AppState,
};

const HISTORY_LIMIT: i64 = 20;
/// Tope de seguridad: el cliente HTTP del analista ya tiene su propio timeout (`LLM_TIMEOUT_SECS`).
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn latest(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<Option<Analysis>>> {
    Ok(Json(repo::analyses::latest(&state.db, &id).await?))
}

pub async fn history(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<Vec<Analysis>>> {
    Ok(Json(repo::analyses::list_for_asset(&state.db, &id, HISTORY_LIMIT).await?))
}

pub async fn snapshot(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<AssetSnapshot>> {
    let mut snapshot = state.market.snapshot(&id).await?;
    (state.annotate_news)(&mut snapshot.news);
    Ok(Json(snapshot))
}

/// Genera un análisis nuevo. Single-flight por activo: si ya hay uno en curso, espera a ese.
pub async fn generate(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<Json<Analysis>> {
    let job_state = state.clone();
    let job_id = id.clone();
    let (result, leader) = state
        .analysis_flights
        .run(id.clone(), move || async move { run_analysis(job_state, job_id).await.map(Arc::new) })
        .await;
    if !leader {
        tracing::debug!(%id, "análisis ya en curso: se comparte el resultado");
    }
    let analysis = result?;
    Ok(Json(Analysis::clone(&analysis)))
}

async fn run_analysis(state: AppState, id: AssetId) -> Result<Analysis, DomainError> {
    let snapshot = state.market.snapshot(&id).await?;
    cache_summaries(&state, std::slice::from_ref(&snapshot.summary)).await;

    let mut analysis = tokio::time::timeout(ANALYZE_TIMEOUT, state.analyst.analyze(&snapshot))
        .await
        .map_err(|_| DomainError::Unavailable("el análisis tardó demasiado".into()))??;
    if analysis.id.is_empty() {
        analysis.id = new_id();
    }
    // Si guardar falla, el análisis (caro de generar) se devuelve igualmente.
    if let Err(e) = repo::analyses::insert(&state.db, &analysis, "analyze").await {
        tracing::error!(%id, error = %e, "no se pudo guardar el análisis");
    }
    Ok(analysis)
}
