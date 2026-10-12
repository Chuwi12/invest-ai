//! Servidor Ollama simulado para los tests (puerto efímero, sin red real ni Ollama).
//!
//! `/api/chat` responde con guiones: cada petición consume el siguiente elemento de `chats`
//! (una lista de líneas NDJSON). Si se agotan, responde 500. Se registran los cuerpos recibidos.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::{Value, json};

#[derive(Default)]
pub struct MockState {
    pub chats: Mutex<VecDeque<Vec<String>>>,
    pub chat_requests: Mutex<Vec<Value>>,
    pub tags: Mutex<Value>,
    pub show: Mutex<Value>,
    pub pull_lines: Mutex<Vec<String>>,
    pub deleted: Mutex<Vec<String>>,
    /// Retardo entre líneas de la respuesta de chat (para probar cancelación y timeouts).
    pub line_delay: Mutex<Duration>,
    pub open_streams: std::sync::atomic::AtomicUsize,
}

pub struct MockOllama {
    pub url: String,
    pub state: Arc<MockState>,
}

/// Línea NDJSON de un fragmento de contenido.
pub fn content_line(text: &str) -> String {
    json!({"message": {"role": "assistant", "content": text}, "done": false}).to_string()
}

pub fn thinking_line(text: &str) -> String {
    json!({"message": {"role": "assistant", "content": "", "thinking": text}, "done": false}).to_string()
}

pub fn tool_call_line(name: &str, args: Value) -> String {
    json!({"message": {"role": "assistant", "content": "",
        "tool_calls": [{"function": {"name": name, "arguments": args}}]}, "done": false})
    .to_string()
}

pub fn done_line() -> String {
    json!({"message": {"role": "assistant", "content": ""}, "done": true, "done_reason": "stop",
        "prompt_eval_count": 120, "eval_count": 30})
    .to_string()
}

/// Respuesta completa de un solo golpe (contenido troceado en dos fragmentos).
pub fn reply(text: &str) -> Vec<String> {
    let mid = text.char_indices().nth(text.chars().count() / 2).map(|(i, _)| i).unwrap_or(0);
    vec![content_line(&text[..mid]), content_line(&text[mid..]), done_line()]
}

impl MockOllama {
    pub async fn start() -> Self {
        let state = Arc::new(MockState::default());
        *state.tags.lock().unwrap() = json!({"models": [
            {"name": "qwen2.5:7b", "size": 4683087332u64,
             "details": {"family": "qwen2", "parameter_size": "7.6B", "quantization_level": "Q4_K_M"}}
        ]});
        *state.show.lock().unwrap() = json!({
            "capabilities": ["completion", "tools"],
            "details": {"family": "qwen2", "parameter_size": "7.6B", "quantization_level": "Q4_K_M"},
            "model_info": {"general.architecture": "qwen2", "qwen2.block_count": 28,
                "qwen2.context_length": 32768, "qwen2.attention.head_count": 28,
                "qwen2.attention.head_count_kv": 4, "qwen2.embedding_length": 3584}
        });
        let app = Router::new()
            .route("/api/chat", post(chat))
            .route("/api/tags", get(|State(s): State<Arc<MockState>>| async move { Json(s.tags.lock().unwrap().clone()) }))
            .route("/api/show", post(|State(s): State<Arc<MockState>>| async move { Json(s.show.lock().unwrap().clone()) }))
            .route("/api/ps", get(|| async { Json(json!({"models": []})) }))
            .route("/api/pull", post(pull))
            .route("/api/delete", delete(del))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, state }
    }

    pub fn push_chat(&self, lines: Vec<String>) {
        self.state.chats.lock().unwrap().push_back(lines);
    }

    pub fn chat_requests(&self) -> Vec<Value> {
        self.state.chat_requests.lock().unwrap().clone()
    }
}

async fn chat(State(s): State<Arc<MockState>>, Json(body): Json<Value>) -> Response {
    s.chat_requests.lock().unwrap().push(body);
    let Some(lines) = s.chats.lock().unwrap().pop_front() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "sin guion para esta petición"}))).into_response();
    };
    let delay = *s.line_delay.lock().unwrap();
    let st = s.clone();
    let stream = async_stream::stream! {
        st.open_streams.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        struct Guard(Arc<MockState>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.open_streams.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _g = Guard(st.clone());
        for l in lines {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(format!("{l}\n")));
        }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn pull(State(s): State<Arc<MockState>>, Json(_): Json<Value>) -> Response {
    let lines = s.pull_lines.lock().unwrap().clone();
    let body = lines.join("\n") + "\n";
    Response::builder().header(header::CONTENT_TYPE, "application/x-ndjson").body(Body::from(body)).unwrap()
}

async fn del(State(s): State<Arc<MockState>>, Json(body): Json<Value>) -> Response {
    let name = body["model"].as_str().unwrap_or_default().to_string();
    let exists = s.tags.lock().unwrap()["models"].as_array().is_some_and(|m| m.iter().any(|x| x["name"] == name));
    if !exists {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("model '{name}' not found")}))).into_response();
    }
    s.deleted.lock().unwrap().push(name);
    StatusCode::OK.into_response()
}
