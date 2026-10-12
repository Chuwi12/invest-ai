//! Utilidades compartidas por los tests de integración. No todos los ficheros usan todo.
#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::{
    AppState, Settings, db,
    fake::{FakeAnalyst, FakeMarket, annotate_fake},
};
use tower::ServiceExt;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub market: Arc<FakeMarket>,
    pub analyst: Arc<FakeAnalyst>,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with(FakeMarket::default(), FakeAnalyst::default()).await
    }

    pub async fn with(market: FakeMarket, analyst: FakeAnalyst) -> Self {
        Self::with_static(market, analyst, None).await
    }

    pub async fn with_static(market: FakeMarket, analyst: FakeAnalyst, static_dir: Option<std::path::PathBuf>) -> Self {
        // Con `RUST_LOG` se ven los errores internos (p. ej. de SQLite) que la API oculta al cliente.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let pool = db::connect_memory().await.expect("BD en memoria");
        let (market, analyst) = (Arc::new(market), Arc::new(analyst));
        let state = AppState::new(
            market.clone(),
            analyst.clone(),
            pool,
            Settings { language: "es".into(), allowed_origins: vec!["http://localhost:5180".into()] },
        )
        .with_news_annotator(annotate_fake);
        let router = server::app(state.clone(), static_dir);
        Self { router, state, market, analyst }
    }

    pub async fn call(&self, req: Request<Body>) -> Response {
        self.router.clone().oneshot(req).await.expect("el router no falla")
    }

    pub async fn raw(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, HeaderMap, Bytes) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(v) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self.call(builder.body(body).unwrap()).await;
        let (parts, body) = res.into_parts();
        (parts.status, parts.headers, body.collect().await.expect("cuerpo").to_bytes())
    }

    /// Devuelve el JSON de la respuesta (`Value::Null` si no hay cuerpo).
    pub async fn send(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, bytes) = self.raw(method, uri, body).await;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("JSON inválido ({e}): {}", String::from_utf8_lossy(&bytes)))
        };
        (status, value)
    }

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send(Method::GET, uri, None).await
    }

    pub async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send(Method::POST, uri, Some(body)).await
    }

    pub async fn create_conversation(&self, body: Value) -> Value {
        let (status, conv) = self.post("/api/conversations", body).await;
        assert_eq!(status, StatusCode::OK, "{conv}");
        conv
    }
}

/// Comprueba que un objeto JSON tiene exactamente estas claves.
pub fn assert_keys(value: &Value, expected: &[&str]) {
    let obj = value.as_object().unwrap_or_else(|| panic!("se esperaba un objeto: {value}"));
    let mut got: Vec<&str> = obj.keys().map(String::as_str).collect();
    let mut want: Vec<&str> = expected.to_vec();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want, "claves distintas en {value}");
}

/// Sustituye por `"<dyn>"` los campos que cambian en cada ejecución (ids, fechas), comprobando
/// que existen y son cadenas.
pub fn mask(value: &mut Value, keys: &[&str]) {
    for key in keys {
        let slot = value.get_mut(*key).unwrap_or_else(|| panic!("falta el campo {key}"));
        assert!(slot.is_string(), "el campo {key} debería ser una cadena: {slot}");
        *slot = json!("<dyn>");
    }
}

/// Espera (con sondeo) a que `check` devuelva `Some`.
pub async fn wait_for<T, Fut: std::future::Future<Output = Option<T>>>(mut check: impl FnMut() -> Fut) -> T {
    for _ in 0..100 {
        if let Some(v) = check().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("condición no cumplida a tiempo");
}

/// Eventos `data:` de un cuerpo SSE ya completo, como JSON (ignora los comentarios de keep-alive).
pub fn sse_events(body: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(body)
        .split("\n\n")
        .filter_map(|chunk| {
            let data: Vec<&str> = chunk.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect();
            (!data.is_empty()).then(|| serde_json::from_str(&data.join("\n")).expect("evento SSE con JSON válido"))
        })
        .collect()
}
