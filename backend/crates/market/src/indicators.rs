//! Indicadores técnicos puros sobre velas diarias (sin I/O).
//!
//! Convenciones (las consume `ai::scoring` y el frontend):
//! - Rendimientos y distancias son fracciones (0.05 = +5 %).
//! - `max_drawdown1y` es **negativo o cero** (-0.34 = caída máxima del 34 %), igual que
//!   `dist_from_high52w`, que también es ≤ 0.
//! - `supports` y `resistances` van ordenados del más cercano al precio al más lejano.
//! - Los periodos de calendario (1s, 1m, 3m, 1a, 52 semanas) se miden con las marcas de tiempo
//!   de las velas, no contando sesiones, para que valgan igual en bolsa y en cripto (24/7).

use chrono::{DateTime, Datelike, Months, TimeZone, Utc};
use domain::{Candle, Indicators, Trend};

const DAY: i64 = 86_400;

// ---------------------------------------------------------------------------------------------
// Medias móviles
// ---------------------------------------------------------------------------------------------

/// Media simple de los últimos `n` valores.
pub fn sma(values: &[f64], n: usize) -> Option<f64> {
    if n == 0 || values.len() < n {
        return None;
    }
    Some(values[values.len() - n..].iter().sum::<f64>() / n as f64)
}

/// Serie de EMA. `out[i]` corresponde a `values[n - 1 + i]`; la semilla es la SMA de los primeros
/// `n` valores (convención de TradingView/StockCharts).
pub fn ema_series(values: &[f64], n: usize) -> Vec<f64> {
    if n == 0 || values.len() < n {
        return Vec::new();
    }
    let k = 2.0 / (n as f64 + 1.0);
    let mut prev = values[..n].iter().sum::<f64>() / n as f64;
    let mut out = Vec::with_capacity(values.len() - n + 1);
    out.push(prev);
    for &v in &values[n..] {
        prev += k * (v - prev);
        out.push(prev);
    }
    out
}

pub fn ema(values: &[f64], n: usize) -> Option<f64> {
    ema_series(values, n).last().copied()
}

// ---------------------------------------------------------------------------------------------
// Osciladores
// ---------------------------------------------------------------------------------------------

