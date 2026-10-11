//! Historial de búsquedas y de visitas a activos.

use domain::{AssetId, AssetSummary};
use sqlx::{FromRow, SqlitePool};
use tokio::sync::Mutex;

use crate::{
    db::{dt_from_ms, now_ms},
    error::{ApiError, ApiResult},
    models::{SearchEntry, ViewStat},
};

/// Vida media del peso de una visita: tras 7 días cuenta la mitad.
pub const VIEW_HALF_LIFE_MS: f64 = 7.0 * 24.0 * 3600.0 * 1000.0;
/// Entradas máximas que se conservan en el historial de búsquedas.
const MAX_SEARCH_ROWS: i64 = 500;
/// Una búsqueda que extiende a la anterior dentro de esta ventana (mientras se teclea) la sustituye.
const TYPING_WINDOW_MS: i64 = 15_000;
/// Ventana para asociar «el usuario eligió X» con la búsqueda que la originó.
const SELECT_WINDOW_MS: i64 = 10 * 60 * 1000;

// ---------------------------------------------------------------------------------------------
// Búsquedas
// ---------------------------------------------------------------------------------------------

/// Registra una búsqueda. Si la anterior es un prefijo (o igual) de esta, sin selección y
/// reciente, es la misma «tecleada»: se reemplaza para no llenar el historial de «a», «ap», «app».
pub async fn record_search(db: &SqlitePool, query: &str) -> ApiResult<()> {
    let now = now_ms();
    // Una sola sentencia: lee y borra de forma atómica (sin transacción de lectura→escritura,
    // que en SQLite puede fallar con SQLITE_BUSY al promocionarse).
    sqlx::query(
        "DELETE FROM search_history
          WHERE id = (SELECT id FROM search_history ORDER BY id DESC LIMIT 1)
            AND selected IS NULL AND at > ?2
            AND substr(lower(?1), 1, length(query)) = lower(query)",
    )
    .bind(query)
    .bind(now - TYPING_WINDOW_MS)
    .execute(db)
    .await?;
    sqlx::query("INSERT INTO search_history (query, selected, at) VALUES (?1, NULL, ?2)")
        .bind(query)
        .bind(now)
        .execute(db)
        .await?;
    sqlx::query(
        "DELETE FROM search_history
          WHERE id <= (SELECT id FROM search_history ORDER BY id DESC LIMIT 1 OFFSET ?1)",
    )
    .bind(MAX_SEARCH_ROWS)
    .execute(db)
    .await?;
    Ok(())
}

/// Anota qué activo eligió el usuario: se asocia a la última búsqueda igual y reciente sin
/// selección; si no la hay, se crea una entrada nueva.
pub async fn record_selection(db: &SqlitePool, query: &str, selected: &AssetId) -> ApiResult<()> {
    let now = now_ms();
    let res = sqlx::query(
        "UPDATE search_history SET selected = ?2, at = ?3
          WHERE id = (SELECT id FROM search_history
                       WHERE lower(query) = lower(?1) AND selected IS NULL AND at > ?4
                       ORDER BY id DESC LIMIT 1)",
    )
    .bind(query)
    .bind(selected.to_string())
    .bind(now)
    .bind(now - SELECT_WINDOW_MS)
    .execute(db)
    .await?;
    if res.rows_affected() == 0 {
        sqlx::query("INSERT INTO search_history (query, selected, at) VALUES (?1, ?2, ?3)")
            .bind(query)
            .bind(selected.to_string())
            .bind(now)
            .execute(db)
            .await?;
    }
    Ok(())
}

#[derive(FromRow)]
struct SearchRow {
    query: String,
    selected: Option<String>,
    at: i64,
}

/// Búsquedas distintas (sin distinguir mayúsculas), la más reciente de cada una primero.
pub async fn recent_searches(db: &SqlitePool, limit: i64) -> ApiResult<Vec<SearchEntry>> {
    sqlx::query_as::<_, SearchRow>(
        "SELECT query, selected, at FROM search_history
          WHERE id IN (SELECT MAX(id) FROM search_history GROUP BY lower(query))
          ORDER BY id DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|r| {
        Ok(SearchEntry {
            query: r.query,
            selected: r.selected.as_deref().map(str::parse).transpose()?,
            at: dt_from_ms(r.at),
        })
    })
    .collect::<Result<Vec<_>, ApiError>>()
}

