//! Rutas de OPV de EE. UU.: calendario, folleto, rendimiento posterior, tasa base y perfil de
//! riesgo. El perfil es **determinista** (reglas sobre el folleto; sin LLM) y no es una
//! recomendación: ver `docs/notes/ipo-risk.md`.

use std::{sync::LazyLock, time::Duration};

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{Duration as Days, NaiveDate, Utc};
use domain::{
    DomainError, IpoBaseRate, IpoData, IpoEntry, IpoPerformance, IpoRiskProfile, IpoStatus, Prospectus,
};
use market::ipo_risk::{ProfileCache, ProfileInput};
use serde::Deserialize;

use crate::{
    error::{ApiError, ApiResult},
    extract::AppQuery,
    state::AppState,
};

/// Ventana por defecto del calendario: un mes atrás y dos adelante.
const DEFAULT_PAST_DAYS: i64 = 30;
const DEFAULT_FUTURE_DAYS: i64 = 60;
/// Tope del rango pedido (cada tramo de 200 filas de Finnhub cuesta una petición).
const MAX_SPAN_DAYS: i64 = 180;
/// Hasta dónde se puede mirar atrás / adelante.
const MAX_LOOKBACK_DAYS: i64 = 365 * 5;
const MAX_LOOKAHEAD_DAYS: i64 = 365;
const DEFAULT_MONTHS: u32 = 12;
const MAX_MONTHS: u32 = 24;
const DEFAULT_MAX_CHARS: usize = 12_000;
const MIN_MAX_CHARS: usize = 500;
const MAX_MAX_CHARS: usize = 60_000;
/// Cuánto se espera a la tasa base antes de servir el perfil sin ella (el cálculo sigue en
/// segundo plano y la próxima petición la encuentra en caché).
const BASE_RATE_WAIT: Duration = Duration::from_secs(12);
const BENCHMARKS: &[&str] = &["SPY", "IWM", "QQQ"];

/// Perfiles ya construidos, por huella de sus entradas (folleto, calendario, tasa base). Global del
/// proceso como el `HUB` de análisis: depende solo de las entradas, no del estado de la app.
static PROFILES: LazyLock<ProfileCache> = LazyLock::new(ProfileCache::default);

/// Implementación por defecto de `AppState::ipo`: sin datos de OPV configurados.
pub struct NoIpoData;

#[async_trait]
impl IpoData for NoIpoData {
    async fn calendar(&self, _: NaiveDate, _: NaiveDate) -> domain::Result<Vec<IpoEntry>> {
        Err(unconfigured())
    }
    async fn prospectus(&self, _: &str) -> domain::Result<Prospectus> {
        Err(unconfigured())
    }
    async fn performance(&self, _: &str, _: NaiveDate, _: Option<f64>, _: &str) -> domain::Result<IpoPerformance> {
        Err(unconfigured())
    }
    async fn base_rate(&self, _: u32) -> domain::Result<IpoBaseRate> {
        Err(unconfigured())
    }
}

fn unconfigured() -> DomainError {
    DomainError::Unavailable("los datos de OPV no están configurados".into())
}

/// Errores hacia el cliente: sin URLs, proveedores ni mensajes internos (van al log).
fn public(e: DomainError, what: &str) -> ApiError {
    match e {
        DomainError::NotFound(m) => ApiError::not_found(m),
        DomainError::InvalidInput(m) => ApiError::invalid(m),
        DomainError::RateLimited { provider } => {
            tracing::warn!(%provider, what, "límite de peticiones en OPV");
            ApiError::Domain(DomainError::RateLimited { provider: "de datos".into() })
        }
        DomainError::Upstream { provider, message } => {
            tracing::warn!(%provider, %message, what, "fallo del proveedor en OPV");
            ApiError::Domain(DomainError::Upstream {
                provider: "de datos".into(),
                message: format!("{what}: el proveedor de datos no respondió"),
            })
        }
        DomainError::Unavailable(m) => ApiError::Domain(DomainError::Unavailable(m)),
        DomainError::Internal(m) => {
            tracing::error!(error = %m, what, "error interno en OPV");
            ApiError::internal(format!("{what}: error interno"))
        }
    }
}

/// Símbolo de la ruta: validación de `market::symbol` y, además, solo letras, dígitos, `.` y `-`.
fn parse_symbol(raw: &str) -> Result<String, ApiError> {
    let s = raw.trim().to_ascii_uppercase();
    market::symbol::validate_symbol(&s)?;
    if s.len() > 12 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(ApiError::invalid("símbolo de OPV no válido (letras, cifras, «.» y «-»; máx. 12)"));
    }
    Ok(s)
}

