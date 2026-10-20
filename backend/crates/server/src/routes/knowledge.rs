//! `GET /api/knowledge/stats`: estado del archivo de conocimiento (solo lectura).

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{error::{ApiError, ApiResult}, state::AppState};

/// Interruptores y recuentos. Con el archivo cerrado (los tres interruptores a 0, `INVEST_AI_FAKE=1` o
/// no pudo abrirse) solo `{ enabled: false, archive: false, analysis: false, chat: false }`.
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let Some(kb) = &state.knowledge else {
        return Ok(Json(json!({ "enabled": false, "archive": false, "analysis": false, "chat": false })));
    };
    let s = kb.stats().await.map_err(|e| {
        tracing::error!(error = %e, "no se pudieron leer las estadísticas del archivo de conocimiento");
        ApiError::internal("no se pudieron leer las estadísticas del archivo")
    })?;
    let cfg = kb.config();
    Ok(Json(json!({
        "enabled": true,
        "archive": cfg.modes.archive,
        "analysis": cfg.modes.analysis,
        "chat": cfg.modes.chat,
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
        "maxChunks": cfg.max_chunks,
        "newsRetentionDays": cfg.retention_news_days,
        "filingRetentionDays": cfg.retention_filing_days,
    })))
}
