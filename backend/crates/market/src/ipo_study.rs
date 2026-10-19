//! Estudio histórico: ¿se asocian las banderas del perfil de riesgo de una OPV con su exceso de
//! rentabilidad frente a SPY? Lógica pura y determinista (sin red ni reloj); el diseño completo,
//! fijado antes de ejecutar nada, está en `docs/notes/ipo-study.md` y la entrada/salida en
//! `examples/ipo_study.rs`.
//!
//! Sin anticipación: el perfil se reconstruye solo con el folleto definitivo de la OPV (presentado
//! como mucho 3 días después de la primera cotización, con el contenido fijado en el precio) y la
//! entrada del calendario; no usa nada posterior.

use chrono::{DateTime, NaiveDate, Utc};
use domain::{Candle, IpoEntry, IpoIndicatorStatus, IpoPerformance, IpoRiskIndicator, IpoRiskProfile, Prospectus};
use serde::{Deserialize, Serialize};

use crate::{
    ipo::{PerfInput, compute},
    ipo_risk::{ProfileInput, build_profile, offer_size_usd},
    providers::yahoo::DailySeries,
};

/// Semilla de todo el estudio (la fecha del diseño).
pub const SEED: u64 = 20_261_019;
pub const BOOT_REPS: usize = 10_000;
pub const PERM_REPS: usize = 20_000;
pub const PLACEBO_REPS: usize = 1_000;
pub const PLACEBOS: usize = 200;
/// Tamaño mínimo de cada grupo para contrastar.
pub const MIN_GROUP: usize = 10;

// ---------------------------------------------------------------------------------------------
// Estadística
// ---------------------------------------------------------------------------------------------

/// Generador splitmix64: pequeño, reproducible y sin dependencias.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Entero en `0..n` (sesgo despreciable para n pequeño frente a 2^64).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable_by(f64::total_cmp);
    let m = s.len() / 2;
    Some(if s.len() % 2 == 1 { s[m] } else { (s[m - 1] + s[m]) / 2.0 })
}

/// Mediana(a) − mediana(b).
pub fn median_diff(a: &[f64], b: &[f64]) -> Option<f64> {
    Some(median(a)? - median(b)?)
}

/// IC bootstrap percentil de la diferencia de medianas, remuestreando cada grupo por separado.
pub fn bootstrap_ci(a: &[f64], b: &[f64], reps: usize, seed: u64, level: f64) -> Option<(f64, f64)> {
    if a.is_empty() || b.is_empty() || reps < 10 {
        return None;
    }
    let mut rng = Rng::new(seed);
    let mut diffs = Vec::with_capacity(reps);
    let (mut ra, mut rb) = (vec![0.0; a.len()], vec![0.0; b.len()]);
    for _ in 0..reps {
        for x in ra.iter_mut() {
            *x = a[rng.below(a.len())];
        }
        for x in rb.iter_mut() {
            *x = b[rng.below(b.len())];
        }
        diffs.push(median_diff(&ra, &rb)?);
    }
    diffs.sort_unstable_by(f64::total_cmp);
    let tail = (1.0 - level) / 2.0;
    let lo = ((diffs.len() as f64 * tail).floor() as usize).min(diffs.len() - 1);
    let hi = (((diffs.len() as f64) * (1.0 - tail)).ceil() as usize).saturating_sub(1).min(diffs.len() - 1);
    Some((diffs[lo], diffs[hi]))
}

/// p de dos colas por permutación de la diferencia de medianas, con el +1 que evita p = 0.
pub fn permutation_p(a: &[f64], b: &[f64], reps: usize, seed: u64) -> Option<f64> {
    let obs = median_diff(a, b)?.abs();
    if reps == 0 {
        return None;
    }
    let mut pool: Vec<f64> = a.iter().chain(b.iter()).copied().collect();
    let na = a.len();
    let mut rng = Rng::new(seed);
    let mut extreme = 0usize;
    for _ in 0..reps {
        // Fisher-Yates parcial: basta barajar las `na` primeras posiciones.
        for i in 0..na {
            let j = i + rng.below(pool.len() - i);
            pool.swap(i, j);
        }
        let d = median_diff(&pool[..na], &pool[na..])?.abs();
        if d >= obs - 1e-12 {
            extreme += 1;
        }
    }
    Some((extreme as f64 + 1.0) / (reps as f64 + 1.0))
}

