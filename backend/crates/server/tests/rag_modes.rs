//! Archivado frente a recuperación (`RAG_ARCHIVE`, `RAG_ANALYSIS`, `RAG_CHAT`).
//!
//! Con el archivo ENCENDIDO y archivando (def.), pero sin recuperación, lo que se envía al modelo en el
//! análisis y en el chat tiene que ser idéntico, byte a byte, a lo que se envía sin RAG. Se comprueba con
//! el analista REAL (`OllamaAnalyst`) montado por el mismo punto que usa `main` (`server::knowledge::ollama_analyst`)
//! contra un Ollama simulado que registra cada petición. Sin red, sin GPU y con el embedder simulado.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ai::{AiConfig, fixture};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::header,
    response::Response,
    routing::{get, post},
};
use chrono::{TimeZone, Utc};
use domain::{AnalysisDepth, Analyst, AssetId, ChatRequest, IpoData, MarketData};
use futures::StreamExt;
use knowledge::{
    DocKind, EmbedError, EmbedKind, Embedder, FakeEmbedder, KnowledgeConfig, NewDocument, RagModes, Section,
};
use serde_json::{Value, json};
use server::{
    db,
    fake::FakeIpo,
    knowledge::{Rag, open_configured, open_with, ollama_analyst, run_maintenance},
    repo::settings::SqliteSettings,
};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------------- Ollama simulado

#[derive(Default)]
struct StubState {
    chats: Mutex<Vec<Value>>,
}

struct Stub {
    url: String,
    state: Arc<StubState>,
}

/// Mismo contrato que el simulado de `ai` (`/api/tags`, `/api/show`, `/api/ps`, `/api/chat`), con una respuesta
/// fija para toda petición de chat: lo que importa aquí es lo que se ENVÍA, no lo que contesta.
async fn start_stub() -> Stub {
    let state = Arc::new(StubState::default());
    let chat = |State(s): State<Arc<StubState>>, Json(body): Json<Value>| async move {
        s.chats.lock().unwrap().push(body);
        let lines = [
            json!({"message": {"role": "assistant", "content": "{\"stance\":\"neutral\","}, "done": false}),
            json!({"message": {"role": "assistant", "content": "\"confidence\":0.5,\"findings\":[]}"}, "done": false}),
            json!({"message": {"role": "assistant", "content": ""}, "done": true, "done_reason": "stop", "prompt_eval_count": 120, "eval_count": 30}),
        ];
        let body = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
        Response::builder().header(header::CONTENT_TYPE, "application/x-ndjson").body(Body::from(body)).unwrap()
    };
    let app = Router::new()
        .route("/api/chat", post(chat))
        .route(
            "/api/tags",
            get(|| async {
                Json(json!({"models": [{"name": "qwen2.5:7b", "size": 4683087332u64,
                    "details": {"family": "qwen2", "parameter_size": "7.6B", "quantization_level": "Q4_K_M"}}]}))
            }),
        )
        .route(
            "/api/show",
            post(|| async {
                Json(json!({
                    "capabilities": ["completion", "tools"],
                    "details": {"family": "qwen2", "parameter_size": "7.6B", "quantization_level": "Q4_K_M"},
                    "model_info": {"general.architecture": "qwen2", "qwen2.block_count": 28, "qwen2.context_length": 32768,
                        "qwen2.attention.head_count": 28, "qwen2.attention.head_count_kv": 4, "qwen2.embedding_length": 3584}
                }))
            }),
        )
        .route("/api/ps", get(|| async { Json(json!({"models": []})) }))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Stub { url, state }
}

