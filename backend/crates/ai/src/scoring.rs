//! Puntuación cuantitativa determinista (0–100) a partir de un `AssetSnapshot`.
//!
//! El LLM no inventa estos números: los recibe ya calculados, con sus razones, y los interpreta.
//! Todo es función pura del snapshot (el «ahora» para ponderar noticias es
//! `snapshot.generated_at`), así que el mismo snapshot da siempre la misma puntuación.
//!
//! Convenciones:
//! - Cada componente es una media ponderada de sub-puntuaciones; cada una aporta una razón
//!   legible («RSI14 = 72.3 (sobrecompra) → 30»). Sin datos → `None`, y no cuenta en `overall`.
//! - Todas las puntuaciones son «más alto = más favorable» salvo `risk` (más alto = más riesgo),
//!   que se invierte al calcular `overall`.
//! - Los umbrales son heurísticas razonables y están en tablas legibles (`piecewise`), no
//!   magia: se pueden afinar sin tocar la lógica.

use chrono::{DateTime, Utc};
use domain::{
    AssetClass, AssetSnapshot, Fundamentals, Indicators, MacroContext, NewsItem, ScoreBreakdown,
    Trend, Verdict,
};
use serde::Serialize;

use crate::{
    fmt,
    lang::{Lang, tr},
    sentiment,
};

/// Razones de cada componente (mismas claves que `ScoreBreakdown`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreReasons {
    pub technical: Vec<String>,
    pub momentum: Vec<String>,
    pub valuation: Vec<String>,
    pub fundamental: Vec<String>,
    pub sentiment: Vec<String>,
    pub macro_score: Vec<String>,
    pub risk: Vec<String>,
}

/// Resultado completo de la puntuación: números, razones y veredicto cuantitativo provisional.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scorecard {
    pub scores: ScoreBreakdown,
    pub reasons: ScoreReasons,
    /// Veredicto provisional, solo con números: el ancla que el LLM puede matizar.
    pub verdict: Verdict,
    pub verdict_reasons: Vec<String>,
    /// 0–1: fiabilidad de la puntuación según cobertura de datos y coherencia entre señales.
    pub confidence: f32,
    /// 0–1: peso de los componentes con datos sobre el total que aplica a esta clase de activo.
    pub coverage: f32,
    /// Componentes esperados para esta clase de activo que no se pudieron puntuar.
    pub gaps: Vec<String>,
}

type Out = (f32, Vec<String>);

// ---------------------------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------------------------

/// Tablas de umbrales: pares `(valor observado, puntuación 0–100)` ordenados por valor, que
/// `piecewise` interpola. Están aparte (y fuera de rustfmt) para leerlas y afinarlas de un vistazo.
#[rustfmt::skip]
mod tables {
    type T = &'static [(f64, f64)];
    pub const SMA200_DIST: T = &[(-0.30, 8.0), (-0.15, 22.0), (-0.05, 36.0), (0.0, 50.0), (0.05, 64.0), (0.15, 74.0), (0.30, 62.0), (0.60, 40.0)];
    pub const SMA50_DIST: T = &[(-0.15, 12.0), (-0.05, 30.0), (0.0, 48.0), (0.05, 66.0), (0.12, 72.0), (0.25, 50.0), (0.45, 30.0)];
    pub const SMA_CROSS: T = &[(-0.12, 12.0), (-0.03, 30.0), (0.0, 48.0), (0.03, 64.0), (0.12, 78.0)];
    pub const RSI: T = &[(10.0, 45.0), (20.0, 60.0), (30.0, 68.0), (45.0, 58.0), (55.0, 58.0), (65.0, 50.0), (70.0, 40.0), (80.0, 25.0), (90.0, 15.0)];
    pub const MACD_HIST_PCT: T = &[(-1.5, 18.0), (-0.4, 36.0), (0.0, 50.0), (0.4, 64.0), (1.5, 82.0)];
    pub const REWARD_RISK: T = &[(0.3, 25.0), (0.7, 38.0), (1.0, 50.0), (1.5, 64.0), (2.5, 78.0)];
    pub const MOM_1M: T = &[(-0.20, 10.0), (-0.08, 30.0), (0.0, 48.0), (0.05, 65.0), (0.15, 80.0), (0.30, 68.0)];
    pub const MOM_3M: T = &[(-0.30, 10.0), (-0.10, 30.0), (0.0, 48.0), (0.10, 68.0), (0.30, 85.0), (0.80, 70.0)];
    pub const MOM_1Y: T = &[(-0.40, 8.0), (-0.15, 28.0), (0.0, 45.0), (0.15, 65.0), (0.40, 82.0), (1.0, 72.0)];
    pub const HIGH52_DIST: T = &[(0.02, 72.0), (0.08, 66.0), (0.20, 50.0), (0.40, 34.0), (0.60, 20.0)];
    pub const ATH_RATIO: T = &[(0.03, 50.0), (0.10, 60.0), (0.25, 72.0), (0.50, 62.0), (0.80, 48.0), (0.95, 38.0), (1.0, 35.0)];
    pub const FDV_RATIO: T = &[(1.0, 85.0), (1.2, 70.0), (1.5, 55.0), (2.0, 38.0), (4.0, 15.0), (10.0, 5.0)];
    pub const PEG: T = &[(0.4, 88.0), (0.8, 78.0), (1.0, 66.0), (1.5, 45.0), (2.0, 30.0), (3.0, 12.0)];
    pub const FCF_YIELD: T = &[(-0.02, 10.0), (0.0, 25.0), (0.02, 48.0), (0.04, 62.0), (0.07, 78.0), (0.10, 86.0)];
    pub const EXPENSE_RATIO: T = &[(0.0003, 88.0), (0.001, 82.0), (0.003, 68.0), (0.006, 50.0), (0.01, 30.0), (0.02, 12.0)];
    pub const OP_MARGIN: T = &[(-0.10, 5.0), (0.0, 20.0), (0.08, 45.0), (0.15, 62.0), (0.25, 80.0), (0.40, 90.0)];
    pub const NET_MARGIN: T = &[(-0.10, 5.0), (0.0, 20.0), (0.05, 45.0), (0.12, 65.0), (0.25, 85.0)];
    pub const ROE: T = &[(-0.05, 5.0), (0.0, 20.0), (0.08, 45.0), (0.15, 65.0), (0.25, 82.0), (0.50, 85.0)];
    pub const REV_GROWTH: T = &[(-0.15, 5.0), (-0.03, 25.0), (0.03, 45.0), (0.10, 65.0), (0.25, 85.0), (0.50, 92.0)];
    pub const EPS_GROWTH: T = &[(-0.30, 8.0), (0.0, 35.0), (0.10, 55.0), (0.25, 75.0), (0.50, 88.0)];
    pub const DEBT_EQUITY: T = &[(0.0, 85.0), (0.3, 78.0), (0.8, 62.0), (1.5, 42.0), (3.0, 20.0), (6.0, 8.0)];
    pub const CURRENT_RATIO: T = &[(0.5, 15.0), (1.0, 40.0), (1.5, 65.0), (2.5, 80.0)];
    pub const FCF_MARGIN: T = &[(-0.10, 5.0), (0.0, 25.0), (0.05, 50.0), (0.12, 72.0), (0.25, 90.0)];
    pub const ANALYST_RATING: T = &[(1.0, 92.0), (2.0, 74.0), (3.0, 50.0), (4.0, 28.0), (5.0, 10.0)];
    pub const TARGET_UPSIDE: T = &[(-0.20, 18.0), (-0.05, 36.0), (0.05, 52.0), (0.15, 66.0), (0.30, 80.0), (0.60, 88.0)];
    pub const SHORT_INTEREST: T = &[(0.02, 60.0), (0.05, 50.0), (0.15, 30.0), (0.30, 15.0)];
    pub const CURVE: T = &[(-1.0, 25.0), (-0.3, 38.0), (0.0, 48.0), (0.5, 58.0), (1.5, 68.0)];
    pub const VIX: T = &[(12.0, 72.0), (16.0, 66.0), (20.0, 55.0), (25.0, 40.0), (30.0, 28.0), (40.0, 12.0)];
    pub const RATE_CHANGE: T = &[(-0.30, 66.0), (-0.10, 57.0), (0.0, 50.0), (0.10, 43.0), (0.30, 34.0)];
    pub const CPI: T = &[(1.5, 55.0), (2.5, 62.0), (3.5, 48.0), (5.0, 32.0), (8.0, 15.0)];
    pub const VOLATILITY: T = &[(0.10, 10.0), (0.20, 30.0), (0.30, 45.0), (0.50, 65.0), (0.80, 85.0), (1.20, 95.0)];
    pub const DRAWDOWN: T = &[(0.05, 10.0), (0.15, 30.0), (0.30, 55.0), (0.50, 78.0), (0.75, 92.0)];
    pub const ATR_PCT: T = &[(0.005, 10.0), (0.015, 30.0), (0.03, 55.0), (0.06, 80.0)];
    pub const BETA: T = &[(0.4, 15.0), (0.8, 30.0), (1.0, 42.0), (1.3, 58.0), (1.8, 78.0), (2.5, 92.0)];
    pub const MCAP_CRYPTO: T = &[(8.0, 88.0), (9.0, 70.0), (10.0, 50.0), (11.0, 32.0), (12.0, 22.0)];
    pub const MCAP_STOCK: T = &[(8.5, 80.0), (9.3, 62.0), (10.0, 48.0), (10.7, 35.0), (11.3, 22.0), (12.0, 15.0)];
}

