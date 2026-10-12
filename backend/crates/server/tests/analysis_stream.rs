//! `POST /api/assets/{id}/analysis/stream`: progreso por SSE, single-flight compartido con el
//! POST, guardado en BD y continuidad si el cliente se desconecta.

mod common;

use std::{sync::atomic::Ordering, time::Duration};

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_keys, sse_events};
use serde_json::Value;
use server::fake::{FakeAnalyst, FakeMarket};

const URI: &str = "/api/assets/stock%3AAAPL/analysis/stream";

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn streams_stages_then_done_and_saves_the_analysis() {
    let app = TestApp::new().await;
    let (status, headers, body) = app.raw(Method::POST, URI, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let events = sse_events(&body);
    assert_eq!(types(&events), ["stage", "stage", "stage", "stage", "done"]);
    assert_keys(&events[0], &["type", "stage", "label", "index", "total"]);
    assert_eq!(events[0]["stage"], "snapshot");
    assert_eq!((events[0]["index"].as_u64(), events[0]["total"].as_u64()), (Some(1), Some(4)));
    assert_eq!(events[3]["stage"], "verify");
    assert_keys(&events[4], &["type", "analysis"]);
    assert_eq!(events[4]["analysis"]["asset"], "stock:AAPL");
    let (_, latest) = app.get("/api/assets/stock%3AAAPL/analysis").await;
    assert_eq!(latest["id"], events[4]["analysis"]["id"], "guardado en BD igual que el POST");
}

#[tokio::test]
async fn errors_come_as_an_error_event_and_bad_depth_is_400() {
    let app = TestApp::new().await;
    let (status, _, body) = app.raw(Method::POST, "/api/assets/stock%3AZZZZ/analysis/stream", None).await;
    assert_eq!(status, StatusCode::OK);
    let events = sse_events(&body);
    let last = events.last().unwrap();
    assert_eq!(last["type"], "error");
    assert_keys(last, &["type", "code", "message"]);
    assert_eq!(last["code"], "not_found");
    let (status, _, _) = app.raw(Method::POST, &format!("{URI}?depth=enorme"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn concurrent_streams_and_post_share_one_analysis() {
    let analyst = FakeAnalyst::default();
    *analyst.analyze_delay.lock().unwrap() = Duration::from_millis(300);
    let app = std::sync::Arc::new(TestApp::with(FakeMarket::default(), analyst).await);
    let a = tokio::spawn({
        let app = app.clone();
        async move { app.raw(Method::POST, URI, None).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let b = tokio::spawn({
        let app = app.clone();
        async move { app.raw(Method::POST, URI, None).await }
    });
    let post = tokio::spawn({
        let app = app.clone();
        async move { app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await }
    });
    let (ra, rb, rp) = (a.await.unwrap(), b.await.unwrap(), post.await.unwrap());
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 1, "un solo análisis para los tres clientes");
    let (ea, eb) = (sse_events(&ra.2), sse_events(&rb.2));
    assert_eq!(ea.last().unwrap()["analysis"]["id"], eb.last().unwrap()["analysis"]["id"]);
    assert_eq!(rp.0, StatusCode::OK);
    assert_eq!(rp.1["id"], ea.last().unwrap()["analysis"]["id"]);
    // El que llegó tarde ve al menos la etapa en curso y el final.
    assert_eq!(types(&eb).last(), Some(&"done"));
    assert!(types(&eb).contains(&"stage"));
}

#[tokio::test]
async fn analysis_is_saved_even_if_the_client_disconnects() {
    let analyst = FakeAnalyst::default();
    *analyst.analyze_delay.lock().unwrap() = Duration::from_millis(200);
    let app = TestApp::with(FakeMarket::default(), analyst).await;
    // El cliente espera solo 50 ms y se va (se suelta la petición).
    let req = app.raw(Method::POST, URI, None);
    assert!(tokio::time::timeout(Duration::from_millis(50), req).await.is_err());
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (_, latest) = app.get("/api/assets/stock%3AAAPL/analysis").await;
    assert_eq!(latest["asset"], "stock:AAPL", "el análisis se guardó sin cliente");
}

#[tokio::test]
async fn without_a_permit_post_is_429_and_sse_sends_a_rate_limited_error() {
    let app = TestApp::new().await;
    let held: Vec<_> = (0..server::limits::MAX_ANALYSES)
        .map(|_| app.state.limits.analyses.clone().try_acquire_owned().unwrap())
        .collect();
    let (status, body) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");
    let (status, _, body) = app.raw(Method::POST, URI, None).await;
    assert_eq!(status, StatusCode::OK);
    let events = sse_events(&body);
    assert_eq!(events.last().unwrap()["type"], "error");
    assert_eq!(events.last().unwrap()["code"], "rate_limited");
    drop(held);
    // Liberados los permisos, vuelve a funcionar y el permiso se devuelve al terminar.
    let (status, _) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.state.limits.analyses.available_permits(), server::limits::MAX_ANALYSES);
}
