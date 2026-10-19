//! Redes sociales en el servidor: apagado por defecto, rutas, guardado con fecha, métricas y
//! presupuesto de X persistente. Sin red real: proveedores simulados y un servidor X local.

mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{Router, extract::Query, extract::State, http::StatusCode, routing::get};
use chrono::{Duration as Dur, Utc};
use common::TestApp;
use domain::{AssetClass, SocialPost, SocialSource};
use market::{
    http::build_client,
    providers::{PResult, ProviderError, x::X},
    secret::Secret,
    social::{ReadBudget, SocialBatch, SocialConfig, SocialProvider, SocialQuery, XConfig},
};
use server::social::{SocialService, SqliteBudget};
use sqlx::Row;

struct Fake {
    calls: AtomicU32,
    fail: bool,
}

#[async_trait]
impl SocialProvider for Fake {
    fn source(&self) -> SocialSource {
        SocialSource::Reddit
    }
    async fn fetch(&self, _: &SocialQuery) -> PResult<SocialBatch> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(ProviderError::upstream("HTTP 503 con la clave sk-supersecreta"));
        }
        let now = Utc::now();
        let mk = |i: i64, score: i64, text: &str| SocialPost {
            id: format!("reddit:p{i}"),
            source: SocialSource::Reddit,
            community: "stocks".into(),
            author: format!("autor{}", i % 2),
            title: Some("AAPL".into()),
            text: text.into(),
            score,
            comments: 3,
            created_at: now - Dur::hours(i + 1),
            url: format!("https://www.reddit.com/r/stocks/comments/p{i}/x/"),
        };
        Ok(SocialBatch { posts: vec![mk(0, 40, "bullish, will surge"), mk(1, 90, "bearish, will crash"), mk(2, 5, "earnings on Thursday")], requests: 1, ..Default::default() })
    }
}

async fn app_with(provider: Arc<Fake>, refresh_minutes: u64) -> TestApp {
    let mut app = TestApp::new().await;
    let cfg = SocialConfig { refresh_minutes, ..Default::default() };
    let budget = Arc::new(SqliteBudget::new(app.state.db.clone()));
    let svc = SocialService::with_providers(cfg, app.state.db.clone(), vec![provider], budget);
    app.state = app.state.clone().with_social(Some(svc), vec![]);
    app.router = server::app(app.state.clone(), None);
    app
}

#[tokio::test]
async fn off_by_default_routes_report_disabled_and_write_nothing() {
    let app = TestApp::new().await;
    let (st, v) = app.get("/api/social/status").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["enabled"], false);
    let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!((v["enabled"].clone(), v["digest"].clone()), (false.into(), serde_json::Value::Null));
    let n: i64 = sqlx::query("SELECT (SELECT COUNT(*) FROM social_posts) + (SELECT COUNT(*) FROM social_runs) + (SELECT COUNT(*) FROM social_metrics)")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 0);
    // Sin credenciales en el entorno de pruebas: la configuración no crea ningún servicio.
    assert!(SocialService::from_config(SocialConfig::from_lookup(|_| None), app.state.db.clone()).is_none());
}

#[tokio::test]
async fn asset_route_returns_metrics_and_excerpts_and_stores_them_with_dates() {
    let fake = Arc::new(Fake { calls: AtomicU32::new(0), fail: false });
    let app = app_with(fake.clone(), 360).await;
    let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], true);
    let d = &v["digest"];
    assert_eq!(d["metrics"]["mentions"], 3);
    assert_eq!(d["metrics"]["authors"], 2);
    assert_eq!(d["metrics"]["bullish"], 1);
    assert_eq!(d["metrics"]["bearish"], 1);
    assert!(d["metrics"]["ratio"].is_null(), "sin histórico no hay cociente");
    let ex = d["excerpts"].as_array().unwrap();
    assert_eq!(ex[0]["score"], 90, "mejor puntuación primero");
    assert_eq!(ex[0]["stance"], "bearish");
    assert!(ex[0]["url"].as_str().unwrap().starts_with("https://www.reddit.com/"));
    assert!(v["disclaimer"].as_str().unwrap().contains("no verificada"));
    // Guardado con la fecha real de la publicación y una instantánea con su `as_of`.
    let created: i64 = sqlx::query("SELECT MIN(created_at) FROM social_posts").fetch_one(&app.state.db).await.unwrap().get(0);
    assert!(created < Utc::now().timestamp_millis() - 3_000_000 && created > Utc::now().timestamp_millis() - 4 * 3_600_000);
    let snaps: i64 = sqlx::query("SELECT COUNT(*) FROM social_metrics").fetch_one(&app.state.db).await.unwrap().get(0);
    assert_eq!(snaps, 1);
    // Una segunda vista dentro del periodo de refresco no vuelve a consultar la red (ni gasta).
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let snaps: i64 = sqlx::query("SELECT COUNT(*) FROM social_metrics").fetch_one(&app.state.db).await.unwrap().get(0);
    assert_eq!(snaps, 2, "las instantáneas se añaden, nunca se pisan");
}

