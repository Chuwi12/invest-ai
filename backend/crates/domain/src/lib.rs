//! Tipos y contratos compartidos por todos los crates del backend.
//!
//! Es la fuente de verdad del contrato con el frontend: todo lo que viaja en JSON sale de aquí
//! (structs en camelCase, enums en snake_case). `frontend/src/api/types.ts` lo refleja 1:1;
//! si cambias algo aquí, cámbialo también allí y en `docs/API.md`.

use std::{collections::BTreeMap, fmt, str::FromStr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("no encontrado: {0}")]
    NotFound(String),
    #[error("entrada no válida: {0}")]
    InvalidInput(String),
    #[error("error del proveedor {provider}: {message}")]
    Upstream { provider: String, message: String },
    #[error("límite de peticiones del proveedor {provider}")]
    RateLimited { provider: String },
    #[error("servicio no disponible: {0}")]
    Unavailable(String),
    #[error("error interno: {0}")]
    Internal(String),
}

impl DomainError {
    /// Código estable de la API (`docs/API.md`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::RateLimited { .. } => "rate_limited",
            Self::Upstream { .. } => "upstream",
            Self::Unavailable(_) => "unavailable",
            Self::Internal(_) => "internal",
        }
    }

    /// Inversa aproximada de `code` (para errores recibidos como evento).
    pub fn from_code(code: &str, message: String) -> Self {
        match code {
            "not_found" => Self::NotFound(message),
            "invalid_input" => Self::InvalidInput(message),
            "rate_limited" => Self::RateLimited { provider: message },
            "upstream" => Self::Upstream { provider: "llm".into(), message },
            "unavailable" => Self::Unavailable(message),
            _ => Self::Internal(message),
        }
    }
}

pub type Result<T, E = DomainError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------------------------
// Activos
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Stock,
    Etf,
    Fund,
    Index,
    Crypto,
    Forex,
    Commodity,
    Bond,
    Future,
    Option,
    Other,
}

impl AssetClass {
    pub const ALL: [AssetClass; 11] = [
        Self::Stock,
        Self::Etf,
        Self::Fund,
        Self::Index,
        Self::Crypto,
        Self::Forex,
        Self::Commodity,
        Self::Bond,
        Self::Future,
        Self::Option,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stock => "stock",
            Self::Etf => "etf",
            Self::Fund => "fund",
            Self::Index => "index",
            Self::Crypto => "crypto",
            Self::Forex => "forex",
            Self::Commodity => "commodity",
            Self::Bond => "bond",
            Self::Future => "future",
            Self::Option => "option",
            Self::Other => "other",
        }
    }
}

impl FromStr for AssetClass {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| DomainError::InvalidInput(format!("clase de activo desconocida: {s}")))
    }
}

/// Identificador canónico de un activo: `"<clase>:<símbolo>"`.
///
/// Convención de símbolos (la usan todos los proveedores para enrutar):
/// - stock / etf / fund / index / future / commodity / forex / option: símbolo de Yahoo Finance
///   (`AAPL`, `SAN.MC`, `SPY`, `^GSPC`, `ES=F`, `GC=F`, `EURUSD=X`, `AAPL261218C00200000`).
/// - crypto: ticker base en mayúsculas (`BTC`, `ETH`, `SOL`); el proveedor resuelve el par
///   (Binance `BTCUSDT`, CoinGecko `bitcoin`).
/// - bond: serie de FRED (`DGS10`, `DGS2`) o símbolo de Yahoo (`^TNX`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId {
    pub class: AssetClass,
    pub symbol: String,
}

impl AssetId {
    pub fn new(class: AssetClass, symbol: impl Into<String>) -> Self {
        Self { class, symbol: symbol.into() }
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.class.as_str(), self.symbol)
    }
}

/// Lista blanca del símbolo: `^[A-Za-z0-9.^=\-_]{1,32}$` (`AAPL`, `^GSPC`, `GC=F`, `EURUSD=X`,
/// `BRK-B`, `DX-Y.NYB`). Excluye controles, `/`, `%`, espacios y Unicode (seguridad S-09).
pub fn valid_symbol(symbol: &str) -> bool {
    (1..=32).contains(&symbol.len())
        && symbol.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'^' | b'=' | b'-' | b'_'))
}

impl FromStr for AssetId {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self> {
        let (class, symbol) = s
            .split_once(':')
            .ok_or_else(|| DomainError::InvalidInput(format!("id de activo sin clase: {s}")))?;
        let symbol = symbol.trim();
        if !valid_symbol(symbol) {
            // No se refleja el símbolo: puede llevar saltos de línea u otros controles (log forging).
            return Err(DomainError::InvalidInput("símbolo de activo no válido".to_string()));
        }
        Ok(Self { class: class.parse()?, symbol: symbol.to_string() })
    }
}

