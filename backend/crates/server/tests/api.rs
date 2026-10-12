//! Tests de integración REST: router real + FakeMarket/FakeAnalyst + SQLite en memoria.

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, assert_keys, mask};
use serde_json::{Value, json};
use server::fake::{FakeAnalyst, FakeMarket};
use std::sync::atomic::Ordering;

fn error_of(body: &Value) -> (&str, &str) {
    assert_keys(body, &["error"]);
    assert_keys(&body["error"], &["code", "message"]);
    (body["error"]["code"].as_str().unwrap(), body["error"]["message"].as_str().unwrap())
}

fn btc_summary() -> Value {
    json!({
        "id": "crypto:BTC", "symbol": "BTC", "name": "Bitcoin", "class": "crypto",
        "exchange": null, "currency": "USD", "country": null, "source": "fake", "sourceUrl": null
    })
}

// ---------------------------------------------------------------------------------------------
// Sistema
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn health_and_status() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }));

    let (status, body) = app.get("/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&body, &["llm", "providers"]);
    assert_eq!(
        body["llm"],
        json!({
            "available": true, "baseUrl": "http://fake.invalid", "model": "fake-model",
            "installedModels": ["fake-model"], "supportsTools": true, "error": null
        })
    );
    assert_keys(&body["providers"][0], &[
        "name", "enabled", "requiresKey", "hasKey", "capabilities", "assetClasses", "lastError", "lastOk",
        "latencyMs", "blockedSecs",
    ]);
}

#[tokio::test]
async fn unknown_routes_and_methods_return_json_errors() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/no-existe").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_of(&body).0, "not_found");

    let (status, body) = app.send(Method::DELETE, "/api/health", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error_of(&body).0, "method_not_allowed");
}

