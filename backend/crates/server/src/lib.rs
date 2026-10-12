//! Servidor de invest-ai: API HTTP, SSE y WebSocket, persistencia SQLite y cableado.
//!
//! Todo el código habla con `market` y `ai` a través de los traits de `domain`; solo `main.rs`
//! conoce las implementaciones concretas.

pub mod config;
pub mod db;
pub mod error;
pub mod extract;
pub mod fake;
pub mod models;
pub mod repo;
pub mod resolve;
pub mod routes;
pub mod singleflight;
pub mod state;

pub use routes::app;
pub use state::{AppState, Settings};
