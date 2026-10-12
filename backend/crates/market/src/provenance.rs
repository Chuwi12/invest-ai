//! Procedencia de las respuestas que el contrato devuelve "desnudas" (`Candle[]`).
//!
//! `MarketData::history` devuelve solo velas, así que el servicio anota aquí qué proveedor las
//! sirvió. Las rutas HTTP lo leen para poner `X-Data-Source`, `X-Data-Source-Url` y
//! `X-Data-As-Of`. Es una tabla acotada y con caducidad, común a todo el proceso.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use domain::{AssetId, Interval, Range};

const TTL: Duration = Duration::from_secs(30 * 60);
const MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryOrigin {
    pub source: String,
    pub source_url: Option<String>,
    /// Inicio de la última vela (dato del proveedor).
    pub as_of: Option<DateTime<Utc>>,
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
        let o = HistoryOrigin { source: "yahoo".into(), source_url: Some("https://x".into()), as_of: None };
        record_history(&id, Range::Y1, Interval::D1, o.clone());
        assert_eq!(history_origin(&id, Range::Y1, Interval::D1), Some(o));
        assert!(history_origin(&id, Range::M1, Interval::D1).is_none());
    }
}
