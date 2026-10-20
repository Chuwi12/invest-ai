//! Configuración por entorno (`RAG_*`). Valores vacíos o ilegibles se ignoran.
//!
//! # Interruptores: archivado frente a recuperación
//! El archivo de conocimiento tiene tres interruptores independientes ([`RagModes`]):
//!
//! | Variable | Def. | Qué activa |
//! |---|---|---|
//! | `RAG_ARCHIVE` | 1 | **Archivado**: abre la base, archiva noticias y folletos (OPV y ETF) y el mantenimiento (`embed_pending`, `purge`). |
//! | `RAG_ANALYSIS` | 0 | **Recuperación** en el análisis (bloque `<knowledge_base>`). |
//! | `RAG_CHAT` | 0 | **Recuperación** en el chat (herramienta `search_knowledge`). |
//!
//! Precedencia (de mayor a menor), para cada interruptor por separado:
//! 1. `INVEST_AI_FAKE=1` apaga los tres (lo aplica el servidor, no esta configuración).
//! 2. La propia variable (`RAG_ARCHIVE`, `RAG_ANALYSIS` o `RAG_CHAT`) si trae un valor legible.
//! 3. `RAG_ENABLED` (**obsoleta**), si existe: `1` pone a 1 los interruptores no fijados y `0` los pone a 0.
//! 4. El valor por defecto de la tabla.
//!
//! Un valor ilegible en una de las tres variables se ignora (cae al nivel siguiente) y deja una nota en
//! [`KnowledgeConfig::notes`], igual que el uso de `RAG_ENABLED`: el servidor las registra con `tracing::warn!`.

use std::{collections::HashMap, time::Duration};

use crate::{
    chunk::ChunkConfig,
    model::DocKind,
    ollama::{OllamaEmbedConfig, PromptStyle},
};

/// Modelo recomendado (ver docs/notes/rag.md). Debe descargarse a mano: `ollama pull`.
pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma:300m-qat-q8_0";

/// Los tres interruptores del RAG (ver la documentación del módulo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RagModes {
    /// `RAG_ARCHIVE`: archiva (y mantiene) lo que la app ya descarga.
    pub archive: bool,
    /// `RAG_ANALYSIS`: el análisis recupera del archivo.
    pub analysis: bool,
    /// `RAG_CHAT`: el chat puede llamar a `search_knowledge`.
    pub chat: bool,
}

impl RagModes {
    /// Valores por defecto: se archiva, no se recupera (medición del 20-oct en `docs/notes/rag.md`).
    pub const DEFAULT: Self = Self { archive: true, analysis: false, chat: false };
    /// Todo apagado (p. ej. `INVEST_AI_FAKE=1`).
    pub const OFF: Self = Self { archive: false, analysis: false, chat: false };

    /// ¿Hay algo que hacer con el archivo? Si no, ni se abre.
    pub fn any(self) -> bool {
        self.archive || self.analysis || self.chat
    }
}