#[tokio::test]
async fn a_failing_network_never_breaks_the_route_and_is_reported() {
    let fake = Arc::new(Fake { calls: AtomicU32::new(0), fail: true });
    let app = app_with(fake.clone(), 360).await;
    let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(st, StatusCode::OK);
    assert!(v["digest"].is_null());
    // El segundo intento cae en el periodo de espera tras fallo: no martillea a la red.
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let (_, s) = app.get("/api/social/status").await;
    assert_eq!(s["enabled"], true);
    assert!(s["lastError"]["message"].as_str().unwrap().contains("503"), "{s}");
}

// --- presupuesto de X persistente ---------------------------------------------------------------

#[tokio::test]
async fn sqlite_budget_caps_reservations_and_refunds() {
    let pool = server::db::connect_memory().await.unwrap();
    let b = Arc::new(SqliteBudget::new(pool));
    assert_eq!(b.reserve("2026-10", 60, 100).await.unwrap(), 60);
    assert_eq!(b.reserve("2026-10", 60, 100).await.unwrap(), 40);
    assert_eq!(b.reserve("2026-10", 10, 100).await.unwrap(), 0);
    b.settle("2026-10", 40, 10).await;
    assert_eq!(b.used("2026-10").await, 70);
    // Concurrencia: 30 reservas de 10 con 30 de saldo → solo 3 se conceden.
    let granted: Vec<u32> = futures::future::join_all((0..30).map(|_| {
        let b = b.clone();
        async move { b.reserve("2026-10", 10, 100).await.unwrap() }
    }))
    .await;
    assert_eq!(granted.iter().sum::<u32>(), 30);
    assert_eq!(b.used("2026-10").await, 100);
}

async fn x_mock() -> (SocketAddr, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let app = Router::new()
        .route(
            "/2/tweets/search/recent",
            get(|State(c): State<Arc<AtomicU32>>, Query(q): Query<std::collections::HashMap<String, String>>| async move {
                c.fetch_add(1, Ordering::SeqCst);
                let max: usize = q["max_results"].parse().unwrap();
                let ts = (Utc::now() - Dur::hours(1)).format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
                let data: Vec<_> = (0..max)
                    .map(|i| serde_json::json!({"id": format!("19{i:017}"), "text": "$AAPL up", "created_at": ts, "author_id": "1", "public_metrics": {"like_count": 1, "reply_count": 0}}))
                    .collect();
                serde_json::json!({"data": data, "meta": {"result_count": max}}).to_string()
            }),
        )
        .with_state(calls.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, calls)
}

fn x_cfg(addr: SocketAddr, budget: u32) -> XConfig {
    XConfig {
        bearer: Secret::new("x-token-123").unwrap(),
        monthly_read_budget: budget,
        accounts: vec!["una".into()],
        search_all: false,
        langs: vec![],
        results_per_request: 10,
        cost_per_read_usd: 0.005,
        api_base: format!("http://{addr}"),
    }
}

#[tokio::test]
async fn x_budget_survives_a_restart_and_stops_at_the_cap() {
    let (addr, calls) = x_mock().await;
    let dir = std::env::temp_dir().join(format!("invest-ai-social-{}", std::process::id()));
    let url = format!("sqlite://{}/db.sqlite", dir.display());
    let q = |days| SocialQuery { class: AssetClass::Stock, symbol: "AAPL".into(), name: None, days };

    // Primer «arranque»: tope 25 → caben dos peticiones de 10.
    let pool = server::db::connect(&url).await.unwrap();
    let x = X::new(build_client(Duration::from_secs(5)).unwrap(), x_cfg(addr, 25), Arc::new(SqliteBudget::new(pool.clone())));
    assert_eq!(x.fetch(&q(7)).await.unwrap().reads, 10);
    assert_eq!(x.fetch(&q(6)).await.unwrap().reads, 10);
    pool.close().await;

    // Reinicio: otro pool, otro proveedor (sin la caché en memoria) sobre el mismo fichero.
    let pool = server::db::connect(&url).await.unwrap();
    let budget = Arc::new(SqliteBudget::new(pool.clone()));
    let month = Utc::now().format("%Y-%m").to_string();
    assert_eq!(budget.used(&month).await, 20, "el contador sobrevivió al reinicio");
    let x = X::new(build_client(Duration::from_secs(5)).unwrap(), x_cfg(addr, 25), budget.clone());
    let b = x.fetch(&q(5)).await.unwrap();
    assert!(b.budget_exhausted && b.reads == 0, "quedan 5 (< 10): se corta");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "X solo recibió las 2 peticiones previas al reinicio");
    assert_eq!(budget.used(&month).await, 20);

    // La ruta de estado refleja uso, restante y coste estimado.
    let cfg = SocialConfig { x: Some(x_cfg(addr, 25)), ..Default::default() };
    let svc = SocialService::with_providers(cfg, pool.clone(), vec![], budget);
    let s = svc.status().await;
    assert_eq!((s["x"]["used"].clone(), s["x"]["remaining"].clone()), (20.into(), 5.into()));
    assert_eq!(s["x"]["estimatedCostUsd"], 0.1);
    assert_eq!(s["x"]["maxMonthlyCostUsd"], 0.125);
    assert_eq!(s["x"]["exhausted"], true);
    pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}
