//! Lista de seguimiento: programador con reloj controlable y analista simulado (sin red ni GPU).

mod common;

use std::{sync::{Arc, atomic::Ordering}, time::Duration};

use axum::http::{Method, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use common::{TestApp, wait_for};
use serde_json::{Value, json};
use server::{
    AppState, Settings,
    fake::{FakeAnalyst, FakeMarket, annotate_fake},
    repo::watchlist as wrepo,
    watchlist::{
        Clock, DEFAULT_WATCHLIST, ManualClock, ManualPower, PowerProbe, RetryPolicy, SystemPower, WatchlistConfig, next_due, run_scheduler,
        window::window_of,
    },
};

/// Zona horaria de las pruebas: UTC+2 (Madrid en verano). 23:00 local = 21:00 UTC.
const OFFSET: i32 = 2 * 3600;

fn utc(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
}

fn cfg(list: &str) -> WatchlistConfig {
    let mut c = WatchlistConfig::build(true, list, "23:00", "standard").unwrap();
    c.retry = RetryPolicy {
        attempts: 3,
        backoff: vec![Duration::from_secs(60), Duration::from_secs(300)],
        abort_after: 2,
        round_cooldown: Duration::from_secs(1800),
        max_rounds: 4,
    };
    c
}

async fn start(app: &TestApp, cfg: WatchlistConfig, clock: Arc<ManualClock>) -> (AppState, tokio::task::JoinHandle<()>) {
    start_with_power(app, cfg, clock, Arc::new(ManualPower::default())).await
}

/// Por defecto, energía desconocida: la prueba no depende de si este equipo va con batería.
async fn start_with_power(
    app: &TestApp,
    cfg: WatchlistConfig,
    clock: Arc<ManualClock>,
    power: Arc<dyn PowerProbe>,
) -> (AppState, tokio::task::JoinHandle<()>) {
    let state = app.state.clone().with_watchlist(cfg, clock.clone()).with_power(power);
    let handle = tokio::spawn(run_scheduler(state.clone(), state.watchlist.clone(), clock));
    (state, handle)
}

/// Apaga y espera al programador con tope: un bucle desbocado hace fallar la prueba en vez de colgarla.
async fn stop(state: &AppState, handle: tokio::task::JoinHandle<()>) {
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("el programador termina al apagar").unwrap();
}

async fn runs(app: &TestApp) -> Vec<Value> {
    let (_, v) = app.get("/api/watchlist").await;
    v["recentRuns"].as_array().unwrap().iter().rev().cloned().collect()
}

async fn origins(app: &TestApp) -> Vec<(String, String)> {
    sqlx::query_as("SELECT asset_id, origin FROM analyses ORDER BY generated_at, id")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn runs_at_the_set_time_one_asset_at_a_time_and_saves_with_origin_watchlist() {
    let app = TestApp::new().await;
    // 20:00 UTC = 22:00 local; la pasada toca a las 21:00 UTC. El reloj salta solo hasta las 22:00 UTC:
    // la pasada de mañana (23:00 local del 16) queda fuera.
    let clock = ManualClock::new(utc(15, 20, 0), OFFSET);
    clock.auto_advance_until(utc(15, 22, 0));
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT, crypto:ETH"), clock.clone()).await;

    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let runs = runs(&app).await;
    assert_eq!(runs.len(), 1, "una sola pasada ese día: {runs:?}");
    let run = &runs[0];
    assert_eq!((run["trigger"].as_str(), run["ok"].as_i64(), run["failed"].as_i64()), (Some("schedule"), Some(3), Some(0)));
    assert_eq!(run["depth"], "standard");
    assert_eq!(run["localDate"], "2026-10-15");
    assert_eq!(run["scheduledFor"], "2026-10-15T21:00:00.000Z");
    assert_eq!(run["startedAt"], "2026-10-15T21:00:00.000Z");

    assert_eq!(
        origins(&app).await,
        [("stock:AAPL", "watchlist"), ("stock:MSFT", "watchlist"), ("crypto:ETH", "watchlist")]
            .map(|(a, o)| (a.to_string(), o.to_string()))
    );
    let started: Vec<String> = app.analyst.started.lock().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(started, ["stock:AAPL", "stock:MSFT", "crypto:ETH"], "en el orden de la lista");
    assert_eq!(app.analyst.max_running.load(Ordering::SeqCst), 1, "nunca dos análisis a la vez");
    assert!(app.analyst.seen_depths.lock().unwrap().iter().all(|d| *d == Some(domain::AnalysisDepth::Standard)));

    // Mañana: no ha vuelto a ejecutar.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 3, "ni un análisis más");
    assert_eq!(runs.len(), 1);
    stop(&state, handle).await;
}

#[tokio::test]
async fn waits_for_the_preferred_time_inside_the_window_and_runs_when_the_clock_gets_there() {
    let app = TestApp::new().await;
    // 20:10 UTC = 22:10 locales: EE. UU. cerró a las 22:00 y la ventana está abierta, pero la hora
    // preferida son las 23:00 locales (21:00 UTC).
    let clock = ManualClock::new(utc(15, 20, 10), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runs(&app).await.is_empty());
    clock.set(utc(15, 20, 59)); // 22:59 local
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(runs(&app).await.is_empty());
    clock.set(utc(15, 21, 0));
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!((r["trigger"].as_str(), r["localDate"].as_str()), (Some("schedule"), Some("2026-10-15")));
    stop(&state, handle).await;
}

#[tokio::test]
async fn a_preferred_time_outside_the_window_is_adjusted_to_just_after_the_close() {
    let app = TestApp::new().await;
    let mut c = cfg("stock:AAPL");
    c.time = chrono::NaiveTime::from_hms_opt(16, 0, 0).unwrap(); // 16:00 locales = en plena sesión de EE. UU.
    let clock = ManualClock::new(utc(15, 20, 10), OFFSET); // 22:10 locales; cierre 22:00
    let (state, handle) = start(&app, c, clock.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runs(&app).await.is_empty(), "no antes del margen de 30 min tras el cierre");
    clock.set(utc(15, 20, 30));
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!(r["scheduledFor"], "2026-10-15T20:30:00.000Z", "cierre + 30 min");
    assert_eq!((r["trigger"].as_str(), r["localDate"].as_str()), (Some("schedule"), Some("2026-10-15")));
    stop(&state, handle).await;
}

#[tokio::test]
async fn recovers_a_missed_run_on_startup_and_records_the_real_time_but_only_once() {
    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(15, 21, 45), OFFSET); // 23:45 local: la pasada de 23:00 ya pasó
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!(r["trigger"], "recovery");
    assert_eq!(r["scheduledFor"], "2026-10-15T21:00:00.000Z", "la hora prevista");
    assert_eq!(r["startedAt"], "2026-10-15T21:45:00.000Z", "la hora real");
    stop(&state, handle).await;

    // Reinicio del servidor ese mismo día: ya está hecho, no repite.
    let again = AppState::new(app.market.clone(), app.analyst.clone(), app.state.db.clone(), Settings::default())
        .with_news_annotator(annotate_fake)
        .with_power(Arc::new(ManualPower::default()))
        .with_watchlist(cfg("stock:AAPL, stock:MSFT"), clock.clone());
    let handle = tokio::spawn(run_scheduler(again.clone(), again.watchlist.clone(), clock.clone()));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(runs(&app).await.len(), 1, "no se repite el mismo día");
    stop(&again, handle).await;
}

#[tokio::test]
async fn a_failing_asset_is_recorded_and_does_not_stop_the_list() {
    let app = TestApp::new().await;
    app.analyst.fail_symbols.lock().unwrap().insert("stock:MSFT".into(), (u32::MAX, "not_found".into()));
    let clock = ManualClock::new(utc(15, 21, 5), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT, crypto:ETH"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;

    let (_, w) = app.get("/api/watchlist").await;
    let results = w["lastRunResults"].as_array().unwrap();
    let by = |id: &str| results.iter().find(|r| r["assetId"] == id).unwrap().clone();
    assert_eq!(by("stock:AAPL")["status"], "ok");
    assert_eq!(by("crypto:ETH")["status"], "ok", "la lista siguió tras el fallo");
    let bad = by("stock:MSFT");
    assert_eq!((bad["status"].as_str(), bad["transient"].as_bool(), bad["attempts"].as_i64()), (Some("failed"), Some(false), Some(1)));
    assert_eq!(bad["errorCode"], "not_found", "el fallo no se oculta");
    assert!(bad["error"].as_str().unwrap().contains("fallo simulado"));
    assert_eq!((w["lastRun"]["ok"].as_i64(), w["lastRun"]["failed"].as_i64()), (Some(2), Some(1)));
    // Un fallo permanente no se reintenta ese día.
    assert_eq!(w["recentRuns"].as_array().unwrap().len(), 1);
    let msft = w["assets"].as_array().unwrap().iter().find(|a| a["id"] == "stock:MSFT").unwrap();
    assert_eq!(msft["last"]["status"], "failed");
    assert!(msft["lastOkAt"].is_null());
    stop(&state, handle).await;
}

#[tokio::test]
async fn transient_failures_are_retried_with_backoff() {
    let app = TestApp::new().await;
    app.analyst.fail_symbols.lock().unwrap().insert("stock:AAPL".into(), (2, "unavailable".into()));
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let (_, w) = app.get("/api/watchlist").await;
    let r = &w["lastRunResults"][0];
    assert_eq!((r["status"].as_str(), r["attempts"].as_i64()), (Some("ok"), Some(3)));
    // 60 s + 300 s de espera entre intentos, medidos con el reloj controlado.
    let (s, f) = (r["startedAt"].as_str().unwrap(), r["finishedAt"].as_str().unwrap());
    let secs = (f.parse::<DateTime<Utc>>().unwrap() - s.parse::<DateTime<Utc>>().unwrap()).num_seconds();
    assert_eq!(secs, 360);
    stop(&state, handle).await;
}

#[tokio::test]
async fn aborts_when_the_model_is_down_and_finishes_the_list_in_a_later_round() {
    let app = TestApp::new().await;
    // Con 2 intentos por activo, los 2 primeros activos agotan 4 generaciones fallidas -> se aborta.
    app.analyst.fail_first_calls.store(4, Ordering::SeqCst);
    let mut c = cfg("stock:AAPL, stock:MSFT, stock:TSLA, crypto:ETH");
    c.retry.attempts = 2;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let (state, handle) = start(&app, c, clock.clone()).await;
    wait_for(|| async { (runs(&app).await.len() == 2 && runs(&app).await[1]["status"] == "completed").then_some(()) }).await;

    let runs = runs(&app).await;
    assert_eq!((runs[0]["status"].as_str(), runs[0]["trigger"].as_str()), (Some("aborted"), Some("schedule")));
    assert_eq!((runs[0]["ok"].as_i64(), runs[0]["failed"].as_i64()), (Some(0), Some(4)));
    assert_eq!((runs[1]["trigger"].as_str(), runs[1]["ok"].as_i64()), (Some("retry"), Some(4)));
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT asset_id, status FROM watchlist_results WHERE run_id = ?1 ORDER BY id")
        .bind(runs[0]["id"].as_i64().unwrap())
        .fetch_all(&state.db)
        .await
        .unwrap();
    let statuses: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(statuses, ["failed", "failed", "skipped", "skipped"], "lo no intentado se anota como skipped, no se oculta");
    // 30 minutos entre rondas.
    let d = |v: &Value| v.as_str().unwrap().parse::<DateTime<Utc>>().unwrap();
    assert!(d(&runs[1]["startedAt"]) - d(&runs[0]["finishedAt"]) >= chrono::Duration::minutes(30));
    stop(&state, handle).await;
}

#[tokio::test]
async fn shutdown_stops_the_scheduler_mid_wait_and_an_unfinished_run_is_marked_interrupted() {
    let app = TestApp::new().await;
    *app.analyst.analyze_delay.lock().unwrap() = Duration::from_secs(30);
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock.clone()).await;
    wait_for(|| async { (!runs(&app).await.is_empty()).then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["status"], "running");
    let (_, w) = app.get("/api/watchlist").await;
    assert_eq!(w["running"], true);
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("el programador termina al apagar").unwrap();
    assert_eq!(runs(&app).await[0]["status"], "interrupted");
}

#[tokio::test]
async fn a_stale_running_run_from_a_previous_process_is_marked_interrupted_on_startup() {
    let app = TestApp::new().await;
    // Empezada dentro de la ventana de D=14 (con `started_at = 0` la reconciliación vería actividad en 1970 y cerraría 10 ventanas).
    sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('2026-10-14', 0, ?1, 'schedule', 'running', 'standard', 3)")
        .bind(utc(14, 21, 0).timestamp_millis())
        .execute(&app.state.db)
        .await
        .unwrap();
    let clock = ManualClock::new(utc(15, 8, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { (runs(&app).await[0]["status"] == "interrupted").then_some(()) }).await;
    stop(&state, handle).await;
}

#[test]
fn next_due_is_today_before_the_time_and_tomorrow_after() {
    let clock = ManualClock::new(utc(15, 0, 0), OFFSET);
    let t = chrono::NaiveTime::from_hms_opt(23, 0, 0).unwrap();
    assert_eq!(next_due(clock.as_ref(), utc(15, 12, 0), t), utc(15, 21, 0));
    assert_eq!(next_due(clock.as_ref(), utc(15, 21, 0), t), utc(16, 21, 0));
    // 23:30 UTC ya es el día 16 en local (01:30): la próxima es la del 16.
    assert_eq!(next_due(clock.as_ref(), utc(15, 23, 30), t), utc(16, 21, 0));
}

#[test]
fn config_parsing_and_defaults() {
    let d = WatchlistConfig::build(true, "", "23:00", "standard").unwrap();
    assert!(d.default_list);
    assert_eq!(d.assets.len(), 20);
    // La lista por defecto solo contiene lo que `learning` sabe resolver: nada de sufijos de bolsa extranjera.
    for a in &d.assets {
        assert!(learning::rules::benchmark_for(a).is_some(), "{a} no se resolvería");
        assert!(!a.symbol.contains('.'), "{a}");
    }
    let c = WatchlistConfig::build(true, "stock:AAPL, stock:AAPL ,crypto:BTC", "07:05", "deep").unwrap();
    assert_eq!(c.assets.len(), 2, "sin duplicados");
    assert!(!c.default_list);
    for bad in [("AAPL", "23:00", "standard"), ("stock:AAPL", "25:00", "standard"), ("stock:AAPL", "23:00", "enorme")] {
        assert!(WatchlistConfig::build(true, bad.0, bad.1, bad.2).is_err(), "{bad:?}");
    }
}

#[tokio::test]
async fn watchlist_endpoint_when_disabled_and_empty() {
    let app = TestApp::new().await;
    let (status, w) = app.get("/api/watchlist").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w["enabled"], false);
    assert!(w["nextRun"].is_null());
    assert!(w["lastRun"].is_null());
    assert_eq!(w["lastRunResults"], json!([]));
    assert_eq!(w["analysesTotal"], 0);
}

#[tokio::test]
async fn learning_routes_work_empty_and_rollback_is_guarded() {
    let app = TestApp::new().await;
    let (status, p) = app.get("/api/learning/performance").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["analysesTotal"], 0);
    assert_eq!(p["horizons"].as_array().unwrap().len(), 3);
    assert!(p["caveats"].as_array().unwrap().len() >= 3);
    assert_eq!(p["horizons"][1]["resolved"], 0);

    let (_, w) = app.get("/api/learning/weights").await;
    assert_eq!(w, json!({ "active": null, "versions": [] }));

    // Volver atrás a una versión inexistente -> 404; a los pesos por defecto -> crea una versión 'rollback'.
    let (status, e) = app.post("/api/learning/weights/rollback", json!({ "version": 7 })).await;
    assert_eq!((status, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
    let (status, v) = app.post("/api/learning/weights/rollback", json!({ "version": null })).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["kind"].as_str(), v["status"].as_str()), (Some("rollback"), Some("active")));
    let (_, w) = app.get("/api/learning/weights").await;
    assert_eq!(w["versions"].as_array().unwrap().len(), 1);
    assert_eq!(w["active"]["version"], v["version"]);
    ai::scoring::set_learned_multipliers(None); // el rollback instala estado global del proceso

    // Cuerpo no válido y Origin ajeno (guard de seguridad existente).
    let (status, _) = app.post("/api/learning/weights/rollback", json!({ "version": "tres" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/learning/weights/rollback")
        .header("content-type", "application/json")
        .header("origin", "https://evil.example")
        .body(axum::body::Body::from("{\"version\":null}"))
        .unwrap();
    assert_eq!(app.call(req).await.status(), StatusCode::FORBIDDEN);
    let (_, w) = app.get("/api/learning/weights").await;
    assert_eq!(w["versions"].as_array().unwrap().len(), 1, "la petición bloqueada no hizo nada");

    // Confianza calibrada: sin datos suficientes -> sin calibrar, con el motivo.
    let (status, c) = app.get("/api/learning/calibrated-confidence?confidence=0.7").await;
    assert_eq!(status, StatusCode::OK);
    assert!(c["calibrated"].is_null());
    assert_eq!((c["raw"].as_f64(), c["horizonWeeks"].as_i64(), c["n"].as_i64()), (Some(0.7), Some(4), Some(0)));
    assert!(!c["reason"].as_str().unwrap().is_empty());
    assert_eq!(app.get("/api/learning/calibrated-confidence?confidence=1.5").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.get("/api/learning/calibrated-confidence?confidence=0.5&horizon=3").await.0, StatusCode::BAD_REQUEST);
    // Con `model`, se calibra con ese modelo (sin historial suyo: n = 0); vacío, no vale.
    let (status, c) = app.get("/api/learning/calibrated-confidence?confidence=0.7&model=otro-modelo%3A7b").await;
    assert_eq!(status, StatusCode::OK);
    assert!(c["calibrated"].is_null());
    assert_eq!((c["raw"].as_f64(), c["n"].as_i64()), (Some(0.7), Some(0)));
    assert_eq!(app.get("/api/learning/calibrated-confidence?confidence=0.7&model=").await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn learning_loop_runs_a_cycle_and_stops_on_shutdown() {
    let app = TestApp::with(FakeMarket::default(), FakeAnalyst::default()).await;
    let state = app.state.clone();
    let handle = tokio::spawn(server::watchlist::run_learning_loop(state.clone(), Duration::from_millis(30), Duration::from_millis(1)));
    tokio::time::sleep(Duration::from_millis(150)).await;
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle).await.expect("termina al apagar").unwrap();
}

// --- Ollama simulado: el modelo falta, aparece o desaparece ------------------------------------

use async_trait::async_trait;
use domain::{
    Analysis, AnalysisDepth, AnalysisEvent, Analyst, AssetSnapshot, ChatEvent, ChatRequest, DomainError, HardwareInfo,
    LlmModel, LlmStatus, ModelService, ModelsResponse, PullEvent,
};
use futures::{StreamExt, stream::BoxStream};

const MODEL: &str = "qwen3.5:9b";

/// `ModelService` de pruebas: el modelo activo puede estar instalado o no y Ollama puede no responder.
struct SimOllama {
    installed: std::sync::atomic::AtomicBool,
    reachable: std::sync::atomic::AtomicBool,
    /// Con `true`, `list` no responde (comprobación previa lenta) hasta que vuelva a `false`.
    slow: std::sync::atomic::AtomicBool,
    list_calls: std::sync::atomic::AtomicUsize,
}

impl SimOllama {
    fn new(installed: bool) -> Arc<Self> {
        Arc::new(Self { installed: installed.into(), reachable: true.into(), slow: false.into(), list_calls: 0.into() })
    }

    fn set_installed(&self, v: bool) {
        self.installed.store(v, Ordering::SeqCst);
    }

    fn is_installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelService for SimOllama {
    async fn list(&self) -> ModelsResponse {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        while self.slow.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let reachable = self.reachable.load(Ordering::SeqCst);
        let model = LlmModel {
            id: MODEL.into(), name: MODEL.into(), family: "qwen3.5".into(), parameters: "9B".into(), active_parameters: None,
            quantization: "Q4_K_M".into(), context_window: 32768, tier: None, supports_tools: true, reasoning: true,
            description: String::new(), requirements: None, curated: true, installed: self.is_installed(), active: true,
            force_gpu: false, gpu_fallback: None,
        };
        ModelsResponse {
            active: MODEL.into(),
            ollama_reachable: reachable,
            // Como el real: sin Ollama no hay lista de instalados.
            models: if reachable { vec![model] } else { Vec::new() },
            hardware: HardwareInfo { cpu_model: "CPU simulada".into(), cpu_threads: 8, ram_total_gb: 16.0, ram_available_gb: 8.0, gpus: Vec::new(), disk_free_gb: None },
            loaded: Vec::new(),
            num_gpu: None,
        }
    }

    async fn set_active(&self, _id: &str) -> domain::Result<()> {
        Err(DomainError::Internal("no se usa en estas pruebas".into()))
    }

    async fn pull(&self, _id: &str) -> domain::Result<BoxStream<'static, PullEvent>> {
        Err(DomainError::Internal("no se usa en estas pruebas".into()))
    }

    async fn delete(&self, _id: &str) -> domain::Result<()> {
        Err(DomainError::Internal("no se usa en estas pruebas".into()))
    }

    async fn set_force_gpu(&self, _id: &str, _force: bool) -> domain::Result<()> {
        Err(DomainError::Internal("no se usa en estas pruebas".into()))
    }
}

/// Analista que falla como el real cuando Ollama no tiene el modelo (el mismo texto que quedó en el
/// registro del 20-oct-2026) y, si no, delega en el simulado.
struct GatedAnalyst {
    inner: Arc<FakeAnalyst>,
    ollama: Arc<SimOllama>,
    /// Falla con un `NotFound` de texto desconocido Y hace desaparecer el modelo (para comprobar que
    /// el programador contrasta con el estado real de Ollama).
    odd_not_found_and_lose_model: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl Analyst for GatedAnalyst {
    async fn analyze(&self, snapshot: &AssetSnapshot) -> domain::Result<Analysis> {
        self.inner.analyze(snapshot).await
    }

    fn chat(&self, req: ChatRequest) -> BoxStream<'static, ChatEvent> {
        self.inner.chat(req)
    }

    async fn status(&self) -> LlmStatus {
        self.inner.status().await
    }

    fn analyze_stream(self: Arc<Self>, snapshot: AssetSnapshot, depth: Option<AnalysisDepth>) -> BoxStream<'static, AnalysisEvent> {
        let fail = |message: String| futures::stream::once(async move { AnalysisEvent::Error { code: "not_found".into(), message } }).boxed();
        if self.odd_not_found_and_lose_model.load(Ordering::SeqCst) {
            self.ollama.set_installed(false);
            return fail("no encontrado: 404 de origen desconocido".into());
        }
        if !self.ollama.is_installed() {
            return fail(format!("no encontrado: no encontrado: modelo «{MODEL}»: model '{MODEL}' not found"));
        }
        self.inner.clone().analyze_stream(snapshot, depth)
    }
}

async fn start_sim(
    app: &TestApp,
    cfg: WatchlistConfig,
    clock: Arc<ManualClock>,
    ollama: &Arc<SimOllama>,
) -> (AppState, tokio::task::JoinHandle<()>, Arc<GatedAnalyst>) {
    let analyst = Arc::new(GatedAnalyst {
        inner: Arc::new(FakeAnalyst::default()),
        ollama: ollama.clone(),
        odd_not_found_and_lose_model: false.into(),
    });
    let state = AppState::new(app.market.clone(), analyst.clone(), app.state.db.clone(), Settings::default())
        .with_news_annotator(annotate_fake)
        .with_power(Arc::new(ManualPower::default()))
        .with_models(ollama.clone())
        .with_watchlist(cfg, clock.clone());
    let handle = tokio::spawn(run_scheduler(state.clone(), state.watchlist.clone(), clock));
    (state, handle, analyst)
}

/// (estado, transitorio, código de error) de todos los resultados de una pasada, en orden.
async fn result_rows(app: &TestApp, run_id: i64) -> Vec<(String, bool, Option<String>)> {
    sqlx::query_as("SELECT status, transient, error_code FROM watchlist_results WHERE run_id = ?1 ORDER BY id")
        .bind(run_id)
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

fn run_id(run: &Value) -> i64 {
    run["id"].as_i64().unwrap()
}

#[tokio::test]
async fn a_missing_model_aborts_the_run_before_it_starts_and_marks_nothing_as_permanent() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(false); // Ollama responde, pero sin el modelo activo
    let mut c = cfg("stock:AAPL, stock:MSFT");
    c.retry.max_rounds = 2;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let (state, handle, analyst) = start_sim(&app, c.clone(), clock.clone(), &ollama).await;
    wait_for(|| async { (runs(&app).await.len() == 2).then_some(()) }).await;

    let rs = runs(&app).await;
    assert_eq!((rs[0]["status"].as_str(), rs[0]["trigger"].as_str()), (Some("aborted"), Some("schedule")));
    assert_eq!((rs[1]["status"].as_str(), rs[1]["trigger"].as_str()), (Some("aborted"), Some("retry")), "reintenta con las rondas de siempre");
    let d = |v: &Value| v.as_str().unwrap().parse::<DateTime<Utc>>().unwrap();
    assert!(d(&rs[1]["startedAt"]) - d(&rs[0]["finishedAt"]) >= chrono::Duration::minutes(30));
    assert_eq!((rs[0]["ok"].as_i64(), rs[0]["planned"].as_i64()), (Some(0), Some(2)));
    for r in &rs {
        // Sin generar nada: ni un solo análisis, y todo `skipped` y transitorio con el motivo a la vista.
        let rows = result_rows(&app, run_id(r)).await;
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(s, t, c)| s == "skipped" && *t && c.as_deref() == Some("llm_unavailable")), "{rows:?}");
    }
    let (_, w) = app.get("/api/watchlist").await;
    let first = &w["lastRunResults"][0];
    assert!(first["error"].as_str().unwrap().contains(&format!("«{MODEL}»")), "{first}");
    assert!(first["error"].as_str().unwrap().contains("no está instalado"), "{first}");
    let permanent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_results WHERE transient = 0").fetch_one(&state.db).await.unwrap();
    assert_eq!(permanent, 0, "ningún activo queda como fallo permanente");
    assert_eq!(analyst.inner.analyze_calls.load(Ordering::SeqCst), 0, "no se lanzó ningún análisis");
    assert!(origins(&app).await.is_empty());
    stop(&state, handle).await;

    // Se agotaron las rondas de hoy, pero los activos siguen pendientes: al volver el modelo y
    // reiniciar el servicio ese mismo día se recuperan.
    ollama.set_installed(true);
    let (again, handle, analyst) = start_sim(&app, c, clock.clone(), &ollama).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let last = runs(&app).await.pop().unwrap();
    assert_eq!((last["trigger"].as_str(), last["ok"].as_i64(), last["planned"].as_i64()), (Some("recovery"), Some(2), Some(2)));
    let started: Vec<String> = analyst.inner.started.lock().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(started, ["stock:AAPL", "stock:MSFT"]);
    stop(&again, handle).await;
}

#[tokio::test]
async fn a_model_that_appears_in_the_second_round_completes_the_run() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(false);
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET); // sin avance automático: lo mueve la prueba
    let (state, handle, analyst) = start_sim(&app, cfg("stock:AAPL, stock:MSFT, crypto:ETH"), clock.clone(), &ollama).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "aborted").then_some(()) }).await;

    ollama.set_installed(true); // se arregla el Ollama; la siguiente ronda es 30 min después
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(runs(&app).await.len(), 1, "espera la pausa entre rondas");
    clock.set(utc(15, 21, 30));
    wait_for(|| async { (runs(&app).await.len() == 2 && runs(&app).await[1]["status"] == "completed").then_some(()) }).await;

    let rs = runs(&app).await;
    assert_eq!((rs[0]["status"].as_str(), rs[0]["ok"].as_i64()), (Some("aborted"), Some(0)));
    assert_eq!((rs[1]["trigger"].as_str(), rs[1]["ok"].as_i64(), rs[1]["planned"].as_i64()), (Some("retry"), Some(3), Some(3)));
    assert_eq!(rs[1]["model"], "fake-model");
    let started: Vec<String> = analyst.inner.started.lock().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(started, ["stock:AAPL", "stock:MSFT", "crypto:ETH"]);
    assert_eq!(origins(&app).await.len(), 3);
    // Sin pasadas de más: ya está todo hecho.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(runs(&app).await.len(), 2);
    stop(&state, handle).await;
}

