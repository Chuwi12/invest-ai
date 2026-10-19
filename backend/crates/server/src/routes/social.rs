//! Redes sociales (solo lectura): estado de la integración y métricas/extractos por activo.
//! Es OPINIÓN de usuarios anónimos, no hechos, y no entra en la puntuación (`docs/notes/social.md`).

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{
    error::ApiResult,
    extract::AssetPath,
    resolve::resolve_summary_lenient,
    social::disabled_status,
    state::AppState,
};

const DISCLAIMER_ES: &str = "Opinión de usuarios anónimos de Reddit y X: no verificada y posiblemente manipulada. Mide atención y tono, no hechos ni cifras.";

/// `GET /api/social/status`: activado, presupuesto de X (usado/restante), coste estimado y último error.
pub async fn status(State(state): State<AppState>) -> Json<Value> {
    match &state.social {
        Some(s) => Json(s.status().await),
        None => Json(disabled_status(&market::social::SocialConfig { notes: state.social_notes.to_vec(), ..Default::default() })),
    }
}

/// `GET /api/assets/{id}/social`: métricas y extractos. Consulta las redes solo si lo guardado
/// tiene más de `SOCIAL_REFRESH_MINUTES`; un fallo de redes nunca devuelve un error (`digest: null`).
pub async fn asset_social(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<Json<Value>> {
    let Some(social) = &state.social else {
        return Ok(Json(json!({ "enabled": false, "digest": null, "disclaimer": DISCLAIMER_ES })));
    };
    let summary = resolve_summary_lenient(&state, &id).await?;
    let digest = match social.digest_for(&summary).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(%id, error = %e, "redes sociales no disponibles");
            None
        }
    };
    Ok(Json(json!({ "enabled": true, "digest": digest, "disclaimer": DISCLAIMER_ES })))
}