/// p ajustados por Holm (control de FWER). Las entradas `None` (no contrastables) no cuentan en la
/// familia y siguen siendo `None`.
pub fn holm(p: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut idx: Vec<usize> = (0..p.len()).filter(|&i| p[i].is_some()).collect();
    idx.sort_by(|&a, &b| p[a].unwrap().total_cmp(&p[b].unwrap()));
    let m = idx.len();
    let mut out = vec![None; p.len()];
    let mut running = 0.0f64;
    for (rank, &i) in idx.iter().enumerate() {
        let adj = ((m - rank) as f64 * p[i].unwrap()).min(1.0);
        running = running.max(adj);
        out[i] = Some(running);
    }
    out
}

/// Desviación robusta (1,4826 · MAD).
pub fn robust_sigma(v: &[f64]) -> Option<f64> {
    let m = median(v)?;
    let dev: Vec<f64> = v.iter().map(|x| (x - m).abs()).collect();
    Some(1.4826 * median(&dev)?)
}

/// Efecto mínimo detectable (pp) para la diferencia de medianas con grupos `n1` y `n0`, una
/// dispersión robusta `sigma`, nivel `alpha` (de dos colas) y potencia 80 %. Aproximación normal
/// con se(mediana) = 1,2533 σ/√n.
pub fn mde(sigma: f64, n1: usize, n0: usize, alpha: f64) -> f64 {
    let z = |p: f64| inv_norm(p);
    let se = |n: usize| 1.2533 * sigma / (n as f64).sqrt();
    (z(1.0 - alpha / 2.0) + z(0.80)) * (se(n1).powi(2) + se(n0).powi(2)).sqrt()
}

/// Inversa de la normal estándar (Acklam, error relativo < 1,2e-9).
#[allow(clippy::excessive_precision)]
pub fn inv_norm(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    const B: [f64; 5] = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    const C: [f64; 6] = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    const D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    let lo = 0.02425;
    if p < lo {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - lo {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -inv_norm(1.0 - p)
    }
}

// ---------------------------------------------------------------------------------------------
// Banderas
// ---------------------------------------------------------------------------------------------

pub const N_FLAGS: usize = 10;

/// Definición de una bandera: id y nombre (umbrales en `flag_values` y en el documento).
pub const FLAGS: [(&str, &str); N_FLAGS] = [
    ("V1", "secondary_gt50: venta secundaria > 50 %"),
    ("V2", "debt_repayment: devolver deuda"),
    ("V3", "multi_vote: voto múltiple"),
    ("V4", "controlled: «controlled company»"),
    ("V5", "dilution_high: dilución ≥ 75 % del precio"),
    ("V6", "float_low: capital ofrecido < 15 %"),
    ("V7", "net_loss: pérdidas en todos los periodos"),
    ("V8", "lockup_short: lock-up < 180 días"),
    ("V9", "size_micro: oferta < 50 M USD"),
    ("V10", "size_large: oferta ≥ 250 M USD"),
];

pub const DILUTION_HIGH: f64 = 0.75;
pub const FLOAT_LOW_PCT: f64 = 15.0;
pub const LOCKUP_SHORT_DAYS: f64 = 180.0;
pub const SIZE_MICRO_USD: f64 = 50e6;
pub const SIZE_LARGE_USD: f64 = 250e6;

fn ind<'a>(p: &'a IpoRiskProfile, key: &str) -> Option<&'a IpoRiskIndicator> {
    p.indicators.iter().find(|i| i.key == key && i.status == IpoIndicatorStatus::Extracted)
}

fn has_flag(i: &IpoRiskIndicator, code: &str) -> bool {
    i.flags.iter().any(|f| f.code == code)
}