#[tokio::test]
async fn a_non_existent_asset_stays_permanent_even_with_the_llm_healthy() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(true);
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    // ZZZZ no existe en el mercado: el `NotFound` sale de los datos del activo, no del modelo.
    let (state, handle, analyst) = start_sim(&app, cfg("stock:AAPL, stock:ZZZZ, crypto:ETH"), clock.clone(), &ollama).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;

    let (_, w) = app.get("/api/watchlist").await;
    let results = w["lastRunResults"].as_array().unwrap();
    let bad = results.iter().find(|r| r["assetId"] == "stock:ZZZZ").unwrap();
    assert_eq!((bad["status"].as_str(), bad["transient"].as_bool(), bad["attempts"].as_i64()), (Some("failed"), Some(false), Some(1)), "{bad}");
    assert_eq!(bad["errorCode"], "not_found");
    assert!(!bad["error"].as_str().unwrap().contains("modelo «"), "{bad}");
    assert_eq!(results.iter().filter(|r| r["status"] == "ok").count(), 2, "la lista siguió");
    assert_eq!((w["lastRun"]["ok"].as_i64(), w["lastRun"]["failed"].as_i64()), (Some(2), Some(1)));
    // Un fallo permanente no vuelve a intentarse ese día: una sola pasada, sin rondas de reintento.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(runs(&app).await.len(), 1);
    assert_eq!(analyst.inner.analyze_calls.load(Ordering::SeqCst), 2, "ZZZZ no llegó al analista");
    stop(&state, handle).await;
}

