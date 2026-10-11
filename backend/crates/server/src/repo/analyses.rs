//! Análisis de IA guardados (JSON completo + columnas para filtrar y ordenar).

use domain::{Analysis, AssetId};
use sqlx::SqlitePool;

use crate::error::ApiResult;

fn verdict_str(a: &Analysis) -> String {
    // El veredicto es un enum serializable en snake_case: se guarda tal cual aparece en el JSON.
    serde_json::to_value(a.verdict)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Inserta (o reemplaza, si el id ya existía) un análisis. `origin`: `analyze` o `chat`.
pub async fn insert(db: &SqlitePool, a: &Analysis, origin: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO analyses (id, asset_id, verdict, confidence, price_at_analysis, model, origin, generated_at, json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, verdict = excluded.verdict,
             confidence = excluded.confidence, price_at_analysis = excluded.price_at_analysis,
             model = excluded.model, generated_at = excluded.generated_at",
    )
    .bind(&a.id)
    .bind(a.asset.to_string())
    .bind(verdict_str(a))
    .bind(f64::from(a.confidence))
    .bind(a.price_at_analysis)
    .bind(&a.model)
    .bind(origin)
    .bind(a.generated_at.timestamp_millis())
    .bind(serde_json::to_string(a)?)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn latest(db: &SqlitePool, asset: &AssetId) -> ApiResult<Option<Analysis>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT json FROM analyses WHERE asset_id = ?1 ORDER BY generated_at DESC, id DESC LIMIT 1",
    )
    .bind(asset.to_string())
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(json,)| serde_json::from_str(&json)).transpose()?)
}

/// Histórico de un activo, el más reciente primero.
pub async fn list_for_asset(db: &SqlitePool, asset: &AssetId, limit: i64) -> ApiResult<Vec<Analysis>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT json FROM analyses WHERE asset_id = ?1 ORDER BY generated_at DESC, id DESC LIMIT ?2",
    )
    .bind(asset.to_string())
    .bind(limit)
    .fetch_all(db)
    .await?;
    rows.into_iter().map(|(json,)| Ok(serde_json::from_str(&json)?)).collect()
}

/// El último análisis de cada activo, ordenados por fecha (para el dashboard).
pub async fn latest_per_asset(db: &SqlitePool, limit: i64) -> ApiResult<Vec<Analysis>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT a.json FROM analyses a
          WHERE a.id = (SELECT b.id FROM analyses b WHERE b.asset_id = a.asset_id
                         ORDER BY b.generated_at DESC, b.id DESC LIMIT 1)
          ORDER BY a.generated_at DESC, a.id DESC
          LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(db)
    .await?;
    rows.into_iter().map(|(json,)| Ok(serde_json::from_str(&json)?)).collect()
}