pub async fn clear_searches(db: &SqlitePool) -> ApiResult<()> {
    sqlx::query("DELETE FROM search_history").execute(db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Visitas
// ---------------------------------------------------------------------------------------------

/// SQLite no trae `pow()` (el SQLite embebido se compila sin funciones matemáticas), así que
/// la actualización del puntaje es leer→escribir; este cerrojo serializa las visitas dentro del
/// proceso para no perder incrementos.
static VIEW_LOCK: Mutex<()> = Mutex::const_new(());

/// Peso de las visitas pasado `elapsed_ms` desde la última: decaimiento exponencial.
pub fn decay(score: f64, elapsed_ms: i64) -> f64 {
    score * 0.5_f64.powf(elapsed_ms.max(0) as f64 / VIEW_HALF_LIFE_MS)
}

/// Cuenta una visita. El puntaje es la suma de visitas, cada una ponderada por 0.5^(edad/7 días),
/// mantenida de forma incremental: `score = decay(score) + 1`.
pub async fn record_view(db: &SqlitePool, summary: &AssetSummary) -> ApiResult<()> {
    let id = summary.id.to_string();
    let now = now_ms();
    let json = serde_json::to_string(summary)?;
    let _guard = VIEW_LOCK.lock().await;
    let prev: Option<(f64, i64)> =
        sqlx::query_as("SELECT score, last_viewed_at FROM asset_views WHERE asset_id = ?1")
            .bind(&id)
            .fetch_optional(db)
            .await?;
    let score = prev.map_or(1.0, |(score, last)| decay(score, now - last) + 1.0);
    sqlx::query(
        "INSERT INTO asset_views (asset_id, views, score, first_viewed_at, last_viewed_at, summary)
         VALUES (?1, 1, ?2, ?3, ?3, ?4)
         ON CONFLICT(asset_id) DO UPDATE SET
             views = views + 1, score = excluded.score,
             last_viewed_at = excluded.last_viewed_at, summary = excluded.summary",
    )
    .bind(&id)
    .bind(score)
    .bind(now)
    .bind(json)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct ViewRow {
    asset_id: String,
    views: i64,
    score: f64,
    last_viewed_at: i64,
    summary: String,
}

/// Activos más vistos, ordenados por visitas ponderadas por recencia.
pub async fn most_viewed(db: &SqlitePool, limit: usize) -> ApiResult<Vec<ViewStat>> {
    let now = now_ms();
    // Una visita de hace > 120 días pesa < 1e-5: no aporta nada y así la consulta está acotada.
    let rows = sqlx::query_as::<_, ViewRow>(
        "SELECT asset_id, views, score, last_viewed_at, summary FROM asset_views
          WHERE last_viewed_at > ?1 ORDER BY last_viewed_at DESC LIMIT 2000",
    )
    .bind(now - 120 * 24 * 3600 * 1000)
    .fetch_all(db)
    .await?;
    let mut ranked: Vec<(f64, ViewRow)> =
        rows.into_iter().map(|r| (decay(r.score, now - r.last_viewed_at), r)).collect();
    // Desempate por visita más reciente para que el orden sea estable.
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.last_viewed_at.cmp(&a.1.last_viewed_at)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, r)| {
            Ok(ViewStat {
                id: r.asset_id.parse()?,
                summary: serde_json::from_str(&r.summary)?,
                views: r.views.max(0) as u64,
                last_viewed_at: dt_from_ms(r.last_viewed_at),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_halves_every_half_life() {
        let hl = VIEW_HALF_LIFE_MS as i64;
        assert!((decay(8.0, hl) - 4.0).abs() < 1e-9);
        assert!((decay(8.0, 3 * hl) - 1.0).abs() < 1e-9);
        assert_eq!(decay(8.0, 0), 8.0);
        // Un reloj que va hacia atrás no debe inflar el puntaje.
        assert_eq!(decay(8.0, -1000), 8.0);
    }
}