impl Serialize for AssetId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSummary {
    pub id: AssetId,
    pub symbol: String,
    pub name: String,
    pub class: AssetClass,
    pub exchange: Option<String>,
    pub currency: Option<String>,
    pub country: Option<String>,
    /// Proveedor que devolvió el resultado (yahoo, coingecko, ...).
    pub source: String,
    /// Página pública del proveedor donde comprobar el dato.
    #[serde(default)]
    pub source_url: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Precios
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketState {
    Pre,
    Regular,
    Post,
    Closed,
    /// Mercados 24/7 (cripto).
    Continuous,
    Unknown,
}

/// De dónde sale una marca de tiempo: la dio el proveedor o es la hora local de la consulta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampOrigin {
    Provider,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub id: AssetId,
    pub price: f64,
    pub change: Option<f64>,
    /// Variación respecto al cierre anterior en PUNTOS porcentuales (1.2 = +1,2 %), no en fracción.
    pub change_pct: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    pub volume: Option<f64>,
    pub market_cap: Option<f64>,
    pub currency: Option<String>,
    pub market_state: MarketState,
    pub timestamp: DateTime<Utc>,
    pub source: String,
    /// Retraso del dato respecto al mercado en segundos (0 = tiempo real). None = desconocido.
    pub delay_secs: Option<u32>,
    /// Página pública del proveedor donde comprobar la cifra.
    #[serde(default)]
    pub source_url: Option<String>,
    /// Si `timestamp` lo dio el proveedor o es la hora local. None = desconocido.
    #[serde(default)]
    pub timestamp_origin: Option<TimestampOrigin>,
}

/// Actualización de precio en tiempo real (WebSocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTick {
    pub id: AssetId,
    pub price: f64,
    /// Variación respecto al cierre anterior en PUNTOS porcentuales (1.2 = +1,2 %), no en fracción.
    pub change_pct: Option<f64>,
    pub volume: Option<f64>,
    pub ts: DateTime<Utc>,
    pub source: String,
}

/// Vela OHLCV con claves cortas: es el payload más voluminoso de la API.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    /// Unix timestamp en segundos (inicio de la vela, UTC).
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Range {
    #[serde(rename = "1d")]
    D1,
    #[serde(rename = "5d")]
    D5,
    #[serde(rename = "1mo")]
    M1,
    #[serde(rename = "3mo")]
    M3,
    #[serde(rename = "6mo")]
    M6,
    #[serde(rename = "ytd")]
    Ytd,
    #[serde(rename = "1y")]
    Y1,
    #[serde(rename = "2y")]
    Y2,
    #[serde(rename = "5y")]
    Y5,
    #[serde(rename = "10y")]
    Y10,
    #[serde(rename = "max")]
    Max,
}

