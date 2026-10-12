//! Configuración del servidor, leída del entorno (`.env` se carga antes en `main`).

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    /// Orígenes permitidos por CORS (y para abrir el WebSocket). `FRONTEND_ORIGIN`, separados por comas.
    pub frontend_origins: Vec<String>,
    /// Carpeta del frontend compilado, si existe.
    pub static_dir: Option<PathBuf>,
    /// Idioma de respuesta del asistente (BCP-47).
    pub language: String,
    /// `INVEST_AI_FAKE=1`: datos y analista de mentira (desarrollo sin red ni Ollama).
    pub fake: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr: SocketAddr = env_or("BIND_ADDR", "127.0.0.1:8787")
            .parse()
            .context("BIND_ADDR no es una dirección válida (p. ej. 127.0.0.1:8787)")?;
        let allow_remote = matches!(std::env::var("INVEST_AI_ALLOW_REMOTE").ok().as_deref(), Some("1"));
        check_bind(bind_addr, allow_remote)?;
        let frontend_origins = std::env::var("FRONTEND_ORIGIN")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(Self {
            bind_addr,
            database_url: env_or("DATABASE_URL", "sqlite://data/invest-ai.db"),
            frontend_origins,
            static_dir: resolve_static_dir(std::env::var("STATIC_DIR").ok().as_deref()),
            language: env_or("LLM_LANGUAGE", "es"),
            fake: matches!(
                std::env::var("INVEST_AI_FAKE").ok().as_deref(),
                Some("1" | "true" | "TRUE" | "yes")
            ),
        })
    }
}

/// S-05: la API no tiene autenticación, así que solo escucha en loopback salvo opt-out explícito.
fn check_bind(addr: SocketAddr, allow_remote: bool) -> anyhow::Result<()> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    anyhow::ensure!(
        allow_remote,
        "BIND_ADDR={addr} no es loopback: la API no tiene autenticación y quedaría expuesta a la red. \
         Usa 127.0.0.1/[::1], o define INVEST_AI_ALLOW_REMOTE=1 si sabes lo que haces"
    );
    tracing::warn!(
        "############################################################################\n\
         ## INVEST_AI_ALLOW_REMOTE=1: escuchando en {addr}, NO es loopback.\n\
         ## La API NO tiene autenticación: cualquiera con acceso a la red puede leer\n\
         ## tus chats, borrar modelos de Ollama y consumir tu GPU. Protégela con un\n\
         ## cortafuegos o un proxy con autenticación.\n\
         ############################################################################"
    );
    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    }
}

/// `STATIC_DIR` si está definido; si no, busca `frontend/dist` desde la raíz del repo o desde
/// `backend/` (el cwd habitual con `cargo run`). Solo vale una carpeta con `index.html`.
fn resolve_static_dir(explicit: Option<&str>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(dir) => vec![PathBuf::from(dir)],
        None => vec![PathBuf::from("frontend/dist"), PathBuf::from("../frontend/dist")],
    };
    candidates.into_iter().find(|dir| has_index(dir))
}

fn has_index(dir: &Path) -> bool {
    dir.join("index.html").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_must_be_loopback_unless_opted_out() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        for ok in ["127.0.0.1:8787", "127.0.0.2:80", "[::1]:8787"] {
            assert!(check_bind(a(ok), false).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0:8787", "192.168.1.5:8787", "[::]:8787", "8.8.8.8:80"] {
            let err = check_bind(a(bad), false).unwrap_err().to_string();
            assert!(err.contains("INVEST_AI_ALLOW_REMOTE"), "{bad}: {err}");
            assert!(check_bind(a(bad), true).is_ok(), "{bad} con opt-out");
        }
    }

    #[test]
    fn static_dir_needs_index_html() {
        let dir = std::env::temp_dir().join(format!("invest-ai-static-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve_static_dir(Some(dir.to_str().unwrap())), None);
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        assert_eq!(resolve_static_dir(Some(dir.to_str().unwrap())), Some(dir.clone()));
        std::fs::remove_dir_all(dir).ok();
    }
}