/// Valores de las 10 banderas; `None` = indicador no extraído (no entra en ese contraste).
pub fn flag_values(p: &IpoRiskProfile, entry: &IpoEntry, offer_price: Option<f64>) -> [Option<bool>; N_FLAGS] {
    let size = offer_size_usd(entry);
    [
        ind(p, "secondary_sale").and_then(|i| i.numeric).map(|v| v > 50.0),
        ind(p, "use_of_proceeds").map(|i| has_flag(i, "debt_repayment")),
        ind(p, "voting").map(|i| has_flag(i, "multi_vote")),
        ind(p, "controlled").map(|i| has_flag(i, "controlled_company")),
        ind(p, "dilution").and_then(|i| i.numeric).zip(offer_price).filter(|(_, pr)| *pr > 0.0).map(|(d, pr)| d > 0.0 && d / pr >= DILUTION_HIGH),
        ind(p, "float_offered").and_then(|i| i.numeric).map(|v| v < FLOAT_LOW_PCT),
        ind(p, "net_result").map(|i| has_flag(i, "net_loss")),
        ind(p, "lock_up").and_then(|i| i.numeric).map(|d| d < LOCKUP_SHORT_DAYS),
        size.map(|s| s < SIZE_MICRO_USD),
        size.map(|s| s >= SIZE_LARGE_USD),
    ]
}

/// El folleto vale para la OPV si es definitivo y se presentó entre 10 días antes y 3 después de la
/// primera cotización (su contenido está fijado en el precio: no hay información posterior).
pub fn prospectus_valid_at(p: &Prospectus, ipo_date: NaiveDate) -> bool {
    let d = (p.filed_on - ipo_date).num_days();
    !p.preliminary && (-10..=3).contains(&d)
}

// ---------------------------------------------------------------------------------------------
// Resultados
// ---------------------------------------------------------------------------------------------

/// Exceso (puntos porcentuales) a 3, 6 y 12 meses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcomes {
    /// Desde el precio de colocación frente a SPY (como la app).
    pub placement: [Option<f64>; 3],
    /// Desde el cierre del primer día frente a SPY entre ese día y la misma sesión de salida.
    pub day1: [Option<f64>; 3],
}

pub const HORIZONS: [&str; 3] = ["3m", "6m", "12m"];

fn date_of(c: &Candle) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp(c.t, 0).map(|d| d.date_naive())
}

/// Cálculo de los resultados. `perf` sale de `ipo::compute`; `spy` es la serie de SPY.
pub fn outcomes(perf: &IpoPerformance, spy: &DailySeries) -> Outcomes {
    let bench: Vec<(NaiveDate, f64)> = spy.candles.iter().zip(&spy.adj_close).filter_map(|(c, a)| date_of(c).map(|d| (d, *a))).collect();
    let at = |day: NaiveDate| bench.iter().rposition(|(d, _)| *d <= day).map(|k| bench[k].1);
    let mut o = Outcomes::default();
    for (k, h) in HORIZONS.iter().enumerate() {
        let Some(pt) = perf.points.iter().find(|p| p.horizon == *h) else { continue };
        o.placement[k] = pt.excess_pct;
        if let (Some(since), Some(end), Some(base)) = (pt.since_first_close_pct, pt.price_date.and_then(at), at(perf.ipo_date)) {
            o.day1[k] = Some(since - (end / base - 1.0) * 100.0);
        }
    }
    o
}

/// Motivo por el que una OPV no entra en el estudio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exclusion {
    NoProspectus,
    ProspectusOutOfWindow,
    NoYahooSeries,
    BadPerformance,
}

/// Una observación del estudio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Obs {
    pub symbol: String,
    pub ipo_date: NaiveDate,
    pub size_usd: Option<f64>,
    pub flags: [Option<bool>; N_FLAGS],
    pub outcomes: Outcomes,
}

/// Reconstruye el perfil de la OPV **con lo que existía en el precio** y calcula sus resultados.
pub fn build_obs(
    entry: &IpoEntry,
    prospectus: Option<&Prospectus>,
    stock: Option<&DailySeries>,
    spy: &DailySeries,
    now: DateTime<Utc>,
) -> Result<Obs, Exclusion> {
    let symbol = entry.symbol.clone().unwrap_or_default();
    let p = prospectus.ok_or(Exclusion::NoProspectus)?;
    if !prospectus_valid_at(p, entry.date) {
        return Err(Exclusion::ProspectusOutOfWindow);
    }
    let stock = stock.ok_or(Exclusion::NoYahooSeries)?;
    let offer = entry.price_low.ok_or(Exclusion::BadPerformance)?;
    let perf = compute(&PerfInput {
        symbol: &symbol,
        ipo_date: entry.date,
        offer_price: offer,
        offer_price_source: "calendar",
        series: stock,
        bench: spy,
        bench_name: "SPY",
        source_url: None,
        now,
    })
    .map_err(|_| Exclusion::BadPerformance)?;
    let profile = build_profile(ProfileInput { symbol: &symbol, entry: Some(entry), prospectus: Some(p), base_rate: None, now });
    Ok(Obs {
        symbol,
        ipo_date: entry.date,
        size_usd: offer_size_usd(entry),
        flags: flag_values(&profile, entry, Some(offer)),
        outcomes: outcomes(&perf, spy),
    })
}

