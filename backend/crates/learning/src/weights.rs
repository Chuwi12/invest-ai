//! Pesos de la puntuación aprendidos con puerta de validación.
//!
//! Lo que se aprende son 7 MULTIPLICADORES (uno por componente) que se aplican a los pesos por
//! clase de activo del código (`ai::scoring::default_weights`): así se conserva la estructura por
//! clase (cripto sin fundamentales…) y solo se estiman 7 números con una muestra agrupada.
//!
//! Entrenamiento: ridge ponderado por recencia hacia la dirección de los pesos vigentes (con poca
//! evidencia el resultado se queda en los pesos actuales). Puerta: el candidato solo se adopta si
//! mejora a los vigentes en un tramo POSTERIOR al entrenamiento (con purga de etiquetas
//! solapadas), con margen y test de bootstrap por bloques. Ver `docs/notes/learning.md`.

use ai::scoring::default_weights;
use chrono::{DateTime, Utc};
use domain::AssetClass;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    LearningError,
    rules::RULE_VERSION,
    stats::{self, cluster_bootstrap, n_clusters, solve, spearman},
    store::{self, FACTOR_NAMES, Factors, N_FACTORS},
};

pub const ONES: [f64; N_FACTORS] = [1.0; N_FACTORS];

#[derive(Debug, Clone)]
pub struct LearnRow {
    pub asset_id: String,
    pub class: AssetClass,
    pub t_ms: i64,
    pub excess: f64,
    pub factors: Factors,
}

#[derive(Debug, Clone)]
pub struct WeightsConfig {
    /// Horizonte (semanas) contra el que se aprende. 4 por defecto: 1 es casi todo ruido y 12 tarda mucho.
    pub horizon_weeks: u32,
    pub min_train: usize,
    pub min_val: usize,
    /// Bloques temporales independientes mínimos en validación.
    pub min_clusters_val: usize,
    /// Fracción final de las filas dedicada a validar.
    pub val_frac: f64,
    /// Vida media del peso de recencia, en semanas.
    pub half_life_weeks: f64,
    /// Fuerza del ridge hacia los pesos vigentes, en «observaciones».
    pub ridge_alpha: f64,
    pub m_min: f64,
    pub m_max: f64,
    /// Mejora mínima de la correlación de rangos (IC) en validación.
    pub margin_ic: f64,
    /// Nivel (unilateral) del test de bootstrap; se divide por el nº de candidatos (Bonferroni).
    pub alpha: f64,
    pub boot_reps: usize,
    pub seed: u64,
    /// Filas independientes nuevas necesarias para volver a evaluar la puerta.
    pub min_new_rows: usize,
    /// Retención: los pesos aprendidos se conservan mientras la evidencia reciente (posterior a su
    /// validación) los respalde frente a los de por defecto con este p-valor (más laxo que `alpha`:
    /// la carga de la prueba para MANTENER es menor que para ADOPTAR, pero existe).
    pub retain_p: f64,
    /// Filas recientes máximas de la ventana de retención.
    pub retain_max_rows: usize,
}

impl Default for WeightsConfig {
    fn default() -> Self {
        Self {
            horizon_weeks: 4,
            min_train: 150,
            min_val: 80,
            min_clusters_val: 8,
            val_frac: 0.3,
            half_life_weeks: 26.0,
            ridge_alpha: 100.0,
            m_min: 0.25,
            m_max: 3.0,
            margin_ic: 0.03,
            alpha: 0.05,
            boot_reps: 500,
            seed: 20261014,
            min_new_rows: 40,
            retain_p: 0.25,
            retain_max_rows: 400,
        }
    }
}

/// Puntuación global con unos multiplicadores: delega en `ai::scoring::combine` (misma fórmula que `score`).
pub fn combine(class: AssetClass, f: &Factors, m: &[f64; N_FACTORS]) -> f64 {
    ai::scoring::combine(class, f, m)
}

/// Una fila por activo y ventana de `h` semanas (filas ordenadas por tiempo).
pub fn dedupe(rows: &[LearnRow], h: u32) -> Vec<LearnRow> {
    store::independent(rows, h, |r| (r.asset_id.as_str(), r.t_ms))
        .into_iter()
        .map(|i| rows[i].clone())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    pub multipliers: [f64; N_FACTORS],
    /// Coeficientes ridge (unidades estandarizadas).
    pub beta: [f64; N_FACTORS],
    pub active: [bool; N_FACTORS],
    pub n_eff: f64,
    pub prior_slope: f64,
}

fn weighted_median_free(v: &[f64]) -> f64 {
    stats::median(v)
}

