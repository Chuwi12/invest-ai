//! Chat: un error a mitad de respuesta no debe guardarse como si fuera una respuesta completa (R-08).

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, sse_events};
use serde_json::json;

async fn assistant_message(app: &TestApp, text: &str) -> (Vec<serde_json::Value>, serde_json::Value) {
    let conv = app.create_conversation(json!({})).await;
    let cid = conv["id"].as_str().unwrap().to_string();
    let (status, _, body) = app
        .raw(Method::POST, &format!("/api/conversations/{cid}/messages"), Some(json!({ "content": text })))
        .await;
    assert_eq!(status, StatusCode::OK);
    let events = sse_events(&body);
    let (_, detail) = app.get(&format!("/api/conversations/{cid}")).await;
    let msgs = detail["messages"].as_array().unwrap().clone();
    let assistant = msgs.iter().find(|m| m["role"] == "assistant").cloned().expect("mensaje del asistente");
    (events, assistant)
}

#[tokio::test]
async fn error_mid_answer_saves_the_partial_text_marked_as_truncated() {
    let app = TestApp::new().await;
    let (events, msg) = assistant_message(&app, "esto falla").await;
    assert_eq!(events.last().unwrap()["type"], "error");
    assert_eq!(msg["content"], "Empiezo a responder");
    assert_eq!(msg["truncated"], true, "el texto parcial debe marcarse: {msg}");
}

#[tokio::test]
async fn complete_answers_are_not_truncated() {
    let app = TestApp::new().await;
    let (events, msg) = assistant_message(&app, "hola").await;
    assert_eq!(events.last().unwrap()["type"], "done");
    assert_eq!(msg["truncated"], false, "{msg}");
}
