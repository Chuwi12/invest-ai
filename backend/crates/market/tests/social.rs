//! Reddit y X contra servidores axum locales (sin red real, sin claves). Los fixtures imitan la
//! estructura real de las respuestas: `data.children` (Reddit) y `data/includes/meta` (X).

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{
    Form, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Duration as Dur, Utc};
use domain::AssetClass;
use market::{
    http::build_client,
    providers::{reddit::Reddit, x::X},
    secret::Secret,
    social::{MemoryBudget, ReadBudget, RedditConfig, SocialProvider, SocialQuery, XConfig},
};
use std::collections::HashMap;

const REDDIT: &str = include_str!("fixtures/reddit_search_aapl.json");
const TOKEN: &str = include_str!("fixtures/reddit_token.json");
const XRESP: &str = include_str!("fixtures/x_search_aapl.json");

fn fill(t: &str) -> String {
    let now = Utc::now();
    let mut s = t.to_string();
    for (k, h) in [("H2", 2), ("H3", 3), ("H4", 4), ("H5", 5), ("H6", 6)] {
        s = s.replace(&format!("@AGE_{k}@"), &((now - Dur::hours(h)).timestamp()).to_string());
        s = s.replace(&format!("@ISO_{k}@"), &(now - Dur::hours(h)).format("%Y-%m-%dT%H:%M:%S.000Z").to_string());
    }
    s.replace("@AGE_D40@", &((now - Dur::days(40)).timestamp()).to_string())
}

#[derive(Default)]
struct Mock {
    token_calls: AtomicU32,
    search_calls: AtomicU32,
    x_calls: AtomicU32,
    /// 429 en las primeras N búsquedas de Reddit.
    reddit_429: AtomicU32,
    /// 401 la primera búsqueda con el token viejo.
    reddit_401_once: AtomicU32,
    seen_ua: Mutex<Vec<String>>,
    x_queries: Mutex<Vec<(String, String)>>,
    x_results_cap: AtomicU32,
    /// Rellena la respuesta hasta `max_results` posts (simula una búsqueda con muchos resultados).
    x_pad: std::sync::atomic::AtomicBool,
}

async fn token(State(m): State<Arc<Mock>>, headers: HeaderMap, Form(f): Form<HashMap<String, String>>) -> Response {
    m.token_calls.fetch_add(1, Ordering::SeqCst);
    // base64("test-id:test-secret")
    let ok = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some("Basic dGVzdC1pZDp0ZXN0LXNlY3JldA==");
    if !ok || f.get("grant_type").map(String::as_str) != Some("client_credentials") {
        return (StatusCode::UNAUTHORIZED, r#"{"error":"invalid_grant"}"#).into_response();
    }
    m.seen_ua.lock().unwrap().push(headers.get(header::USER_AGENT).unwrap().to_str().unwrap().to_string());
    TOKEN.into_response()
}

async fn rsearch(State(m): State<Arc<Mock>>, Path(subs): Path<String>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    m.search_calls.fetch_add(1, Ordering::SeqCst);
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer tok-reddit-0001") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if m.reddit_401_once.swap(0, Ordering::SeqCst) == 1 {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if m.reddit_429.load(Ordering::SeqCst) > 0 {
        m.reddit_429.fetch_sub(1, Ordering::SeqCst);
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "0")], "slow down").into_response();
    }
    m.seen_ua.lock().unwrap().push(headers.get(header::USER_AGENT).unwrap().to_str().unwrap().to_string());
    assert_eq!(q["restrict_sr"], "1");
    assert!(q["q"].contains("$AAPL"), "{q:?}");
    assert!(subs.contains("stocks") || subs.contains("SpainFIRE") || subs.contains("CryptoCurrency"), "{subs}");
    fill(REDDIT).into_response()
}

async fn xsearch(State(m): State<Arc<Mock>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    m.x_calls.fetch_add(1, Ordering::SeqCst);
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer x-token-123") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let max: u32 = q["max_results"].parse().unwrap();
    m.x_queries.lock().unwrap().push((q["query"].clone(), q["max_results"].clone()));
    m.x_results_cap.store(max, Ordering::SeqCst);
    // El servidor real nunca devuelve más de `max_results`.
    let mut v: serde_json::Value = serde_json::from_str(&fill(XRESP)).unwrap();
    let data = v["data"].as_array_mut().unwrap();
    if m.x_pad.load(Ordering::SeqCst) {
        let first = data[0].clone();
        while data.len() < max as usize {
            let mut c = first.clone();
            c["id"] = format!("18{:017}", data.len()).into();
            data.push(c);
        }
    }
    data.truncate(max as usize);
    v.to_string().into_response()
}