/// RSI con suavizado de Wilder. Necesita al menos `period + 1` cierres.
pub fn rsi(closes: &[f64], period: usize) -> Option<f64> {
    if period == 0 || closes.len() <= period {
        return None;
    }
    let p = period as f64;
    let (mut gain, mut loss) = (0.0, 0.0);
    for w in closes[..=period].windows(2) {
        let d = w[1] - w[0];
        if d >= 0.0 {
            gain += d;
        } else {
            loss -= d;
        }
    }
    gain /= p;
    loss /= p;
    for w in closes[period..].windows(2) {
        let d = w[1] - w[0];
        let (g, l) = if d >= 0.0 { (d, 0.0) } else { (0.0, -d) };
        gain = (gain * (p - 1.0) + g) / p;
        loss = (loss * (p - 1.0) + l) / p;
    }
    Some(if loss == 0.0 {
        if gain == 0.0 { 50.0 } else { 100.0 }
    } else {
        100.0 - 100.0 / (1.0 + gain / loss)
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Macd {
    pub macd: f64,
    pub signal: f64,
    pub hist: f64,
}

/// MACD (12, 26, 9 por defecto). Necesita `slow + signal - 1` cierres.
pub fn macd(closes: &[f64], fast: usize, slow: usize, signal: usize) -> Option<Macd> {
    if fast == 0 || fast >= slow {
        return None;
    }
    let fast_s = ema_series(closes, fast);
    let slow_s = ema_series(closes, slow);
    if slow_s.is_empty() {
        return None;
    }
    // Alinea por el final: slow_s[j] ↔ closes[slow-1+j] ↔ fast_s[slow-fast+j].
    let offset = slow - fast;
    let line: Vec<f64> = slow_s
        .iter()
        .enumerate()
        .map(|(j, s)| fast_s[offset + j] - s)
        .collect();
    let sig = ema_series(&line, signal);
    let (m, s) = (line.last()?, sig.last()?);
    Some(Macd {
        macd: *m,
        signal: *s,
        hist: m - s,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bollinger {
    pub upper: f64,
    pub middle: f64,
    pub lower: f64,
}

/// Bandas de Bollinger con desviación típica poblacional (la usan TradingView y StockCharts).
pub fn bollinger(closes: &[f64], n: usize, k: f64) -> Option<Bollinger> {
    if n == 0 || closes.len() < n {
        return None;
    }
    let w = &closes[closes.len() - n..];
    let mean = w.iter().sum::<f64>() / n as f64;
    let var = w.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    Some(Bollinger {
        upper: mean + k * sd,
        middle: mean,
        lower: mean - k * sd,
    })
}

/// ATR con suavizado de Wilder. Necesita `n + 1` velas.
pub fn atr(candles: &[Candle], n: usize) -> Option<f64> {
    if n == 0 || candles.len() <= n {
        return None;
    }
    let tr = |prev: &Candle, c: &Candle| -> f64 {
        (c.h - c.l)
            .max((c.h - prev.c).abs())
            .max((c.l - prev.c).abs())
    };
    let trs: Vec<f64> = candles.windows(2).map(|w| tr(&w[0], &w[1])).collect();
    let nf = n as f64;
    let mut a = trs[..n].iter().sum::<f64>() / nf;
    for t in &trs[n..] {
        a = (a * (nf - 1.0) + t) / nf;
    }
    Some(a)
}

// ---------------------------------------------------------------------------------------------
// Riesgo
// ---------------------------------------------------------------------------------------------

/// Volatilidad anualizada: desviación típica muestral de los últimos `window` rendimientos
/// logarítmicos × √`periods_per_year`.
pub fn annualized_volatility(closes: &[f64], window: usize, periods_per_year: f64) -> Option<f64> {
    if window < 2 || closes.len() <= window {
        return None;
    }
    let tail = &closes[closes.len() - window - 1..];
    let rets: Vec<f64> = tail.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    if rets.iter().any(|r| !r.is_finite()) {
        return None;
    }
    let mean = rets.iter().sum::<f64>() / rets.len() as f64;
    let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
    Some(var.sqrt() * periods_per_year.sqrt())
}

/// Máxima caída pico-valle como fracción negativa (0.0 si nunca cae).
pub fn max_drawdown(closes: &[f64]) -> Option<f64> {
    let mut it = closes.iter().copied();
    let mut peak = it.next()?;
    let mut worst = 0.0_f64;
    for c in it {
        peak = peak.max(c);
        worst = worst.min(c / peak - 1.0);
    }
    Some(worst)
}

/// 252 sesiones al año en bolsa; 365 si la serie incluye fines de semana (cripto).
pub fn periods_per_year(candles: &[Candle]) -> f64 {
    let tail = &candles[candles.len().saturating_sub(60)..];
    if tail.is_empty() {
        return 252.0;
    }
    let weekend = tail
        .iter()
        .filter(|c| {
            Utc.timestamp_opt(c.t, 0)
                .single()
                .map(|d| d.weekday().number_from_monday() >= 6)
                .unwrap_or(false)
        })
        .count();
    if weekend as f64 / tail.len() as f64 > 0.1 {
        365.0
    } else {
        252.0
    }
}

// ---------------------------------------------------------------------------------------------
// Tendencia, soportes y resistencias
// ---------------------------------------------------------------------------------------------

/// Tendencia según el precio respecto a SMA50/SMA200 y la pendiente de la SMA50 (10 sesiones).
pub fn trend(closes: &[f64]) -> Option<Trend> {
    let price = *closes.last()?;
    let sma50 = sma(closes, 50)?;
    let sign = |x: f64| {
        if x > 0.0 {
            1
        } else if x < 0.0 {
            -1
        } else {
            0
        }
    };

    let slope50 = (closes.len() >= 60)
        .then(|| sma(&closes[..closes.len() - 10], 50))
        .flatten()
        .map(|prev| sma50 / prev - 1.0);
    let slope_score = match slope50 {
        Some(s) if s > 0.005 => 1,
        Some(s) if s < -0.005 => -1,
        _ => 0,
    };
    let mut score = sign(price - sma50) + slope_score;

    Some(match sma(closes, 200) {
        Some(sma200) => {
            score += sign(price - sma200) + sign(sma50 - sma200);
            match score {
                4 => Trend::StrongUp,
                2..=3 => Trend::Up,
                -3..=-2 => Trend::Down,
                -4 => Trend::StrongDown,
                _ => Trend::Sideways,
            }
        }
        None => match score {
            2 => Trend::Up,
            -2 => Trend::Down,
            _ => Trend::Sideways,
        },
    })
}

/// Soportes y resistencias: pivotes locales (máximo/mínimo de una ventana de ±`k` velas)
/// agrupados cuando distan menos de `tol` (fracción), y partidos según el precio actual.
/// Devuelve `(soportes, resistencias)` con los `top` más cercanos a cada lado.
pub fn support_resistance(
    candles: &[Candle],
    k: usize,
    tol: f64,
    top: usize,
) -> (Vec<f64>, Vec<f64>) {
    let Some(last) = candles.last() else {
        return (vec![], vec![]);
    };
    if candles.len() < 2 * k + 1 {
        return (vec![], vec![]);
    }
    let mut levels: Vec<f64> = Vec::new();
    for i in k..candles.len() - k {
        let win = &candles[i - k..=i + k];
        if win.iter().all(|c| c.h <= candles[i].h) {
            levels.push(candles[i].h);
        }
        if win.iter().all(|c| c.l >= candles[i].l) {
            levels.push(candles[i].l);
        }
    }
    levels.sort_by(f64::total_cmp);

    // Agrupa niveles contiguos; el nivel del grupo es la media (pesa por número de toques).
    let mut clusters: Vec<f64> = Vec::new();
    let mut group: Vec<f64> = Vec::new();
    for l in levels {
        match group.first() {
            Some(first) if l / first - 1.0 > tol => {
                clusters.push(group.iter().sum::<f64>() / group.len() as f64);
                group = vec![l];
            }
            _ => group.push(l),
        }
    }
    if !group.is_empty() {
        clusters.push(group.iter().sum::<f64>() / group.len() as f64);
    }

    let price = last.c;
    // Un nivel casi pegado al precio no sirve ni de soporte ni de resistencia.
    let near = tol / 3.0;
    let mut sup: Vec<f64> = clusters
        .iter()
        .copied()
        .filter(|l| *l < price * (1.0 - near))
        .collect();
    let mut res: Vec<f64> = clusters
        .iter()
        .copied()
        .filter(|l| *l > price * (1.0 + near))
        .collect();
    sup.sort_by(|a, b| b.total_cmp(a));
    res.sort_by(f64::total_cmp);
    sup.truncate(top);
    res.truncate(top);
    (sup, res)
}

// ---------------------------------------------------------------------------------------------
// Cálculo completo
// ---------------------------------------------------------------------------------------------

fn at_or_before(candles: &[Candle], ts: i64) -> Option<f64> {
    let idx = candles.partition_point(|c| c.t <= ts);
    idx.checked_sub(1).map(|i| candles[i].c)
}

fn sub_months(ts: i64, months: u32) -> Option<i64> {
    let dt = DateTime::<Utc>::from_timestamp(ts, 0)?;
    dt.checked_sub_months(Months::new(months))
        .map(|d| d.timestamp())
}

/// Limpia la serie: ordena por tiempo, descarta velas inválidas y repara máximos/mínimos rotos.
fn sanitize(candles: &[Candle]) -> Vec<Candle> {
    let mut v: Vec<Candle> = candles
        .iter()
        .filter(|c| c.c.is_finite() && c.c > 0.0)
        .map(|c| {
            let o = if c.o.is_finite() && c.o > 0.0 {
                c.o
            } else {
                c.c
            };
            let h = if c.h.is_finite() && c.h > 0.0 {
                c.h.max(o).max(c.c)
            } else {
                o.max(c.c)
            };
            let l = if c.l.is_finite() && c.l > 0.0 {
                c.l.min(o).min(c.c)
            } else {
                o.min(c.c)
            };
            Candle { o, h, l, ..*c }
        })
        .collect();
    v.sort_by_key(|c| c.t);
    v.dedup_by_key(|c| c.t);
    v
}

/// Calcula todos los indicadores del dominio a partir de velas diarias (idealmente 2 años).
/// Lo que no se pueda calcular por falta de histórico queda en `None`/vacío.
pub fn compute(candles: &[Candle]) -> Indicators {
    let candles = sanitize(candles);
    let mut out = Indicators::default();
    let Some(last) = candles.last().copied() else {
        return out;
    };
    let closes: Vec<f64> = candles.iter().map(|c| c.c).collect();

    out.as_of = DateTime::<Utc>::from_timestamp(last.t, 0);
    out.last_close = Some(last.c);
    out.sma20 = sma(&closes, 20);
    out.sma50 = sma(&closes, 50);
    out.sma200 = sma(&closes, 200);
    out.ema12 = ema(&closes, 12);
    out.ema26 = ema(&closes, 26);
    out.rsi14 = rsi(&closes, 14);
    if let Some(m) = macd(&closes, 12, 26, 9) {
        out.macd = Some(m.macd);
        out.macd_signal = Some(m.signal);
        out.macd_hist = Some(m.hist);
    }
    if let Some(b) = bollinger(&closes, 20, 2.0) {
        out.bb_upper = Some(b.upper);
        out.bb_middle = Some(b.middle);
        out.bb_lower = Some(b.lower);
    }
    out.atr14 = atr(&candles, 14);
    out.volatility30d = annualized_volatility(&closes, 30, periods_per_year(&candles));

    // Ventana de un año hasta la última vela.
    let year_start = last.t - 365 * DAY;
    let first_in_year = candles.partition_point(|c| c.t <= year_start);
    let year = &candles[first_in_year..];
    out.max_drawdown1y = max_drawdown(&year.iter().map(|c| c.c).collect::<Vec<_>>());
    // Solo se publican 52 semanas si el histórico las cubre (≥ ~11 meses), si no sería engañoso.
    if candles.first().is_some_and(|c| c.t <= last.t - 335 * DAY) {
        let high = year.iter().map(|c| c.h).fold(f64::MIN, f64::max);
        let low = year.iter().map(|c| c.l).fold(f64::MAX, f64::min);
        out.high52w = Some(high);
        out.low52w = Some(low);
        out.dist_from_high52w = Some(last.c / high - 1.0);
    }

    let ret = |target: Option<i64>| {
        target
            .and_then(|t| at_or_before(&candles, t))
            .map(|past| last.c / past - 1.0)
    };
    out.return1w = ret(Some(last.t - 7 * DAY));
    out.return1m = ret(sub_months(last.t, 1));
    out.return3m = ret(sub_months(last.t, 3));
    out.return1y = ret(sub_months(last.t, 12));
    let jan1 = DateTime::<Utc>::from_timestamp(last.t, 0)
        .and_then(|d| Utc.with_ymd_and_hms(d.year(), 1, 1, 0, 0, 0).single())
        .map(|d| d.timestamp() - 1);
    out.return_ytd = ret(jan1);

    // Volumen: las series sin volumen (índices, divisas) devuelven 0 o None.
    let vols: Vec<f64> = candles
        .iter()
        .rev()
        .take(20)
        .filter_map(|c| c.v.filter(|v| *v > 0.0))
        .collect();
    if vols.len() >= 10 {
        let avg = vols.iter().sum::<f64>() / vols.len() as f64;
        out.volume_avg20 = Some(avg);
        out.relative_volume = last.v.filter(|v| *v > 0.0).map(|v| v / avg);
    }

    out.trend = trend(&closes);
    let recent = &candles[candles.len().saturating_sub(252)..];
    let (sup, res) = support_resistance(recent, 5, 0.015, 3);
    out.supports = sup;
    out.resistances = res;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) {
        assert!(
            (a - b).abs() <= tol,
            "esperado {b}, obtenido {a} (tol {tol})"
        );
    }

    /// Serie de la hoja de referencia de StockCharts (Wilder, 14). Sus RSI publicados (70.53 y
    /// 37.77) salen de promedios redondeados; los valores exactos de abajo se contrastaron con una
    /// implementación independiente en Python.
    const WILDER: [f64; 33] = [
        44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03, 45.61,
        46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64, 46.21, 46.25, 45.71, 46.45, 45.78, 45.35,
        44.03, 44.18, 44.22, 44.57, 43.42, 42.66, 43.13,
    ];

    #[test]
    fn sma_and_ema_basics() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sma(&v, 5), Some(3.0));
        assert_eq!(sma(&v, 2), Some(4.5));
        assert_eq!(sma(&v, 6), None);
        // EMA(3): semilla = (1+2+3)/3 = 2; k = 0.5 → 3, 4.
        assert_eq!(ema_series(&v, 3), vec![2.0, 3.0, 4.0]);
        assert_eq!(ema(&v, 6), None);
    }

    #[test]
    fn rsi_matches_stockcharts_reference() {
        approx(rsi(&WILDER[..15], 14).unwrap(), 70.464_135_021, 1e-6);
        approx(rsi(&WILDER[..16], 14).unwrap(), 66.249_618_554, 1e-6);
        approx(rsi(&WILDER, 14).unwrap(), 37.788_771_982, 1e-6);
        assert_eq!(rsi(&WILDER[..14], 14), None);
    }

    #[test]
    fn rsi_extremes() {
        let up: Vec<f64> = (1..=30).map(f64::from).collect();
        assert_eq!(rsi(&up, 14), Some(100.0));
        let flat = vec![10.0; 30];
        assert_eq!(rsi(&flat, 14), Some(50.0));
        let down: Vec<f64> = (1..=30).rev().map(f64::from).collect();
        assert_eq!(rsi(&down, 14), Some(0.0));
    }

    #[test]
    fn macd_on_linear_series() {
        // En una recta de pendiente 1, EMA(n) = precio - (n-1)/2, así que MACD = (26-12)/2 = 7
        // y la señal converge al mismo valor (histograma 0).
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let m = macd(&v, 12, 26, 9).unwrap();
        approx(m.macd, 7.0, 1e-9);
        approx(m.signal, 7.0, 1e-9);
        approx(m.hist, 0.0, 1e-9);
        assert!(macd(&v[..33], 12, 26, 9).is_none());
        assert!(macd(&v[..34], 12, 26, 9).is_some());
    }

    #[test]
    fn bollinger_known_values() {
        // 20 valores 1..=20: media 10.5, σ poblacional = sqrt((20²-1)/12) = 5.7663.
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        let b = bollinger(&v, 20, 2.0).unwrap();
        approx(b.middle, 10.5, 1e-12);
        approx(b.upper, 10.5 + 2.0 * 5.766_281_297, 1e-6);
        approx(b.lower, 10.5 - 2.0 * 5.766_281_297, 1e-6);
    }

    fn candle(t: i64, o: f64, h: f64, l: f64, c: f64) -> Candle {
        Candle {
            t,
            o,
            h,
            l,
            c,
            v: Some(1000.0),
        }
    }

    #[test]
    fn atr_known_values() {
        // Rango constante de 2 y sin huecos: TR = 2 siempre → ATR = 2.
        let cs: Vec<Candle> = (0..30)
            .map(|i| candle(i * DAY, 10.0, 11.0, 9.0, 10.0))
            .collect();
        approx(atr(&cs, 14).unwrap(), 2.0, 1e-12);
        // Un hueco al alza: TR = |h - cierre previo| domina.
        let mut gap = cs.clone();
        gap.push(candle(30 * DAY, 15.0, 16.0, 14.0, 15.0)); // TR = 6
        approx(atr(&gap, 14).unwrap(), (2.0 * 13.0 + 6.0) / 14.0, 1e-12);
        assert!(atr(&cs[..14], 14).is_none());
    }

    /// Serie sintética con valores de referencia calculados con una implementación independiente
    /// en Python (misma fórmula, sin compartir código).
    #[test]
    fn matches_independent_reference_on_synthetic_series() {
        let n = 120;
        let c: Vec<f64> = (0..n)
            .map(|i| 100.0 + 10.0 * (i as f64 / 5.0).sin() + i as f64 * 0.1)
            .collect();
        let candles: Vec<Candle> = c
            .iter()
            .enumerate()
            .map(|(i, &x)| Candle {
                t: i as i64 * DAY,
                o: x,
                h: x + 1.0 + 0.5 * (i as f64).cos(),
                l: x - 1.0 - 0.5 * (i as f64 * 1.3).sin(),
                c: x,
                v: None,
            })
            .collect();
        approx(ema(&c, 12).unwrap(), 105.841_229_411_862, 1e-9);
        approx(ema(&c, 26).unwrap(), 108.323_882_028_931, 1e-9);
        let m = macd(&c, 12, 26, 9).unwrap();
        approx(m.macd, -2.482_652_617_069, 1e-9);
        approx(m.signal, -0.967_687_871_727, 1e-9);
        approx(m.hist, -1.514_964_745_342, 1e-9);
        let b = bollinger(&c, 20, 2.0).unwrap();
        approx(b.upper, 125.627_076_587_05, 1e-8);
        approx(b.middle, 111.364_522_770_10, 1e-8);
        approx(b.lower, 97.101_968_953_15, 1e-8);
        approx(atr(&candles, 14).unwrap(), 2.361_379_879_206, 1e-9);
    }

    #[test]
    fn volatility_and_drawdown() {
        // Rendimientos alternos ±1 % en log: σ muestral conocida.
        let mut closes: Vec<f64> = vec![100.0];
        for i in 0..30 {
            let last = *closes.last().unwrap();
            closes.push(if i % 2 == 0 { last * 1.01 } else { last / 1.01 });
        }
        let rets: Vec<f64> = closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
        let mean = rets.iter().sum::<f64>() / 30.0;
        let sd = (rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / 29.0).sqrt();
        approx(
            annualized_volatility(&closes, 30, 252.0).unwrap(),
            sd * 252f64.sqrt(),
            1e-12,
        );
        assert!(annualized_volatility(&closes[..30], 30, 252.0).is_none());

        assert_eq!(max_drawdown(&[1.0, 2.0, 3.0]), Some(0.0));
        approx(
            max_drawdown(&[100.0, 120.0, 60.0, 90.0, 80.0]).unwrap(),
            -0.5,
            1e-12,
        );
    }

    #[test]
    fn calendar_returns_and_52w() {
        // 2 años de cierres diarios 1..=730 (día 0 = 2024-01-01), siempre al alza.
        let start = Utc
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let cs: Vec<Candle> = (0..730)
            .map(|i| {
                let c = 100.0 + i as f64;
                candle(start + i * DAY, c, c + 1.0, c - 1.0, c)
            })
            .collect();
        let ind = compute(&cs);
        let last = 100.0 + 729.0; // 2025-12-30
        approx(ind.last_close.unwrap(), last, 1e-9);
        approx(ind.return1w.unwrap(), last / (last - 7.0) - 1.0, 1e-12);
        // 1 mes antes de 2025-12-30 = 2025-11-30 → 30 días atrás.
        approx(ind.return1m.unwrap(), last / (last - 30.0) - 1.0, 1e-12);
        approx(ind.return3m.unwrap(), last / (last - 91.0) - 1.0, 1e-12);
        approx(ind.return1y.unwrap(), last / (last - 365.0) - 1.0, 1e-12);
        // YTD: cierre del 2024-12-31 (día 365) → el 2025-01-01 es el día 366; 2025-12-30 = 729.
        approx(ind.return_ytd.unwrap(), last / (100.0 + 365.0) - 1.0, 1e-12);
        approx(ind.high52w.unwrap(), last + 1.0, 1e-9);
        approx(
            ind.dist_from_high52w.unwrap(),
            last / (last + 1.0) - 1.0,
            1e-12,
        );
        assert_eq!(ind.max_drawdown1y, Some(0.0));
        assert_eq!(ind.trend, Some(Trend::StrongUp));
        approx(ind.volume_avg20.unwrap(), 1000.0, 1e-9);
        approx(ind.relative_volume.unwrap(), 1.0, 1e-9);
    }

    #[test]
    fn short_history_leaves_none() {
        let cs: Vec<Candle> = (0..10)
            .map(|i| candle(i * DAY, 10.0, 11.0, 9.0, 10.0))
            .collect();
        let ind = compute(&cs);
        assert!(ind.sma20.is_none() && ind.rsi14.is_none() && ind.trend.is_none());
        assert!(
            ind.high52w.is_none(),
            "no debe inventarse 52 semanas con 10 días"
        );
        assert_eq!(ind.last_close, Some(10.0));
        assert!(compute(&[]).last_close.is_none());
    }

    #[test]
    fn trend_classification() {
        let up: Vec<f64> = (0..250).map(|i| 100.0 + i as f64).collect();
        assert_eq!(trend(&up), Some(Trend::StrongUp));
        let down: Vec<f64> = (0..250).map(|i| 400.0 - i as f64).collect();
        assert_eq!(trend(&down), Some(Trend::StrongDown));
        let flat = vec![100.0; 250];
        assert_eq!(trend(&flat), Some(Trend::Sideways));
        // Sin SMA200: solo precio vs SMA50 y pendiente.
        let short_up: Vec<f64> = (0..80).map(|i| 100.0 + i as f64).collect();
        assert_eq!(trend(&short_up), Some(Trend::Up));
        assert_eq!(trend(&short_up[..40]), None);
    }

    #[test]
    fn support_resistance_from_pivots() {
        // Onda que oscila entre ~90 y ~110 durante 120 sesiones y termina en 100.
        let cs: Vec<Candle> = (0..120)
            .map(|i| {
                let c = 100.0 + 10.0 * (i as f64 * std::f64::consts::PI / 10.0).sin();
                candle(i * DAY, c, c + 0.5, c - 0.5, c)
            })
            .collect();
        let mut cs = cs;
        cs.push(candle(120 * DAY, 100.0, 100.5, 99.5, 100.0));
        let (sup, res) = support_resistance(&cs, 5, 0.015, 3);
        assert_eq!(sup.len(), 1, "un único soporte agrupado: {sup:?}");
        assert_eq!(res.len(), 1, "una única resistencia agrupada: {res:?}");
        approx(sup[0], 89.5, 0.5);
        approx(res[0], 110.5, 0.5);
    }
}
