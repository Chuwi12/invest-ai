//! Extractores propios: los rechazos de axum son texto plano; aquí se convierten al JSON de error
//! del contrato.

use axum::{
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Path, Request},
    http::{StatusCode, request::Parts},
};
use domain::AssetId;
use serde::de::DeserializeOwned;

use crate::error::{ApiError, from_rejection};

/// `axum::Json` con errores en el formato de la API.
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct AppJson<T>(pub T);

/// `axum::extract::Query` con errores en el formato de la API.
#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct AppQuery<T>(pub T);

/// Cuerpo JSON opcional: vacío = `T::default()`. No exige `Content-Type` cuando no hay cuerpo;
/// si lo hay, exige `application/json` (415 si no): `text/plain` es una petición CORS "simple"
/// que un navegador envía sin preflight (S-02).
pub struct OptionalJson<T>(pub T);

impl<S, T> FromRequest<S> for OptionalJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let json_type = req
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json_content_type);
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| from_rejection(e.status(), format!("no se pudo leer el cuerpo: {}", e.body_text())))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(T::default()));
        }
        if !json_type {
            return Err(from_rejection(StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new()));
        }
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|e| ApiError::invalid(format!("cuerpo JSON no válido: {e}")))
    }
}

/// Parámetro de ruta `{id}`: axum ya lo ha decodificado de percent-encoding (`stock%3AAAPL`).
pub struct AssetPath(pub AssetId);

impl<S: Send + Sync> FromRequestParts<S> for AssetPath {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state).await?;
        Ok(Self(raw.parse()?))
    }
}

fn is_json_content_type(value: &str) -> bool {
    let essence = value.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    essence == "application/json" || (essence.starts_with("application/") && essence.ends_with("+json"))
}
