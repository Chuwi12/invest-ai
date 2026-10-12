//! Frontend compilado (`frontend/dist`) servido por el propio binario, con fallback de SPA.

use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use tower_http::services::ServeDir;

/// Los ficheros de `/assets/` llevan hash en el nombre (Vite): se pueden cachear para siempre.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub fn router(dir: PathBuf) -> Router {
    let index = Arc::new(dir.join("index.html"));
    // `ServeDir` resuelve ficheros, ETag y Range; lo que no encuentra pasa al fallback.
    let files = ServeDir::new(dir).fallback(get(spa_fallback).with_state(index));
    Router::new().fallback_service(files).layer(middleware::from_fn(cache_headers))
}

/// Rutas de la SPA (`/dashboard`, `/asset/stock%3AAAPL`...) devuelven `index.html`; lo que parece
/// un fichero (tiene extensión) o cuelga de `/assets/` y no existe es un 404 de verdad, para no
/// servir HTML donde el navegador espera JS o CSS.
async fn spa_fallback(State(index): State<Arc<PathBuf>>, req: Request) -> Response {
    let path = req.uri().path();
    let last = path.rsplit('/').next().unwrap_or_default();
    if path.starts_with("/assets/") || last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read_to_string(index.as_path()).await {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "no se pudo leer index.html");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn cache_headers(req: Request, next: Next) -> Response {
    let hashed = is_hashed_asset(req.uri().path());
    let mut res = next.run(req).await;
    if res.status().is_success() && !res.headers().contains_key(header::CACHE_CONTROL) {
        if hashed {
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
        } else {
            // El HTML referencia los ficheros con hash: debe revalidarse siempre; lo demás (sin hash
            // en el nombre) también, para no servir versiones viejas "para siempre".
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
    }
    res
}

/// `/assets/<nombre>-<hash de 8 caracteres>.<ext>` (Vite). Solo esos ficheros son inmutables.
fn is_hashed_asset(path: &str) -> bool {
    let Some(name) = path.strip_prefix("/assets/") else { return false };
    if name.contains('/') {
        return false;
    }
    let Some((stem, _ext)) = name.rsplit_once('.') else { return false };
    let b = stem.as_bytes();
    b.len() >= 10
        && b[b.len() - 9] == b'-'
        && b[b.len() - 8..].iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::is_hashed_asset;

    #[test]
    fn hashed_assets() {
        for ok in ["/assets/index-CV56swxT.js", "/assets/Dashboard-E-YmeDM_.js", "/assets/geist-latin-wght-normal-BgDaEnEv.woff2"] {
            assert!(is_hashed_asset(ok), "{ok}");
        }
        for bad in ["/assets/app.js", "/assets/index.js", "/assets/x/index-CV56swxT.js", "/index-CV56swxT.js", "/assets/-CV56swxT.js", "/assets/"] {
            assert!(!is_hashed_asset(bad), "{bad}");
        }
    }
}