impl Range {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::D1 => "1d",
            Self::D5 => "5d",
            Self::M1 => "1mo",
            Self::M3 => "3mo",
            Self::M6 => "6mo",
            Self::Ytd => "ytd",
            Self::Y1 => "1y",
            Self::Y2 => "2y",
            Self::Y5 => "5y",
            Self::Y10 => "10y",
            Self::Max => "max",
        }
    }

    /// Intervalo razonable por defecto para un rango (≈ 100–2000 velas).
    pub fn default_interval(self) -> Interval {
        match self {
            Self::D1 => Interval::M5,
            Self::D5 => Interval::M15,
            Self::M1 => Interval::H1,
            Self::M3 | Self::M6 | Self::Ytd | Self::Y1 | Self::Y2 => Interval::D1,
            Self::Y5 | Self::Y10 => Interval::W1,
            Self::Max => Interval::Mo1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interval {
    #[serde(rename = "1m")]
    M1,
    #[serde(rename = "5m")]
    M5,
    #[serde(rename = "15m")]
    M15,
    #[serde(rename = "30m")]
    M30,
    #[serde(rename = "1h")]
    H1,
    #[serde(rename = "1d")]
    D1,
    #[serde(rename = "1wk")]
    W1,
    #[serde(rename = "1mo")]
    Mo1,
}

impl Interval {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1 => "1h",
            Self::D1 => "1d",
            Self::W1 => "1wk",
            Self::Mo1 => "1mo",
        }
    }

    pub fn seconds(self) -> i64 {
        match self {
            Self::M1 => 60,
            Self::M5 => 300,
            Self::M15 => 900,
            Self::M30 => 1800,
            Self::H1 => 3600,
            Self::D1 => 86_400,
            Self::W1 => 604_800,
            Self::Mo1 => 2_592_000,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fundamentales e indicadores
// ---------------------------------------------------------------------------------------------

/// Datos fundamentales. Todo es opcional: cada clase de activo rellena lo que tenga sentido.
/// Ratios y márgenes como fracción (0.25 = 25 %), no como porcentaje.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fundamentals {
    pub description: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub website: Option<String>,
    pub employees: Option<u64>,
    pub country: Option<String>,
    /// Moneda ISO de las cifras en precio (capitalización, objetivos de analistas, máximo
    /// histórico). Las cuentas anuales pueden venir en otra (`extra.financialCurrency`).
    #[serde(default)]
    pub currency: Option<String>,

    // Valoración
    pub market_cap: Option<f64>,
    pub enterprise_value: Option<f64>,
    pub pe_trailing: Option<f64>,
    pub pe_forward: Option<f64>,
    pub peg: Option<f64>,
    pub price_to_book: Option<f64>,
    pub price_to_sales: Option<f64>,
    pub ev_to_ebitda: Option<f64>,

    // Resultados
    pub eps_trailing: Option<f64>,
    pub eps_forward: Option<f64>,
    pub revenue_ttm: Option<f64>,
    pub revenue_growth_yoy: Option<f64>,
    pub earnings_growth_yoy: Option<f64>,
    pub gross_margin: Option<f64>,
    pub operating_margin: Option<f64>,
    pub profit_margin: Option<f64>,
    pub roe: Option<f64>,
    pub roa: Option<f64>,
    pub free_cash_flow: Option<f64>,
    pub total_cash: Option<f64>,
    pub total_debt: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub current_ratio: Option<f64>,

    // Accionista
    pub dividend_yield: Option<f64>,
    pub payout_ratio: Option<f64>,
    pub beta: Option<f64>,
    pub shares_outstanding: Option<f64>,
    pub short_pct_float: Option<f64>,
    pub insider_pct: Option<f64>,
    pub institutional_pct: Option<f64>,

    // Consenso de analistas
    pub analyst_target_mean: Option<f64>,
    pub analyst_target_high: Option<f64>,
    pub analyst_target_low: Option<f64>,
    /// 1 = compra fuerte … 5 = venta (escala de Yahoo).
    pub analyst_rating_mean: Option<f64>,
    pub analyst_count: Option<u32>,
    pub next_earnings_date: Option<NaiveDate>,

    // Cripto
    pub circulating_supply: Option<f64>,
    pub total_supply: Option<f64>,
    pub max_supply: Option<f64>,
    pub fully_diluted_valuation: Option<f64>,
    pub all_time_high: Option<f64>,
    pub all_time_high_date: Option<NaiveDate>,

    // ETF / fondos
    pub expense_ratio: Option<f64>,
    pub total_assets: Option<f64>,
    pub top_holdings: Vec<Holding>,

    // Bonos
    pub yield_to_maturity: Option<f64>,
    pub coupon: Option<f64>,
    pub maturity_date: Option<NaiveDate>,

    /// Campos adicionales que no encajan arriba (se muestran tal cual en el frontend).
    pub extra: BTreeMap<String, serde_json::Value>,
    pub sources: Vec<String>,
    pub as_of: Option<DateTime<Utc>>,
    /// Página pública del primer proveedor de `sources` donde comprobar las cifras.
    #[serde(default)]
    pub source_url: Option<String>,
    /// Si `as_of` lo dio el proveedor o es la hora local de la consulta.
    #[serde(default)]
    pub timestamp_origin: Option<TimestampOrigin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holding {
    pub symbol: Option<String>,
    pub name: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    StrongUp,
    Up,
    Sideways,
    Down,
    StrongDown,
}

/// Indicadores técnicos calculados sobre velas diarias. Rendimientos y distancias como fracción.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Indicators {
    pub as_of: Option<DateTime<Utc>>,
    /// Proveedor de las velas con las que se calcularon (el cálculo es propio).
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    pub last_close: Option<f64>,
    pub sma20: Option<f64>,
    pub sma50: Option<f64>,
    pub sma200: Option<f64>,
    pub ema12: Option<f64>,
    pub ema26: Option<f64>,
    pub rsi14: Option<f64>,
    pub macd: Option<f64>,
    pub macd_signal: Option<f64>,
    pub macd_hist: Option<f64>,
    pub bb_upper: Option<f64>,
    pub bb_middle: Option<f64>,
    pub bb_lower: Option<f64>,
    pub atr14: Option<f64>,
    pub volatility30d: Option<f64>,
    pub max_drawdown1y: Option<f64>,
    pub return1w: Option<f64>,
    pub return1m: Option<f64>,
    pub return3m: Option<f64>,
    pub return_ytd: Option<f64>,
    pub return1y: Option<f64>,
    pub high52w: Option<f64>,
    pub low52w: Option<f64>,
    pub dist_from_high52w: Option<f64>,
    pub volume_avg20: Option<f64>,
    pub relative_volume: Option<f64>,
    pub trend: Option<Trend>,
    pub supports: Vec<f64>,
    pub resistances: Vec<f64>,
}

// ---------------------------------------------------------------------------------------------
// Noticias y macro
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sentiment {
    /// -1 (muy negativo) … 1 (muy positivo).
    pub score: f32,
    pub label: SentimentLabel,
    /// Quién lo calculó: "llm", "lexicon", "provider:<nombre>".
    pub method: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentimentLabel {
    VeryNegative,
    Negative,
    Neutral,
    Positive,
    VeryPositive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    /// Hash estable de la URL canónica (deduplicación entre proveedores).
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub url: String,
    pub publisher: Option<String>,
    pub published_at: DateTime<Utc>,
    pub image_url: Option<String>,
    pub tickers: Vec<String>,
    pub sentiment: Option<Sentiment>,
    /// Proveedor de la API (gdelt, finnhub, yahoo_rss, google_news, ...).
    pub provider: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroIndicator {
    /// Clave estable: fed_funds, us10y, us2y, curve_10y2y, cpi_yoy, unemployment, vix, dxy,
    /// eurusd, gold, wti, ecb_rate, ...
    pub key: String,
    pub name: String,
    pub value: f64,
    pub change: Option<f64>,
    pub unit: String,
    pub as_of: DateTime<Utc>,
    pub source: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub timestamp_origin: Option<TimestampOrigin>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroContext {
    pub indicators: Vec<MacroIndicator>,
    pub as_of: Option<DateTime<Utc>>,
}

/// Todo lo que se sabe de un activo en un instante: es el contexto que recibe el LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSnapshot {
    pub summary: AssetSummary,
    pub quote: Option<Quote>,
    pub fundamentals: Option<Fundamentals>,
    pub indicators: Option<Indicators>,
    pub news: Vec<NewsItem>,
    pub macro_context: Option<MacroContext>,
    /// Datos que no se pudieron obtener (p. ej. "fundamentals: yahoo 429"). El LLM debe decirlo.
    pub missing: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------------------------
// Análisis de IA
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Buen momento para comprar.
    Buy,
    /// Comprar de forma escalonada (DCA).
    Accumulate,
    /// Mantener si ya se tiene; no añadir.
    Hold,
    /// Esperar un mejor punto de entrada o un catalizador.
    Wait,
    /// Reducir posición.
    Reduce,
    /// Evitar / vender.
    Avoid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Horizon {
    /// Días a semanas.
    Short,
    /// Meses.
    Medium,
    /// Años.
    Long,
}

/// Puntuaciones 0–100 (más alto = más favorable para invertir; en `risk`, más alto = más riesgo).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    pub overall: f32,
    pub technical: Option<f32>,
    pub momentum: Option<f32>,
    pub fundamental: Option<f32>,
    pub valuation: Option<f32>,
    pub sentiment: Option<f32>,
    pub macro_score: Option<f32>,
    pub risk: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalyst {
    pub date: Option<NaiveDate>,
    pub description: String,
    /// Impacto esperado: -1 … 1.
    pub impact: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyLevels {
    pub supports: Vec<f64>,
    pub resistances: Vec<f64>,
    pub entry_zone: Option<(f64, f64)>,
    pub stop_loss: Option<f64>,
    pub targets: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub title: String,
    pub url: Option<String>,
    pub provider: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub id: String,
    pub asset: AssetId,
    pub verdict: Verdict,
    /// 0–1.
    pub confidence: f32,
    pub horizon: Horizon,
    pub scores: ScoreBreakdown,
    /// Resumen de 2–4 frases.
    pub summary: String,
    pub thesis: String,
    pub bull_case: Vec<String>,
    pub bear_case: Vec<String>,
    pub risks: Vec<String>,
    pub catalysts: Vec<Catalyst>,
    pub key_levels: KeyLevels,
    /// Cómo entrar si se decide invertir (tramos, tamaño de posición, condiciones).
    pub strategy: Option<String>,
    /// Avisos sobre datos ausentes o poco fiables.
    pub data_warnings: Vec<String>,
    pub sources: Vec<SourceRef>,
    pub price_at_analysis: Option<f64>,
    pub model: String,
    pub generated_at: DateTime<Utc>,
    /// Explicación de por qué faltan (o cómo se calcularon) entrada, stop y objetivos. `None` en
    /// análisis antiguos.
    #[serde(default)]
    pub levels_note: Option<String>,
    /// Escenarios alcista/base/bajista con probabilidades que suman 100 (solo `deep`).
    #[serde(default)]
    pub scenarios: Option<Vec<Scenario>>,
    /// Rentabilidad esperada ponderada por probabilidad, en % respecto al precio del análisis.
    #[serde(default)]
    pub expected_return_pct: Option<f64>,
    /// Qué tendría que pasar para cambiar de veredicto (comprobable con datos).
    #[serde(default)]
    pub would_change_if: Option<Vec<String>>,
    /// Por qué ese horizonte.
    #[serde(default)]
    pub horizon_note: Option<String>,
    /// Tamaño de posición orientativo calculado en código según ATR/volatilidad.
    #[serde(default)]
    pub position_sizing: Option<PositionSizing>,
    /// Nota de método: modelo, profundidad, llamadas, tokens, tiempo y aviso de no asesoramiento.
    #[serde(default)]
    pub method: Option<String>,
    /// Avisos de calidad del propio análisis (afirmaciones descartadas, cifras sin verificar…),
    /// separados de `data_warnings`, que solo recoge carencias reales de los datos.
    #[serde(default)]
    pub quality_notes: Option<Vec<String>>,
    /// Profundidad con la que se generó.
    #[serde(default)]
    pub depth: Option<AnalysisDepth>,
    /// Duración total de la generación en milisegundos.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Llamadas al LLM (incluidos reintentos y correcciones).
    #[serde(default)]
    pub llm_calls: Option<u32>,
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
}

/// Un escenario del análisis profundo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    /// `bullish` | `base` | `bearish`.
    pub kind: String,
    /// 0–100; las de todos los escenarios suman 100.
    pub probability: f32,
    pub description: String,
    /// Precio objetivo tomado de un nivel de los datos (resistencia, soporte, objetivo de analistas…).
    pub target_price: Option<f64>,
    /// Qué nivel de los datos se usó como precio objetivo.
    pub target_basis: Option<String>,
    /// % de variación desde el precio del análisis hasta `target_price`.
    pub return_pct: Option<f64>,
}

/// Tamaño de posición orientativo (no es asesoramiento).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionSizing {
    /// % máximo orientativo de la cartera en este activo.
    pub max_portfolio_pct: f64,
    /// Riesgo asumido por operación como % de la cartera (si hay stop).
    pub risk_per_trade_pct: Option<f64>,
    /// Distancia de entrada a stop en %, o 2×ATR si no hay stop.
    pub stop_distance_pct: Option<f64>,
    pub atr_pct: Option<f64>,
    pub volatility30d: Option<f64>,
    /// Cómo se calculó, en lenguaje llano.
    pub rationale: String,
}

/// Progreso del análisis en streaming (`POST /assets/{id}/analysis/stream`, SSE).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum AnalysisEvent {
    /// Empieza una etapa. `stage` es un id estable: snapshot, technical, fundamental, sentiment,
    /// macro_risk, debate, judge, verify. `index` empieza en 1.
    Stage { stage: String, label: String, index: u32, total: u32 },
    Done { analysis: Analysis },
    Error { code: String, message: String },
}

// ---------------------------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Resumen corto del resultado para mostrar en la UI.
    pub result_summary: Option<String>,
    pub ok: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCallRecord>,
    /// Si el asistente emitió un veredicto estructurado en este mensaje.
    pub analysis: Option<Analysis>,
    /// `true` si la respuesta se cortó por un error a mitad de generación (el texto es parcial).
    #[serde(default)]
    pub truncated: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub asset_ids: Vec<AssetId>,
    pub pinned: bool,
    pub message_count: u32,
    pub last_message_preview: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Evento del stream de chat (SSE, campo `data` en JSON, discriminado por `type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ChatEvent {
    Start { message_id: String },
    /// Razonamiento del modelo (si el modelo lo expone). La UI lo muestra plegado.
    Thinking { text: String },
    Token { text: String },
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    ToolResult { id: String, name: String, ok: bool, summary: String },
    /// En `Box`: un `Analysis` es mucho mayor que el resto de variantes (clippy::large_enum_variant).
    /// El JSON no cambia.
    Analysis { analysis: Box<Analysis> },
    Done { message_id: String, model: String, prompt_tokens: Option<u32>, completion_tokens: Option<u32> },
    Error { message: String },
}

pub struct ChatRequest {
    pub conversation_id: String,
    /// Mensajes anteriores (sin el nuevo), en orden cronológico.
    pub history: Vec<ChatMessage>,
    pub user_message: String,
    /// Activos sobre los que gira la conversación (se precargan en el contexto).
    pub focus: Vec<AssetId>,
    /// Acceso a datos en vivo para el tool calling del modelo.
    pub market: Arc<dyn MarketData>,
    /// Idioma de respuesta (BCP-47), p. ej. "es".
    pub language: String,
}

// ---------------------------------------------------------------------------------------------
// Estado de servicios
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub name: String,
    pub enabled: bool,
    pub requires_key: bool,
    pub has_key: bool,
    /// Qué cubre: search, quote, realtime, history, fundamentals, news, macro, options.
    pub capabilities: Vec<String>,
    pub asset_classes: Vec<AssetClass>,
    pub last_error: Option<String>,
    pub last_ok: Option<DateTime<Utc>>,
    /// Latencia de la última petición, en milisegundos.
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Segundos que quedan de pausa tras un 429.
    #[serde(default)]
    pub blocked_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    pub available: bool,
    pub base_url: String,
    pub model: String,
    pub installed_models: Vec<String>,
    pub supports_tools: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Contratos entre crates
// ---------------------------------------------------------------------------------------------

/// Acceso unificado a datos de mercado. Lo implementa `market` (agregando proveedores con
/// fallback y caché) y lo consumen `server` (rutas HTTP/WS) y `ai` (tool calling).
#[async_trait]
pub trait MarketData: Send + Sync + 'static {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<AssetSummary>>;
    async fn quote(&self, id: &AssetId) -> Result<Quote>;
    /// Cotizaciones en lote (una sola petición al proveedor cuando sea posible).
    /// Los activos que fallen se omiten.
    async fn quotes(&self, ids: &[AssetId]) -> Result<Vec<Quote>>;
    async fn history(&self, id: &AssetId, range: Range, interval: Interval) -> Result<Vec<Candle>>;
    async fn fundamentals(&self, id: &AssetId) -> Result<Option<Fundamentals>>;
    async fn indicators(&self, id: &AssetId) -> Result<Indicators>;
    /// `None` = noticias generales de mercado.
    async fn news(&self, id: Option<&AssetId>, limit: usize) -> Result<Vec<NewsItem>>;
    async fn macro_context(&self) -> Result<MacroContext>;
    /// Agrega quote + fundamentales + indicadores + noticias + macro en paralelo. Tolera fallos
    /// parciales y los anota en `missing`; solo falla si el activo no existe.
    async fn snapshot(&self, id: &AssetId) -> Result<AssetSnapshot>;
    /// Stream de precios en tiempo real. Se cancela al soltarlo.
    fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick>;
    fn providers_status(&self) -> Vec<ProviderStatus>;
}

/// Analista de IA sobre un LLM local. Lo implementa `ai`.
#[async_trait]
pub trait Analyst: Send + Sync + 'static {
    /// Veredicto estructurado basado solo en los datos del snapshot.
    async fn analyze(&self, snapshot: &AssetSnapshot) -> Result<Analysis>;
    /// Respuesta de chat en streaming, con tool calling sobre `req.market`.
    fn chat(&self, req: ChatRequest) -> BoxStream<'static, ChatEvent>;
    async fn status(&self) -> LlmStatus;
    /// Como `analyze`, con profundidad explícita (`?depth=` de la API). `None` = la que
    /// corresponda al modelo activo. Por defecto ignora la profundidad.
    async fn analyze_with(&self, snapshot: &AssetSnapshot, depth: Option<AnalysisDepth>) -> Result<Analysis> {
        let _ = depth;
        self.analyze(snapshot).await
    }
    /// Análisis con progreso: etapas y, al final, `Done` o `Error`. Por defecto no informa de
    /// etapas intermedias. Recibe `Arc<Self>` para que el stream sea `'static`.
    fn analyze_stream(
        self: std::sync::Arc<Self>,
        snapshot: AssetSnapshot,
        depth: Option<AnalysisDepth>,
    ) -> BoxStream<'static, AnalysisEvent> {
        use futures::StreamExt as _;
        futures::stream::once(async move {
            match self.analyze_with(&snapshot, depth).await {
                Ok(analysis) => AnalysisEvent::Done { analysis },
                Err(e) => AnalysisEvent::Error { code: e.code().to_string(), message: e.to_string() },
            }
        })
        .boxed()
    }
}

// ---------------------------------------------------------------------------------------------
// Modelos de IA elegibles por el usuario (Ajustes). Añadido por el área `ai`.
// ---------------------------------------------------------------------------------------------

/// Profundidad del análisis: más etapas del pipeline = más calidad y más tiempo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisDepth {
    Quick,
    Standard,
    Deep,
}

impl FromStr for AnalysisDepth {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "quick" => Ok(Self::Quick),
            "standard" => Ok(Self::Standard),
            "deep" => Ok(Self::Deep),
            _ => Err(DomainError::InvalidInput(format!("profundidad desconocida: {s} (quick|standard|deep)"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Light,
    Balanced,
    Heavy,
    Max,
}

/// Requisitos estimados (GiB = 2^30 bytes) para el `num_ctx` configurado. Ver `docs/notes/ai.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequirements {
    pub min_vram_gb: f32,
    pub recommended_vram_gb: f32,
    pub min_ram_gb: f32,
    pub disk_gb: f32,
    pub cpu_only_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModel {
    /// Tag de Ollama, p. ej. "qwen3:8b".
    pub id: String,
    pub name: String,
    pub family: String,
    /// "8.2B".
    pub parameters: String,
    /// Parámetros activos por token (MoE).
    pub active_parameters: Option<String>,
    pub quantization: String,
    pub context_window: u32,
    pub tier: Option<ModelTier>,
    pub supports_tools: bool,
    pub reasoning: bool,
    pub description: String,
    pub requirements: Option<ModelRequirements>,
    pub curated: bool,
    pub installed: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    pub vram_total_gb: f32,
    pub vram_free_gb: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    pub cpu_model: String,
    pub cpu_threads: u32,
    pub ram_total_gb: f32,
    pub ram_available_gb: f32,
    pub gpus: Vec<GpuInfo>,
    pub disk_free_gb: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsResponse {
    pub active: String,
    pub ollama_reachable: bool,
    pub models: Vec<LlmModel>,
    pub hardware: HardwareInfo,
}

/// Evento del SSE de `POST /models/{id}/pull`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PullEvent {
    Progress { status: String, completed: Option<u64>, total: Option<u64> },
    Done,
    Error { message: String },
}

/// Gestión de modelos del LLM. La implementa `ai`; las rutas `/models` la consumen.
#[async_trait]
pub trait ModelService: Send + Sync + 'static {
    async fn list(&self) -> ModelsResponse;
    /// Cambia el modelo activo (persistente). `InvalidInput` si no existe o no está instalado.
    async fn set_active(&self, id: &str) -> Result<()>;
    /// Descarga un modelo del catálogo curado. `InvalidInput` si no es curado; `Unavailable`
    /// si ya se está descargando. Soltar el stream cancela la descarga.
    async fn pull(&self, id: &str) -> Result<BoxStream<'static, PullEvent>>;
    /// Borra un modelo instalado. `InvalidInput` si es el activo.
    async fn delete(&self, id: &str) -> Result<()>;
}

/// Almacén clave-valor de ajustes persistentes (tabla `settings` de SQLite en `server`).
#[async_trait]
pub trait SettingsStore: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------------------------
// OPV (IPO): calendario, folleto y rendimiento posterior
// ---------------------------------------------------------------------------------------------

/// Estado de una OPV en el calendario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpoStatus {
    /// Anunciada con rango de precio y fecha prevista.
    Expected,
    /// Fijado el precio definitivo (la fecha es la del primer día de cotización).
    Priced,
    /// Registro presentado, sin fecha firme (la fecha es la de presentación).
    Filed,
    Withdrawn,
    Unknown,
}

/// Una OPV del calendario. Todo lo que el proveedor no da va a `null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoEntry {
    pub date: NaiveDate,
    pub symbol: Option<String>,
    pub name: String,
    pub exchange: Option<String>,
    pub status: IpoStatus,
    /// Límite bajo del rango de precio, o el precio definitivo si `priceHigh` es igual.
    pub price_low: Option<f64>,
    pub price_high: Option<f64>,
    pub shares: Option<u64>,
    /// Importe en USD. En `expected` es el máximo posible (límite alto x acciones x 1,15 de
    /// sobreasignación); en `priced`, acciones x precio.
    pub total_value: Option<f64>,
    /// Indicio (no certeza) de sociedad vehículo (SPAC): nombre con «Acquisition» y precio 10.
    pub spac_likely: bool,
    pub source: String,
    pub source_url: Option<String>,
    pub as_of: DateTime<Utc>,
}

/// Sección de un folleto, en texto limpio.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProspectusSection {
    /// summary, offering, summary_financial_data, risk_factors, use_of_proceeds, dividend_policy,
    /// capitalization, dilution, principal_stockholders, shares_eligible, lock_up, underwriting.
    pub key: String,
    /// Título tal como aparece en el documento.
    pub title: String,
    pub text: String,
    /// Clave de la sección que ya contiene este texto (subsecciones: no indexar dos veces).
    pub parent: Option<String>,
    /// `text` se cortó por el tope de caracteres.
    pub truncated: bool,
    /// Caracteres del texto completo antes de cortar.
    pub original_chars: usize,
}

