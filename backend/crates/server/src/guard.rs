//! Defensa navegador -> servidor local (S-01, S-02, S-04).
//!
//! - `Host` debe ser `localhost`, `127.0.0.1`, `[::1]` (cualquier puerto: el nombre es lo que un
//!   atacante de DNS rebinding no controla) o el authority de algún `FRONTEND_ORIGIN`.
//! - En métodos que cambian estado, en el upgrade del WebSocket y en TODA petición bajo `/api`
//!   (también los GET: M1, varios hacen trabajo con coste, como consultar X o bajar folletos), si
//!   hay `Origin` debe estar en la lista exacta (esquema + host + puerto) o ser el propio servidor
//!   (`Origin` = `Host`).
//! - `Sec-Fetch-Site: cross-site` se rechaza en esos mismos casos.
//! - Los ficheros estáticos del frontend (fuera de `/api`) NO pasan por el control de origen: un
//!   enlace desde otra web a la app debe poder abrirla. El preflight (`OPTIONS`) tampoco: no tiene
//!   efectos y lo resuelve la capa CORS.
//!
//! Sin `Origin` ni `Sec-Fetch-Site` (curl, scripts, el proxy de Vite en desarrollo cuando el
//! navegador no los manda) todo sigue funcionando: un navegador siempre envía `Origin` en
//! POST/PUT/PATCH/DELETE cross-origin, en WebSockets y en los GET con CORS, y `Sec-Fetch-Site` en
//! cualquier petición (Chrome 76+, Firefox 90+, Safari 16.4+). Un `<img src>` cross-origin sin CORS
//! no lleva `Origin` y solo lo delata `Sec-Fetch-Site`; en navegadores que no lo envían la
//! segunda barrera es el tope de activos por hora de X (`social.rs`).

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, state::Settings};

fn authority_of(origin: &str) -> Option<&str> {
    origin.split_once("://").map(|(_, a)| a.trim_end_matches('/'))
}

fn forbidden(message: &'static str) -> Response {
    ApiError::Http { status: StatusCode::FORBIDDEN, code: "forbidden", message: message.into() }.into_response()
}

/// Nombre de host de un authority (`localhost:8787` -> `localhost`, `[::1]:80` -> `::1`).
fn host_name(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    }
}

pub fn host_allowed(host: &str, allowed_origins: &[String]) -> bool {
    let name = host_name(host);
    ["localhost", "127.0.0.1", "::1"].iter().any(|l| name.eq_ignore_ascii_case(l))
        || allowed_origins.iter().filter_map(|o| authority_of(o)).any(|a| a.eq_ignore_ascii_case(host))
}

/// `Origin` permitido: ausente, de la lista exacta o el propio servidor. Sin comodines de puerto.
pub fn origin_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let origin = origin.trim_end_matches('/');
    if allowed_origins.iter().any(|a| a.eq_ignore_ascii_case(origin)) {
        return true;
    }
    // Mismo origen que el propio servidor (un único binario). El `Host` ya se validó antes.
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin.strip_prefix("http://"), host) {
        (Some(authority), Some(host)) => authority.eq_ignore_ascii_case(host) && host_allowed(host, allowed_origins),
        _ => false,
    }
}

fn cross_site(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("cross-site"))
}

/// `/api` o cualquier ruta bajo `/api/`. `req.uri()` lleva la ruta completa (el guard envuelve al
/// router raíz, antes del `nest`).
fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn is_ws_upgrade(headers: &HeaderMap) -> bool {
    headers.get(header::UPGRADE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Comprobación completa de una petición que cambia estado o abre un socket.
pub fn browser_request_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    origin_allowed(headers, allowed_origins) && !cross_site(headers)
}

pub async fn guard(State(settings): State<Arc<Settings>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    // Un navegador siempre envía `Host`; si falta (HTTP/1.0, pruebas en proceso) no hay rebinding posible.
    if let Some(host) = headers.get(header::HOST)
        && !host.to_str().is_ok_and(|h| host_allowed(h, &settings.allowed_origins))
    {
        return forbidden("host no permitido");
    }
    let method = req.method();
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    // M1: los GET de `/api` también hacen trabajo (redes sociales, folletos, caché): una web ajena
    // los dispararía con un `<img src>` aunque no pudiera leer la respuesta.
    let api_read = matches!(*method, Method::GET | Method::HEAD) && is_api_path(req.uri().path());
    if (!safe || api_read || is_ws_upgrade(headers)) && !browser_request_allowed(headers, &settings.allowed_origins) {
        return forbidden("origen no permitido");
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn h(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        if let Some(o) = origin {
            m.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
        }
        m
    }

    #[test]
    fn hosts() {
        let allowed = vec!["http://localhost:5180".to_string(), "https://app.example.com".to_string()];
        for ok in ["localhost:8787", "127.0.0.1:8795", "[::1]:8787", "LOCALHOST", "app.example.com"] {
            assert!(host_allowed(ok, &allowed), "{ok}");
        }
        for bad in ["evil.example:8787", "localhost.evil.example", "127.0.0.1.evil.example:80", "app.example.com:81", "", "[::2]:80"] {
            assert!(!host_allowed(bad, &allowed), "{bad}");
        }
    }

    #[test]
    fn api_paths() {
        for api in ["/api", "/api/health", "/api/assets/stock%3AAAPL/social", "/api/"] {
            assert!(is_api_path(api), "{api}");
        }
        for other in ["/", "/assets/index-CV56swxT.js", "/apix", "/apicture", "/asset/api", "//api/health", "/API/health"] {
            assert!(!is_api_path(other), "{other}");
        }
    }

    #[test]
    fn origins() {
        let allowed = vec!["http://localhost:5180".to_string()];
        let o = |host: &str, origin| origin_allowed(&h(host, origin), &allowed);
        assert!(o("127.0.0.1:8787", None));
        assert!(o("localhost:5180", Some("http://localhost:5180")));
        assert!(o("127.0.0.1:8787", Some("http://localhost:5180/")));
        assert!(o("127.0.0.1:8787", Some("http://127.0.0.1:8787")));
        assert!(!o("127.0.0.1:8787", Some("http://localhost:9999")), "otro puerto de loopback");
        assert!(!o("127.0.0.1:8787", Some("http://127.0.0.1:5180")), "sin comodines de puerto");
        assert!(!o("127.0.0.1:8787", Some("https://evil.example")));
        assert!(!o("evil.example:8787", Some("http://evil.example:8787")), "Host no permitido");
        assert!(!o("127.0.0.1:8787", Some("null")));
        assert!(!o("127.0.0.1:8787", Some("http://localhost.evil.example")));
    }
}
