//! Rutas `/api/models`, ajustes persistentes y `?depth=` del análisis (con los fakes del servidor).

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_keys, sse_events};
use domain::SettingsStore;
use serde_json::json;

#[tokio::test]
async fn lists_models_with_hardware_in_the_contract_shape() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&body, &["active", "ollamaReachable", "models", "hardware", "loaded", "numGpu"]);
    assert_keys(&body["loaded"][0], &["id", "size", "sizeVram", "gpuShare"]);
    assert_keys(&body["hardware"], &["cpuModel", "cpuThreads", "ramTotalGb", "ramAvailableGb", "gpus", "diskFreeGb"]);
    assert_keys(&body["hardware"]["gpus"][0], &["name", "vendor", "vramTotalGb", "vramFreeGb"]);
    let m = &body["models"][1];
    assert_keys(
        m,
        &["id", "name", "family", "parameters", "activeParameters", "quantization", "contextWindow", "tier", "supportsTools", "reasoning", "description", "requirements", "curated", "installed", "active", "forceGpu", "gpuFallback"],
    );
    assert_keys(&m["requirements"], &["minVramGb", "recommendedVramGb", "minRamGb", "diskGb", "cpuOnlyOk"]);
    assert_eq!(m["tier"], "balanced");
    assert_eq!(body["active"], "qwen2.5:7b");
}

#[tokio::test]
async fn active_model_can_be_switched_only_to_installed_models() {
    let app = TestApp::new().await;
    let (s, b) = app.send(Method::PUT, "/api/models/active", Some(json!({"id": "qwen3:14b"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"]["code"], "invalid_input");
    let (s, b) = app.send(Method::PUT, "/api/models/active", Some(json!({"id": "mi-modelo:3b"}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["active"], "mi-modelo:3b");
    let (s, _) = app.send(Method::PUT, "/api/models/active", Some(json!({"nope": 1}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // El activo no se puede borrar; otro instalado sí (204) y luego ya no existe (404).
    let (s, b) = app.send(Method::DELETE, "/api/models/mi-modelo%3A3b", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let (s, _) = app.send(Method::DELETE, "/api/models/qwen2.5%3A7b", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = app.send(Method::DELETE, "/api/models/qwen2.5%3A7b", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Un nombre cualquiera que no está instalado tampoco llega a Ollama.
    let (s, b) = app.send(Method::DELETE, "/api/models/fantasma%3A1b", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
}

#[tokio::test]
async fn pull_streams_sse_progress_and_only_for_curated_ids() {
    let app = TestApp::new().await;
    let (s, h, bytes) = app.raw(Method::POST, "/api/models/qwen3%3A4b/pull", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let ev = sse_events(&bytes);
    assert_eq!(ev.first().unwrap()["type"], "progress");
    assert_eq!(ev.first().unwrap()["total"], 100);
    assert_eq!(ev.last().unwrap()["type"], "done");
    let (s, b) = app.send(Method::POST, "/api/models/evil%2Fmodel%3Alatest/pull", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
}

#[tokio::test]
async fn analysis_accepts_depth_and_rejects_unknown_values() {
    let app = TestApp::new().await;
    for d in ["quick", "standard", "deep"] {
        let (s, b) = app.send(Method::POST, &format!("/api/assets/stock%3AAAPL/analysis?depth={d}"), None).await;
        assert_eq!(s, StatusCode::OK, "{d}: {b}");
    }
    let (s, b) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis?depth=ultra", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"]["code"], "invalid_input");
    let (s, _) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn settings_are_persisted_in_sqlite() {
    let app = TestApp::new().await;
    let store = server::repo::settings::SqliteSettings::new(app.state.db.clone());
    assert_eq!(store.get("llm.active_model").await.unwrap(), None);
    store.set("llm.active_model", "qwen3:8b").await.unwrap();
    store.set("llm.active_model", "qwen3:14b").await.unwrap();
    assert_eq!(store.get("llm.active_model").await.unwrap().as_deref(), Some("qwen3:14b"));
}

#[tokio::test]
async fn force_gpu_can_be_toggled_per_model_and_defaults_to_off() {
    let app = TestApp::new().await;
    let (_, b) = app.get("/api/models").await;
    assert!(b["models"].as_array().unwrap().iter().all(|m| m["forceGpu"] == false), "por defecto decide Ollama");
    assert!(b["loaded"][0]["gpuShare"].as_f64().unwrap() < 1.0);
    let (s, b) = app.send(Method::PUT, "/api/models/qwen2.5%3A7b/gpu", Some(json!({"force": true}))).await;
    assert_eq!(s, StatusCode::OK);
    let q = b["models"].as_array().unwrap().iter().find(|m| m["id"] == "qwen2.5:7b").unwrap();
    assert_eq!(q["forceGpu"], true);
    assert_eq!(b["models"].as_array().unwrap().iter().filter(|m| m["forceGpu"] == true).count(), 1);
    assert_eq!(b["loaded"][0]["gpuShare"], 1.0);
    let (_, b) = app.send(Method::PUT, "/api/models/qwen2.5%3A7b/gpu", Some(json!({"force": false}))).await;
    assert!(b["models"].as_array().unwrap().iter().all(|m| m["forceGpu"] == false));
    let (s, _) = app.send(Method::PUT, "/api/models/qwen2.5%3A7b/gpu", Some(json!({"nope": 1}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
