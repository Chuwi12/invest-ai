//! Configuración por entorno (`RAG_*`). Valores vacíos o ilegibles se ignoran.

use std::{collections::HashMap, time::Duration};

use crate::{
    chunk::ChunkConfig,
    model::DocKind,
    ollama::{OllamaEmbedConfig, PromptStyle},
};

/// Modelo recomendado (ver docs/notes/rag.md). Debe descargarse a mano: `ollama pull`.
pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma:300m-qat-q8_0";

#[derive(Debug, Clone)]
pub struct KnowledgeConfig {
    /// `RAG_ENABLED`: apagado por defecto hasta que el modelo esté instalado.
    pub enabled: bool,
    pub embed: OllamaEmbedConfig,
    pub chunk: ChunkConfig,
    pub top_k: usize,
    /// Tope duro de fragmentos almacenados.
    pub max_chunks: usize,
    pub max_doc_bytes: usize,
    pub max_chunks_per_doc: usize,
    /// Días de retención por tipo; 0 = indefinida.
    pub retention_news_days: u32,
    pub retention_filing_days: u32,
    pub retention_analysis_days: u32,
    /// Peso de fuente (0–1) por proveedor; ver `source_weight`.
    pub source_weights: HashMap<String, f32>,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embed: OllamaEmbedConfig::new("http://127.0.0.1:11434", DEFAULT_EMBED_MODEL),
            chunk: ChunkConfig::default(),
            top_k: 6,
            max_chunks: 100_000,
            max_doc_bytes: 4 * 1024 * 1024,
            max_chunks_per_doc: 3000,
            retention_news_days: 90,
            retention_filing_days: 1095,
            retention_analysis_days: 0,
            source_weights: HashMap::from([("sec".into(), 1.0), ("invest-ai".into(), 0.6), ("google_news".into(), 0.6)]),
        }
    }
}

impl KnowledgeConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let text = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let num = |k: &str| text(k).and_then(|v| v.parse::<usize>().ok());
        if let Some(v) = text("RAG_ENABLED") {
            c.enabled = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "si" | "sí");
        }
        let base = text("RAG_EMBED_BASE_URL").or_else(|| text("LLM_BASE_URL")).unwrap_or_else(|| "http://127.0.0.1:11434".into());
        let model = text("RAG_EMBED_MODEL").unwrap_or_else(|| DEFAULT_EMBED_MODEL.into());
        let mut e = OllamaEmbedConfig::new(&base, &model);
        if let Some(s) = text("RAG_EMBED_PROMPT_STYLE").filter(|s| !s.eq_ignore_ascii_case("auto")).and_then(|s| PromptStyle::parse(&s)) {
            e.style = s;
        }
        e.dims = num("RAG_EMBED_DIMS").filter(|d| (16..=8192).contains(d));
        if let Some(b) = num("RAG_EMBED_BATCH") {
            e.batch_size = b.clamp(1, 128);
        }
        if let Some(k) = text("RAG_EMBED_KEEP_ALIVE") {
            e.keep_alive = k;
        }
        if let Some(t) = num("RAG_EMBED_TIMEOUT_SECS") {
            e.timeout = Duration::from_secs(t.clamp(5, 1800) as u64);
        }
        e.num_gpu = text("RAG_EMBED_NUM_GPU").and_then(|v| v.parse::<i32>().ok());
        c.embed = e;
        if let Some(v) = num("RAG_CHUNK_TARGET_TOKENS") {
            c.chunk.target_tokens = v;
        }
        if let Some(v) = num("RAG_CHUNK_MAX_TOKENS") {
            c.chunk.max_tokens = v;
        }
        if let Some(v) = num("RAG_CHUNK_OVERLAP_TOKENS") {
            c.chunk.overlap_tokens = v;
        }
        c.chunk = c.chunk.sanitized();
        if let Some(v) = num("RAG_TOP_K") {
            c.top_k = v.clamp(1, 30);
        }
        if let Some(v) = num("RAG_MAX_CHUNKS") {
            c.max_chunks = v.clamp(1000, 2_000_000);
        }
        if let Some(v) = num("RAG_NEWS_RETENTION_DAYS") {
            c.retention_news_days = v.min(36_500) as u32;
        }
        if let Some(v) = num("RAG_FILING_RETENTION_DAYS") {
            c.retention_filing_days = v.min(36_500) as u32;
        }
        if let Some(v) = num("RAG_ANALYSIS_RETENTION_DAYS") {
            c.retention_analysis_days = v.min(36_500) as u32;
        }
        c
    }

    pub fn retention_days(&self, kind: DocKind) -> u32 {
        match kind {
            DocKind::News => self.retention_news_days,
            DocKind::Filing => self.retention_filing_days,
            DocKind::Analysis => self.retention_analysis_days,
        }
    }

    /// Peso de fuente 0–1 para el reranking: por proveedor si está configurado; si no, por tipo
    /// (folleto 1,0; noticia 0,75; análisis propio 0,6: no es una fuente primaria).
    pub fn source_weight(&self, kind: DocKind, source: &str) -> f32 {
        if let Some(w) = self.source_weights.get(&source.to_ascii_lowercase()) {
            return w.clamp(0.0, 1.0);
        }
        match kind {
            DocKind::Filing => 1.0,
            DocKind::News => 0.75,
            DocKind::Analysis => 0.6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_safe() {
        let c = KnowledgeConfig::from_lookup(|_| None);
        assert!(!c.enabled);
        assert_eq!(c.embed.model, DEFAULT_EMBED_MODEL);
        assert_eq!(c.embed.style, PromptStyle::Gemma);
        assert_eq!(c.chunk.target_tokens, 400);
    }

    #[test]
    fn reads_and_clamps() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("RAG_ENABLED", "1"),
            ("RAG_EMBED_MODEL", "qwen3-embedding:0.6b"),
            ("LLM_BASE_URL", "http://h:1/"),
            ("RAG_EMBED_DIMS", "512"),
            ("RAG_EMBED_BATCH", "9999"),
            ("RAG_TOP_K", "0"),
            ("RAG_CHUNK_TARGET_TOKENS", "9999"),
            ("RAG_NEWS_RETENTION_DAYS", "abc"),
        ]);
        let c = KnowledgeConfig::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert!(c.enabled);
        assert_eq!(c.embed.style, PromptStyle::Qwen3);
        assert_eq!(c.embed.base_url, "http://h:1");
        assert_eq!(c.embed.dims, Some(512));
        assert_eq!(c.embed.batch_size, 128);
        assert_eq!(c.top_k, 1);
        assert!(c.chunk.target_tokens <= c.chunk.max_tokens);
        assert_eq!(c.retention_news_days, 90);
    }

    #[test]
    fn source_weights() {
        let c = KnowledgeConfig::default();
        assert_eq!(c.source_weight(DocKind::Filing, "SEC"), 1.0);
        assert_eq!(c.source_weight(DocKind::News, "yahoo"), 0.75);
        assert!(c.source_weight(DocKind::News, "google_news") < 0.75);
    }
}