impl Default for RagModes {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone)]
pub struct KnowledgeConfig {
    /// `RAG_ARCHIVE` / `RAG_ANALYSIS` / `RAG_CHAT` (con `RAG_ENABLED` obsoleta como respaldo).
    pub modes: RagModes,
    /// Avisos de configuración (variable obsoleta, valor ilegible) para que el arranque los registre.
    pub notes: Vec<String>,
    pub embed: OllamaEmbedConfig,
    pub chunk: ChunkConfig,
    pub top_k: usize,
    /// `RAG_HYBRID` (def. sí): búsqueda léxica FTS5/BM25 fusionada con la vectorial (RRF). Con 0 no se crea ni se toca la tabla FTS5.
    pub hybrid: bool,
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
            modes: RagModes::DEFAULT,
            notes: Vec::new(),
            embed: OllamaEmbedConfig::new("http://127.0.0.1:11434", DEFAULT_EMBED_MODEL),
            chunk: ChunkConfig::default(),
            top_k: 6,
            hybrid: true,
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

/// Booleano de una variable de entorno; `None` si no se entiende.
fn parse_flag(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "si" | "sí" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Resuelve los tres interruptores según la precedencia del módulo.
fn read_modes(text: &impl Fn(&str) -> Option<String>, notes: &mut Vec<String>) -> RagModes {
    // `RAG_ENABLED` se interpreta como antes: solo los valores «verdaderos» lo encienden; cualquier otro, apagado.
    let legacy = text("RAG_ENABLED").map(|v| parse_flag(&v) == Some(true));
    let mut pick = |name: &str, default: bool| -> bool {
        let fallback = legacy.unwrap_or(default);
        let Some(raw) = text(name) else { return fallback };
        parse_flag(&raw).unwrap_or_else(|| {
            notes.push(format!("{name}={raw:?} no es 1/0, true/false, yes/no ni on/off: se ignora y queda en {}", u8::from(fallback)));
            fallback
        })
    };
    let modes = RagModes {
        archive: pick("RAG_ARCHIVE", RagModes::DEFAULT.archive),
        analysis: pick("RAG_ANALYSIS", RagModes::DEFAULT.analysis),
        chat: pick("RAG_CHAT", RagModes::DEFAULT.chat),
    };
    if let Some(on) = legacy {
        let (on, a, n, c) = (u8::from(on), u8::from(modes.archive), u8::from(modes.analysis), u8::from(modes.chat));
        notes.push(format!(
            "RAG_ENABLED está obsoleta (se mantiene solo por compatibilidad): RAG_ENABLED={on} fija a {on} los interruptores que no se indiquen. \
Usa RAG_ARCHIVE, RAG_ANALYSIS y RAG_CHAT. Resultado: archivado={a}, análisis={n}, chat={c}"
        ));
    }
    modes
}

impl KnowledgeConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let text = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let num = |k: &str| text(k).and_then(|v| v.parse::<usize>().ok());
        c.modes = read_modes(&text, &mut c.notes);
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
        if let Some(v) = text("RAG_HYBRID") {
            c.hybrid = !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
        }
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

    /// Interruptores con un entorno dado.
    fn modes(env: &[(&str, &str)]) -> (RagModes, Vec<String>) {
        let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let c = KnowledgeConfig::from_lookup(|k| env.get(k).cloned());
        (c.modes, c.notes)
    }

    const fn m(archive: bool, analysis: bool, chat: bool) -> RagModes {
        RagModes { archive, analysis, chat }
    }

    #[test]
    fn defaults_archive_but_do_not_retrieve() {
        let c = KnowledgeConfig::from_lookup(|_| None);
        assert_eq!(c.modes, m(true, false, false));
        assert_eq!(KnowledgeConfig::default().modes, c.modes);
        assert!(c.notes.is_empty(), "sin variables no hay avisos: {:?}", c.notes);
        assert!(c.modes.any() && !RagModes::OFF.any());
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
        assert_eq!(c.modes, m(true, true, true), "RAG_ENABLED=1 enciende las tres");
        assert_eq!(c.embed.style, PromptStyle::Qwen3);
        assert_eq!(c.embed.base_url, "http://h:1");
        assert_eq!(c.embed.dims, Some(512));
        assert_eq!(c.embed.batch_size, 128);
        assert_eq!(c.top_k, 1);
        assert!(c.chunk.target_tokens <= c.chunk.max_tokens);
        assert_eq!(c.retention_news_days, 90);
    }

    #[test]
    fn explicit_switches_are_independent() {
        assert_eq!(modes(&[("RAG_ARCHIVE", "0")]).0, m(false, false, false));
        assert_eq!(modes(&[("RAG_ANALYSIS", "1")]).0, m(true, true, false));
        assert_eq!(modes(&[("RAG_CHAT", "true")]).0, m(true, false, true));
        assert_eq!(modes(&[("RAG_ARCHIVE", "off"), ("RAG_ANALYSIS", "yes"), ("RAG_CHAT", "sí")]).0, m(false, true, true), "recuperar sin archivar es válido");
        assert_eq!(modes(&[("RAG_ARCHIVE", " 1 "), ("RAG_ANALYSIS", "")]).0, m(true, false, false), "espacios y vacío");
        assert!(modes(&[("RAG_ANALYSIS", "1")]).1.is_empty(), "las variables nuevas no avisan");
    }

    #[test]
    fn legacy_rag_enabled_is_the_fallback_and_warns() {
        let (on, notes) = modes(&[("RAG_ENABLED", "1")]);
        assert_eq!(on, m(true, true, true));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("obsoleta") && notes[0].contains("archivado=1, análisis=1, chat=1"), "{notes:?}");
        let (off, notes) = modes(&[("RAG_ENABLED", "0")]);
        assert_eq!(off, m(false, false, false));
        assert!(notes[0].contains("archivado=0, análisis=0, chat=0"), "{notes:?}");
        // Valor que antes contaba como «apagado» (no era un verdadero reconocido) sigue apagando.
        assert_eq!(modes(&[("RAG_ENABLED", "quizás")]).0, m(false, false, false));
    }

    #[test]
    fn explicit_switch_beats_legacy_rag_enabled_in_both_directions() {
        // RAG_ENABLED=1, pero la recuperación en el chat se apaga a mano.
        assert_eq!(modes(&[("RAG_ENABLED", "1"), ("RAG_CHAT", "0")]).0, m(true, true, false));
        // RAG_ENABLED=0, pero el archivado se enciende a mano (el caso que quiere el usuario con una .env antigua).
        assert_eq!(modes(&[("RAG_ENABLED", "0"), ("RAG_ARCHIVE", "1")]).0, m(true, false, false));
        assert_eq!(modes(&[("RAG_ENABLED", "1"), ("RAG_ARCHIVE", "0"), ("RAG_ANALYSIS", "0"), ("RAG_CHAT", "0")]).0, m(false, false, false));
    }

    #[test]
    fn unreadable_switch_is_ignored_with_a_note() {
        let (got, notes) = modes(&[("RAG_ANALYSIS", "tal vez")]);
        assert_eq!(got, m(true, false, false), "cae al defecto");
        assert!(notes.iter().any(|n| n.contains("RAG_ANALYSIS") && n.contains("se ignora")), "{notes:?}");
        // Con RAG_ENABLED=1 cae a él, no al defecto.
        let (got, notes) = modes(&[("RAG_ENABLED", "1"), ("RAG_CHAT", "dunno")]);
        assert_eq!(got, m(true, true, true));
        assert_eq!(notes.len(), 2, "el valor ilegible y la variable obsoleta: {notes:?}");
    }

    #[test]
    fn source_weights() {
        let c = KnowledgeConfig::default();
        assert_eq!(c.source_weight(DocKind::Filing, "SEC"), 1.0);
        assert_eq!(c.source_weight(DocKind::News, "yahoo"), 0.75);
        assert!(c.source_weight(DocKind::News, "google_news") < 0.75);
    }
}
