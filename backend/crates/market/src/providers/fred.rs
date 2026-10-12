//! FRED (Reserva Federal de St. Louis). Requiere `FRED_API_KEY` (gratuita).
//! Curva de tipos, tipo de la Fed, IPC, paro, VIX... Cada observación lleva la fecha que da FRED
//! (día de la observación, a las 00:00 UTC): las series son diarias/mensuales, no tick a tick.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use domain::{AssetClass, AssetId, Candle, Interval, MacroIndicator, MarketState, Quote, Range};
use reqwest::Client;
use serde_json::Value;
use url::Url;

use super::{Capability, PResult, Provider, ProviderError, ProviderInfo, from_ymd, num};
use crate::{
    http::{check_status, parse_json, send},
    secret::Secret,
};

const BASE: &str = "https://api.stlouisfed.org/fred/series/observations";

#[derive(Clone, Copy)]
enum Kind {
    Level,
    /// Variación interanual en % a partir de un índice mensual (IPC).
    YoY,
}

struct Def {
    key: &'static str,
    name: &'static str,
    series: &'static str,
    unit: &'static str,
    kind: Kind,
}

const fn d(
    key: &'static str,
    name: &'static str,
    series: &'static str,
    unit: &'static str,
    kind: Kind,
) -> Def {
    Def {
        key,
        name,
        series,
        unit,
        kind,
    }
}

const SERIES: &[Def] = &[
    d(
        "fed_funds",
        "Tipo efectivo de los fondos federales",
        "DFF",
        "%",
        Kind::Level,
    ),
    d(
        "us3m",
        "Rendimiento de la letra de EE. UU. a 3 meses",
        "DGS3MO",
        "%",
        Kind::Level,
    ),
    d(
        "us2y",
        "Rendimiento del bono de EE. UU. a 2 años",
        "DGS2",
        "%",
        Kind::Level,
    ),
    d(
        "us5y",
        "Rendimiento del bono de EE. UU. a 5 años",
        "DGS5",
        "%",
        Kind::Level,
    ),
    d(
        "us10y",
        "Rendimiento del bono de EE. UU. a 10 años",
        "DGS10",
        "%",
        Kind::Level,
    ),
    d(
        "us30y",
        "Rendimiento del bono de EE. UU. a 30 años",
        "DGS30",
        "%",
        Kind::Level,
    ),
    d(
        "curve_10y2y",
        "Diferencial de la curva 10 años - 2 años",
        "T10Y2Y",
        "pp",
        Kind::Level,
    ),
    d(
        "cpi_yoy",
        "Inflación (IPC EE. UU., interanual)",
        "CPIAUCSL",
        "%",
        Kind::YoY,
    ),
    d(
        "unemployment",
        "Tasa de paro de EE. UU.",
        "UNRATE",
        "%",
        Kind::Level,
    ),
    d(
        "vix",
        "Índice de volatilidad VIX",
        "VIXCLS",
        "puntos",
        Kind::Level,
    ),
    d(
        "ecb_rate",
        "Tipo de la facilidad de depósito del BCE",
        "ECBDFR",
        "%",
        Kind::Level,
    ),
];

pub struct Fred {
    http: Client,
    info: ProviderInfo,
    key: Option<Secret>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Obs {
    pub date: String,
    pub value: f64,
}

impl Fred {
    pub fn new(http: Client, key: Option<Secret>) -> Self {
        Self {
            http,
            info: ProviderInfo {
                name: "fred",
                requires_key: true,
                has_key: key.is_some(),
                capabilities: vec![Capability::Macro, Capability::Quote, Capability::History],
                classes: vec![AssetClass::Bond],
                // Límite oficial: 120 peticiones/min.
                rate_per_sec: 2.0,
                burst: 10,
            },
            key,
        }
    }

    async fn fetch_indicator(&self, def: &'static Def) -> Option<MacroIndicator> {
        let limit = if matches!(def.kind, Kind::YoY) { 14 } else { 2 };
        match self.observations(def.series, limit, None, false).await {
            Ok(obs) => indicator(def.key, def.name, def.unit, def.kind, &obs).map(|mut i| {
                i.source_url = Some(format!(
                    "https://fred.stlouisfed.org/series/{}",
                    crate::symbol::enc(def.series)
                ));
                i
            }),
            Err(e) => {
                tracing::debug!(series = def.series, error = %e, "FRED: serie no disponible");
                None
            }
        }
    }

