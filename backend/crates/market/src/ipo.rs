//! Rendimiento posterior a una OPV: del precio de salida y la serie diaria desde el primer día
//! se calcula la rentabilidad del primer día y a 1, 3, 6 y 12 meses, frente a un índice.
//!
//! Convenciones (todas visibles en los campos de `IpoPerformance`):
//! - la rentabilidad «desde la OPV» parte del PRECIO DE SALIDA (lo que pagó quien recibió
//!   acciones en la colocación); `since_first_close_pct` parte del cierre del primer día (lo que
//!   obtiene quien compró en el mercado);
//! - el índice se mide desde el cierre de la sesión anterior a la OPV hasta la misma sesión que
//!   el valor, con el cierre ajustado de Yahoo (incluye dividendos);
//! - los precios de Yahoo vienen ajustados por escisiones; el precio de salida se ajusta con las
//!   escisiones posteriores a la OPV, y se avisa de ello;
//! - un horizonte aún no alcanzado queda a `null`; nunca se extrapola.

use chrono::{DateTime, Months, NaiveDate, Utc};
use domain::{Candle, IpoPerformance, IpoReturnPoint};

use crate::providers::yahoo::DailySeries;

/// Índices admitidos como referencia.
pub const BENCHMARKS: &[&str] = &["SPY", "IWM", "QQQ"];

fn date_of(c: &Candle) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp(c.t, 0).map(|d| d.date_naive())
}

fn pct(now: f64, base: f64) -> f64 {
    (now / base - 1.0) * 100.0
}

/// Último índice con fecha `<= d`.
fn last_on_or_before(dates: &[NaiveDate], d: NaiveDate) -> Option<usize> {
    dates.iter().rposition(|x| *x <= d)
}

pub struct PerfInput<'a> {
    pub symbol: &'a str,
    pub ipo_date: NaiveDate,
    pub offer_price: f64,
    pub offer_price_source: &'a str,
    pub series: &'a DailySeries,
    pub bench: &'a DailySeries,
    pub bench_name: &'a str,
    pub source_url: Option<String>,
    pub now: DateTime<Utc>,
}

