use std::time::Duration;

use domain::{DomainError, PullEvent};
use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::testutil::*;

fn client(m: &MockOllama) -> OllamaClient {
    OllamaClient::new(&m.url, Duration::from_secs(5)).unwrap()
}

fn params(model: &str) -> ChatParams {
    ChatParams { model: model.into(), messages: vec![WireMessage::user("hola")], ..Default::default() }
}

#[tokio::test]
async fn streams_content_and_usage() {
    let m = MockOllama::start().await;
    m.push_chat(reply("Hola mundo"));
    let out = client(&m).chat(params("qwen2.5:7b")).await.unwrap();
    assert_eq!(out.content, "Hola mundo");
    assert_eq!(out.usage.prompt_tokens, Some(120));
    assert_eq!(out.usage.completion_tokens, Some(30));
    assert_eq!(out.done_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn request_carries_options_format_tools_and_think() {
    let m = MockOllama::start().await;
    m.push_chat(reply("{}"));
    let mut p = params("qwen3:8b");
    p.num_ctx = Some(4096);
    p.temperature = Some(0.1);
    p.num_predict = Some(500);
    p.keep_alive = Some("30m".into());
    p.format = Some(json!({"type": "object"}));
    p.think = Some(json!(false));
    p.tools = vec![json!({"type": "function", "function": {"name": "x"}})];
    client(&m).chat(p).await.unwrap();
    let req = &m.chat_requests()[0];
    assert_eq!(req["options"]["num_ctx"], 4096);
    assert_eq!(req["options"]["num_predict"], 500);
    assert_eq!(req["keep_alive"], "30m");
    assert_eq!(req["format"]["type"], "object");
    assert_eq!(req["think"], false);
    assert_eq!(req["tools"][0]["function"]["name"], "x");
    assert_eq!(req["stream"], true);
}

#[tokio::test]
async fn separates_thinking_field_and_think_tags() {
    let m = MockOllama::start().await;
    m.push_chat(vec![
        thinking_line("pienso"),
        content_line("<think>más "),
        content_line("razonamiento</think>Respuesta"),
        done_line(),
    ]);
    let out = client(&m).chat(params("qwen3:8b")).await.unwrap();
    assert_eq!(out.content, "Respuesta");
    assert_eq!(out.thinking, "piensomás razonamiento");
}

#[tokio::test]
async fn collects_tool_calls() {
    let m = MockOllama::start().await;
    m.push_chat(vec![tool_call_line("get_quote", json!({"id": "stock:AAPL"})), done_line()]);
    let out = client(&m).chat(params("qwen2.5:7b")).await.unwrap();
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].function.name, "get_quote");
    assert_eq!(out.tool_calls[0].function.arguments["id"], "stock:AAPL");
}

#[tokio::test]
async fn truncated_stream_is_an_error() {
    let m = MockOllama::start().await;
    m.push_chat(vec![content_line("a medias")]);
    let err = client(&m).chat(params("x")).await.unwrap_err();
    assert!(matches!(err, DomainError::Upstream { .. }), "{err:?}");
}

#[tokio::test]
async fn error_line_and_http_errors_are_mapped() {
    let m = MockOllama::start().await;
    m.push_chat(vec![json!({"error": "model runner crashed"}).to_string()]);
    let err = client(&m).chat(params("x")).await.unwrap_err();
    assert!(err.to_string().contains("model runner crashed"), "{err}");
    // Sin guion => 500 con cuerpo JSON.
    let err = client(&m).chat(params("x")).await.unwrap_err();
    assert!(matches!(err, DomainError::Upstream { .. }));
}

#[tokio::test]
async fn unreachable_server_is_unavailable() {
    let c = OllamaClient::new("http://127.0.0.1:9", Duration::from_secs(2)).unwrap();
    assert!(matches!(c.tags().await.unwrap_err(), DomainError::Unavailable(_)));
}

#[tokio::test]
async fn read_timeout_when_server_goes_silent() {
    let m = MockOllama::start().await;
    *m.state.line_delay.lock().unwrap() = Duration::from_secs(3);
    m.push_chat(reply("lento"));
    let c = OllamaClient::new(&m.url, Duration::from_millis(300)).unwrap();
    let err = c.chat(params("x")).await.unwrap_err();
    assert!(matches!(err, DomainError::Upstream { .. }), "{err:?}");
}

