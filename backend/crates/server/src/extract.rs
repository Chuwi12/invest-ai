//! Extractores propios: los rechazos de axum son texto plano; aquí se convierten al JSON de error
//! del contrato.

use axum::{
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Path, Request},
    http::request::Parts,
};
use domain::AssetId;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// `axum::Json` con errores en el formato de la API.
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct AppJson<T>(pub T);

/// `axum::extract::Query` con errores en el formato de la API.
#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct AppQuery<T>(pub T);

/// Cuerpo JSON opcional: vacío = `T::default()`. No exige `Content-Type` cuando no hay cuerpo.
pub struct OptionalJson<T>(pub T);

impl<S, T> FromRequest<S> for OptionalJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::invalid(format!("no se pudo leer el cuerpo: {}", e.body_text())))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(T::default()));
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
