//! Acceso SQLite del crate (consultas en tiempo de ejecución, sin macros que necesiten BD al compilar).

use domain::{AssetClass, ScoreBreakdown, Verdict};
use sqlx::SqlitePool;

use crate::{LearningError, rules};

pub const N_FACTORS: usize = 7;
/// Orden de los factores = el de `ai::scoring::default_weights`.
pub const FACTOR_NAMES: [&str; N_FACTORS] = [
    "technical",
    "momentum",
    "valuation",
    "fundamental",
    "sentiment",
    "macro",
    "risk",
];

/// Factores en orientación «favorable» (riesgo invertido: 100 − riesgo).
pub type Factors = [Option<f64>; N_FACTORS];

pub fn factors_of(s: &ScoreBreakdown) -> Factors {
    let f = |v: Option<f32>| v.map(f64::from).filter(|x| x.is_finite());
    [
        f(s.technical),
        f(s.momentum),
        f(s.valuation),
        f(s.fundamental),
        f(s.sentiment),
        f(s.macro_score),
        f(s.risk).map(|r| 100.0 - r),
    ]
}

/// Fila de `analysis_outcomes` ya resuelta, con lo que necesitan las estadísticas.
#[derive(Debug, Clone)]
pub struct OutcomeRow {
    pub analysis_id: String,
    pub horizon_weeks: u32,
    pub asset_id: String,
    pub class: AssetClass,
    pub verdict: Verdict,
    pub confidence: f64,
    pub model: String,
    pub generated_at_ms: i64,
    pub bench_kind: String,
    pub excess: f64,
    pub hit: bool,
    pub factors: Factors,
}

#[derive(sqlx::FromRow)]
struct RawOutcome {
    analysis_id: String,
    horizon_weeks: i64,
    asset_id: String,
    asset_class: String,
    verdict: String,
    confidence: f64,
    model: String,
    generated_at: i64,
    bench_kind: Option<String>,
    excess_return: Option<f64>,
    hit: Option<i64>,
    factors: Option<String>,
}

/// Filas resueltas con la regla vigente, de un horizonte, en orden cronológico. Incluye, con su marca
/// (`fallback_for`), las que no se pudieron recalcular y conservan las cifras de la versión anterior: sin
/// ellas, el activo deslistado cuya pérdida ya estaba medida desaparecería de las métricas. Solo esas:
/// cualquier otra fila de una versión anterior queda fuera.
pub async fn resolved_rows(
    db: &SqlitePool,
    horizon_weeks: u32,
) -> Result<Vec<OutcomeRow>, LearningError> {
    let raw: Vec<RawOutcome> = sqlx::query_as(
        "SELECT analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model, generated_at,
                bench_kind, excess_return, hit, factors
           FROM analysis_outcomes
          WHERE status = 'resolved' AND (rule_version = ?1 OR fallback_for = ?1) AND horizon_weeks = ?2
          ORDER BY generated_at, analysis_id",
    )
    .bind(rules::RULE_VERSION)
    .bind(i64::from(horizon_weeks))
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        let (Some(excess), Some(hit), Some(verdict), Ok(class)) = (
            r.excess_return,
            r.hit,
            rules::parse_verdict(&r.verdict),
            r.asset_class.parse::<AssetClass>(),
        ) else {
            continue;
        };
        let factors: Factors = r
            .factors
            .and_then(|j| serde_json::from_str::<Vec<Option<f64>>>(&j).ok())
            .and_then(|v| <[Option<f64>; N_FACTORS]>::try_from(v).ok())
            .unwrap_or([None; N_FACTORS]);
        out.push(OutcomeRow {
            analysis_id: r.analysis_id,
            horizon_weeks: r.horizon_weeks as u32,
            asset_id: r.asset_id,
            class,
            verdict,
            confidence: r.confidence,
            model: r.model,
            generated_at_ms: r.generated_at,
            bench_kind: r.bench_kind.unwrap_or_default(),
            excess,
            hit: hit == 1,
            factors,
        });
    }
    Ok(out)
}

/// Deja una fila por activo y ventana de `horizon` semanas (la más antigua): las ventanas que se
/// solapan del mismo activo comparten casi todo su resultado y contarlas inflaría la muestra.
pub fn independent<T>(
    rows: &[T],
    horizon_weeks: u32,
    key: impl Fn(&T) -> (&str, i64),
) -> Vec<usize> {
    let stride = i64::from(horizon_weeks) * 7 * 86_400_000;
    let mut last: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut keep = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let (asset, t) = key(r);
        if last.get(asset).is_none_or(|prev| t - prev >= stride) {
            last.insert(asset, t);
            keep.push(i);
        }
    }
    keep
}