#[tokio::test]
async fn dropping_the_stream_closes_the_connection() {
    let m = MockOllama::start().await;
    *m.state.line_delay.lock().unwrap() = Duration::from_millis(100);
    m.push_chat((0..50).map(|i| content_line(&format!("t{i}"))).chain([done_line()]).collect());
    let c = client(&m);
    let mut s = c.chat_stream(params("x"));
    assert!(matches!(s.next().await, Some(Ok(ChatChunk::Content(_)))));
    assert_eq!(m.state.open_streams.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(s);
    for _ in 0..50 {
        if m.state.open_streams.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("el servidor siguió generando tras soltar el stream");
}

#[tokio::test]
async fn tags_show_ps_delete() {
    let m = MockOllama::start().await;
    let c = client(&m);
    let tags = c.tags().await.unwrap();
    assert_eq!(tags[0].name, "qwen2.5:7b");
    assert_eq!(tags[0].details.parameter_size, "7.6B");
    let show = c.show("qwen2.5:7b").await.unwrap();
    assert!(show.has("tools") && !show.has("thinking"));
    assert_eq!(show.arch_u64("block_count"), Some(28));
    assert!(c.ps().await.unwrap().is_empty());
    c.delete("qwen2.5:7b").await.unwrap();
    assert_eq!(m.state.deleted.lock().unwrap().as_slice(), ["qwen2.5:7b"]);
    assert!(matches!(c.delete("nope:1b").await.unwrap_err(), DomainError::NotFound(_)));
}

#[tokio::test]
async fn pull_reports_progress_then_done() {
    let m = MockOllama::start().await;
    *m.state.pull_lines.lock().unwrap() = vec![
        json!({"status": "pulling manifest"}).to_string(),
        json!({"status": "pulling abc", "digest": "sha256:abc", "total": 100, "completed": 40}).to_string(),
        json!({"status": "success"}).to_string(),
    ];
    let events: Vec<PullEvent> = client(&m).pull_stream("qwen3:4b").collect().await;
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[1], PullEvent::Progress { completed: Some(40), total: Some(100), .. }));
    assert!(matches!(events[2], PullEvent::Done));
}

#[tokio::test]
async fn pull_error_line_ends_with_error() {
    let m = MockOllama::start().await;
    *m.state.pull_lines.lock().unwrap() = vec![json!({"error": "pull model manifest: file does not exist"}).to_string()];
    let events: Vec<PullEvent> = client(&m).pull_stream("nada:1b").collect().await;
    assert!(matches!(&events[0], PullEvent::Error { message } if message.contains("does not exist")));
}

#[tokio::test]
async fn num_gpu_goes_in_options_only_when_set() {
    let m = MockOllama::start().await;
    m.push_chat(reply("a"));
    m.push_chat(reply("b"));
    let c = client(&m);
    c.chat(params("qwen3:8b")).await.unwrap();
    c.chat(ChatParams { num_gpu: Some(999), ..params("qwen3:8b") }).await.unwrap();
    let reqs = m.chat_requests();
    assert!(reqs[0]["options"].get("num_gpu").is_none(), "por defecto decide Ollama");
    assert_eq!(reqs[1]["options"]["num_gpu"], 999);
}

#[tokio::test]
async fn forced_gpu_that_runs_out_of_memory_retries_without_forcing_and_remembers() {
    // Los textos de falta de memoria de llama.cpp/CUDA tal como Ollama 0.40.x los devuelve en un 500.
    for msg in [
        "llama runner process has terminated: cudaMalloc failed: out of memory",
        "unable to allocate CUDA0 buffer of size 6710886400",
        "model requires more system memory (12.3 GiB) than is available (9.1 GiB)",
        "memory layout cannot be allocated",
    ] {
        let m = MockOllama::start().await;
        m.push_chat_failure(500, msg);
        m.push_chat(reply("respuesta sin forzar"));
        let c = client(&m);
        let out = c.chat(ChatParams { num_gpu: Some(999), ..params("qwen3:8b") }).await.unwrap_or_else(|e| panic!("{msg}: {e}"));
        assert_eq!(out.content, "respuesta sin forzar", "{msg}");
        let reqs = m.chat_requests();
        assert_eq!(reqs.len(), 2, "{msg}");
        assert_eq!(reqs[0]["options"]["num_gpu"], 999);
        assert!(reqs[1]["options"].get("num_gpu").is_none(), "el reintento no fuerza");
        // Se recuerda, se avisa una sola vez y se puede olvidar.
        assert!(c.gpu_fallback("qwen3:8b").is_some());
        assert!(c.take_gpu_fallback_notice("qwen3:8b").is_some());
        assert!(c.take_gpu_fallback_notice("qwen3:8b").is_none(), "el aviso se da una vez");
        assert!(c.gpu_fallback("qwen3:8b").is_some());
        assert!(c.gpu_fallback("otro:1b").is_none());
        c.clear_gpu_fallback("qwen3:8b");
        assert!(c.gpu_fallback("qwen3:8b").is_none());
    }
}

