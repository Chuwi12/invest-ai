//! `GET /api/knowledge/stats`: estado del archivo de conocimiento (solo lectura).

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{error::{ApiError, ApiResult}, state::AppState};

/// `{"enabled": false}` si el RAG está apagado (o no pudo abrirse); si no, las cifras del archivo.
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Some(kb) = &state.knowledge else { return Ok(Json(json!({ "enabled": false }))) };
    let s = kb.stats().await.map_err(|e| {
        tracing::error!(error = %e, "no se pudieron leer las estadísticas del archivo de conocimiento");
        ApiError::internal("no se pudieron leer las estadísticas del archivo")
    })?;
    Ok(Json(json!({
        "enabled": true,
        "model": s.model,
        "documents": s.documents,
        "chunks": s.chunks,
        "indexed": s.indexed,
        "pending": s.pending,
        "textBytes": s.text_bytes,
        "embeddingBytes": s.embedding_bytes,
        "indexMemoryBytes": s.index_memory_bytes,
        "lexical": s.lexical,
        "lexicalIndexed": s.lexical_indexed,
    })))
}
