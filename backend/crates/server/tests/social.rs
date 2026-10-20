//! Redes sociales en el servidor: apagado por defecto, rutas, guardado con fecha, métricas y
//! presupuesto de X persistente. Sin red real: proveedores simulados y un servidor X local.

mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
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

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Ok,
    Fail,
    /// Responde bien pero sin presupuesto: `budget_exhausted` y sin publicaciones.
    Exhausted,
}

struct Fake {
    calls: AtomicU32,
    mode: Mutex<Mode>,
    source: SocialSource,
}

impl Fake {
    fn new(source: SocialSource, mode: Mode) -> Arc<Self> {
        Arc::new(Self { calls: AtomicU32::new(0), mode: Mutex::new(mode), source })
    }
    fn reddit(mode: Mode) -> Arc<Self> {
        Self::new(SocialSource::Reddit, mode)
    }
    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SocialProvider for Fake {
    fn source(&self) -> SocialSource {
        self.source
    }
    async fn fetch(&self, _: &SocialQuery) -> PResult<SocialBatch> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mode = *self.mode.lock().unwrap();
        if mode == Mode::Fail {
            return Err(ProviderError::upstream("HTTP 503 con la clave sk-supersecreta"));
        }
        if mode == Mode::Exhausted {
            return Ok(SocialBatch { budget_exhausted: true, ..Default::default() });
        }
        let now = Utc::now();
        let prefix = self.source.as_str();
        let source = self.source;
        let mk = |i: i64, score: i64, text: &str| SocialPost {
            id: format!("{prefix}:p{i}"),
            source,
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
    app_with_providers(vec![provider], refresh_minutes, None).await
}

/// `x_cap`: tope de activos distintos por hora en X (`None` = el de por defecto).
async fn app_with_providers(providers: Vec<Arc<Fake>>, refresh_minutes: u64, x_cap: Option<u32>) -> TestApp {
    let mut app = TestApp::new().await;
    let cfg = SocialConfig { refresh_minutes, ..Default::default() };
    let budget = Arc::new(SqliteBudget::new(app.state.db.clone()));
    let providers: Vec<Arc<dyn SocialProvider>> = providers.into_iter().map(|p| p as Arc<dyn SocialProvider>).collect();
    let svc = match x_cap {
        Some(cap) => SocialService::with_providers_and_x_cap(cfg, app.state.db.clone(), providers, budget, cap),
        None => SocialService::with_providers(cfg, app.state.db.clone(), providers, budget),
    };
    app.state = app.state.clone().with_social(Some(svc), vec![]);
    app.router = server::app(app.state.clone(), None);
    app
}

async fn count(app: &TestApp, table: &str) -> i64 {
    sqlx::query(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(&app.state.db).await.unwrap().get(0)
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
    let fake = Fake::reddit(Mode::Ok);
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
    // Una segunda vista dentro del periodo de refresco no vuelve a consultar la red (ni gasta) y
    // tampoco anota otra instantánea: no hay datos nuevos y duplicarla sesgaría a `learning` (B3).
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(fake.calls(), 1);
    assert_eq!(count(&app, "social_metrics").await, 1, "una lectura de lo guardado no es una observación nueva");
}

#[tokio::test]
async fn metrics_snapshot_needs_a_real_refresh_and_is_at_most_hourly() {
    let fake = Fake::reddit(Mode::Ok);
    // `refresh_minutes: 0`: todas las vistas refrescan, así se prueba el tope horario por separado.
    let app = app_with(fake.clone(), 0).await;
    app.get("/api/assets/stock:AAPL/social").await;
    app.get("/api/assets/stock:AAPL/social").await;
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(fake.calls(), 3, "cada vista refrescó");
    assert_eq!(count(&app, "social_metrics").await, 1, "como mucho una instantánea por activo y hora");
    // Otro activo tiene la suya.
    app.get("/api/assets/stock:MSFT/social").await;
    assert_eq!(count(&app, "social_metrics").await, 2);
    // Pasada una hora, la siguiente observación sí se anota (las instantáneas se añaden, nunca se pisan).
    sqlx::query("UPDATE social_metrics SET as_of = as_of - 7200000").execute(&app.state.db).await.unwrap();
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(count(&app, "social_metrics").await, 3);
    let n: i64 = sqlx::query("SELECT COUNT(*) FROM social_metrics WHERE asset_id = 'stock:AAPL'").fetch_one(&app.state.db).await.unwrap().get(0);
    assert_eq!(n, 2);
}

#[tokio::test]
async fn a_failed_or_exhausted_refresh_records_no_snapshot() {
    let fake = Fake::reddit(Mode::Ok);
    let app = app_with(fake.clone(), 0).await;
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(count(&app, "social_metrics").await, 1);
    sqlx::query("UPDATE social_metrics SET as_of = as_of - 7200000").execute(&app.state.db).await.unwrap();
    // Con lo guardado de antes y el refresco fallido, el digest se sirve pero no es una observación nueva.
    fake.set(Mode::Fail);
    let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["digest"]["metrics"]["mentions"], 3, "se sirve lo guardado");
    assert_eq!(count(&app, "social_metrics").await, 1);
    // Y con el presupuesto agotado (otro activo con datos previos) tampoco.
    let fake_x = Fake::new(SocialSource::X, Mode::Ok);
    let app = app_with(fake_x.clone(), 0).await;
    app.get("/api/assets/stock:AAPL/social").await;
    sqlx::query("UPDATE social_metrics SET as_of = as_of - 7200000").execute(&app.state.db).await.unwrap();
    fake_x.set(Mode::Exhausted);
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(count(&app, "social_metrics").await, 1);
}

#[tokio::test]
async fn exhausted_budget_has_a_cooldown_and_does_not_write_a_run_per_get() {
    let fake = Fake::new(SocialSource::X, Mode::Exhausted);
    let app = app_with(fake.clone(), 360).await;
    for _ in 0..5 {
        let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(v["digest"].is_null());
    }
    assert_eq!(fake.calls(), 1, "con el presupuesto agotado no se vuelve a intentar durante la espera");
    assert_eq!(count(&app, "social_runs").await, 1, "una fila, no una por GET");
    assert_eq!(count(&app, "social_metrics").await, 0);
    // Otro activo no hereda la espera.
    app.get("/api/assets/stock:MSFT/social").await;
    assert_eq!(fake.calls(), 2);
}

#[tokio::test]
async fn retention_purges_old_runs_and_metrics_but_keeps_recent_ones() {
    let day: i64 = 86_400_000;
    let now = Utc::now().timestamp_millis();
    let seed = |app: &TestApp| {
        let db = app.state.db.clone();
        async move {
            for (asset, age_days) in [("stock:OLDRUN", 50), ("stock:NEWRUN", 20)] {
                sqlx::query("INSERT INTO social_runs (asset_id, source, fetched_at, window_start, ok) VALUES (?1, 'reddit', ?2, ?2, 1)")
                    .bind(asset)
                    .bind(now - age_days * day)
                    .execute(&db)
                    .await
                    .unwrap();
            }
            for (asset, age_days) in [("stock:OLDMET", 400), ("stock:NEWMET", 200)] {
                sqlx::query(
                    "INSERT INTO social_metrics (asset_id, as_of, window_days, mentions, authors, bullish, bearish, neutral, mentions_window, bullish_window, bearish_window, baseline_days, truncated, by_source)
                     VALUES (?1, ?2, 7, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, '[]')",
                )
                .bind(asset)
                .bind(now - age_days * day)
                .execute(&db)
                .await
                .unwrap();
            }
        }
    };
    let survivors = |app: &TestApp| {
        let db = app.state.db.clone();
        async move {
            let runs: Vec<String> = sqlx::query("SELECT asset_id FROM social_runs WHERE asset_id LIKE 'stock:%RUN' ORDER BY asset_id")
                .fetch_all(&db)
                .await
                .unwrap()
                .iter()
                .map(|r| r.get(0))
                .collect();
            let mets: Vec<String> = sqlx::query("SELECT asset_id FROM social_metrics WHERE asset_id LIKE 'stock:%MET' ORDER BY asset_id")
                .fetch_all(&db)
                .await
                .unwrap()
                .iter()
                .map(|r| r.get(0))
                .collect();
            (runs, mets)
        }
    };

    // Tras un refresco correcto.
    let app = app_with(Fake::reddit(Mode::Ok), 360).await;
    seed(&app).await;
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(survivors(&app).await, (vec!["stock:NEWRUN".to_string()], vec!["stock:NEWMET".to_string()]));

    // Y también cuando solo hay fallos (`store` no corre): si no, la tabla crecería sin límite.
    let app = app_with(Fake::reddit(Mode::Fail), 360).await;
    seed(&app).await;
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(survivors(&app).await, (vec!["stock:NEWRUN".to_string()], vec!["stock:NEWMET".to_string()]));
}

// --- M1: segunda barrera de X (activos distintos por hora) y control de origen --------------------

#[test]
fn x_asset_cap_setting_is_parsed_safely() {
    use server::social::{DEFAULT_X_MAX_ASSETS_PER_HOUR as D, parse_x_max_assets_per_hour as parse};
    assert_eq!(D, 30);
    for unset in [None, Some(""), Some("  "), Some("abc"), Some("0"), Some("-3"), Some("1.5")] {
        assert_eq!(parse(unset), D, "{unset:?}");
    }
    assert_eq!(parse(Some("5")), 5);
    assert_eq!(parse(Some(" 12 ")), 12);
    assert_eq!(parse(Some("99999")), 1000, "con techo");
}

#[tokio::test]
async fn x_is_queried_for_a_limited_number_of_distinct_assets_per_hour() {
    let reddit = Fake::reddit(Mode::Ok);
    let x = Fake::new(SocialSource::X, Mode::Ok);
    // Sin caché de refresco (0 min) para que repetir un activo vuelva a salir a las redes.
    let app = app_with_providers(vec![reddit.clone(), x.clone()], 0, Some(2)).await;
    let ok = |v: &serde_json::Value| v["enabled"] == true;
    for id in ["AAPL", "MSFT", "TSLA"] {
        let (st, v) = app.get(&format!("/api/assets/stock:{id}/social")).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(ok(&v));
    }
    assert_eq!(x.calls(), 2, "el tercer activo distinto no llega a X");
    assert_eq!(reddit.calls(), 3, "Reddit no tiene ese tope");
    // Un activo ya contado en la hora sigue pudiendo consultarse.
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(x.calls(), 3);
    // El rechazado no escribió nada de X: ni ejecuciones ni publicaciones.
    let n: i64 = sqlx::query("SELECT COUNT(*) FROM social_runs WHERE asset_id = 'stock:TSLA' AND source = 'x'").fetch_one(&app.state.db).await.unwrap().get(0);
    assert_eq!(n, 0);
    // Lo de Reddit de TSLA sí se sirve.
    let (_, v) = app.get("/api/assets/stock:TSLA/social").await;
    assert_eq!(v["digest"]["metrics"]["mentions"], 3);
    assert_eq!(x.calls(), 3, "TSLA sigue sin hueco en X");
}

async fn get_with(app: &TestApp, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut b = axum::http::Request::builder().method("GET").uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    app.call(b.body(axum::body::Body::empty()).unwrap()).await.status()
}

#[tokio::test]
async fn cross_origin_gets_never_reach_the_networks() {
    let fake = Fake::reddit(Mode::Ok);
    let app = app_with(fake.clone(), 0).await;
    let uri = "/api/assets/stock:AAPL/social";
    let host = ("host", "127.0.0.1:8787");
    // `<img src>` desde otra web: Sec-Fetch-Site cross-site (sin Origin) o con Origin ajeno.
    for h in [
        vec![host, ("sec-fetch-site", "cross-site")],
        vec![host, ("sec-fetch-site", "cross-site"), ("sec-fetch-mode", "no-cors"), ("sec-fetch-dest", "image")],
        vec![host, ("origin", "https://evil.example")],
        vec![host, ("origin", "http://localhost:9999")],
    ] {
        assert_eq!(get_with(&app, uri, &h).await, StatusCode::FORBIDDEN, "{h:?}");
    }
    assert_eq!(fake.calls(), 0, "ninguna consulta a las redes");
    assert_eq!(count(&app, "social_runs").await + count(&app, "social_posts").await + count(&app, "social_metrics").await, 0);
    // Legítimos: sin cabeceras, mismo origen, Vite (FRONTEND_ORIGIN), navegación directa del usuario.
    for h in [
        vec![host],
        vec![host, ("sec-fetch-site", "same-origin")],
        vec![host, ("sec-fetch-site", "none")],
        vec![host, ("origin", "http://127.0.0.1:8787"), ("sec-fetch-site", "same-origin")],
        vec![("host", "localhost:5180"), ("sec-fetch-site", "same-origin")],
        vec![host, ("origin", "http://localhost:5180")],
    ] {
        assert_eq!(get_with(&app, uri, &h).await, StatusCode::OK, "{h:?}");
    }
    assert_eq!(fake.calls(), 6, "cada petición legítima sí consulta");
}

#[tokio::test]
async fn a_failing_network_never_breaks_the_route_and_is_reported() {
    let fake = Fake::reddit(Mode::Fail);
    let app = app_with(fake.clone(), 360).await;
    let (st, v) = app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(st, StatusCode::OK);
    assert!(v["digest"].is_null());
    // El segundo intento cae en el periodo de espera tras fallo: no martillea a la red.
    app.get("/api/assets/stock:AAPL/social").await;
    assert_eq!(fake.calls(), 1);
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
