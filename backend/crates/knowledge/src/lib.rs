//! Archivo de conocimiento local (RAG) de invest-ai.
//!
//! El LLM no aprende: consulta este archivo. Cada fragmento recuperado llega al modelo con
//! fuente, fecha y URL, numerado `[D#]`, y la cita de la respuesta se verifica contra lo que
//! realmente se le entregó. Diseño, decisiones y mediciones en `docs/notes/rag.md`.
//!
//! Piezas: [`chunk`] (troceado), [`embed`]/[`ollama`] (embeddings), [`index`] (búsqueda exacta en
//! memoria), [`store`] (SQLite), [`KnowledgeBase`] (todo junto), [`citation`] (citas y contexto
//! seguro) e [`ingest`] (enganche de noticias).

pub mod chunk;
pub mod citation;
pub mod config;
pub mod embed;
pub mod error;
pub mod index;
pub mod ingest;
pub mod kb;
pub mod model;
pub mod ollama;
pub mod store;
pub mod testutil;
pub mod text;

pub use chunk::{ChunkConfig, Section, chunk_sections, sections_from_text};
pub use citation::{CitationIssue, Citations};
pub use config::{DEFAULT_EMBED_MODEL, KnowledgeConfig};
pub use embed::{EmbedKind, Embedder, FakeEmbedder};
pub use error::{EmbedError, KnowledgeError, Result};
pub use ingest::{NewsSink, spawn_news_worker};
pub use kb::{KnowledgeBase, KnowledgeSearch};
pub use model::*;
pub use ollama::{OllamaEmbedConfig, OllamaEmbedder, PromptStyle};
pub use store::SCHEMA_SQL;