fn check_date(label: &str, d: NaiveDate, today: NaiveDate) -> Result<(), ApiError> {
    if d < today - Days::days(MAX_LOOKBACK_DAYS) || d > today + Days::days(MAX_LOOKAHEAD_DAYS) {
        return Err(ApiError::invalid(format!("{label} fuera de rango (hasta 5 años atrás y 1 año adelante)")));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarParams {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

/// `GET /api/ipo/calendar?from=&to=`. Por defecto de hoy−30 a hoy+60 días; rango máx. 180 días.
pub async fn calendar(State(state): State<AppState>, AppQuery(p): AppQuery<CalendarParams>) -> ApiResult<Json<Vec<IpoEntry>>> {
    let today = Utc::now().date_naive();
    let from = p.from.unwrap_or(today - Days::days(DEFAULT_PAST_DAYS));
    let to = p.to.unwrap_or_else(|| if p.from.is_none() { today + Days::days(DEFAULT_FUTURE_DAYS) } else { from + Days::days(DEFAULT_PAST_DAYS + DEFAULT_FUTURE_DAYS) });
    if from > to {
        return Err(ApiError::invalid("from no puede ser posterior a to"));
    }
    if (to - from).num_days() > MAX_SPAN_DAYS {
        return Err(ApiError::invalid(format!("el rango máximo es de {MAX_SPAN_DAYS} días")));
    }
    check_date("from", from, today)?;
    check_date("to", to, today)?;
    let mut rows = state.ipo.calendar(from, to).await.map_err(|e| public(e, "calendario de OPV"))?;
    rows.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.name.cmp(&b.name)));
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProspectusParams {
    max_chars: Option<usize>,
}

/// Recorta cada sección a `max` caracteres (el texto completo se usa para el perfil de riesgo).
fn cut_sections(p: &mut Prospectus, max: usize) {
    for s in &mut p.sections {
        if s.text.chars().count() > max {
            s.text = s.text.chars().take(max).collect();
            s.truncated = true;
        }
    }
}

/// `GET /api/ipo/{symbol}/prospectus?maxChars=`. Secciones del folleto de la OPV en texto limpio.
pub async fn prospectus(
    State(state): State<AppState>,
    Path(symbol): Path<String>,
    AppQuery(p): AppQuery<ProspectusParams>,
) -> ApiResult<Json<Prospectus>> {
    let sym = parse_symbol(&symbol)?;
    let max = p.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
    if !(MIN_MAX_CHARS..=MAX_MAX_CHARS).contains(&max) {
        return Err(ApiError::invalid(format!("maxChars debe estar entre {MIN_MAX_CHARS} y {MAX_MAX_CHARS}")));
    }
    let mut pr = state.ipo.prospectus(&sym).await.map_err(|e| public(e, "folleto"))?;
    cut_sections(&mut pr, max);
    Ok(Json(pr))
}

/// Entrada del calendario de `symbol`: con `date`, ±7 días; sin ella, de hoy−120 a hoy+90.
/// Prefiere precio fijado > prevista > presentada.
async fn find_entry(state: &AppState, symbol: &str, date: Option<NaiveDate>) -> Option<IpoEntry> {
    let today = Utc::now().date_naive();
    let (from, to) = match date {
        Some(d) => (d - Days::days(7), d + Days::days(7)),
        None => (today - Days::days(120), today + Days::days(90)),
    };
    let rows = match state.ipo.calendar(from, to).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "calendario de OPV no disponible para buscar la entrada");
            return None;
        }
    };
    let rank = |s: IpoStatus| match s {
        IpoStatus::Priced => 0,
        IpoStatus::Expected => 1,
        IpoStatus::Filed => 2,
        _ => 3,
    };
    rows.into_iter()
        .filter(|e| e.symbol.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(symbol)))
        .min_by_key(|e| (rank(e.status), (e.date - today).num_days().abs()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceParams {
    date: Option<NaiveDate>,
    offer_price: Option<f64>,
    benchmark: Option<String>,
}

/// `GET /api/ipo/{symbol}/performance?date=&offerPrice=&benchmark=SPY`. Rentabilidad tras la
/// OPV (día 1, 1/3/6/12 meses) y exceso frente al índice. Sin `date`, la del calendario.
pub async fn performance(
    State(state): State<AppState>,
    Path(symbol): Path<String>,
    AppQuery(p): AppQuery<PerformanceParams>,
) -> ApiResult<Json<IpoPerformance>> {
    let sym = parse_symbol(&symbol)?;
    let today = Utc::now().date_naive();
    if let Some(d) = p.date {
        check_date("date", d, today)?;
    }
    if let Some(price) = p.offer_price
        && !(price.is_finite() && price > 0.0 && price < 100_000.0)
    {
        return Err(ApiError::invalid("offerPrice debe estar entre 0 y 100000"));
    }
    let bench = p.benchmark.as_deref().unwrap_or("SPY").trim().to_ascii_uppercase();
    if !BENCHMARKS.contains(&bench.as_str()) {
        return Err(ApiError::invalid(format!("benchmark no admitido (usa {})", BENCHMARKS.join(", "))));
    }
    let (date, price) = match p.date {
        Some(d) => (d, p.offer_price),
        None => {
            let e = find_entry(&state, &sym, None)
                .await
                .filter(|e| e.status == IpoStatus::Priced)
                .ok_or_else(|| ApiError::not_found("la OPV no está en el calendario con precio fijado; indica date"))?;
            (e.date, p.offer_price.or(e.price_low))
        }
    };
    if date > today {
        return Err(ApiError::invalid("la OPV aún no cotiza: no hay rendimiento posterior"));
    }
    let perf = state.ipo.performance(&sym, date, price, &bench).await.map_err(|e| public(e, "rendimiento de la OPV"))?;
    Ok(Json(perf))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentParams {
    months: Option<u32>,
}

/// `GET /api/ipo/recent-performance?months=12`. Tasa base: medianas del primer día y de 3 meses
/// de las OPV recientes, con el tamaño de muestra. Describe el pasado, no predice.
pub async fn recent_performance(State(state): State<AppState>, AppQuery(p): AppQuery<RecentParams>) -> ApiResult<Json<IpoBaseRate>> {
    let months = p.months.unwrap_or(DEFAULT_MONTHS);
    if !(1..=MAX_MONTHS).contains(&months) {
        return Err(ApiError::invalid(format!("months debe estar entre 1 y {MAX_MONTHS}")));
    }
    Ok(Json(state.ipo.base_rate(months).await.map_err(|e| public(e, "tasa base de OPV"))?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileParams {
    date: Option<NaiveDate>,
}

/// `GET /api/ipo/{symbol}/risk-profile?date=`. Perfil de riesgo determinista: indicadores del
/// folleto y del calendario (cada uno con su fuente y su evidencia, o «no extraído») y la tasa
/// base de OPV recientes. Si el folleto no está, devuelve el perfil con lo que haya y avisos.
pub async fn risk_profile(
    State(state): State<AppState>,
    Path(symbol): Path<String>,
    AppQuery(p): AppQuery<ProfileParams>,
) -> ApiResult<Json<IpoRiskProfile>> {
    let sym = parse_symbol(&symbol)?;
    let today = Utc::now().date_naive();
    if let Some(d) = p.date {
        check_date("date", d, today)?;
    }
    let base = async {
        match tokio::time::timeout(BASE_RATE_WAIT, state.ipo.base_rate(DEFAULT_MONTHS)).await {
            Ok(Ok(b)) => (Some(b), None),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "tasa base de OPV no disponible");
                (None, None)
            }
            Err(_) => (None, Some("La tasa base de OPV recientes se está calculando; recarga en unos segundos.")),
        }
    };
    let (entry, prospectus, (base_rate, base_note)) =
        tokio::join!(find_entry(&state, &sym, p.date), state.ipo.prospectus(&sym), base);
    let prospectus = match prospectus {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(symbol = %sym, error = %e, "folleto de OPV no disponible para el perfil");
            None
        }
    };
    if entry.is_none() && prospectus.is_none() {
        return Err(ApiError::not_found("no se encontró la OPV ni su folleto"));
    }
    // El perfil lee todo el texto del folleto (CPU pura, sin `await`): fuera del worker async, donde
    // ni el timeout de 30 s podría cortarlo, y con caché para no repetirlo en cada recarga.
    let name = sym.clone();
    let built = tokio::task::spawn_blocking(move || {
        PROFILES.get_or_build(ProfileInput {
            symbol: &name,
            entry: entry.as_ref(),
            prospectus: prospectus.as_ref(),
            base_rate,
            now: Utc::now(),
        })
    })
    .await
    .map_err(|e| {
        tracing::error!(symbol = %sym, error = %e, "el cálculo del perfil de OPV falló");
        ApiError::internal("perfil de riesgo: error interno")
    })?;
    let mut profile = IpoRiskProfile::clone(&built);
    if let Some(note) = base_note {
        profile.warnings.retain(|w| !w.contains("tasa base de OPV recientes no está disponible"));
        profile.warnings.push(note.to_string());
    }
    Ok(Json(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_validated() {
        assert_eq!(parse_symbol(" trxb ").unwrap(), "TRXB");
        assert_eq!(parse_symbol("BRK.B").unwrap(), "BRK.B");
        for bad in ["", "../etc", "A/B", "AB CD", "A^B", "A=B", "TOOLONGSYMBOLXX", "a;b"] {
            assert!(parse_symbol(bad).is_err(), "{bad}");
        }
    }
}
