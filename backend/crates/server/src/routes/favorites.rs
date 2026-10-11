//! Favoritos.

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    error::{ApiError, ApiResult},
    extract::{AssetPath, OptionalJson},
    models::{Favorite, PutFavorite},
    repo::favorites as repo,
    resolve::resolve_summary_lenient,
    state::AppState,
};

const MAX_NOTE_CHARS: usize = 2_000;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<Favorite>>> {
    Ok(Json(repo::list(&state.db).await?))
}

/// Alta idempotente. Sin cuerpo (o sin `note`) conserva la nota que hubiera.
pub async fn put(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
    OptionalJson(body): OptionalJson<PutFavorite>,
) -> ApiResult<Json<Favorite>> {
    let note = match body.note {
        None => None,
        Some(None) => Some(None),
        Some(Some(n)) => {
            let n = n.trim().to_string();
            if n.chars().count() > MAX_NOTE_CHARS {
                return Err(ApiError::invalid(format!("note no puede superar {MAX_NOTE_CHARS} caracteres")));
            }
            Some((!n.is_empty()).then_some(n))
        }
    };
    let summary = resolve_summary_lenient(&state, &id).await?;
    repo::upsert(&state.db, &summary, note).await?;
    let favorite = repo::get(&state.db, &id)
        .await?
        .ok_or_else(|| ApiError::internal("el favorito recién guardado no existe"))?;
    Ok(Json(favorite))
}

/// Idempotente: quitar algo que no era favorito también es 204.
pub async fn delete(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<StatusCode> {
    repo::delete(&state.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
