//! Aprendizaje por resultados: resolución de resultados de los análisis, calibración de la
//! confianza, pesos de la puntuación aprendidos con puerta de validación y datos del panel de
//! rendimiento. No reentrena el LLM. Ver `docs/notes/learning.md`.

pub mod calibration;
pub mod report;
pub mod resolve;
pub mod rules;
pub mod stats;
pub mod store;
pub mod weights;

#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod synthetic_tests;

use chrono::{DateTime, Utc};
use domain::MarketData;
use serde::Serialize;
use sqlx::SqlitePool;

pub use calibration::{CalibrationConfig, CalibrationStatus, Calibrator};
pub use report::{PerformanceReport, performance_report};
pub use resolve::{ResolveConfig, ResolveReport, resolve_pending};
pub use weights::{WeightVersion, WeightsConfig, activate, rollback};

#[derive(Debug, thiserror::Error)]
pub enum LearningError {
    #[error("base de datos: {0}")]
    Db(#[from] sqlx::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("la versión de pesos {0} no existe")]
    NoSuchVersion(i64),
}

#[derive(Debug, Clone, Default)]
pub struct LearningConfig {
    pub resolve: ResolveConfig,
    pub weights: WeightsConfig,
    pub calibration: CalibrationConfig,
    /// Modelo activo (la calibración solo mira sus análisis).
    pub active_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleReport {
    pub resolve: ResolveReport,
    pub weights: weights::WeightsCycle,
}

/// Ciclo completo (pensado para llamarlo cada pocas horas): resolver resultados maduros y
/// evaluar la puerta de pesos. Barato: sin análisis maduros no hay peticiones de red.
pub async fn run_cycle(
    db: &SqlitePool,
    market: &dyn MarketData,
    now: DateTime<Utc>,
    cfg: &LearningConfig,
) -> Result<CycleReport, LearningError> {
    let resolve = resolve_pending(db, market, now, &cfg.resolve).await?;
    let weights = weights::run_weights_cycle(db, now, &cfg.weights).await?;
    Ok(CycleReport { resolve, weights })
}

/// Confianza declarada junto a la calibrada (si el calibrador está aplicado para ese horizonte).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibratedConfidence {
    pub raw: f64,
    pub calibrated: Option<f64>,
    pub horizon_weeks: u32,
    pub n: usize,
    pub min_required: usize,
    pub reason: String,
}

pub async fn calibrated_confidence(
    db: &SqlitePool,
    raw: f64,
    horizon_weeks: u32,
    cfg: &LearningConfig,
) -> Result<CalibratedConfidence, LearningError> {
    let rows = store::resolved_rows(db, horizon_weeks).await?;
    let (_, _, _, st, _) = report::horizon_stats(
        &rows,
        horizon_weeks,
        cfg.active_model.as_deref(),
        &cfg.calibration,
    );
    Ok(CalibratedConfidence {
        raw,
        calibrated: st.calibrator.as_ref().map(|c| c.apply(raw)),
        horizon_weeks,
        n: st.n,
        min_required: st.min_required,
        reason: st.reason,
    })
}
