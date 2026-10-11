//! Historial de búsquedas y activos más vistos.

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    error::{ApiError, ApiResult},
    extract::AppJson,
    models::{RecordSearch, SearchEntry, ViewStat},
    repo::history as repo,
    state::AppState,
};

const SEARCHES_LIMIT: i64 = 50;
const VIEWS_LIMIT: usize = 20;

pub async fn searches(State(state): State<AppState>) -> ApiResult<Json<Vec<SearchEntry>>> {
    Ok(Json(repo::recent_searches(&state.db, SEARCHES_LIMIT).await?))
}

pub async fn clear_searches(State(state): State<AppState>) -> ApiResult<StatusCode> {
    repo::clear_searches(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// El usuario eligió un resultado de una búsqueda.
pub async fn record_selection(
    State(state): State<AppState>,
    AppJson(body): AppJson<RecordSearch>,
) -> ApiResult<StatusCode> {
    let query = body.query.trim();
    if query.is_empty() || query.chars().count() > 100 {
        return Err(ApiError::invalid("query debe tener entre 1 y 100 caracteres"));
    }
    repo::record_selection(&state.db, query, &body.selected).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn views(State(state): State<AppState>) -> ApiResult<Json<Vec<ViewStat>>> {
    Ok(Json(repo::most_viewed(&state.db, VIEWS_LIMIT).await?))
}
