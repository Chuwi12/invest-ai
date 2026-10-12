//! Límites de concurrencia (S-06): sockets simultáneos y tareas de LLM (análisis, chat).
//! Se rechaza al instante con 429 en vez de encolar sin fin.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::error::ApiError;

pub const MAX_WEBSOCKETS: usize = 64;
pub const MAX_ANALYSES: usize = 2;
pub const MAX_CHATS: usize = 4;

#[derive(Debug)]
pub struct Limits {
    pub websockets: Arc<Semaphore>,
    pub analyses: Arc<Semaphore>,
    pub chats: Arc<Semaphore>,
}

impl Limits {
    pub fn new(websockets: usize, analyses: usize, chats: usize) -> Self {
        Self {
            websockets: Arc::new(Semaphore::new(websockets)),
            analyses: Arc::new(Semaphore::new(analyses)),
            chats: Arc::new(Semaphore::new(chats)),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(MAX_WEBSOCKETS, MAX_ANALYSES, MAX_CHATS)
    }
}

pub fn too_many(what: &str) -> Response {
    ApiError::Http {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "rate_limited",
        message: format!("demasiadas {what} simultáneas: espera a que termine alguna"),
    }
    .into_response()
}

/// Middleware: toma un permiso (sin esperar) y lo conserva hasta que termina de enviarse el
/// cuerpo de la respuesta (también SSE), no solo hasta que el handler devuelve la cabecera.
pub async fn limit(State(sem): State<Arc<Semaphore>>, req: Request, next: Next) -> Response {
    let Ok(permit) = sem.try_acquire_owned() else { return too_many("operaciones de IA") };
    let (parts, body) = next.run(req).await.into_parts();
    let guarded = body.into_data_stream().map(move |chunk| {
        let _keep = &permit;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(guarded))
}
