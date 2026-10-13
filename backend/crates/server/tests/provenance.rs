//! Procedencia del histórico en cabeceras y nuevos campos de `/status`.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use domain::{AssetClass, AssetId, Interval, Range};
use market::provenance::{HistoryOrigin, record_history};

#[tokio::test]
async fn history_exposes_provider_headers_when_known() {
    let app = TestApp::new().await;
    let id = AssetId::new(AssetClass::Stock, "AAPL");
    record_history(
        &id,
        Range::Y1,
        Interval::D1,
        HistoryOrigin {
            source: "yahoo".into(),
            source_url: Some("https://finance.yahoo.com/quote/AAPL".into()),
            as_of: chrono::DateTime::from_timestamp(1_700_000_000, 0),
            interval: Some(Interval::M5),
        },
    );
    let (status, headers, _) = app.raw(Method::GET, "/api/assets/stock%3AAAPL/history", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-data-source"], "yahoo");
    assert_eq!(headers["x-data-source-url"], "https://finance.yahoo.com/quote/AAPL");
    assert_eq!(headers["x-data-as-of"], "2023-11-14T22:13:20Z");
    // Intervalo realmente servido (aquí el proveedor subió de 1d a 5m: solo para el test).
    assert_eq!(headers["x-data-interval"], "5m");
}

#[tokio::test]
async fn history_without_known_origin_has_no_provenance_headers() {
    let app = TestApp::new().await;
    let (status, headers, _) = app.raw(Method::GET, "/api/assets/stock%3AMSFT/history", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("x-data-source").is_none());
}