/// Folleto de una OPV obtenido de SEC EDGAR.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prospectus {
    pub issuer: String,
    pub cik: u64,
    pub ticker: Option<String>,
    /// 424B4, 424B1, 253G2 (definitivos) o S-1/A, F-1/A... (preliminares).
    pub form: String,
    pub preliminary: bool,
    pub filed_on: NaiveDate,
    pub accession: String,
    /// Documento original en sec.gov (verificable).
    pub url: String,
    pub index_url: String,
    /// Precio de la OPV leído de la portada, si se encontró.
    pub offer_price: Option<f64>,
    pub sections: Vec<ProspectusSection>,
    /// Claves de secciones que no se localizaron.
    pub missing: Vec<String>,
    pub document_chars: usize,
    pub source: String,
    pub fetched_at: DateTime<Utc>,
}

/// Rentabilidad a un horizonte tras la OPV.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoReturnPoint {
    /// 1m, 3m, 6m, 12m.
    pub horizon: String,
    pub target_date: NaiveDate,
    /// Sesión usada (última con cierre a o antes de `targetDate`); `null` si aún no se alcanza.
    pub price_date: Option<NaiveDate>,
    pub price: Option<f64>,
    /// Desde el precio de la OPV, en puntos porcentuales.
    pub return_pct: Option<f64>,
    /// Desde el cierre del primer día.
    pub since_first_close_pct: Option<f64>,
    /// Del índice de referencia en el mismo periodo (cierre previo a la OPV hasta `priceDate`).
    pub benchmark_return_pct: Option<f64>,
    pub excess_pct: Option<f64>,
}