/// Ridge ponderado por recencia hacia la dirección de los pesos por defecto.
pub fn fit(train: &[LearnRow], t_ref_ms: i64, cfg: &WeightsConfig) -> Option<Fit> {
    let n = train.len();
    if n < 10 {
        return None;
    }
    let hl = cfg.half_life_weeks * 7.0 * 86_400_000.0;
    let mut w: Vec<f64> = train
        .iter()
        .map(|r| 0.5f64.powf((t_ref_ms - r.t_ms).max(0) as f64 / hl))
        .collect();
    let sw: f64 = w.iter().sum();
    for x in &mut w {
        *x *= n as f64 / sw;
    }
    let n_eff = (n as f64).powi(2) / w.iter().map(|x| x * x).sum::<f64>();

    let (mut mu, mut sd, mut active) = ([0.0; N_FACTORS], [1.0; N_FACTORS], [false; N_FACTORS]);
    for j in 0..N_FACTORS {
        let (mut sw_j, mut s, mut cnt) = (0.0, 0.0, 0usize);
        for (r, wi) in train.iter().zip(&w) {
            if let Some(x) = r.factors[j] {
                sw_j += wi;
                s += wi * x;
                cnt += 1;
            }
        }
        if cnt < 30.max(n / 5) {
            continue;
        }
        let m = s / sw_j;
        let var: f64 = train
            .iter()
            .zip(&w)
            .filter_map(|(r, wi)| r.factors[j].map(|x| wi * (x - m).powi(2)))
            .sum::<f64>()
            / sw_j;
        if var.sqrt() > 1e-6 {
            (mu[j], sd[j], active[j]) = (m, var.sqrt(), true);
        }
    }
    if !active.iter().any(|a| *a) {
        return None;
    }
    let z: Vec<[f64; N_FACTORS]> = train
        .iter()
        .map(|r| {
            let mut row = [0.0; N_FACTORS];
            for j in 0..N_FACTORS {
                if active[j]
                    && let Some(x) = r.factors[j]
                {
                    row[j] = (x - mu[j]) / sd[j];
                }
            }
            row
        })
        .collect();

    // Objetivo: exceso de rentabilidad recortado a mediana ± 4·σ_robusta y estandarizado.
    let ys: Vec<f64> = train.iter().map(|r| r.excess).collect();
    let med = weighted_median_free(&ys);
    let mad = weighted_median_free(&ys.iter().map(|y| (y - med).abs()).collect::<Vec<_>>());
    let mut scale = 1.4826 * mad;
    if scale <= 1e-12 {
        scale = ys.iter().map(|y| (y - med).abs()).sum::<f64>() / n as f64;
    }
    if scale <= 1e-12 {
        return None;
    }
    let clipped: Vec<f64> = ys
        .iter()
        .map(|y| y.clamp(med - 4.0 * scale, med + 4.0 * scale))
        .collect();
    let ym = clipped.iter().zip(&w).map(|(y, wi)| y * wi).sum::<f64>() / n as f64;
    let ysd = (clipped
        .iter()
        .zip(&w)
        .map(|(y, wi)| wi * (y - ym).powi(2))
        .sum::<f64>()
        / n as f64)
        .sqrt();
    if ysd < 1e-12 {
        return None;
    }
    let yz: Vec<f64> = clipped.iter().map(|y| (y - ym) / ysd).collect();

    // Dirección de los pesos por defecto (agrupada sobre las clases de la muestra).
    let mut d = [0.0; N_FACTORS];
    for (r, wi) in train.iter().zip(&w) {
        let dw = default_weights(r.class);
        let tot: f64 = (0..N_FACTORS)
            .filter(|&j| active[j] && r.factors[j].is_some())
            .map(|j| dw[j])
            .sum();
        if tot > 0.0 {
            for j in 0..N_FACTORS {
                if active[j] && r.factors[j].is_some() {
                    d[j] += wi * dw[j] / tot;
                }
            }
        }
    }
    let sumd: f64 = d.iter().sum();
    if sumd <= 0.0 {
        return None;
    }
    for x in &mut d {
        *x /= sumd;
    }
    let dt: Vec<f64> = (0..N_FACTORS).map(|j| d[j] * sd[j]).collect();
    let s0: Vec<f64> = z
        .iter()
        .map(|row| (0..N_FACTORS).map(|j| row[j] * dt[j]).sum())
        .collect();
    let num: f64 = (0..n).map(|i| w[i] * s0[i] * yz[i]).sum();
    let den: f64 = (0..n).map(|i| w[i] * s0[i] * s0[i]).sum();
    let c = if den > 1e-12 {
        (num / den).max(0.0)
    } else {
        0.0
    };

    let alpha = cfg.ridge_alpha;
    let mut a = vec![vec![0.0; N_FACTORS]; N_FACTORS];
    let mut b = vec![0.0; N_FACTORS];
    for i in 0..n {
        for j in 0..N_FACTORS {
            b[j] += w[i] * z[i][j] * yz[i];
            for k in 0..N_FACTORS {
                a[j][k] += w[i] * z[i][j] * z[i][k];
            }
        }
    }
    for j in 0..N_FACTORS {
        a[j][j] += alpha;
        b[j] += alpha * c * dt[j];
    }
    let beta_v = solve(a, b)?;
    let mut beta = [0.0; N_FACTORS];
    beta.copy_from_slice(&beta_v);

    let v: Vec<f64> = (0..N_FACTORS)
        .map(|j| {
            if active[j] {
                beta[j].max(0.0) / sd[j]
            } else {
                0.0
            }
        })
        .collect();
    let sumv: f64 = v.iter().sum();
    let mut m = [1.0; N_FACTORS];
    for j in 0..N_FACTORS {
        if active[j] && d[j] > 1e-9 {
            let share = if sumv > 1e-12 { v[j] / sumv } else { d[j] };
            m[j] = (share / d[j]).clamp(cfg.m_min, cfg.m_max);
        }
    }
    let norm: f64 = (0..N_FACTORS)
        .filter(|&j| active[j] && d[j] > 1e-9)
        .map(|j| d[j] * m[j])
        .sum();
    if norm > 1e-12 {
        for j in 0..N_FACTORS {
            if active[j] && d[j] > 1e-9 {
                m[j] /= norm;
            }
        }
    }
    Some(Fit {
        multipliers: m,
        beta,
        active,
        n_eff,
        prior_slope: c,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Adopt,
    /// Los pesos vigentes ya no tienen respaldo reciente: se encogen hacia los de por defecto.
    Decay,
    Keep,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// `learned` (ridge), `rollback` (volver a los pesos por defecto) o `decay` (encogimiento).
    pub kind: &'static str,
    pub multipliers: [f64; N_FACTORS],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateReport {
    pub decision: Decision,
    pub reason: String,
    pub candidate: Option<Candidate>,
    pub n_independent: usize,
    pub n_train: usize,
    pub n_val: usize,
    pub train_end_ms: Option<i64>,
    pub val_start_ms: Option<i64>,
    pub val_end_ms: Option<i64>,
    pub ic_incumbent: Option<f64>,
    pub ic_candidate: Option<f64>,
    pub delta_ic: Option<f64>,
    pub p_value: Option<f64>,
    pub fit: Option<Fit>,
    pub retention: Option<Retention>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    pub n: usize,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub ic_incumbent: f64,
    pub ic_defaults: f64,
    pub p_value: f64,
    pub supported: bool,
}

impl GateReport {
    fn keep(reason: impl Into<String>, n_independent: usize) -> Self {
        Self {
            decision: Decision::Keep,
            reason: reason.into(),
            candidate: None,
            n_independent,
            n_train: 0,
            n_val: 0,
            train_end_ms: None,
            val_start_ms: None,
            val_end_ms: None,
            ic_incumbent: None,
            ic_candidate: None,
            delta_ic: None,
            p_value: None,
            fit: None,
            retention: None,
        }
    }
}

fn cluster_of(t_ms: i64, h: u32) -> u64 {
    (t_ms.max(0) / (i64::from(h) * 7 * 86_400_000)) as u64
}

/// Puerta de validación. `rows` en orden cronológico y SOLO con etiquetas ya conocidas.
pub fn evaluate_gate(
    rows: &[LearnRow],
    incumbent: &[f64; N_FACTORS],
    not_before_ms: Option<i64>,
    alpha_eff: f64,
    cfg: &WeightsConfig,
) -> GateReport {
    let rows = dedupe(rows, cfg.horizon_weeks);
    let n = rows.len();
    if n < cfg.min_train + cfg.min_val {
        return GateReport::keep(
            format!(
                "muestra insuficiente: {n} filas independientes < {} (entrenamiento {} + validación {})",
                cfg.min_train + cfg.min_val,
                cfg.min_train,
                cfg.min_val
            ),
            n,
        );
    }
    let n_val = ((n as f64 * cfg.val_frac).ceil() as usize)
        .max(cfg.min_val)
        .min(n);
    let mut start = n - n_val;
    if let Some(nb) = not_before_ms {
        start = start.max(rows.partition_point(|r| r.t_ms <= nb));
    }
    let val = &rows[start.min(n)..];
    if val.len() < cfg.min_val {
        return GateReport::keep(
            format!(
                "validación posterior a la última adopción insuficiente: {} < {}",
                val.len(),
                cfg.min_val
            ),
            n,
        );
    }
    let t_v = val[0].t_ms;
    let h_ms = i64::from(cfg.horizon_weeks) * 7 * 86_400_000;
    // Purga: una etiqueta de entrenamiento cuyo horizonte invade la validación comparte movimientos de mercado con ella.
    let train: Vec<LearnRow> = rows[..start]
        .iter()
        .filter(|r| r.t_ms + h_ms <= t_v)
        .cloned()
        .collect();
    if train.len() < cfg.min_train {
        return GateReport::keep(
            format!(
                "entrenamiento insuficiente tras la purga: {} < {}",
                train.len(),
                cfg.min_train
            ),
            n,
        );
    }
    let val_clusters: Vec<u64> = val
        .iter()
        .map(|r| cluster_of(r.t_ms, cfg.horizon_weeks))
        .collect();
    if n_clusters(&val_clusters) < cfg.min_clusters_val {
        return GateReport::keep(
            format!(
                "validación con {} bloques temporales < {}",
                n_clusters(&val_clusters),
                cfg.min_clusters_val
            ),
            n,
        );
    }
    let t_ref = train.last().map_or(t_v, |r| r.t_ms);
    let Some(fit) = fit(&train, t_ref, cfg) else {
        return GateReport::keep("no se pudo ajustar el modelo (datos degenerados)", n);
    };

    let y: Vec<f64> = val.iter().map(|r| r.excess).collect();
    let scores = |m: &[f64; N_FACTORS]| -> Vec<f64> {
        val.iter()
            .map(|r| combine(r.class, &r.factors, m))
            .collect()
    };
    let s_inc = scores(incumbent);
    let ic_inc = spearman(&s_inc, &y);

    let mut cands: Vec<Candidate> = Vec::new();
    let near = |a: &[f64; N_FACTORS], b: &[f64; N_FACTORS]| {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
    };
    if !near(&fit.multipliers, incumbent) {
        cands.push(Candidate {
            kind: "learned",
            multipliers: fit.multipliers,
        });
    }
    if !near(&ONES, incumbent) {
        cands.push(Candidate {
            kind: "rollback",
            multipliers: ONES,
        });
    }
    let mut report = GateReport {
        decision: Decision::Keep,
        reason: String::new(),
        candidate: None,
        n_independent: n,
        n_train: train.len(),
        n_val: val.len(),
        train_end_ms: Some(t_ref),
        val_start_ms: Some(t_v),
        val_end_ms: val.last().map(|r| r.t_ms),
        ic_incumbent: ic_inc,
        ic_candidate: None,
        delta_ic: None,
        p_value: None,
        fit: Some(fit),
        retention: None,
    };
    let Some(ic_inc) = ic_inc else {
        report.reason = "correlación vigente no calculable en validación".into();
        return report;
    };
    if cands.is_empty() {
        report.reason = "el candidato entrenado coincide con los pesos vigentes".into();
        return report;
    }
    let alpha = alpha_eff / cands.len() as f64;
    let mut best: Option<(f64, f64, f64, Candidate)> = None; // (delta, ic, p, cand)
    let mut notes = Vec::new();
    for c in cands {
        let s_c = scores(&c.multipliers);
        let Some(ic_c) = spearman(&s_c, &y) else {
            continue;
        };
        let delta = ic_c - ic_inc;
        let reps = cluster_bootstrap(&val_clusters, cfg.boot_reps, cfg.seed, |idx| {
            let a: Vec<f64> = idx.iter().map(|&i| s_c[i]).collect();
            let b: Vec<f64> = idx.iter().map(|&i| s_inc[i]).collect();
            let yy: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
            Some(spearman(&a, &yy)? - spearman(&b, &yy)?)
        });
        if reps.len() < cfg.boot_reps / 2 {
            continue;
        }
        let p =
            (reps.iter().filter(|d| **d <= 0.0).count() as f64 + 1.0) / (reps.len() as f64 + 1.0);
        let pass = delta >= cfg.margin_ic && p <= alpha && ic_c > 0.0;
        notes.push(format!(
            "{}: IC {ic_c:+.3} (vigente {ic_inc:+.3}, Δ {delta:+.3}, p {p:.3}) {}",
            c.kind,
            if pass {
                "supera la puerta"
            } else {
                "no supera la puerta"
            }
        ));
        if pass && best.as_ref().is_none_or(|b| delta > b.0) {
            best = Some((delta, ic_c, p, c));
        } else if best.is_none() && report.delta_ic.is_none_or(|d| delta > d) {
            report.delta_ic = Some(delta);
            report.ic_candidate = Some(ic_c);
            report.p_value = Some(p);
        }
    }
    match best {
        Some((delta, ic_c, p, c)) => {
            report.decision = Decision::Adopt;
            report.delta_ic = Some(delta);
            report.ic_candidate = Some(ic_c);
            report.p_value = Some(p);
            report.reason = format!(
                "adoptado ({}); margen ΔIC ≥ {:.2}, p ≤ {alpha:.3}: {}",
                c.kind,
                cfg.margin_ic,
                notes.join("; ")
            );
            report.candidate = Some(c);
        }
        None => {
            report.reason = if notes.is_empty() {
                "ningún candidato evaluable".into()
            } else {
                format!("se conservan los pesos vigentes; {}", notes.join("; "))
            };
        }
    }
    report
}

/// ¿Sigue respaldado el vigente frente al de por defecto en la evidencia POSTERIOR a su validación?
/// `None` = ventana insuficiente (no se puede afirmar nada: se conserva).
pub fn retention_check(
    rows: &[LearnRow],
    incumbent: &[f64; N_FACTORS],
    not_before_ms: Option<i64>,
    cfg: &WeightsConfig,
) -> Option<Retention> {
    let rows = dedupe(rows, cfg.horizon_weeks);
    let from = not_before_ms.map_or(0, |nb| rows.partition_point(|r| r.t_ms <= nb));
    let fresh = &rows[from.min(rows.len())..];
    let w = &fresh[fresh.len().saturating_sub(cfg.retain_max_rows)..];
    let clusters: Vec<u64> = w
        .iter()
        .map(|r| cluster_of(r.t_ms, cfg.horizon_weeks))
        .collect();
    if w.len() < cfg.min_val || n_clusters(&clusters) < cfg.min_clusters_val {
        return None;
    }
    let y: Vec<f64> = w.iter().map(|r| r.excess).collect();
    let s_inc: Vec<f64> = w
        .iter()
        .map(|r| combine(r.class, &r.factors, incumbent))
        .collect();
    let s_def: Vec<f64> = w
        .iter()
        .map(|r| combine(r.class, &r.factors, &ONES))
        .collect();
    let (ic_i, ic_d) = (spearman(&s_inc, &y)?, spearman(&s_def, &y)?);
    let reps = cluster_bootstrap(&clusters, cfg.boot_reps, cfg.seed ^ 0x5eed, |idx| {
        let yy: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let a: Vec<f64> = idx.iter().map(|&i| s_inc[i]).collect();
        let b: Vec<f64> = idx.iter().map(|&i| s_def[i]).collect();
        Some(spearman(&a, &yy)? - spearman(&b, &yy)?)
    });
    if reps.len() < cfg.boot_reps / 2 {
        return None;
    }
    let p = (reps.iter().filter(|d| **d <= 0.0).count() as f64 + 1.0) / (reps.len() as f64 + 1.0);
    Some(Retention {
        n: w.len(),
        window_start_ms: w[0].t_ms,
        window_end_ms: w[w.len() - 1].t_ms,
        ic_incumbent: ic_i,
        ic_defaults: ic_d,
        p_value: p,
        supported: p <= cfg.retain_p && ic_i > ic_d,
    })
}

/// Estado que arrastra el ciclo de aprendizaje (en producción se reconstruye desde SQLite).
#[derive(Debug, Clone)]
pub struct WeightState {
    pub incumbent: [f64; N_FACTORS],
    /// Fin de la validación de la última adopción: la siguiente validación debe ser posterior.
    pub not_before_ms: Option<i64>,
    /// Filas independientes en la última evaluación.
    pub last_eval_rows: usize,
    /// Evaluaciones de la puerta desde la última adopción (gasto de alfa: α·0,61/(1+k)²).
    pub evals_since_change: usize,
}

impl Default for WeightState {
    fn default() -> Self {
        Self {
            incumbent: ONES,
            not_before_ms: None,
            last_eval_rows: 0,
            evals_since_change: 0,
        }
    }
}

/// Un intento del ciclo «como si fuera `as_of`»: solo cuentan filas con etiqueta ya conocida,
/// espera a tener filas nuevas, gasta alfa (α·0,61/(1+k)² tras k evaluaciones sin cambio: mirar muchas
/// veces los mismos datos infla los falsos positivos) y actualiza el estado si adopta o encoge.
/// `None` = en pausa por falta de datos nuevos.
pub fn attempt(
    rows: &[LearnRow],
    as_of_ms: i64,
    st: &mut WeightState,
    cfg: &WeightsConfig,
) -> Option<GateReport> {
    let h_ms = i64::from(cfg.horizon_weeks) * 7 * 86_400_000;
    let known: Vec<LearnRow> = rows
        .iter()
        .filter(|r| r.t_ms + h_ms <= as_of_ms)
        .cloned()
        .collect();
    let n = dedupe(&known, cfg.horizon_weeks).len();
    if st.last_eval_rows > 0 && n < st.last_eval_rows + cfg.min_new_rows {
        return None;
    }
    // Gasto de alfa sumable: α·(6/π²)/(1+k)² (la suma sobre todos los intentos es ≤ α).
    let alpha_eff = cfg.alpha * 0.6079 / (1.0 + st.evals_since_change as f64).powi(2);
    let mut rep = evaluate_gate(&known, &st.incumbent, st.not_before_ms, alpha_eff, cfg);
    if rep.fit.is_some() {
        st.evals_since_change += 1;
    }
    if n >= cfg.min_train + cfg.min_val {
        st.last_eval_rows = n;
    }
    if rep.decision == Decision::Keep
        && st.incumbent != ONES
        && let Some(ret) = retention_check(&known, &st.incumbent, st.not_before_ms, cfg)
    {
        if !ret.supported {
            let mut next = st.incumbent.map(f64::sqrt);
            if next.iter().all(|m| (m - 1.0).abs() < 0.05) {
                next = ONES;
            }
            rep.reason = format!(
                "decaimiento: la evidencia reciente (n {}, IC vigente {:+.3} frente a {:+.3} por defecto, p {:.2}) ya no respalda los pesos aprendidos",
                ret.n, ret.ic_incumbent, ret.ic_defaults, ret.p_value
            );
            rep.decision = Decision::Decay;
            rep.candidate = Some(Candidate {
                kind: "decay",
                multipliers: next,
            });
            rep.val_end_ms = Some(ret.window_end_ms);
            rep.val_start_ms = Some(ret.window_start_ms);
            rep.n_val = ret.n;
        }
        rep.retention = Some(ret);
    }
    if matches!(rep.decision, Decision::Adopt | Decision::Decay)
        && let Some(c) = &rep.candidate
    {
        st.incumbent = c.multipliers;
        st.not_before_ms = rep.val_end_ms;
        st.evals_since_change = 0;
    }
    Some(rep)
}

// ---------------------------------------------------------------------------------------------
// Persistencia y activación
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightVersion {
    pub version: i64,
    pub kind: String,
    pub status: String,
    pub multipliers: [f64; N_FACTORS],
    pub factor_names: [&'static str; N_FACTORS],
    pub created_at_ms: i64,
    pub horizon_weeks: Option<i64>,
    pub trained_to_ms: Option<i64>,
    pub validated_from_ms: Option<i64>,
    pub validated_to_ms: Option<i64>,
    pub n_train: Option<i64>,
    pub n_val: Option<i64>,
    pub metrics: serde_json::Value,
    pub parent_version: Option<i64>,
    pub note: Option<String>,
    /// Regla de resultados (`rules::RULE_VERSION`) con la que se ajustaron. Una versión de otra regla no
    /// puede seguir activa: `activate` la retira.
    pub rule_version: i64,
}

type VersionTuple = (
    i64,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
    Option<i64>,
    Option<String>,
    i64,
);

const VERSION_COLS: &str =
    "version, kind, status, multipliers, created_at, horizon_weeks, trained_to, validated_from,
     validated_to, n_train, n_val, metrics, parent_version, note, rule_version";

fn to_version(t: VersionTuple) -> Option<WeightVersion> {
    let m: Vec<f64> = serde_json::from_str(&t.3).ok()?;
    Some(WeightVersion {
        version: t.0,
        kind: t.1,
        status: t.2,
        multipliers: <[f64; N_FACTORS]>::try_from(m).ok()?,
        factor_names: FACTOR_NAMES,
        created_at_ms: t.4,
        horizon_weeks: t.5,
        trained_to_ms: t.6,
        validated_from_ms: t.7,
        validated_to_ms: t.8,
        n_train: t.9,
        n_val: t.10,
        metrics: serde_json::from_str(&t.11).unwrap_or(serde_json::Value::Null),
        parent_version: t.12,
        note: t.13,
        rule_version: t.14,
    })
}

pub async fn active_version(db: &SqlitePool) -> Result<Option<WeightVersion>, LearningError> {
    let row: Option<VersionTuple> = sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM scoring_weights WHERE status = 'active'"
    ))
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(to_version))
}

pub async fn list_versions(db: &SqlitePool) -> Result<Vec<WeightVersion>, LearningError> {
    let rows: Vec<VersionTuple> = sqlx::query_as(&format!(
        "SELECT {VERSION_COLS} FROM scoring_weights ORDER BY version DESC"
    ))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().filter_map(to_version).collect())
}

