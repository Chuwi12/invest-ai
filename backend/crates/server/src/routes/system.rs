//! `/health` y `/status`.

use std::time::Duration;

use axum::{Json, extract::State};
use domain::LlmStatus;
use serde_json::{Value, json};

use crate::state::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

pub async fn status(State(state): State<AppState>) -> Json<Value> {
    // Si Ollama no contesta, /status no puede colgarse: es justo lo que sirve para diagnosticarlo.
    let llm = match tokio::time::timeout(Duration::from_secs(5), state.analyst.status()).await {
        Ok(llm) => llm,
        Err(_) => LlmStatus {
            available: false,
            base_url: String::new(),
            model: String::new(),
            installed_models: Vec::new(),
            supports_tools: false,
            error: Some("tiempo de espera agotado consultando al LLM".into()),
        },
    };
    // `outbound`: peticiones HTTP salientes a proveedores desde el arranque (diagnóstico de caché).
    let outbound: Vec<Value> = market::http::outbound_stats()
        .into_iter()
        .map(|(host, requests, avg_ms)| json!({ "host": host, "requests": requests, "avgMs": (avg_ms * 10.0).round() / 10.0 }))
        .collect();
    Json(json!({ "llm": llm, "providers": state.market.providers_status(), "outbound": outbound }))
}