#[tokio::test]
async fn an_unrecognised_not_found_is_transient_when_ollama_has_lost_the_model() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(true); // la comprobación previa pasa
    let mut c = cfg("stock:AAPL, stock:MSFT, crypto:ETH");
    c.retry.attempts = 1;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    let (state, handle, analyst) = start_sim(&app, c, clock.clone(), &ollama).await;
    analyst.odd_not_found_and_lose_model.store(true, Ordering::SeqCst);
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "aborted").then_some(()) }).await;

    // Con el texto de un activo inexistente pero el modelo ya ausente, se clasifica por el estado real:
    // dos transitorios seguidos abortan la pasada y el tercero queda sin intentar.
    let first = runs(&app).await.remove(0);
    let rows = result_rows(&app, run_id(&first)).await;
    let codes: Vec<(&str, bool)> = rows.iter().map(|(s, t, _)| (s.as_str(), *t)).collect();
    assert_eq!(codes, [("failed", true), ("failed", true), ("skipped", true)], "{rows:?}");
    assert_eq!(rows[0].2.as_deref(), Some("not_found"), "el error original no se oculta");

    // Vuelve el modelo: la siguiente ronda hace la lista entera.
    ollama.set_installed(true);
    analyst.odd_not_found_and_lose_model.store(false, Ordering::SeqCst);
    clock.set(utc(15, 21, 31));
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let done = runs(&app).await.pop().unwrap();
    assert_eq!((done["trigger"].as_str(), done["ok"].as_i64()), (Some("retry"), Some(3)));
    stop(&state, handle).await;
}

