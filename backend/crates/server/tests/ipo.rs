//! Rutas `/api/ipo/*` con la OPV simulada (`FakeIpo`): contrato, validaciones y que el perfil de
//! riesgo se calcula con el extractor real sobre el texto de folleto inventado.

mod common;

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_keys};
use serde_json::Value;
use server::{AppState, Settings, db, fake::{FakeAnalyst, FakeMarket}};

fn indicator<'a>(profile: &'a Value, key: &str) -> &'a Value {
    profile["indicators"].as_array().unwrap().iter().find(|i| i["key"] == key).unwrap_or_else(|| panic!("falta {key}"))
}

#[tokio::test]
async fn calendar_defaults_to_a_window_around_today_and_is_sorted() {
    let app = TestApp::new().await;
    let (s, body) = app.get("/api/ipo/calendar").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    assert!(rows.len() >= 6, "{body}");
    assert_keys(
        &rows[0],
        &["date", "symbol", "name", "exchange", "status", "priceLow", "priceHigh", "shares", "totalValue", "spacLikely", "source", "sourceUrl", "asOf"],
    );
    let dates: Vec<&str> = rows.iter().map(|r| r["date"].as_str().unwrap()).collect();
    let mut sorted = dates.clone();
    sorted.sort_unstable();
    assert_eq!(dates, sorted);
    assert!(rows.iter().all(|r| r["source"] == "fake"), "todo marcado como simulado");
    let statuses: Vec<&str> = rows.iter().map(|r| r["status"].as_str().unwrap()).collect();
    for want in ["priced", "expected", "filed", "withdrawn"] {
        assert!(statuses.contains(&want), "{want} en {statuses:?}");
    }
    // Los datos antiguos quedan fuera de la ventana por defecto.
    assert!(rows.iter().all(|r| r["symbol"] != "KLRN"));
}

#[tokio::test]
async fn calendar_validates_dates_and_range() {
    let app = TestApp::new().await;
    for (q, why) in [
        ("from=2026-02-30", "fecha inexistente"),
        ("from=hoy", "no es fecha"),
        ("from=2026-10-10&to=2026-10-01", "from > to"),
        ("from=2026-01-01&to=2026-12-31", "rango > 180 días"),
        ("from=1990-01-01&to=1990-02-01", "demasiado antiguo"),
        ("from=2090-01-01&to=2090-02-01", "demasiado lejano"),
    ] {
        let (s, b) = app.get(&format!("/api/ipo/calendar?{q}")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{why}: {b}");
        assert_eq!(b["error"]["code"], "invalid_input");
    }
    let (s, b) = app.get("/api/ipo/calendar?from=2024-01-01&to=2024-03-01").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, serde_json::json!([]));
}