pub fn compute(i: &PerfInput<'_>) -> Result<IpoPerformance, String> {
    if !(i.offer_price.is_finite() && i.offer_price > 0.0) {
        return Err("precio de salida no válido".into());
    }
    let mut warnings = Vec::new();
    let stock: Vec<(NaiveDate, &Candle)> = i
        .series
        .candles
        .iter()
        .filter_map(|c| date_of(c).map(|d| (d, c)))
        .filter(|(d, _)| *d >= i.ipo_date)
        .collect();
    let Some((first_date, first)) = stock.first().copied() else {
        return Err(format!("sin cotizaciones de {} desde {}", i.symbol, i.ipo_date));
    };
    let gap = (first_date - i.ipo_date).num_days();
    if gap > 5 {
        return Err(format!("la primera cotización es del {first_date}, {gap} días después de la OPV ({}): ¿fecha errónea?", i.ipo_date));
    }
    if gap > 0 {
        warnings.push(format!("la primera sesión disponible es {first_date}, no {}", i.ipo_date));
    }
    // Escisiones posteriores a la salida: Yahoo ya ajustó los precios, no el precio de salida.
    let mut offer = i.offer_price;
    for (ts, n, d) in &i.series.splits {
        if *ts >= first.t - 86_400 {
            offer *= d / n;
            warnings.push(format!("escisión {n}:{d} posterior a la OPV: precio de salida ajustado de {} a {:.4}", i.offer_price, offer));
        }
    }
    let dates: Vec<NaiveDate> = stock.iter().map(|s| s.0).collect();
    let closes: Vec<f64> = stock.iter().map(|s| s.1.c).collect();
    let first_close = first.c;
    let first_day_return_pct = pct(first_close, offer);
    let first_day_high_pct = pct(first.h, offer);
    if first_day_return_pct.abs() > 400.0 {
        warnings.push("rentabilidad del primer día extrema: revisar precio de salida y escisiones".into());
    }

    // Índice: cierre ajustado de la sesión previa a la OPV.
    let bench: Vec<(NaiveDate, f64)> = i
        .bench
        .candles
        .iter()
        .zip(&i.bench.adj_close)
        .filter_map(|(c, a)| date_of(c).map(|d| (d, *a)))
        .collect();
    let bdates: Vec<NaiveDate> = bench.iter().map(|b| b.0).collect();
    let base_idx = bdates.iter().rposition(|d| *d < i.ipo_date);
    if base_idx.is_none() {
        warnings.push(format!("sin cierre de {} previo a la OPV: sin comparación con el índice", i.bench_name));
    }
    let bench_ret = |day: NaiveDate| -> Option<f64> {
        let b = base_idx?;
        let e = last_on_or_before(&bdates, day)?;
        (e >= b).then(|| pct(bench[e].1, bench[b].1))
    };

    let last_date = *dates.last().unwrap_or(&first_date);
    let mut points = Vec::new();
    for (label, months) in [("1m", 1u32), ("3m", 3), ("6m", 6), ("12m", 12)] {
        let target = i.ipo_date.checked_add_months(Months::new(months)).unwrap_or(i.ipo_date);
        let reached = target <= last_date;
        let idx = reached.then(|| last_on_or_before(&dates, target)).flatten();
        let (price_date, price) = idx.map_or((None, None), |k| (Some(dates[k]), Some(closes[k])));
        let ret = price.map(|p| pct(p, offer));
        let b = price_date.and_then(bench_ret);
        points.push(IpoReturnPoint {
            horizon: label.into(),
            target_date: target,
            price_date,
            price,
            return_pct: ret,
            since_first_close_pct: price.map(|p| pct(p, first_close)),
            benchmark_return_pct: b,
            excess_pct: ret.zip(b).map(|(r, b)| r - b),
        });
    }

    let mut peak = first_close;
    let mut max_dd = 0.0f64;
    for c in &closes {
        peak = peak.max(*c);
        max_dd = max_dd.min(pct(*c, peak));
    }
    let latest = *closes.last().unwrap_or(&first_close);
    if (i.now.date_naive() - last_date).num_days() > 7 {
        warnings.push(format!("última sesión disponible: {last_date}"));
    }
    Ok(IpoPerformance {
        symbol: i.symbol.to_string(),
        ipo_date: first_date,
        offer_price: offer,
        offer_price_source: i.offer_price_source.to_string(),
        first_day_close: first_close,
        first_day_return_pct,
        first_day_high_pct,
        points,
        latest_price: latest,
        latest_date: last_date,
        latest_return_pct: pct(latest, offer),
        max_drawdown_pct: max_dd,
        benchmark: i.bench_name.to_string(),
        source: "yahoo".into(),
        source_url: i.source_url.clone(),
        warnings,
        as_of: i.now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn ts(date: NaiveDate) -> i64 {
        date.and_hms_opt(14, 30, 0).unwrap().and_utc().timestamp()
    }

    /// Serie con un cierre por día laborable que sube `step` cada día.
    fn series(start: NaiveDate, days: i64, first: f64, step: f64) -> DailySeries {
        let mut candles = Vec::new();
        let mut day = start;
        let mut px = first;
        for _ in 0..days {
            if day.format("%u").to_string().parse::<u32>().unwrap() < 6 {
                candles.push(Candle { t: ts(day), o: px, h: px * 1.05, l: px * 0.95, c: px, v: Some(1.0) });
                px += step;
            }
            day += chrono::Duration::days(1);
        }
        let adj_close = candles.iter().map(|c| c.c).collect();
        DailySeries { candles, adj_close, splits: vec![] }
    }

    fn input<'a>(s: &'a DailySeries, b: &'a DailySeries, now: DateTime<Utc>) -> PerfInput<'a> {
        PerfInput {
            symbol: "TEST",
            ipo_date: d(2025, 3, 3),
            offer_price: 10.0,
            offer_price_source: "manual",
            series: s,
            bench: b,
            bench_name: "SPY",
            source_url: None,
            now,
        }
    }

    #[test]
    fn horizons_and_benchmark() {
        let s = series(d(2025, 3, 3), 420, 12.0, 0.01);
        let b = series(d(2025, 2, 20), 440, 100.0, 0.1);
        let now = DateTime::<Utc>::from_timestamp(ts(d(2026, 4, 30)), 0).unwrap();
        let p = compute(&input(&s, &b, now)).unwrap();
        assert!((p.first_day_return_pct - 20.0).abs() < 1e-9);
        assert!((p.first_day_high_pct - 26.0).abs() < 1e-9);
        assert_eq!(p.points.len(), 4);
        let m1 = &p.points[0];
        assert_eq!(m1.target_date, d(2025, 4, 3));
        assert_eq!(m1.price_date, Some(d(2025, 4, 3)));
        assert!(m1.return_pct.unwrap() > 20.0);
        assert!(m1.benchmark_return_pct.is_some() && m1.excess_pct.is_some());
        assert!(p.points[3].price.is_some(), "12m alcanzado");
        assert!(p.max_drawdown_pct <= 0.0);
    }

    #[test]
    fn unreached_horizons_are_null_not_extrapolated() {
        let s = series(d(2025, 3, 3), 70, 12.0, 0.0);
        let b = series(d(2025, 2, 20), 100, 100.0, 0.0);
        let now = DateTime::<Utc>::from_timestamp(ts(d(2025, 5, 12)), 0).unwrap();
        let p = compute(&input(&s, &b, now)).unwrap();
        assert!(p.points[0].price.is_some());
        assert!(p.points[2].price.is_none() && p.points[2].return_pct.is_none());
        assert!(p.points[3].excess_pct.is_none());
    }

    #[test]
    fn reverse_split_adjusts_offer_price_and_warns() {
        let mut s = series(d(2025, 3, 3), 30, 100.0, 0.0);
        s.splits.push((ts(d(2025, 3, 10)), 1.0, 10.0));
        let b = series(d(2025, 2, 20), 60, 100.0, 0.0);
        let now = DateTime::<Utc>::from_timestamp(ts(d(2025, 4, 10)), 0).unwrap();
        let p = compute(&input(&s, &b, now)).unwrap();
        assert!((p.offer_price - 100.0).abs() < 1e-9);
        assert!(p.first_day_return_pct.abs() < 1e-9);
        assert!(p.warnings.iter().any(|w| w.contains("escisión")));
    }

    #[test]
    fn wrong_date_is_an_error() {
        let s = series(d(2025, 4, 1), 30, 12.0, 0.0);
        let b = series(d(2025, 2, 20), 60, 100.0, 0.0);
        let now = DateTime::<Utc>::from_timestamp(ts(d(2025, 5, 1)), 0).unwrap();
        assert!(compute(&input(&s, &b, now)).is_err());
        let mut i = input(&s, &b, now);
        i.offer_price = 0.0;
        assert!(compute(&i).is_err());
    }
}
