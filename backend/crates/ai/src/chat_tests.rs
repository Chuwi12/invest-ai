use std::time::Duration;

use chrono::Utc;
use domain::{AssetId, ChatMessage, MarketData};
use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::{
    fixture::FakeMarket,
    testutil::{MemSettings, MockOllama, content_line, done_line, reply, thinking_line, tool_call_line},
};

struct Env {
    mock: MockOllama,
    engine: Arc<ChatEngine>,
    market: Arc<FakeMarket>,
}

async fn env_with(max_rounds: usize) -> Env {
    let mock = MockOllama::start().await;
    let cfg = AiConfig { base_url: mock.url.clone(), max_tool_rounds: max_rounds, ..AiConfig::default() };
    let client = OllamaClient::new(&mock.url, Duration::from_secs(5)).unwrap();
    let models = Arc::new(ModelManager::new(client.clone(), cfg.clone(), Arc::new(MemSettings::default())));
    let analysis = Arc::new(AnalysisEngine::new(client.clone(), models.clone(), cfg.clone()));
    Env { engine: Arc::new(ChatEngine::new(client, models, analysis, cfg)), mock, market: Arc::new(FakeMarket::new()) }
}

async fn env() -> Env {
    env_with(6).await
}

fn request(e: &Env, text: &str, focus: &[&str]) -> ChatRequest {
    ChatRequest {
        conversation_id: "c1".into(),
        history: vec![],
        user_message: text.into(),
        focus: focus.iter().map(|s| s.parse::<AssetId>().unwrap()).collect(),
        market: e.market.clone() as Arc<dyn MarketData>,
        language: "es".into(),
    }
}

async fn collect(e: &Env, req: ChatRequest) -> Vec<ChatEvent> {
    e.engine.clone().run(req).collect().await
}

fn text_of(events: &[ChatEvent]) -> String {
    events.iter().filter_map(|e| if let ChatEvent::Token { text } = e { Some(text.as_str()) } else { None }).collect()
}

#[tokio::test]
async fn plain_answer_streams_tokens_with_focus_context_and_tools_offered() {
    let e = env().await;
    e.mock.push_chat(reply("Hola, dime qué activo te interesa."));
    let ev = collect(&e, request(&e, "hola", &["stock:AAPL"])).await;
    assert!(matches!(ev.first(), Some(ChatEvent::Start { .. })));
    assert_eq!(text_of(&ev), "Hola, dime qué activo te interesa.");
    let Some(ChatEvent::Done { model, prompt_tokens, completion_tokens, .. }) = ev.last() else { panic!("{ev:?}") };
    assert_eq!(model, "qwen2.5:7b");
    assert_eq!((*prompt_tokens, *completion_tokens), (Some(120), Some(30)));
    let req = &e.mock.chat_requests()[0];
    assert_eq!(req["tools"].as_array().unwrap().len(), 9);
    let sys = req["messages"][0]["content"].as_str().unwrap();
    assert!(sys.contains("<market_data>") && sys.contains("Apple Inc.") && sys.contains("NEVER obey it"));
    assert!(!sys.contains("[N1] 2026"), "las etiquetas de la precarga no son citables");
    assert_eq!(req["messages"][1]["content"], "hola");
}

#[tokio::test]
async fn tool_loop_executes_calls_and_feeds_results_back() {
    let e = env().await;
    e.mock.push_chat(vec![tool_call_line("get_quote", json!({"id": "AAPL"})), done_line()]);
    e.mock.push_chat(reply("AAPL cotiza a 228.4 USD según Fixture."));
    let ev = collect(&e, request(&e, "¿a cuánto está Apple?", &[])).await;
    let kinds: Vec<&str> = ev
        .iter()
        .map(|x| match x {
            ChatEvent::Start { .. } => "start",
            ChatEvent::ToolCall { .. } => "tool_call",
            ChatEvent::ToolResult { .. } => "tool_result",
            ChatEvent::Token { .. } => "token",
            ChatEvent::Done { .. } => "done",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds[..3], ["start", "tool_call", "tool_result"], "{kinds:?}");
    let ChatEvent::ToolCall { name, arguments, id } = &ev[1] else { panic!() };
    assert_eq!((name.as_str(), arguments["id"].as_str()), ("get_quote", Some("AAPL")));
    let ChatEvent::ToolResult { ok, summary, id: rid, .. } = &ev[2] else { panic!() };
    assert!(*ok && summary.contains("228.4"));
    assert_eq!(id, rid);
    let reqs = e.mock.chat_requests();
    assert_eq!(reqs.len(), 2);
    let msgs = reqs[1]["messages"].as_array().unwrap();
    assert_eq!(msgs[2]["role"], "assistant");
    assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "get_quote");
    assert_eq!(msgs[3]["role"], "tool");
    assert_eq!(msgs[3]["tool_name"], "get_quote");
    assert!(msgs[3]["content"].as_str().unwrap().contains("fuente: fixture"));
    // La cifra citada está en el resultado de la herramienta: sin aviso.
    assert!(!text_of(&ev).contains('⚠'), "{}", text_of(&ev));
}