/// Historial de resultados de una OPV pasada.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoPerformance {
    pub symbol: String,
    pub ipo_date: NaiveDate,
    pub offer_price: f64,
    /// calendar, prospectus o manual.
    pub offer_price_source: String,
    pub first_day_close: f64,
    pub first_day_return_pct: f64,
    pub first_day_high_pct: f64,
    pub points: Vec<IpoReturnPoint>,
    pub latest_price: f64,
    pub latest_date: NaiveDate,
    pub latest_return_pct: f64,
    /// Peor caída desde un máximo de cierre, desde el primer día (valor <= 0).
    pub max_drawdown_pct: f64,
    pub benchmark: String,
    pub source: String,
    pub source_url: Option<String>,
    pub warnings: Vec<String>,
    pub as_of: DateTime<Utc>,
}

// ---------------------------------------------------------------------------------------------
// OPV: perfil de riesgo determinista (sin LLM) y tasa base
// ---------------------------------------------------------------------------------------------

/// Si el indicador se pudo leer del folleto o de los datos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpoIndicatorStatus {
    /// Valor leído con una regla verificable; `evidence` cita el texto.
    Extracted,
    /// No se encontró o no se pudo leer con fiabilidad. Nunca se rellena ni se estima.
    NotExtracted,
}

/// De dónde sale un indicador.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoIndicatorSource {
    /// `prospectus` (sección de un folleto en EDGAR) o `calendar` (calendario de Finnhub/EDGAR).
    pub kind: String,
    /// Clave de la sección del folleto (`use_of_proceeds`, `offering`...), si procede.
    pub section: Option<String>,
    /// Texto legible de la fuente («Folleto 424B4, sección «Use of Proceeds»»).
    pub label: String,
    /// Documento verificable (folleto en sec.gov o página del calendario).
    pub url: Option<String>,
}