/// Interpolación lineal por tramos; fuera del rango extremo se queda en el valor del extremo.
/// `pts` debe estar ordenado por `x` creciente.
fn piecewise(x: f64, pts: &[(f64, f64)]) -> f64 {
    debug_assert!(!pts.is_empty());
    if x <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    pts[pts.len() - 1].1
}

fn fin(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite())
}

fn round1(x: f64) -> f32 {
    ((x * 10.0).round() / 10.0) as f32
}

/// Acumula sub-puntuaciones ponderadas con su razón.
#[derive(Default)]
struct Parts {
    items: Vec<(f64, f64, String)>,
}

impl Parts {
    fn add(&mut self, weight: f64, score: f64, reason: String) {
        let s = score.clamp(0.0, 100.0);
        self.items.push((weight, s, format!("{reason} → {s:.0}")));
    }

    fn finish(self) -> Option<Out> {
        let w: f64 = self.items.iter().map(|(w, _, _)| w).sum();
        if self.items.is_empty() || w <= 0.0 {
            return None;
        }
        let s: f64 = self.items.iter().map(|(w, s, _)| w * s).sum::<f64>() / w;
        Some((round1(s.clamp(0.0, 100.0)), self.items.into_iter().map(|(_, _, r)| r).collect()))
    }
}

struct Ctx<'a> {
    l: Lang,
    class: AssetClass,
    price: Option<f64>,
    ind: Option<&'a Indicators>,
    fun: Option<&'a Fundamentals>,
    news: &'a [NewsItem],
    macro_ctx: Option<&'a MacroContext>,
    now: DateTime<Utc>,
    market_cap: Option<f64>,
}

fn trend_label(l: Lang, t: Trend) -> &'static str {
    match t {
        Trend::StrongUp => l.t("alcista fuerte", "strong uptrend"),
        Trend::Up => l.t("alcista", "uptrend"),
        Trend::Sideways => l.t("lateral", "sideways"),
        Trend::Down => l.t("bajista", "downtrend"),
        Trend::StrongDown => l.t("bajista fuerte", "strong downtrend"),
    }
}

fn trend_score(t: Trend) -> f64 {
    match t {
        Trend::StrongUp => 90.0,
        Trend::Up => 72.0,
        Trend::Sideways => 48.0,
        Trend::Down => 28.0,
        Trend::StrongDown => 10.0,
    }
}

