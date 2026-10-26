//! Pruebas con SQLite en memoria y un mercado simulado (sin red).

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::*;
use futures::{StreamExt, stream::BoxStream};
use sqlx::SqlitePool;

use crate::{
    LearningConfig, report::performance_report, resolve::ResolveConfig, resolve_pending,
    rules::RULE_VERSION, run_cycle, weights,
};

struct Stub {
    hist: Mutex<HashMap<AssetId, Result<Vec<Candle>, DomainError>>>,
    calls: Mutex<Vec<String>>,
}

impl Stub {
    fn new() -> Self {
        Self {
            hist: Mutex::new(HashMap::new()),
            calls: Mutex::new(vec![]),
        }
    }
    fn set(&self, id: &AssetId, c: Result<Vec<Candle>, DomainError>) {
        self.hist.lock().unwrap().insert(id.clone(), c);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn nope<T>() -> Result<T> {
    Err(DomainError::Unavailable("stub".into()))
}

#[async_trait]
impl MarketData for Stub {
    async fn search(&self, _: &str, _: usize) -> Result<Vec<AssetSummary>> {
        nope()
    }
    async fn quote(&self, _: &AssetId) -> Result<Quote> {
        nope()
    }
    async fn quotes(&self, _: &[AssetId]) -> Result<Vec<Quote>> {
        nope()
    }
    async fn history(&self, id: &AssetId, range: Range, interval: Interval) -> Result<Vec<Candle>> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{id} {} {}", range.as_str(), interval.as_str()));
        match self.hist.lock().unwrap().get(id) {
            Some(Ok(c)) => Ok(c.clone()),
            Some(Err(DomainError::NotFound(m))) => Err(DomainError::NotFound(m.clone())),
            Some(Err(_)) => Err(DomainError::Upstream {
                provider: "stub".into(),
                message: "caído".into(),
            }),
            None => Err(DomainError::NotFound(id.to_string())),
        }
    }
    async fn fundamentals(&self, _: &AssetId) -> Result<Option<Fundamentals>> {
        nope()
    }
    async fn indicators(&self, _: &AssetId) -> Result<Indicators> {
        nope()
    }
    async fn news(&self, _: Option<&AssetId>, _: usize) -> Result<Vec<NewsItem>> {
        nope()
    }
    async fn macro_context(&self) -> Result<MacroContext> {
        nope()
    }
    async fn snapshot(&self, _: &AssetId) -> Result<AssetSnapshot> {
        nope()
    }
    fn subscribe(&self, _: &[AssetId]) -> BoxStream<'static, PriceTick> {
        futures::stream::empty().boxed()
    }
    fn providers_status(&self) -> Vec<ProviderStatus> {
        vec![]
    }
}

async fn db() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        include_str!("../../server/migrations/0001_init.sql"),
        include_str!("../../server/migrations/0005_learning.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn analysis(id: &str, asset: &str, verdict: Verdict, conf: f32, at: DateTime<Utc>) -> Analysis {
    Analysis {
        id: id.into(),
        asset: asset.parse().unwrap(),
        verdict,
        confidence: conf,
        horizon: Horizon::Medium,
        scores: ScoreBreakdown {
            overall: 60.0,
            technical: Some(70.0),
            momentum: Some(65.0),
            fundamental: None,
            valuation: Some(40.0),
            sentiment: Some(55.0),
            macro_score: None,
            risk: Some(30.0),
        },
        summary: String::new(),
        thesis: String::new(),
        bull_case: vec![],
        bear_case: vec![],
        risks: vec![],
        catalysts: vec![],
        key_levels: KeyLevels::default(),
        strategy: None,
        data_warnings: vec![],
        sources: vec![],
        price_at_analysis: Some(100.0),
        model: "qwen2.5:7b".into(),
        generated_at: at,
        levels_note: None,
        scenarios: None,
        expected_return_pct: None,
        would_change_if: None,
        horizon_note: None,
        position_sizing: None,
        method: None,
        quality_notes: None,
        depth: None,
        duration_ms: None,
        llm_calls: None,
        prompt_tokens: None,
        completion_tokens: None,
    }
}

