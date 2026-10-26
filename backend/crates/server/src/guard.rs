//! Defensa navegador -> servidor local (S-01, S-02, S-04).
//!
//! - `Host` debe ser `localhost`, `127.0.0.1`, `[::1]` (cualquier puerto: el nombre es lo que un
//!   atacante de DNS rebinding no controla) o el authority de algún `FRONTEND_ORIGIN`.
//! - En métodos que cambian estado, en el upgrade del WebSocket y en TODA petición bajo `/api`
//!   (también los GET: M1, varios hacen trabajo con coste, como consultar X o bajar folletos), si
//!   hay `Origin` debe estar en la lista exacta (esquema + host + puerto) o ser el propio servidor
//!   (`Origin` = `Host`).
//! - `Sec-Fetch-Site: cross-site` se rechaza en esos mismos casos.
//! - Una cabecera `Host`, `Origin` o `Sec-Fetch-Site` repetida (o con una lista separada por comas)
//!   es ambigua y se rechaza siempre con 403 (N6): un cliente que mandara `Sec-Fetch-Site:
//!   same-origin` y luego `cross-site` pasaba la comprobación del primer valor, y lo mismo con dos
//!   `Origin` o dos `Host`. Un navegador nunca las repite.
//! - Sin cabecera `Host` (HTTP/2 y h2c llevan el host en `:authority`, que hyper expone como el
//!   authority de la URI; también la forma absoluta de HTTP/1.1) se valida ese authority con la misma
//!   regla, y se usa también para decidir si un `Origin` es el propio servidor. Si hay `Host` Y
//!   authority, los dos deben ser válidos. Sin ninguno de los dos (HTTP/1.0, pruebas en proceso) no hay
//!   rebinding posible, porque un navegador siempre manda uno.
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
    http::{HeaderMap, HeaderName, Method, StatusCode, Uri, header},
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
    // Un `Host` con credenciales (`localhost:80@evil.example`) no es un host: su nombre real es el de después de la `@`.
    if host.contains('@') {
        return false;
    }
    let name = host_name(host);
    ["localhost", "127.0.0.1", "::1"].iter().any(|l| name.eq_ignore_ascii_case(l))
        || allowed_origins.iter().filter_map(|o| authority_of(o)).any(|a| a.eq_ignore_ascii_case(host))
}

/// `Origin` permitido: ausente, de la lista exacta o el propio servidor. Sin comodines de puerto.
pub fn origin_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    origin_allowed_for(headers, headers.get(header::HOST).and_then(|h| h.to_str().ok()), allowed_origins)
}

/// [`origin_allowed`] con el host efectivo ya calculado (cabecera `Host` o authority de la URI).
fn origin_allowed_for(headers: &HeaderMap, host: Option<&str>, allowed_origins: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let origin = origin.trim_end_matches('/');
    if allowed_origins.iter().any(|a| a.eq_ignore_ascii_case(origin)) {
        return true;
    }
    // Mismo origen que el propio servidor (un único binario). El `Host` ya se validó antes.
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

fn browser_request_allowed_for(headers: &HeaderMap, host: Option<&str>, allowed_origins: &[String]) -> bool {
    origin_allowed_for(headers, host, allowed_origins) && !cross_site(headers)
}

/// ¿Alguna de las cabeceras de las que depende el guard (`Host`, `Origin`, `Sec-Fetch-Site`) viene repetida,
/// como lista separada por comas (un proxy puede haberlas fusionado) o con bytes que no son texto? Con más
/// de un valor el servidor y el guard podrían quedarse con valores distintos, así que se rechaza.
fn ambiguous_headers(headers: &HeaderMap) -> bool {
    const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");
    [header::HOST, header::ORIGIN, SEC_FETCH_SITE].iter().any(|name| {
        let mut values = headers.get_all(name).iter();
        let Some(first) = values.next() else { return false };
        values.next().is_some() || !first.to_str().is_ok_and(|v| !v.contains(','))
    })
}

/// Authority de la URI (`:authority` de HTTP/2, forma absoluta de HTTP/1.1), si lo trae.
fn uri_authority(uri: &Uri) -> Option<&str> {
    uri.authority().map(axum::http::uri::Authority::as_str)
}

pub async fn guard(State(settings): State<Arc<Settings>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if ambiguous_headers(headers) {
        return forbidden("cabecera Host, Origin o Sec-Fetch-Site repetida o no válida");
    }
    // Host efectivo: la cabecera `Host` y, si no hay, el authority de la URI (h2c). Si vienen los dos, los dos
    // deben ser válidos; uno solo basta para saber que no es un navegador sin `Host` ni authority.
    let host_header = headers.get(header::HOST);
    let authority = uri_authority(req.uri());
    if let Some(host) = host_header
        && !host.to_str().is_ok_and(|h| host_allowed(h, &settings.allowed_origins))
    {
        return forbidden("host no permitido");
    }
    if let Some(a) = authority
        && !host_allowed(a, &settings.allowed_origins)
    {
        return forbidden("host no permitido");
    }
    let host = host_header.and_then(|h| h.to_str().ok()).or(authority);
    let method = req.method();
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    // M1: los GET de `/api` también hacen trabajo (redes sociales, folletos, caché): una web ajena
    // los dispararía con un `<img src>` aunque no pudiera leer la respuesta.
    let api_read = matches!(*method, Method::GET | Method::HEAD) && is_api_path(req.uri().path());
    if (!safe || api_read || is_ws_upgrade(headers)) && !browser_request_allowed_for(headers, host, &settings.allowed_origins) {
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
    fn hosts_with_credentials_are_not_hosts() {
        let allowed = vec!["http://localhost:5180".to_string()];
        for bad in ["localhost:80@evil.example", "user@localhost", "localhost@127.0.0.1:8787", "a@[::1]:80"] {
            assert!(!host_allowed(bad, &allowed), "{bad}");
        }
    }

    fn map(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(HeaderName::from_static(k), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn repeated_or_listed_security_headers_are_ambiguous() {
        // N6: `Sec-Fetch-Site: same-origin` + `cross-site` pasaba porque solo se miraba el primer valor.
        for dup in ["host", "origin", "sec-fetch-site"] {
            assert!(ambiguous_headers(&map(&[(dup, "a"), (dup, "a")])), "{dup} repetida con el mismo valor");
            assert!(ambiguous_headers(&map(&[(dup, "same-origin"), (dup, "cross-site")])), "{dup} repetida con valores distintos");
            assert!(ambiguous_headers(&map(&[(dup, "same-origin, cross-site")])), "{dup} como lista");
        }
        assert!(ambiguous_headers(&{
            let mut m = HeaderMap::new();
            m.insert(header::HOST, HeaderValue::from_bytes(b"local\xffhost").unwrap());
            m
        }), "bytes que no son texto");
        // Una sola copia de cada una, o ninguna, no es ambiguo; otras cabeceras sí pueden repetirse.
        assert!(!ambiguous_headers(&HeaderMap::new()));
        assert!(!ambiguous_headers(&map(&[("host", "localhost:8787"), ("origin", "http://localhost:5180"), ("sec-fetch-site", "same-origin")])));
        assert!(!ambiguous_headers(&map(&[("accept", "a"), ("accept", "b"), ("cookie", "x=1"), ("cookie", "y=2")])));
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
