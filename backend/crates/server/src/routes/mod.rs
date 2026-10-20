//! Router: agrupa las rutas según las capas que les corresponden.

use std::{path::PathBuf, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use domain::DomainError;
use serde_json::json;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{error::ApiError, state::AppState};

pub mod analysis;
pub mod chat;
pub mod conversations;
pub mod dashboard;
pub mod favorites;
pub mod history;
pub mod ipo;
pub mod knowledge;
pub mod learning;
pub mod market;
pub mod models;
pub mod social;
pub mod static_files;
pub mod system;
pub mod watchlist;
pub mod ws;

/// Tiempo máximo de las rutas REST normales (no SSE, WebSocket ni generación de análisis).
pub const REST_TIMEOUT: Duration = Duration::from_secs(30);
/// Tamaño máximo del cuerpo de una petición.
pub const BODY_LIMIT: usize = 256 * 1024;

pub fn app(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let origins = state.settings.allowed_origins.clone();
    let settings = state.settings.clone();
    let mut app = Router::new().nest("/api", api(state));
    if let Some(dir) = static_dir {
        app = app.merge(static_files::router(dir));
    }
    app.layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(cors(&origins))
        .layer(middleware::from_fn_with_state(settings, crate::guard::guard))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(middleware::from_fn(crate::security_headers::apply))
}

fn api(state: AppState) -> Router {
    // REST normal: timeout de 30 s y compresión.
    let rest = Router::new()
        .route("/health", get(system::health))
        .route("/status", get(system::status))
        .route("/search", get(market::search))
        .route("/assets/{id}", get(market::asset))
        .route("/assets/{id}/quote", get(market::quote))
        .route("/quotes", get(market::quotes))
        .route("/assets/{id}/history", get(market::history))
        .route("/assets/{id}/indicators", get(market::indicators))
        .route("/assets/{id}/fundamentals", get(market::fundamentals))
        .route("/assets/{id}/news", get(market::asset_news))
        .route("/news", get(market::market_news))
        .route("/macro", get(market::macro_context))
        .route("/assets/{id}/analysis", get(analysis::latest))
        .route("/assets/{id}/analyses", get(analysis::history))
        .route("/assets/{id}/snapshot", get(analysis::snapshot))
        .route("/conversations", get(conversations::list).post(conversations::create))
        .route(
            "/conversations/{cid}",
            get(conversations::get).patch(conversations::patch).delete(conversations::delete),
        )
        .route("/favorites", get(favorites::list))
        .route("/favorites/{id}", axum::routing::put(favorites::put).delete(favorites::delete))
        .route("/history/searches", get(history::searches).post(history::record_selection).delete(history::clear_searches))
        .route("/history/views", get(history::views))
        .route("/dashboard", get(dashboard::dashboard))
        .route("/ipo/calendar", get(ipo::calendar))
        .route("/ipo/recent-performance", get(ipo::recent_performance))
        .route("/ipo/{symbol}/prospectus", get(ipo::prospectus))
        .route("/ipo/{symbol}/performance", get(ipo::performance))
        .route("/ipo/{symbol}/risk-profile", get(ipo::risk_profile))
        .route("/knowledge/stats", get(knowledge::stats))
        .route("/social/status", get(social::status))
        .route("/assets/{id}/social", get(social::asset_social))
        .route("/learning/performance", get(learning::performance))
        .route("/learning/weights", get(learning::weights))
        .route("/learning/weights/rollback", post(learning::rollback))
        .route("/learning/calibrated-confidence", get(learning::calibrated_confidence))
        .route("/watchlist", get(watchlist::status))
        .route("/models", get(models::list))
        .route("/models/active", put(models::set_active))
        .route("/models/{id}/gpu", put(models::set_force_gpu))
        .route("/models/{id}", axum::routing::delete(models::delete))
        .layer(middleware::from_fn(rest_timeout))
        .layer(CompressionLayer::new());

    // Generar un análisis puede tardar un minuto: sin timeout de 30 s.
    let generation = Router::new()
        .route("/assets/{id}/analysis", post(analysis::generate))
        .layer(CompressionLayer::new());

    // Chat (SSE): tope de generaciones simultáneas; el permiso dura lo que el stream.
    let chat = Router::new()
        .route("/conversations/{cid}/messages", post(chat::send_message))
        .layer(middleware::from_fn_with_state(state.limits.chats.clone(), crate::limits::limit));

    // SSE y WebSocket, sin timeout ni compresión (comprimir rompería el streaming y el handshake 101).
    let streaming = Router::new()
        .route("/assets/{id}/analysis/stream", post(analysis::generate_stream))
        .route("/models/{id}/pull", post(models::pull))
        .route("/ws", get(ws::handler));

    Router::new()
        .merge(rest)
        .merge(generation)
        .merge(streaming)
        .merge(chat)
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(unknown_route)
        .with_state(state)
}

async fn rest_timeout(req: Request, next: Next) -> Response {
    match tokio::time::timeout(REST_TIMEOUT, next.run(req)).await {
        Ok(res) => res,
        Err(_) => ApiError::Domain(DomainError::Unavailable("tiempo de espera agotado".into())).into_response(),
    }
}

async fn unknown_route() -> Response {
    ApiError::not_found("ruta de la API desconocida").into_response()
}

async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({ "error": { "code": "method_not_allowed", "message": "método no permitido" } })),
    )
        .into_response()
}

fn cors(origins: &[String]) -> CorsLayer {
    let allowed: Vec<HeaderValue> = origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::ACCEPT])
        .max_age(Duration::from_secs(3600))
}