/// Una versión de pesos ajustada con otra regla de resultados no puede seguir activa en silencio: sus
/// multiplicadores salen de cifras medidas con otra definición de la entrada. Se retira (`retired`, nunca
/// se borra: vuelven los pesos por defecto) con una nota y una entrada `deactivate` en `learning_runs`, que es
/// el registro de decisiones del aprendizaje. `rollback` o la puerta pueden dar luego una versión nueva.
async fn retire_stale(db: &SqlitePool, now_ms: i64) -> Result<(), LearningError> {
    let mut tx = db.begin().await?;
    let stale: Vec<(i64, i64)> = sqlx::query_as(
        "UPDATE scoring_weights
            SET status = 'retired',
                note = COALESCE(note || ' | ', '') || 'desactivada: ajustada con la regla v' || rule_version || ', vigente v' || ?1
          WHERE status = 'active' AND rule_version <> ?1
          RETURNING version, rule_version",
    )
    .bind(RULE_VERSION)
    .fetch_all(&mut *tx)
    .await?;
    for (version, fitted) in stale {
        let reason = format!(
            "pesos de la versión {version} desactivados: ajustados con la regla v{fitted}, vigente v{RULE_VERSION}; rigen los de por defecto"
        );
        sqlx::query("INSERT INTO learning_runs (ran_at, horizon_weeks, decision, reason, n_rows, version, metrics) VALUES (?1, 0, 'deactivate', ?2, 0, ?3, '{}')")
            .bind(now_ms)
            .bind(reason)
            .bind(version)
            .execute(&mut *tx)
            .await?;
    }
    Ok(tx.commit().await?)
}

