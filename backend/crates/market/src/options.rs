//! Cadena de opciones (Yahoo). No forma parte del contrato `domain`: se expone como método
//! público adicional de `MarketService` (ver `docs/notes/market.md`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionContract {
    /// Símbolo OCC del contrato (`AAPL261218C00200000`).
    pub symbol: String,
    pub strike: f64,
    pub last_price: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    pub volume: Option<f64>,
    pub open_interest: Option<f64>,
    /// Volatilidad implícita como fracción (0.35 = 35 %).
    pub implied_volatility: Option<f64>,
    pub in_the_money: Option<bool>,
    pub last_trade: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionsChain {
    pub underlying: String,
    pub spot: Option<f64>,
    /// Vencimientos disponibles (Unix, segundos).
    pub expirations: Vec<i64>,
    /// Vencimiento de `calls` y `puts` (Unix, segundos).
    pub expiration: Option<i64>,
    pub calls: Vec<OptionContract>,
    pub puts: Vec<OptionContract>,
}