#[tokio::test]
async fn old_model_failures_stored_as_permanent_become_pending_again_but_real_ones_do_not() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(true);
    // Lo que dejó la pasada del 20-oct-2026: fallos `not_found` del MODELO guardados con transient = 0,
    // un fallo de verdad de un activo y un acierto.
    let at = utc(15, 21, 0).timestamp_millis();
    sqlx::query("INSERT INTO watchlist_runs (id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, planned, ok, failed) VALUES (1, '2026-10-15', ?1, ?1, ?1, 'recovery', 'completed', 'standard', 4, 1, 3)")
        .bind(at)
        .execute(&app.state.db)
        .await
        .unwrap();
    let old = "no encontrado: no encontrado: no encontrado: modelo «qwen3.5:9b»: model 'qwen3.5:9b' not found";
    for (asset, status, error) in [
        ("stock:AAPL", "failed", Some(old)),
        ("stock:MSFT", "failed", Some("no encontrado: activo desconocido: stock:MSFT")),
        ("stock:NVDA", "ok", None),
        ("stock:TSLA", "failed", Some(old)),
    ] {
        sqlx::query("INSERT INTO watchlist_results (run_id, local_date, asset_id, status, transient, attempts, error_code, error, started_at, finished_at) VALUES (1, '2026-10-15', ?1, ?2, 0, 1, ?3, ?4, ?5, ?5)")
            .bind(asset)
            .bind(status)
            .bind(error.map(|_| "not_found"))
            .bind(error)
            .bind(at)
            .execute(&app.state.db)
            .await
            .unwrap();
    }
    // Reinicio del servicio a las 23:45 locales del mismo día, con el modelo ya instalado.
    let clock = ManualClock::new(utc(15, 21, 45), OFFSET);
    let (state, handle, analyst) = start_sim(&app, cfg("stock:AAPL, stock:MSFT, stock:NVDA, stock:TSLA"), clock, &ollama).await;
    // La pasada sembrada (id 1) ya está `completed`: se espera a la que lanza esta prueba (id > 1).
    let last = wait_for(|| async { runs(&app).await.into_iter().find(|r| run_id(r) > 1 && r["status"] == "completed") }).await;
    assert_eq!((last["trigger"].as_str(), last["planned"].as_i64(), last["ok"].as_i64()), (Some("recovery"), Some(2), Some(2)), "{last}");
    let started: Vec<String> = analyst.inner.started.lock().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(started, ["stock:AAPL", "stock:TSLA"], "solo los del modelo; MSFT (activo) y NVDA (ok) no");
    // El histórico no se tocó: las filas antiguas siguen con transient = 0.
    let old_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_results WHERE run_id = 1 AND transient = 0").fetch_one(&state.db).await.unwrap();
    assert_eq!(old_rows, 4);
    stop(&state, handle).await;
}

// --- Ventana de la pasada: del cierre de EE. UU. de D a la apertura siguiente ---------------------

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

