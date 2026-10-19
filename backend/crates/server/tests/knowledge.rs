//! Cableado del RAG en el servidor: ruta de estadísticas, archivado de noticias y folletos sin
//! peticiones extra, pausa durante las generaciones y arranque tolerante a fallos. Sin Ollama:
//! embedder simulado (`FakeEmbedder`).

mod common;

use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use common::TestApp;
use knowledge::{DocKind, EmbedKind, Embedder, FakeEmbedder, KnowledgeConfig, SearchRequest};
use serde_json::json;
use server::knowledge::{Activity, GatedEmbedder, Rag, TrackedAnalyst, open_with, run_maintenance};
use tokio_util::sync::CancellationToken;

/// App con el RAG cableado igual que en `main`, pero con embedder simulado.
async fn app_with_rag() -> (TestApp, Arc<knowledge::KnowledgeBase>, CancellationToken) {
    let mut app = TestApp::new().await;
    let shutdown = CancellationToken::new();
    let rag: Rag = open_with(KnowledgeConfig::default(), app.state.db.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.expect("archivo abierto");
    let kb = rag.kb.clone();
    let (market, ipo, analyst) = rag.wrap(app.state.market.clone(), app.state.ipo.clone(), app.state.analyst.clone());
    let mut state = app.state.clone().with_knowledge(Some(kb.clone()));
    (state.market, state.ipo, state.analyst) = (market, ipo, analyst);
    rag.spawn_background(&state.tasks, shutdown.clone());
    app.router = server::app(state.clone(), None);
    app.state = state;
    (app, kb, shutdown)
}

async fn wait_for(mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("la condición no se cumplió a tiempo");
}

#[tokio::test]
async fn stats_route_reports_disabled_without_rag() {
    let app = TestApp::new().await;
    let (s, body) = app.get("/api/knowledge/stats").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, json!({ "enabled": false }));
    assert!(app.state.knowledge.is_none());
}

#[tokio::test]
async fn news_and_prospectuses_that_pass_through_are_archived_without_extra_requests() {
    let (app, kb, shutdown) = app_with_rag().await;
    let (s, body) = app.get("/api/knowledge/stats").await;
    assert_eq!((s, &body["enabled"], &body["documents"]), (StatusCode::OK, &json!(true), &json!(0)));
    for k in ["model", "chunks", "indexed", "pending", "textBytes", "embeddingBytes", "indexMemoryBytes"] {
        assert!(body.get(k).is_some(), "falta {k}: {body}");
    }

    use std::sync::atomic::Ordering;
    let before = app.market.snapshot_calls.load(Ordering::SeqCst);
    let (s, snap) = app.get("/api/assets/stock:AAPL/snapshot").await;
    assert_eq!(s, StatusCode::OK, "{snap}");
    wait_for(async || kb.stats().await.unwrap().documents > 0).await;
    assert_eq!(app.market.snapshot_calls.load(Ordering::SeqCst) - before, 1, "archivar noticias no hace peticiones extra al proveedor");
    let (s, news) = app.get("/api/assets/stock:AAPL/news").await;
    assert_eq!(s, StatusCode::OK, "{news}");

    let (s, p) = app.get("/api/ipo/ORFS/prospectus").await;
    assert_eq!(s, StatusCode::OK, "{p}");
    wait_for(async || {
        kb.search(&SearchRequest { kinds: vec![DocKind::Filing], k: 3, include_general: true, ..SearchRequest::new("risk factors") }).await.is_ok_and(|r| !r.hits.is_empty())
    })
    .await;
    let r = kb.search(&SearchRequest { kinds: vec![DocKind::Filing], k: 3, include_general: true, ..SearchRequest::new("use of proceeds") }).await.unwrap();
    let h = &r.hits[0];
    assert_eq!(h.doc.kind, DocKind::Filing);
    assert_eq!(h.doc.source, "sec");
    assert!(h.doc.url.as_deref().is_some_and(|u| u.starts_with("http")), "{:?}", h.doc.url);
    assert!(h.section.is_some(), "el folleto conserva sus secciones");
    let (_, stats) = app.get("/api/knowledge/stats").await;
    assert!(stats["documents"].as_u64().unwrap() >= 2 && stats["chunks"].as_u64().unwrap() >= 2, "{stats}");
    shutdown.cancel();
}

#[tokio::test]
async fn opening_failure_does_not_stop_startup() {
    // BD sin la migración 0004: `KnowledgeBase::open` falla y `open_with` devuelve None (la app sigue).
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    let rag = open_with(KnowledgeConfig::default(), pool, Arc::new(FakeEmbedder::default()), CancellationToken::new()).await;
    assert!(rag.is_none());
}

#[tokio::test]
async fn document_embeddings_wait_while_a_generation_is_running_but_queries_do_not() {
    let activity = Activity::default();
    let gated = Arc::new(GatedEmbedder::new(Arc::new(FakeEmbedder::default()), activity.clone(), CancellationToken::new()).with_poll(Duration::from_millis(10)));
    let guard = activity.begin();
    // Las consultas pasan aunque haya una generación.
    tokio::time::timeout(Duration::from_secs(2), gated.embed(&["pregunta".into()], EmbedKind::Query)).await.expect("la consulta no espera").unwrap();
    // Los documentos esperan.
    let g2 = gated.clone();
    let doc = tokio::spawn(async move { g2.embed(&["texto".into()], EmbedKind::Document).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!doc.is_finished(), "el documento debe esperar mientras hay una generación");
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), doc).await.expect("se reanuda al terminar").unwrap().unwrap();
}

#[tokio::test]
async fn tracked_analyst_marks_chat_and_analysis_streams_until_dropped() {
    use domain::{Analyst, ChatRequest};
    use futures::StreamExt;
    let app = TestApp::new().await;
    let activity = Activity::default();
    let tracked = Arc::new(TrackedAnalyst::new(app.state.analyst.clone(), activity.clone()));
    assert!(!activity.busy());
    let snap = app.state.market.snapshot(&"stock:AAPL".parse().unwrap()).await.unwrap();
    let mut stream = tracked.clone().analyze_stream(snap, None);
    assert!(activity.busy(), "el análisis en curso cuenta");
    while stream.next().await.is_some() {}
    drop(stream);
    assert!(!activity.busy(), "al soltar el stream se libera");
    let req = ChatRequest {
        conversation_id: "c".into(),
        history: vec![],
        user_message: "hola".into(),
        focus: vec![],
        market: app.state.market.clone(),
        language: "es".into(),
    };
    let chat = tracked.chat(req);
    assert!(activity.busy(), "el chat en curso cuenta");
    drop(chat);
    assert!(!activity.busy());
}

#[tokio::test]
async fn maintenance_completes_pending_embeddings_when_idle_and_stops_on_shutdown() {
    let app = TestApp::new().await;
    let shutdown = CancellationToken::new();
    let rag = open_with(KnowledgeConfig::default(), app.state.db.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.unwrap();
    let task = tokio::spawn(run_maintenance(rag.kb.clone(), rag.activity.clone(), shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task).await.expect("termina con el apagado").unwrap();
}