/// Tendencia aproximada cuando el proveedor no la da: precio frente a SMA50 y SMA200.
fn derive_trend(price: Option<f64>, sma50: Option<f64>, sma200: Option<f64>) -> Option<Trend> {
    let p = price?;
    match (sma50, sma200) {
        (Some(a), Some(b)) if p > a && p > b => Some(Trend::Up),
        (Some(a), Some(b)) if p < a && p < b => Some(Trend::Down),
        (Some(_), Some(_)) => Some(Trend::Sideways),
        (Some(a), None) => Some(if p > a { Trend::Up } else { Trend::Down }),
        (None, Some(b)) => Some(if p > b { Trend::Up } else { Trend::Down }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Técnico
// ---------------------------------------------------------------------------------------------

fn technical(c: &Ctx) -> Option<Out> {
    let ind = c.ind?;
    let l = c.l;
    let mut p = Parts::default();
    let sma50 = fin(ind.sma50);
    let sma200 = fin(ind.sma200);

    if let Some(t) = ind.trend.or_else(|| derive_trend(c.price, sma50, sma200)) {
        p.add(2.0, trend_score(t), tr!(l, "Tendencia {}", "Trend: {}", trend_label(l, t)));
    }
    if let (Some(px), Some(s)) = (c.price, sma200) {
        let d = px / s - 1.0;
        let pts = tables::SMA200_DIST;
        p.add(
            1.5,
            piecewise(d, pts),
            tr!(
                l,
                "Precio {} respecto a la SMA200 ({})",
                "Price {} vs SMA200 ({})",
                fmt::pct_signed(d),
                fmt::price(s)
            ),
        );
    }
    if let (Some(px), Some(s)) = (c.price, sma50) {
        let d = px / s - 1.0;
        let pts = tables::SMA50_DIST;
        let note = if d > 0.20 { l.t(", muy extendido", ", overextended") } else { "" };
        p.add(
            1.0,
            piecewise(d, pts),
            tr!(
                l,
                "Precio {} respecto a la SMA50 ({}){}",
                "Price {} vs SMA50 ({}){}",
                fmt::pct_signed(d),
                fmt::price(s),
                note
            ),
        );
    }
    if let (Some(a), Some(b)) = (sma50, sma200) {
        let r = a / b - 1.0;
        let pts = tables::SMA_CROSS;
        let what = if r >= 0.0 {
            l.t("cruz dorada", "golden cross")
        } else {
            l.t("cruz de la muerte", "death cross")
        };
        p.add(
            1.0,
            piecewise(r, pts),
            tr!(
                l,
                "SMA50 {} la SMA200 ({}): {}",
                "SMA50 {} SMA200 ({}): {}",
                if r >= 0.0 { l.t("sobre", "above") } else { l.t("bajo", "below") },
                fmt::pct_signed(r),
                what
            ),
        );
    }
    if let Some(rsi) = fin(ind.rsi14) {
        let pts = tables::RSI;
        let zone = match rsi {
            r if r >= 70.0 => l.t("sobrecompra", "overbought"),
            r if r <= 30.0 => l.t("sobreventa", "oversold"),
            _ => l.t("zona neutra", "neutral zone"),
        };
        p.add(
            1.0,
            piecewise(rsi, pts),
            tr!(l, "RSI14 = {:.1} ({})", "RSI14 = {:.1} ({})", rsi, zone),
        );
    }
    let hist =
        fin(ind.macd_hist).or_else(|| fin(ind.macd).zip(fin(ind.macd_signal)).map(|(m, s)| m - s));
    if let Some(h) = hist {
        // Histograma normalizado por el precio para que sea comparable entre activos.
        let base = c.price.or(fin(ind.last_close)).filter(|v| *v > 0.0);
        let norm = base.map(|b| h / b * 100.0).unwrap_or(h.signum() * 0.3);
        let pts = tables::MACD_HIST_PCT;
        let what = if h >= 0.0 { l.t("alcista", "bullish") } else { l.t("bajista", "bearish") };
        p.add(
            1.0,
            piecewise(norm, pts),
            tr!(l, "MACD histograma {} ({})", "MACD histogram {} ({})", fmt::num(h), what),
        );
    }
    // Recompensa/riesgo entre el soporte y la resistencia más cercanos.
    if let Some(px) = c.price {
        let sup = ind
            .supports
            .iter()
            .copied()
            .filter(|s| s.is_finite() && *s > 0.0 && *s < px)
            .fold(None, |a: Option<f64>, s| Some(a.map_or(s, |m| m.max(s))));
        let res = ind
            .resistances
            .iter()
            .copied()
            .filter(|r| r.is_finite() && *r > px)
            .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |m| m.min(r))));
        if let (Some(s), Some(r)) = (sup, res) {
            let (down, up) = ((px - s) / px, (r - px) / px);
            if down > 0.0005 {
                let rr = up / down;
                let pts = tables::REWARD_RISK;
                p.add(
                    1.0,
                    piecewise(rr, pts),
                    tr!(
                        l,
                        "Soporte {} ({}) y resistencia {} ({}): recompensa/riesgo {:.1}",
                        "Support {} ({}) and resistance {} ({}): reward/risk {:.1}",
                        fmt::price(s),
                        fmt::pct_signed(-down),
                        fmt::price(r),
                        fmt::pct_signed(up),
                        rr
                    ),
                );
            }
        }
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Momentum
// ---------------------------------------------------------------------------------------------

/// Escala típica de los rendimientos de la clase: un +20 % en 3 meses es normal en cripto y
/// excepcional en un bono.
fn return_scale(class: AssetClass) -> f64 {
    match class {
        AssetClass::Crypto => 2.2,
        AssetClass::Etf => 0.8,
        AssetClass::Fund => 0.7,
        AssetClass::Index => 0.6,
        AssetClass::Commodity => 0.8,
        AssetClass::Forex => 0.25,
        AssetClass::Bond => 0.3,
        _ => 1.0,
    }
}

fn momentum(c: &Ctx) -> Option<Out> {
    let ind = c.ind?;
    let l = c.l;
    let k = return_scale(c.class);
    let mut p = Parts::default();
    // Tablas en unidades «de la clase»: rendimiento / escala. Las subidas parabólicas puntúan
    // algo menos que las sostenidas (riesgo de agotamiento).
    let m1 = tables::MOM_1M;
    let m3 = tables::MOM_3M;
    let y1 = tables::MOM_1Y;
    for (w, v, name, pts) in [
        (1.0, fin(ind.return1m), l.t("1 mes", "1 month"), m1),
        (1.5, fin(ind.return3m), l.t("3 meses", "3 months"), m3),
        (1.5, fin(ind.return1y), l.t("1 año", "1 year"), y1),
    ] {
        if let Some(r) = v {
            p.add(
                w,
                piecewise(r / k, pts),
                tr!(l, "Rendimiento a {}: {}", "Return over {}: {}", name, fmt::pct_signed(r)),
            );
        }
    }
    if let Some(d) = fin(ind.dist_from_high52w).map(f64::abs) {
        // Cerca del máximo anual = fuerza relativa; muy lejos = castigo.
        let pts = tables::HIGH52_DIST;
        p.add(
            0.6,
            piecewise(d / k, pts),
            tr!(l, "A {} del máximo de 52 semanas", "{} below the 52-week high", fmt::pct(d)),
        );
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Valoración
// ---------------------------------------------------------------------------------------------

/// Múltiplos «barato / razonable / caro» de un sector.
struct Profile {
    pe: (f64, f64, f64),
    ps: (f64, f64, f64),
    ev_ebitda: Option<(f64, f64, f64)>,
    pb: Option<(f64, f64, f64)>,
    financial: bool,
}

fn profile(sector: Option<&str>, class: AssetClass) -> Profile {
    if matches!(class, AssetClass::Etf | AssetClass::Fund | AssetClass::Index) {
        return Profile {
            pe: (13.0, 19.0, 27.0),
            ps: (1.0, 2.5, 4.5),
            ev_ebitda: None,
            pb: None,
            financial: false,
        };
    }
    let s = sector.unwrap_or("").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    let p = |pe, ps, ev| Profile { pe, ps, ev_ebitda: Some(ev), pb: None, financial: false };
    if has(&["tech"]) {
        p((16.0, 26.0, 42.0), (3.0, 7.0, 14.0), (11.0, 18.0, 30.0))
    } else if has(&["communication", "telecom"]) {
        p((12.0, 20.0, 32.0), (1.5, 3.5, 7.0), (7.0, 12.0, 20.0))
    } else if has(&["health"]) {
        p((13.0, 21.0, 32.0), (1.5, 4.0, 8.0), (9.0, 14.0, 22.0))
    } else if has(&["financ", "bank", "insur"]) {
        Profile {
            pe: (7.0, 12.0, 18.0),
            ps: (1.5, 3.0, 5.0),
            ev_ebitda: None,
            pb: Some((0.8, 1.4, 2.4)),
            financial: true,
        }
    } else if has(&["cyclical", "discretionary"]) {
        p((11.0, 19.0, 30.0), (0.5, 1.5, 3.5), (7.0, 12.0, 20.0))
    } else if has(&["defensive", "staples"]) {
        p((14.0, 21.0, 28.0), (0.6, 1.5, 3.0), (9.0, 13.0, 18.0))
    } else if has(&["industrial"]) {
        p((13.0, 20.0, 28.0), (0.8, 1.8, 3.5), (8.0, 13.0, 19.0))
    } else if has(&["energy"]) {
        p((6.0, 11.0, 17.0), (0.5, 1.2, 2.5), (3.5, 6.0, 9.0))
    } else if has(&["utilit"]) {
        p((11.0, 17.0, 23.0), (1.0, 2.0, 3.5), (7.0, 10.0, 14.0))
    } else if has(&["real estate", "reit"]) {
        Profile {
            pe: (14.0, 28.0, 48.0),
            ps: (4.0, 7.0, 12.0),
            ev_ebitda: Some((12.0, 18.0, 26.0)),
            pb: Some((0.7, 1.5, 2.5)),
            financial: false,
        }
    } else if has(&["material", "mining", "chemical"]) {
        p((9.0, 15.0, 23.0), (0.8, 1.8, 3.5), (5.0, 9.0, 14.0))
    } else {
        p((12.0, 19.0, 30.0), (1.0, 3.0, 7.0), (7.0, 12.0, 20.0))
    }
}

/// Puntúa un múltiplo con umbrales (barato, razonable, caro): más bajo = más favorable.
fn multiple_score((cheap, fair, expensive): (f64, f64, f64), x: f64) -> f64 {
    piecewise(
        x,
        &[
            (cheap * 0.4, 82.0),
            (cheap, 76.0),
            (fair, 55.0),
            (expensive, 30.0),
            (expensive * 2.0, 8.0),
        ],
    )
}

fn zone_label(l: Lang, (cheap, fair, expensive): (f64, f64, f64), x: f64) -> &'static str {
    if x <= cheap {
        l.t("barato", "cheap")
    } else if x <= fair {
        l.t("razonable", "fair")
    } else if x <= expensive {
        l.t("exigente", "full")
    } else {
        l.t("caro", "expensive")
    }
}

fn valuation(c: &Ctx) -> Option<Out> {
    let l = c.l;
    let mut p = Parts::default();
    let f = c.fun;

    if c.class == AssetClass::Crypto {
        if let (Some(px), Some(ath)) = (c.price, f.and_then(|f| fin(f.all_time_high)))
            && ath > 0.0 {
                let x = px / ath;
                let pts = tables::ATH_RATIO;
                p.add(
                    1.5,
                    piecewise(x, pts),
                    tr!(
                        l,
                        "Precio al {} de su máximo histórico ({})",
                        "Price at {} of its all-time high ({})",
                        fmt::pct(x),
                        fmt::price(ath)
                    ),
                );
            }
        if let Some(f) = f {
            let mcap = fin(f.market_cap).or(c.market_cap).filter(|v| *v > 0.0);
            let ratio = match (fin(f.fully_diluted_valuation), mcap) {
                (Some(fdv), Some(m)) => Some((fdv / m, false)),
                _ => match (fin(f.max_supply), fin(f.circulating_supply)) {
                    (Some(max), Some(circ)) if circ > 0.0 => Some((max / circ, true)),
                    _ => None,
                },
            };
            if let Some((r, from_supply)) = ratio {
                let pts = tables::FDV_RATIO;
                let src = if from_supply {
                    l.t("oferta máx./circulante", "max/circulating supply")
                } else {
                    "FDV / market cap"
                };
                p.add(
                    1.5,
                    piecewise(r, pts),
                    tr!(
                        l,
                        "{} = {:.2}x (dilución futura)",
                        "{} = {:.2}x (future dilution)",
                        src,
                        r
                    ),
                );
            }
        }
        return p.finish();
    }

    let f = f?;
    let prof = profile(f.sector.as_deref(), c.class);
    let sector = f.sector.as_deref().unwrap_or(l.t("general", "general"));

    // PER: preferimos el forward (descuenta lo ya conocido) si es positivo.
    let pe = fin(f.pe_forward)
        .filter(|v| *v > 0.0)
        .map(|v| (v, "forward"))
        .or_else(|| fin(f.pe_trailing).map(|v| (v, "trailing")));
    if let Some((v, kind)) = pe {
        if v <= 0.0 {
            p.add(
                2.0,
                12.0,
                tr!(
                    l,
                    "PER {} no positivo: beneficios negativos",
                    "{} P/E not positive: negative earnings",
                    kind
                ),
            );
        } else {
            p.add(
                2.0,
                multiple_score(prof.pe, v),
                tr!(
                    l,
                    "PER {} = {} ({}; sector {})",
                    "{} P/E = {} ({}; sector {})",
                    kind,
                    fmt::num(v),
                    zone_label(l, prof.pe, v),
                    sector
                ),
            );
        }
    }
    if let Some(peg) = fin(f.peg).filter(|v| *v > 0.0) {
        let pts = tables::PEG;
        p.add(1.5, piecewise(peg, pts), tr!(l, "PEG = {}", "PEG = {}", fmt::num(peg)));
    }
    if !prof.financial
        && let Some(ps) = fin(f.price_to_sales).filter(|v| *v > 0.0) {
            p.add(
                1.0,
                multiple_score(prof.ps, ps),
                tr!(l, "P/S = {} ({})", "P/S = {} ({})", fmt::num(ps), zone_label(l, prof.ps, ps)),
            );
        }
    if let (Some(th), Some(ev)) = (prof.ev_ebitda, fin(f.ev_to_ebitda)) {
        if ev <= 0.0 {
            p.add(
                1.5,
                10.0,
                tr!(
                    l,
                    "EV/EBITDA = {}: EBITDA negativo",
                    "EV/EBITDA = {}: negative EBITDA",
                    fmt::num(ev)
                ),
            );
        } else {
            p.add(
                1.5,
                multiple_score(th, ev),
                tr!(
                    l,
                    "EV/EBITDA = {} ({})",
                    "EV/EBITDA = {} ({})",
                    fmt::num(ev),
                    zone_label(l, th, ev)
                ),
            );
        }
    }
    if let (Some(th), Some(pb)) = (prof.pb, fin(f.price_to_book).filter(|v| *v > 0.0)) {
        p.add(
            1.0,
            multiple_score(th, pb),
            tr!(l, "P/B = {} ({})", "P/B = {} ({})", fmt::num(pb), zone_label(l, th, pb)),
        );
    }
    // Rentabilidad por caja libre sobre capitalización.
    if let (Some(fcf), Some(m)) =
        (fin(f.free_cash_flow), fin(f.market_cap).or(c.market_cap).filter(|v| *v > 0.0))
        && !prof.financial {
            let y = fcf / m;
            let pts = tables::FCF_YIELD;
            p.add(
                1.2,
                piecewise(y, pts),
                tr!(l, "Rentabilidad por FCF = {}", "FCF yield = {}", fmt::pct(y)),
            );
        }
    // Coste de un ETF/fondo (el ratio de gastos es una «valoración» en sentido amplio).
    if matches!(c.class, AssetClass::Etf | AssetClass::Fund)
        && let Some(mut er) = fin(f.expense_ratio).filter(|v| *v >= 0.0) {
            if er > 0.03 {
                er /= 100.0; // algunos proveedores lo dan en %
            }
            let pts = tables::EXPENSE_RATIO;
            p.add(
                0.8,
                piecewise(er, pts),
                tr!(l, "Coste anual (TER) = {}", "Expense ratio = {}", fmt::pct(er)),
            );
        }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Fundamentales
// ---------------------------------------------------------------------------------------------

/// Algunos proveedores dan la deuda/capital en % (150 = 1,5x). Una razón > 25x es irreal fuera
/// de casos extremos, así que se interpreta como porcentaje.
fn debt_equity_ratio(v: f64) -> f64 {
    if v > 25.0 { v / 100.0 } else { v }
}

fn fundamental(c: &Ctx) -> Option<Out> {
    if matches!(
        c.class,
        AssetClass::Crypto | AssetClass::Forex | AssetClass::Commodity | AssetClass::Index
    ) {
        return None;
    }
    let f = c.fun?;
    let l = c.l;
    let prof = profile(f.sector.as_deref(), c.class);
    let mut p = Parts::default();

    if let Some(m) = fin(f.operating_margin) {
        let pts = tables::OP_MARGIN;
        p.add(
            1.0,
            piecewise(m, pts),
            tr!(l, "Margen operativo = {}", "Operating margin = {}", fmt::pct(m)),
        );
    }
    if let Some(m) = fin(f.profit_margin) {
        let pts = tables::NET_MARGIN;
        p.add(1.0, piecewise(m, pts), tr!(l, "Margen neto = {}", "Net margin = {}", fmt::pct(m)));
    }
    if let Some(roe) = fin(f.roe) {
        let pts = tables::ROE;
        p.add(
            if prof.financial { 1.5 } else { 1.0 },
            piecewise(roe, pts),
            tr!(l, "ROE = {}", "ROE = {}", fmt::pct(roe)),
        );
    }
    if let Some(g) = fin(f.revenue_growth_yoy) {
        let pts = tables::REV_GROWTH;
        p.add(
            1.5,
            piecewise(g, pts),
            tr!(l, "Crecimiento de ingresos = {}", "Revenue growth = {}", fmt::pct_signed(g)),
        );
    }
    if let Some(g) = fin(f.earnings_growth_yoy) {
        let pts = tables::EPS_GROWTH;
        p.add(
            1.0,
            piecewise(g, pts),
            tr!(l, "Crecimiento de beneficios = {}", "Earnings growth = {}", fmt::pct_signed(g)),
        );
    }
    if !prof.financial {
        if let Some(de) = fin(f.debt_to_equity).filter(|v| *v >= 0.0).map(debt_equity_ratio) {
            let pts = tables::DEBT_EQUITY;
            p.add(
                1.0,
                piecewise(de, pts),
                tr!(l, "Deuda/capital = {:.2}x", "Debt/equity = {:.2}x", de),
            );
        }
        if let (Some(cash), Some(debt)) = (fin(f.total_cash), fin(f.total_debt))
            && debt > 0.0 && cash > debt {
                p.add(
                    0.8,
                    85.0,
                    tr!(
                        l,
                        "Caja neta: caja {} > deuda {}",
                        "Net cash: cash {} > debt {}",
                        fmt::big(cash),
                        fmt::big(debt)
                    ),
                );
            }
        if let Some(cr) = fin(f.current_ratio).filter(|v| *v > 0.0) {
            let pts = tables::CURRENT_RATIO;
            p.add(
                0.5,
                piecewise(cr, pts),
                tr!(l, "Ratio de liquidez corriente = {}", "Current ratio = {}", fmt::num(cr)),
            );
        }
        if let Some(fcf) = fin(f.free_cash_flow) {
            match fin(f.revenue_ttm).filter(|v| *v > 0.0) {
                Some(rev) => {
                    let m = fcf / rev;
                    let pts = tables::FCF_MARGIN;
                    p.add(
                        1.5,
                        piecewise(m, pts),
                        tr!(l, "Margen de FCF = {}", "FCF margin = {}", fmt::pct(m)),
                    );
                }
                None => {
                    let (s, what) = if fcf > 0.0 {
                        (60.0, l.t("positivo", "positive"))
                    } else {
                        (20.0, l.t("negativo", "negative"))
                    };
                    p.add(
                        1.0,
                        s,
                        tr!(
                            l,
                            "Flujo de caja libre {} ({})",
                            "Free cash flow {} ({})",
                            what,
                            fmt::big(fcf)
                        ),
                    );
                }
            }
        }
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Sentimiento
// ---------------------------------------------------------------------------------------------

/// Vida media del peso de una noticia: a las 48 h pesa la mitad.
const NEWS_HALF_LIFE_HOURS: f64 = 48.0;
/// Las noticias más viejas que esto se ignoran.
const NEWS_MAX_AGE_HOURS: f64 = 24.0 * 14.0;

fn sentiment_score(c: &Ctx) -> Option<Out> {
    let l = c.l;
    let mut p = Parts::default();

    let (mut wsum, mut ssum, mut n, mut pos, mut neg) = (0.0f64, 0.0f64, 0usize, 0usize, 0usize);
    for item in c.news {
        let age_h = ((c.now - item.published_at).num_seconds() as f64 / 3600.0).max(0.0);
        if age_h > NEWS_MAX_AGE_HOURS {
            continue;
        }
        let s = match &item.sentiment {
            Some(s) => s.score as f64,
            None => sentiment::sentiment_of(item).score as f64,
        };
        let w = 0.5f64.powf(age_h / NEWS_HALF_LIFE_HOURS);
        wsum += w;
        ssum += w * s;
        n += 1;
        if s > 0.15 {
            pos += 1;
        } else if s < -0.15 {
            neg += 1;
        }
    }
    if n > 0 && wsum > 0.0 {
        let mean = ssum / wsum;
        // Con pocas noticias el peso de esta señal se reduce (n_eff / (n_eff + 2)).
        let weight = 2.0 * wsum / (wsum + 2.0);
        let score = 50.0 + 45.0 * (mean / 0.5).clamp(-1.0, 1.0);
        p.add(
            weight,
            score,
            tr!(
                l,
                "{} noticias recientes, sentimiento medio ponderado {:+.2} (positivas: {}, negativas: {})",
                "{} recent news items, recency-weighted mean sentiment {:+.2} (positive: {}, negative: {})",
                n,
                mean,
                pos,
                neg
            ),
        );
    }
    if let Some(f) = c.fun {
        let n_an = f.analyst_count.unwrap_or(0);
        if let Some(r) = fin(f.analyst_rating_mean).filter(|v| (1.0..=5.0).contains(v)) {
            let pts = tables::ANALYST_RATING;
            let w = if n_an >= 5 { 1.5 } else { 0.8 };
            p.add(
                w,
                piecewise(r, pts),
                tr!(
                    l,
                    "Consenso de analistas {:.1}/5 ({} analistas; 1 = compra)",
                    "Analyst consensus {:.1}/5 ({} analysts; 1 = buy)",
                    r,
                    n_an
                ),
            );
        }
        if let (Some(t), Some(px)) = (fin(f.analyst_target_mean).filter(|v| *v > 0.0), c.price)
            && (n_an == 0 || n_an >= 3) {
                let up = t / px - 1.0;
                let pts = tables::TARGET_UPSIDE;
                p.add(
                    1.0,
                    piecewise(up, pts),
                    tr!(
                        l,
                        "Precio objetivo medio {} ({} potencial)",
                        "Mean price target {} ({} upside)",
                        fmt::price(t),
                        fmt::pct_signed(up)
                    ),
                );
            }
        if let Some(s) = fin(f.short_pct_float).filter(|v| *v >= 0.0) {
            let pts = tables::SHORT_INTEREST;
            p.add(
                0.5,
                piecewise(s, pts),
                tr!(
                    l,
                    "Interés corto = {} del free float",
                    "Short interest = {} of float",
                    fmt::pct(s)
                ),
            );
        }
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Macro
// ---------------------------------------------------------------------------------------------

/// Cambios de tipos: si la unidad está en puntos básicos se pasa a puntos porcentuales.
fn to_pp(v: f64, unit: &str) -> f64 {
    if unit.to_lowercase().contains("bp") { v / 100.0 } else { v }
}

fn macro_score(c: &Ctx) -> Option<Out> {
    let m = c.macro_ctx?;
    let l = c.l;
    let find = |key: &str| m.indicators.iter().find(|i| i.key == key && i.value.is_finite());
    let mut p = Parts::default();

    let curve = find("curve_10y2y")
        .map(|i| to_pp(i.value, &i.unit))
        .or_else(|| find("us10y").zip(find("us2y")).map(|(a, b)| a.value - b.value));
    if let Some(v) = curve {
        let pts = tables::CURVE;
        let what = if v < 0.0 {
            l.t("invertida: señal de recesión", "inverted: recession signal")
        } else {
            l.t("normal", "normal")
        };
        p.add(
            1.0,
            piecewise(v, pts),
            tr!(l, "Curva 10a-2a = {:+.2} pp ({})", "10y-2y curve = {:+.2} pp ({})", v, what),
        );
    }
    if let Some(i) = find("vix") {
        let pts = tables::VIX;
        let what = match i.value {
            v if v < 16.0 => l.t("calma", "calm"),
            v if v < 25.0 => l.t("normal", "normal"),
            v if v < 30.0 => l.t("tensión", "stress"),
            _ => l.t("miedo", "fear"),
        };
        p.add(
            1.5,
            piecewise(i.value, pts),
            tr!(l, "VIX = {:.1} ({})", "VIX = {:.1} ({})", i.value, what),
        );
    }
    let rates = find("us10y").or_else(|| find("fed_funds"));
    if let Some((i, ch)) = rates.and_then(|i| fin(i.change).map(|ch| (i, to_pp(ch, &i.unit)))) {
        let pts = tables::RATE_CHANGE;
        let dir = if ch > 0.0 {
            l.t("al alza", "rising")
        } else if ch < 0.0 {
            l.t("a la baja", "falling")
        } else {
            l.t("estables", "flat")
        };
        p.add(
            1.0,
            piecewise(ch, pts),
            tr!(l, "Tipos ({}) {}: {:+.2} pp", "Rates ({}) {}: {:+.2} pp", i.name, dir, ch),
        );
    }
    if let Some(i) = find("cpi_yoy") {
        let pts = tables::CPI;
        p.add(
            0.7,
            piecewise(i.value, pts),
            tr!(l, "Inflación interanual = {:.1}%", "Inflation (YoY) = {:.1}%", i.value),
        );
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Riesgo (más alto = más riesgo)
// ---------------------------------------------------------------------------------------------

fn risk(c: &Ctx) -> Option<Out> {
    let l = c.l;
    let mut p = Parts::default();
    if let Some(ind) = c.ind {
        if let Some(v) = fin(ind.volatility30d).filter(|v| *v >= 0.0) {
            let pts = tables::VOLATILITY;
            p.add(
                2.0,
                piecewise(v, pts),
                tr!(
                    l,
                    "Volatilidad 30d anualizada = {}",
                    "30d annualised volatility = {}",
                    fmt::pct(v)
                ),
            );
        }
        if let Some(dd) = fin(ind.max_drawdown1y).map(f64::abs) {
            let pts = tables::DRAWDOWN;
            p.add(
                1.5,
                piecewise(dd, pts),
                tr!(l, "Caída máxima a 1 año = {}", "1-year max drawdown = {}", fmt::pct(dd)),
            );
        }
        if let (Some(atr), Some(px)) = (fin(ind.atr14), c.price.filter(|v| *v > 0.0)) {
            let a = atr / px;
            let pts = tables::ATR_PCT;
            p.add(
                0.6,
                piecewise(a, pts),
                tr!(
                    l,
                    "ATR14 = {} del precio (rango diario típico)",
                    "ATR14 = {} of price (typical daily range)",
                    fmt::pct(a)
                ),
            );
        }
    }
    if let Some(beta) = c.fun.and_then(|f| fin(f.beta)).filter(|b| *b > -5.0 && *b < 10.0) {
        let pts = tables::BETA;
        p.add(1.0, piecewise(beta, pts), tr!(l, "Beta = {}", "Beta = {}", fmt::num(beta)));
    }
    // Tamaño: capitalización pequeña = más frágil. En cripto la escala es otra.
    if let Some(m) = c.market_cap.filter(|v| *v > 0.0) {
        let x = m.log10();
        let (pts, w): (&[(f64, f64)], f64) = match c.class {
            AssetClass::Crypto => (tables::MCAP_CRYPTO, 1.0),
            AssetClass::Stock => (tables::MCAP_STOCK, 0.8),
            _ => (&[], 0.0),
        };
        if w > 0.0 {
            p.add(w, piecewise(x, pts), tr!(l, "Capitalización {}", "Market cap {}", fmt::big(m)));
        }
    }
    p.finish()
}

// ---------------------------------------------------------------------------------------------
// Ensamblado
// ---------------------------------------------------------------------------------------------

/// Peso de cada componente en `overall`, según la clase de activo:
/// `[técnico, momentum, valoración, fundamentales, sentimiento, macro, riesgo]`.
fn weights(class: AssetClass) -> [f64; 7] {
    match class {
        AssetClass::Stock => [0.17, 0.13, 0.20, 0.22, 0.08, 0.08, 0.12],
        AssetClass::Etf | AssetClass::Fund | AssetClass::Index => {
            [0.24, 0.18, 0.16, 0.10, 0.08, 0.12, 0.12]
        }
        AssetClass::Crypto => [0.24, 0.20, 0.14, 0.0, 0.12, 0.10, 0.20],
        _ => [0.28, 0.22, 0.05, 0.0, 0.08, 0.20, 0.17],
    }
}

/// Componentes que *deberían* tener datos para esta clase (para listar carencias).
fn expected(class: AssetClass) -> [bool; 7] {
    match class {
        AssetClass::Stock => [true, true, true, true, true, true, true],
        AssetClass::Etf | AssetClass::Fund | AssetClass::Index => {
            [true, true, false, false, true, true, true]
        }
        AssetClass::Crypto => [true, true, true, false, true, true, true],
        _ => [true, true, false, false, false, true, true],
    }
}

fn component_names(l: Lang) -> [&'static str; 7] {
    [
        l.t("Técnico", "Technical"),
        l.t("Momentum", "Momentum"),
        l.t("Valoración", "Valuation"),
        l.t("Fundamentales", "Fundamentals"),
        l.t("Sentimiento", "Sentiment"),
        l.t("Macro", "Macro"),
        l.t("Riesgo", "Risk"),
    ]
}

/// Puntúa un snapshot. Pura y barata (microsegundos).
pub fn score(snapshot: &AssetSnapshot, lang: Lang) -> Scorecard {
    let class = snapshot.summary.class;
    let price = fin(snapshot.quote.as_ref().map(|q| q.price)).filter(|p| *p > 0.0).or_else(|| {
        snapshot.indicators.as_ref().and_then(|i| fin(i.last_close)).filter(|p| *p > 0.0)
    });
    let market_cap = fin(snapshot.fundamentals.as_ref().and_then(|f| f.market_cap))
        .or_else(|| fin(snapshot.quote.as_ref().and_then(|q| q.market_cap)));
    let ctx = Ctx {
        l: lang,
        class,
        price,
        ind: snapshot.indicators.as_ref(),
        fun: snapshot.fundamentals.as_ref(),
        news: &snapshot.news,
        macro_ctx: snapshot.macro_context.as_ref(),
        now: snapshot.generated_at,
        market_cap,
    };

    let outs: [Option<Out>; 7] = [
        technical(&ctx),
        momentum(&ctx),
        valuation(&ctx),
        fundamental(&ctx),
        sentiment_score(&ctx),
        macro_score(&ctx),
        risk(&ctx),
    ];
    let w = weights(class);

    // overall: media ponderada sobre los componentes con datos (riesgo invertido).
    let (mut num, mut den) = (0.0f64, 0.0f64);
    let mut favorable = Vec::new();
    for (i, o) in outs.iter().enumerate() {
        if let Some((s, _)) = o {
            let fav = if i == 6 { 100.0 - *s as f64 } else { *s as f64 };
            num += w[i] * fav;
            den += w[i];
            favorable.push(fav);
        }
    }
    let total_w: f64 = w.iter().sum();
    let coverage = if total_w > 0.0 { (den / total_w).clamp(0.0, 1.0) } else { 0.0 };
    let overall = if den > 0.0 { num / den } else { 50.0 };

    let mut gaps = Vec::new();
    let names = component_names(lang);
    let exp = expected(class);
    for i in 0..7 {
        if exp[i] && outs[i].is_none() {
            gaps.push(tr!(
                lang,
                "{}: sin datos suficientes para puntuar",
                "{}: not enough data to score",
                names[i]
            ));
        }
    }

    let [t, m, v, f, s, mc, r] = outs;
    let scores = ScoreBreakdown {
        overall: round1(overall),
        technical: t.as_ref().map(|o| o.0),
        momentum: m.as_ref().map(|o| o.0),
        valuation: v.as_ref().map(|o| o.0),
        fundamental: f.as_ref().map(|o| o.0),
        sentiment: s.as_ref().map(|o| o.0),
        macro_score: mc.as_ref().map(|o| o.0),
        risk: r.as_ref().map(|o| o.0),
    };
    let reasons = ScoreReasons {
        technical: t.map(|o| o.1).unwrap_or_default(),
        momentum: m.map(|o| o.1).unwrap_or_default(),
        valuation: v.map(|o| o.1).unwrap_or_default(),
        fundamental: f.map(|o| o.1).unwrap_or_default(),
        sentiment: s.map(|o| o.1).unwrap_or_default(),
        macro_score: mc.map(|o| o.1).unwrap_or_default(),
        risk: r.map(|o| o.1).unwrap_or_default(),
    };

    let (verdict, verdict_reasons) = quantitative_verdict(&scores, &ctx, coverage);
    let confidence = confidence(overall, coverage, &favorable, snapshot);
    Scorecard {
        scores,
        reasons,
        verdict,
        verdict_reasons,
        confidence,
        coverage: round1(coverage * 100.0) / 100.0,
        gaps,
    }
}

/// Veredicto provisional: umbrales sobre `overall` y ajustes de prudencia (entrada, riesgo, datos).
fn quantitative_verdict(s: &ScoreBreakdown, c: &Ctx, coverage: f64) -> (Verdict, Vec<String>) {
    let l = c.l;
    let mut why = Vec::new();
    if coverage <= 0.0 {
        why.push(tr!(
            l,
            "Sin datos para puntuar: no hay base para una recomendación",
            "No data to score: no basis for a recommendation"
        ));
        return (Verdict::Wait, why);
    }
    let o = s.overall as f64;
    let mut v = match o {
        x if x >= 70.0 => Verdict::Buy,
        x if x >= 60.0 => Verdict::Accumulate,
        x if x >= 50.0 => Verdict::Hold,
        x if x >= 42.0 => Verdict::Wait,
        x if x >= 32.0 => Verdict::Reduce,
        _ => Verdict::Avoid,
    };
    why.push(tr!(
        l,
        "Puntuación global {:.1}/100 → {}",
        "Overall score {:.1}/100 → {}",
        o,
        verdict_name(l, v)
    ));

    if v == Verdict::Hold && s.technical.is_some_and(|t| t < 40.0) {
        v = Verdict::Wait;
        why.push(tr!(
            l,
            "Señal técnica débil ({:.0}): mejor esperar un mejor punto de entrada",
            "Weak technical signal ({:.0}): better to wait for a better entry",
            s.technical.unwrap_or(0.0)
        ));
    }
    let overextended = c.ind.is_some_and(|i| {
        fin(i.rsi14).is_some_and(|r| r >= 75.0)
            || c.price.zip(fin(i.sma50)).is_some_and(|(p, m)| m > 0.0 && p / m - 1.0 > 0.20)
    });
    if overextended && matches!(v, Verdict::Buy | Verdict::Accumulate) {
        let down = if v == Verdict::Buy { Verdict::Accumulate } else { Verdict::Wait };
        why.push(tr!(
            l,
            "Precio sobreextendido (RSI ≥ 75 o > 20 % sobre la SMA50): {} en vez de {}",
            "Overextended price (RSI ≥ 75 or > 20% above SMA50): {} instead of {}",
            verdict_name(l, down),
            verdict_name(l, v)
        ));
        v = down;
    }
    if v == Verdict::Buy && s.risk.is_some_and(|r| r >= 75.0) {
        v = Verdict::Accumulate;
        why.push(tr!(
            l,
            "Riesgo muy alto ({:.0}/100): entrada escalonada en vez de compra de golpe",
            "Very high risk ({:.0}/100): staged entry instead of a lump-sum buy",
            s.risk.unwrap_or(0.0)
        ));
    }
    if coverage < 0.4 {
        let capped = match v {
            Verdict::Buy | Verdict::Accumulate => Verdict::Hold,
            Verdict::Reduce | Verdict::Avoid => Verdict::Wait,
            other => other,
        };
        if capped != v {
            why.push(tr!(
                l,
                "Cobertura de datos baja ({:.0} %): veredicto limitado a {}",
                "Low data coverage ({:.0}%): verdict capped at {}",
                coverage * 100.0,
                verdict_name(l, capped)
            ));
            v = capped;
        }
    }
    (v, why)
}

pub fn verdict_name(l: Lang, v: Verdict) -> &'static str {
    match v {
        Verdict::Buy => l.t("comprar", "buy"),
        Verdict::Accumulate => l.t("acumular", "accumulate"),
        Verdict::Hold => l.t("mantener", "hold"),
        Verdict::Wait => l.t("esperar", "wait"),
        Verdict::Reduce => l.t("reducir", "reduce"),
        Verdict::Avoid => l.t("evitar", "avoid"),
    }
}

/// Posición del veredicto en una escala ordenada (Buy = 5 … Avoid = 0), para medir distancias.
pub fn verdict_rank(v: Verdict) -> i32 {
    match v {
        Verdict::Buy => 5,
        Verdict::Accumulate => 4,
        Verdict::Hold => 3,
        Verdict::Wait => 2,
        Verdict::Reduce => 1,
        Verdict::Avoid => 0,
    }
}

/// Confianza 0–1: sube con la cobertura y con la distancia al centro; baja si las señales se
/// contradicen y si la cotización es antigua.
fn confidence(overall: f64, coverage: f64, favorable: &[f64], snap: &AssetSnapshot) -> f32 {
    if favorable.is_empty() {
        return 0.1;
    }
    let mean = favorable.iter().sum::<f64>() / favorable.len() as f64;
    let var = favorable.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / favorable.len() as f64;
    let dispersion = (var.sqrt() / 30.0).clamp(0.0, 1.0);
    let mut c = 0.30 + 0.45 * coverage + 0.15 * ((overall - 50.0).abs() / 30.0).clamp(0.0, 1.0)
        - 0.20 * dispersion;
    if let Some(q) = &snap.quote {
        let stale = (snap.generated_at - q.timestamp).num_hours() >= 24
            && !matches!(q.market_state, domain::MarketState::Closed);
        if stale || q.delay_secs.is_some_and(|d| d >= 3600) {
            c -= 0.05;
        }
    } else {
        c -= 0.10;
    }
    (c.clamp(0.1, 0.85) * 100.0).round() as f32 / 100.0
}

#[cfg(test)]
#[path = "scoring_tests.rs"]
mod tests;