/// Una bandera dentro de un indicador (p. ej. «devolver deuda»). Es un hecho leído del texto,
/// no un veredicto: `attention` solo marca lo que conviene mirar con más cuidado.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoRiskFlag {
    pub code: String,
    pub label: String,
    pub attention: bool,
    /// Frase literal del folleto que la sustenta.
    pub evidence: String,
}

/// Un indicador del perfil de riesgo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoRiskIndicator {
    pub key: String,
    pub label: String,
    pub status: IpoIndicatorStatus,
    /// Valor en texto, listo para mostrar (`null` si no se extrajo).
    pub value: Option<String>,
    /// Valor numérico principal, si lo hay.
    pub numeric: Option<f64>,
    pub unit: Option<String>,
    pub flags: Vec<IpoRiskFlag>,
    /// Cita literal del folleto de la que sale el valor.
    pub evidence: Option<String>,
    /// Limitación o motivo de «no extraído».
    pub note: Option<String>,
    pub source: IpoIndicatorSource,
}

/// Tasa base de las OPV recientes: lo que ha pasado de media, no lo que pasará.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoBaseRate {
    pub months: u32,
    pub benchmark: String,
    /// OPV (con precio definitivo, sin SPAC) incluidas en la muestra.
    pub sample: usize,
    /// Con dato del primer día / con dato a 3 meses (las más recientes aún no lo tienen).
    pub n_day1: usize,
    pub n_3m: usize,
    pub median_day1_pct: Option<f64>,
    /// % de OPV con cierre del primer día por encima del precio de salida.
    pub positive_day1_pct: Option<f64>,
    pub median_3m_pct: Option<f64>,
    /// Mediana del exceso frente al índice a 3 meses.
    pub median_3m_excess_pct: Option<f64>,
    /// % de OPV con rentabilidad a 3 meses por encima del índice.
    pub beat_benchmark_3m_pct: Option<f64>,
    pub symbols: Vec<String>,
    pub source: String,
    pub as_of: DateTime<Utc>,
    /// Sesgos conocidos de la muestra.
    pub caveats: Vec<String>,
}