#[tokio::test]
async fn other_errors_with_forced_gpu_are_not_swallowed() {
    let m = MockOllama::start().await;
    m.push_chat_failure(500, "alguna otra cosa rara del servidor");
    let c = client(&m);
    let err = c.chat(ChatParams { num_gpu: Some(999), ..params("qwen3:8b") }).await.unwrap_err();
    assert!(matches!(err, DomainError::Upstream { .. }));
    assert_eq!(m.chat_requests().len(), 1, "sin reintento");
    assert!(c.gpu_fallback("qwen3:8b").is_none());
    // Y sin forzar, un fallo de memoria no se reintenta (no hay nada que relajar).
    m.push_chat_failure(500, "cudaMalloc failed: out of memory");
    assert!(c.chat(params("qwen3:8b")).await.is_err());
    assert_eq!(m.chat_requests().len(), 2);
}

#[test]
fn memory_error_detection_is_specific() {
    let up = |m: &str| DomainError::Upstream { provider: "ollama".into(), message: m.into() };
    assert!(is_gpu_memory_error(&up("500 Internal Server Error: CUDA error: out of memory")));
    assert!(is_gpu_memory_error(&up("failed to allocate 6.2 GiB")));
    assert!(is_gpu_memory_error(&up("runner OOM")));
    assert!(!is_gpu_memory_error(&up("modelo no encontrado")));
    assert!(!is_gpu_memory_error(&up("room for improvement"))); // «oom» dentro de una palabra
    assert!(!is_gpu_memory_error(&DomainError::Unavailable("out of memory".into())));
}

// --- Tamaño de las respuestas: topes por trozos, sin descompresión ---------------------------------