// ---------------------------------------------------------------------------------------------
// Mercado
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn search_returns_summaries_and_validates_params() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/search?q=app").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([{
            "id": "stock:AAPL", "symbol": "AAPL", "name": "Apple Inc.", "class": "stock",
            "exchange": "NASDAQ", "currency": "USD", "country": null, "source": "fake", "sourceUrl": null
        }])
    );

    // Filtro por clase.
    let (_, crypto) = app.get("/api/search?q=crypto&class=crypto").await;
    assert_eq!(crypto.as_array().unwrap().len(), 2);
    let (_, none) = app.get("/api/search?q=crypto&class=stock").await;
    assert_eq!(none, json!([]));
    // `limit` recorta y se acota a 50.
    let (_, limited) = app.get("/api/search?q=e&limit=3").await;
    assert_eq!(limited.as_array().unwrap().len(), 3);
    let (status, _) = app.get("/api/search?q=e&limit=9999").await;
    assert_eq!(status, StatusCode::OK);

    // Errores.
    for uri in ["/api/search", "/api/search?q=", "/api/search?q=%20%20", "/api/search?q=a&class=planeta", "/api/search?q=a&limit=abc"] {
        let (status, body) = app.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(error_of(&body).0, "invalid_input", "{uri}");
    }
    let (status, _) = app.get(&format!("/api/search?q={}", "x".repeat(101))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn asset_detail_decodes_percent_encoded_ids() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/assets/stock%3AAAPL").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&body, &["summary", "quote", "fundamentals", "isFavorite"]);
    assert_eq!(body["summary"]["id"], "stock:AAPL");
    assert_eq!(body["quote"]["price"], 190.0);
    assert_eq!(body["fundamentals"]["sector"], "Technology");
    assert_eq!(body["isFavorite"], false);

    // Con `^` y `:` codificados.
    let (status, body) = app.get("/api/assets/index%3A%5EGSPC/quote").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "index:^GSPC");

    // Un `/` codificado dentro del símbolo sigue siendo un único segmento de ruta.
    let (status, body) = app.get("/api/assets/stock%3ABRK%2FB/quote").await;
    // S-09: el símbolo se valida con lista blanca; `/` no está permitido, así que es 400 (no llega al proveedor).
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (code, _) = error_of(&body);
    assert_eq!(code, "invalid_input");
    // El símbolo real de Yahoo, con guion, sigue siendo válido (404 porque el fake no lo conoce).
    let (status, _) = app.get("/api/assets/stock%3ABRK-B/quote").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Sin codificar también vale (curl, tests...).
    let (status, _) = app.get("/api/assets/crypto:BTC/quote").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn invalid_and_unknown_assets_give_400_and_404() {
    let app = TestApp::new().await;
    for uri in ["/api/assets/BTC", "/api/assets/planeta%3AX", "/api/assets/stock%3A", "/api/assets/BTC/quote", "/api/assets/BTC/analysis"] {
        let (status, body) = app.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(error_of(&body).0, "invalid_input", "{uri}");
    }
    for uri in [
        "/api/assets/stock%3AZZZZ",
        "/api/assets/stock%3AZZZZ/quote",
        "/api/assets/stock%3AZZZZ/history",
        "/api/assets/stock%3AZZZZ/indicators",
        "/api/assets/stock%3AZZZZ/fundamentals",
        "/api/assets/stock%3AZZZZ/news",
        "/api/assets/stock%3AZZZZ/snapshot",
    ] {
        let (status, body) = app.get(uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(error_of(&body).0, "not_found", "{uri}");
    }
    let (status, body) = app.send(Method::POST, "/api/assets/stock%3AZZZZ/analysis", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_of(&body).0, "not_found");
}

#[tokio::test]
async fn quote_and_quotes_json_shape() {
    let app = TestApp::new().await;
    let (_, quote) = app.get("/api/assets/stock%3AAAPL/quote").await;
    assert_keys(&quote, &[
        "id", "price", "change", "changePct", "open", "high", "low", "prevClose", "volume", "marketCap",
        "currency", "marketState", "timestamp", "source", "delaySecs", "sourceUrl", "timestampOrigin",
    ]);
    assert_eq!(quote["id"], "stock:AAPL");
    assert_eq!(quote["price"], 190.0);
    assert_eq!(quote["changePct"], 1.0);
    assert_eq!(quote["marketCap"], Value::Null);
    assert_eq!(quote["marketState"], "regular");
    assert_eq!(quote["timestamp"], "2025-01-02T15:30:00Z");
    assert_eq!(quote["delaySecs"], 0);

    let (status, quotes) = app.get("/api/quotes?ids=crypto%3ABTC,stock%3AAAPL,stock%3AAAPL,stock%3AZZZZ").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = quotes.as_array().unwrap().iter().map(|q| q["id"].as_str().unwrap()).collect();
    // Duplicados fuera; los desconocidos los omite el mercado.
    assert_eq!(ids, ["crypto:BTC", "stock:AAPL"]);
    assert_eq!(quotes[0]["marketState"], "continuous");

    // Lista vacía = `[]`; sin parámetro, 400; demasiados, 400.
    assert_eq!(app.get("/api/quotes?ids=").await.1, json!([]));
    assert_eq!(app.get("/api/quotes").await.0, StatusCode::BAD_REQUEST);
    let many = (0..51).map(|i| format!("stock:T{i}")).collect::<Vec<_>>().join(",");
    let (status, body) = app.get(&format!("/api/quotes?ids={many}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_of(&body).0, "invalid_input");
    assert_eq!(app.get("/api/quotes?ids=BTC").await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_uses_default_range_and_interval() {
    let app = TestApp::new().await;
    let spacing = |candles: &Value| candles[1]["t"].as_i64().unwrap() - candles[0]["t"].as_i64().unwrap();

    let (status, candles) = app.get("/api/assets/stock%3AAAPL/history").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&candles[0], &["t", "o", "h", "l", "c", "v"]);
    assert_eq!(spacing(&candles), 86_400, "1y → velas diarias");
    let (_, candles) = app.get("/api/assets/stock%3AAAPL/history?range=1d").await;
    assert_eq!(spacing(&candles), 300, "1d → 5m");
    let (_, candles) = app.get("/api/assets/stock%3AAAPL/history?range=5d&interval=1h").await;
    assert_eq!(spacing(&candles), 3600);

    for uri in ["/api/assets/stock%3AAAPL/history?range=3d", "/api/assets/stock%3AAAPL/history?interval=2m"] {
        let (status, body) = app.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(error_of(&body).0, "invalid_input");
    }
}

#[tokio::test]
async fn indicators_fundamentals_news_macro() {
    let app = TestApp::new().await;
    let (_, ind) = app.get("/api/assets/stock%3AAAPL/indicators").await;
    assert_eq!(ind["trend"], "up");
    assert_eq!(ind["rsi14"], 55.0);
    assert!(ind.get("lastClose").is_some() && ind.get("macdHist").is_some() && ind.get("high52w").is_some());

    // Fundamentales: objeto para acciones, `null` para el resto.
    let (_, fund) = app.get("/api/assets/stock%3AAAPL/fundamentals").await;
    assert_eq!(fund["peTrailing"], 28.5);
    assert!(fund.get("topHoldings").is_some() && fund.get("extra").is_some());
    let (status, headers, bytes) = app.raw(Method::GET, "/api/assets/crypto%3ABTC/fundamentals", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"null");
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");

    // Noticias: `limit` y sentimiento añadido por el anotador.
    let (_, news) = app.get("/api/assets/stock%3AAAPL/news?limit=2").await;
    assert_eq!(news.as_array().unwrap().len(), 2);
    assert_keys(&news[0], &[
        "id", "title", "summary", "url", "publisher", "publishedAt", "imageUrl", "tickers", "sentiment", "provider", "language",
    ]);
    assert_eq!(news[0]["sentiment"], json!({ "score": 0.5, "label": "positive", "method": "fake" }));
    assert_eq!(news[0]["tickers"], json!(["AAPL"]));
    let (_, all) = app.get("/api/news").await;
    assert_eq!(all.as_array().unwrap().len(), 5);
    assert_eq!(all[0]["tickers"], json!([]));
    assert_eq!(app.get("/api/news?limit=-1").await.0, StatusCode::BAD_REQUEST);

    let (_, macro_ctx) = app.get("/api/macro").await;
    assert_keys(&macro_ctx, &["indicators", "asOf"]);
    assert_keys(&macro_ctx["indicators"][0], &["key", "name", "value", "change", "unit", "asOf", "source", "sourceUrl", "timestampOrigin"]);

    // Fallo del proveedor → 502 con el código del contrato.
    app.market.fail_news.store(true, Ordering::SeqCst);
    let (status, body) = app.get("/api/news").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(error_of(&body).0, "upstream");
    app.market.fail_quotes.store(true, Ordering::SeqCst);
    let (status, body) = app.get("/api/assets/stock%3AAAPL/quote").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_of(&body).0, "unavailable");
}

// ---------------------------------------------------------------------------------------------
// Conversaciones
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn conversation_crud() {
    let app = TestApp::new().await;

    // Crear (con y sin cuerpo).
    let mut conv = app.create_conversation(json!({ "title": "  Mi   análisis ", "assetIds": ["stock:AAPL", "stock:AAPL"] })).await;
    let id = conv["id"].as_str().unwrap().to_string();
    mask(&mut conv, &["id", "createdAt", "updatedAt"]);
    assert_eq!(
        conv,
        json!({
            "id": "<dyn>", "title": "Mi análisis", "assetIds": ["stock:AAPL"], "pinned": false,
            "messageCount": 0, "lastMessagePreview": null, "createdAt": "<dyn>", "updatedAt": "<dyn>"
        })
    );
    let (status, empty) = app.send(Method::POST, "/api/conversations", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["title"], "Nueva conversación");
    assert_eq!(empty["assetIds"], json!([]));
    let empty_id = empty["id"].as_str().unwrap().to_string();

    // Listar: la más reciente primero.
    let (_, list) = app.get("/api/conversations").await;
    let ids: Vec<&str> = list.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [empty_id.as_str(), id.as_str()]);

    // Detalle con mensajes.
    let (status, detail) = app.get(&format!("/api/conversations/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&detail, &["conversation", "messages"]);
    assert_eq!(detail["conversation"]["id"], id.as_str());
    assert_eq!(detail["messages"], json!([]));

    // PATCH parcial y completo.
    let (status, patched) = app.send(Method::PATCH, &format!("/api/conversations/{id}"), Some(json!({ "pinned": true }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((patched["pinned"].as_bool(), patched["title"].as_str()), (Some(true), Some("Mi análisis")));
    let body = json!({ "title": "Otro título", "assetIds": ["crypto:BTC", "crypto:BTC", "stock:AAPL"], "pinned": false });
    let (_, patched) = app.send(Method::PATCH, &format!("/api/conversations/{id}"), Some(body)).await;
    assert_eq!(patched["title"], "Otro título");
    assert_eq!(patched["assetIds"], json!(["crypto:BTC", "stock:AAPL"]));
    assert_eq!(patched["pinned"], false);
    let (status, same) = app.send(Method::PATCH, &format!("/api/conversations/{id}"), Some(json!({}))).await;
    assert_eq!((status, &same["title"]), (StatusCode::OK, &json!("Otro título")));

    // Borrar.
    let (status, body) = app.send(Method::DELETE, &format!("/api/conversations/{id}"), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    let (status, body) = app.get(&format!("/api/conversations/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_of(&body).0, "not_found");
    assert_eq!(app.send(Method::DELETE, &format!("/api/conversations/{id}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.send(Method::PATCH, &format!("/api/conversations/{id}"), Some(json!({}))).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn conversation_validation_errors() {
    let app = TestApp::new().await;
    let conv = app.create_conversation(json!({})).await;
    let uri = format!("/api/conversations/{}", conv["id"].as_str().unwrap());

    let too_many: Vec<String> = (0..21).map(|i| format!("stock:T{i}")).collect();
    let cases: Vec<(Method, String, Value)> = vec![
        (Method::POST, "/api/conversations".into(), json!({ "assetIds": ["BTC"] })),
        (Method::POST, "/api/conversations".into(), json!({ "assetIds": "crypto:BTC" })),
        (Method::POST, "/api/conversations".into(), json!({ "title": "x".repeat(201) })),
        (Method::POST, "/api/conversations".into(), json!({ "assetIds": too_many })),
        (Method::PATCH, uri.clone(), json!({ "assetIds": ["planeta:X"] })),
        (Method::PATCH, uri.clone(), json!({ "pinned": "sí" })),
    ];
    for (method, uri, body) in cases {
        let (status, resp) = app.send(method.clone(), &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} {body}");
        assert_eq!(error_of(&resp).0, "invalid_input");
    }

    // JSON mal formado y Content-Type incorrecto.
    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri(&uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from("{no es json"))
        .unwrap();
    let res = app.call(req).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri(&uri)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    assert_eq!(app.call(req).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn oversized_bodies_are_rejected_with_json_413() {
    let app = TestApp::new().await;
    let conv = app.create_conversation(json!({})).await;
    let big = json!({ "title": "x".repeat(300 * 1024) });
    let (status, body) = app
        .send(Method::PATCH, &format!("/api/conversations/{}", conv["id"].as_str().unwrap()), Some(big.clone()))
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_of(&body).0, "payload_too_large");
    // Los extractores de cuerpo opcional también respetan el límite.
    let (status, body) = app.send(Method::POST, "/api/conversations", Some(big)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_of(&body).0, "payload_too_large");
}

// ---------------------------------------------------------------------------------------------
// Favoritos
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn favorites_lifecycle() {
    let app = TestApp::new().await;
    assert_eq!(app.get("/api/favorites").await.1, json!([]));

    // Alta sin cuerpo: el resumen sale del mercado.
    let (status, mut fav) = app.send(Method::PUT, "/api/favorites/crypto%3ABTC", None).await;
    assert_eq!(status, StatusCode::OK);
    mask(&mut fav, &["addedAt"]);
    assert_eq!(fav, json!({ "id": "crypto:BTC", "summary": btc_summary(), "note": null, "addedAt": "<dyn>" }));

    // Con nota; repetir sin cuerpo conserva la nota y la fecha de alta.
    let (_, with_note) = app.send(Method::PUT, "/api/favorites/crypto%3ABTC", Some(json!({ "note": "  largo plazo " }))).await;
    assert_eq!(with_note["note"], "largo plazo");
    let (_, again) = app.send(Method::PUT, "/api/favorites/crypto%3ABTC", None).await;
    assert_eq!(again["note"], "largo plazo");
    assert_eq!(again["addedAt"], with_note["addedAt"]);
    // `null` o vacío borran la nota.
    let (_, cleared) = app.send(Method::PUT, "/api/favorites/crypto%3ABTC", Some(json!({ "note": null }))).await;
    assert_eq!(cleared["note"], Value::Null);

    // Aparece en la lista y en la ficha del activo.
    app.send(Method::PUT, "/api/favorites/stock%3AAAPL", None).await;
    let (_, list) = app.get("/api/favorites").await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["id"], "stock:AAPL", "el último añadido primero");
    assert_eq!(app.get("/api/assets/stock%3AAAPL").await.1["isFavorite"], true);

    // Baja idempotente.
    for _ in 0..2 {
        let (status, body) = app.send(Method::DELETE, "/api/favorites/crypto%3ABTC", None).await;
        assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    }
    assert_eq!(app.get("/api/favorites").await.1.as_array().unwrap().len(), 1);

    // Errores.
    assert_eq!(app.send(Method::PUT, "/api/favorites/BTC", None).await.0, StatusCode::BAD_REQUEST);
    let (status, _) = app.send(Method::PUT, "/api/favorites/stock%3AZZZZ", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "un activo inexistente no se puede marcar");
    let (status, _) = app.send(Method::PUT, "/api/favorites/stock%3AAAPL", Some(json!({ "note": 5 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn favorites_survive_a_provider_outage_with_a_minimal_summary() {
    let app = TestApp::new().await;
    // Con el proveedor caído, la búsqueda para resolver el nombre falla igual que todo: se
    // usa un resumen mínimo en lugar de impedir marcar el favorito. (El fake solo falla en
    // quotes/news, así que para forzarlo se usa un activo que ni siquiera está en el catálogo
    // y un snapshot imposible → NotFound; aquí se comprueba el caso de red caída con la caché.)
    app.send(Method::PUT, "/api/favorites/stock%3AAAPL", None).await;
    app.market.fail_quotes.store(true, Ordering::SeqCst);
    let (_, list) = app.get("/api/favorites").await;
    assert_eq!(list[0]["summary"]["name"], "Apple Inc.", "el listado no depende del proveedor");
}

// ---------------------------------------------------------------------------------------------
// Historial y más vistos
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn search_history_dedupes_typing_and_records_selection() {
    let app = TestApp::new().await;
    // Mientras se teclea, cada pulsación reemplaza a la anterior.
    for q in ["a", "ap", "app"] {
        app.get(&format!("/api/search?q={q}")).await;
    }
    let (status, mut history) = app.get("/api/history/searches").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 1);
    mask(&mut history[0], &["at"]);
    assert_eq!(history[0], json!({ "query": "app", "selected": null, "at": "<dyn>" }));

    // Otra búsqueda distinta y la elección del usuario.
    app.get("/api/search?q=micro").await;
    let (status, body) = app
        .post("/api/history/searches", json!({ "query": "micro", "selected": "stock:MSFT" }))
        .await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    let (_, history) = app.get("/api/history/searches").await;
    let rows: Vec<(&str, &Value)> =
        history.as_array().unwrap().iter().map(|e| (e["query"].as_str().unwrap(), &e["selected"])).collect();
    assert_eq!(rows, [("micro", &json!("stock:MSFT")), ("app", &Value::Null)]);

    // Una selección sin búsqueda previa crea su propia entrada.
    app.post("/api/history/searches", json!({ "query": "tesla", "selected": "stock:TSLA" })).await;
    assert_eq!(app.get("/api/history/searches").await.1[0]["query"], "tesla");

    // Errores de validación.
    for body in [json!({ "query": "x" }), json!({ "query": "  ", "selected": "stock:AAPL" }), json!({ "query": "x", "selected": "AAPL" })] {
        assert_eq!(app.post("/api/history/searches", body).await.0, StatusCode::BAD_REQUEST);
    }

    // Borrar todo.
    let (status, body) = app.send(Method::DELETE, "/api/history/searches", None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(app.get("/api/history/searches").await.1, json!([]));
}

#[tokio::test]
async fn views_are_counted_and_ranked_by_recency() {
    let app = TestApp::new().await;
    for _ in 0..3 {
        app.get("/api/assets/stock%3AAAPL").await;
    }
    app.get("/api/assets/crypto%3ABTC").await;
    // Una consulta que falla (404) no cuenta como visita.
    app.get("/api/assets/stock%3AZZZZ").await;

    let (status, mut views) = app.get("/api/history/views").await;
    assert_eq!(status, StatusCode::OK);
    let rows = views.as_array_mut().unwrap();
    assert_eq!(rows.len(), 2);
    mask(&mut rows[0], &["lastViewedAt"]);
    assert_eq!(rows[0]["views"], 3);
    assert_eq!(rows[0]["id"], "stock:AAPL");
    assert_keys(&rows[0], &["id", "summary", "views", "lastViewedAt"]);
    assert_eq!(rows[0]["summary"]["name"], "Apple Inc.");
    assert_eq!(rows[1]["id"], "crypto:BTC");

    // Ponderación por recencia: muchas visitas antiguas pesan menos que una reciente.
    let now = chrono::Utc::now().timestamp_millis();
    let day = 24 * 3600 * 1000;
    let summary = |sym: &str| json!({
        "id": format!("stock:{sym}"), "symbol": sym, "name": sym, "class": "stock",
        "exchange": null, "currency": null, "country": null, "source": "fake", "sourceUrl": null
    }).to_string();
    for (sym, views, score, last) in [("OLD", 50, 50.0, now - 60 * day), ("NEW", 1, 1.0, now - 3_600_000)] {
        sqlx::query("INSERT INTO asset_views (asset_id, views, score, first_viewed_at, last_viewed_at, summary) VALUES (?1, ?2, ?3, ?4, ?4, ?5)")
            .bind(format!("stock:{sym}"))
            .bind(views)
            .bind(score)
            .bind(last)
            .bind(summary(sym))
            .execute(&app.state.db)
            .await
            .unwrap();
    }
    let (_, views) = app.get("/api/history/views").await;
    let order: Vec<&str> = views.as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(order, ["stock:AAPL", "crypto:BTC", "stock:NEW", "stock:OLD"]);
}

// ---------------------------------------------------------------------------------------------
// Análisis
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn analysis_generate_store_and_list() {
    let app = TestApp::new().await;
    let (status, none) = app.get("/api/assets/stock%3AAAPL/analysis").await;
    assert_eq!((status, none), (StatusCode::OK, Value::Null));
    assert_eq!(app.get("/api/assets/stock%3AAAPL/analyses").await.1, json!([]));

    let (status, analysis) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&analysis, &[
        "id", "asset", "verdict", "confidence", "horizon", "scores", "summary", "thesis", "bullCase", "bearCase",
        "risks", "catalysts", "keyLevels", "strategy", "dataWarnings", "sources", "priceAtAnalysis", "model",
        "generatedAt", "levelsNote", "scenarios", "expectedReturnPct", "wouldChangeIf", "horizonNote",
        "positionSizing", "method", "qualityNotes", "depth", "durationMs", "llmCalls", "promptTokens", "completionTokens",
    ]);
    assert_keys(&analysis["scores"], &[
        "overall", "technical", "momentum", "fundamental", "valuation", "sentiment", "macroScore", "risk",
    ]);
    assert_keys(&analysis["keyLevels"], &["supports", "resistances", "entryZone", "stopLoss", "targets"]);
    assert_keys(&analysis["sources"][0], &["title", "url", "provider", "publishedAt"]);
    assert_eq!(analysis["asset"], "stock:AAPL");
    assert_eq!(analysis["verdict"], "accumulate");
    assert_eq!(analysis["horizon"], "medium");
    assert_eq!(analysis["priceAtAnalysis"], 190.0);

    // El último guardado es ese mismo.
    let (_, latest) = app.get("/api/assets/stock%3AAAPL/analysis").await;
    assert_eq!(latest, analysis);

    // Un segundo análisis pasa a ser el último; el histórico va del más reciente al más antiguo.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (_, second) = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_ne!(second["id"], analysis["id"]);
    let (_, list) = app.get("/api/assets/stock%3AAAPL/analyses").await;
    let ids: Vec<&Value> = list.as_array().unwrap().iter().map(|a| &a["id"]).collect();
    assert_eq!(ids, [&second["id"], &analysis["id"]]);
    assert_eq!(app.get("/api/assets/stock%3AAAPL/analysis").await.1["id"], second["id"]);
    // Otro activo no se mezcla.
    assert_eq!(app.get("/api/assets/crypto%3ABTC/analysis").await.1, Value::Null);
}

#[tokio::test]
async fn concurrent_analysis_requests_share_one_generation() {
    let analyst = FakeAnalyst::default();
    *analyst.analyze_delay.lock().unwrap() = Duration::from_millis(300);
    let app = TestApp::with(FakeMarket::default(), analyst).await;

    let post = || app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None);
    let (a, b, c) = tokio::join!(post(), post(), post());
    for (status, _) in [&a, &b, &c] {
        assert_eq!(*status, StatusCode::OK);
    }
    assert_eq!(a.1["id"], b.1["id"]);
    assert_eq!(a.1["id"], c.1["id"]);
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.market.snapshot_calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.get("/api/assets/stock%3AAAPL/analyses").await.1.as_array().unwrap().len(), 1);

    // Terminada la generación, la siguiente petición lanza una nueva.
    app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 2);

    // Activos distintos no se bloquean entre sí.
    let post_other = app.send(Method::POST, "/api/assets/crypto%3ABTC/analysis", None);
    let post_aapl = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None);
    let (x, y) = tokio::join!(post_other, post_aapl);
    assert_ne!(x.1["id"], y.1["id"]);
}

#[tokio::test]
async fn analysis_generation_survives_a_dropped_client() {
    let analyst = FakeAnalyst::default();
    *analyst.analyze_delay.lock().unwrap() = Duration::from_millis(150);
    let app = TestApp::with(FakeMarket::default(), analyst).await;

    // El cliente se va (la futura de la petición se cancela) antes de que termine el análisis.
    let request = app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None);
    assert!(tokio::time::timeout(Duration::from_millis(30), request).await.is_err());
    let saved = common::wait_for(|| async {
        let (_, v) = app.get("/api/assets/stock%3AAAPL/analysis").await;
        (!v.is_null()).then_some(v)
    })
    .await;
    assert_eq!(saved["asset"], "stock:AAPL");
}

#[tokio::test]
async fn snapshot_shows_the_llm_context() {
    let app = TestApp::new().await;
    let (status, snap) = app.get("/api/assets/stock%3AAAPL/snapshot").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&snap, &[
        "summary", "quote", "fundamentals", "indicators", "news", "macroContext", "missing", "generatedAt",
    ]);
    assert_eq!(snap["news"][0]["sentiment"]["method"], "fake");
}

// ---------------------------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------------------------

async fn seed(app: &TestApp) {
    app.send(Method::PUT, "/api/favorites/crypto%3ABTC", None).await;
    for _ in 0..3 {
        app.get("/api/assets/stock%3AAAPL").await;
    }
    app.get("/api/assets/stock%3AMSFT").await;
    app.get("/api/search?q=micro").await;
    app.create_conversation(json!({ "title": "Charla" })).await;
    app.send(Method::POST, "/api/assets/stock%3AAAPL/analysis", None).await;
}

#[tokio::test]
async fn dashboard_aggregates_everything() {
    let app = TestApp::new().await;
    // Vacío: estructura completa igualmente.
    let (status, empty) = app.get("/api/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["favorites"], json!([]));
    assert_eq!(empty["market"].as_array().unwrap().len(), 11);

    seed(&app).await;
    let (status, dash) = app.get("/api/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert_keys(&dash, &[
        "favorites", "mostViewed", "recentSearches", "recentConversations", "latestAnalyses", "market", "news",
    ]);

    assert_keys(&dash["favorites"][0], &["favorite", "quote"]);
    assert_eq!(dash["favorites"][0]["favorite"]["id"], "crypto:BTC");
    assert_eq!(dash["favorites"][0]["quote"]["id"], "crypto:BTC");

    assert_keys(&dash["mostViewed"][0], &["stat", "quote"]);
    let viewed: Vec<(&str, u64)> = dash["mostViewed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["stat"]["id"].as_str().unwrap(), v["stat"]["views"].as_u64().unwrap()))
        .collect();
    assert_eq!(viewed, [("stock:AAPL", 3), ("stock:MSFT", 1)]);
    assert_eq!(dash["mostViewed"][0]["quote"]["price"], 190.0);

    assert_eq!(dash["recentSearches"][0]["query"], "micro");
    assert_eq!(dash["recentConversations"][0]["title"], "Charla");
    assert_eq!(dash["latestAnalyses"].as_array().unwrap().len(), 1);
    assert_eq!(dash["latestAnalyses"][0]["asset"], "stock:AAPL");

    // Panel fijo, en su orden.
    let panel: Vec<&str> = dash["market"].as_array().unwrap().iter().map(|q| q["id"].as_str().unwrap()).collect();
    assert_eq!(panel, [
        "index:^GSPC", "index:^IXIC", "index:^STOXX50E", "index:^IBEX", "crypto:BTC", "crypto:ETH",
        "commodity:GC=F", "commodity:CL=F", "forex:EURUSD=X", "bond:^TNX", "index:^VIX",
    ]);
    assert_eq!(dash["news"].as_array().unwrap().len(), 5);
    assert_eq!(dash["news"][0]["sentiment"]["method"], "fake");
}

#[tokio::test]
async fn dashboard_tolerates_partial_failures() {
    let app = TestApp::new().await;
    seed(&app).await;
    app.market.fail_quotes.store(true, Ordering::SeqCst);
    app.market.fail_news.store(true, Ordering::SeqCst);

    let (status, dash) = app.get("/api/dashboard").await;
    assert_eq!(status, StatusCode::OK, "{dash}");
    // Lo que sale de la BD sigue ahí; las cotizaciones y titulares faltan.
    assert_eq!(dash["favorites"][0]["favorite"]["id"], "crypto:BTC");
    assert_eq!(dash["favorites"][0]["quote"], Value::Null);
    assert_eq!(dash["mostViewed"][0]["quote"], Value::Null);
    assert_eq!(dash["market"], json!([]));
    assert_eq!(dash["news"], json!([]));
    assert_eq!(dash["recentConversations"][0]["title"], "Charla");
    assert_eq!(dash["latestAnalyses"].as_array().unwrap().len(), 1);

    // Solo las noticias caídas: el resto con datos.
    app.market.fail_quotes.store(false, Ordering::SeqCst);
    let (_, dash) = app.get("/api/dashboard").await;
    assert_eq!(dash["market"].as_array().unwrap().len(), 11);
    assert_eq!(dash["news"], json!([]));
}

// ---------------------------------------------------------------------------------------------
// Capas: compresión y CORS
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn responses_are_compressed_on_request() {
    let app = TestApp::new().await;
    let req = |encoding: &str| {
        axum::http::Request::builder()
            .uri("/api/dashboard")
            .header(header::ACCEPT_ENCODING, encoding)
            .body(axum::body::Body::empty())
            .unwrap()
    };
    for encoding in ["gzip", "br"] {
        let res = app.call(req(encoding)).await;
        assert_eq!(res.headers()[header::CONTENT_ENCODING], encoding);
    }
    assert!(app.call(req("identity")).await.headers().get(header::CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn cors_allows_only_the_frontend_origin() {
    let app = TestApp::new().await;
    let preflight = |origin: &str| {
        axum::http::Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/conversations")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let res = app.call(preflight("http://localhost:5180")).await;
    assert_eq!(res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5180");
    assert!(res.headers()[header::ACCESS_CONTROL_ALLOW_METHODS].to_str().unwrap().contains("PATCH"));
    let res = app.call(preflight("https://evil.example")).await;
    assert!(res.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
}