/// (estado, transitorio, código) de todos los resultados de un día de bolsa.
async fn day_rows(app: &TestApp, day: &str) -> Vec<(String, bool, Option<String>)> {
    sqlx::query_as("SELECT status, transient, error_code FROM watchlist_results WHERE local_date = ?1 ORDER BY id")
        .bind(day)
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn switched_on_at_9am_the_next_day_it_does_the_previous_trading_days_pass() {
    let app = TestApp::new().await;
    // Jueves 15-oct, 09:00 locales (07:00 UTC): EE. UU. no abre hasta las 15:30 locales y la ventana
    // del miércoles 14 sigue abierta (el PC no estaba encendido a las 23:00).
    let clock = ManualClock::new(utc(15, 7, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = runs(&app).await.remove(0);
    assert_eq!((r["localDate"].as_str(), r["trigger"].as_str(), r["ok"].as_i64()), (Some("2026-10-14"), Some("recovery"), Some(2)), "{r}");
    assert_eq!(r["scheduledFor"], "2026-10-14T21:00:00.000Z", "la hora preferida de D (23:00 locales)");
    assert_eq!(r["startedAt"], "2026-10-15T07:00:00.000Z", "la hora real");
    // Los resultados cuelgan del día de bolsa, no del día de ejecución.
    assert!(day_rows(&app, "2026-10-15").await.is_empty());
    assert_eq!(day_rows(&app, "2026-10-14").await.len(), 2);
    // Llega la apertura: la pasada de D estaba hecha, no se registra nada perdido.
    clock.set(utc(15, 14, 0));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(runs(&app).await.len(), 1);
    // Y al cierre y a la hora preferida de D+1, su propia pasada, con su día de bolsa.
    clock.set(utc(15, 21, 0));
    wait_for(|| async { (runs(&app).await.len() == 2 && runs(&app).await[1]["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[1];
    assert_eq!((r["localDate"].as_str(), r["trigger"].as_str()), (Some("2026-10-15"), Some("schedule")));
    stop(&state, handle).await;
}

#[tokio::test]
async fn friday_pass_is_still_valid_on_monday_morning() {
    let app = TestApp::new().await;
    // Lunes 19-oct, 09:00 locales: la pasada del viernes 16 vale hasta la apertura del lunes (15:30).
    let clock = ManualClock::new(utc(19, 7, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL, crypto:ETH"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = runs(&app).await.remove(0);
    assert_eq!((r["localDate"].as_str(), r["trigger"].as_str(), r["ok"].as_i64()), (Some("2026-10-16"), Some("recovery"), Some(2)), "{r}");
    assert_eq!(r["scheduledFor"], "2026-10-16T21:00:00.000Z");
    assert!(day_rows(&app, "2026-10-19").await.is_empty());
    stop(&state, handle).await;
    // El sábado y el domingo valen igual.
    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(18, 12, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["localDate"], "2026-10-16");
    stop(&state, handle).await;
}

#[tokio::test]
async fn switched_on_after_the_open_the_pass_is_missed_and_not_run_with_the_market_open() {
    let app = TestApp::new().await;
    // Jueves 15-oct, 16:00 locales: EE. UU. abrió a las 15:30 locales sin que la pasada del miércoles 14 se hiciera.
    let clock = ManualClock::new(utc(15, 14, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT"), clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "missed").then_some(()) }).await;

    let r = runs(&app).await.remove(0);
    assert_eq!((r["status"].as_str(), r["localDate"].as_str(), r["planned"].as_i64(), r["ok"].as_i64()), (Some("missed"), Some("2026-10-14"), Some(2), Some(0)), "{r}");
    let rows = day_rows(&app, "2026-10-14").await;
    assert!(rows.iter().all(|(s, t, c)| s == "skipped" && !*t && c.as_deref() == Some("missed")), "{rows:?}");
    assert_eq!(rows.len(), 2);
    let (_, w) = app.get("/api/watchlist").await;
    let first = &w["lastRunResults"][0];
    assert!(first["error"].as_str().unwrap().contains("perdida"), "{first}");
    assert!(first["error"].as_str().unwrap().contains("mercado abierto"), "{first}");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 0, "no se analizó con el mercado abierto");
    assert!(origins(&app).await.is_empty());
    stop(&state, handle).await;

    // Reiniciar el servicio durante la sesión (estado nuevo: `stop` canceló el apagado del anterior) no
    // vuelve a registrarla ni la analiza.
    let again = AppState::new(app.market.clone(), app.analyst.clone(), app.state.db.clone(), Settings::default())
        .with_news_annotator(annotate_fake)
        .with_power(Arc::new(ManualPower::default()))
        .with_watchlist(cfg("stock:AAPL, stock:MSFT"), clock.clone());
    let handle = tokio::spawn(run_scheduler(again.clone(), again.watchlist.clone(), clock.clone()));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(runs(&app).await.len(), 1, "una sola pasada perdida");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 0);
    // Al cierre de ese día empieza la ventana siguiente, con su pasada normal.
    clock.set(utc(15, 21, 0));
    wait_for(|| async { (runs(&app).await.len() == 2 && runs(&app).await[1]["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[1];
    assert_eq!((r["localDate"].as_str(), r["ok"].as_i64()), (Some("2026-10-15"), Some(2)));
    stop(&again, handle).await;
}

#[tokio::test]
async fn a_pass_in_progress_when_the_market_opens_is_cut_and_the_rest_is_recorded_as_missed() {
    let app = TestApp::new().await;
    *app.analyst.analyze_delay.lock().unwrap() = Duration::from_millis(400);
    // Jueves 15-oct, 15:29 locales (13:29 UTC): queda un minuto de ventana (abre a las 13:30 UTC).
    let clock = ManualClock::new(utc(15, 13, 29), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL, stock:MSFT, stock:TSLA, crypto:ETH"), clock.clone()).await;
    wait_for(|| async { (!app.analyst.started.lock().unwrap().is_empty()).then_some(()) }).await;
    clock.set(utc(15, 13, 31)); // abre EE. UU. mientras analiza el primero
    wait_for(|| async { (runs(&app).await.len() == 2).then_some(()) }).await;

    let rs = runs(&app).await;
    assert_eq!((rs[0]["status"].as_str(), rs[0]["ok"].as_i64(), rs[0]["planned"].as_i64()), (Some("aborted"), Some(1), Some(4)), "{rs:?}");
    assert_eq!((rs[1]["status"].as_str(), rs[1]["planned"].as_i64(), rs[1]["localDate"].as_str()), (Some("missed"), Some(3), Some("2026-10-14")), "{rs:?}");
    let started: Vec<String> = app.analyst.started.lock().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(started, ["stock:AAPL"], "tras la apertura no se empezó ningún análisis más");
    let rows = day_rows(&app, "2026-10-14").await;
    assert_eq!(rows.iter().filter(|r| r.0 == "ok").count(), 1);
    assert_eq!(rows.iter().filter(|r| r.0 == "skipped" && r.2.as_deref() == Some("missed")).count(), 3);
    stop(&state, handle).await;
}

#[tokio::test]
async fn in_the_weeks_the_clock_changes_do_not_match_the_window_follows_new_york_not_madrid() {
    // Madrid en invierno (+1) desde el 25-oct-2026 y EE. UU. en verano hasta el 1-nov: la diferencia es de
    // 5 h y el mercado cierra a las 21:00 y abre a las 14:30 de Madrid (no 22:00 y 15:30).
    const CET: i32 = 3600;
    let day = chrono::NaiveTime::from_hms_opt;

    // 14:45 de Madrid del miércoles 28: el mercado ya abrió (14:30). Con una regla «15:30 de Madrid» se
    // habría analizado con la sesión abierta; aquí la pasada del martes 27 está perdida.
    let app = TestApp::new().await;
    let clock = ManualClock::new(at(2026, 10, 28, 13, 45), CET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "missed").then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["localDate"], "2026-10-27");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 0);
    stop(&state, handle).await;

    // 14:15 de Madrid: aún quedan 15 minutos de ventana -> se hace la pasada del martes.
    let app = TestApp::new().await;
    let clock = ManualClock::new(at(2026, 10, 28, 13, 15), CET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["localDate"], "2026-10-27");
    stop(&state, handle).await;

    // Por la tarde del martes: cierra a las 21:00 de Madrid. Con la hora preferida a las 21:30 (inválida
    // en una semana normal, válida en esta) la pasada sale a las 21:30 y no antes.
    let app = TestApp::new().await;
    let mut c = cfg("stock:AAPL");
    c.time = day(21, 30, 0).unwrap();
    let clock = ManualClock::new(at(2026, 10, 27, 20, 10), CET); // 21:10 de Madrid
    let (state, handle) = start(&app, c, clock.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runs(&app).await.is_empty());
    clock.set(at(2026, 10, 27, 20, 30));
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!((r["localDate"].as_str(), r["scheduledFor"].as_str()), (Some("2026-10-27"), Some("2026-10-27T20:30:00.000Z")));
    stop(&state, handle).await;

    // Y la otra semana desfasada, la de marzo (EE. UU. en verano desde el 8-mar, Madrid en invierno hasta el 29):
    // miércoles 11-mar a las 14:45 de Madrid, perdida la del martes 10.
    let app = TestApp::new().await;
    let clock = ManualClock::new(at(2026, 3, 11, 13, 45), CET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "missed").then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["localDate"], "2026-03-10");
    stop(&state, handle).await;
}

#[tokio::test]
async fn migration_0008_keeps_the_existing_runs_and_results_and_allows_missed() {
    use sqlx::migrate::Migrator;
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migrator = Migrator::new(dir.clone()).await.unwrap();
    // El esquema que tiene el usuario hasta ahora: solo las migraciones 1..=7.
    let mut until_7 = Migrator::new(dir).await.unwrap();
    until_7.migrations = std::borrow::Cow::Owned(migrator.migrations.iter().filter(|m| m.version <= 7).cloned().collect());
    until_7.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO watchlist_runs (id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, model, planned, ok, failed) VALUES (1, '2026-10-20', 10, 20, 30, 'recovery', 'completed', 'standard', 'qwen3.5:9b', 2, 0, 2)")
        .execute(&pool).await.unwrap();
    for (asset, err) in [("stock:AAPL", "no encontrado: modelo «x»"), ("stock:MSFT", "otro")] {
        sqlx::query("INSERT INTO watchlist_results (run_id, local_date, asset_id, status, transient, attempts, error_code, error, started_at, finished_at) VALUES (1, '2026-10-20', ?1, 'failed', 0, 1, 'not_found', ?2, 40, 50)")
            .bind(asset).bind(err).execute(&pool).await.unwrap();
    }
    type RunRow = (i64, String, i64, String, String, Option<String>, i64, i64);
    type ResultRow = (i64, i64, String, String, i64, Option<String>);
    let dump = |pool: sqlx::SqlitePool| async move {
        let runs: Vec<RunRow> =
            sqlx::query_as("SELECT id, local_date, started_at, trigger, status, model, ok, failed FROM watchlist_runs ORDER BY id").fetch_all(&pool).await.unwrap();
        let results: Vec<ResultRow> =
            sqlx::query_as("SELECT id, run_id, asset_id, status, transient, error FROM watchlist_results ORDER BY id").fetch_all(&pool).await.unwrap();
        (runs, results)
    };
    let before = dump(pool.clone()).await;
    migrator.run(&pool).await.unwrap(); // aplica la 0008
    assert_eq!(dump(pool.clone()).await, before, "ni una fila cambia");
    let violations: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap();
    assert!(violations.is_empty());
    // Ahora admite 'missed', sigue rechazando estados raros y la cascada funciona.
    sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('2026-10-21', 0, 0, 'schedule', 'missed', 'standard', 1)")
        .execute(&pool).await.unwrap();
    assert!(sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('x', 0, 0, 'schedule', 'otro', 'standard', 1)")
        .execute(&pool).await.is_err());
    sqlx::query("DELETE FROM watchlist_runs WHERE id = 1").execute(&pool).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_results").fetch_one(&pool).await.unwrap();
    assert_eq!(left, 0, "ON DELETE CASCADE sigue vigente tras reconstruir las tablas");
}

// --- Pasadas antiguas, ventanas cerradas y equipo apagado ------------------------------------------

/// Madrid en invierno (desde el 25-oct-2026): +01:00.
const MADRID_WINTER: i32 = 3600;

const OLD_ERR: &str = "no encontrado: no encontrado: no encontrado: modelo «qwen3.5:9b»: model 'qwen3.5:9b' not found";

/// Los cinco activos con `ok` del run 3 de la BD real.
const RUN3_OK: [&str; 5] = ["stock:AAPL", "stock:NVDA", "stock:KO", "etf:GLD", "stock:ASML"];

/// (id, `local_date`, estado) de todas las pasadas, en orden de registro.
async fn db_runs(app: &TestApp) -> Vec<(i64, String, String)> {
    sqlx::query_as("SELECT id, local_date, status FROM watchlist_runs ORDER BY id").fetch_all(&app.state.db).await.unwrap()
}

/// Servicio nuevo sobre la misma BD (`stop` canceló el apagado del anterior): un reinicio.
async fn restart(app: &TestApp, cfg: WatchlistConfig, clock: Arc<ManualClock>) -> (AppState, tokio::task::JoinHandle<()>) {
    let state = AppState::new(app.market.clone(), app.analyst.clone(), app.state.db.clone(), Settings::default())
        .with_news_annotator(annotate_fake)
        .with_power(Arc::new(ManualPower::default()))
        .with_watchlist(cfg, clock.clone());
    let handle = tokio::spawn(run_scheduler(state.clone(), state.watchlist.clone(), clock));
    (state, handle)
}

/// Siembra una pasada ANTERIOR a la 0008: `local_date` es el día local de ejecución y todo cuelga de `started`.
async fn seed_run(app: &TestApp, id: i64, local_date: &str, started: DateTime<Utc>, status: &str, rows: &[(&str, &str)]) {
    let ms = started.timestamp_millis();
    let ok = rows.iter().filter(|r| r.1 == "ok").count() as i64;
    sqlx::query("INSERT INTO watchlist_runs (id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, planned, ok, failed) VALUES (?1, ?2, ?3, ?3, ?3, 'recovery', ?4, 'standard', ?5, ?6, ?7)")
        .bind(id).bind(local_date).bind(ms).bind(status).bind(rows.len() as i64).bind(ok).bind(rows.len() as i64 - ok)
        .execute(&app.state.db).await.unwrap();
    for (asset, st) in rows {
        let err = (*st == "failed").then_some(OLD_ERR);
        sqlx::query("INSERT INTO watchlist_results (run_id, local_date, asset_id, status, transient, attempts, error_code, error, started_at, finished_at) VALUES (?1, ?2, ?3, ?4, 0, 1, ?5, ?6, ?7, ?7)")
            .bind(id).bind(local_date).bind(asset).bind(st).bind(err.map(|_| "not_found")).bind(err).bind(ms)
            .execute(&app.state.db).await.unwrap();
    }
}

/// Las tres pasadas de la BD real (instantes en UTC), con la lista por defecto de 20 activos.
async fn seed_real_db(app: &TestApp) {
    let all = |st| DEFAULT_WATCHLIST.iter().map(|a| (*a, st)).collect::<Vec<_>>();
    seed_run(app, 1, "2026-10-20", at(2026, 10, 20, 21, 32), "completed", &all("failed")).await;
    // Con el mercado ABIERTO del viernes 23 (abre 13:30 UTC): no es la pasada de ningún día de bolsa.
    seed_run(app, 2, "2026-10-23", at(2026, 10, 23, 15, 8), "completed", &all("ok")).await;
    // Domingo 25: ya dentro de la ventana del viernes 23.
    seed_run(app, 3, "2026-10-25", at(2026, 10, 25, 11, 5), "interrupted", &RUN3_OK.map(|a| (a, "ok"))).await;
}

/// Activos que intentó una pasada, en orden. (El mercado simulado solo conoce unos pocos de la lista por
/// defecto: el resto falla como «activo desconocido», así que se mira qué se intentó, no cuántos salieron `ok`.)
async fn attempted(app: &TestApp, run_id: i64) -> Vec<String> {
    sqlx::query_scalar("SELECT asset_id FROM watchlist_results WHERE run_id = ?1 ORDER BY id").bind(run_id).fetch_all(&app.state.db).await.unwrap()
}

fn not_in_run3() -> Vec<String> {
    DEFAULT_WATCHLIST.iter().filter(|a| !RUN3_OK.contains(a)).map(ToString::to_string).collect()
}

#[tokio::test]
async fn old_rows_count_for_the_window_they_started_in_and_not_for_their_local_date() {
    let app = TestApp::new().await;
    seed_real_db(&app).await;
    // La regla, directamente: de D=23 están hechos exactamente los 5 del run 3 (empezó dentro de su ventana,
    // aunque su `local_date` sea el domingo 25); el run 2 (mercado abierto, `local_date` = 23) no cuenta.
    let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
    let states = wrepo::day_states(&wrepo::window_results(&app.state.db, &window_of(day(23))).await.unwrap());
    let mut done: Vec<&str> = states.iter().filter(|(_, s)| **s == wrepo::DayState::Done).map(|(a, _)| a.as_str()).collect();
    done.sort_unstable();
    let mut expected = RUN3_OK.to_vec();
    expected.sort_unstable();
    assert_eq!(done, expected);
    assert_eq!(states.len(), 5, "el resto no tiene nada en la ventana: pendiente");
    assert!(wrepo::window_results(&app.state.db, &window_of(day(22))).await.unwrap().is_empty(), "el run 2 no es de D=22 ni de D=23");

    // Desplegado ANTES del cierre de la ventana (lunes 26 a las 08:00 UTC, abre a las 13:30): se hacen los 15 que faltan.
    let clock = ManualClock::new(at(2026, 10, 26, 8, 0), MADRID_WINTER);
    let (state, handle) = start(&app, cfg(""), clock.clone()).await;
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.0 > 3 && r.2 == "completed").then_some(()) }).await;
    let r = runs(&app).await.pop().unwrap();
    assert_eq!((r["localDate"].as_str(), r["trigger"].as_str(), r["planned"].as_i64()), (Some("2026-10-23"), Some("recovery"), Some(15)), "{r}");
    assert_eq!(attempted(&app, run_id(&r)).await, not_in_run3(), "ni los 5 del run 3 ni nada por el run 2");
    // Llega la apertura: D=23 está completa, no se pierde nada.
    clock.set(at(2026, 10, 26, 13, 30));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(db_runs(&app).await.len(), 4, "tres antiguas y la de hoy; ninguna missed");
    stop(&state, handle).await;
}

#[tokio::test]
async fn old_rows_deployed_after_the_close_leave_exactly_the_missing_assets_as_missed() {
    let app = TestApp::new().await;
    seed_real_db(&app).await;
    // Desplegado DESPUÉS de la apertura (lunes 26, 15:00 UTC): de D=23 faltan 15 y ya no se analizan.
    let clock = ManualClock::new(at(2026, 10, 26, 15, 0), MADRID_WINTER);
    let (state, handle) = start(&app, cfg(""), clock.clone()).await;
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "missed").then_some(()) }).await;
    let missed: Vec<(String, String, String)> =
        sqlx::query_as("SELECT asset_id, local_date, error FROM watchlist_results WHERE error_code = 'missed' ORDER BY id")
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert_eq!(missed.iter().map(|m| m.0.clone()).collect::<Vec<_>>(), not_in_run3());
    assert!(missed.iter().all(|m| m.1 == "2026-10-23"), "{missed:?}");
    assert!(missed.iter().all(|m| !m.2.contains("Último intento")), "los fallos del 20-oct no se cuelan en el 23");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 0, "no se analizó con el mercado abierto");
    let planned: i64 = sqlx::query_scalar("SELECT planned FROM watchlist_runs WHERE status = 'missed'").fetch_one(&state.db).await.unwrap();
    assert_eq!(planned, 15);
    stop(&state, handle).await;

    // Reiniciar no la duplica, y D=26 hace su pasada con sus 20 activos (las pérdidas de D=23 no le cuentan).
    let (again, handle) = restart(&app, cfg(""), clock.clone()).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(db_runs(&app).await.len(), 4);
    clock.set(at(2026, 10, 26, 22, 0));
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.0 > 4 && r.2 == "completed").then_some(()) }).await;
    let r = runs(&app).await.pop().unwrap();
    assert_eq!((r["localDate"].as_str(), r["planned"].as_i64()), (Some("2026-10-26"), Some(20)), "{r}");
    assert_eq!(attempted(&app, run_id(&r)).await.len(), 20);
    stop(&again, handle).await;
}

/// Pasada completa del lunes 12-oct (D=12) con un servicio que luego se apaga.
async fn complete_monday_12(app: &TestApp, list: &str) {
    let clock = ManualClock::new(utc(12, 21, 0), OFFSET);
    clock.auto_advance_until(utc(12, 23, 0));
    let (state, handle) = start(app, cfg(list), clock).await;
    wait_for(|| async { db_runs(app).await.iter().any(|r| r.2 == "completed").then_some(()) }).await;
    stop(&state, handle).await;
}

#[tokio::test]
async fn pc_off_during_a_whole_session_records_the_lost_window_without_touching_the_next_one() {
    let app = TestApp::new().await;
    const LIST: &str = "stock:AAPL, stock:MSFT";
    complete_monday_12(&app, LIST).await;
    // El martes 13 no se enciende el equipo en toda su ventana ni en la sesión del miércoles: vuelve el
    // miércoles 14 a las 20:10 UTC (ya abierta la ventana de D=14). Antes la de D=13 se quedaba pendiente para siempre.
    let clock = ManualClock::new(utc(14, 20, 10), OFFSET);
    let (state, handle) = restart(&app, cfg(LIST), clock.clone()).await;
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "missed").then_some(()) }).await;
    let runs_now = db_runs(&app).await;
    assert_eq!(runs_now.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(), [("2026-10-12", "completed"), ("2026-10-13", "missed")]);
    // La pérdida se anota DENTRO de la ventana de D=14, pero no cuenta para ella: su pasada se hace entera.
    clock.set(utc(14, 21, 0));
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.1 == "2026-10-14" && r.2 == "completed").then_some(()) }).await;
    let r = runs(&app).await.pop().unwrap();
    assert_eq!((r["planned"].as_i64(), r["ok"].as_i64()), (Some(2), Some(2)), "{r}");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 4, "D=12 y D=14; nada de D=13");
    stop(&state, handle).await;

    // Reiniciar no duplica nada.
    let (again, handle) = restart(&app, cfg(LIST), ManualClock::new(utc(14, 22, 0), OFFSET)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(db_runs(&app).await.len(), 3);
    stop(&again, handle).await;
}

