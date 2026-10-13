//! Rutas de mercado: búsqueda, ficha, cotizaciones, histórico, indicadores, noticias y macro.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
};
use domain::{
    AssetClass, AssetId, AssetSummary, DomainError, Fundamentals, Indicators, Interval, MacroContext,
    NewsItem, Quote, Range,
};
use serde::Deserialize;

use crate::{
    error::{ApiError, ApiResult},
    extract::{AppQuery, AssetPath},
    models::AssetDetail,
    repo,
    resolve::{cache_summaries, resolve_summary},
    state::AppState,
};

const MAX_QUERY_CHARS: usize = 100;
const MAX_QUOTES_PER_REQUEST: usize = 50;
const DEFAULT_NEWS_LIMIT: usize = 30;
const MAX_NEWS_LIMIT: usize = 100;

#[derive(Deserialize)]
pub struct SearchParams {
    q: String,
    limit: Option<usize>,
    class: Option<AssetClass>,
}

pub async fn search(
    State(state): State<AppState>,
    AppQuery(p): AppQuery<SearchParams>,
) -> ApiResult<Json<Vec<AssetSummary>>> {
    let q = p.q.trim();
    if q.is_empty() {
        return Err(ApiError::invalid("q es obligatorio"));
    }
    if q.chars().count() > MAX_QUERY_CHARS {
        return Err(ApiError::invalid(format!("q no puede superar {MAX_QUERY_CHARS} caracteres")));
    }
    let limit = p.limit.unwrap_or(20).clamp(1, 50);
    // El trait no filtra por clase: con filtro se pide el máximo para que el recorte no deje
    // la lista vacía de forma artificial.
    let fetch = if p.class.is_some() { 50 } else { limit };
    let mut results = state.market.search(q, fetch).await?;
    if let Some(class) = p.class {
        results.retain(|r| r.class == class);
    }
    results.truncate(limit);

    // Efectos secundarios (caché e historial): nunca hacen fallar la búsqueda.
    cache_summaries(&state, &results).await;
    if let Err(e) = repo::history::record_search(&state.db, q).await {
        tracing::warn!(error = %e, "no se pudo registrar la búsqueda");
    }
    Ok(Json(results))
}

/// Registra el fallo de una parte opcional de la respuesta y la deja en `None`.
fn soft<T>(what: &str, id: &AssetId, res: Result<T, DomainError>) -> Option<T> {
    res.map_err(|e| tracing::warn!(%id, error = %e, "{what} no disponible")).ok()
}

pub async fn asset(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<Json<AssetDetail>> {
    let (summary, quote, fundamentals, is_favorite) = tokio::join!(
        resolve_summary(&state, &id),
        state.market.quote(&id),
        state.market.fundamentals(&id),
        repo::favorites::is_favorite(&state.db, &id),
    );
    let summary = summary?;
    if let Err(e) = repo::history::record_view(&state.db, &summary).await {
        tracing::warn!(error = %e, "no se pudo registrar la visita");
    }
    Ok(Json(AssetDetail {
        summary,
        quote: soft("cotización", &id, quote),
        fundamentals: soft("fundamentales", &id, fundamentals).flatten(),
        is_favorite: is_favorite.unwrap_or(false),
    }))
}

pub async fn quote(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<Json<Quote>> {
    Ok(Json(state.market.quote(&id).await?))
}

#[derive(Deserialize)]
pub struct QuotesParams {
    ids: String,
}

pub async fn quotes(
    State(state): State<AppState>,
    AppQuery(p): AppQuery<QuotesParams>,
) -> ApiResult<Json<Vec<Quote>>> {
    let mut ids: Vec<AssetId> = Vec::new();
    for raw in p.ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let id: AssetId = raw.parse()?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() > MAX_QUOTES_PER_REQUEST {
        return Err(ApiError::invalid(format!("ids admite como máximo {MAX_QUOTES_PER_REQUEST} activos")));
    }
    if ids.is_empty() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(state.market.quotes(&ids).await?))
}

#[derive(Deserialize)]
pub struct HistoryParams {
    range: Option<Range>,
    interval: Option<Interval>,
}

/// Procedencia del histórico en cabeceras (el cuerpo sigue siendo `Candle[]`).
pub const H_SOURCE: HeaderName = HeaderName::from_static("x-data-source");
pub const H_SOURCE_URL: HeaderName = HeaderName::from_static("x-data-source-url");
pub const H_AS_OF: HeaderName = HeaderName::from_static("x-data-as-of");
/// Intervalo realmente servido (`1m`, `5m`, `1h`, `1d`, `1wk`, `1mo`); difiere del pedido si el
/// proveedor no admite esa combinación rango/intervalo y se subió de intervalo.
pub const H_INTERVAL: HeaderName = HeaderName::from_static("x-data-interval");

fn provenance_headers(o: &market::provenance::HistoryOrigin) -> HeaderMap {
    let mut h = HeaderMap::new();
    let mut put = |name: HeaderName, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(name, v);
        }
    };
    put(H_SOURCE, &o.source);
    if let Some(u) = &o.source_url {
        put(H_SOURCE_URL, u);
    }
    if let Some(t) = o.as_of {
        put(H_AS_OF, &t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Some(i) = o.interval {
        put(H_INTERVAL, i.as_str());
    }
    h
}

pub async fn history(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
    AppQuery(p): AppQuery<HistoryParams>,
) -> ApiResult<Response> {
    let range = p.range.unwrap_or(Range::Y1);
    let interval = p.interval.unwrap_or_else(|| range.default_interval());
    let candles = state.market.history(&id, range, interval).await?;
    let headers = market::provenance::history_origin(&id, range, interval)
        .map(|o| provenance_headers(&o))
        .unwrap_or_default();
    Ok((headers, Json(candles)).into_response())
}

pub async fn indicators(State(state): State<AppState>, AssetPath(id): AssetPath) -> ApiResult<Json<Indicators>> {
    Ok(Json(state.market.indicators(&id).await?))
}

pub async fn fundamentals(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<Option<Fundamentals>>> {
    Ok(Json(state.market.fundamentals(&id).await?))
}

#[derive(Deserialize)]
pub struct NewsParams {
    limit: Option<usize>,
}

impl NewsParams {
    fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_NEWS_LIMIT).clamp(1, MAX_NEWS_LIMIT)
    }
}

pub async fn asset_news(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
    AppQuery(p): AppQuery<NewsParams>,
) -> ApiResult<Json<Vec<NewsItem>>> {
    let mut news = state.market.news(Some(&id), p.limit()).await?;
    (state.annotate_news)(&mut news);
    Ok(Json(news))
}

pub async fn market_news(
    State(state): State<AppState>,
    AppQuery(p): AppQuery<NewsParams>,
) -> ApiResult<Json<Vec<NewsItem>>> {
    let mut news = state.market.news(None, p.limit()).await?;
    (state.annotate_news)(&mut news);
    Ok(Json(news))
}

pub async fn macro_context(State(state): State<AppState>) -> ApiResult<Json<MacroContext>> {
    Ok(Json(state.market.macro_context().await?))
}