async fn serve() -> (SocketAddr, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    let app = Router::new()
        .route("/api/v1/access_token", post(token))
        .route("/r/{subs}/search", get(rsearch))
        .route("/2/tweets/search/recent", get(xsearch))
        .with_state(mock.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, mock)
}

fn reddit_cfg(addr: SocketAddr, id: &str, subs: Option<Vec<String>>) -> RedditConfig {
    RedditConfig {
        client_id: Secret::new(id).unwrap(),
        client_secret: Secret::new("test-secret").unwrap(),
        user_agent: "linux:invest-ai-test:0.1 (by /u/tester)".into(),
        subreddits: subs,
        api_base: format!("http://{addr}"),
        auth_base: format!("http://{addr}"),
    }
}

fn x_cfg(addr: SocketAddr, budget: u32) -> XConfig {
    XConfig {
        bearer: Secret::new("x-token-123").unwrap(),
        monthly_read_budget: budget,
        accounts: vec!["cuenta_uno".into(), "cuenta_dos".into()],
        search_all: false,
        langs: vec!["en".into(), "es".into()],
        results_per_request: 10,
        cost_per_read_usd: 0.005,
        api_base: format!("http://{addr}"),
    }
}

fn aapl() -> SocialQuery {
    SocialQuery { class: AssetClass::Stock, symbol: "AAPL".into(), name: Some("Apple Inc.".into()), days: 7 }
}

fn client() -> reqwest::Client {
    build_client(Duration::from_secs(5)).unwrap()
}

