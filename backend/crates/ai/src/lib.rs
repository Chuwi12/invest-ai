//! Capa de IA: analista sobre Ollama (`OllamaAnalyst`), gestión de modelos, pipeline de análisis,
//! chat con tool calling y la puntuación cuantitativa determinista. Ver `docs/notes/ai.md`.

pub mod analysis;
mod analyst;
pub mod catalog;
pub mod chat;
mod config;
pub mod context;
pub mod fidelity;
#[doc(hidden)]
pub mod fixture;
mod fmt;
pub mod hardware;
mod lang;
pub mod models;
pub mod ollama;
pub mod safety;
pub mod scoring;
mod sentiment;
#[cfg(test)]
mod testutil;
mod think;
mod tokens;
pub mod tools;

pub use analyst::OllamaAnalyst;
pub use config::AiConfig;
pub use models::ModelManager;
pub use sentiment::annotate_sentiment;