#[tokio::test]
async fn prospectus_cuts_sections_and_validates_the_symbol() {
    let app = TestApp::new().await;
    let (s, body) = app.get("/api/ipo/ORFS/prospectus?maxChars=500").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "fake");
    assert!(body["sections"].as_array().unwrap().iter().all(|x| x["text"].as_str().unwrap().chars().count() <= 500));
    let (_, full) = app.get("/api/ipo/orfs/prospectus").await;
    let offering = full["sections"].as_array().unwrap().iter().find(|x| x["key"] == "offering").unwrap();
    assert!(offering["text"].as_str().unwrap().contains("offered by the selling stockholders"));
    assert_eq!(offering["truncated"], false);
    for bad in ["A%2FB", "..", "A$B", "THISISWAYTOOLONGASYMBOL"] {
        let (s, b) = app.get(&format!("/api/ipo/{bad}/prospectus")).await;
        assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::NOT_FOUND, "{bad}: {s} {b}");
    }
    let (s, b) = app.get("/api/ipo/ORFS/prospectus?maxChars=3").await;
    assert_eq!((s, b["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_input")));
    let (s, b) = app.get("/api/ipo/NOPE/prospectus").await;
    assert_eq!((s, b["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
    // Presentada sin datos de la oferta: sin folleto utilizable.
    let (s, _) = app.get("/api/ipo/VLPM/prospectus").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn performance_from_the_calendar_date_and_validation() {
    let app = TestApp::new().await;
    let (s, b) = app.get("/api/ipo/ORFS/performance").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["symbol"], "ORFS");
    assert_eq!(b["benchmark"], "SPY");
    assert_eq!(b["points"].as_array().unwrap().len(), 4);
    // A 27 días ningún horizonte se ha cumplido: todo `null`, nunca se extrapola.
    assert!(b["points"].as_array().unwrap().iter().all(|p| p["returnPct"].is_null()), "{b}");
    // GPKS salió hace 75 días: hay punto de 1 mes y aún no de 3.
    let (s, b) = app.get("/api/ipo/GPKS/performance?date=").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "date vacía no es una fecha: {b}");
    let date = (chrono::Utc::now().date_naive() - chrono::Duration::days(75)).to_string();
    let (s, b) = app.get(&format!("/api/ipo/GPKS/performance?date={date}&offerPrice=20")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(b["points"][0]["returnPct"].is_number() && b["points"][1]["returnPct"].is_null(), "{b}");
    let (s, b) = app.get("/api/ipo/ORFS/performance?benchmark=QQQ&offerPrice=24").await;
    assert_eq!((s, b["benchmark"].as_str()), (StatusCode::OK, Some("QQQ")));
    for q in ["benchmark=DIA", "offerPrice=-1", "offerPrice=abc", "date=2020-13-01"] {
        let (s, b) = app.get(&format!("/api/ipo/ORFS/performance?{q}")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{q}: {b}");
    }
    // Una OPV que aún no cotiza no tiene rendimiento.
    let (s, _) = app.get("/api/ipo/CBME/performance").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recent_performance_is_the_base_rate_with_sample_sizes() {
    let app = TestApp::new().await;
    let (s, b) = app.get("/api/ipo/recent-performance?months=12").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(b["sample"].as_u64().unwrap() >= 4, "{b}");
    assert!(b["nDay1"].as_u64().unwrap() >= b["n3m"].as_u64().unwrap());
    assert!(b["medianDay1Pct"].is_number() && b["median3mPct"].is_number());
    assert!(b["caveats"][0].as_str().unwrap().contains("simulados"));
    for q in ["months=0", "months=25", "months=x"] {
        let (s, _) = app.get(&format!("/api/ipo/recent-performance?{q}")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{q}");
    }
}

#[tokio::test]
async fn risk_profile_is_computed_by_the_real_extractor() {
    let app = TestApp::new().await;
    let (s, p) = app.get("/api/ipo/ORFS/risk-profile").await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["symbol"], "ORFS");
    assert_eq!(p["entry"]["status"], "priced");
    assert_eq!(p["prospectus"]["form"], "424B4");
    assert!(p["disclaimer"].as_str().unwrap().contains("no una recomendación"));
    assert_eq!(p["indicators"].as_array().unwrap().len(), 11);
    // 18 de 33 millones los venden accionistas existentes: 54,5 %.
    let sec = indicator(&p, "secondary_sale");
    assert_eq!(sec["status"], "extracted");
    assert!((sec["numeric"].as_f64().unwrap() - 54.5).abs() < 0.1, "{sec}");
    assert_keys(&sec["source"], &["kind", "section", "label", "url"]);
    assert_eq!(sec["source"]["section"], "offering");
    let flags: Vec<&str> = indicator(&p, "use_of_proceeds")["flags"].as_array().unwrap().iter().map(|f| f["code"].as_str().unwrap()).collect();
    assert!(flags.contains(&"debt_repayment") && flags.contains(&"selling_holders"), "{flags:?}");
    assert_eq!(indicator(&p, "lock_up")["numeric"], 180.0);
    assert_eq!(indicator(&p, "voting")["numeric"], 10.0);
    assert_eq!(indicator(&p, "controlled")["numeric"], 71.0);
    assert!(indicator(&p, "offer_size")["value"].as_str().unwrap().contains("M USD"));
    assert!(p["baseRate"]["sample"].as_u64().unwrap() > 0);
    // Folleto preliminar de una OPV sin cotizar: avisos y sin inventar lo que no hay.
    let (s, p) = app.get("/api/ipo/CBME/risk-profile").await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let w = p["warnings"].to_string();
    assert!(w.contains("preliminar") && w.contains("aún no cotiza"), "{w}");
    // Presentada sin folleto utilizable: perfil con lo que haya (calendario) y todo lo demás «no extraído».
    let (s, p) = app.get("/api/ipo/VLPM/risk-profile").await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert!(p["prospectus"].is_null());
    assert_eq!(indicator(&p, "lock_up")["status"], "not_extracted");
    assert!(indicator(&p, "lock_up")["value"].is_null());
    // Ni calendario ni folleto: 404.
    let (s, _) = app.get("/api/ipo/ZZZZ/risk-profile").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn without_ipo_data_configured_the_routes_say_unavailable_without_internals() {
    let pool = db::connect_memory().await.unwrap();
    let state = AppState::new(Arc::new(FakeMarket::default()), Arc::new(FakeAnalyst::default()), pool, Settings::default());
    let router = server::app(state, None);
    let res = tower::ServiceExt::oneshot(
        router,
        axum::http::Request::builder().method(Method::GET).uri("/api/ipo/calendar").body(axum::body::Body::empty()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}
