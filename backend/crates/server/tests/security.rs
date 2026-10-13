//! Pruebas de las pruebas de concepto de `docs/notes/security.md` (S-01 ... S-09): el ataque
//! ahora falla y el uso legítimo sigue funcionando.

mod common;

use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use common::TestApp;
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::limits::Limits;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, client::IntoClientRequest, protocol::Message},
};

async fn call(
    router: &axum::Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, &str)>,
) -> (StatusCode, HeaderMap, String) {
    use tower::ServiceExt;
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = match body {
        Some((ct, data)) => {
            b = b.header(header::CONTENT_TYPE, ct);
            Body::from(data.to_string())
        }
        None => Body::empty(),
    };
    let res = router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, parts.headers, String::from_utf8_lossy(&bytes).into_owned())
}

fn code_of(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["error"]["code"].as_str().unwrap_or_default().to_string()
}

// ---------- S-01: Host ----------

#[tokio::test]
async fn s01_dns_rebinding_host_is_rejected() {
    let app = TestApp::new().await;
    let r = &app.router;
    let evil = [("host", "evil.example:8795"), ("origin", "http://evil.example:8795")];
    let (st, _, body) = call(r, Method::GET, "/api/conversations", &evil, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code_of(&body), "forbidden");
    for host in ["localhost:8795", "127.0.0.1:8795", "[::1]:8795", "localhost:5180"] {
        let (st, _, body) = call(r, Method::GET, "/api/conversations", &[("host", host)], None).await;
        assert_eq!(st, StatusCode::OK, "{host}: {body}");
    }
    // Truco de sufijo: no basta con que contenga "localhost".
    for host in ["localhost.evil.example", "127.0.0.1.evil.example:8795"] {
        let (st, ..) = call(r, Method::GET, "/api/health", &[("host", host)], None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{host}");
    }
    // curl sin Origin y con Host de loopback.
    let (st, ..) = call(r, Method::GET, "/api/health", &[("host", "127.0.0.1:8795")], None).await;
    assert_eq!(st, StatusCode::OK);
}

// ---------- S-02 / S-04: Origin en métodos no seguros ----------

#[tokio::test]
async fn s02_csrf_simple_requests_are_rejected() {
    let app = TestApp::new().await;
    let r = &app.router;
    let h = "127.0.0.1:8795";
    // PoC del informe: POST sin cuerpo desde otro origen.
    for uri in ["/api/models/qwen3:14b/pull", "/api/assets/stock:AAPL/analysis", "/api/conversations"] {
        let (st, _, body) = call(r, Method::POST, uri, &[("host", h), ("origin", "https://evil.example")], None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(code_of(&body), "forbidden");
    }
    // text/plain con Origin ajeno.
    let (st, ..) = call(
        r, Method::POST, "/api/conversations",
        &[("host", h), ("origin", "https://evil.example")], Some(("text/plain", r#"{"title":"pwned"}"#)),
    ).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // Sec-Fetch-Site: cross-site aunque el Origin sea aceptable.
    let (st, ..) = call(r, Method::POST, "/api/conversations", &[("host", h), ("sec-fetch-site", "cross-site")], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // DELETE/PUT/PATCH también.
    for m in [Method::DELETE, Method::PUT, Method::PATCH] {
        let (st, ..) = call(r, m, "/api/favorites/stock:AAPL", &[("host", h), ("origin", "https://evil.example")], None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }
    // Otra app local en otro puerto (S-04 para HTTP): sin comodines de puerto.
    let (st, ..) = call(r, Method::POST, "/api/conversations", &[("host", h), ("origin", "http://localhost:9999")], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // El preflight de un origen ajeno sigue sin `Access-Control-Allow-Origin`.
    let (_, hd, _) = call(
        r, Method::OPTIONS, "/api/conversations",
        &[("host", h), ("origin", "https://evil.example"), ("access-control-request-method", "DELETE")], None,
    ).await;
    assert!(!hd.contains_key("access-control-allow-origin"));
}

#[tokio::test]
async fn s02_legitimate_clients_still_work() {
    let app = TestApp::new().await;
    let r = &app.router;
    // curl: sin Origin.
    let (st, ..) = call(r, Method::POST, "/api/conversations", &[("host", "127.0.0.1:8795")], None).await;
    assert_eq!(st, StatusCode::OK);
    // Frontend mismo origen.
    let (st, ..) = call(
        r, Method::POST, "/api/conversations",
        &[("host", "127.0.0.1:8795"), ("origin", "http://127.0.0.1:8795"), ("sec-fetch-site", "same-origin")],
        Some(("application/json", r#"{"title":"x"}"#)),
    ).await;
    assert_eq!(st, StatusCode::OK);
    // Vite en 5180 con proxy sin changeOrigin: Host = Origin = localhost:5180.
    let (st, ..) = call(
        r, Method::POST, "/api/conversations",
        &[("host", "localhost:5180"), ("origin", "http://localhost:5180"), ("sec-fetch-site", "same-origin")],
        Some(("application/json; charset=utf-8", r#"{"title":"x"}"#)),
    ).await;
    assert_eq!(st, StatusCode::OK);
    // Vite con changeOrigin: Host = 127.0.0.1:8787, Origin = FRONTEND_ORIGIN.
    let (st, ..) = call(
        r, Method::DELETE, "/api/favorites/stock:AAPL",
        &[("host", "127.0.0.1:8787"), ("origin", "http://localhost:5180")], None,
    ).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn s02bis_optional_json_requires_json_content_type() {
    let app = TestApp::new().await;
    let r = &app.router;
    let h = [("host", "127.0.0.1:8795")];
    let (st, _, body) = call(r, Method::POST, "/api/conversations", &h, Some(("text/plain", r#"{"title":"pwned"}"#))).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    assert_eq!(code_of(&body), "unsupported_media_type");
    let (st, ..) = call(r, Method::POST, "/api/conversations", &h, Some(("application/x-www-form-urlencoded", "title=x"))).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // Cuerpo vacío con cualquier Content-Type, y JSON correcto.
    let (st, ..) = call(r, Method::POST, "/api/conversations", &h, Some(("text/plain", ""))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, ..) = call(r, Method::POST, "/api/conversations", &h, Some(("application/json", r#"{"title":"ok"}"#))).await;
    assert_eq!(st, StatusCode::OK);
}

// ---------- S-03: cabeceras ----------

/// `frontend/dist`, o la carpeta de `INVEST_AI_DIST_DIR` (para probar otra compilación, p. ej. una
/// con el CSS de lightweight-charts alterado y comprobar que el test del hash la detecta).
fn dist_dir() -> PathBuf {
    std::env::var_os("INVEST_AI_DIST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../frontend/dist"), PathBuf::from)
}

fn temp_static() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("invest-ai-sec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><html></html>").unwrap();
    std::fs::write(dir.join("assets/index-CV56swxT.js"), "console.log(1)").unwrap();
    std::fs::write(dir.join("assets/plain.js"), "console.log(2)").unwrap();
    dir
}

fn assert_common_headers(h: &HeaderMap) {
    for (k, v) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        ("x-frame-options", "DENY"),
    ] {
        assert_eq!(h.get(k).and_then(|v| v.to_str().ok()), Some(v), "{k} en {h:?}");
    }
}

#[tokio::test]
async fn s03_security_headers_everywhere() {
    let dir = temp_static();
    let app = TestApp::with_static(Default::default(), Default::default(), Some(dir.clone())).await;
    let r = &app.router;
    let h = [("host", "127.0.0.1:8795")];

    let (st, hd, _) = call(r, Method::GET, "/api/health", &h, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_common_headers(&hd);
    assert_eq!(hd[header::CACHE_CONTROL], "no-store");
    // Errores 404 de la API y respuestas 403 también.
    let (_, hd, _) = call(r, Method::GET, "/api/nope", &h, None).await;
    assert_common_headers(&hd);
    assert_eq!(hd[header::CACHE_CONTROL], "no-store");
    let (st, hd, _) = call(r, Method::GET, "/api/health", &[("host", "evil.example")], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_common_headers(&hd);

    // HTML: CSP completa con frame-ancestors.
    for path in ["/", "/asset/stock%3AAAPL"] {
        let (st, hd, _) = call(r, Method::GET, path, &h, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_common_headers(&hd);
        let csp = hd[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains(server::security_headers::CHARTS_STYLE_HASH));
        assert!(!csp.contains("unsafe-inline"));
        assert_eq!(hd[header::CACHE_CONTROL], "no-cache");
    }
    // Caché inmutable solo para /assets/ con hash.
    let (_, hd, _) = call(r, Method::GET, "/assets/index-CV56swxT.js", &h, None).await;
    assert_common_headers(&hd);
    assert_eq!(hd[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
    let (_, hd, _) = call(r, Method::GET, "/assets/plain.js", &h, None).await;
    assert_eq!(hd[header::CACHE_CONTROL], "no-cache");
    std::fs::remove_dir_all(dir).ok();
}

/// Si cambia la versión de lightweight-charts (o su CSS en línea), este test falla: hay que
/// actualizar `CHARTS_STYLE_HASH` (si no, el navegador bloquea el logo/estilo del gráfico).
#[test]
fn csp_style_hash_matches_dist() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let assets = dist_dir().join("assets");
    let Ok(entries) = std::fs::read_dir(&assets) else {
        eprintln!("SKIP: no hay frontend/dist (compila el frontend para verificar el hash de la CSP)");
        return;
    };
    let marker = "innerText=`a#tv-attr-logo";
    let mut found = Vec::new();
    for e in entries.flatten() {
        let Ok(js) = std::fs::read_to_string(e.path()) else { continue };
        let mut rest = js.as_str();
        while let Some(i) = rest.find(marker) {
            let start = i + "innerText=`".len();
            let end = rest[start..].find('`').expect("cadena sin cerrar") + start;
            found.push(rest[start..end].to_string());
            rest = &rest[end..];
        }
    }
    assert!(!found.is_empty(), "no se encontró el <style> en línea de lightweight-charts en dist/");
    for css in found {
        let hash = format!("sha256-{}", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(css.as_bytes())));
        assert_eq!(hash, server::security_headers::CHARTS_STYLE_HASH, "actualiza CHARTS_STYLE_HASH");
    }
}

// ---------- S-09 ----------

#[tokio::test]
async fn s09_hostile_symbols_are_rejected() {
    let app = TestApp::new().await;
    let h = [("host", "127.0.0.1:8795")];
    for uri in [
        "/api/assets/stock%3A%00/quote",
        "/api/assets/stock%3Aa%0d%0ab/quote",
        "/api/assets/stock%3A..%2F..%2Fx/quote",
        "/api/assets/stock%3Aa%20b/quote",
    ] {
        let (st, _, body) = call(&app.router, Method::GET, uri, &h, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(code_of(&body), "invalid_input");
        assert!(!body.contains('\r') && !body.contains("\\r") && !body.contains("\\u0000"), "{body}");
    }
    for ok in ["index%3A%5EGSPC", "crypto%3ABTC", "stock%3AAAPL"] {
        let (st, ..) = call(&app.router, Method::GET, &format!("/api/assets/{ok}/quote"), &h, None).await;
        assert_eq!(st, StatusCode::OK, "{ok}");
    }
}

// ---------- WebSocket y límites (servidor real) ----------

async fn serve(state: server::AppState) -> std::net::SocketAddr {
    let app = server::app(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(addr: std::net::SocketAddr, host: Option<&str>, origin: Option<&str>) -> Result<Ws, StatusCode> {
    let mut req = format!("ws://{addr}/api/ws").into_client_request().unwrap();
    if let Some(h) = host {
        req.headers_mut().insert(header::HOST, h.parse().unwrap());
    }
    if let Some(o) = origin {
        req.headers_mut().insert(header::ORIGIN, o.parse().unwrap());
    }
    match connect_async(req).await {
        Ok((s, resp)) => {
            assert_eq!(resp.status(), 101);
            Ok(s)
        }
        Err(tungstenite::Error::Http(resp)) => Err(resp.status()),
        Err(e) => panic!("error inesperado: {e}"),
    }
}

#[tokio::test]
async fn s01_s04_websocket_origin_and_host() {
    let app = TestApp::new().await;
    let addr = serve(app.state.clone()).await;
    let own = format!("http://{addr}");
    // PoC S-01: Host y Origin del atacante tras el rebinding.
    assert_eq!(ws(addr, Some("evil.example:8795"), Some("http://evil.example:8795")).await.err(), Some(StatusCode::FORBIDDEN));
    // PoC S-04: otro puerto de loopback.
    assert_eq!(ws(addr, None, Some("http://localhost:9999")).await.err(), Some(StatusCode::FORBIDDEN));
    assert_eq!(ws(addr, None, Some("https://evil.example")).await.err(), Some(StatusCode::FORBIDDEN));
    // Legítimos: sin Origin (script), mismo origen, Vite (FRONTEND_ORIGIN).
    assert!(ws(addr, None, None).await.is_ok());
    assert!(ws(addr, None, Some(&own)).await.is_ok());
    let mut vite = ws(addr, Some("localhost:5180"), Some("http://localhost:5180")).await.expect("Vite");
    vite.send(Message::text(json!({"type":"ping"}).to_string())).await.unwrap();
    let msg = vite.next().await.unwrap().unwrap();
    assert!(msg.to_text().unwrap().contains("pong"), "{msg:?}");
}

#[tokio::test]
async fn s06_websocket_cap_and_subscription_cap() {
    let app = TestApp::new().await;
    let mut state = app.state.clone();
    state.limits = Arc::new(Limits::new(2, 2, 2));
    let addr = serve(state).await;
    let a = ws(addr, None, None).await.unwrap();
    let mut b = ws(addr, None, None).await.unwrap();
    assert_eq!(ws(addr, None, None).await.err(), Some(StatusCode::TOO_MANY_REQUESTS));
    // Al cerrar uno se libera el hueco.
    drop(a);
    let mut again = None;
    for _ in 0..50 {
        match ws(addr, None, None).await {
            Ok(s) => {
                again = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    assert!(again.is_some(), "el permiso debe liberarse al cerrar el socket");
    // Más de 100 suscripciones por socket -> error.
    let ids: Vec<String> = (0..101).map(|i| format!("crypto:C{i}")).collect();
    b.send(Message::text(json!({"type":"subscribe","ids":ids}).to_string())).await.unwrap();
    let msg = b.next().await.unwrap().unwrap();
    assert!(msg.to_text().unwrap().contains("demasiadas suscripciones"), "{msg:?}");
}

#[tokio::test]
async fn s06_chat_concurrency_cap_is_immediate_429() {
    let app = TestApp::new().await;
    let mut state = app.state.clone();
    state.limits = Arc::new(Limits::new(2, 2, 0));
    let router = server::app(state, None);
    let h = [("host", "127.0.0.1:8795")];
    let (_, _, conv) = call(&router, Method::POST, "/api/conversations", &h, None).await;
    let cid = serde_json::from_str::<Value>(&conv).unwrap()["id"].as_str().unwrap().to_string();
    let (st, _, body) = call(
        &router, Method::POST, &format!("/api/conversations/{cid}/messages"), &h,
        Some(("application/json", r#"{"content":"hola"}"#)),
    ).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(code_of(&body), "rate_limited");
}

#[tokio::test]
async fn s06_chat_permit_is_released_after_stream() {
    let app = TestApp::new().await; // Limits por defecto: 4 chats
    let h = [("host", "127.0.0.1:8795")];
    let (_, _, conv) = call(&app.router, Method::POST, "/api/conversations", &h, None).await;
    let cid = serde_json::from_str::<Value>(&conv).unwrap()["id"].as_str().unwrap().to_string();
    for _ in 0..6 {
        let (st, _, body) = call(
            &app.router, Method::POST, &format!("/api/conversations/{cid}/messages"), &h,
            Some(("application/json", r#"{"content":"hola"}"#)),
        ).await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
    assert_eq!(app.state.limits.chats.available_permits(), 4);
}

#[tokio::test]
async fn s06_analysis_limit_counts_generations_not_requests() {
    use server::fake::{FakeAnalyst, FakeMarket};
    use std::sync::atomic::Ordering;
    let analyst = FakeAnalyst::default();
    *analyst.analyze_delay.lock().unwrap() = std::time::Duration::from_millis(400);
    let app = TestApp::with(FakeMarket::default(), analyst).await;
    let mut state = app.state.clone();
    state.limits = Arc::new(Limits::new(2, 2, 2));
    let router = server::app(state, None);
    let h = [("host", "127.0.0.1:8795")];
    let post = |asset: &'static str| {
        let router = router.clone();
        async move { call(&router, Method::POST, &format!("/api/assets/{asset}/analysis"), &h, None).await }
    };
    // 4 peticiones del mismo activo: una generación, todas 200.
    let rs = tokio::join!(post("stock%3AAAPL"), post("stock%3AAAPL"), post("stock%3AAAPL"), post("stock%3AAAPL"));
    for r in [&rs.0, &rs.1, &rs.2, &rs.3] {
        assert_eq!(r.0, StatusCode::OK, "{}", r.2);
    }
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 1);
    // 3 activos distintos con límite 2: dos generan y la tercera recibe 429.
    let rs = tokio::join!(post("stock%3AAAPL"), post("stock%3AMSFT"), post("stock%3ATSLA"));
    let mut codes = [rs.0.0, rs.1.0, rs.2.0];
    codes.sort();
    assert_eq!(codes, [StatusCode::OK, StatusCode::OK, StatusCode::TOO_MANY_REQUESTS]);
}
