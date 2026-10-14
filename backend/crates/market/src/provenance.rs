//! Procedencia de las respuestas que el contrato devuelve "desnudas" (`Candle[]`).
//!
//! `MarketData::history` devuelve solo velas, así que el servicio anota aquí qué proveedor las
//! sirvió. Las rutas HTTP lo leen para poner `X-Data-Source`, `X-Data-Source-Url` y
//! `X-Data-As-Of` y `X-Data-Interval`. Es una tabla acotada y con caducidad, común a todo el proceso.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use domain::{AssetId, Candle, Interval, Range};

const TTL: Duration = Duration::from_secs(30 * 60);
const MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryOrigin {
    pub source: String,
    pub source_url: Option<String>,
    /// Inicio de la última vela (dato del proveedor).
    pub as_of: Option<DateTime<Utc>>,
    /// Intervalo realmente servido (puede ser mayor que el pedido si el proveedor no admite esa
    /// combinación rango/intervalo y se subió de intervalo). `None` si no se puede deducir.
    pub interval: Option<Interval>,
}

/// Intervalo de una serie de velas, deducido de la mediana de la separación entre velas (robusta
/// frente a fines de semana, festivos y huecos nocturnos). Así vale igual para cualquier proveedor
/// sin tocar el trait: es lo que realmente se sirvió, no lo que se pidió. `None` con < 3 velas.
pub fn infer_interval(candles: &[Candle]) -> Option<Interval> {
    if candles.len() < 3 {
        return None;
    }
    let mut gaps: Vec<i64> = candles.windows(2).map(|w| w[1].t - w[0].t).filter(|g| *g > 0).collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    let median = gaps[(gaps.len() - 1) / 2] as f64; // mediana INFERIOR: con 2 separaciones [1 d, 3 d] no gana el fin de semana
    [
        Interval::M1,
        Interval::M5,
        Interval::M15,
        Interval::M30,
        Interval::H1,
        Interval::D1,
        Interval::W1,
        Interval::Mo1,
    ]
    .into_iter()
    .min_by(|a, b| {
        let d = |i: &Interval| (median / i.seconds() as f64).ln().abs();
        d(a).total_cmp(&d(b))
    })
}

type Table = Mutex<HashMap<String, (Instant, HistoryOrigin)>>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn key(id: &AssetId, range: Range, interval: Interval) -> String {
    format!("{id}|{}|{}", range.as_str(), interval.as_str())
}

pub fn record_history(id: &AssetId, range: Range, interval: Interval, origin: HistoryOrigin) {
    let mut t = table().lock().unwrap_or_else(|p| p.into_inner());
    if t.len() >= MAX_ENTRIES {
        let now = Instant::now();
        t.retain(|_, (at, _)| now.duration_since(*at) < TTL);
        if t.len() >= MAX_ENTRIES {
            t.clear();
        }
    }
    t.insert(key(id, range, interval), (Instant::now(), origin));
}

/// Proveedor que sirvió el último histórico de `id` con ese rango e intervalo.
pub fn history_origin(id: &AssetId, range: Range, interval: Interval) -> Option<HistoryOrigin> {
    let t = table().lock().unwrap_or_else(|p| p.into_inner());
    t.get(&key(id, range, interval))
        .filter(|(at, _)| at.elapsed() < TTL)
        .map(|(_, o)| o.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::AssetClass;

    #[test]
    fn records_and_reads_back() {
        let id = AssetId::new(AssetClass::Stock, "PROVTEST");
        assert!(history_origin(&id, Range::Y1, Interval::D1).is_none());
        let o = HistoryOrigin { source: "yahoo".into(), source_url: Some("https://x".into()), as_of: None, interval: Some(Interval::D1) };
        record_history(&id, Range::Y1, Interval::D1, o.clone());
        assert_eq!(history_origin(&id, Range::Y1, Interval::D1), Some(o));
        assert!(history_origin(&id, Range::M1, Interval::D1).is_none());
    }

    fn at(ts: &[i64]) -> Vec<Candle> {
        ts.iter().map(|t| Candle { t: *t, o: 1.0, h: 1.0, l: 1.0, c: 1.0, v: None }).collect()
    }

    #[test]
    fn a_short_series_with_a_weekend_gap_is_still_daily() {
        // Vie, lun, mar: separaciones [1 d, 3 d]. Con la mediana SUPERIOR salía 3 d -> «1wk».
        let day = 86_400;
        assert_eq!(infer_interval(&at(&[0, day, 4 * day])), Some(Interval::D1));
        assert_eq!(infer_interval(&at(&[0, 3 * day, 4 * day])), Some(Interval::D1));
        // Semanal con un festivo (lunes festivo: 8 d) y 3 velas.
        assert_eq!(infer_interval(&at(&[0, 7 * day, 15 * day])), Some(Interval::W1));
    }

    #[test]
    fn infers_the_served_interval_from_candle_spacing() {
        let day = 86_400;
        // Diario de bolsa: 5 velas por semana (el hueco del fin de semana no cambia la mediana).
        let stock: Vec<i64> = (0..60).filter(|d| d % 7 < 5).map(|d| d * day).collect();
        assert_eq!(infer_interval(&at(&stock)), Some(Interval::D1));
        // 5 min con hueco nocturno.
        let intraday: Vec<i64> = (0..78).map(|k| k * 300).chain((0..78).map(|k| day + k * 300)).collect();
        assert_eq!(infer_interval(&at(&intraday)), Some(Interval::M5));
        let weekly: Vec<i64> = (0..30).map(|k| k * 7 * day).collect();
        assert_eq!(infer_interval(&at(&weekly)), Some(Interval::W1));
        let monthly: Vec<i64> = (0..30).map(|k| k * [31, 28, 30][(k % 3) as usize] * day).collect();
        assert_eq!(infer_interval(&at(&monthly)), Some(Interval::Mo1));
        assert_eq!(infer_interval(&at(&[0, 60])), None);
        assert_eq!(infer_interval(&[]), None);
    }
}