// ---------------------------------------------------------------------------------------------
// Contrastes (estratificados por tramo de tamaño y con IC por conglomerados de mes)
// ---------------------------------------------------------------------------------------------

/// Tramos de tamaño para estratificar (0 = < 50 M, 1 = 50–250 M, 2 = ≥ 250 M).
pub fn tier_of(size_usd: f64) -> usize {
    usize::from(size_usd >= SIZE_MICRO_USD) + usize::from(size_usd >= SIZE_LARGE_USD)
}

/// Mínimo por grupo dentro de un tramo para que el tramo cuente en el estadístico estratificado.
pub const MIN_STRATUM_GROUP: usize = 5;
pub const CLUSTER_REPS: usize = 5_000;

/// Una observación para un contraste: bandera, resultado, tramo y mes de salida a bolsa.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub flag: bool,
    pub y: f64,
    pub tier: usize,
    /// Año × 12 + mes (conglomerado temporal).
    pub month: i32,
}

/// Filas de una bandera y un resultado. Si `stratified`, solo las OPV con tamaño conocido.
pub fn rows_of(obs: &[Obs], flag: usize, outcome: impl Fn(&Obs) -> Option<f64>, stratified: bool) -> Vec<Row> {
    use chrono::Datelike;
    obs.iter()
        .filter_map(|o| {
            let f = o.flags[flag]?;
            let y = outcome(o)?;
            let tier = match o.size_usd {
                Some(sz) => tier_of(sz),
                None if stratified => return None,
                None => 0,
            };
            Some(Row { flag: f, y, tier: if stratified { tier } else { 0 }, month: o.ipo_date.year() * 12 + o.ipo_date.month0() as i32 })
        })
        .collect()
}

type Strata = Vec<(Vec<f64>, Vec<f64>)>;

fn strata_of(rows: &[Row]) -> Strata {
    let mut st: Strata = vec![(Vec::new(), Vec::new()); 3];
    for r in rows {
        if r.flag { st[r.tier].0.push(r.y) } else { st[r.tier].1.push(r.y) }
    }
    st
}

/// Media ponderada por n de las diferencias de medianas (con − sin) de los tramos con al menos
/// `min_each` en cada grupo. Devuelve (estadístico, n con bandera, n sin bandera) de los tramos usados.
fn strat_stat(st: &Strata, min_each: usize) -> Option<(f64, usize, usize)> {
    let (mut num, mut den, mut n1, mut n0) = (0.0, 0.0, 0, 0);
    for (a, b) in st {
        if a.len() >= min_each && b.len() >= min_each && !a.is_empty() && !b.is_empty() {
            let w = (a.len() + b.len()) as f64;
            num += w * median_diff(a, b)?;
            den += w;
            n1 += a.len();
            n0 += b.len();
        }
    }
    (den > 0.0).then(|| (num / den, n1, n0))
}

/// p de dos colas: se permuta la bandera DENTRO de cada tramo.
pub fn strat_perm_p(rows: &[Row], min_each: usize, reps: usize, seed: u64) -> Option<f64> {
    let obs_stat = strat_stat(&strata_of(rows), min_each)?.0.abs();
    let mut rng = Rng::new(seed);
    let mut by_tier: Vec<Vec<f64>> = vec![Vec::new(); 3];
    let mut k_tier = [0usize; 3];
    for r in rows {
        by_tier[r.tier].push(r.y);
        k_tier[r.tier] += usize::from(r.flag);
    }
    let mut extreme = 0usize;
    for _ in 0..reps {
        let mut st: Strata = vec![(Vec::new(), Vec::new()); 3];
        for t in 0..3 {
            let ys = &mut by_tier[t];
            let k = k_tier[t];
            for i in 0..k.min(ys.len()) {
                let j = i + rng.below(ys.len() - i);
                ys.swap(i, j);
            }
            st[t] = (ys[..k].to_vec(), ys[k..].to_vec());
        }
        if strat_stat(&st, min_each).is_some_and(|(d, _, _)| d.abs() >= obs_stat - 1e-12) {
            extreme += 1;
        }
    }
    Some((extreme as f64 + 1.0) / (reps as f64 + 1.0))
}

