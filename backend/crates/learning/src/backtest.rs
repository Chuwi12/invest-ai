//! Backtest de los factores de precio de `ai::scoring` (técnico, momentum, riesgo-precio y su
//! compuesto) sobre el exceso de rentabilidad a 1/4/12 semanas. Diseño fijado ANTES de ejecutar en
//! `docs/notes/backtest.md`; este módulo es lógica pura (sin red ni BD) y determinista.
//!
//! Sin anticipación: la señal de un viernes usa solo velas con día UTC ≤ ese viernes
//! ([`signals_at`]); la rentabilidad futura empieza en el SIGUIENTE cierre ([`forward_excess`]).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use domain::{AssetClass, AssetId, AssetSnapshot, AssetSummary, Candle, Verdict};
use serde::{Deserialize, Serialize};

use crate::{
    rules::{self, BenchKind, Computation},
    stats::{self, Rng},
};

pub const DAY: i64 = 86_400;
/// Horizontes en semanas (los mismos que el resolutor de la app).
pub const HORIZONS: [u32; 3] = rules::HORIZONS_WEEKS;
/// Nombres de las señales, en el orden de [`Obs::sig`].
pub const SIGNALS: [&str; 5] = ["technical", "momentum", "risk", "composite", "mom12_1"];
pub const SIG_TECHNICAL: usize = 0;
pub const SIG_MOMENTUM: usize = 1;
pub const SIG_RISK: usize = 2;
pub const SIG_COMPOSITE: usize = 3;
pub const SIG_MOM12_1: usize = 4;
/// Historia mínima (días naturales) antes de la fecha para puntuar: 52 semanas + rentabilidad a 1 año.
pub const MIN_HISTORY_DAYS: i64 = 400;
/// Ventana de velas que ve la puntuación (como `Range::Y2` en la app).
pub const WINDOW_DAYS: i64 = 730;
/// Longitud (semanas) del bloque del bootstrap por bloques.
pub const BLOCK_WEEKS: usize = 13;

fn day(t: i64) -> i64 {
    t.div_euclid(DAY)
}

pub fn day_of(date: NaiveDate) -> i64 {
    date.signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default())
        .num_days()
}

pub fn date_of(day: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .unwrap_or_default()
        .checked_add_signed(chrono::Duration::days(day))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Datos
// ---------------------------------------------------------------------------------------------

/// Una serie tal como se congela en disco.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesFile {
    /// `clase:SÍMBOLO` (formato de `AssetId`).
    pub id: String,
    /// Velas diarias ajustadas por escisiones (no por dividendos).
    pub candles: Vec<Candle>,
    /// Cierre ajustado por dividendos, alineado con `candles`.
    pub adj_close: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetFile {
    /// Instante (Unix s) de la descarga: decide qué velas finales están completas.
    pub fetched_at: i64,
    pub from: i64,
    pub series: Vec<SeriesFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Etf,
    Stock,
    Crypto,
}

#[derive(Debug, Clone)]
pub struct Series {
    pub id: AssetId,
    pub group: Group,
    /// Velas para la puntuación (como las ve la app).
    pub raw: Vec<Candle>,
    /// Mismas fechas con `c` = cierre ajustado por dividendos: para rentabilidades y momentum 12-1.
    pub adj: Vec<Candle>,
}

impl Series {
    pub fn new(id: AssetId, raw: Vec<Candle>, adj_close: &[f64]) -> Result<Self, String> {
        if raw.len() != adj_close.len() {
            return Err(format!("{id}: velas y cierres ajustados desalineados"));
        }
        let group = match id.class {
            AssetClass::Stock => Group::Stock,
            AssetClass::Etf | AssetClass::Fund => Group::Etf,
            AssetClass::Crypto => Group::Crypto,
            c => return Err(format!("{id}: clase {c:?} fuera del backtest")),
        };
        if raw.windows(2).any(|w| w[1].t <= w[0].t) {
            return Err(format!("{id}: velas sin orden estricto"));
        }
        let adj = raw
            .iter()
            .zip(adj_close)
            .map(|(c, a)| Candle { c: *a, ..*c })
            .collect();
        Ok(Self { id, group, raw, adj })
    }

    pub fn from_file(f: &SeriesFile) -> Result<Self, String> {
        let id: AssetId = f.id.parse().map_err(|e| format!("{}: {e}", f.id))?;
        Self::new(id, f.candles.clone(), &f.adj_close)
    }
}

pub fn load_dataset(path: &std::path::Path) -> Result<(DatasetFile, Vec<Series>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ds: DatasetFile = serde_json::from_str(&text).map_err(|e| format!("JSON: {e}"))?;
    let series = ds
        .series
        .iter()
        .map(Series::from_file)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ds, series))
}

// ---------------------------------------------------------------------------------------------
// Señales sin anticipación
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signals {
    /// En el orden de [`SIGNALS`]; todo «más alto = más favorable» (el riesgo, ya invertido).
    pub v: [f64; 5],
    /// Veredicto cuantitativo provisional de `score_en` sobre el snapshot de precios.
    pub verdict: Verdict,
}

/// Último cierre (ajustado) con día ≤ `d` y a como mucho `tol` días.
fn close_on_or_before(c: &[Candle], d: i64, tol: i64) -> Option<f64> {
    let i = c.partition_point(|x| day(x.t) <= d);
    let k = c.get(i.checked_sub(1)?)?;
    (d - day(k.t) <= tol && k.c > 0.0 && k.c.is_finite()).then_some(k.c)
}