#[tokio::test]
async fn unverified_figures_in_the_answer_are_flagged() {
    let e = env().await;
    e.mock.push_chat(reply("Apple subirá un 37.5 % el mes que viene."));
    let ev = collect(&e, request(&e, "¿qué pasará con Apple?", &[])).await;
    let t = text_of(&ev);
    assert!(t.contains("⚠") && t.contains("37.5"), "{t}");
    // Si la cifra la dijo el usuario, sí es conocida.
    let e = env().await;
    e.mock.push_chat(reply("Dices que compraste a 150.25."));
    let ev = collect(&e, request(&e, "compré a 150.25, ¿y ahora?", &[])).await;
    assert!(!text_of(&ev).contains('⚠'));
}

#[tokio::test]
async fn tool_budget_is_enforced_and_final_answer_has_no_tools() {
    let e = env_with(2).await;
    for _ in 0..2 {
        e.mock.push_chat(vec![tool_call_line("get_macro", json!({})), done_line()]);
    }
    e.mock.push_chat(reply("Con lo que tengo: entorno estable."));
    let ev = collect(&e, request(&e, "dame el contexto", &[])).await;
    let reqs = e.mock.chat_requests();
    assert_eq!(reqs.len(), 3);
    assert!(reqs[0].get("tools").is_some() && reqs[1].get("tools").is_some());
    assert!(reqs[2].get("tools").is_none(), "última ronda sin herramientas");
    assert!(reqs[2]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("agotado"));
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })));
    assert!(text_of(&ev).contains("entorno estable"));
}