fn percentile_ci(mut diffs: Vec<f64>, level: f64) -> Option<(f64, f64)> {
    if diffs.len() < 10 {
        return None;
    }
    diffs.sort_unstable_by(f64::total_cmp);
    let tail = (1.0 - level) / 2.0;
    let lo = ((diffs.len() as f64 * tail).floor() as usize).min(diffs.len() - 1);
    let hi = (((diffs.len() as f64) * (1.0 - tail)).ceil() as usize).saturating_sub(1).min(diffs.len() - 1);
    Some((diffs[lo], diffs[hi]))
}

/// IC bootstrap ordinario: remuestrea dentro de cada tramo y grupo.
pub fn strat_boot_ci(rows: &[Row], min_each: usize, reps: usize, seed: u64) -> Option<(f64, f64)> {
    let base = strata_of(rows);
    strat_stat(&base, min_each)?;
    let mut rng = Rng::new(seed);
    let mut diffs = Vec::with_capacity(reps);
    for _ in 0..reps {
        let st: Strata = base
            .iter()
            .map(|(a, b)| {
                let draw = |v: &Vec<f64>, rng: &mut Rng| -> Vec<f64> { (0..v.len()).map(|_| v[rng.below(v.len())]).collect() };
                (if a.is_empty() { Vec::new() } else { draw(a, &mut rng) }, if b.is_empty() { Vec::new() } else { draw(b, &mut rng) })
            })
            .collect();
        if let Some((d, _, _)) = strat_stat(&st, min_each) {
            diffs.push(d);
        }
    }
    percentile_ci(diffs, 0.95)
}