/// Sustituye la hora real («AAAA-MM-DD HH:MM UTC») y la antigüedad del dato, que dependen del reloj del test.
fn mask_clock(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let shape = "dddd-dd-dd dd:dd UTC";
    let (mut out, mut i) = (String::new(), 0);
    while i < b.len() {
        let ok = i + shape.len() <= b.len() && shape.chars().enumerate().all(|(k, p)| if p == 'd' { b[i + k].is_ascii_digit() } else { b[i + k] == p });
        if ok {
            out.push_str("<T>");
            i += shape.len();
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    let key = "Antigüedad del dato | ";
    out.split(key)
        .enumerate()
        .map(|(i, p)| if i == 0 { p.to_string() } else { format!("<age>{}", p.trim_start_matches(|c: char| c.is_ascii_digit())) })
        .collect::<Vec<_>>()
        .join(key)
}

// ------------------------------------------------------------------------------------------ montaje

/// Lo observable de una pasada: peticiones enviadas al modelo y lo que devolvió la app.
struct Run {
    analysis_requests: Vec<Value>,
    analysis_outcome: String,
    chat_requests: Vec<Value>,
    chat_events: String,
    /// Documentos en el archivo al terminar (0 sin RAG).
    documents: usize,
}

fn aapl() -> AssetId {
    "stock:AAPL".parse().unwrap()
}

fn seeded(key: &str, kind: DocKind, title: &str, text: &str) -> NewDocument {
    NewDocument {
        doc_key: key.into(),
        kind,
        subtype: (kind == DocKind::Filing).then(|| "S-1".to_string()),
        source: if kind == DocKind::Filing { "sec".into() } else { "yahoo".into() },
        publisher: None,
        title: title.into(),
        url: Some(format!("https://example.test/{key}")),
        issuer: None,
        language: Some("en".into()),
        published_at: Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
        assets: vec!["stock:AAPL".into()],
        sections: vec![Section { heading: None, text: text.into() }],
        meta: json!({}),
    }
}

/// Una pasada completa (análisis estándar + un turno de chat) con `modes`; `None` = sin RAG en absoluto.
async fn run(modes: Option<RagModes>) -> Run {
    let stub = start_stub().await;
    let pool = db::connect_memory().await.expect("BD en memoria");
    let shutdown = CancellationToken::new();
    let raw_market: Arc<dyn MarketData> = Arc::new(fixture::FakeMarket::new());
    let raw_ipo: Arc<dyn IpoData> = Arc::new(FakeIpo::default());

    let rag = match modes {
        Some(modes) => {
            let cfg = KnowledgeConfig { modes, ..KnowledgeConfig::default() };
            let rag = open_with(cfg, pool.clone(), Arc::new(FakeEmbedder::default()), shutdown.clone()).await.expect("archivo abierto");
            // Contenido sobre AAPL que una recuperación encontraría: si llegara al modelo, se vería.
            rag.kb.ingest(seeded("news:1", DocKind::News, "Apple reports services revenue growth of 31.4 percent", "Apple reports services revenue growth of 31.4 percent in the third quarter, driven by subscriptions.")).await.unwrap();
            rag.kb.ingest(seeded("filing:1", DocKind::Filing, "Apple prospectus", "Services revenue growth of 31.4 percent in the third quarter was driven by subscriptions and results outlook guidance risks.")).await.unwrap();
            Some(rag)
        }
        None => None,
    };

    let cfg = AiConfig { base_url: stub.url.clone(), timeout: Duration::from_secs(20), ..AiConfig::default() };
    let settings = Arc::new(SqliteSettings::new(pool.clone()));
    let real = ollama_analyst(cfg, settings, rag.as_ref(), None).expect("analista");
    let real: Arc<dyn Analyst> = Arc::new(real);
    let (market, _ipo, analyst) = match &rag {
        Some(rag) => rag.wrap(raw_market, raw_ipo, real),
        None => (raw_market, raw_ipo, real),
    };

    // Análisis estándar sobre el fixture (determinista).
    let result = analyst.analyze_with(&fixture::stock_snapshot(), Some(AnalysisDepth::Standard)).await;
    let analysis_outcome = match result {
        Ok(a) => {
            let mut v = serde_json::to_value(&a).unwrap();
            for k in ["id", "generatedAt", "durationMs", "method"] {
                v[k] = json!(null);
            }
            format!("ok {v}")
        }
        Err(e) => format!("error {e}"),
    };
    let analysis_requests = stub.state.chats.lock().unwrap().clone();
    stub.state.chats.lock().unwrap().clear();

    // Un turno de chat con el activo en foco (la precarga pasa por el mercado envuelto).
    let req = ChatRequest {
        conversation_id: "c1".into(),
        history: vec![],
        user_message: "¿qué opinas de Apple?".into(),
        focus: vec![aapl()],
        market: market.clone(),
        language: "es".into(),
    };
    let events: Vec<String> = analyst
        .chat(req)
        .map(|e| match e {
            domain::ChatEvent::Start { .. } => "start".to_string(),
            domain::ChatEvent::Done { model, prompt_tokens, completion_tokens, .. } => format!("done {model} {prompt_tokens:?} {completion_tokens:?}"),
            other => format!("{other:?}"),
        })
        .collect()
        .await;
    let chat_requests = stub.state.chats.lock().unwrap().clone();

    // Si hay archivado, lo que pasa por el mercado envuelto llega al archivo (sin recuperación de por medio).
    let documents = match &rag {
        Some(rag) => {
            if rag.modes().archive {
                let before = rag.kb.stats().await.unwrap().documents;
                let _ = market.snapshot(&aapl()).await.unwrap();
                for _ in 0..200 {
                    if rag.kb.stats().await.unwrap().documents > before {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
            rag.kb.stats().await.unwrap().documents
        }
        None => 0,
    };
    shutdown.cancel();
    Run { analysis_requests, analysis_outcome, chat_requests, chat_events: mask_clock(&events.join("\n")), documents }
}

fn text_of(reqs: &[Value]) -> String {
    mask_clock(&serde_json::to_string(reqs).unwrap())
}

const fn m(archive: bool, analysis: bool, chat: bool) -> RagModes {
    RagModes { archive, analysis, chat }
}

// ----------------------------------------------------------------------------------------- tests

#[tokio::test]
async fn archive_on_without_retrieval_leaves_analysis_and_chat_byte_identical_to_rag_off() {
    let off = run(None).await;
    assert!(off.analysis_requests.len() >= 2, "el análisis hizo varias peticiones: {}", off.analysis_requests.len());
    assert!(!off.chat_requests.is_empty());

    // El caso por defecto: archivado encendido, recuperación apagada.
    let on = run(Some(m(true, false, false))).await;
    assert_eq!(text_of(&on.analysis_requests), text_of(&off.analysis_requests), "peticiones del análisis");
    assert_eq!(on.analysis_outcome, off.analysis_outcome, "resultado del análisis");
    assert_eq!(text_of(&on.chat_requests), text_of(&off.chat_requests), "peticiones del chat");
    assert_eq!(on.chat_events, off.chat_events, "eventos del chat");
    // …y aun así se archivó: 2 documentos sembrados + las noticias del snapshot que pasó por el mercado envuelto.
    assert!(on.documents > 2, "el archivado sigue activo: {} documentos", on.documents);
    // El chat no ofrece `search_knowledge` ni el análisis lleva `<knowledge_base>`.
    assert!(!text_of(&on.analysis_requests).contains("knowledge_base"));
    assert!(!text_of(&on.chat_requests).contains("search_knowledge") && !text_of(&on.chat_requests).contains("knowledge_base"));
}

#[tokio::test]
async fn each_retrieval_switch_reaches_only_its_own_side() {
    let off = run(None).await;
    let (off_a, off_c) = (text_of(&off.analysis_requests), text_of(&off.chat_requests));

    // Control: con las dos encendidas el prompt SÍ cambia (si no, esta batería no probaría nada).
    let both = run(Some(m(true, true, true))).await;
    assert!(text_of(&both.analysis_requests).contains("<knowledge_base>"), "con RAG_ANALYSIS=1 el juez recibe fragmentos");
    assert!(text_of(&both.chat_requests).contains("search_knowledge"), "con RAG_CHAT=1 el chat ofrece la herramienta");

    // Solo análisis: el chat no cambia.
    let only_analysis = run(Some(m(true, true, false))).await;
    assert!(text_of(&only_analysis.analysis_requests).contains("<knowledge_base>"));
    assert_eq!(text_of(&only_analysis.chat_requests), off_c, "RAG_CHAT=0: chat idéntico");
    assert_eq!(only_analysis.chat_events, off.chat_events);

    // Solo chat: el análisis no cambia.
    let only_chat = run(Some(m(true, false, true))).await;
    assert!(text_of(&only_chat.chat_requests).contains("search_knowledge"));
    assert_eq!(text_of(&only_chat.analysis_requests), off_a, "RAG_ANALYSIS=0: análisis idéntico");
    assert_eq!(only_chat.analysis_outcome, off.analysis_outcome);
}

#[tokio::test]
async fn retrieval_without_archiving_is_read_only_and_still_identical_when_off() {
    // RAG_ARCHIVE=0 con recuperación en el análisis: lee lo que hay, no archiva nada nuevo.
    let r = run(Some(m(false, true, false))).await;
    assert!(text_of(&r.analysis_requests).contains("<knowledge_base>"));
    assert_eq!(r.documents, 2, "sin archivado solo están los 2 documentos sembrados");
}

#[tokio::test]
async fn search_handles_follow_the_switches() {
    let pool = db::connect_memory().await.unwrap();
    for (a, c) in [(false, false), (true, false), (false, true), (true, true)] {
        let rag = open_with(KnowledgeConfig { modes: m(true, a, c), ..KnowledgeConfig::default() }, pool.clone(), Arc::new(FakeEmbedder::default()), CancellationToken::new())
            .await
            .unwrap();
        let s = rag.search();
        assert_eq!((s.analysis.is_some(), s.chat.is_some()), (a, c));
    }
}

#[tokio::test]
async fn without_archive_nothing_is_wrapped_or_ingested_and_background_is_a_no_op() {
    let pool = db::connect_memory().await.unwrap();
    let shutdown = CancellationToken::new();
    let rag = open_with(KnowledgeConfig { modes: m(false, true, true), ..KnowledgeConfig::default() }, pool, Arc::new(FakeEmbedder::default()), shutdown.clone()).await.unwrap();
    let market: Arc<dyn MarketData> = Arc::new(fixture::FakeMarket::new());
    let ipo: Arc<dyn IpoData> = Arc::new(FakeIpo::default());
    let analyst: Arc<dyn Analyst> = Arc::new(server::fake::FakeAnalyst::default());
    let (m2, i2, a2) = rag.wrap(market.clone(), ipo.clone(), analyst.clone());
    // Mismos objetos: ni TeeMarket, ni TeeIpo, ni TrackedAnalyst.
    assert!(std::ptr::addr_eq(Arc::as_ptr(&m2), Arc::as_ptr(&market)));
    assert!(std::ptr::addr_eq(Arc::as_ptr(&i2), Arc::as_ptr(&ipo)));
    assert!(std::ptr::addr_eq(Arc::as_ptr(&a2), Arc::as_ptr(&analyst)));
    let _ = m2.snapshot(&aapl()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rag.kb.stats().await.unwrap().documents, 0, "nada se archiva");
    let tasks = tokio_util::task::TaskTracker::new();
    rag.spawn_background(&tasks, shutdown.clone());
    tasks.close();
    tokio::time::timeout(Duration::from_secs(1), tasks.wait()).await.expect("sin trabajadores que esperar");
}

#[tokio::test]
async fn open_configured_respects_fake_and_all_off() {
    let pool = db::connect_memory().await.unwrap();
    let shutdown = CancellationToken::new();
    // INVEST_AI_FAKE=1: todo apagado aunque el archivado esté encendido.
    assert!(open_configured(KnowledgeConfig::default(), true, pool.clone(), shutdown.clone()).await.is_none());
    // Los tres a 0: ni se abre.
    assert!(open_configured(KnowledgeConfig { modes: RagModes::OFF, ..KnowledgeConfig::default() }, false, pool.clone(), shutdown.clone()).await.is_none());
    // Por defecto y sin fake: abierto, archivando y sin recuperación.
    let rag = open_configured(KnowledgeConfig::default(), false, pool, shutdown.clone()).await.expect("abierto por defecto");
    assert_eq!(rag.modes(), m(true, false, false));
    let s = rag.search();
    assert!(s.analysis.is_none() && s.chat.is_none());
    shutdown.cancel();
}

/// Embedder caído (sin Ollama o sin GPU/modelo): cuenta las llamadas.
struct Down(Arc<AtomicUsize>);

#[async_trait]
impl Embedder for Down {
    fn model_id(&self) -> String {
        FakeEmbedder::default().model_id()
    }
    async fn embed(&self, _: &[String], _: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(EmbedError::Unavailable("conexión rechazada".into()))
    }
}

#[tokio::test]
async fn archiving_without_ollama_keeps_the_text_and_the_maintenance_does_not_spin() {
    let pool = db::connect_memory().await.unwrap();
    let shutdown = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let rag: Rag = open_with(KnowledgeConfig::default(), pool, Arc::new(Down(calls.clone())), shutdown.clone()).await.unwrap();
    for i in 0..10 {
        let d = seeded(&format!("news:{i}"), DocKind::News, &format!("Apple headline number {i}"), &format!("Apple headline number {i} about services and guidance."));
        let r = rag.kb.ingest(d).await.unwrap();
        assert_eq!((r.embedded, r.pending), (0, 1));
    }
    // El mantenimiento arranca (embed_pending + purge) y se le deja correr: no puede entrar en bucle.
    let task = tokio::spawn(run_maintenance(rag.kb.clone(), rag.activity.clone(), shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(400)).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task).await.expect("termina con el apagado").unwrap();
    let s = rag.kb.stats().await.unwrap();
    assert_eq!((s.documents, s.chunks, s.pending), (10, 10, 10), "{s:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "una sola llamada al embedder caído en toda la sesión");
}
