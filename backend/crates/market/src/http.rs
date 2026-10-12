//! Cliente HTTP compartido y utilidades para mapear respuestas a `ProviderError`.

use std::time::Duration;

use domain::DomainError;
use reqwest::{RequestBuilder, StatusCode, header};
use serde::de::DeserializeOwned;

use crate::providers::{PResult, ProviderError};

/// User-Agent de navegador: Yahoo rechaza o limita agresivamente los UA de librerías.
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Tope de tamaño de respuesta que aceptamos leer (company_tickers de la SEC pesa ~1 MB).
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Un único cliente para todo el crate: pool de conexiones, HTTP/2, gzip y brotli.
pub fn build_client(timeout: Duration) -> Result<reqwest::Client, DomainError> {
    reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(6))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_nodelay(true)
        .gzip(true)
        .brotli(true)
        .build()
        .map_err(|e| DomainError::Internal(format!("no se pudo crear el cliente HTTP: {e}")))
}

/// Convierte un error de `reqwest` en `ProviderError` sin filtrar la URL (puede llevar claves).
pub fn map_reqwest(e: reqwest::Error) -> ProviderError {
    let e = e.without_url();
    if e.is_timeout() {
        ProviderError::Upstream("timeout".into())
    } else if e.is_connect() {
        ProviderError::Upstream(format!("sin conexión: {e}"))
    } else {
        ProviderError::Upstream(format!("error de red: {e}"))
    }
}

fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| Duration::from_secs(s.min(3600)))
}

/// Respuesta ya leída: estado + cuerpo.
pub struct Body {
    pub status: StatusCode,
    pub text: String,
}

/// Envía la petición y lee el cuerpo (con tope de tamaño). 429 → `RateLimited`.
/// El resto de códigos los interpreta quien llama (algunos proveedores usan 404/400 con
/// significado propio).
pub async fn send(req: RequestBuilder) -> PResult<Body> {
    let resp = req.send().await.map_err(map_reqwest)?;
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited {
            retry_after: retry_after(resp.headers()),
        });
    }
    if let Some(len) = resp.content_length()
        && len as usize > MAX_BODY_BYTES
    {
        return Err(ProviderError::upstream("respuesta demasiado grande"));
    }
    let bytes = resp.bytes().await.map_err(map_reqwest)?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(ProviderError::upstream("respuesta demasiado grande"));
    }
    Ok(Body {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// `send` exigiendo 2xx. 404 → `NotFound`; 401/403 → error de acceso; resto → `Upstream`.
pub async fn send_ok(req: RequestBuilder) -> PResult<String> {
    let body = send(req).await?;
    check_status(&body)?;
    Ok(body.text)
}

pub fn check_status(body: &Body) -> PResult<()> {
    let s = body.status;
    if s.is_success() {
        return Ok(());
    }
    Err(match s {
        StatusCode::NOT_FOUND => ProviderError::NotFound("HTTP 404".into()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ProviderError::Upstream(format!("acceso denegado (HTTP {})", s.as_u16()))
        }
        _ => ProviderError::Upstream(format!("HTTP {} {}", s.as_u16(), snippet(&body.text, 120))),
    })
}

/// Deserializa JSON indicando de qué proveedor viene el fallo de formato.
pub fn parse_json<T: DeserializeOwned>(text: &str) -> PResult<T> {
    serde_json::from_str(text).map_err(|e| {
        ProviderError::Upstream(format!("respuesta no válida ({e}): {}", snippet(text, 80)))
    })
}

pub async fn get_json<T: DeserializeOwned>(req: RequestBuilder) -> PResult<T> {
    parse_json(&send_ok(req).await?)
}

/// Primeros `n` caracteres en una sola línea, para mensajes de error.
pub fn snippet(text: &str, n: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut it = one_line.chars();
    let head: String = it.by_ref().take(n).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