/// IC bootstrap por conglomerados: se remuestrean meses de salida a bolsa enteros (con todas sus OPV).
pub fn cluster_boot_ci(rows: &[Row], min_each: usize, reps: usize, seed: u64) -> Option<(f64, f64)> {
    let mut months: Vec<i32> = rows.iter().map(|r| r.month).collect();
    months.sort_unstable();
    months.dedup();
    if months.len() < 3 {
        return None;
    }
    let by_month: Vec<Vec<Row>> = months.iter().map(|m| rows.iter().filter(|r| r.month == *m).copied().collect()).collect();
    let mut rng = Rng::new(seed);
    let mut diffs = Vec::with_capacity(reps);
    for _ in 0..reps {
        let mut sample = Vec::with_capacity(rows.len());
        for _ in 0..by_month.len() {
            sample.extend_from_slice(&by_month[rng.below(by_month.len())]);
        }
        if let Some((d, _, _)) = strat_stat(&strata_of(&sample), min_each) {
            diffs.push(d);
        }
    }
    percentile_ci(diffs, 0.95)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contrast {
    pub flag: usize,
    /// ¿Contraste estratificado por tramo de tamaño? (V1–V8 sí; V9 y V10, el tamaño mismo, no).
    pub stratified: bool,
    /// n de cada grupo en los tramos usados por el estadístico principal.
    pub n1: usize,
    pub n0: usize,
    /// Estadístico principal: diferencia de medianas (estratificada o no, según `stratified`).
    pub diff: Option<f64>,
    /// IC 95 % bootstrap ordinario (dentro de tramo y grupo).
    pub ci: Option<(f64, f64)>,
    /// IC 95 % bootstrap por conglomerados de mes de salida a bolsa.
    pub ci_cluster: Option<(f64, f64)>,
    pub p: Option<f64>,
    pub p_holm: Option<f64>,
    /// Efecto mínimo detectable (pp), 80 % de potencia y α = 0,05/10 (aproximación sin estratificar).
    pub mde: Option<f64>,
    pub testable: bool,
    /// Descriptivo sin estratificar: n con / sin, medianas, diferencia, IC y p (sin ajustar).
    pub raw_n1: usize,
    pub raw_n0: usize,
    pub raw_median1: Option<f64>,
    pub raw_median0: Option<f64>,
    pub raw_diff: Option<f64>,
    pub raw_ci: Option<(f64, f64)>,
    pub raw_p: Option<f64>,
}

/// Banderas estratificadas por tamaño en el análisis principal (el tamaño mismo no se estratifica).
pub fn is_stratified(flag: usize) -> bool {
    flag < 8
}

/// Contrasta las 10 banderas para un resultado, con Holm sobre la familia.
pub fn contrasts(obs: &[Obs], outcome: impl Fn(&Obs) -> Option<f64> + Copy, seed: u64) -> Vec<Contrast> {
    let all: Vec<f64> = obs.iter().filter_map(outcome).collect();
    let sigma = robust_sigma(&all);
    let mut rows: Vec<Contrast> = (0..N_FLAGS)
        .map(|f| {
            let s = seed.wrapping_add(f as u64 * 7919);
            let strat = is_stratified(f);
            let main_rows = rows_of(obs, f, outcome, strat);
            let min_each = if strat { MIN_STRATUM_GROUP } else { 1 };
            let st = strat_stat(&strata_of(&main_rows), min_each);
            let (n1, n0) = st.map_or((0, 0), |x| (x.1, x.2));
            let testable = n1 >= MIN_GROUP && n0 >= MIN_GROUP;
            let (a, b) = groups(obs, f, outcome);
            let raw_testable = a.len() >= MIN_GROUP && b.len() >= MIN_GROUP;
            Contrast {
                flag: f,
                stratified: strat,
                n1,
                n0,
                diff: st.map(|x| x.0),
                ci: if testable { strat_boot_ci(&main_rows, min_each, BOOT_REPS, s) } else { None },
                ci_cluster: if testable { cluster_boot_ci(&main_rows, min_each, CLUSTER_REPS, s ^ 0x1357) } else { None },
                p: if testable { strat_perm_p(&main_rows, min_each, PERM_REPS, s ^ 0xABCD) } else { None },
                p_holm: None,
                mde: if testable { sigma.map(|sg| mde(sg, n1, n0, 0.05 / N_FLAGS as f64)) } else { None },
                testable,
                raw_n1: a.len(),
                raw_n0: b.len(),
                raw_median1: median(&a),
                raw_median0: median(&b),
                raw_diff: median_diff(&a, &b),
                raw_ci: if raw_testable { bootstrap_ci(&a, &b, BOOT_REPS, s, 0.95) } else { None },
                raw_p: if raw_testable { permutation_p(&a, &b, PERM_REPS, s ^ 0xABCD) } else { None },
            }
        })
        .collect();
    let adj = holm(&rows.iter().map(|r| r.p).collect::<Vec<_>>());
    for (r, a) in rows.iter_mut().zip(adj) {
        r.p_holm = a;
    }
    rows
}

/// Grupos (con bandera, sin bandera) sin estratificar.
pub fn groups(obs: &[Obs], flag: usize, outcome: impl Fn(&Obs) -> Option<f64>) -> (Vec<f64>, Vec<f64>) {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for o in obs {
        if let (Some(f), Some(y)) = (o.flags[flag], outcome(o)) {
            if f { a.push(y) } else { b.push(y) }
        }
    }
    (a, b)
}

/// Referencia de azar: por cada bandera, asigna 200 vectores aleatorios con la misma prevalencia
/// (dentro de cada tramo para V1–V8, como el análisis principal) y devuelve la proporción con p
/// ajustado (Holm sobre la familia) < 0,05.
pub fn placebo_rate(obs: &[Obs], outcome: impl Fn(&Obs) -> Option<f64> + Copy, seed: u64) -> f64 {
    let mut rng = Rng::new(seed);
    let base: Vec<(usize, Vec<Row>)> = (0..N_FLAGS)
        .map(|f| (f, rows_of(obs, f, outcome, is_stratified(f))))
        .filter(|(f, r)| {
            let m = if is_stratified(*f) { MIN_STRATUM_GROUP } else { 1 };
            strat_stat(&strata_of(r), m).is_some_and(|x| x.1 >= MIN_GROUP && x.2 >= MIN_GROUP)
        })
        .collect();
    if base.is_empty() {
        return f64::NAN;
    }
    let (mut hits, mut total) = (0usize, 0usize);
    for _ in 0..PLACEBOS {
        let mut ps = Vec::new();
        for (f, rows) in &base {
            // Baraja la bandera dentro de cada tramo (o globalmente si no se estratifica).
            let mut shuffled = rows.clone();
            for t in 0..3 {
                let idx: Vec<usize> = (0..shuffled.len()).filter(|&i| shuffled[i].tier == t).collect();
                let flags: Vec<bool> = idx.iter().map(|&i| shuffled[i].flag).collect();
                let mut order: Vec<usize> = (0..flags.len()).collect();
                for i in 0..order.len() {
                    let j = i + rng.below(order.len() - i);
                    order.swap(i, j);
                }
                for (slot, &o) in idx.iter().zip(&order) {
                    shuffled[*slot].flag = flags[o];
                }
            }
            let m = if is_stratified(*f) { MIN_STRATUM_GROUP } else { 1 };
            ps.push(strat_perm_p(&shuffled, m, PLACEBO_REPS, rng.next_u64()));
        }
        for p in holm(&ps).into_iter().flatten() {
            total += 1;
            if p < 0.05 {
                hits += 1;
            }
        }
    }
    hits as f64 / total.max(1) as f64
}

/// Coeficiente phi entre dos banderas (OPV con ambas conocidas) y n de ese solape.
pub fn phi(obs: &[Obs], a: usize, b: usize) -> (Option<f64>, usize) {
    let (mut n11, mut n10, mut n01, mut n00) = (0f64, 0f64, 0f64, 0f64);
    for o in obs {
        if let (Some(x), Some(y)) = (o.flags[a], o.flags[b]) {
            match (x, y) {
                (true, true) => n11 += 1.0,
                (true, false) => n10 += 1.0,
                (false, true) => n01 += 1.0,
                (false, false) => n00 += 1.0,
            }
        }
    }
    let n = (n11 + n10 + n01 + n00) as usize;
    let den = ((n11 + n10) * (n01 + n00) * (n11 + n01) * (n10 + n00)).sqrt();
    ((den > 0.0).then(|| (n11 * n00 - n10 * n01) / den), n)
}

/// Regla de decisión a priori. `primary` = contraste a 6 m desde colocación; `m3`, `m12` = a 3 y 12 m;
/// `day1_6` = a 6 m desde el cierre del día 1. Devuelve (se asocia, aprovechable por un inversor normal).
/// El criterio 2 exige que **ambos** IC (ordinario y por conglomerados de mes) excluyan 0.
pub fn decide(primary: &Contrast, m3: &Contrast, m12: &Contrast, day1_6: &Contrast) -> (bool, bool) {
    let sign = |c: &Contrast| c.diff.map(|d| d.signum());
    let excl = |ci: Option<(f64, f64)>| ci.is_some_and(|(lo, hi)| lo > 0.0 || hi < 0.0);
    let both = |c: &Contrast| excl(c.ci) && excl(c.ci_cluster);
    let same = |c: &Contrast| !c.testable || sign(c) == sign(primary);
    let assoc = primary.testable && primary.p_holm.is_some_and(|p| p < 0.05) && both(primary) && same(m3) && same(m12);
    let usable = assoc && day1_6.testable && excl(day1_6.ci) && sign(day1_6) == sign(primary);
    (assoc, usable)
}

// ---------------------------------------------------------------------------------------------
// Serie congelada
// ---------------------------------------------------------------------------------------------

/// `DailySeries` serializable (la de Yahoo no lo es).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenSeries {
    pub candles: Vec<Candle>,
    pub adj_close: Vec<f64>,
    pub splits: Vec<(i64, f64, f64)>,
}

