//! Embeddings sobre `POST /api/embed` de Ollama. Nunca descarga modelos: si falta, error claro.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{
    embed::{EmbedKind, Embedder, normalize_l2},
    error::EmbedError,
};

/// Formato de prompt que exige cada familia de modelos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    /// bge-m3, mxbai, snowflake…: texto tal cual.
    None,
    /// nomic-embed-text: `search_document: ` / `search_query: `.
    Nomic,
    /// embeddinggemma: `title: none | text: …` / `task: search result | query: …`.
    Gemma,
    /// qwen3-embedding: la consulta lleva instrucción; el documento no.
    Qwen3,
}

impl PromptStyle {
    pub fn detect(model: &str) -> Self {
        let m = model.to_ascii_lowercase();
        if m.contains("nomic") {
            Self::Nomic
        } else if m.contains("embeddinggemma") {
            Self::Gemma
        } else if m.contains("qwen3-embedding") {
            Self::Qwen3
        } else {
            Self::None
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Some(Self::None),
            "nomic" => Some(Self::Nomic),
            "gemma" => Some(Self::Gemma),
            "qwen3" => Some(Self::Qwen3),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "plain",
            Self::Nomic => "nomic",
            Self::Gemma => "gemma",
            Self::Qwen3 => "qwen3",
        }
    }