/// Señales de un activo en la fecha `date_day` (día UTC), usando SOLO velas con día ≤ `date_day`.
/// `None` si no hay historia suficiente, la última vela es vieja o falta algún componente.
pub fn signals_at(s: &Series, date_day: i64) -> Option<Signals> {
    let end = s.raw.partition_point(|c| day(c.t) <= date_day);
    let last = *s.raw[..end].last()?;
    if date_day - day(last.t) > 4 {
        return None;
    }
    if date_day - day(s.raw.first()?.t) < MIN_HISTORY_DAYS {
        return None;
    }
    let start = s.raw[..end].partition_point(|c| c.t < last.t - WINDOW_DAYS * DAY);
    let window = &s.raw[start..end];
    let ind = market::indicators::compute_at(window, (date_day + 2) * DAY);
    let snap = AssetSnapshot {
        summary: AssetSummary {
            id: s.id.clone(),
            symbol: s.id.symbol.clone(),
            name: String::new(),
            class: s.id.class,
            exchange: None,
            currency: None,
            country: None,
            source: "backtest".into(),
            source_url: None,
        },
        quote: None,
        fundamentals: None,
        indicators: Some(ind),
        news: Vec::new(),
        macro_context: None,
        missing: Vec::new(),
        generated_at: DateTime::<Utc>::from_timestamp((date_day + 1) * DAY - 1, 0)?,
    };
    let sc = ai::scoring::score_en(&snap);
    let (t, m, r) = (sc.scores.technical?, sc.scores.momentum?, sc.scores.risk?);
    // Momentum 12-1 estándar: cierre ajustado de hace 1 mes sobre el de hace 12 meses.
    let (a, b) = (
        close_on_or_before(&s.adj, date_day - 30, 5)?,
        close_on_or_before(&s.adj, date_day - 365, 5)?,
    );
    Some(Signals {
        v: [
            f64::from(t),
            f64::from(m),
            100.0 - f64::from(r),
            f64::from(sc.scores.overall),
            a / b - 1.0,
        ],
        verdict: sc.verdict,
    })
}