impl From<&DailySeries> for FrozenSeries {
    fn from(s: &DailySeries) -> Self {
        Self { candles: s.candles.clone(), adj_close: s.adj_close.clone(), splits: s.splits.clone() }
    }
}

impl From<FrozenSeries> for DailySeries {
    fn from(s: FrozenSeries) -> Self {
        DailySeries { candles: s.candles, adj_close: s.adj_close, splits: s.splits }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_reproducible_and_spread() {
        let (mut a, mut b) = (Rng::new(SEED), Rng::new(SEED));
        let xs: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        assert_eq!(xs, (0..5).map(|_| b.next_u64()).collect::<Vec<_>>());
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn holm_adjusts_monotonically_and_skips_none() {
        // p = 0,01, 0,04, 0,03, 0,005 con m = 4 → ordenados 0,005·4, 0,01·3, 0,03·2, 0,04·1 con máximo acumulado.
        let adj = holm(&[Some(0.01), Some(0.04), Some(0.03), Some(0.005)]);
        let want = [0.03, 0.06, 0.06, 0.02];
        for (a, w) in adj.iter().zip(want) {
            assert!((a.unwrap() - w).abs() < 1e-12, "{adj:?}");
        }
        let adj = holm(&[Some(0.5), None, Some(0.4)]);
        assert_eq!(adj[1], None);
        assert!((adj[2].unwrap() - 0.8).abs() < 1e-12 && (adj[0].unwrap() - 0.8).abs() < 1e-12);
        assert!(holm(&[Some(0.9), Some(0.9)]).iter().all(|p| p.unwrap() <= 1.0));
    }

    #[test]
    fn bootstrap_and_permutation_are_seeded_and_detect_a_shift() {
        let mut r = Rng::new(7);
        let noise = |r: &mut Rng| (r.next_u64() % 1000) as f64 / 100.0; // 0..10
        let a: Vec<f64> = (0..60).map(|_| noise(&mut r) + 6.0).collect();
        let b: Vec<f64> = (0..80).map(|_| noise(&mut r)).collect();
        let ci1 = bootstrap_ci(&a, &b, 2000, 11, 0.95).unwrap();
        let ci2 = bootstrap_ci(&a, &b, 2000, 11, 0.95).unwrap();
        assert_eq!(ci1, ci2, "misma semilla, mismo IC");
        assert_ne!(ci1, bootstrap_ci(&a, &b, 2000, 12, 0.95).unwrap());
        assert!(ci1.0 > 3.0 && ci1.1 < 9.0, "{ci1:?}");
        let p = permutation_p(&a, &b, 2000, 3).unwrap();
        assert_eq!(p, permutation_p(&a, &b, 2000, 3).unwrap());
        assert!(p < 0.01, "{p}");
        // Sin diferencia real, p grande y el IC contiene 0.
        // Dos grupos entrelazados de la misma distribución (pares e impares): sin diferencia real.
        let c: Vec<f64> = (0..70).map(|k| f64::from(k * 2)).collect();
        let d: Vec<f64> = (0..70).map(|k| f64::from(k * 2 + 1)).collect();
        assert!(permutation_p(&c, &d, 2000, 3).unwrap() > 0.05);
        let ci = bootstrap_ci(&c, &d, 2000, 5, 0.95).unwrap();
        assert!(ci.0 < 0.0 && ci.1 > 0.0, "{ci:?}");
    }

    #[test]
    fn inverse_normal_and_mde() {
        assert!((inv_norm(0.975) - 1.959964).abs() < 1e-5);
        assert!((inv_norm(0.80) - 0.841621).abs() < 1e-5);
        assert!((inv_norm(0.5)).abs() < 1e-9);
        // σ = 30, n = 50 y 100, α = 0,005: (2,807 + 0,842) · √(se1² + se0²).
        let want = (inv_norm(0.9975) + inv_norm(0.8)) * ((1.2533f64 * 30.0 / 50f64.sqrt()).powi(2) + (1.2533f64 * 30.0 / 100f64.sqrt()).powi(2)).sqrt();
        assert!((mde(30.0, 50, 100, 0.005) - want).abs() < 1e-9);
        assert!(mde(30.0, 50, 100, 0.005) > mde(30.0, 200, 400, 0.005), "más muestra, menos MDE");
    }

    #[test]
    fn median_and_diff() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
        assert_eq!(median_diff(&[5.0, 7.0], &[1.0]), Some(5.0));
        assert!((robust_sigma(&[1.0, 2.0, 3.0, 4.0, 100.0]).unwrap() - 1.4826).abs() < 1e-9);
    }
}