    async fn observations(
        &self,
        series: &str,
        limit: usize,
        start: Option<&str>,
        asc: bool,
    ) -> PResult<Vec<Obs>> {
        let key = self.key.as_ref().ok_or(ProviderError::Unsupported)?;
        let mut url = Url::parse(BASE).map_err(|e| ProviderError::upstream(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("series_id", series)
                .append_pair("api_key", key.expose())
                .append_pair("file_type", "json")
                .append_pair("sort_order", if asc { "asc" } else { "desc" })
                .append_pair("limit", &limit.to_string());
            if let Some(s) = start {
                q.append_pair("observation_start", s);
            }
        }
        let body = send(self.http.get(url)).await?;
        if body.status == reqwest::StatusCode::BAD_REQUEST
            && let Ok(v) = serde_json::from_str::<Value>(&body.text)
        {
            let msg = v["error_message"].as_str().unwrap_or("");
            if msg.contains("does not exist") {
                return Err(ProviderError::NotFound(format!(
                    "serie {series} no existe en FRED"
                )));
            }
        }
        check_status(&body)?;
        parse_observations(&body.text)
    }
}

/// Observaciones válidas (FRED marca los huecos con `"."`), en el orden de la respuesta.
pub fn parse_observations(text: &str) -> PResult<Vec<Obs>> {
    let v: Value = parse_json(text)?;
    let arr = v["observations"]
        .as_array()
        .ok_or_else(|| ProviderError::upstream("FRED sin observaciones"))?;
    Ok(arr
        .iter()
        .filter_map(|o| {
            Some(Obs {
                date: o["date"].as_str()?.to_string(),
                value: num(&o["value"])?,
            })
        })
        .collect())
}

fn indicator(key: &str, name: &str, unit: &str, kind: Kind, obs: &[Obs]) -> Option<MacroIndicator> {
    let (value, change, date) = match kind {
        Kind::Level => {
            let cur = obs.first()?;
            (
                cur.value,
                obs.get(1).map(|p| cur.value - p.value),
                &cur.date,
            )
        }
        Kind::YoY => {
            let (cur, year_ago) = (obs.first()?, obs.get(12)?);
            if year_ago.value <= 0.0 {
                return None;
            }
            let yoy = (cur.value / year_ago.value - 1.0) * 100.0;
            let prev = match (obs.get(1), obs.get(13)) {
                (Some(a), Some(b)) if b.value > 0.0 => Some((a.value / b.value - 1.0) * 100.0),
                _ => None,
            };
            (yoy, prev.map(|p| yoy - p), &cur.date)
        }
    };
    Some(MacroIndicator {
        key: key.into(),
        name: name.into(),
        value,
        change,
        unit: unit.into(),
        as_of: from_ymd(date)?,
        source: "fred".into(),
        source_url: None,
        timestamp_origin: Some(domain::TimestampOrigin::Provider),
    })
}

fn is_series_id(id: &AssetId) -> bool {
    id.class == AssetClass::Bond
        && !id.symbol.is_empty()
        && id
            .symbol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[async_trait]
impl Provider for Fred {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn macro_indicators(&self) -> PResult<Vec<MacroIndicator>> {
        if self.key.is_none() {
            return Err(ProviderError::Unsupported);
        }
        let results: Vec<Option<MacroIndicator>> =
            futures::future::join_all(SERIES.iter().map(|def| self.fetch_indicator(def))).await;
        let out: Vec<MacroIndicator> = results.into_iter().flatten().collect();
        if out.is_empty() {
            return Err(ProviderError::upstream("FRED no devolvió ninguna serie"));
        }
        // Orden estable (el de SERIES), independiente de qué petición terminó antes.
        let mut out = out;
        out.sort_by_key(|i| {
            SERIES
                .iter()
                .position(|s| s.key == i.key)
                .unwrap_or(usize::MAX)
        });
        Ok(out)
    }

    async fn quote(&self, id: &AssetId) -> PResult<Quote> {
        if !is_series_id(id) {
            return Err(ProviderError::Unsupported);
        }
        let obs = self
            .observations(&id.symbol.to_ascii_uppercase(), 2, None, false)
            .await?;
        let cur = obs
            .first()
            .ok_or_else(|| ProviderError::NotFound("serie sin datos".into()))?;
        let prev = obs.get(1);
        Ok(Quote {
            id: id.clone(),
            price: cur.value,
            change: prev.map(|p| cur.value - p.value),
            // En un rendimiento el «cambio %» relativo engaña; se da la variación en puntos.
            change_pct: None,
            open: None,
            high: None,
            low: None,
            prev_close: prev.map(|p| p.value),
            volume: None,
            market_cap: None,
            currency: Some("%".into()),
            market_state: MarketState::Closed,
            timestamp: from_ymd(&cur.date)
                .ok_or_else(|| ProviderError::upstream("fecha no válida"))?,
            source: "fred".into(),
            delay_secs: None,
            source_url: None,
            timestamp_origin: Some(domain::TimestampOrigin::Provider),
        })
    }

    async fn history(
        &self,
        id: &AssetId,
        range: Range,
        _interval: Interval,
    ) -> PResult<Vec<Candle>> {
        if !is_series_id(id) {
            return Err(ProviderError::Unsupported);
        }
        let days = match range {
            Range::D1 | Range::D5 => 10,
            Range::M1 => 31,
            Range::M3 => 92,
            Range::M6 => 183,
            Range::Ytd | Range::Y1 => 366,
            Range::Y2 => 731,
            Range::Y5 => 1827,
            Range::Y10 => 3653,
            Range::Max => 36_500,
        };
        let start = (Utc::now() - Duration::days(days))
            .format("%Y-%m-%d")
            .to_string();
        let obs = self
            .observations(&id.symbol.to_ascii_uppercase(), 10_000, Some(&start), true)
            .await?;
        // Serie de un solo valor por día: o = h = l = c.
        Ok(obs
            .iter()
            .filter_map(|o| {
                Some(Candle {
                    t: from_ymd(&o.date)?.timestamp(),
                    o: o.value,
                    h: o.value,
                    l: o.value,
                    c: o.value,
                    v: None,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Respuesta con la forma documentada de `series/observations` (sort_order=desc), con un hueco «.».
    const DGS10: &str = r#"{"realtime_start":"2026-10-12","realtime_end":"2026-10-12","observation_start":"1600-01-01","observation_end":"9999-12-31","units":"lin","output_type":1,"file_type":"json","order_by":"observation_date","sort_order":"desc","count":3,"offset":0,"limit":3,"observations":[{"realtime_start":"2026-10-12","realtime_end":"2026-10-12","date":"2026-10-09","value":"."},{"realtime_start":"2026-10-12","realtime_end":"2026-10-12","date":"2026-10-08","value":"4.20"},{"realtime_start":"2026-10-12","realtime_end":"2026-10-12","date":"2026-10-07","value":"4.12"}]}"#;

    #[test]
    fn skips_missing_observations() {
        let o = parse_observations(DGS10).unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(
            o[0],
            Obs {
                date: "2026-10-08".into(),
                value: 4.20
            }
        );
        let ind = indicator("us10y", "x", "%", Kind::Level, &o).unwrap();
        assert_eq!(ind.value, 4.20);
        assert!((ind.change.unwrap() - 0.08).abs() < 1e-9);
        assert_eq!(ind.as_of.to_rfc3339(), "2026-10-08T00:00:00+00:00");
        assert_eq!(ind.source, "fred");
    }

    #[test]
    fn yoy_from_monthly_index() {
        // 14 observaciones, la más reciente primero: índice que sube 0.25 al mes desde 300.
        let obs: Vec<Obs> = (0..14)
            .map(|i| Obs {
                date: format!("2026-{:02}-01", 12 - i.min(11)),
                value: 300.0 + 0.25 * (13 - i) as f64,
            })
            .collect();
        let ind = indicator("cpi_yoy", "IPC", "%", Kind::YoY, &obs).unwrap();
        let expect = (obs[0].value / obs[12].value - 1.0) * 100.0;
        assert!((ind.value - expect).abs() < 1e-12);
        assert!(indicator("cpi_yoy", "IPC", "%", Kind::YoY, &obs[..5]).is_none());
    }

    #[test]
    fn bad_input_is_error() {
        assert!(parse_observations(r#"{"error_code":400}"#).is_err());
        assert!(parse_observations("<html>").is_err());
    }

    #[test]
    fn debug_never_prints_key() {
        let f = Fred::new(reqwest::Client::new(), Secret::new("supersecretkey123"));
        assert!(f.info.enabled());
        assert!(!format!("{:?}", f.key).contains("supersecret"));
        assert!(!Fred::new(reqwest::Client::new(), None).info.enabled());
    }
}
