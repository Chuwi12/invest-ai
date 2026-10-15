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
        let http = reqwest::Client::builder()
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
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let msg = format!("HTTP {status}: {}", text.chars().take(200).collect::<String>());
            return Err(if status.is_server_error() { EmbedError::Unavailable(msg) } else { EmbedError::Bad(msg) });
        }
        let parsed: EmbedResponse = resp.json().await.map_err(|e| EmbedError::Bad(e.without_url().to_string()))?;
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
            .route("/nan/api/embed", post(|| async { Json(json!({ "embeddings": [[0.0, 0.0]] })) }));
        let url = serve(app).await;
        let go = |p: &str| {
            let e = OllamaEmbedder::new(OllamaEmbedConfig::new(&format!("{url}/{p}"), "m")).unwrap();
            async move { e.embed(&["a".into(), "b".into()], EmbedKind::Document).await }
        };
        assert!(matches!(go("missing").await, Err(EmbedError::ModelMissing(_))));
        assert!(matches!(go("boom").await, Err(EmbedError::Unavailable(_))));
        assert!(matches!(go("short").await, Err(EmbedError::Bad(_))));
        assert!(matches!(go("nan").await, Err(EmbedError::Bad(_))));
        let down = OllamaEmbedder::new(OllamaEmbedConfig::new("http://127.0.0.1:1", "m")).unwrap();
        let err = down.embed(&["a".into()], EmbedKind::Document).await.unwrap_err();
        assert!(err.is_transient());
    }
}
