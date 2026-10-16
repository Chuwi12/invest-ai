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
    watchlist::{ManualClock, ManualPower, PowerProbe, RetryPolicy, SystemPower, WatchlistConfig, next_due, run_scheduler},
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
async fn does_not_run_before_its_time_then_runs_when_the_clock_gets_there() {
    let app = TestApp::new().await;
    let clock = ManualClock::new(utc(15, 8, 0), OFFSET); // 10:00 local
    let (state, handle) = start(&app, cfg("stock:AAPL"), clock.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runs(&app).await.is_empty());
    clock.set(utc(15, 20, 59)); // 22:59 local
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(runs(&app).await.is_empty());
    clock.set(utc(15, 21, 0));
    wait_for(|| async { runs(&app).await.iter().any(|r| r["status"] == "completed").then_some(()) }).await;
    assert_eq!(runs(&app).await[0]["trigger"], "schedule");
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
    sqlx::query("INSERT INTO watchlist_runs (local_date, scheduled_for, started_at, trigger, status, depth, planned) VALUES ('2026-10-14', 0, 0, 'schedule', 'running', 'standard', 3)")
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
