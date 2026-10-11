//! Error central de la API: todo error HTTP sale como `{ "error": { "code", "message" } }`.

use axum::{
    Json,
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use domain::DomainError;
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    Domain(DomainError),
    /// Errores del propio HTTP que no existen en el dominio (405, 413, 415...).
    Http { status: StatusCode, code: &'static str, message: String },
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Tabla de `docs/API.md`: DomainError → (status, code).
pub fn status_and_code(err: &DomainError) -> (StatusCode, &'static str) {
    match err {
        DomainError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        DomainError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_input"),
        DomainError::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        DomainError::Upstream { .. } => (StatusCode::BAD_GATEWAY, "upstream"),
        DomainError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        DomainError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

/// `DomainError` no es `Clone`; hace falta para repartir un mismo fallo entre varios esperadores.
pub fn clone_domain_error(err: &DomainError) -> DomainError {
    match err {
        DomainError::NotFound(m) => DomainError::NotFound(m.clone()),
        DomainError::InvalidInput(m) => DomainError::InvalidInput(m.clone()),
        DomainError::Upstream { provider, message } => {
            DomainError::Upstream { provider: provider.clone(), message: message.clone() }
        }
        DomainError::RateLimited { provider } => DomainError::RateLimited { provider: provider.clone() },
        DomainError::Unavailable(m) => DomainError::Unavailable(m.clone()),
        DomainError::Internal(m) => DomainError::Internal(m.clone()),
    }
}

impl ApiError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::Domain(DomainError::NotFound(msg.into()))
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Domain(DomainError::InvalidInput(msg.into()))
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Domain(DomainError::Internal(msg.into()))
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Domain(e) => status_and_code(e).0,
            Self::Http { status, .. } => *status,
        }
    }

    fn parts(&self) -> (StatusCode, &str, String) {
        match self {
            Self::Domain(e) => {
                let (status, code) = status_and_code(e);
                (status, code, e.to_string())
            }
            Self::Http { status, code, message } => (*status, code, message.clone()),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (status, code, message) = self.parts();
        write!(f, "{status} {code}: {message}")
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        if status.is_server_error() {
            tracing::error!(%status, code, %message, "error en la petición");
        } else {
            tracing::debug!(%status, code, %message, "petición rechazada");
        }
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        Self::Domain(e)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        // El detalle va al log; al cliente no se le filtran consultas ni rutas de ficheros.
        tracing::error!(error = %e, "error de base de datos");
        Self::internal("error de base de datos")
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        tracing::error!(error = %e, "error de (de)serialización");
        Self::internal("error de serialización")
    }
}

/// Convierte el status de un rechazo de extractor de axum a la tabla de errores del contrato:
/// 400/422 → `invalid_input`; el resto conserva su status.
fn from_rejection(status: StatusCode, message: String) -> ApiError {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => ApiError::invalid(message),
        StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::Http { status, code: "payload_too_large", message: "el cuerpo es demasiado grande".into() }
        }
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::Http {
            status,
            code: "unsupported_media_type",
            message: "se esperaba Content-Type: application/json".into(),
        },
        other => ApiError::Http { status: other, code: "bad_request", message },
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        from_rejection(r.status(), format!("cuerpo JSON no válido: {}", r.body_text()))
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        from_rejection(r.status(), format!("parámetros de consulta no válidos: {}", r.body_text()))
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        from_rejection(r.status(), format!("parámetro de ruta no válido: {}", r.body_text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_errors_map_to_the_documented_table() {
        let cases = [
            (DomainError::NotFound("x".into()), 404, "not_found"),
            (DomainError::InvalidInput("x".into()), 400, "invalid_input"),
            (DomainError::RateLimited { provider: "p".into() }, 429, "rate_limited"),
            (DomainError::Upstream { provider: "p".into(), message: "m".into() }, 502, "upstream"),
            (DomainError::Unavailable("x".into()), 503, "unavailable"),
            (DomainError::Internal("x".into()), 500, "internal"),
        ];
        for (err, status, code) in cases {
            let (s, c) = status_and_code(&err);
            assert_eq!((s.as_u16(), c), (status, code));
        }
    }
}
