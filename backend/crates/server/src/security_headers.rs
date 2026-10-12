//! Cabeceras de seguridad en todas las respuestas (S-03).

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// Hash (CSP) del único `<style>` en línea que inyecta lightweight-charts (logo de atribución).
/// El test `tests/security.rs::csp_style_hash_matches_dist` lo recalcula contra `frontend/dist`
/// y falla si cambia: actualizar aquí en ese caso.
pub const CHARTS_STYLE_HASH: &str = "sha256-3pRED1tOXas1FXFoPb9TGCjmYe9XQsmO9OV23khV2nY=";

pub fn csp() -> String {
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self' '{CHARTS_STYLE_HASH}'; font-src 'self'; \
         img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; object-src 'none'; \
         frame-ancestors 'none'"
    )
}

const STATIC_HEADERS: [(&str, &str); 6] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("x-frame-options", "DENY"),
    ("permissions-policy", "camera=(), microphone=(), geolocation=()"),
];

pub async fn apply(req: Request, next: Next) -> Response {
    let is_api = req.uri().path() == "/api" || req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    for (name, value) in STATIC_HEADERS {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    let is_html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html && let Ok(v) = HeaderValue::from_str(&csp()) {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}