#[tokio::test]
async fn pc_off_for_three_days_records_every_lost_window_once() {
    let app = TestApp::new().await;
    const LIST: &str = "stock:AAPL, stock:MSFT";
    complete_monday_12(&app, LIST).await;
    // Apagado martes 13, miércoles 14 y jueves 15: vuelve el viernes 16 a las 20:10 UTC. Antes solo se
    // registraba la última ventana, y solo con el equipo encendido durante la apertura.
    let clock = ManualClock::new(utc(16, 20, 10), OFFSET);
    let (state, handle) = restart(&app, cfg(LIST), clock.clone()).await;
    wait_for(|| async { (db_runs(&app).await.iter().filter(|r| r.2 == "missed").count() == 3).then_some(()) }).await;
    clock.set(utc(16, 21, 0));
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.1 == "2026-10-16" && r.2 == "completed").then_some(()) }).await;
    let expected = [("2026-10-12", "completed"), ("2026-10-13", "missed"), ("2026-10-14", "missed"), ("2026-10-15", "missed"), ("2026-10-16", "completed")];
    let got = db_runs(&app).await;
    assert_eq!(got.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(), expected, "en orden y sin duplicados");
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT local_date, COUNT(*) FROM watchlist_results WHERE error_code = 'missed' GROUP BY local_date ORDER BY local_date")
        .fetch_all(&state.db)
        .await
        .unwrap();
    assert_eq!(rows, [("2026-10-13".to_string(), 2), ("2026-10-14".to_string(), 2), ("2026-10-15".to_string(), 2)]);
    stop(&state, handle).await;

    let (again, handle) = restart(&app, cfg(LIST), ManualClock::new(utc(16, 23, 0), OFFSET)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(db_runs(&app).await.len(), 5, "un reinicio no repite ninguna");
    stop(&again, handle).await;
}