/// Referencia del folleto del que sale el perfil.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoProspectusRef {
    pub form: String,
    pub preliminary: bool,
    pub filed_on: NaiveDate,
    pub url: String,
    pub index_url: String,
    pub document_chars: usize,
    pub missing: Vec<String>,
}

/// Perfil de riesgo de una OPV: indicadores verificables y tasa base. No es una recomendación
/// ni una predicción.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoRiskProfile {
    pub symbol: String,
    pub name: Option<String>,
    /// Entrada del calendario usada (si se encontró).
    pub entry: Option<IpoEntry>,
    pub prospectus: Option<IpoProspectusRef>,
    pub indicators: Vec<IpoRiskIndicator>,
    pub base_rate: Option<IpoBaseRate>,
    /// Avisos: folleto preliminar, SPAC probable, folleto o tasa base no disponibles...
    pub warnings: Vec<String>,
    pub disclaimer: String,
    pub generated_at: DateTime<Utc>,
}

/// Acceso a los datos de OPV de EE. UU. Lo implementa `market` (`MarketService`); el modo
/// simulado del servidor implementa una versión con datos inventados.
#[async_trait]
pub trait IpoData: Send + Sync + 'static {
    async fn calendar(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<IpoEntry>>;
    /// Folleto de la OPV con el texto completo de las secciones.
    async fn prospectus(&self, issuer: &str) -> Result<Prospectus>;
    async fn performance(
        &self,
        symbol: &str,
        ipo_date: NaiveDate,
        offer_price: Option<f64>,
        benchmark: &str,
    ) -> Result<IpoPerformance>;
    /// Tasa base de las OPV de los últimos `months` meses.
    async fn base_rate(&self, months: u32) -> Result<IpoBaseRate>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_id_roundtrip() {
        let id: AssetId = "crypto:BTC".parse().unwrap();
        assert_eq!(id, AssetId::new(AssetClass::Crypto, "BTC"));
        assert_eq!(id.to_string(), "crypto:BTC");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"crypto:BTC\"");
        let yahoo: AssetId = "stock:BRK-B".parse().unwrap();
        assert_eq!(yahoo.symbol, "BRK-B");
        assert!("BTC".parse::<AssetId>().is_err());
        assert!("planet:X".parse::<AssetId>().is_err());
    }

    #[test]
    fn asset_symbol_whitelist() {
        for ok in ["index:^GSPC", "future:GC=F", "forex:EURUSD=X", "stock:BRK-B", "index:DX-Y.NYB", "stock:SAN.MC", "bond:DGS10", "crypto:BTC"] {
            assert!(ok.parse::<AssetId>().is_ok(), "{ok} debería ser válido");
        }
        let long = format!("stock:{}", "A".repeat(33));
        for bad in ["stock:", "stock:a/b", "stock:../x", "stock:a%00", "stock:a\r\nb", "stock:a\0", "stock:a b", "stock:ñu", "stock:a?b", "stock:a#b", long.as_str()] {
            assert!(bad.parse::<AssetId>().is_err(), "{bad:?} debería rechazarse");
        }
        // El error no refleja la entrada (log forging).
        let err = "stock:a\r\nb".parse::<AssetId>().unwrap_err().to_string();
        assert!(!err.contains('\n') && !err.contains('\r'));
        assert!(serde_json::from_str::<AssetId>("\"stock:a/b\"").is_err());
        assert!("stock:A".parse::<AssetId>().is_ok());
        assert!(format!("stock:{}", "A".repeat(32)).parse::<AssetId>().is_ok());
    }

    #[test]
    fn chat_event_is_tagged() {
        let json = serde_json::to_string(&ChatEvent::Token { text: "hola".into() }).unwrap();
        assert_eq!(json, r#"{"type":"token","text":"hola"}"#);
    }
}
