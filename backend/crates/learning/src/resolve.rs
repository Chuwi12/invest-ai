//! Resolución de resultados: para cada análisis guardado y horizonte maduro, rentabilidad del
//! activo, del índice, exceso y acierto. Idempotente (las filas `resolved`/`skipped` no se tocan),
//! reanudable (tope de activos por pasada; lo que falta se hace en la siguiente) y barato (cero
//! peticiones si nada ha madurado; una petición de histórico por activo y por benchmark).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use domain::{Analysis, AssetId, Candle, DomainError, Interval, MarketData, Range};
use sqlx::SqlitePool;

use crate::{
    LearningError,
    rules::{self, Computation, HORIZONS_WEEKS, RULE_VERSION},
    store::factors_of,
};

#[derive(Debug, Clone)]
pub struct ResolveConfig {
    pub horizons: Vec<u32>,
    /// Activos distintos por pasada (acota el número de peticiones).
    pub max_assets_per_run: usize,
    /// Intentos fallidos antes de dar una fila por `skipped`.
    pub max_attempts: i64,
    /// Margen tras la fecha objetivo antes de intentar (un día).
    pub buffer_secs: i64,
}

impl Default for ResolveConfig {
    fn default() -> Self {
        Self {
            horizons: HORIZONS_WEEKS.to_vec(),
            max_assets_per_run: 25,
            max_attempts: 6,
            buffer_secs: 86_400,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveReport {
    /// Pares (análisis, horizonte) maduros y sin resolver al empezar.
    pub pending_pairs: usize,
    pub resolved: usize,
    pub skipped: usize,
    pub retry: usize,
    /// Todavía sin cierre completo (no se escribe nada).
    pub not_yet: usize,
    pub history_calls: usize,
    /// Activos que quedan para la siguiente pasada por el tope.
    pub deferred_assets: usize,
}

/// Rango diario mínimo que cubre `days` días hacia atrás.
pub fn range_for_days(days: i64) -> Range {
    const STEPS: [(i64, Range); 7] = [
        (27, Range::M1),
        (85, Range::M3),
        (170, Range::M6),
        (355, Range::Y1),
        (715, Range::Y2),
        (1790, Range::Y5),
        (3600, Range::Y10),
    ];
    STEPS
        .iter()
        .find(|(d, _)| days <= *d)
        .map_or(Range::Max, |(_, r)| *r)
}

#[derive(sqlx::FromRow)]
struct AnalysisHead {
    id: String,
    asset_id: String,
    verdict: String,
    confidence: f64,
    model: String,
    generated_at: i64,
}

#[derive(Clone)]
struct Pair {
    head_idx: usize,
    horizon: u32,
    attempts: i64,
}

enum Written {
    Skipped,
    Retry,
}

/// Una pasada de resolución. `now` se inyecta para poder probar con el tiempo simulado.
pub async fn resolve_pending(
    db: &SqlitePool,
    market: &dyn MarketData,
    now: DateTime<Utc>,
    cfg: &ResolveConfig,
) -> Result<ResolveReport, LearningError> {
    let mut report = ResolveReport::default();
    let Some(min_h) = cfg.horizons.iter().min().copied() else {
        return Ok(report);
    };
    let now_ms = now.timestamp_millis();
    let week_ms = 7 * 86_400_000i64;
    let cut = now_ms - (i64::from(min_h) * week_ms + cfg.buffer_secs * 1000);

    let heads: Vec<AnalysisHead> = sqlx::query_as(
        "SELECT id, asset_id, verdict, confidence, model, generated_at FROM analyses
          WHERE generated_at <= ?1 ORDER BY generated_at, id",
    )
    .bind(cut)
    .fetch_all(db)
    .await?;
    if heads.is_empty() {
        return Ok(report);
    }

    // Estado ya escrito: definitivas, o a la espera de reintento.
    let existing: Vec<(String, i64, String, Option<i64>, i64)> = sqlx::query_as(
        "SELECT analysis_id, horizon_weeks, status, retry_after, attempts FROM analysis_outcomes",
    )
    .fetch_all(db)
    .await?;
    let mut state: HashMap<(String, i64), (String, Option<i64>, i64)> = HashMap::new();
    for (a, h, s, r, n) in existing {
        state.insert((a, h), (s, r, n));
    }

    let mut by_asset: BTreeMap<String, Vec<Pair>> = BTreeMap::new();
    for (i, h) in heads.iter().enumerate() {
        for &hw in &cfg.horizons {
            if h.generated_at + i64::from(hw) * week_ms + cfg.buffer_secs * 1000 > now_ms {
                continue;
            }
            let mut attempts = 0;
            if let Some((status, retry_after, n)) = state.get(&(h.id.clone(), i64::from(hw))) {
                if status != "retry" || retry_after.is_some_and(|r| r > now_ms) {
                    continue;
                }
                attempts = *n;
            }
            by_asset.entry(h.asset_id.clone()).or_default().push(Pair {
                head_idx: i,
                horizon: hw,
                attempts,
            });
        }
    }
    report.pending_pairs = by_asset.values().map(Vec::len).sum();
    if by_asset.is_empty() {
        return Ok(report);
    }

    // Los activos con trabajo más antiguo primero; el resto, a la siguiente pasada.
    let mut assets: Vec<(String, Vec<Pair>)> = by_asset.into_iter().collect();
    assets.sort_by_key(|(_, p)| p.iter().map(|x| heads[x.head_idx].generated_at).min());
    report.deferred_assets = assets.len().saturating_sub(cfg.max_assets_per_run);
    assets.truncate(cfg.max_assets_per_run);

    // Un histórico por activo distinto (los benchmarks y los activos que también lo son se piden
    // una sola vez), con el rango que cubre al análisis más antiguo de quienes lo usan.
    let mut need: HashMap<AssetId, i64> = HashMap::new();
    for (aid, pairs) in &assets {
        let Ok(id) = aid.parse::<AssetId>() else {
            continue;
        };
        let Some(bench) = rules::benchmark_for(&id) else {
            continue;
        };
        let oldest = pairs
            .iter()
            .map(|p| heads[p.head_idx].generated_at)
            .min()
            .unwrap_or(now_ms);
        let mut want = vec![id];
        want.extend(bench.id);
        for a in want {
            need.entry(a)
                .and_modify(|o| *o = (*o).min(oldest))
                .or_insert(oldest);
        }
    }
    let mut ordered: Vec<(AssetId, i64)> = need.into_iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let mut cache: HashMap<AssetId, Result<Vec<Candle>, DomainError>> = HashMap::new();
    for (a, oldest) in ordered {
        let days = (now_ms - oldest) / 86_400_000 + 10;
        report.history_calls += 1;
        let res = market.history(&a, range_for_days(days), Interval::D1).await;
        cache.insert(a, res.map(sorted));
    }

    for (aid, pairs) in assets {
        let parsed = aid.parse::<AssetId>();
        let bench = parsed.as_ref().ok().and_then(rules::benchmark_for);
        let (Ok(id), Some(bench)) = (parsed, bench) else {
            for p in &pairs {
                let h = &heads[p.head_idx];
                write_final(
                    db,
                    h,
                    p.horizon,
                    "skipped",
                    "sin benchmark definido para este activo",
                    now_ms,
                )
                .await?;
                report.skipped += 1;
            }
            continue;
        };
        let candles: &[Candle] = match cache.get(&id) {
            Some(Ok(c)) => c,
            Some(Err(DomainError::NotFound(m))) => {
                for p in &pairs {
                    let h = &heads[p.head_idx];
                    write_final(
                        db,
                        h,
                        p.horizon,
                        "skipped",
                        &format!("activo sin histórico: {m}"),
                        now_ms,
                    )
                    .await?;
                    report.skipped += 1;
                }
                continue;
            }
            other => {
                let why = match other {
                    Some(Err(e)) => e.to_string(),
                    _ => "histórico no disponible".into(),
                };
                for p in &pairs {
                    count(
                        &mut report,
                        write_retry(db, &heads[p.head_idx], p, &why, now_ms, cfg).await?,
                    );
                }
                continue;
            }
        };
        let bench_slice: Option<&[Candle]> = match &bench.id {
            None => None,
            Some(b) => match cache.get(b) {
                Some(Ok(c)) => Some(c.as_slice()),
                other => {
                    let why = match other {
                        Some(Err(e)) => format!("benchmark no disponible: {e}"),
                        _ => "benchmark no disponible".into(),
                    };
                    for p in &pairs {
                        count(
                            &mut report,
                            write_retry(db, &heads[p.head_idx], p, &why, now_ms, cfg).await?,
                        );
                    }
                    continue;
                }
            },
        };
        for p in &pairs {
            let h = &heads[p.head_idx];
            match rules::compute(
                candles,
                bench_slice,
                h.generated_at,
                p.horizon,
                now.timestamp(),
            ) {
                Computation::NotYet => report.not_yet += 1,
                Computation::NoData(why) => {
                    count(&mut report, write_retry(db, h, p, &why, now_ms, cfg).await?)
                }
                Computation::Resolved(r) => {
                    let (factors, overall) = analysis_factors(db, &h.id).await;
                    let verdict = rules::parse_verdict(&h.verdict);
                    let hit = verdict.map(|v| rules::is_hit(v, r.excess, p.horizon));
                    let class = id.class.as_str();
                    sqlx::query(
                        "INSERT INTO analysis_outcomes
                            (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model, generated_at,
                             status, note, attempts, retry_after, rule_version, bench_id, bench_kind,
                             entry_ts, exit_ts, entry_price, exit_price, bench_entry_price, bench_exit_price,
                             asset_return, bench_return, excess_return, hit, factors, overall, resolved_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'resolved',NULL,?9,NULL,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)
                         ON CONFLICT(analysis_id, horizon_weeks) DO UPDATE SET
                            status='resolved', note=NULL, retry_after=NULL, attempts=excluded.attempts,
                            rule_version=excluded.rule_version, bench_id=excluded.bench_id, bench_kind=excluded.bench_kind,
                            entry_ts=excluded.entry_ts, exit_ts=excluded.exit_ts, entry_price=excluded.entry_price,
                            exit_price=excluded.exit_price, bench_entry_price=excluded.bench_entry_price,
                            bench_exit_price=excluded.bench_exit_price, asset_return=excluded.asset_return,
                            bench_return=excluded.bench_return, excess_return=excluded.excess_return, hit=excluded.hit,
                            factors=excluded.factors, overall=excluded.overall, resolved_at=excluded.resolved_at
                         WHERE analysis_outcomes.status = 'retry'",
                    )
                    .bind(&h.id)
                    .bind(i64::from(p.horizon))
                    .bind(&h.asset_id)
                    .bind(class)
                    .bind(&h.verdict)
                    .bind(h.confidence)
                    .bind(&h.model)
                    .bind(h.generated_at)
                    .bind(p.attempts)
                    .bind(RULE_VERSION)
                    .bind(bench.id.as_ref().map(ToString::to_string))
                    .bind(bench.kind.as_str())
                    .bind(r.entry_ts)
                    .bind(r.exit_ts)
                    .bind(r.entry_price)
                    .bind(r.exit_price)
                    .bind(r.bench_entry_price)
                    .bind(r.bench_exit_price)
                    .bind(r.asset_return)
                    .bind(r.bench_return)
                    .bind(r.excess)
                    .bind(hit.map(i64::from))
                    .bind(factors)
                    .bind(overall)
                    .bind(now_ms)
                    .execute(db)
                    .await?;
                    report.resolved += 1;
                }
            }
        }
    }
    Ok(report)
}

fn sorted(mut c: Vec<Candle>) -> Vec<Candle> {
    c.sort_by_key(|x| x.t);
    c.dedup_by_key(|x| x.t);
    c
}

fn count(r: &mut ResolveReport, w: Written) {
    match w {
        Written::Skipped => r.skipped += 1,
        Written::Retry => r.retry += 1,
    }
}

async fn analysis_factors(db: &SqlitePool, id: &str) -> (Option<String>, Option<f64>) {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM analyses WHERE id = ?1")
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    let Some((json,)) = row else {
        return (None, None);
    };
    match serde_json::from_str::<Analysis>(&json) {
        Ok(a) => (
            serde_json::to_string(&factors_of(&a.scores)).ok(),
            Some(f64::from(a.scores.overall)).filter(|x| x.is_finite()),
        ),
        Err(_) => (None, None),
    }
}

/// Fila definitiva sin precios (`skipped`). No pisa una fila ya escrita salvo `retry`.
async fn write_final(
    db: &SqlitePool,
    h: &AnalysisHead,
    horizon: u32,
    status: &str,
    note: &str,
    now_ms: i64,
) -> Result<(), LearningError> {
    let class = h.asset_id.split_once(':').map_or("other", |(c, _)| c);
    sqlx::query(
        "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
                                        generated_at, status, note, rule_version, resolved_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(analysis_id, horizon_weeks) DO UPDATE SET status=excluded.status, note=excluded.note,
            retry_after=NULL, resolved_at=excluded.resolved_at
         WHERE analysis_outcomes.status = 'retry'",
    )
    .bind(&h.id)
    .bind(i64::from(horizon))
    .bind(&h.asset_id)
    .bind(class)
    .bind(&h.verdict)
    .bind(h.confidence)
    .bind(&h.model)
    .bind(h.generated_at)
    .bind(status)
    .bind(note)
    .bind(RULE_VERSION)
    .bind(now_ms)
    .execute(db)
    .await?;
    Ok(())
}

/// Reintento con retroceso exponencial (6 h · 2^intentos, tope 7 días); al agotarlos, `skipped`.
async fn write_retry(
    db: &SqlitePool,
    h: &AnalysisHead,
    p: &Pair,
    why: &str,
    now_ms: i64,
    cfg: &ResolveConfig,
) -> Result<Written, LearningError> {
    let attempts = p.attempts + 1;
    if attempts >= cfg.max_attempts {
        let note = format!("sin datos tras {attempts} intentos: {why}");
        sqlx::query(
            "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
                                            generated_at, status, note, attempts, rule_version, resolved_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'skipped',?9,?10,?11,?12)
             ON CONFLICT(analysis_id, horizon_weeks) DO UPDATE SET status='skipped', note=excluded.note,
                attempts=excluded.attempts, retry_after=NULL, resolved_at=excluded.resolved_at
             WHERE analysis_outcomes.status = 'retry'",
        )
        .bind(&h.id)
        .bind(i64::from(p.horizon))
        .bind(&h.asset_id)
        .bind(h.asset_id.split_once(':').map_or("other", |(c, _)| c))
        .bind(&h.verdict)
        .bind(h.confidence)
        .bind(&h.model)
        .bind(h.generated_at)
        .bind(note)
        .bind(attempts)
        .bind(RULE_VERSION)
        .bind(now_ms)
        .execute(db)
        .await?;
        return Ok(Written::Skipped);
    }
    let wait = (6 * 3600i64 * (1i64 << attempts.min(10))).min(7 * 86_400) * 1000;
    sqlx::query(
        "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
                                        generated_at, status, note, attempts, retry_after, rule_version, resolved_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'retry',?9,?10,?11,?12,?13)
         ON CONFLICT(analysis_id, horizon_weeks) DO UPDATE SET note=excluded.note, attempts=excluded.attempts,
            retry_after=excluded.retry_after, resolved_at=excluded.resolved_at
         WHERE analysis_outcomes.status = 'retry'",
    )
    .bind(&h.id)
    .bind(i64::from(p.horizon))
    .bind(&h.asset_id)
    .bind(h.asset_id.split_once(':').map_or("other", |(c, _)| c))
    .bind(&h.verdict)
    .bind(h.confidence)
    .bind(&h.model)
    .bind(h.generated_at)
    .bind(why)
    .bind(attempts)
    .bind(now_ms + wait)
    .bind(RULE_VERSION)
    .bind(now_ms)
    .execute(db)
    .await?;
    Ok(Written::Retry)
}
