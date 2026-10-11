//! Favoritos.

use std::collections::HashSet;

use domain::{AssetId, AssetSummary};
use sqlx::{FromRow, SqlitePool};

use crate::{
    db::{dt_from_ms, now_ms},
    error::{ApiError, ApiResult},
    models::Favorite,
};

#[derive(FromRow)]
struct Row {
    asset_id: String,
    note: Option<String>,
    summary: String,
    added_at: i64,
}

impl TryFrom<Row> for Favorite {
    type Error = ApiError;

    fn try_from(r: Row) -> Result<Self, ApiError> {
        Ok(Favorite {
            id: r.asset_id.parse()?,
            summary: serde_json::from_str(&r.summary)?,
            note: r.note,
            added_at: dt_from_ms(r.added_at),
        })
    }
}

/// Más recientes primero.
pub async fn list(db: &SqlitePool) -> ApiResult<Vec<Favorite>> {
    sqlx::query_as::<_, Row>(
        "SELECT asset_id, note, summary, added_at FROM favorites ORDER BY added_at DESC, asset_id",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(Favorite::try_from)
    .collect()
}

pub async fn get(db: &SqlitePool, id: &AssetId) -> ApiResult<Option<Favorite>> {
    sqlx::query_as::<_, Row>("SELECT asset_id, note, summary, added_at FROM favorites WHERE asset_id = ?1")
        .bind(id.to_string())
        .fetch_optional(db)
        .await?
        .map(Favorite::try_from)
        .transpose()
}

pub async fn is_favorite(db: &SqlitePool, id: &AssetId) -> ApiResult<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM favorites WHERE asset_id = ?1")
        .bind(id.to_string())
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

pub async fn ids(db: &SqlitePool) -> ApiResult<HashSet<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT asset_id FROM favorites").fetch_all(db).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Alta o actualización. `note`: `None` = conservar la que hubiera; `Some(None)` = borrarla.
/// `added_at` no cambia al repetir el PUT.
pub async fn upsert(
    db: &SqlitePool,
    summary: &AssetSummary,
    note: Option<Option<String>>,
) -> ApiResult<()> {
    let (note_given, note) = match note {
        Some(n) => (true, n),
        None => (false, None),
    };
    sqlx::query(
        "INSERT INTO favorites (asset_id, note, summary, added_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(asset_id) DO UPDATE SET
             summary = excluded.summary,
             note = CASE WHEN ?5 THEN excluded.note ELSE favorites.note END",
    )
    .bind(summary.id.to_string())
    .bind(note)
    .bind(serde_json::to_string(summary)?)
    .bind(now_ms())
    .bind(note_given)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: &AssetId) -> ApiResult<bool> {
    let res = sqlx::query("DELETE FROM favorites WHERE asset_id = ?1")
        .bind(id.to_string())
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
