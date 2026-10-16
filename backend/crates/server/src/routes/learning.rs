//! Rendimiento medido, pesos aprendidos y confianza calibrada (crate `learning`).

use axum::{Json, extract::State};
use chrono::Utc;
use learning::{CalibratedConfidence, CalibrationConfig, LearningConfig, LearningError, PerformanceReport, WeightVersion};
use serde::{Deserialize, Serialize};

use crate::{
    error::{ApiError, ApiResult},
    extract::{AppJson, AppQuery},
    state::AppState,
};

impl From<LearningError> for ApiError {
    fn from(e: LearningError) -> Self {
        match e {
            LearningError::NoSuchVersion(_) => ApiError::not_found(e.to_string()),
            other => {
                tracing::error!(error = %other, "error del crate learning");
                ApiError::internal("error en el módulo de aprendizaje")
            }
        }
    }
}

/// La calibración solo mira los análisis del modelo activo.
async fn active_model(state: &AppState) -> String {
    state.models.list().await.active
}

pub async fn performance(State(state): State<AppState>) -> ApiResult<Json<PerformanceReport>> {
    let model = active_model(&state).await;
    Ok(Json(learning::performance_report(&state.db, Utc::now(), Some(&model), &CalibrationConfig::default()).await?))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightsResponse {
    pub active: Option<WeightVersion>,
    /// Todas las versiones, la más reciente primero.
    pub versions: Vec<WeightVersion>,
}

pub async fn weights(State(state): State<AppState>) -> ApiResult<Json<WeightsResponse>> {
    let versions = learning::weights::list_versions(&state.db).await?;
    let active = versions.iter().find(|v| v.status == "active").cloned();
    Ok(Json(WeightsResponse { active, versions }))
}

#[derive(Deserialize)]
pub struct RollbackBody {
    /// Versión a restaurar; `null` = pesos por defecto.
    pub version: Option<i64>,
}

pub async fn rollback(State(state): State<AppState>, AppJson(body): AppJson<RollbackBody>) -> ApiResult<Json<WeightVersion>> {
    Ok(Json(learning::rollback(&state.db, body.version, Utc::now()).await?))
}

#[derive(Deserialize)]
pub struct ConfidenceQuery {
    /// Confianza declarada (0–1).
    pub confidence: f64,
    #[serde(default = "default_horizon")]
    pub horizon: u32,
    /// Modelo con cuyo historial se calibra (el que hizo el análisis). Sin él, el modelo activo.
    #[serde(default)]
    pub model: Option<String>,
}

fn default_horizon() -> u32 {
    4
}

pub async fn calibrated_confidence(
    State(state): State<AppState>,
    AppQuery(q): AppQuery<ConfidenceQuery>,
) -> ApiResult<Json<CalibratedConfidence>> {
    if !(0.0..=1.0).contains(&q.confidence) {
        return Err(ApiError::invalid("confidence debe estar entre 0 y 1"));
    }
    if !learning::rules::HORIZONS_WEEKS.contains(&q.horizon) {
        return Err(ApiError::invalid("horizon debe ser 1, 4 o 12 (semanas)"));
    }
    let model = match q.model {
        Some(m) if m.trim().is_empty() => return Err(ApiError::invalid("model no puede estar vacío")),
        Some(m) => m,
        None => active_model(&state).await,
    };
    let cfg = LearningConfig { active_model: Some(model), ..Default::default() };
    Ok(Json(learning::calibrated_confidence(&state.db, q.confidence, q.horizon, &cfg).await?))
}