    pub fn apply(self, text: &str, kind: EmbedKind) -> String {
        match (self, kind) {
            (Self::None, _) | (Self::Qwen3, EmbedKind::Document) => text.to_string(),
            (Self::Nomic, EmbedKind::Document) => format!("search_document: {text}"),
            (Self::Nomic, EmbedKind::Query) => format!("search_query: {text}"),
            (Self::Gemma, EmbedKind::Document) => format!("title: none | text: {text}"),
            (Self::Gemma, EmbedKind::Query) => format!("task: search result | query: {text}"),
            (Self::Qwen3, EmbedKind::Query) => {
                format!("Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:{text}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct OllamaEmbedConfig {
    pub base_url: String,
    pub model: String,
    pub style: PromptStyle,
    /// Trunca a las primeras `dims` componentes y renormaliza (solo modelos Matryoshka: gemma,
    /// qwen3, nomic v1.5). `None` = dimensión nativa.
    pub dims: Option<usize>,
    pub batch_size: usize,
    pub keep_alive: String,
    pub timeout: Duration,
    /// `options.num_gpu` (0 = forzar CPU para no competir por VRAM con el LLM). Sin verificar.
    pub num_gpu: Option<i32>,
}

impl OllamaEmbedConfig {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            style: PromptStyle::detect(model),
            dims: None,
            batch_size: 16,
            keep_alive: "2m".into(),
            timeout: Duration::from_secs(120),
            num_gpu: None,
        }
    }
}

/// Tope del cuerpo de respuesta (2xx y error): una real, de 16 vectores × 4096 floats, ronda 1 MB.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Lee el cuerpo por trozos y aborta al pasar de `MAX_BODY_BYTES` (mismo patrón que `market::http::read_limited`;
/// `market` no es dependencia de este crate). Pasarse es `Bad` (reintentar no lo arregla); un corte o un timeout a
/// mitad de cuerpo es transitorio. Con `Response::json` los dos serían «decode», de ahí que se parsee aparte.
async fn read_limited(mut resp: reqwest::Response) -> Result<Vec<u8>, EmbedError> {
    let too_big = || EmbedError::Bad(format!("respuesta demasiado grande (más de {MAX_BODY_BYTES} bytes)"));
    if resp.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(too_big());
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| EmbedError::Unavailable(e.without_url().to_string()))? {
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(too_big());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

pub struct OllamaEmbedder {
    cfg: OllamaEmbedConfig,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbedder {
    pub fn new(cfg: OllamaEmbedConfig) -> Result<Self, EmbedError> {
        // Ollama no comprime: sin descompresión una bomba gzip/brotli no se expande (aunque otro crate active las features).
        let http = reqwest::Client::builder()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(3))
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| EmbedError::Unavailable(e.to_string()))?;
        Ok(Self { cfg, http })
    }

    pub fn config(&self) -> &OllamaEmbedConfig {
        &self.cfg
    }

    async fn embed_batch(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut body = json!({
            "model": self.cfg.model,
            "input": inputs,
            "truncate": true,
            "keep_alive": self.cfg.keep_alive,
        });
        if let Some(n) = self.cfg.num_gpu {
            body["options"] = json!({ "num_gpu": n });
        }
        let url = format!("{}/api/embed", self.cfg.base_url);
        let resp = self.http.post(url).json(&body).send().await.map_err(|e| EmbedError::Unavailable(e.without_url().to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(EmbedError::ModelMissing(self.cfg.model.clone()));
        }
        let body = read_limited(resp).await?;
        if !status.is_success() {
            let msg = format!("HTTP {status}: {}", String::from_utf8_lossy(&body).chars().take(200).collect::<String>());
            return Err(if status.is_server_error() { EmbedError::Unavailable(msg) } else { EmbedError::Bad(msg) });
        }
        let parsed: EmbedResponse = serde_json::from_slice(&body).map_err(|e| EmbedError::Bad(format!("JSON no válido: {e}")))?;
        if parsed.embeddings.len() != inputs.len() {
            return Err(EmbedError::Bad(format!("se pidieron {} vectores y llegaron {}", inputs.len(), parsed.embeddings.len())));
        }
        let mut out = Vec::with_capacity(parsed.embeddings.len());
        for mut v in parsed.embeddings {
            if let Some(d) = self.cfg.dims {
                if v.len() < d {
                    return Err(EmbedError::Bad(format!("el modelo da {} dimensiones y se pidieron {d}", v.len())));
                }
                v.truncate(d);
            }
            normalize_l2(&mut v)?;
            if let Some(first) = out.first().map(Vec::len)
                && first != v.len()
            {
                return Err(EmbedError::Bad("dimensiones distintas dentro del mismo lote".into()));
            }
            out.push(v);
        }
        Ok(out)
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    fn model_id(&self) -> String {
        let dims = self.cfg.dims.map_or("native".to_string(), |d| d.to_string());
        format!("{}|{}|{}", self.cfg.model, self.cfg.style.name(), dims)
    }

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        let prepared: Vec<String> = texts.iter().map(|t| self.cfg.style.apply(t, kind)).collect();
        let mut out = Vec::with_capacity(prepared.len());
        for batch in prepared.chunks(self.cfg.batch_size.max(1)) {
            out.extend(self.embed_batch(batch).await?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, http::StatusCode, routing::post};
    use serde_json::Value;

    use super::*;

    async fn serve(handler: Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, handler).await.unwrap() });
        format!("http://{addr}")
    }

    #[test]
    fn prompt_styles_follow_model_cards() {
        assert_eq!(PromptStyle::detect("embeddinggemma:300m-qat-q8_0"), PromptStyle::Gemma);
        assert_eq!(PromptStyle::detect("nomic-embed-text"), PromptStyle::Nomic);
        assert_eq!(PromptStyle::detect("qwen3-embedding:0.6b"), PromptStyle::Qwen3);
        assert_eq!(PromptStyle::detect("bge-m3"), PromptStyle::None);
        assert_eq!(PromptStyle::Gemma.apply("x", EmbedKind::Query), "task: search result | query: x");
        assert_eq!(PromptStyle::Gemma.apply("x", EmbedKind::Document), "title: none | text: x");
        assert_eq!(PromptStyle::Nomic.apply("x", EmbedKind::Document), "search_document: x");
        assert!(PromptStyle::Qwen3.apply("x", EmbedKind::Query).starts_with("Instruct: "));
        assert_eq!(PromptStyle::Qwen3.apply("x", EmbedKind::Document), "x");
    }

    #[tokio::test]
    async fn sends_batches_with_prefixes_and_normalises() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s2 = seen.clone();
        let app = Router::new().route(
            "/api/embed",
            post(move |Json(b): Json<Value>| {
                let s2 = s2.clone();
                async move {
                    let n = b["input"].as_array().unwrap().len();
                    s2.lock().unwrap().push(b);
                    Json(json!({ "embeddings": (0..n).map(|_| vec![3.0, 4.0, 0.0]).collect::<Vec<_>>() }))
                }
            }),
        );
        let url = serve(app).await;
        let mut cfg = OllamaEmbedConfig::new(&url, "embeddinggemma:300m");
        cfg.batch_size = 2;
        cfg.num_gpu = Some(0);
        cfg.dims = Some(2);
        let e = OllamaEmbedder::new(cfg).unwrap();
        let out = e.embed(&["a".into(), "b".into(), "c".into()], EmbedKind::Query).await.unwrap();
        assert_eq!(out.len(), 3);
        assert!((out[0][0] - 0.6).abs() < 1e-6 && out[0].len() == 2, "truncado y renormalizado: {:?}", out[0]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "3 textos con lote 2 = 2 peticiones");
        assert_eq!(seen[0]["model"], "embeddinggemma:300m");
        assert_eq!(seen[0]["input"][0], "task: search result | query: a");
        assert_eq!(seen[0]["truncate"], true);
        assert_eq!(seen[0]["options"]["num_gpu"], 0);
        assert_eq!(e.model_id(), "embeddinggemma:300m|gemma|2");
    }

    #[tokio::test]
    async fn maps_errors() {
        let app = Router::new()
            .route("/missing/api/embed", post(|| async { (StatusCode::NOT_FOUND, "model not found") }))
            .route("/boom/api/embed", post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "x") }))
            .route("/short/api/embed", post(|| async { Json(json!({ "embeddings": [[1.0, 0.0]] })) }))
            .route("/nan/api/embed", post(|| async { Json(json!({ "embeddings": [[0.0, 0.0]] })) }))
            .route("/garbage/api/embed", post(|| async { "esto no es JSON" }));
        let url = serve(app).await;
        let go = |p: &str| {
            let e = OllamaEmbedder::new(OllamaEmbedConfig::new(&format!("{url}/{p}"), "m")).unwrap();
            async move { e.embed(&["a".into(), "b".into()], EmbedKind::Document).await }
        };
        assert!(matches!(go("missing").await, Err(EmbedError::ModelMissing(_))));
        assert!(matches!(go("boom").await, Err(EmbedError::Unavailable(_))));
        assert!(matches!(go("short").await, Err(EmbedError::Bad(_))));
        assert!(matches!(go("nan").await, Err(EmbedError::Bad(_))));
        assert!(matches!(go("garbage").await, Err(EmbedError::Bad(_))), "un JSON inválido no se arregla reintentando");
        let down = OllamaEmbedder::new(OllamaEmbedConfig::new("http://127.0.0.1:1", "m")).unwrap();
        let err = down.embed(&["a".into()], EmbedKind::Document).await.unwrap_err();
        assert!(err.is_transient());
    }

    /// Servidor que promete 500 bytes de cuerpo, envía un trozo y luego corta (`stall = false`) o se queda callado.
    async fn half_body(stall: bool) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let _ = s.read(&mut [0u8; 4096]).await;
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 500\r\n\r\n{\"embeddings\":[[1.0,").await.unwrap();
            if stall {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_cut_or_stalled_body_is_transient_not_bad() {
        for stall in [false, true] {
            let mut cfg = OllamaEmbedConfig::new(&half_body(stall).await, "m");
            cfg.timeout = Duration::from_millis(300);
            let err = OllamaEmbedder::new(cfg).unwrap().embed(&["a".into()], EmbedKind::Document).await.unwrap_err();
            assert!(matches!(err, EmbedError::Unavailable(_)), "stall={stall}: {err:?}");
        }
    }

    /// Servidor que responde `head` y luego `n` bytes en trozos de 64 KiB (chunked, sin `Content-Length`) hasta que el cliente corta.
    async fn flood(head: &str, n: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let head = head.to_owned();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let _ = s.read(&mut [0u8; 4096]).await;
            if s.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let chunk = vec![b' '; 0x10000];
            for _ in 0..n / chunk.len() {
                if s.write_all(b"10000\r\n").await.is_err() || s.write_all(&chunk).await.is_err() || s.write_all(b"\r\n").await.is_err() {
                    return;
                }
            }
        });
        format!("http://{addr}")
    }

    async fn embed_one(url: &str) -> Result<Vec<Vec<f32>>, EmbedError> {
        OllamaEmbedder::new(OllamaEmbedConfig::new(url, "m")).unwrap().embed(&["a".into()], EmbedKind::Document).await
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_bad_not_transient() {
        const CHUNKED: &str = "transfer-encoding: chunked\r\n\r\n";
        let over = MAX_BODY_BYTES + 0x100000;
        for head in [
            format!("HTTP/1.1 200 OK\r\n{CHUNKED}"),
            // Lo que se cuenta es lo que llega: aunque se anuncie gzip no se descomprime.
            format!("HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\n{CHUNKED}"),
            format!("HTTP/1.1 400 Bad Request\r\n{CHUNKED}"),
            format!("HTTP/1.1 500 Internal Server Error\r\n{CHUNKED}"),
        ] {
            let err = embed_one(&flood(&head, over).await).await.unwrap_err();
            assert!(matches!(&err, EmbedError::Bad(m) if m.contains("demasiado grande")), "{head:?}: {err:?}");
        }
        // Con `Content-Length` enorme se rechaza sin leer nada (el servidor cierra sin enviar cuerpo).
        let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        let err = embed_one(&flood(&head, 0).await).await.unwrap_err();
        assert!(matches!(&err, EmbedError::Bad(m) if m.contains("demasiado grande")), "{err:?}");
    }

    /// Ollama no comprime: el cliente no descomprime nunca (un `Content-Encoding` hostil no puede disparar una bomba).
    /// Si la descompresión estuviera activa, reqwest intentaría expandir este JSON plano y fallaría.
    #[tokio::test]
    async fn content_encoding_is_not_decoded() {
        for enc in ["gzip", "br"] {
            let app = Router::new().route("/api/embed", post(move || async move { ([("content-encoding", enc)], r#"{"embeddings":[[3.0,4.0]]}"#) }));
            let out = embed_one(&serve(app).await).await.unwrap_or_else(|e| panic!("{enc}: {e}"));
            assert_eq!(out.len(), 1);
        }
    }
}
