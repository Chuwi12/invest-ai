//! Resolución de `AssetSummary` a partir de un `AssetId`.
//!
//! `MarketData` no tiene una operación «dame el resumen de este id» (propuesta al dominio en
//! `docs/notes/server.md`), así que se resuelve en cascada: caché en BD → búsqueda por símbolo →
//! snapshot completo (caro, último recurso).

use domain::{AssetId, AssetSummary, DomainError};

use crate::{
    error::{ApiError, ApiResult},
    repo,
    state::AppState,
};

/// Guarda resúmenes en la caché sin que un fallo de BD rompa la petición.
pub async fn cache_summaries(state: &AppState, items: &[AssetSummary]) {
    if let Err(e) = repo::assets::put_many(&state.db, items).await {
        tracing::warn!(error = %e, "no se pudo guardar en asset_cache");
    }
}

pub async fn resolve_summary(state: &AppState, id: &AssetId) -> ApiResult<AssetSummary> {
    if let Some(summary) = repo::assets::get(&state.db, id).await? {
        return Ok(summary);
    }
    match state.market.search(&id.symbol, 20).await {
        Ok(results) => {
            cache_summaries(state, &results).await;
            if let Some(found) = results.into_iter().find(|s| s.id == *id) {
                return Ok(found);
            }
        }
        Err(e) => tracing::debug!(%id, error = %e, "búsqueda para resolver el activo falló"),
    }
    let snapshot = state.market.snapshot(id).await?;
    cache_summaries(state, std::slice::from_ref(&snapshot.summary)).await;
    Ok(snapshot.summary)
}

/// Como `resolve_summary`, pero si los proveedores fallan (no «no existe») devuelve un resumen
/// mínimo: marcar un favorito no debe depender de que haya red. No se guarda en la caché.
pub async fn resolve_summary_lenient(state: &AppState, id: &AssetId) -> ApiResult<AssetSummary> {
    match resolve_summary(state, id).await {
        Ok(s) => Ok(s),
        Err(ApiError::Domain(
            DomainError::Upstream { .. } | DomainError::Unavailable(_) | DomainError::RateLimited { .. },
        )) => {
            tracing::warn!(%id, "proveedores no disponibles: se usa un resumen mínimo");
            Ok(AssetSummary {
                id: id.clone(),
                symbol: id.symbol.clone(),
                name: id.symbol.clone(),
                class: id.class,
                exchange: None,
                currency: None,
                country: None,
                source: "local".into(),
            })
        }
        Err(e) => Err(e),
    }
}