async fn insert(pool: &SqlitePool, a: &Analysis) {
    sqlx::query(
        "INSERT INTO analyses (id, asset_id, verdict, confidence, price_at_analysis, model, origin, generated_at, json)
         VALUES (?1,?2,?3,?4,?5,?6,'analyze',?7,?8)",
    )
    .bind(&a.id)
    .bind(a.asset.to_string())
    .bind(crate::rules::verdict_str(a.verdict))
    .bind(f64::from(a.confidence))
    .bind(a.price_at_analysis)
    .bind(&a.model)
    .bind(a.generated_at.timestamp_millis())
    .bind(serde_json::to_string(a).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

/// Velas diarias (lun-dom) desde `start`, crecimiento compuesto diario `g`.
fn series(start: DateTime<Utc>, days: usize, g: f64) -> Vec<Candle> {
    let d0 = start.timestamp().div_euclid(86_400);
    (0..days)
        .map(|i| {
            let c = 100.0 * (1.0 + g).powi(i as i32);
            Candle {
                t: (d0 + i as i64) * 86_400 + 13 * 3600,
                o: c,
                h: c,
                l: c,
                c,
                v: None,
            }
        })
        .collect()
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
}

#[tokio::test]
async fn resolves_idempotently_with_one_request_per_asset_and_benchmark() {
    let pool = db().await;
    let m = Stub::new();
    let t_old = now() - Duration::days(40);
    let aapl = "stock:AAPL";
    insert(&pool, &analysis("a1", aapl, Verdict::Buy, 0.7, t_old)).await;
    insert(
        &pool,
        &analysis("a2", aapl, Verdict::Avoid, 0.6, t_old + Duration::days(1)),
    )
    .await;
    insert(
        &pool,
        &analysis("a3", aapl, Verdict::Hold, 0.5, now() - Duration::days(3)),
    )
    .await;
    insert(
        &pool,
        &analysis("a4", "stock:SAN.MC", Verdict::Buy, 0.5, t_old),
    )
    .await;
    insert(
        &pool,
        &analysis("a5", "crypto:BTC", Verdict::Buy, 0.5, t_old),
    )
    .await;
    insert(
        &pool,
        &analysis("a6", "crypto:ETH", Verdict::Buy, 0.5, t_old),
    )
    .await;
    let start = now() - Duration::days(60);
    m.set(&aapl.parse().unwrap(), Ok(series(start, 61, 0.004))); // +0,4 %/día
    m.set(&"etf:SPY".parse().unwrap(), Ok(series(start, 61, 0.001)));
    m.set(&"crypto:BTC".parse().unwrap(), Ok(series(start, 61, -0.01)));
    m.set(&"crypto:ETH".parse().unwrap(), Ok(series(start, 61, -0.02)));

    let cfg = ResolveConfig::default();
    let r = resolve_pending(&pool, &m, now(), &cfg).await.unwrap();
    println!("resolve: {r:?}; llamadas: {:?}", m.calls());
    // Maduros: a1,a2 (1 y 4 s), a4 (skipped x2), a5, a6 (1 y 4 s); a3 no ha madurado a 1 semana.
    assert_eq!(r.resolved, 8);
    assert_eq!(r.skipped, 2);
    let calls = m.calls();
    // AAPL, SPY, BTC (absoluto: sin benchmark), ETH: 4 peticiones, sin repetir ninguna.
    assert_eq!(calls.len(), 4, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|c| c.ends_with("3mo 1d") || c.ends_with("1y 1d")),
        "{calls:?}"
    );

    let row: (f64, f64, f64, i64, String) = sqlx::query_as(
        "SELECT asset_return, bench_return, excess_return, hit, bench_kind FROM analysis_outcomes WHERE analysis_id='a1' AND horizon_weeks=4",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((row.0 - (1.004f64.powi(28) - 1.0)).abs() < 1e-9);
    assert!((row.2 - (row.0 - row.1)).abs() < 1e-12);
    assert_eq!((row.3, row.4.as_str()), (1, "relative"));
    // Evitar con exceso positivo = fallo.
    let (hit,): (i64,) = sqlx::query_as(
        "SELECT hit FROM analysis_outcomes WHERE analysis_id='a2' AND horizon_weeks=4",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hit, 0);
    // BTC es su propio índice: exceso = rentabilidad absoluta (negativa), así que comprar falla.
    let (kind, ex, hit): (String, f64, i64) =
        sqlx::query_as("SELECT bench_kind, excess_return, hit FROM analysis_outcomes WHERE analysis_id='a5' AND horizon_weeks=4").fetch_one(&pool).await.unwrap();
    assert!(kind == "absolute" && ex < 0.0 && hit == 0);
    // Factores guardados con el riesgo invertido.
    let (f,): (String,) = sqlx::query_as(
        "SELECT factors FROM analysis_outcomes WHERE analysis_id='a1' AND horizon_weeks=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(f, "[70.0,65.0,40.0,null,55.0,null,70.0]");

    // Idempotente y barato: segunda pasada, cero trabajo y cero peticiones.
    let before = m.calls().len();
    let r2 = resolve_pending(&pool, &m, now(), &cfg).await.unwrap();
    assert_eq!((r2.pending_pairs, r2.resolved, r2.history_calls), (0, 0, 0));
    assert_eq!(m.calls().len(), before);

    // Panel.
    let rep = performance_report(&pool, now(), None, &Default::default())
        .await
        .unwrap();
    let h4 = rep.horizons.iter().find(|h| h.horizon_weeks == 4).unwrap();
    assert_eq!(h4.analyses_total, 6);
    assert_eq!(h4.resolved, 4);
    assert_eq!(h4.skipped, 1);
    assert_eq!(h4.pending, 0);
    assert_eq!(h4.not_matured, 1);
    assert!(!h4.calibration.applied);
    assert_eq!(rep.rule_version, RULE_VERSION);
}

#[tokio::test]
async fn failures_are_retried_with_backoff_and_resumed() {
    let pool = db().await;
    let m = Stub::new();
    insert(
        &pool,
        &analysis(
            "b1",
            "stock:MSFT",
            Verdict::Buy,
            0.7,
            now() - Duration::days(40),
        ),
    )
    .await;
    m.set(
        &"stock:MSFT".parse().unwrap(),
        Err(DomainError::Unavailable("x".into())),
    );
    m.set(
        &"etf:SPY".parse().unwrap(),
        Ok(series(now() - Duration::days(60), 61, 0.0)),
    );
    let cfg = ResolveConfig::default();
    let r = resolve_pending(&pool, &m, now(), &cfg).await.unwrap();
    assert_eq!((r.resolved, r.retry), (0, 2));
    // Dentro de la espera de reintento no se vuelve a pedir nada.
    let calls = m.calls().len();
    let r = resolve_pending(&pool, &m, now() + Duration::hours(1), &cfg)
        .await
        .unwrap();
    assert_eq!((r.pending_pairs, m.calls().len()), (0, calls));
    // Cuando el proveedor se recupera y pasa la espera, se reanuda.
    m.set(
        &"stock:MSFT".parse().unwrap(),
        Ok(series(now() - Duration::days(60), 61, 0.002)),
    );
    let later = now() + Duration::days(2);
    let r = resolve_pending(&pool, &m, later, &cfg).await.unwrap();
    assert_eq!(r.resolved, 2);
    let (st,): (String,) = sqlx::query_as(
        "SELECT status FROM analysis_outcomes WHERE analysis_id='b1' AND horizon_weeks=4",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(st, "resolved");
}

#[tokio::test]
async fn deleting_an_analysis_keeps_its_outcome() {
    let pool = db().await;
    let m = Stub::new();
    insert(
        &pool,
        &analysis(
            "c1",
            "stock:MSFT",
            Verdict::Buy,
            0.7,
            now() - Duration::days(40),
        ),
    )
    .await;
    m.set(
        &"stock:MSFT".parse().unwrap(),
        Ok(series(now() - Duration::days(60), 61, 0.002)),
    );
    m.set(
        &"etf:SPY".parse().unwrap(),
        Ok(series(now() - Duration::days(60), 61, 0.0)),
    );
    resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    sqlx::query("DELETE FROM analyses")
        .execute(&pool)
        .await
        .unwrap();
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM analysis_outcomes WHERE status='resolved'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn weights_are_versioned_adopted_activated_and_reversible() {
    use crate::{stats::Rng, store::N_FACTORS};
    let pool = db().await;
    // 1500 resultados sintéticos (momentum predice con corr 0,2), 4 semanas.
    let mut r = Rng::new(42);
    let week = 7 * 86_400_000i64;
    let t0 = now().timestamp_millis() - 140 * week;
    for w in 0..120i64 {
        for k in 0..12i64 {
            let z: Vec<f64> = (0..N_FACTORS).map(|_| r.normal()).collect();
            let f: Vec<Option<f64>> = z
                .iter()
                .map(|x| Some((50.0 + 15.0 * x).clamp(0.0, 100.0)))
                .collect();
            let ex = 0.07 * (0.35 * z[1] + (1.0f64 - 0.1225).sqrt() * r.normal());
            sqlx::query(
                "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
                   generated_at, status, rule_version, bench_kind, excess_return, hit, factors, resolved_at)
                 VALUES (?1, 4, ?2, 'stock', 'buy', 0.6, 'm', ?3, 'resolved', ?4, 'relative', ?5, ?6, ?7, 0)",
            )
            .bind(format!("s{w}-{k}"))
            .bind(format!("stock:S{}", (w * 12 + k) % 60))
            .bind(t0 + w * week + k * 3_600_000)
            .bind(RULE_VERSION)
            .bind(ex)
            .bind(i64::from(ex > 0.0))
            .bind(serde_json::to_string(&f).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    assert!(ai::scoring::learned_multipliers().is_none());
    assert!(weights::active_version(&pool).await.unwrap().is_none());
    let cfg = LearningConfig::default();
    let m = Stub::new();
    let cycle = run_cycle(&pool, &m, now(), &cfg).await.unwrap();
    let rep = cycle.weights.report.unwrap();
    println!("ciclo de pesos: {}", rep.reason);
    let v = cycle
        .weights
        .adopted_version
        .expect("adopta con señal real y muestra suficiente");
    let active = weights::active_version(&pool).await.unwrap().unwrap();
    assert_eq!(
        (active.version, active.kind.as_str(), active.status.as_str()),
        (v, "learned", "active")
    );
    assert!(active.multipliers[1] > 1.3 && active.n_val.unwrap() >= 80);
    assert_eq!(ai::scoring::learned_multipliers(), Some(active.multipliers));
    // Sin filas nuevas, el siguiente ciclo no vuelve a evaluar (espera datos).
    let again = run_cycle(&pool, &m, now(), &cfg).await.unwrap();
    assert!(!again.weights.ran);
    // Vuelta atrás: nueva versión (la historia se conserva) con los pesos por defecto.
    let back = rollback(&pool, None, now()).await.unwrap();
    assert_eq!(
        (back.kind.as_str(), back.multipliers),
        ("rollback", [1.0; 7])
    );
    assert_eq!(weights::list_versions(&pool).await.unwrap().len(), 2);
    assert_eq!(ai::scoring::learned_multipliers(), Some([1.0; 7]));
    let (n_active,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM scoring_weights WHERE status='active'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n_active, 1);
    ai::scoring::set_learned_multipliers(None);
}

use crate::rollback;

#[tokio::test]
async fn calibrated_confidence_uses_only_the_requested_model() {
    use crate::calibrated_confidence;
    let pool = db().await;
    let t0 = now().timestamp_millis() - 100 * 86_400_000;
    // 250 resultados independientes (un activo distinto cada uno) del modelo «a» y 7 del «b».
    for (model, count) in [("a", 250i64), ("b", 7)] {
        for i in 0..count {
            sqlx::query(
                "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
                   generated_at, status, rule_version, bench_kind, excess_return, hit, resolved_at)
                 VALUES (?1, 4, ?2, 'stock', 'buy', 0.6, ?3, ?4, 'resolved', ?5, 'relative', 0.01, 1, 0)",
            )
            .bind(format!("{model}{i}"))
            .bind(format!("stock:{model}{i}"))
            .bind(model)
            .bind(t0 + i * 3_600_000)
            .bind(RULE_VERSION)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    let n_of = |m: Option<&str>| {
        let cfg = LearningConfig { active_model: m.map(String::from), ..Default::default() };
        let pool = pool.clone();
        async move { calibrated_confidence(&pool, 0.6, 4, &cfg).await.unwrap().n }
    };
    assert_eq!(n_of(Some("a")).await, 250);
    assert_eq!(n_of(Some("b")).await, 7);
    assert_eq!(n_of(Some("otro")).await, 0);
    assert_eq!(n_of(None).await, 257, "sin modelo se mezclan todos");
}

// ---------------------------------------------------------------------------------------------
// Regla de entrada v2 y cambios de versión (fechas concretas; `now()` = miércoles 14-oct-2026 12:00 UTC)
// ---------------------------------------------------------------------------------------------

const DAY_S: i64 = 86_400;

fn day_no(y: i32, m: u32, d: u32) -> i64 {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0)
        .unwrap()
        .timestamp()
        .div_euclid(DAY_S)
}

/// Velas de bolsa (solo lunes a viernes) desde `from`, con `t` = apertura en horario de verano (13:30 UTC)
/// y cierre = 100 + días desde `from`: el precio de entrada dice qué sesión se eligió.
fn weekday_series(from: i64, days: i64) -> Vec<Candle> {
    (0..days)
        .map(|i| from + i)
        .filter(|d| !matches!((d + 3).rem_euclid(7), 5 | 6)) // 1970-01-01 fue jueves
        .map(|d| {
            let c = 100.0 + (d - from) as f64;
            Candle {
                t: d * DAY_S + 13 * 3600 + 1800,
                o: c,
                h: c,
                l: c,
                c,
                v: None,
            }
        })
        .collect()
}

/// Mercado simulado: AAPL (laborables, cierre = 100 + día), SPY plano, BTC y ETH (todos los días).
fn market_2026() -> (Stub, i64) {
    let m = Stub::new();
    let from = day_no(2026, 8, 15);
    m.set(&"stock:AAPL".parse().unwrap(), Ok(weekday_series(from, 62)));
    m.set(
        &"etf:SPY".parse().unwrap(),
        Ok(weekday_series(from, 62)
            .into_iter()
            .map(|c| Candle {
                o: 100.0,
                h: 100.0,
                l: 100.0,
                c: 100.0,
                ..c
            })
            .collect()),
    );
    let start = Utc.timestamp_opt(from * DAY_S, 0).unwrap();
    m.set(&"crypto:BTC".parse().unwrap(), Ok(series(start, 62, 0.0)));
    m.set(&"crypto:ETH".parse().unwrap(), Ok(series(start, 62, 0.001)));
    (m, from)
}

/// (día de la entrada, precio de entrada) de una fila resuelta.
async fn entry_of(pool: &SqlitePool, id: &str, horizon: i64) -> (i64, f64) {
    let (ts, price): (i64, f64) = sqlx::query_as(
        "SELECT entry_ts, entry_price FROM analysis_outcomes WHERE analysis_id = ?1 AND horizon_weeks = ?2 AND status = 'resolved'",
    )
    .bind(id)
    .bind(horizon)
    .fetch_one(pool)
    .await
    .unwrap();
    (ts.div_euclid(DAY_S), price)
}

#[tokio::test]
async fn analysis_before_the_open_is_measured_from_the_previous_close() {
    let pool = db().await;
    let (m, from) = market_2026();
    // Martes 15-sep-2026 a las 07:15 UTC (09:15 en Madrid): EE. UU. abre a las 13:30 UTC.
    let tue = Utc.with_ymd_and_hms(2026, 9, 15, 7, 15, 0).unwrap();
    insert(
        &pool,
        &analysis("aapl", "stock:AAPL", Verdict::Buy, 0.7, tue),
    )
    .await;
    insert(
        &pool,
        &analysis("eth", "crypto:ETH", Verdict::Buy, 0.7, tue),
    )
    .await;
    let r = resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    // AAPL a 1 y 4 semanas y ETH a 1; la salida de ETH a 4 semanas (13-oct) aún no tiene la vela completa
    // (las velas del simulador llevan la hora de las 13:00 UTC y ahora son las 12:00 del 14-oct).
    assert_eq!((r.resolved, r.skipped, r.reopened, r.not_yet), (3, 0, 0, 1));

    // La acción entra en el cierre del lunes 14 (lo que vio el análisis) y sale una semana después.
    let (entry_day, price) = entry_of(&pool, "aapl", 1).await;
    assert_eq!(entry_day, day_no(2026, 9, 14));
    assert_eq!(price, 100.0 + (day_no(2026, 9, 14) - from) as f64);
    let (exit_ts,): (i64,) = sqlx::query_as(
        "SELECT exit_ts FROM analysis_outcomes WHERE analysis_id='aapl' AND horizon_weeks=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exit_ts.div_euclid(DAY_S), day_no(2026, 9, 21));
    // 4 semanas: del lunes 14-sep al lunes 12-oct.
    let (ts4,): (i64,) = sqlx::query_as(
        "SELECT exit_ts FROM analysis_outcomes WHERE analysis_id='aapl' AND horizon_weeks=4",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ts4.div_euclid(DAY_S), day_no(2026, 10, 12));
    // El benchmark entra en el mismo día que el activo.
    let (be,): (f64,) = sqlx::query_as("SELECT bench_entry_price FROM analysis_outcomes WHERE analysis_id='aapl' AND horizon_weeks=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(be, 100.0);

    // Cripto (24/7): el cierre del día UTC del análisis (martes 15), no el del lunes.
    assert_eq!(entry_of(&pool, "eth", 1).await.0, day_no(2026, 9, 15));
}

#[tokio::test]
async fn the_time_the_analysis_took_moves_the_reference_instant_before_the_open() {
    let pool = db().await;
    let (m, _) = market_2026();
    // Termina el martes 15-sep a las 13:34 UTC, cuatro minutos después de abrir EE. UU. (13:30), pero tardó
    // 6 minutos: vio los datos a las 13:28, con el mercado cerrado. Entra en el cierre del lunes 14.
    let end = Utc.with_ymd_and_hms(2026, 9, 15, 13, 34, 0).unwrap();
    let mut slow = analysis("slow", "stock:AAPL", Verdict::Buy, 0.7, end);
    slow.duration_ms = Some(6 * 60_000);
    insert(&pool, &slow).await;
    // Sin duración (análisis antiguo): se usa `generated_at`, ya con la sesión abierta -> cierre del martes 15.
    insert(
        &pool,
        &analysis("nodur", "stock:AAPL", Verdict::Buy, 0.7, end),
    )
    .await;
    // Una duración absurda (10 h) no se cree.
    let mut absurd = analysis("absurd", "stock:AAPL", Verdict::Buy, 0.7, end);
    absurd.duration_ms = Some(10 * 3_600_000);
    insert(&pool, &absurd).await;
    resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    assert_eq!(entry_of(&pool, "slow", 1).await.0, day_no(2026, 9, 14));
    assert_eq!(entry_of(&pool, "nodur", 1).await.0, day_no(2026, 9, 15));
    assert_eq!(entry_of(&pool, "absurd", 1).await.0, day_no(2026, 9, 15));
}

#[tokio::test]
async fn analysis_with_the_market_open_is_measured_from_that_session_close() {
    let pool = db().await;
    let (m, from) = market_2026();
    // Miércoles 16-sep-2026 a las 15:00 UTC (11:00 en Nueva York), mercado abierto.
    let wed = Utc.with_ymd_and_hms(2026, 9, 16, 15, 0, 0).unwrap();
    insert(
        &pool,
        &analysis("open", "stock:AAPL", Verdict::Buy, 0.7, wed),
    )
    .await;
    resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    let (entry_day, price) = entry_of(&pool, "open", 1).await;
    assert_eq!(entry_day, day_no(2026, 9, 16));
    assert_eq!(price, 100.0 + (day_no(2026, 9, 16) - from) as f64);
}

/// Fila antigua (regla de otra versión) ya resuelta, con cifras que no coinciden con las de la regla vigente.
async fn insert_old_outcome(
    pool: &SqlitePool,
    a: &Analysis,
    horizon: i64,
    status: &str,
    version: i64,
) {
    sqlx::query(
        "INSERT INTO analysis_outcomes (analysis_id, horizon_weeks, asset_id, asset_class, verdict, confidence, model,
           generated_at, status, note, rule_version, bench_kind, entry_ts, entry_price, excess_return, hit, factors, overall, resolved_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'relative',?12,?13,?14,?15,?16,?17,0)",
    )
    .bind(&a.id)
    .bind(horizon)
    .bind(a.asset.to_string())
    .bind(a.asset.class.as_str())
    .bind(crate::rules::verdict_str(a.verdict))
    .bind(f64::from(a.confidence))
    .bind(&a.model)
    .bind(a.generated_at.timestamp_millis())
    .bind(status)
    .bind((status == "skipped").then_some("sin benchmark definido para este activo"))
    .bind(version)
    .bind((status == "resolved").then_some(1_i64))
    .bind((status == "resolved").then_some(7.0_f64))
    .bind((status == "resolved").then_some(0.5_f64))
    .bind((status == "resolved").then_some(1_i64))
    .bind("[1.0,2.0,3.0,null,5.0,null,7.0]")
    .bind(42.0_f64)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_rule_version_bump_relabels_history_instead_of_mixing_versions() {
    let pool = db().await;
    let (m, _) = market_2026();
    let tue = Utc.with_ymd_and_hms(2026, 9, 15, 7, 15, 0).unwrap();
    let a = analysis("old", "stock:AAPL", Verdict::Buy, 0.7, tue);
    let spain = analysis("old-es", "stock:SAN.MC", Verdict::Buy, 0.7, tue);
    insert(&pool, &a).await;
    insert(&pool, &spain).await;
    let v_old = RULE_VERSION - 1;
    for h in [1, 4] {
        insert_old_outcome(&pool, &a, h, "resolved", v_old).await;
        insert_old_outcome(&pool, &spain, h, "skipped", v_old).await;
    }

    // Antes de recalcular: las filas de la versión anterior no entran en ninguna métrica...
    assert!(
        crate::store::resolved_rows(&pool, 1)
            .await
            .unwrap()
            .is_empty()
    );
    let rep = performance_report(&pool, now(), None, &Default::default())
        .await
        .unwrap();
    let h1 = rep.horizons.iter().find(|h| h.horizon_weeks == 1).unwrap();
    assert_eq!(
        (h1.resolved, h1.skipped, h1.resolved_independent),
        (0, 0, 0)
    );
    assert!(h1.overall.hit_rate.is_none() && h1.base_rate_outperform.is_none());
    // ...y cuentan como pendientes de resolver (maduras sin resultado vigente), no como resueltas.
    assert_eq!(h1.pending, 2);

    // La pasada las reabre y las recalcula con la regla vigente (entrada = cierre del lunes 14).
    let r = resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    assert_eq!((r.reopened, r.resolved, r.skipped), (4, 2, 2), "{r:?}");
    let (ver, n_old): (i64, i64) =
        sqlx::query_as("SELECT MIN(rule_version), SUM(rule_version < ?1) FROM analysis_outcomes")
            .bind(RULE_VERSION)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (ver, n_old),
        (RULE_VERSION, 0),
        "ninguna fila queda con la versión antigua"
    );
    assert_eq!(
        entry_of(&pool, "old", 1).await,
        (
            day_no(2026, 9, 14),
            100.0 + (day_no(2026, 9, 14) - day_no(2026, 8, 15)) as f64
        )
    );
    let (ex, factors, overall, note): (f64, String, f64, Option<String>) = sqlx::query_as(
        "SELECT excess_return, factors, overall, note FROM analysis_outcomes WHERE analysis_id='old' AND horizon_weeks=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (ex - 0.5).abs() > 1e-6,
        "la cifra de la regla antigua no sobrevive: {ex}"
    );
    assert_eq!(note, None);
    // Los factores se releen del análisis (no se pierden ni se heredan de la fila antigua).
    assert_eq!(factors, "[70.0,65.0,40.0,null,55.0,null,70.0]");
    assert_eq!(overall, 60.0);
    // El activo sin benchmark vuelve a quedar descartado, sin pedir su histórico.
    let (st, note): (String, Option<String>) = sqlx::query_as(
        "SELECT status, note FROM analysis_outcomes WHERE analysis_id='old-es' AND horizon_weeks=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(st, "skipped");
    assert!(note.unwrap().contains("sin benchmark"));
    assert!(
        m.calls().iter().all(|c| !c.contains("SAN.MC")),
        "{:?}",
        m.calls()
    );

    // El panel ya ve las dos filas resueltas de la versión vigente, y nada pendiente.
    let rep = performance_report(&pool, now(), None, &Default::default())
        .await
        .unwrap();
    let h1 = rep.horizons.iter().find(|h| h.horizon_weeks == 1).unwrap();
    assert_eq!((h1.resolved, h1.skipped, h1.pending), (1, 1, 0));
    assert_eq!(
        crate::store::resolved_rows(&pool, 1).await.unwrap().len(),
        1
    );

    // Idempotente: otra pasada no reabre ni pide nada.
    let calls = m.calls().len();
    let again = resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    assert_eq!(
        (again.reopened, again.pending_pairs, again.history_calls),
        (0, 0, 0)
    );
    assert_eq!(m.calls().len(), calls);
}

#[tokio::test]
async fn a_rule_version_bump_also_recomputes_rows_of_deleted_analyses() {
    let pool = db().await;
    let (m, _) = market_2026();
    let tue = Utc.with_ymd_and_hms(2026, 9, 15, 7, 15, 0).unwrap();
    let a = analysis("gone", "stock:AAPL", Verdict::Avoid, 0.6, tue);
    // Sin fila en `analyses` (el usuario la borró): la fila de resultados es lo único que queda.
    for h in [1, 4] {
        insert_old_outcome(&pool, &a, h, "resolved", RULE_VERSION - 1).await;
    }
    let r = resolve_pending(&pool, &m, now(), &ResolveConfig::default())
        .await
        .unwrap();
    assert_eq!((r.reopened, r.resolved), (2, 2), "{r:?}");
    let (version, factors, overall, hit): (i64, String, f64, i64) = sqlx::query_as(
        "SELECT rule_version, factors, overall, hit FROM analysis_outcomes WHERE analysis_id='gone' AND horizon_weeks=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(version, RULE_VERSION);
    // Sin análisis que releer, se conservan los factores guardados.
    assert_eq!(factors, "[1.0,2.0,3.0,null,5.0,null,7.0]");
    assert_eq!(overall, 42.0);
    // «Evitar» con el activo subiendo frente a un SPY plano: fallo.
    assert_eq!(hit, 0);
    assert_eq!(entry_of(&pool, "gone", 1).await.0, day_no(2026, 9, 14));
}