#[tokio::test]
async fn reddit_oauth_search_and_parsing() {
    let (addr, mock) = serve().await;
    let r = Reddit::new(client(), reddit_cfg(addr, "test-id", Some(vec!["stocks".into(), "investing".into()]))).with_backoff(Duration::from_millis(5));
    let batch = r.fetch(&aapl()).await.unwrap();
    // abc333 («Pineapple pizza») no menciona el activo; abc444 es de hace 40 días; abc555 está [removed].
    let ids: Vec<_> = batch.posts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["reddit:abc111", "reddit:abc222"], "{ids:?}");
    let p = &batch.posts[0];
    assert_eq!((p.community.as_str(), p.score, p.comments), ("stocks", 420, 87));
    assert_eq!(p.url, "https://www.reddit.com/r/stocks/comments/abc111/aapl_looks_strong/");
    assert_eq!(p.author.len(), 16);
    assert!(!p.author.contains("user"), "el autor va como seudónimo");
    assert!(p.created_at > Utc::now() - Dur::hours(3));
    // OAuth: un token, con credenciales y User-Agent correctos en AMBAS peticiones.
    assert_eq!(mock.token_calls.load(Ordering::SeqCst), 1);
    assert!(mock.seen_ua.lock().unwrap().iter().all(|u| u == "linux:invest-ai-test:0.1 (by /u/tester)"));
    // Caché: la segunda consulta no sale a la red.
    let before = mock.search_calls.load(Ordering::SeqCst);
    let again = r.fetch(&aapl()).await.unwrap();
    assert_eq!((again.posts.len(), again.requests), (2, 0));
    assert_eq!(mock.search_calls.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn reddit_bad_credentials_fail_without_leaking_them() {
    let (addr, _) = serve().await;
    let r = Reddit::new(client(), reddit_cfg(addr, "otro-id", None)).with_backoff(Duration::from_millis(5));
    let e = r.fetch(&aapl()).await.unwrap_err().to_string();
    assert!(e.contains("autenticación") && !e.contains("test-secret") && !e.contains("otro-id"), "{e}");
}

#[tokio::test]
async fn reddit_retries_429_and_renews_an_expired_token() {
    let (addr, mock) = serve().await;
    mock.reddit_429.store(2, Ordering::SeqCst);
    let r = Reddit::new(client(), reddit_cfg(addr, "test-id", Some(vec!["stocks".into()]))).with_backoff(Duration::from_millis(5));
    assert_eq!(r.fetch(&aapl()).await.unwrap().posts.len(), 2, "tras dos 429 sale bien");
    // Tres 429 seguidos agotan los reintentos.
    mock.reddit_429.store(5, Ordering::SeqCst);
    let q = SocialQuery { days: 6, ..aapl() }; // otra clave de caché
    assert!(r.fetch(&q).await.is_err());
    // 401: renueva el token una vez y reintenta.
    mock.reddit_429.store(0, Ordering::SeqCst);
    mock.reddit_401_once.store(1, Ordering::SeqCst);
    let calls = mock.token_calls.load(Ordering::SeqCst);
    let q = SocialQuery { days: 5, ..aapl() };
    assert_eq!(r.fetch(&q).await.unwrap().posts.len(), 2);
    assert_eq!(mock.token_calls.load(Ordering::SeqCst), calls + 1);
}

#[tokio::test]
async fn reddit_ambiguous_ticker_without_name_does_no_request() {
    let (addr, mock) = serve().await;
    let r = Reddit::new(client(), reddit_cfg(addr, "test-id", None));
    let q = SocialQuery { class: AssetClass::Stock, symbol: "AI".into(), name: None, days: 7 };
    let b = r.fetch(&q).await.unwrap();
    assert!(b.posts.is_empty());
    assert_eq!((mock.token_calls.load(Ordering::SeqCst), mock.search_calls.load(Ordering::SeqCst)), (0, 0));
}

#[tokio::test]
async fn x_query_filters_and_parsing() {
    let (addr, mock) = serve().await;
    let budget = Arc::new(MemoryBudget::default());
    let x = X::new(client(), x_cfg(addr, 1000), budget.clone());
    let b = x.fetch(&aapl()).await.unwrap();
    // El tercer post no lleva el cashtag: se descarta aunque la API lo devolviera.
    assert_eq!(b.posts.len(), 2);
    assert_eq!(b.posts[0].url, "https://x.com/cuenta_uno/status/1800000000000000001");
    assert_eq!((b.posts[0].score, b.posts[0].comments, b.posts[0].community.as_str()), (120, 4, "x"));
    let qs = mock.x_queries.lock().unwrap().clone();
    assert_eq!(qs.len(), 1, "las dos cuentas caben en una consulta");
    assert_eq!(qs[0].0, "$AAPL (from:cuenta_uno OR from:cuenta_dos) -is:retweet");
    // Se pagan los posts devueltos (3), no los reservados (10).
    assert_eq!((b.reads, budget.used(&Utc::now().format("%Y-%m").to_string()).await), (3, 3));
}

#[tokio::test]
async fn x_search_all_adds_filters() {
    let (addr, mock) = serve().await;
    let mut cfg = x_cfg(addr, 1000);
    cfg.accounts.clear();
    cfg.search_all = true;
    let x = X::new(client(), cfg, Arc::new(MemoryBudget::default()));
    x.fetch(&aapl()).await.unwrap();
    let qs = mock.x_queries.lock().unwrap().clone();
    assert_eq!(qs[0].0, "$AAPL -is:retweet (lang:en OR lang:es)");
}

#[tokio::test]
async fn x_stops_at_the_monthly_cap_and_never_exceeds_it() {
    let (addr, mock) = serve().await;
    mock.x_pad.store(true, Ordering::SeqCst);
    let budget = Arc::new(MemoryBudget::default());
    // Tope 25 con peticiones de 10: caben 2 completas; la tercera tendría 5 (< 10 mínimo): se corta.
    let x = X::new(client(), x_cfg(addr, 25), budget.clone());
    let month = Utc::now().format("%Y-%m").to_string();
    let mut total_reads = 0;
    for (i, d) in [7u32, 6, 5, 4].into_iter().enumerate() {
        // días distintos = claves de caché distintas, para forzar peticiones reales
        let b = x.fetch(&SocialQuery { days: d, ..aapl() }).await.unwrap();
        total_reads += b.reads;
        if i >= 2 {
            assert!(b.budget_exhausted && b.requests == 0, "pasado el tope no hay peticiones");
        }
    }
    assert_eq!(mock.x_calls.load(Ordering::SeqCst), 2, "solo 2 peticiones llegaron a X");
    assert_eq!(total_reads, 20, "2 peticiones de 10 posts");
    assert!(budget.used(&month).await <= 25);
    // Lo pedido a X nunca superó el saldo.
    assert!(mock.x_results_cap.load(Ordering::SeqCst) <= 10);
}

#[tokio::test]
async fn x_never_requests_more_than_the_remaining_budget() {
    let (addr, mock) = serve().await;
    let budget = Arc::new(MemoryBudget::default());
    let month = Utc::now().format("%Y-%m").to_string();
    budget.reserve(&month, 95, 100).await.unwrap(); // quedan 5 < 10
    let x = X::new(client(), x_cfg(addr, 100), budget.clone());
    let b = x.fetch(&aapl()).await.unwrap();
    assert!(b.budget_exhausted && b.posts.is_empty());
    assert_eq!(mock.x_calls.load(Ordering::SeqCst), 0);
    assert_eq!(budget.used(&month).await, 95, "la reserva fallida se devuelve");
}

#[tokio::test]
async fn x_error_refunds_the_reservation() {
    let (addr, _) = serve().await;
    let mut cfg = x_cfg(addr, 100);
    cfg.bearer = Secret::new("token-malo").unwrap();
    let budget = Arc::new(MemoryBudget::default());
    let x = X::new(client(), cfg, budget.clone());
    assert!(x.fetch(&aapl()).await.is_err());
    assert_eq!(budget.used(&Utc::now().format("%Y-%m").to_string()).await, 0);
}