#[tokio::test]
async fn the_backfill_of_lost_windows_is_capped_at_ten_trading_days_and_skipped_on_an_empty_db() {
    // BD vacía durante la ventana de D=23 (sábado 24): no se rellena el pasado.
    let app = TestApp::new().await;
    let clock = ManualClock::new(at(2026, 10, 24, 12, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "completed").then_some(()) }).await;
    assert_eq!(db_runs(&app).await.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["2026-10-23"]);
    stop(&state, handle).await;

    // Con una pasada del lunes 5-oct y tres semanas apagado, solo se cierran las 10 últimas ventanas.
    let app = TestApp::new().await;
    seed_run(&app, 1, "2026-10-05", at(2026, 10, 5, 21, 0), "completed", &[("stock:AAPL", "ok")]).await;
    let clock = ManualClock::new(at(2026, 10, 24, 12, 0), OFFSET);
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock).await;
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "completed" && r.0 > 1).then_some(()) }).await;
    let missed: Vec<String> = db_runs(&app).await.into_iter().filter(|r| r.2 == "missed").map(|r| r.1).collect();
    assert_eq!(
        missed,
        ["2026-10-09", "2026-10-12", "2026-10-13", "2026-10-14", "2026-10-15", "2026-10-16", "2026-10-19", "2026-10-20", "2026-10-21", "2026-10-22"]
    );
    stop(&state, handle).await;
}

#[tokio::test]
async fn waking_up_with_the_window_closed_during_the_wait_between_rounds_creates_no_empty_aborted_run() {
    let app = TestApp::new().await;
    // Los 4 primeros intentos fallan: la ronda se aborta y se espera 30 min a la siguiente.
    app.analyst.fail_first_calls.store(4, Ordering::SeqCst);
    let mut c = cfg("stock:AAPL, stock:MSFT, stock:TSLA, crypto:ETH");
    c.retry.attempts = 2;
    let clock = ManualClock::new(utc(14, 21, 0), OFFSET);
    clock.auto_advance_until(utc(14, 21, 10)); // las esperas cortas saltan; la de 30 min no
    let (state, handle) = start(&app, c, clock.clone()).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "aborted").then_some(()) }).await;
    // El equipo se suspende y despierta con EE. UU. ya abierto (jueves 15, 14:00 UTC): no hay otra ronda.
    clock.set(utc(15, 14, 0));
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "missed").then_some(()) }).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let got = db_runs(&app).await;
    assert_eq!(got.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(), [("2026-10-14", "aborted"), ("2026-10-14", "missed")]);
    let empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_runs r WHERE NOT EXISTS (SELECT 1 FROM watchlist_results WHERE run_id = r.id)")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(empty, 0, "ninguna pasada vacía");
    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 4, "ni un análisis con el mercado abierto");
    let planned: i64 = sqlx::query_scalar("SELECT planned FROM watchlist_runs WHERE status = 'missed'").fetch_one(&state.db).await.unwrap();
    assert_eq!(planned, 4);
    stop(&state, handle).await;
}

#[tokio::test]
async fn migration_0008_keeps_the_autoincrement_sequence_of_the_old_tables() {
    use sqlx::migrate::Migrator;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migrator = Migrator::new(dir.clone()).await.unwrap();
    // `true`: se borra TODO antes de migrar (la tabla nueva no recibe filas, así que no tendría fila en
    // `sqlite_sequence`); `false`: solo el último id (la nueva volvería a `max(id)`).
    for delete_all in [false, true] {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let mut until_7 = Migrator::new(dir.clone()).await.unwrap();
        until_7.migrations = std::borrow::Cow::Owned(migrator.migrations.iter().filter(|m| m.version <= 7).cloned().collect());
        until_7.run(&pool).await.unwrap();
        for i in 1..=3 {
            let run = sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('2026-10-20', 0, 0, 'schedule', 'completed', 'standard', 1)")
                .execute(&pool).await.unwrap().last_insert_rowid();
            assert_eq!(run, i);
            sqlx::query("INSERT INTO watchlist_results (run_id, local_date, asset_id, status, started_at, finished_at) VALUES (?1, '2026-10-20', 'stock:AAPL', 'ok', 0, 0)")
                .bind(run).execute(&pool).await.unwrap();
        }
        // El borrado en cascada se lleva también los resultados del run 3 (o de todos).
        sqlx::query(if delete_all { "DELETE FROM watchlist_runs" } else { "DELETE FROM watchlist_runs WHERE id = 3" }).execute(&pool).await.unwrap();
        migrator.run(&pool).await.unwrap();

        let run = sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('2026-10-21', 0, 0, 'schedule', 'missed', 'standard', 1)")
            .execute(&pool).await.unwrap().last_insert_rowid();
        let result = sqlx::query("INSERT INTO watchlist_results (run_id, local_date, asset_id, status, started_at, finished_at) VALUES (?1, '2026-10-21', 'stock:AAPL', 'skipped', 0, 0)")
            .bind(run).execute(&pool).await.unwrap().last_insert_rowid();
        assert_eq!((run, result), (4, 4), "los ids borrados no se reutilizan (delete_all = {delete_all})");
        let leftovers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_sequence WHERE name LIKE '%\\_new' ESCAPE '\\'").fetch_one(&pool).await.unwrap();
        assert_eq!(leftovers, 0, "sin contadores huérfanos de las tablas temporales");
    }
}

// --- Energía ---------------------------------------------------------------------------------

fn fake_supply(root: &std::path::Path, name: &str, kind: &str, online: Option<&str>) {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
    if let Some(o) = online {
        std::fs::write(d.join("online"), format!("{o}\n")).unwrap();
    }
}

