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

/// Contadores de peticiones salientes por host (cabeceras recibidas, sin leer el cuerpo).
/// Solo hay un puñado de hosts distintos (los de los proveedores), así que el mapa no crece.
static OUTBOUND: std::sync::Mutex<Vec<(String, u64, u64)>> = std::sync::Mutex::new(Vec::new());

fn record_outbound(host: &str, elapsed: Duration) {
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    let mut v = OUTBOUND.lock().unwrap_or_else(|p| p.into_inner());
    match v.iter_mut().find(|(h, ..)| h == host) {
        Some((_, n, total)) => {
            *n += 1;
            *total = total.saturating_add(micros);
        }
        None => v.push((host.to_owned(), 1, micros)),
    }
}

/// Peticiones HTTP salientes desde el arranque: `(host, peticiones, latencia media en ms hasta
/// recibir cabeceras)`. Sirve para medir el single-flight y la caché sin depender de los logs.
pub fn outbound_stats() -> Vec<(String, u64, f64)> {
    let v = OUTBOUND.lock().unwrap_or_else(|p| p.into_inner());
    v.iter().map(|(h, n, t)| (h.clone(), *n, *t as f64 / *n as f64 / 1000.0)).collect()
}

/// Un único cliente para todo el crate: pool de conexiones, HTTP/2, gzip y brotli.
pub fn build_client(timeout: Duration) -> Result<reqwest::Client, DomainError> {
    build_client_with(timeout, Duration::from_secs(6))
}

/// Cliente para proveedores muy lentos (GDELT): su apretón de manos TLS tarda 8-10 s (medido con
/// curl, openssl y Python: es del servidor, no del cliente ni de IPv6), así que con el
/// `connect_timeout` de 6 s del cliente común SIEMPRE daba «timeout» a los 6,0 s exactos.
pub fn build_slow_client(timeout: Duration) -> Result<reqwest::Client, DomainError> {
    build_client_with(timeout, Duration::from_secs(20))
}

fn build_client_with(timeout: Duration, connect_timeout: Duration) -> Result<reqwest::Client, DomainError> {
    reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_nodelay(true)
        .gzip(true)
        .brotli(true)
        .redirect(redirect_policy())
        .build()
        .map_err(|e| DomainError::Internal(format!("no se pudo crear el cliente HTTP: {e}")))
}

/// Máximo de redirecciones que se siguen (S-08).
const MAX_REDIRECTS: usize = 3;

/// Política de redirecciones: como mucho 3, solo a `https` y nunca a `localhost` ni a IPs
/// privadas, loopback o link-local (evita SSRF y que las claves de la query viajen a otro sitio
/// interno). Si no se cumple, se devuelve la respuesta 3xx tal cual (`check_status` la rechaza).
pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS || !redirect_target_ok(attempt.url()) {
            attempt.stop()
        } else {
            attempt.follow()
        }
    })
}

fn redirect_target_ok(url: &url::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d != "localhost" && !d.ends_with(".localhost") && !d.ends_with(".local") && !d.ends_with(".internal")
        }
        Some(url::Host::Ipv4(ip)) => !is_private_v4(ip),
        Some(url::Host::Ipv6(ip)) => !is_private_v6(ip),
        None => false,
    }
}

fn is_private_v4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
}

fn is_private_v6(ip: std::net::Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    let seg = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
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
    send_limited(req, MAX_BODY_BYTES).await
}

async fn send_limited(req: RequestBuilder, max: usize) -> PResult<Body> {
    let (client, built) = req.build_split();
    let built = built.map_err(map_reqwest)?;
    let host = built.url().host_str().unwrap_or("?").to_owned();
    let t0 = std::time::Instant::now();
    let sent = client.execute(built).await;
    record_outbound(&host, t0.elapsed());
    let resp = sent.map_err(map_reqwest)?;
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited {
            retry_after: retry_after(resp.headers()),
        });
    }
    let bytes = read_limited(resp, max).await?;
    Ok(Body {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Lee el cuerpo por trozos y aborta en cuanto se pasa de `max` bytes **ya descomprimidos**
/// (gzip/brotli/chunked no pueden saltarse el tope, S-07).
async fn read_limited(mut resp: reqwest::Response, max: usize) -> PResult<Vec<u8>> {
    if let Some(len) = resp.content_length()
        && len > max as u64
    {
        return Err(ProviderError::upstream("respuesta demasiado grande"));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(map_reqwest)? {
        if buf.len() + chunk.len() > max {
            return Err(ProviderError::upstream("respuesta demasiado grande"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
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

#[cfg(test)]
mod tests {
    use std::{io::Write, net::SocketAddr};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    /// Servidor de una sola respuesta cruda por conexión (acepta varias).
    async fn serve(response: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let response = response.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(&response).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        addr
    }

    fn client() -> reqwest::Client {
        build_client(Duration::from_secs(5)).unwrap()
    }

    #[tokio::test]
    async fn chunked_body_over_limit_is_aborted() {
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..10 {
            resp.extend_from_slice(b"400\r\n");
            resp.extend_from_slice(&[b'a'; 0x400]);
            resp.extend_from_slice(b"\r\n");
        }
        resp.extend_from_slice(b"0\r\n\r\n");
        let addr = serve(resp).await;
        let url = format!("http://{addr}/");
        let err = send_limited(client().get(&url), 4096).await.err().expect("debe fallar");
        assert!(err.to_string().contains("demasiado grande"), "{err}");
        let ok = send_limited(client().get(&url), 64 * 1024).await.unwrap();
        assert_eq!(ok.text.len(), 10 * 0x400);
    }

    #[tokio::test]
    async fn gzip_bomb_is_limited_after_decoding() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&vec![0u8; 1024 * 1024]).unwrap();
        let gz = enc.finish().unwrap();
        assert!(gz.len() < 4096, "el cuerpo comprimido es pequeño ({} B)", gz.len());
        let mut resp =
            format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n", gz.len()).into_bytes();
        resp.extend_from_slice(&gz);
        let addr = serve(resp).await;
        let err = send_limited(client().get(format!("http://{addr}/")), 64 * 1024).await.err().expect("debe fallar");
        assert!(err.to_string().contains("demasiado grande"), "{err}");
    }

    #[tokio::test]
    async fn redirect_to_plain_http_or_private_is_not_followed() {
        let addr = serve(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let body = send(client().get(format!("http://{addr}/"))).await.unwrap();
        assert_eq!(body.status, StatusCode::FOUND, "la 302 se devuelve sin seguirla");
        assert!(check_status(&body).is_err());
    }

    #[test]
    fn redirect_targets() {
        let ok = |u: &str| redirect_target_ok(&u.parse().unwrap());
        assert!(ok("https://query1.finance.yahoo.com/x"));
        assert!(ok("https://8.8.8.8/x"));
        for bad in [
            "http://query1.finance.yahoo.com/x",
            "https://127.0.0.1/x",
            "https://localhost/x",
            "https://foo.localhost/x",
            "https://10.1.2.3/x",
            "https://192.168.0.1/x",
            "https://172.16.0.1/x",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/x",
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[fe80::1]/x",
            "https://[fd00::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "ftp://example.com/x",
        ] {
            assert!(!ok(bad), "{bad} debe bloquearse");
        }
    }
}
