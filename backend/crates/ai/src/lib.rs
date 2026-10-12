mod config;
pub mod context;
#[doc(hidden)]
pub mod fixture;
mod fmt;
mod lang;
pub mod catalog;
pub mod hardware;
pub mod models;
pub mod ollama;
pub mod scoring;
mod sentiment;
mod think;
mod tokens;
#[cfg(test)]
mod testutil;
pub use config::AiConfig;
pub use sentiment::annotate_sentiment;