/// Exceso de rentabilidad total a `h` semanas con entrada en el primer cierre POSTERIOR a
/// `date_day` (la señal usa el cierre de ese día, así que no se puede operar a él). Usa las
/// mismas reglas que el resolutor de la app (`rules::compute`) sobre cierres ajustados.
pub fn forward_excess(
    asset_adj: &[Candle],
    bench_adj: &[Candle],
    date_day: i64,
    h: u32,
    now_s: i64,
) -> Option<f64> {
    match rules::compute(
        asset_adj,
        Some(bench_adj),
        (date_day + 1) * DAY * 1000,
        h,
        now_s,
    ) {
        Computation::Resolved(r) => Some(r.excess),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AssetInfo {
    pub id: String,
    pub group: Group,
}

#[derive(Debug, Clone)]
pub struct Obs {
    /// Índice en [`Panel::assets`].
    pub asset: usize,
    pub sig: [f64; 5],
    pub verdict: Verdict,
    /// Exceso a 1/4/12 semanas (`None` si aún no madura o faltan datos).
    pub excess: [Option<f64>; 3],
}

#[derive(Debug, Clone)]
pub struct Week {
    /// Posición en la rejilla de viernes (consecutiva, también con secciones vacías).
    pub idx: usize,
    /// Día UTC del viernes.
    pub day: i64,
    pub obs: Vec<Obs>,
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub assets: Vec<AssetInfo>,
    pub weeks: Vec<Week>,
}

/// Universo de análisis (ver el documento de diseño).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Universe {
    /// ETF + acciones + ADR.
    Equity,
    Etf,
    Stock,
    Crypto,
}

impl Universe {
    pub fn contains(self, g: Group) -> bool {
        match self {
            Self::Equity => g != Group::Crypto,
            Self::Etf => g == Group::Etf,
            Self::Stock => g == Group::Stock,
            Self::Crypto => g == Group::Crypto,
        }
    }
    /// Mínimo de activos en la sección cruzada para calcular un IC o formar quintiles.
    pub fn min_n(self) -> usize {
        if self == Self::Crypto { 6 } else { 15 }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::Etf => "etf",
            Self::Stock => "stock",
            Self::Crypto => "crypto",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PanelConfig {
    /// Primer viernes (día UTC); se ajusta al viernes igual o posterior.
    pub start_day: i64,
    /// Último día a considerar.
    pub end_day: i64,
    /// «Ahora» de la descarga (Unix s).
    pub now_s: i64,
}

/// Viernes entre `start` y `end` (días UTC, inclusive).
pub fn fridays(start: i64, end: i64) -> Vec<i64> {
    // 1970-01-01 fue jueves: día % 7 == 1 → viernes.
    let mut d = start;
    while d.rem_euclid(7) != 1 {
        d += 1;
    }
    let mut out = Vec::new();
    while d <= end {
        out.push(d);
        d += 7;
    }
    out
}

/// Construye el panel semanal. Excluye los activos que son su propio benchmark (SPY, BTC).
pub fn build_panel(series: &[Series], cfg: &PanelConfig) -> Result<Panel, String> {
    let mut refs: Vec<(usize, usize)> = Vec::new(); // (serie, benchmark)
    let mut assets = Vec::new();
    for (i, s) in series.iter().enumerate() {
        let Some(b) = rules::benchmark_for(&s.id) else { continue };
        if b.kind == BenchKind::Absolute {
            continue;
        }
        let Some(bid) = b.id else { continue };
        let Some(bi) = series.iter().position(|x| x.id == bid) else {
            return Err(format!("falta el benchmark {bid} de {}", s.id));
        };
        refs.push((i, bi));
        assets.push(AssetInfo { id: s.id.to_string(), group: s.group });
    }
    let days = fridays(cfg.start_day, cfg.end_day);
    let n_thr = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16);
    let chunk = days.len().div_ceil(n_thr).max(1);
    let build_week = |idx: usize, d: i64| {
        let mut obs = Vec::new();
        for (a, &(si, bi)) in refs.iter().enumerate() {
            let Some(sg) = signals_at(&series[si], d) else { continue };
            let mut excess = [None; 3];
            for (k, h) in HORIZONS.iter().enumerate() {
                excess[k] = forward_excess(&series[si].adj, &series[bi].adj, d, *h, cfg.now_s);
            }
            obs.push(Obs { asset: a, sig: sg.v, verdict: sg.verdict, excess });
        }
        Week { idx, day: d, obs }
    };
    let mut weeks: Vec<Week> = Vec::with_capacity(days.len());
    std::thread::scope(|sc| {
        let handles: Vec<_> = days
            .chunks(chunk)
            .enumerate()
            .map(|(ci, ds)| {
                let bw = &build_week;
                sc.spawn(move || {
                    ds.iter()
                        .enumerate()
                        .map(|(j, &d)| bw(ci * chunk + j, d))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            if let Ok(v) = h.join() {
                weeks.extend(v);
            }
        }
    });
    weeks.sort_by_key(|w| w.idx);
    Ok(Panel { assets, weeks })
}

// ---------------------------------------------------------------------------------------------
// Estadística: Newey-West, IC, subperiodos
// ---------------------------------------------------------------------------------------------

/// Media y error típico de Newey-West (núcleo de Bartlett, rezago `lag`).
pub fn newey_west(x: &[f64], lag: usize) -> Option<(f64, f64)> {
    let n = x.len();
    if n < lag + 3 {
        return None;
    }
    let nf = n as f64;
    let mean = x.iter().sum::<f64>() / nf;
    let d: Vec<f64> = x.iter().map(|v| v - mean).collect();
    let mut omega = d.iter().map(|v| v * v).sum::<f64>() / nf;
    for k in 1..=lag {
        let g: f64 = (k..n).map(|i| d[i] * d[i - k]).sum::<f64>() / nf;
        omega += 2.0 * (1.0 - k as f64 / (lag as f64 + 1.0)) * g;
    }
    Some((mean, (omega.max(1e-30) / nf).sqrt()))
}

#[derive(Debug, Clone, Copy)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub se: f64,
    pub t: f64,
    /// Media ± 1,96·se (Newey-West).
    pub lo: f64,
    pub hi: f64,
    /// IC 95 % por bootstrap por bloques de `BLOCK_WEEKS` semanas.
    pub boot: Option<(f64, f64)>,
}

/// Resume una serie semanal `(semana, valor)`: media, Newey-West con rezago `lag` y bootstrap por bloques.
pub fn summarize(points: &[(usize, f64)], lag: usize, boot: bool) -> Option<Summary> {
    let vals: Vec<f64> = points.iter().map(|p| p.1).collect();
    let (mean, se) = newey_west(&vals, lag)?;
    let boot = if boot {
        let clusters: Vec<u64> = points.iter().map(|p| (p.0 / BLOCK_WEEKS) as u64).collect();
        stats::mean_ci(&vals, &clusters, 2000, 20_261_015)
    } else {
        None
    };
    Some(Summary {
        n: vals.len(),
        mean,
        se,
        t: mean / se,
        lo: mean - 1.96 * se,
        hi: mean + 1.96 * se,
        boot,
    })
}

/// Subperiodos a priori, por año de la fecha de evaluación: (nombre, primer año, último año).
pub const PERIODS: [(&str, i32, i32); 3] =
    [("2018-2020", 2018, 2020), ("2021-2023", 2021, 2023), ("2024-hoy", 2024, 9999)];

pub fn year_of(day: i64) -> i32 {
    date_of(day).year()
}

/// IC de Spearman por semana entre `score` y el exceso a `HORIZONS[hi]`. `(semana, día, IC)`.
pub fn ic_series(
    panel: &Panel,
    u: Universe,
    hi: usize,
    score: &dyn Fn(&Week, &Obs) -> f64,
) -> Vec<(usize, i64, f64)> {
    let mut out = Vec::new();
    let (mut x, mut y) = (Vec::new(), Vec::new());
    for w in &panel.weeks {
        x.clear();
        y.clear();
        for o in &w.obs {
            if !u.contains(panel.assets[o.asset].group) {
                continue;
            }
            if let Some(e) = o.excess[hi] {
                x.push(score(w, o));
                y.push(e);
            }
        }
        if x.len() >= u.min_n()
            && let Some(ic) = stats::spearman(&x, &y)
        {
            out.push((w.idx, w.day, ic));
        }
    }
    out
}

pub fn in_period(day: i64, p: usize) -> bool {
    let y = year_of(day);
    y >= PERIODS[p].1 && y <= PERIODS[p].2
}

/// Puntuación uniforme determinista por (semilla, semana, activo): la «puntuación aleatoria».
pub fn random_score(seed: u64, week: usize, asset: usize) -> f64 {
    Rng::new(seed.wrapping_mul(0x1_0000_0001).wrapping_add(((week as u64) << 20) | asset as u64))
        .uniform()
}

// ---------------------------------------------------------------------------------------------
// Cartera top-vs-bottom quintil
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct CostModel {
    /// Coste por operación (compra o venta), en puntos básicos del nominal negociado.
    pub one_way_bps: f64,
    /// Coste anual de mantener el corto (préstamo/financiación), fracción.
    pub borrow_annual: f64,
}

impl CostModel {
    /// Declarado en el diseño: 10 pb y 0,5 %/año en renta variable; 25 pb y 10 %/año en cripto.
    pub fn default_for(u: Universe) -> Self {
        if u == Universe::Crypto {
            Self { one_way_bps: 25.0, borrow_annual: 0.10 }
        } else {
            Self { one_way_bps: 10.0, borrow_annual: 0.005 }
        }
    }
}

/// Resultado de una cohorte (una semana de entrada, mantenida h semanas).
#[derive(Debug, Clone, Copy)]
pub struct PortPoint {
    pub week: usize,
    pub day: i64,
    /// Largo − corto, bruto.
    pub gross: f64,
    /// Largo − corto, tras costes de operación y préstamo.
    pub net: f64,
    /// Quintil superior − benchmark, neto de costes de operación (sin corto).
    pub long_only_net: f64,
    /// Fracción media sustituida en las dos patas (0 = nada, 1 = todo).
    pub turnover: f64,
}

/// Cartera largo (quintil superior) − corto (inferior) equiponderada. Cada semana abre una
/// cohorte que se mantiene `HORIZONS[hi]` semanas; la rotación se mide contra la cohorte de
/// h semanas antes (la anterior de la misma cadena de rebalanceo).
pub fn long_short(
    panel: &Panel,
    u: Universe,
    hi: usize,
    score: &dyn Fn(&Week, &Obs) -> f64,
    cost: CostModel,
) -> Vec<PortPoint> {
    let h = HORIZONS[hi] as usize;
    let mut held: HashMap<usize, (Vec<usize>, Vec<usize>)> = HashMap::new();
    let mut out = Vec::new();
    let c = cost.one_way_bps / 1e4;
    for w in &panel.weeks {
        let mut cand: Vec<(f64, usize, f64)> = w
            .obs
            .iter()
            .filter(|o| u.contains(panel.assets[o.asset].group))
            .filter_map(|o| o.excess[hi].map(|e| (score(w, o), o.asset, e)))
            .collect();
        if cand.len() < u.min_n() {
            continue;
        }
        cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let k = (cand.len() / 5).max(1);
        let short = &cand[..k];
        let long = &cand[cand.len() - k..];
        let mean = |s: &[(f64, usize, f64)]| s.iter().map(|x| x.2).sum::<f64>() / s.len() as f64;
        let (ml, ms) = (mean(long), mean(short));
        let (lset, sset): (Vec<usize>, Vec<usize>) =
            (long.iter().map(|x| x.1).collect(), short.iter().map(|x| x.1).collect());
        let repl = |now: &[usize], prev: Option<&Vec<usize>>| match prev {
            Some(p) => now.iter().filter(|a| !p.contains(a)).count() as f64 / now.len() as f64,
            None => 1.0,
        };
        let prev = w.idx.checked_sub(h).and_then(|i| held.get(&i));
        let (fl, fs) = (repl(&lset, prev.map(|p| &p.0)), repl(&sset, prev.map(|p| &p.1)));
        let trade = c * 2.0 * (fl + fs);
        let borrow = cost.borrow_annual * h as f64 / 52.0;
        out.push(PortPoint {
            week: w.idx,
            day: w.day,
            gross: ml - ms,
            net: ml - ms - trade - borrow,
            long_only_net: ml - c * 2.0 * fl,
            turnover: (fl + fs) / 2.0,
        });
        held.insert(w.idx, (lset, sset));
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub struct PortSummary {
    pub n: usize,
    /// Media anualizada (aritmética: media por cohorte × 52/h).
    pub ann: f64,
    pub ann_vol: f64,
    pub sharpe: f64,
    /// t de Newey-West de la media por cohorte (rezago h).
    pub t: f64,
    pub turnover: f64,
}

/// Resume una serie de rentabilidades por cohorte de h semanas.
pub fn summarize_port(vals: &[f64], turnover: f64, h: u32) -> Option<PortSummary> {
    let (mean, se) = newey_west(vals, h as usize)?;
    let n = vals.len() as f64;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    let f = 52.0 / f64::from(h);
    let ann_vol = var.sqrt() * f.sqrt();
    Some(PortSummary {
        n: vals.len(),
        ann: mean * f,
        ann_vol,
        sharpe: if ann_vol > 0.0 { mean * f / ann_vol } else { f64::NAN },
        t: mean / se,
        turnover,
    })
}

/// Exceso medio por quintil (0 = el más bajo) de la señal, promediado sobre semanas.
pub fn quintile_means(
    panel: &Panel,
    u: Universe,
    hi: usize,
    score: &dyn Fn(&Week, &Obs) -> f64,
) -> Option<[f64; 5]> {
    let (mut sum, mut cnt) = ([0.0; 5], [0usize; 5]);
    for w in &panel.weeks {
        let mut cand: Vec<(f64, f64)> = w
            .obs
            .iter()
            .filter(|o| u.contains(panel.assets[o.asset].group))
            .filter_map(|o| o.excess[hi].map(|e| (score(w, o), e)))
            .collect();
        if cand.len() < u.min_n() {
            continue;
        }
        cand.sort_by(|a, b| a.0.total_cmp(&b.0));
        let n = cand.len();
        let mut tmp = ([0.0; 5], [0usize; 5]);
        for (i, c) in cand.iter().enumerate() {
            let q = (i * 5 / n).min(4);
            tmp.0[q] += c.1;
            tmp.1[q] += 1;
        }
        for q in 0..5 {
            sum[q] += tmp.0[q] / tmp.1[q].max(1) as f64;
            cnt[q] += 1;
        }
    }
    (cnt[0] > 0).then(|| std::array::from_fn(|q| sum[q] / cnt[q] as f64))
}

// ---------------------------------------------------------------------------------------------
// Puntuación aleatoria: cuánto IC sale por azar
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RandomRef {
    pub seeds: usize,
    /// Media entre semillas del IC medio, su desviación y percentiles 2,5/97,5.
    pub ic_mean: f64,
    pub ic_sd: f64,
    pub ic_p025: f64,
    pub ic_p975: f64,
    /// Fracción de semillas con |t_NW| > 1,96 y > 2,87.
    pub frac_t196: f64,
    pub frac_t287: f64,
    /// Rentabilidad anualizada del largo-corto aleatorio: bruta y neta (media y desviación entre semillas).
    pub gross_mean: f64,
    pub gross_sd: f64,
    pub net_mean: f64,
    pub net_sd: f64,
}

pub fn random_reference(
    panel: &Panel,
    u: Universe,
    hi: usize,
    seeds: std::ops::RangeInclusive<u64>,
    cost: CostModel,
) -> Option<RandomRef> {
    let h = HORIZONS[hi];
    let (mut means, mut ts, mut gross, mut net) = (vec![], vec![], vec![], vec![]);
    for seed in seeds {
        let f = move |w: &Week, o: &Obs| random_score(seed, w.idx, o.asset);
        let ic: Vec<(usize, f64)> = ic_series(panel, u, hi, &f).iter().map(|p| (p.0, p.2)).collect();
        let s = summarize(&ic, h as usize, false)?;
        means.push(s.mean);
        ts.push(s.t);
        let ps = long_short(panel, u, hi, &f, cost);
        let g: Vec<f64> = ps.iter().map(|p| p.gross).collect();
        let n: Vec<f64> = ps.iter().map(|p| p.net).collect();
        let k = 52.0 / f64::from(h);
        gross.push(g.iter().sum::<f64>() / g.len().max(1) as f64 * k);
        net.push(n.iter().sum::<f64>() / n.len().max(1) as f64 * k);
    }
    let sd = |v: &[f64]| {
        let m = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() as f64 - 1.0).max(1.0)).sqrt()
    };
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let mut sorted = means.clone();
    sorted.sort_by(f64::total_cmp);
    let frac = |thr: f64| ts.iter().filter(|t| t.abs() > thr).count() as f64 / ts.len() as f64;
    Some(RandomRef {
        seeds: means.len(),
        ic_mean: mean(&means),
        ic_sd: sd(&means),
        ic_p025: stats::quantile_sorted(&sorted, 0.025),
        ic_p975: stats::quantile_sorted(&sorted, 0.975),
        frac_t196: frac(1.96),
        frac_t287: frac(2.87),
        gross_mean: mean(&gross),
        gross_sd: sd(&gross),
        net_mean: mean(&net),
        net_sd: sd(&net),
    })
}

// ---------------------------------------------------------------------------------------------
// Acierto por veredicto
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VerdictRow {
    pub verdict: Verdict,
    pub n: usize,
    pub hit: f64,
    /// Acierto que tendría ese veredicto aplicado al azar (a toda la muestra).
    pub base: f64,
    /// `hit − base`.
    pub lift: f64,
    /// IC 95 % del `lift` por bootstrap por bloques de semanas.
    pub lift_ci: Option<(f64, f64)>,
}

pub const VERDICTS: [Verdict; 6] = [
    Verdict::Buy,
    Verdict::Accumulate,
    Verdict::Hold,
    Verdict::Wait,
    Verdict::Reduce,
    Verdict::Avoid,
];

pub fn verdict_table(panel: &Panel, u: Universe, hi: usize, reps: usize, seed: u64) -> Vec<VerdictRow> {
    let h = HORIZONS[hi];
    let block = if h >= 12 { 2 * BLOCK_WEEKS } else { BLOCK_WEEKS };
    let (mut verdicts, mut ex, mut clusters) = (vec![], vec![], vec![]);
    for w in &panel.weeks {
        for o in &w.obs {
            if !u.contains(panel.assets[o.asset].group) {
                continue;
            }
            if let Some(e) = o.excess[hi] {
                verdicts.push(o.verdict);
                ex.push(e);
                clusters.push((w.idx / block) as u64);
            }
        }
    }
    let mut rows = Vec::new();
    for v in VERDICTS {
        let x: Vec<f64> = ex.iter().map(|&e| f64::from(u8::from(rules::is_hit(v, e, h)))).collect();
        let g: Vec<bool> = verdicts.iter().map(|&q| q == v).collect();
        let n = g.iter().filter(|b| **b).count();
        if n == 0 {
            continue;
        }
        let lift_of = |idx: &[usize]| -> Option<f64> {
            let (mut sg, mut ng, mut sa) = (0.0, 0.0, 0.0);
            for &i in idx {
                sa += x[i];
                if g[i] {
                    sg += x[i];
                    ng += 1.0;
                }
            }
            (ng > 0.0).then(|| sg / ng - sa / idx.len() as f64)
        };
        let all: Vec<usize> = (0..x.len()).collect();
        let lift = lift_of(&all).unwrap_or(f64::NAN);
        let base = x.iter().sum::<f64>() / x.len() as f64;
        let mut reps_v = stats::cluster_bootstrap(&clusters, reps, seed, lift_of);
        rows.push(VerdictRow {
            verdict: v,
            n,
            hit: base + lift,
            base,
            lift,
            lift_ci: stats::percentile_ci(&mut reps_v),
        });
    }
    rows
}

/// Tasa base de superar al índice (exceso > 0) en la muestra del universo y horizonte.
pub fn base_rate_outperform(panel: &Panel, u: Universe, hi: usize) -> Option<(usize, f64)> {
    let e: Vec<f64> = panel
        .weeks
        .iter()
        .flat_map(|w| w.obs.iter())
        .filter(|o| u.contains(panel.assets[o.asset].group))
        .filter_map(|o| o.excess[hi])
        .collect();
    (!e.is_empty()).then(|| (e.len(), e.iter().filter(|x| **x > 0.0).count() as f64 / e.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- utilidades de datos sintéticos ----

    /// Días hábiles consecutivos desde `start_day`, con paseo aleatorio geométrico.
    fn walk(seed: u64, start_day: i64, n_bdays: usize, drift: f64, vol: f64) -> Vec<Candle> {
        let mut rng = Rng::new(seed);
        let (mut p, mut d, mut out) = (100.0f64, start_day, Vec::new());
        while out.len() < n_bdays {
            if matches!(d.rem_euclid(7), 2 | 3) {
                // 1970-01-01 jueves: día % 7 == 2 → sábado, 3 → domingo.
                d += 1;
                continue;
            }
            p *= (drift + vol * rng.normal()).exp();
            out.push(Candle {
                t: d * DAY + 14 * 3600,
                o: p,
                h: p * 1.005,
                l: p * 0.995,
                c: p,
                v: Some(1e6),
            });
            d += 1;
        }
        out
    }

    fn series(id: &str, candles: Vec<Candle>) -> Series {
        let adj: Vec<f64> = candles.iter().map(|c| c.c).collect();
        Series::new(id.parse().unwrap(), candles, &adj).unwrap()
    }

    const START: i64 = 17_000; // 2016-07-18

    #[test]
    fn fridays_are_fridays() {
        let f = fridays(day_of(NaiveDate::from_ymd_opt(2018, 1, 1).unwrap()), 17_700);
        assert_eq!(date_of(f[0]), NaiveDate::from_ymd_opt(2018, 1, 5).unwrap());
        assert!(f.windows(2).all(|w| w[1] - w[0] == 7));
        assert_eq!(chrono::Datelike::weekday(&date_of(f[10])), chrono::Weekday::Fri);
    }

    // ---- sin anticipación ----

    #[test]
    fn signals_do_not_change_when_future_candles_are_added() {
        let full = walk(7, START, 1200, 0.0003, 0.015);
        let d = day(full[800].t); // una fecha en medio de la serie
        let truncated: Vec<Candle> = full.iter().copied().filter(|c| day(c.t) <= d).collect();
        // Futuro «envenenado»: saltos enormes tras la fecha.
        let poisoned: Vec<Candle> = full
            .iter()
            .map(|c| {
                if day(c.t) > d {
                    Candle { o: c.o * 50.0, h: c.h * 60.0, l: c.l * 40.0, c: c.c * 50.0, ..*c }
                } else {
                    *c
                }
            })
            .collect();
        let a = signals_at(&series("etf:AAA", truncated), d).expect("señal");
        let b = signals_at(&series("etf:AAA", full.clone()), d).expect("señal");
        let c = signals_at(&series("etf:AAA", poisoned), d).expect("señal");
        assert_eq!(a, b, "añadir velas futuras cambió la puntuación pasada");
        assert_eq!(a, c, "un futuro envenenado cambió la puntuación pasada");
        // Y no es trivial: una fecha posterior sí da otra puntuación.
        let later = signals_at(&series("etf:AAA", full.clone()), day(full[900].t)).unwrap();
        assert_ne!(a.v, later.v);
    }

    #[test]
    fn forward_return_starts_at_the_next_close() {
        // Viernes d: 100. Lunes d+3: salta a 110 y se queda. Benchmark plano.
        let d = START + 4; // START % 7 == 3 (domingo+?): se busca un viernes concreto abajo
        let d = (d..d + 7).find(|x| x.rem_euclid(7) == 1).unwrap();
        let mk = |closes: &[(i64, f64)]| -> Vec<Candle> {
            closes
                .iter()
                .map(|&(dd, c)| Candle { t: dd * DAY + 14 * 3600, o: c, h: c, l: c, c, v: None })
                .collect()
        };
        let mut a = vec![(d - 1, 100.0), (d, 100.0), (d + 3, 110.0)];
        let mut b = vec![(d - 1, 50.0), (d, 50.0), (d + 3, 50.0)];
        for k in 1..=2 {
            a.push((d + 3 + 7 * k, 110.0));
            b.push((d + 3 + 7 * k, 50.0));
        }
        let (a, b) = (mk(&a), mk(&b));
        let e = forward_excess(&a, &b, d, 1, i64::MAX / 4).expect("resuelto");
        assert!(e.abs() < 1e-12, "la entrada debe ser el lunes (110), no el viernes: {e}");
        // Con entrada el viernes habría sido +10 %: comprobación de que el salto existe.
        assert!((a[2].c / a[1].c - 1.0 - 0.1).abs() < 1e-12);
        // Sin datos de salida → None, no un número inventado.
        assert!(forward_excess(&a, &b, d, 12, i64::MAX / 4).is_none());
    }

    // ---- reproducibilidad ----

    fn small_panel_series() -> Vec<Series> {
        let mut v = vec![series("etf:SPY", walk(1, START, 1500, 0.0004, 0.01))];
        for i in 0..18 {
            let id = format!("etf:E{i:02}");
            v.push(series(&id, walk(100 + i, START, 1500, 0.0003, 0.012 + 0.0004 * i as f64)));
        }
        v
    }

    #[test]
    fn panel_and_random_reference_are_reproducible_with_a_seed() {
        let s = small_panel_series();
        let cfg = PanelConfig {
            start_day: START + 450,
            end_day: START + 1500,
            now_s: (START + 2100) * DAY,
        };
        let (p1, p2) = (build_panel(&s, &cfg).unwrap(), build_panel(&s, &cfg).unwrap());
        assert_eq!(p1.weeks.len(), p2.weeks.len());
        for (a, b) in p1.weeks.iter().zip(&p2.weeks) {
            assert_eq!(a.obs.len(), b.obs.len());
            for (x, y) in a.obs.iter().zip(&b.obs) {
                assert_eq!(x.sig, y.sig);
                assert_eq!(x.excess, y.excess);
            }
        }
        assert!(p1.weeks.iter().any(|w| w.obs.len() >= 15), "debe haber secciones usables");
        let cost = CostModel::default_for(Universe::Equity);
        let r = |seeds| random_reference(&p1, Universe::Equity, 1, seeds, cost).unwrap();
        let (a, b, c) = (r(1..=8), r(1..=8), r(9..=16));
        assert_eq!(a.ic_mean, b.ic_mean);
        assert_eq!(a.net_mean, b.net_mean);
        assert_ne!(a.ic_mean, c.ic_mean);
        // Una puntuación por (semilla, semana, activo): estable y distinta entre semillas.
        assert_eq!(random_score(5, 3, 2), random_score(5, 3, 2));
        assert_ne!(random_score(5, 3, 2), random_score(6, 3, 2));
    }

    // ---- IC con señal conocida ----

    /// Panel sintético: `n` activos, `w` semanas; el exceso a 4 semanas depende de la señal
    /// compuesta con correlación `rho` (el resto, ruido).
    fn synthetic_panel(seed: u64, n: usize, w: usize, rho: f64) -> Panel {
        let mut rng = Rng::new(seed);
        let assets = (0..n).map(|i| AssetInfo { id: format!("stock:S{i}"), group: Group::Stock }).collect();
        // Primer viernes de 2018. El rango ha de ser acotado: `fridays` materializa todos los viernes
        // del intervalo (con `i64::MAX / 2` como fin reservaba memoria sin límite y tumbaba el equipo).
        let start = day_of(NaiveDate::from_ymd_opt(2018, 1, 1).unwrap());
        let first_friday = fridays(start, start + 6)[0];
        let weeks = (0..w)
            .map(|k| Week {
                idx: k,
                day: first_friday + 7 * k as i64,
                obs: (0..n)
                    .map(|a| {
                        let z = rng.normal();
                        let e = 0.03 * (rho * z + (1.0 - rho * rho).sqrt() * rng.normal());
                        Obs {
                            asset: a,
                            sig: [0.0, 0.0, 0.0, z, 0.0],
                            verdict: Verdict::Hold,
                            excess: [None, Some(e), None],
                        }
                    })
                    .collect(),
            })
            .collect();
        Panel { assets, weeks }
    }

    #[test]
    fn spearman_ic_recovers_a_known_signal_and_is_null_without_one() {
        let comp = |_: &Week, o: &Obs| o.sig[SIG_COMPOSITE];
        // Con normales, Spearman ≈ (6/π)·asin(ρ/2).
        let rho: f64 = 0.30;
        let expected = 6.0 / std::f64::consts::PI * (rho / 2.0).asin();
        let p = synthetic_panel(11, 100, 300, rho);
        let ic: Vec<(usize, f64)> = ic_series(&p, Universe::Stock, 1, &comp).iter().map(|x| (x.0, x.2)).collect();
        let s = summarize(&ic, 4, true).unwrap();
        assert!((s.mean - expected).abs() < 0.01, "IC medio {} vs esperado {expected}", s.mean);
        assert!(s.t > 10.0, "t = {}", s.t);
        assert!(s.lo < s.mean && s.mean < s.hi);
        let (bl, bh) = s.boot.unwrap();
        assert!(bl < expected && expected < bh, "bootstrap {bl}..{bh}");
        // Sin señal: IC ≈ 0 y no significativo.
        let p0 = synthetic_panel(12, 100, 300, 0.0);
        let ic0: Vec<(usize, f64)> = ic_series(&p0, Universe::Stock, 1, &comp).iter().map(|x| (x.0, x.2)).collect();
        let s0 = summarize(&ic0, 4, false).unwrap();
        assert!(s0.mean.abs() < 0.01 && s0.t.abs() < 3.0, "IC nulo: {} (t {})", s0.mean, s0.t);
        // Un horizonte sin datos no produce ICs.
        assert!(ic_series(&p, Universe::Stock, 0, &comp).is_empty());
        // Signo invertido → IC negativo.
        let neg = |_: &Week, o: &Obs| -o.sig[SIG_COMPOSITE];
        let icn = ic_series(&p, Universe::Stock, 1, &neg);
        assert!(icn.iter().map(|x| x.2).sum::<f64>() < 0.0);
    }

    #[test]
    fn newey_west_corrects_for_autocorrelation() {
        let mut rng = Rng::new(3);
        let white: Vec<f64> = (0..4000).map(|_| rng.normal()).collect();
        let (m, se) = newey_west(&white, 8).unwrap();
        let naive = (white.iter().map(|x| (x - m).powi(2)).sum::<f64>() / white.len() as f64 / white.len() as f64).sqrt();
        assert!((se / naive - 1.0).abs() < 0.25, "ruido blanco: {se} vs {naive}");
        // AR(1) con φ = 0,8: el error típico verdadero es √((1+φ)/(1−φ)) = 3 veces el ingenuo. Bartlett
        // con rezago 30 infrapondera las autocovarianzas: en esperanza da ≈ √(1 + 2·(Σφʲ − Σjφʲ/31)) ≈ 2,8.
        let mut x = vec![0.0f64; 4000];
        for i in 1..x.len() {
            x[i] = 0.8 * x[i - 1] + rng.normal();
        }
        let (m, se) = newey_west(&x, 30).unwrap();
        let naive = (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / x.len() as f64 / x.len() as f64).sqrt();
        let ratio = se / naive;
        assert!((2.3..3.3).contains(&ratio), "AR(1): {se} vs ingenuo {naive} (cociente {ratio})");
        assert!(newey_west(&[1.0, 2.0], 4).is_none());
    }

    // ---- cartera ----

    #[test]
    fn portfolio_gross_net_and_turnover_accounting() {
        let p = synthetic_panel(21, 100, 120, 0.3);
        let cost = CostModel { one_way_bps: 10.0, borrow_annual: 0.0 };
        // Previsión perfecta: gana siempre; el coste es lo que resta.
        let oracle = |_: &Week, o: &Obs| o.excess[1].unwrap();
        let pts = long_short(&p, Universe::Stock, 1, &oracle, cost);
        assert!(pts.iter().all(|x| x.gross > 0.0));
        // Señal fija por activo (el mismo ranking siempre): tras la primera cohorte, rotación 0 y neto = bruto.
        let fixed = |_: &Week, o: &Obs| o.asset as f64;
        let pts = long_short(&p, Universe::Stock, 1, &fixed, cost);
        let h = HORIZONS[1] as usize;
        for x in pts.iter().filter(|x| x.week >= h) {
            assert_eq!(x.turnover, 0.0);
            assert!((x.net - x.gross).abs() < 1e-15);
        }
        // La primera cohorte paga la entrada completa: 10 pb × 2 × (1+1) = 40 pb.
        assert!((pts[0].gross - pts[0].net - 0.0040).abs() < 1e-12);
        // Ranking que cambia por completo cada vez: rotación máxima.
        let flip = |w: &Week, o: &Obs| if w.idx.is_multiple_of(2) { o.asset as f64 } else { -(o.asset as f64) };
        let pts = long_short(&p, Universe::Stock, 1, &flip, cost);
        assert!(pts.iter().skip(2).all(|x| x.turnover == 0.0 || x.turnover == 1.0));
        // Préstamo: resta proporcional a h/52.
        let c2 = CostModel { one_way_bps: 0.0, borrow_annual: 0.052 };
        let pts = long_short(&p, Universe::Stock, 1, &fixed, c2);
        assert!((pts[5].gross - pts[5].net - 0.052 * 4.0 / 52.0).abs() < 1e-12);
        // Una puntuación aleatoria no gana en bruto de forma sistemática (≈ 0 entre semillas).
        let r = random_reference(&p, Universe::Stock, 1, 1..=40, CostModel { one_way_bps: 10.0, borrow_annual: 0.0 }).unwrap();
        assert!(r.ic_mean.abs() < 0.01, "IC aleatorio medio {}", r.ic_mean);
        assert!(r.net_mean < r.gross_mean, "los costes deben restar");
        assert!(r.frac_t196 < 0.25);
    }

    #[test]
    fn quintiles_are_monotone_with_a_real_signal() {
        let comp = |_: &Week, o: &Obs| o.sig[SIG_COMPOSITE];
        let q = quintile_means(&synthetic_panel(5, 100, 200, 0.3), Universe::Stock, 1, &comp).unwrap();
        assert!(q.windows(2).all(|w| w[1] > w[0]), "{q:?}");
    }

    #[test]
    fn verdict_table_matches_a_hand_count() {
        // Buy con exceso: +,+,-,+ → 3/4 aciertos; Avoid con -,+ → 1/2.
        let mk = |wk: usize, v: Verdict, e: f64| Obs {
            asset: wk,
            sig: [0.0; 5],
            verdict: v,
            excess: [None, Some(e), None],
        };
        let obs = vec![
            mk(0, Verdict::Buy, 0.05),
            mk(1, Verdict::Buy, 0.02),
            mk(2, Verdict::Buy, -0.01),
            mk(3, Verdict::Buy, 0.03),
            mk(4, Verdict::Avoid, -0.02),
            mk(5, Verdict::Avoid, 0.04),
        ];
        let assets = (0..6).map(|i| AssetInfo { id: format!("stock:X{i}"), group: Group::Stock }).collect();
        let panel = Panel { assets, weeks: vec![Week { idx: 0, day: 0, obs }] };
        let t = verdict_table(&panel, Universe::Stock, 1, 100, 1);
        let buy = t.iter().find(|r| r.verdict == Verdict::Buy).unwrap();
        assert_eq!(buy.n, 4);
        assert!((buy.hit - 0.75).abs() < 1e-12);
        // Base de «comprar» aplicado a las 6 observaciones: 4 positivas de 6.
        assert!((buy.base - 4.0 / 6.0).abs() < 1e-12);
        let avoid = t.iter().find(|r| r.verdict == Verdict::Avoid).unwrap();
        assert!((avoid.hit - 0.5).abs() < 1e-12);
        assert!((avoid.base - 2.0 / 6.0).abs() < 1e-12);
        assert_eq!(base_rate_outperform(&panel, Universe::Stock, 1), Some((6, 4.0 / 6.0)));
    }
}