#[test]
fn system_power_reads_sysfs() {
    let root = std::env::temp_dir().join(format!("invest-ai-power-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(SystemPower::at(&root).on_battery(), None, "sin fuentes externas: desconocido");
    fake_supply(&root, "BAT0", "Battery", None); // las baterías no cuentan
    assert_eq!(SystemPower::at(&root).on_battery(), None);
    fake_supply(&root, "AC", "Mains", Some("0"));
    assert_eq!(SystemPower::at(&root).on_battery(), Some(true));
    fake_supply(&root, "AC", "Mains", Some("1"));
    assert_eq!(SystemPower::at(&root).on_battery(), Some(false));
    assert_eq!(SystemPower::at(root.join("no-existe")).on_battery(), None);
    std::fs::remove_dir_all(root).ok();
    // Sobremesa: puertos USB-C listados y desconectados, sin batería → desconocido (no espera).
    let desk = std::env::temp_dir().join(format!("invest-ai-power-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&desk).unwrap();
    fake_supply(&desk, "ucsi-source-psy-USBC000:001", "USB", Some("0"));
    assert_eq!(SystemPower::at(&desk).on_battery(), None, "sin batería no es «con batería»");
    std::fs::remove_dir_all(desk).ok();
}

fn power_cfg() -> WatchlistConfig {
    let mut c = cfg("stock:AAPL");
    c.battery_max_delay = Duration::from_secs(3600);
    c.power_poll = Duration::from_secs(300);
    c
}

#[tokio::test]
async fn postpones_the_run_while_on_battery_until_plugged_in() {
    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET); // sin avance automático: lo mueve la prueba
    let power = Arc::new(ManualPower(std::sync::Mutex::new(Some(true))));
    let (state, handle) = start_with_power(&app, power_cfg(), clock.clone(), power.clone()).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(runs(&app).await.is_empty(), "con batería no empieza");
    *power.0.lock().unwrap() = Some(false); // se enchufa
    clock.set(utc(15, 21, 5)); // vence el sondeo de 5 min
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!(r["power"], "ac");
    assert_eq!(r["powerWaitSecs"], 300, "esperó un sondeo: {r}");
    stop(&state, handle).await;
}

#[tokio::test]
async fn battery_delay_has_a_cap_and_unknown_power_does_not_wait() {
    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let power = Arc::new(ManualPower(std::sync::Mutex::new(Some(true)))); // nunca se enchufa
    let (state, handle) = start_with_power(&app, power_cfg(), clock.clone(), power).await;
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!((r["power"].as_str(), r["powerWaitSecs"].as_i64()), (Some("battery"), Some(3600)), "tras el tope se hace igual");
    stop(&state, handle).await;

    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(15, 21, 0), OFFSET);
    clock.auto_advance_until(utc(15, 23, 0));
    let (state, handle) = start(&app, power_cfg(), clock).await; // energía desconocida
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    let r = &runs(&app).await[0];
    assert_eq!((r["power"].as_str(), r["powerWaitSecs"].as_i64()), (Some("unknown"), Some(0)));
    stop(&state, handle).await;
}

// --- Casos límite: comprobación previa lenta, suspensión en la espera de reintento, `missed` atómico ---

#[tokio::test]
async fn the_open_arriving_during_a_slow_preflight_creates_no_empty_run_and_the_rest_is_missed() {
    let app = TestApp::new().await;
    let ollama = SimOllama::new(true);
    ollama.slow.store(true, Ordering::SeqCst);
    // Jueves 15-oct 13:29 UTC: queda un minuto de la ventana de D=14 (EE. UU. abre a las 13:30 UTC).
    let clock = ManualClock::new(utc(15, 13, 29), OFFSET);
    let (state, handle, analyst) = start_sim(&app, cfg("stock:AAPL, stock:MSFT"), clock.clone(), &ollama).await;
    wait_for(|| async { (ollama.list_calls.load(Ordering::SeqCst) > 0).then_some(()) }).await;
    clock.set(utc(15, 13, 31)); // abre EE. UU. con la consulta a Ollama aún sin responder
    ollama.slow.store(false, Ordering::SeqCst); // responde: el modelo está, pero ya es tarde
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "missed").then_some(()) }).await;

    let got = db_runs(&app).await;
    assert_eq!(got.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(), [("2026-10-14", "missed")], "ni una pasada `aborted` vacía");
    let rows = day_rows(&app, "2026-10-14").await;
    assert!(rows.len() == 2 && rows.iter().all(|(s, t, c)| s == "skipped" && !*t && c.as_deref() == Some("missed")), "{rows:?}");
    assert_eq!(analyst.inner.analyze_calls.load(Ordering::SeqCst), 0, "no se analizó con el mercado abierto");
    stop(&state, handle).await;
}

/// Reloj que anota cada espera: la prueba sabe cuándo el programador ya duerme y puede mover el reloj sin carreras.
struct SpyClock {
    inner: Arc<ManualClock>,
    sleeps: std::sync::Mutex<Vec<DateTime<Utc>>>,
}

impl SpyClock {
    fn new(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self { inner: ManualClock::new(start, OFFSET), sleeps: Default::default() })
    }

    fn sleeping_until(&self, t: DateTime<Utc>) -> bool {
        self.sleeps.lock().unwrap().contains(&t)
    }
}

#[async_trait]
impl Clock for SpyClock {
    fn now(&self) -> DateTime<Utc> {
        self.inner.now()
    }

    fn utc_to_local(&self, t: DateTime<Utc>) -> chrono::NaiveDateTime {
        self.inner.utc_to_local(t)
    }

    fn local_to_utc(&self, l: chrono::NaiveDateTime) -> DateTime<Utc> {
        self.inner.local_to_utc(l)
    }

    async fn sleep_until(&self, t: DateTime<Utc>) {
        self.sleeps.lock().unwrap().push(t);
        self.inner.sleep_until(t).await;
    }
}

async fn start_spy(app: &TestApp, cfg: WatchlistConfig, clock: Arc<SpyClock>) -> (AppState, tokio::task::JoinHandle<()>) {
    // Estado nuevo sobre la misma BD, como `restart`: un `stop` anterior canceló el apagado del de `app`.
    let state = AppState::new(app.market.clone(), app.analyst.clone(), app.state.db.clone(), Settings::default())
        .with_news_annotator(annotate_fake)
        .with_power(Arc::new(ManualPower::default()))
        .with_watchlist(cfg, clock.clone());
    let handle = tokio::spawn(run_scheduler(state.clone(), state.watchlist.clone(), clock));
    (state, handle)
}

#[tokio::test]
async fn waking_up_after_the_open_during_a_retry_wait_makes_no_new_attempt_and_leaves_the_asset_missed() {
    let app = TestApp::new().await;
    app.analyst.fail_first_calls.store(1, Ordering::SeqCst); // el primer intento de AAPL falla (transitorio)
    // Jueves 15-oct 13:25 UTC; la espera de reintento (60 s) acaba a las 13:26, antes de la apertura (13:30).
    let clock = SpyClock::new(utc(15, 13, 25));
    let (state, handle) = start_spy(&app, cfg("stock:AAPL, stock:MSFT"), clock.clone()).await;
    wait_for(|| async { clock.sleeping_until(utc(15, 13, 26)).then_some(()) }).await;
    clock.inner.set(utc(15, 14, 0)); // el equipo se suspende en la espera y despierta con EE. UU. abierto
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "aborted").then_some(()) }).await;

    assert_eq!(app.analyst.analyze_calls.load(Ordering::SeqCst), 1, "ningún intento nuevo con el mercado abierto");
    wait_for(|| async { db_runs(&app).await.iter().any(|r| r.2 == "missed").then_some(()) }).await;
    let ok: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_results WHERE status = 'ok'").fetch_one(&state.db).await.unwrap();
    assert_eq!(ok, 0, "ningún `ok` para D con datos de mercado abierto");
    let rows = day_rows(&app, "2026-10-14").await;
    let kinds: Vec<(&str, bool)> = rows.iter().map(|r| (r.0.as_str(), r.1)).collect();
    assert_eq!(kinds, [("failed", true), ("skipped", false), ("skipped", false)], "{rows:?}");
    assert_eq!(rows[1].2.as_deref(), Some("missed"));
    assert!(origins(&app).await.is_empty());
    stop(&state, handle).await;
}

#[tokio::test]
async fn a_write_failure_in_the_middle_of_recording_a_lost_window_leaves_nothing_and_the_next_try_records_it_whole() {
    let app = TestApp::new().await;
    const LIST: &str = "stock:AAPL, stock:MSFT";
    complete_monday_12(&app, LIST).await;
    // El equipo vuelve el jueves 15 a las 13:35 UTC (abierto EE. UU.): hay que registrar D=13 y D=14.
    // Un trigger hace fallar la 2.ª inserción de una `missed` (la de MSFT de D=13), es decir, a mitad de la ventana.
    sqlx::raw_sql(
        "CREATE TRIGGER inject_failure BEFORE INSERT ON watchlist_results
         WHEN NEW.error_code = 'missed' AND (SELECT COUNT(*) FROM watchlist_results WHERE error_code = 'missed') = 1
         BEGIN SELECT RAISE(ABORT, 'fallo inyectado'); END",
    )
    .execute(&app.state.db)
    .await
    .unwrap();
    let clock = SpyClock::new(utc(15, 13, 35));
    let (state, handle) = start_spy(&app, cfg(LIST), clock.clone()).await;
    // Falla y el programador espera 5 min para reintentar.
    wait_for(|| async { clock.sleeping_until(utc(15, 13, 40)).then_some(()) }).await;
    let got = db_runs(&app).await;
    assert_eq!(got.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(), [("2026-10-12", "completed")], "ninguna pasada a medias ni `running`");
    assert!(day_rows(&app, "2026-10-13").await.is_empty(), "ningún registro parcial");

    sqlx::raw_sql("DROP TRIGGER inject_failure").execute(&app.state.db).await.unwrap();
    clock.inner.set(utc(15, 13, 41));
    wait_for(|| async { (db_runs(&app).await.len() == 3).then_some(()) }).await;
    let got = db_runs(&app).await;
    assert_eq!(
        got.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect::<Vec<_>>(),
        [("2026-10-12", "completed"), ("2026-10-13", "missed"), ("2026-10-14", "missed")],
        "las dos ventanas, completas y cerradas"
    );
    for day in ["2026-10-13", "2026-10-14"] {
        assert_eq!(day_rows(&app, day).await.len(), 2, "{day}: un `missed` por activo");
    }
    let (_, w) = app.get("/api/watchlist").await;
    assert_eq!(w["running"], false);
    stop(&state, handle).await;
}