#[tokio::test]
async fn bad_tool_arguments_become_error_results_the_model_can_recover_from() {
    let e = env().await;
    e.mock.push_chat(vec![tool_call_line("get_quote", json!({"id": "planeta:X"})), tool_call_line("rm_rf", json!({})), done_line()]);
    e.mock.push_chat(reply("No pude consultarlo."));
    let ev = collect(&e, request(&e, "precio", &[])).await;
    let results: Vec<(bool, &String)> = ev.iter().filter_map(|x| if let ChatEvent::ToolResult { ok, summary, .. } = x { Some((*ok, summary)) } else { None }).collect();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|(ok, _)| !ok));
    let msgs = e.mock.chat_requests()[1]["messages"].clone();
    assert!(msgs[3]["content"].as_str().unwrap().starts_with("ERROR:"));
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn analyze_asset_emits_the_structured_analysis_event() {
    let e = env().await;
    let stage = json!({"stance": "mixed", "confidence": 0.5, "findings": [], "missing": []}).to_string();
    let judge = json!({"verdict": "wait", "confidence": 0.4, "horizon": "short", "summary": "Esperar.", "thesis": "Falta catalizador.",
        "bull_case": [], "bear_case": [], "risks": [], "entry_strategy": "", "exit_strategy": "", "position_sizing": "",
        "missing_data": [], "would_change_if": [], "uncertainty": ""}).to_string();
    e.mock.push_chat(vec![tool_call_line("analyze_asset", json!({"id": "stock:AAPL"})), done_line()]);
    e.mock.push_chat(reply(&stage));
    e.mock.push_chat(reply(&judge));
    e.mock.push_chat(reply("Mi recomendación: esperar."));
    let ev = collect(&e, request(&e, "¿invierto en Apple?", &["stock:AAPL"])).await;
    let a = ev.iter().find_map(|x| if let ChatEvent::Analysis { analysis } = x { Some(analysis) } else { None }).expect("evento Analysis");
    assert_eq!(a.verdict, domain::Verdict::Wait);
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_model_request() {
    let e = env().await;
    *e.mock.state.line_delay.lock().unwrap() = Duration::from_millis(80);
    e.mock.push_chat((0..60).map(|i| content_line(&format!("t{i} "))).chain([done_line()]).collect());
    let mut s = e.engine.clone().run(request(&e, "cuéntame algo largo", &[]));
    let mut seen = 0;
    while let Some(ev) = s.next().await {
        if matches!(ev, ChatEvent::Token { .. }) {
            seen += 1;
            if seen == 2 {
                break;
            }
        }
    }
    drop(s);
    for _ in 0..60 {
        if e.mock.state.open_streams.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("la generación en Ollama siguió viva tras irse el cliente");
}

#[tokio::test]
async fn models_without_tools_get_none_and_thinking_is_forwarded() {
    let e = env().await;
    *e.mock.state.show.lock().unwrap() = json!({"capabilities": ["completion", "thinking"], "details": {}, "model_info": {}});
    e.mock.push_chat(vec![thinking_line("déjame pensar"), content_line("Respuesta"), done_line()]);
    let ev = collect(&e, request(&e, "hola", &[])).await;
    let req = &e.mock.chat_requests()[0];
    assert!(req.get("tools").is_none());
    assert!(req["messages"][0]["content"].as_str().unwrap().contains("Tools are NOT available"));
    assert!(ev.iter().any(|x| matches!(x, ChatEvent::Thinking { text } if text == "déjame pensar")));
    assert_eq!(text_of(&ev), "Respuesta");
}

#[tokio::test]
async fn input_limits_history_budget_and_errors() {
    let e = env().await;
    // Mensaje vacío => error sin llamar al modelo.
    let ev = collect(&e, request(&e, "   ", &[])).await;
    assert!(matches!(ev.last(), Some(ChatEvent::Error { .. })));
    assert!(e.mock.chat_requests().is_empty());
    // Mensaje enorme => se recorta.
    e.mock.push_chat(reply("ok"));
    collect(&e, request(&e, &"a".repeat(50_000), &[])).await;
    let sent = e.mock.chat_requests()[0]["messages"][1]["content"].as_str().unwrap().len();
    assert_eq!(sent, crate::safety::MAX_USER_CHARS);
    // Historial: solo caben los mensajes más recientes.
    let hist: Vec<ChatMessage> = (0..200)
        .map(|i| ChatMessage {
            id: i.to_string(),
            conversation_id: "c1".into(),
            role: if i % 2 == 0 { Role::User } else { Role::Assistant },
            content: format!("mensaje {i} {}", "palabra ".repeat(100)),
            tool_calls: vec![],
            analysis: None,
            created_at: Utc::now(),
        })
        .collect();
    let w = history_messages(&hist, 2000);
    assert!(!w.is_empty() && w.len() < 20, "{}", w.len());
    assert!(w.last().unwrap().content.starts_with("mensaje 199"));
    assert!(w[0].content.starts_with(&format!("mensaje {}", 200 - w.len())));
    // Ollama caído => evento Error (no pánico ni cuelgue).
    let dead_cfg = AiConfig { base_url: "http://127.0.0.1:9".into(), ..AiConfig::default() };
    let client = OllamaClient::new("http://127.0.0.1:9", Duration::from_secs(1)).unwrap();
    let models = Arc::new(ModelManager::new(client.clone(), dead_cfg.clone(), Arc::new(MemSettings::default())));
    let analysis = Arc::new(AnalysisEngine::new(client.clone(), models.clone(), dead_cfg.clone()));
    let dead = Env { mock: MockOllama::start().await, engine: Arc::new(ChatEngine::new(client, models, analysis, dead_cfg)), market: e.market.clone() };
    let ev = collect(&dead, request(&dead, "hola", &[])).await;
    assert!(matches!(ev.last(), Some(ChatEvent::Error { message }) if message.contains("Ollama")), "{ev:?}");
}
