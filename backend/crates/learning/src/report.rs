//! Panel de rendimiento (datos): métricas agregadas listas para mostrar.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    LearningError,
    calibration::{self, CalPoint, CalibrationConfig, CalibrationStatus},
    rules::{self, HORIZONS_WEEKS, RULE_VERSION},
    stats::{self, n_clusters, wilson},
    store::{self, OutcomeRow},
    weights::{self, WeightVersion},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictStats {
    pub verdict: String,
    pub n: usize,
    pub hits: usize,
    pub hit_rate: Option<f64>,
    /// Wilson 95 % sobre la muestra independiente (conservador frente a re-análisis).
    pub hit_rate_ci: Option<(f64, f64)>,
    pub mean_excess: Option<f64>,
    /// Bootstrap por bloques temporales 95 %; `None` con menos de 5 bloques.
    pub mean_excess_ci: Option<(f64, f64)>,
    pub clusters: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HorizonReport {
    pub horizon_weeks: u32,
    pub analyses_total: i64,
    pub resolved: i64,
    /// De los `resolved`, filas que usan la entrada de la regla anterior porque no hay histórico para
    /// recalcularlas (activo deslistado o que el proveedor ya no sirve). Cuentan en todas las métricas.
    pub fallback_rows: i64,
    /// Resueltos tras quitar solapes del mismo activo.
    pub resolved_independent: usize,
    /// Han madurado pero aún no tienen resultado (por resolver, reintento o sin datos).
    pub pending: i64,
    /// Aún no ha llegado su fecha.
    pub not_matured: i64,
    pub skipped: i64,
    /// Proporción de análisis cuyo activo batió a su índice (tasa base para leer los aciertos).
    pub base_rate_outperform: Option<f64>,
    pub overall: VerdictStats,
    pub by_verdict: Vec<VerdictStats>,
    pub calibration: CalibrationStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceReport {
    pub generated_at: DateTime<Utc>,
    pub rule_version: i64,
    pub analyses_total: i64,
    pub horizons: Vec<HorizonReport>,
    pub active_weights: Option<WeightVersion>,
    pub weight_history: Vec<WeightVersion>,
    /// Advertencias honestas sobre la lectura de las cifras.
    pub caveats: Vec<String>,
}

fn stats_for(
    verdict: &str,
    rows: &[&OutcomeRow],
    independent: &[&OutcomeRow],
    h: u32,
) -> VerdictStats {
    let n = rows.len();
    let hits = rows.iter().filter(|r| r.hit).count();
    let clusters: Vec<u64> = rows
        .iter()
        .map(|r| (r.generated_at_ms / (i64::from(h) * 7 * 86_400_000)) as u64)
        .collect();
    let ex: Vec<f64> = rows.iter().map(|r| r.excess).collect();
    let ind_hits = independent.iter().filter(|r| r.hit).count();
    VerdictStats {
        verdict: verdict.into(),
        n,
        hits,
        hit_rate: (n > 0).then(|| hits as f64 / n as f64),
        hit_rate_ci: wilson(ind_hits, independent.len()),
        mean_excess: (n > 0).then(|| ex.iter().sum::<f64>() / n as f64),
        mean_excess_ci: stats::mean_ci(&ex, &clusters, 1000, 7),
        clusters: n_clusters(&clusters),
    }
}

/// Estadísticas de un horizonte a partir de filas resueltas (cronológicas).
pub fn horizon_stats(
    rows: &[OutcomeRow],
    h: u32,
    model: Option<&str>,
    cal: &CalibrationConfig,
) -> (
    VerdictStats,
    Vec<VerdictStats>,
    usize,
    CalibrationStatus,
    Option<f64>,
) {
    let keep = store::independent(rows, h, |r| (r.asset_id.as_str(), r.generated_at_ms));
    let ind_set: std::collections::HashSet<usize> = keep.iter().copied().collect();
    let all: Vec<&OutcomeRow> = rows.iter().collect();
    let ind: Vec<&OutcomeRow> = keep.iter().map(|&i| &rows[i]).collect();
    let overall = stats_for("all", &all, &ind, h);
    let mut groups: BTreeMap<&str, (Vec<&OutcomeRow>, Vec<&OutcomeRow>)> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        let e = groups.entry(rules::verdict_str(r.verdict)).or_default();
        e.0.push(r);
        if ind_set.contains(&i) {
            e.1.push(r);
        }
    }
    let order = ["buy", "accumulate", "hold", "wait", "reduce", "avoid"];
    let mut by: Vec<VerdictStats> = groups
        .iter()
        .map(|(v, (a, b))| stats_for(v, a, b, h))
        .collect();
    by.sort_by_key(|s| order.iter().position(|o| *o == s.verdict).unwrap_or(99));
    let base = (!rows.is_empty())
        .then(|| rows.iter().filter(|r| r.excess > 0.0).count() as f64 / rows.len() as f64);
    // Calibración: solo el modelo indicado (la confianza de otro modelo significa otra cosa).
    let cal_rows: Vec<&OutcomeRow> = rows
        .iter()
        .filter(|r| model.is_none_or(|m| r.model == m))
        .collect();
    let keep_cal = store::independent(&cal_rows, h, |r| (r.asset_id.as_str(), r.generated_at_ms));
    let pts: Vec<CalPoint> = keep_cal
        .iter()
        .map(|&i| CalPoint {
            confidence: cal_rows[i].confidence,
            hit: cal_rows[i].hit,
            t_ms: cal_rows[i].generated_at_ms,
        })
        .collect();
    let status = calibration::assess(&pts, h, cal);
    (overall, by, keep.len(), status, base)
}

/// Informe completo. `model`: modelo activo para la calibración (`None` = todos mezclados).
pub async fn performance_report(
    db: &SqlitePool,
    now: DateTime<Utc>,
    model: Option<&str>,
    cal: &CalibrationConfig,
) -> Result<PerformanceReport, LearningError> {
    let now_ms = now.timestamp_millis();
    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM analyses")
        .fetch_one(db)
        .await?;
    let mut horizons = Vec::new();
    for h in HORIZONS_WEEKS {
        let mature_cut = now_ms - (i64::from(h) * 7 + 1) * 86_400_000;
        let (matured,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM analyses WHERE generated_at <= ?1")
                .bind(mature_cut)
                .fetch_one(db)
                .await?;
        // Mismo criterio que `store::resolved_rows`: la versión vigente y, marcada, la vía de reserva.
        let (resolved, skipped, fallback_rows): (i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(status='resolved' AND (rule_version = ?2 OR fallback_for = ?2)),0),
                    COALESCE(SUM(status='skipped' AND rule_version = ?2),0),
                    COALESCE(SUM(status='resolved' AND fallback_for = ?2),0)
               FROM analysis_outcomes WHERE horizon_weeks = ?1",
        )
        .bind(i64::from(h))
        .bind(RULE_VERSION)
        .fetch_one(db)
        .await?;
        let rows = store::resolved_rows(db, h).await?;
        let (overall, by_verdict, independent, calibration, base) =
            horizon_stats(&rows, h, model, cal);
        horizons.push(HorizonReport {
            horizon_weeks: h,
            analyses_total: total,
            resolved,
            fallback_rows,
            resolved_independent: independent,
            pending: (matured - resolved - skipped).max(0),
            not_matured: total - matured,
            skipped,
            base_rate_outperform: base,
            overall,
            by_verdict,
            calibration,
        });
    }
    let mut caveats = vec![
        "Todos los activos comparten el mismo régimen de mercado: la muestra efectiva es mucho menor que el nº de análisis.".to_string(),
        "Mantener/esperar acierta con una banda de ±2 %·√semanas: su tasa de acierto no es comparable con la de comprar/evitar sin mirar la tasa base.".into(),
        "Los activos que nunca llegaron a tener resultado (sin precio posterior, excluidos de bolsa) no entran: sesgo de supervivencia en el lado optimista.".into(),
    ];
    let fallback: i64 = horizons.iter().map(|h| h.fallback_rows).sum();
    if fallback > 0 {
        caveats.push(format!(
            "{fallback} resultados (entre todos los horizontes) usan la entrada de la regla anterior porque su activo ya no tiene histórico para recalcularlos: cuentan, pero su entrada no es la de la regla v{RULE_VERSION}."
        ));
    }
    Ok(PerformanceReport {
        generated_at: now,
        rule_version: RULE_VERSION,
        analyses_total: total,
        horizons,
        active_weights: weights::active_version(db).await?,
        weight_history: weights::list_versions(db).await?,
        caveats,
    })
}