/// Instala en `ai::scoring` los multiplicadores de la versión activa (o los por defecto si no hay), tras
/// retirar la que se ajustó con otra regla de resultados. Llamar al arrancar la app y tras cada adopción o
/// vuelta atrás.
pub async fn activate(db: &SqlitePool) -> Result<Option<WeightVersion>, LearningError> {
    retire_stale(db, Utc::now().timestamp_millis()).await?;
    let v = active_version(db).await?;
    ai::scoring::set_learned_multipliers(v.as_ref().map(|v| v.multipliers));
    Ok(v)
}

struct NewVersion<'a> {
    kind: &'a str,
    mult: &'a [f64; N_FACTORS],
    now_ms: i64,
    rep: Option<&'a GateReport>,
    horizon: Option<u32>,
    parent: Option<i64>,
    note: &'a str,
}

async fn insert_active(db: &SqlitePool, v: NewVersion<'_>) -> Result<i64, LearningError> {
    let NewVersion {
        kind,
        mult,
        now_ms,
        rep,
        horizon,
        parent,
        note,
    } = v;
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE scoring_weights SET status = 'retired' WHERE status = 'active'")
        .execute(&mut *tx)
        .await?;
    let (next,): (i64,) =
        sqlx::query_as("SELECT COALESCE(MAX(version), 0) + 1 FROM scoring_weights")
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO scoring_weights (version, kind, status, multipliers, created_at, horizon_weeks, trained_to,
             validated_from, validated_to, n_train, n_val, metrics, parent_version, note, rule_version)
         VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )
    .bind(next)
    .bind(kind)
    .bind(serde_json::to_string(mult)?)
    .bind(now_ms)
    .bind(horizon.map(i64::from))
    .bind(rep.and_then(|r| r.train_end_ms))
    .bind(rep.and_then(|r| r.val_start_ms))
    .bind(rep.and_then(|r| r.val_end_ms))
    .bind(rep.map(|r| r.n_train as i64))
    .bind(rep.map(|r| r.n_val as i64))
    .bind(rep.map_or_else(|| "{}".to_string(), |r| serde_json::to_string(r).unwrap_or_default()))
    .bind(parent)
    .bind(note)
    .bind(RULE_VERSION)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(next)
}

