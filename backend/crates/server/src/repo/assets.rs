//! Caché de `AssetSummary` por id: evita volver a pedirle al proveedor el nombre de un activo
//! que ya hemos visto (favoritos, vistas, búsquedas).

use domain::{AssetId, AssetSummary};
use sqlx::SqlitePool;

use crate::{db::now_ms, error::ApiResult};

pub async fn get(db: &SqlitePool, id: &AssetId) -> ApiResult<Option<AssetSummary>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT summary FROM asset_cache WHERE asset_id = ?1")
        .bind(id.to_string())
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(json,)| serde_json::from_str(&json)).transpose()?)
}

/// Guarda varios resúmenes en una transacción (los resultados de una búsqueda, por ejemplo).
pub async fn put_many(db: &SqlitePool, items: &[AssetSummary]) -> ApiResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let mut tx = db.begin().await?;
    for item in items {
        sqlx::query(
            "INSERT INTO asset_cache (asset_id, summary, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id) DO UPDATE SET summary = excluded.summary, updated_at = excluded.updated_at",
        )
        .bind(item.id.to_string())
        .bind(serde_json::to_string(item)?)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
