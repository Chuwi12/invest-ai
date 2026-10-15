//! Calibración de la confianza declarada: curva de fiabilidad y calibrador isotónico (PAV)
//! suavizado hacia la tasa base. Solo se APLICA con muestra suficiente y si mejora fuera de
//! muestra (cronológicamente); por debajo, se informa pero no se corrige.

use serde::Serialize;

use crate::stats::wilson;

#[derive(Debug, Clone, Copy)]
pub struct CalPoint {
    pub confidence: f64,
    pub hit: bool,
    pub t_ms: i64,
}

#[derive(Debug, Clone)]
pub struct CalibrationConfig {
    /// Muestra mínima (independiente) por horizonte para aplicar el calibrador.
    pub min_n: usize,
    /// Pseudo-observaciones que tiran cada bloque hacia la tasa base.
    pub shrink_k: f64,
    /// Mejora mínima del Brier fuera de muestra para aplicarlo.
    pub min_brier_gain: f64,
    /// Fracción final (cronológica) reservada para la comprobación.
    pub holdout_frac: f64,
}

impl Default for CalibrationConfig {
    fn default() -> Self {
        Self {
            min_n: 200,
            shrink_k: 20.0,
            min_brier_gain: 0.002,
            holdout_frac: 0.3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReliabilityBin {
    pub lo: f64,
    pub hi: f64,
    pub n: usize,
    pub mean_confidence: f64,
    pub hit_rate: f64,
    pub ci_low: f64,
    pub ci_high: f64,
}

/// Curva de fiabilidad: 10 tramos de 0,1 de confianza (solo los que tienen datos).
pub fn reliability(points: &[CalPoint]) -> Vec<ReliabilityBin> {
    let mut bins = Vec::new();
    for b in 0..10 {
        let (lo, hi) = (b as f64 / 10.0, (b + 1) as f64 / 10.0);
        let inb: Vec<&CalPoint> = points
            .iter()
            .filter(|p| {
                p.confidence >= lo && (p.confidence < hi || (b == 9 && p.confidence <= 1.0))
            })
            .collect();
        if inb.is_empty() {
            continue;
        }
        let n = inb.len();
        let k = inb.iter().filter(|p| p.hit).count();
        let (ci_low, ci_high) = wilson(k, n).unwrap_or((0.0, 1.0));
        bins.push(ReliabilityBin {
            lo,
            hi,
            n,
            mean_confidence: inb.iter().map(|p| p.confidence).sum::<f64>() / n as f64,
            hit_rate: k as f64 / n as f64,
            ci_low,
            ci_high,
        });
    }
    bins
}

/// Calibrador monótono: nudos `(confianza, probabilidad de acierto)` con interpolación lineal.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Calibrator {
    pub knots: Vec<(f64, f64)>,
}

impl Calibrator {
    pub fn apply(&self, c: f64) -> f64 {
        let k = &self.knots;
        let y = match k.len() {
            0 => return c,
            1 => k[0].1,
            _ if c <= k[0].0 => k[0].1,
            _ if c >= k[k.len() - 1].0 => k[k.len() - 1].1,
            _ => {
                let i = k.partition_point(|x| x.0 <= c);
                let ((x0, y0), (x1, y1)) = (k[i - 1], k[i]);
                if x1 - x0 < 1e-12 {
                    y0
                } else {
                    y0 + (y1 - y0) * (c - x0) / (x1 - x0)
                }
            }
        };
        y.clamp(0.05, 0.95)
    }
}

/// Pool Adjacent Violators sobre bloques `(x medio, y medio, peso)` ya ordenados por x.
fn pav(blocks: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    let mut out: Vec<(f64, f64, f64)> = Vec::new();
    for b in blocks {
        out.push(b);
        while out.len() >= 2 && out[out.len() - 2].1 > out[out.len() - 1].1 {
            let (x1, y1, w1) = out.pop().unwrap_or_default();
            let (x0, y0, w0) = out.pop().unwrap_or_default();
            let w = w0 + w1;
            out.push(((x0 * w0 + x1 * w1) / w, (y0 * w0 + y1 * w1) / w, w));
        }
    }
    out
}

/// Ajusta el calibrador isotónico: PAV, encogimiento de cada bloque hacia la tasa base y PAV otra vez.
pub fn fit(points: &[CalPoint], shrink_k: f64) -> Option<Calibrator> {
    if points.len() < 2 {
        return None;
    }
    let mut p: Vec<&CalPoint> = points.iter().collect();
    p.sort_by(|a, b| a.confidence.total_cmp(&b.confidence));
    let base = points.iter().filter(|x| x.hit).count() as f64 / points.len() as f64;
    let blocks = pav(p
        .iter()
        .map(|x| (x.confidence, f64::from(u8::from(x.hit)), 1.0))
        .collect());
    let shrunk = blocks
        .into_iter()
        .map(|(x, y, w)| (x, (y * w + base * shrink_k) / (w + shrink_k), w + shrink_k))
        .collect();
    let knots = pav(shrunk).into_iter().map(|(x, y, _)| (x, y)).collect();
    Some(Calibrator { knots })
}

pub fn brier(points: &[CalPoint], f: impl Fn(f64) -> f64) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    Some(
        points
            .iter()
            .map(|p| (f(p.confidence) - f64::from(u8::from(p.hit))).powi(2))
            .sum::<f64>()
            / points.len() as f64,
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationStatus {
    pub horizon_weeks: u32,
    /// Muestra independiente usada.
    pub n: usize,
    pub min_required: usize,
    pub applied: bool,
    pub reason: String,
    pub brier_raw: Option<f64>,
    pub brier_holdout_raw: Option<f64>,
    pub brier_holdout_calibrated: Option<f64>,
    pub reliability: Vec<ReliabilityBin>,
    /// `Some` solo si `applied`.
    pub calibrator: Option<Calibrator>,
}

/// Evalúa la calibración con puntos ya independientes y en orden cronológico.
pub fn assess(
    points: &[CalPoint],
    horizon_weeks: u32,
    cfg: &CalibrationConfig,
) -> CalibrationStatus {
    let n = points.len();
    let mut st = CalibrationStatus {
        horizon_weeks,
        n,
        min_required: cfg.min_n,
        applied: false,
        reason: String::new(),
        brier_raw: brier(points, |c| c),
        brier_holdout_raw: None,
        brier_holdout_calibrated: None,
        reliability: reliability(points),
        calibrator: None,
    };
    if n < cfg.min_n {
        st.reason = format!(
            "muestra insuficiente: {n} < {} análisis independientes; se informa, no se corrige",
            cfg.min_n
        );
        return st;
    }
    let split = ((n as f64) * (1.0 - cfg.holdout_frac)).floor() as usize;
    let (train, test) = points.split_at(split.clamp(1, n - 1));
    let Some(cal_train) = fit(train, cfg.shrink_k) else {
        st.reason = "no se pudo ajustar".into();
        return st;
    };
    st.brier_holdout_raw = brier(test, |c| c);
    st.brier_holdout_calibrated = brier(test, |c| cal_train.apply(c));
    let gain = st.brier_holdout_raw.unwrap_or(0.0) - st.brier_holdout_calibrated.unwrap_or(0.0);
    if gain < cfg.min_brier_gain {
        st.reason = format!(
            "el calibrador no mejora el Brier fuera de muestra (ganancia {gain:.4} < {:.4}); no se aplica",
            cfg.min_brier_gain
        );
        return st;
    }
    st.calibrator = fit(points, cfg.shrink_k);
    st.applied = st.calibrator.is_some();
    st.reason = format!("aplicado: Brier fuera de muestra {gain:+.4} mejor");
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Rng;

    fn synth(n: usize, seed: u64, truth: impl Fn(f64) -> f64) -> Vec<CalPoint> {
        let mut r = Rng::new(seed);
        (0..n)
            .map(|i| {
                let c = 0.3 + 0.55 * r.uniform();
                CalPoint {
                    confidence: c,
                    hit: r.uniform() < truth(c),
                    t_ms: i as i64,
                }
            })
            .collect()
    }

    #[test]
    fn pav_is_monotone_and_applies_interpolation() {
        let pts = synth(400, 1, |c| 0.45 + 0.1 * c);
        let cal = fit(&pts, 20.0).unwrap();
        assert!(cal.knots.windows(2).all(|w| w[0].1 <= w[1].1 + 1e-12));
        assert!(cal.apply(0.0) >= 0.05 && cal.apply(1.0) <= 0.95);
    }

    #[test]
    fn overconfident_model_is_corrected_with_enough_data() {
        // Declara 0,3–0,85 pero acierta ~50 % siempre.
        let pts = synth(600, 7, |_| 0.5);
        let st = assess(&pts, 4, &CalibrationConfig::default());
        assert!(st.applied, "{}", st.reason);
        let cal = st.calibrator.unwrap();
        assert!((cal.apply(0.8) - 0.5).abs() < 0.08, "{}", cal.apply(0.8));
        println!(
            "calibración n=600: Brier fuera de muestra {:.4} -> {:.4}; confianza 0.80 -> {:.3}",
            st.brier_holdout_raw.unwrap(),
            st.brier_holdout_calibrated.unwrap(),
            cal.apply(0.8)
        );
    }

    #[test]
    fn small_sample_reports_but_does_not_correct() {
        let pts = synth(80, 3, |_| 0.5);
        let st = assess(&pts, 4, &CalibrationConfig::default());
        assert!(!st.applied && st.calibrator.is_none());
        assert!(!st.reliability.is_empty());
        assert!(st.reason.contains("insuficiente"));
    }

    #[test]
    fn already_calibrated_model_is_left_alone() {
        let pts = synth(800, 11, |c| c);
        let st = assess(&pts, 4, &CalibrationConfig::default());
        assert!(!st.applied, "{}", st.reason);
    }

    #[test]
    fn reliability_bins_cover_all_points() {
        let pts = synth(300, 5, |c| c);
        let rel = reliability(&pts);
        assert_eq!(rel.iter().map(|b| b.n).sum::<usize>(), 300);
    }
}