/// Vuelve atrás: crea una versión nueva (nunca se borra historia) con los multiplicadores de
/// `to_version`, o los por defecto si es `None`, y la activa. Es una decisión explícita: la versión nueva
/// lleva la regla vigente, aunque los multiplicadores vengan de una versión ajustada con otra.
pub async fn rollback(
    db: &SqlitePool,
    to_version: Option<i64>,
    now: DateTime<Utc>,
) -> Result<WeightVersion, LearningError> {
    let current = active_version(db).await?.map(|v| v.version);
    let (mult, note) = match to_version {
        None => (ONES, "vuelta a los pesos por defecto".to_string()),
        Some(v) => {
            let all = list_versions(db).await?;
            let found = all
                .into_iter()
                .find(|x| x.version == v)
                .ok_or(LearningError::NoSuchVersion(v))?;
            (found.multipliers, format!("vuelta a la versión {v}"))
        }
    };
    let id = insert_active(
        db,
        NewVersion {
            kind: "rollback",
            mult: &mult,
            now_ms: now.timestamp_millis(),
            rep: None,
            horizon: None,
            parent: current,
            note: &note,
        },
    )
    .await?;
    sqlx::query("INSERT INTO learning_runs (ran_at, horizon_weeks, decision, reason, n_rows, version, metrics) VALUES (?1, 0, 'rollback', ?2, 0, ?3, '{}')")
        .bind(now.timestamp_millis())
        .bind(&note)
        .bind(id)
        .execute(db)
        .await?;
    activate(db).await?.ok_or(LearningError::NoSuchVersion(id))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightsCycle {
    pub ran: bool,
    pub report: Option<GateReport>,
    pub adopted_version: Option<i64>,
}

/// Ciclo de pesos contra SQLite: carga filas resueltas, evalúa la puerta, registra la decisión y,
/// si adopta, crea la versión y la instala en `ai::scoring`.
pub async fn run_weights_cycle(
    db: &SqlitePool,
    now: DateTime<Utc>,
    cfg: &WeightsConfig,
) -> Result<WeightsCycle, LearningError> {
    let rows: Vec<LearnRow> = store::resolved_rows(db, cfg.horizon_weeks)
        .await?
        .into_iter()
        .filter(|r| r.bench_kind == "relative")
        .map(|r| LearnRow {
            asset_id: r.asset_id,
            class: r.class,
            t_ms: r.generated_at_ms,
            excess: r.excess,
            factors: r.factors,
        })
        .filter(|r| r.factors.iter().filter(|f| f.is_some()).count() >= 2)
        .collect();
    // El incumbente de la puerta nunca es de otra regla: se retira (con la hora simulada) y `activate`
    // desinstala sus multiplicadores de `ai::scoring`.
    retire_stale(db, now.timestamp_millis()).await?;
    let active = activate(db).await?;
    let (last_rows,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(MAX(n_rows), 0) FROM learning_runs WHERE horizon_weeks = ?1 AND decision IN ('adopt', 'keep', 'decay')",
    )
    .bind(i64::from(cfg.horizon_weeks))
    .fetch_one(db)
    .await?;
    let (since,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM learning_runs WHERE horizon_weeks = ?1 AND decision = 'keep'
            AND id > COALESCE((SELECT MAX(id) FROM learning_runs WHERE horizon_weeks = ?1 AND decision IN ('adopt', 'decay')), 0)",
    )
    .bind(i64::from(cfg.horizon_weeks))
    .fetch_one(db)
    .await?;
    let mut st = WeightState {
        incumbent: active.as_ref().map_or(ONES, |v| v.multipliers),
        not_before_ms: active.as_ref().and_then(|v| v.validated_to_ms),
        last_eval_rows: last_rows as usize,
        evals_since_change: since as usize,
    };
    let Some(rep) = attempt(&rows, now.timestamp_millis(), &mut st, cfg) else {
        return Ok(WeightsCycle {
            ran: false,
            report: None,
            adopted_version: None,
        });
    };
    let mut adopted = None;
    if matches!(rep.decision, Decision::Adopt | Decision::Decay)
        && let Some(c) = &rep.candidate
    {
        let kind = match c.kind {
            "rollback" => "rollback",
            "decay" => "decay",
            _ => "learned",
        };
        let id = insert_active(
            db,
            NewVersion {
                kind,
                mult: &c.multipliers,
                now_ms: now.timestamp_millis(),
                rep: Some(&rep),
                horizon: Some(cfg.horizon_weeks),
                parent: active.as_ref().map(|v| v.version),
                note: &rep.reason,
            },
        )
        .await?;
        adopted = Some(id);
        activate(db).await?;
    }
    if rep.fit.is_none() {
        // Muestra insuficiente: no es una evaluación de la puerta; no se registra (evita ruido y cuenta de intentos falsa).
        return Ok(WeightsCycle {
            ran: true,
            report: Some(rep),
            adopted_version: None,
        });
    }
    sqlx::query("INSERT INTO learning_runs (ran_at, horizon_weeks, decision, reason, n_rows, version, metrics) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
        .bind(now.timestamp_millis())
        .bind(i64::from(cfg.horizon_weeks))
        .bind(match (adopted.is_some(), rep.decision) {
            (true, Decision::Decay) => "decay",
            (true, _) => "adopt",
            _ => "keep",
        })
        .bind(&rep.reason)
        .bind(rep.n_independent as i64)
        .bind(adopted)
        .bind(serde_json::to_string(&rep)?)
        .execute(db)
        .await?;
    Ok(WeightsCycle {
        ran: true,
        report: Some(rep),
        adopted_version: adopted,
    })
}