fn bytes_stream(parts: &[&'static str]) -> impl Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static {
    futures::stream::iter(parts.iter().map(|p| Ok(bytes::Bytes::from_static(p.as_bytes()))).collect::<Vec<_>>())
}

async fn collect_lines(s: BoxStream<'static, Result<Vec<u8>, DomainError>>) -> (Vec<String>, Option<DomainError>) {
    let (mut lines, mut err) = (Vec::new(), None);
    let mut s = s;
    while let Some(item) = s.next().await {
        match item {
            Ok(l) => lines.push(String::from_utf8(l).unwrap()),
            Err(e) => err = Some(e),
        }
    }
    (lines, err)
}

#[tokio::test]
async fn ndjson_splits_across_chunks_and_skips_blanks() {
    // Una línea cortada entre trozos, líneas vacías, CRLF y un resto sin salto final.
    let s = ndjson(bytes_stream(&["{\"a\":1}\n{\"b\"", ":2}\r\n\n  \n{\"c\":", "3}\n{\"d\":4}"]), usize::MAX);
    let (lines, err) = collect_lines(s).await;
    assert!(err.is_none(), "{err:?}");
    assert_eq!(lines, [r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#, r#"{"d":4}"#]);
}

#[tokio::test]
async fn ndjson_total_cap_aborts_the_stream() {
    let s = ndjson(bytes_stream(&["aaaa\n"; 10]), 24);
    let (lines, err) = collect_lines(s).await;
    assert_eq!(lines.len(), 4, "las líneas anteriores al tope sí llegan");
    assert!(err.is_some_and(|e| e.to_string().contains("demasiado grande")));
}

#[tokio::test]
async fn ndjson_line_cap_aborts_a_line_without_newline() {
    // 8 MiB + 1 KiB sin un solo salto de línea, en trozos de 1 KiB.
    let chunk = bytes::Bytes::from(vec![b'a'; 1024]);
    let n = MAX_LINE_BYTES / 1024 + 1;
    let s = ndjson(futures::stream::iter((0..n).map(move |_| Ok(chunk.clone()))), usize::MAX);
    let (lines, err) = collect_lines(s).await;
    assert!(lines.is_empty());
    assert!(err.is_some_and(|e| e.to_string().contains("línea de respuesta demasiado grande")));
}

#[tokio::test]
async fn read_limited_aborts_a_chunked_body_over_the_limit() {
    // Sin `Content-Length` (chunked): el tope no puede apoyarse en la cabecera.
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            let chunks = (0..10).map(|_| Ok::<_, std::convert::Infallible>(bytes::Bytes::from(vec![b'a'; 1024])));
            axum::body::Body::from_stream(futures::stream::iter(chunks))
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let err = read_limited(reqwest::get(&url).await.unwrap(), 4096).await.unwrap_err();
    assert!(err.to_string().contains("demasiado grande"), "{err}");
    assert_eq!(read_limited(reqwest::get(&url).await.unwrap(), 64 * 1024).await.unwrap().len(), 10 * 1024);
}

#[tokio::test]
async fn oversized_non_streaming_bodies_are_rejected() {
    let m = MockOllama::start().await;
    *m.state.show.lock().unwrap() = json!({ "modelfile": "a".repeat(MAX_BODY_BYTES + 1) });
    let err = client(&m).show("qwen2.5:7b").await.unwrap_err();
    assert!(matches!(&err, DomainError::Upstream { message, .. } if message.contains("demasiado grande")), "{err:?}");
    // También el cuerpo de un error HTTP.
    m.push_chat_failure(500, &"x".repeat(MAX_BODY_BYTES + 1));
    let err = client(&m).chat(params("x")).await.unwrap_err();
    assert!(matches!(&err, DomainError::Upstream { message, .. } if message.contains("demasiado grande")), "{err:?}");
    // Y uno normal sigue funcionando (`tags_show_ps_delete` cubre el resto de endpoints).
    *m.state.show.lock().unwrap() = json!({ "capabilities": ["tools"] });
    assert!(client(&m).show("qwen2.5:7b").await.unwrap().has("tools"));
}

#[tokio::test]
async fn a_giant_ndjson_line_aborts_chat_and_pull() {
    let m = MockOllama::start().await;
    m.push_chat(vec![content_line(&"a".repeat(MAX_LINE_BYTES + 1))]);
    let err = client(&m).chat(params("x")).await.unwrap_err();
    assert!(err.to_string().contains("línea de respuesta demasiado grande"), "{err}");
    *m.state.pull_lines.lock().unwrap() = vec![json!({ "status": "a".repeat(MAX_LINE_BYTES + 1) }).to_string()];
    let events: Vec<PullEvent> = client(&m).pull_stream("qwen3:4b").collect().await;
    assert!(matches!(&events[..], [PullEvent::Error { message }] if message.contains("demasiado grande")), "{events:?}");
}

#[tokio::test]
async fn a_long_legitimate_stream_is_not_cut() {
    // ~3 MB en 20 000 líneas: mucho más que cualquier respuesta real de un análisis.
    let m = MockOllama::start().await;
    m.push_chat((0..20_000).map(|_| content_line("token ")).chain([done_line()]).collect());
    let out = client(&m).chat(params("x")).await.unwrap();
    assert_eq!(out.content.len(), 20_000 * "token ".len());
}

/// Ollama no comprime: el cliente no descomprime nunca (un `Content-Encoding` hostil no dispara una bomba).
/// Con la descompresión activa, reqwest intentaría expandir este JSON plano y fallaría.
#[tokio::test]
async fn content_encoding_is_not_decoded() {
    for enc in ["gzip", "br"] {
        let app = axum::Router::new()
            .route("/api/tags", axum::routing::get(move || async move { ([("content-encoding", enc)], r#"{"models":[]}"#) }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let c = OllamaClient::new(&url, Duration::from_secs(5)).unwrap();
        assert!(c.tags().await.unwrap_or_else(|e| panic!("{enc}: {e}")).is_empty());
    }
}
