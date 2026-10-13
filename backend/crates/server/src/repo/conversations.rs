//! Conversaciones y mensajes.

use domain::{Analysis, AssetId, ChatMessage, Conversation, Role, ToolCallRecord};
use sqlx::{FromRow, SqlitePool};

use crate::{
    db::{dt_from_ms, new_id, now_ms},
    error::{ApiError, ApiResult},
};

/// Título que se muestra mientras la conversación no tiene uno propio (en BD es `''`).
pub const DEFAULT_TITLE: &str = "Nueva conversación";
const PREVIEW_CHARS: usize = 140;

#[derive(FromRow)]
struct ConversationRow {
    id: String,
    title: String,
    asset_ids: String,
    pinned: bool,
    created_at: i64,
    updated_at: i64,
    message_count: i64,
    preview: Option<String>,
}

impl TryFrom<ConversationRow> for Conversation {
    type Error = ApiError;

    fn try_from(r: ConversationRow) -> Result<Self, ApiError> {
        Ok(Conversation {
            id: r.id,
            title: if r.title.is_empty() { DEFAULT_TITLE.to_string() } else { r.title },
            asset_ids: serde_json::from_str(&r.asset_ids)?,
            pinned: r.pinned,
            message_count: r.message_count.max(0) as u32,
            last_message_preview: r.preview.map(|p| preview(&p)),
            created_at: dt_from_ms(r.created_at),
            updated_at: dt_from_ms(r.updated_at),
        })
    }
}

fn preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(PREVIEW_CHARS).collect();
    format!("{}…", cut.trim_end())
}

// El contador y la vista previa se calculan al vuelo: con el índice (conversation_id, created_at)
// son búsquedas por índice y no pueden quedar desincronizados con `messages`.
const SELECT_CONVERSATION: &str = "
    SELECT c.id, c.title, c.asset_ids, c.pinned, c.created_at, c.updated_at,
        (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count,
        (SELECT substr(m.content, 1, 600) FROM messages m
          WHERE m.conversation_id = c.id AND m.role IN ('user', 'assistant')
          ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS preview
    FROM conversations c";

pub async fn create(db: &SqlitePool, title: &str, asset_ids: &[AssetId]) -> ApiResult<Conversation> {
    let id = new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO conversations (id, title, asset_ids, pinned, created_at, updated_at)
         VALUES (?1, ?2, ?3, 0, ?4, ?4)",
    )
    .bind(&id)
    .bind(title)
    .bind(serde_json::to_string(asset_ids)?)
    .bind(now)
    .execute(db)
    .await?;
    get(db, &id).await?.ok_or_else(|| ApiError::internal("la conversación recién creada no existe"))
}

pub async fn get(db: &SqlitePool, id: &str) -> ApiResult<Option<Conversation>> {
    sqlx::query_as::<_, ConversationRow>(&format!("{SELECT_CONVERSATION} WHERE c.id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .map(Conversation::try_from)
        .transpose()
}

/// Más recientes primero (por última actividad).
pub async fn list(db: &SqlitePool, limit: i64) -> ApiResult<Vec<Conversation>> {
    sqlx::query_as::<_, ConversationRow>(&format!(
        "{SELECT_CONVERSATION} ORDER BY c.updated_at DESC, c.id DESC LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(Conversation::try_from)
    .collect()
}

/// Cambios parciales: `None` = no tocar. No modifica `updated_at` (es la última *actividad*).
#[derive(Default)]
pub struct Patch {
    pub title: Option<String>,
    pub asset_ids: Option<Vec<AssetId>>,
    pub pinned: Option<bool>,
}

/// Devuelve `None` si la conversación no existe.
pub async fn update(db: &SqlitePool, id: &str, patch: Patch) -> ApiResult<Option<Conversation>> {
    let asset_ids = patch.asset_ids.as_deref().map(serde_json::to_string).transpose()?;
    let res = sqlx::query(
        "UPDATE conversations
            SET title = COALESCE(?1, title),
                asset_ids = COALESCE(?2, asset_ids),
                pinned = COALESCE(?3, pinned)
          WHERE id = ?4",
    )
    .bind(patch.title)
    .bind(asset_ids)
    .bind(patch.pinned)
    .bind(id)
    .execute(db)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    get(db, id).await
}

pub async fn delete(db: &SqlitePool, id: &str) -> ApiResult<bool> {
    // Los mensajes caen por ON DELETE CASCADE.
    let res = sqlx::query("DELETE FROM conversations WHERE id = ?1").bind(id).execute(db).await?;
    Ok(res.rows_affected() > 0)
}

/// Pone título solo si la conversación sigue sin él (atómico: gana el primer mensaje).
pub async fn set_title_if_empty(db: &SqlitePool, id: &str, title: &str) -> ApiResult<()> {
    sqlx::query("UPDATE conversations SET title = ?1 WHERE id = ?2 AND title = ''")
        .bind(title)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_asset_ids(db: &SqlitePool, id: &str, asset_ids: &[AssetId]) -> ApiResult<()> {
    sqlx::query("UPDATE conversations SET asset_ids = ?1 WHERE id = ?2")
        .bind(serde_json::to_string(asset_ids)?)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Mensajes
// ---------------------------------------------------------------------------------------------

pub struct NewMessage<'a> {
    pub id: &'a str,
    pub conversation_id: &'a str,
    pub role: Role,
    pub content: &'a str,
    pub tool_calls: &'a [ToolCallRecord],
    pub analysis: Option<&'a Analysis>,
    pub model: Option<&'a str>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    /// Respuesta cortada por un error: el texto es parcial.
    pub truncated: bool,
    pub created_at_ms: i64,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn parse_role(s: &str) -> ApiResult<Role> {
    Ok(match s {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        other => return Err(ApiError::internal(format!("rol desconocido en la BD: {other}"))),
    })
}

/// Inserta el mensaje y actualiza la última actividad de la conversación en una transacción.
/// Falla con clave foránea si la conversación se borró mientras tanto.
pub async fn insert_message(db: &SqlitePool, m: &NewMessage<'_>) -> ApiResult<()> {
    let tool_calls = serde_json::to_string(m.tool_calls)?;
    let analysis = m.analysis.map(serde_json::to_string).transpose()?;
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, role, content, tool_calls, analysis, model,
                               prompt_tokens, completion_tokens, truncated, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(m.id)
    .bind(m.conversation_id)
    .bind(role_str(m.role))
    .bind(m.content)
    .bind(tool_calls)
    .bind(analysis)
    .bind(m.model)
    .bind(m.prompt_tokens)
    .bind(m.completion_tokens)
    .bind(m.truncated)
    .bind(m.created_at_ms)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE conversations SET updated_at = MAX(updated_at, ?1) WHERE id = ?2")
        .bind(m.created_at_ms)
        .bind(m.conversation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(FromRow)]
struct MessageRow {
    id: String,
    conversation_id: String,
    role: String,
    content: String,
    tool_calls: String,
    analysis: Option<String>,
    truncated: bool,
    created_at: i64,
}

impl TryFrom<MessageRow> for ChatMessage {
    type Error = ApiError;

    fn try_from(r: MessageRow) -> Result<Self, ApiError> {
        Ok(ChatMessage {
            id: r.id,
            conversation_id: r.conversation_id,
            role: parse_role(&r.role)?,
            content: r.content,
            tool_calls: serde_json::from_str(&r.tool_calls)?,
            analysis: r.analysis.as_deref().map(serde_json::from_str).transpose()?,
            truncated: r.truncated,
            created_at: dt_from_ms(r.created_at),
        })
    }
}

const MESSAGE_COLUMNS: &str = "id, conversation_id, role, content, tool_calls, analysis, truncated, created_at";

/// Todos los mensajes, en orden cronológico.
pub async fn messages(db: &SqlitePool, conversation_id: &str) -> ApiResult<Vec<ChatMessage>> {
    sqlx::query_as::<_, MessageRow>(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1 ORDER BY created_at, id"
    ))
    .bind(conversation_id)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(ChatMessage::try_from)
    .collect()
}

/// Los últimos `limit` mensajes, en orden cronológico (contexto para el LLM).
pub async fn recent_messages(
    db: &SqlitePool,
    conversation_id: &str,
    limit: i64,
) -> ApiResult<Vec<ChatMessage>> {
    sqlx::query_as::<_, MessageRow>(&format!(
        "SELECT * FROM (
             SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2
         ) ORDER BY created_at, id"
    ))
    .bind(conversation_id)
    .bind(limit)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(ChatMessage::try_from)
    .collect()
}

// ---------------------------------------------------------------------------------------------
// Título automático
// ---------------------------------------------------------------------------------------------

/// Primeros ~60 caracteres del mensaje, cortados en palabra completa.
pub fn auto_title(message: &str) -> String {
    const MAX: usize = 60;
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX {
        return flat;
    }
    let cut: String = flat.chars().take(MAX).collect();
    // Si el corte cae dentro de una palabra, se retrocede al último espacio; si no hay espacios
    // razonables (una URL larga, por ejemplo), se corta tal cual.
    let next_is_space = flat.chars().nth(MAX).is_some_and(char::is_whitespace);
    let cut = if next_is_space {
        cut.as_str()
    } else {
        match cut.rfind(' ') {
            Some(i) if i >= MAX / 3 => &cut[..i],
            _ => cut.as_str(),
        }
    };
    let cut = cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:.-–—(¿¡".contains(c));
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_titles_are_kept() {
        assert_eq!(auto_title("  ¿Qué   opinas de AAPL? "), "¿Qué opinas de AAPL?");
    }

    #[test]
    fn long_titles_cut_at_word_boundary() {
        let t = auto_title("¿Qué opinas de Apple tras los resultados del último trimestre fiscal y su guía?");
        assert!(t.ends_with('…'));
        let body = t.trim_end_matches('…');
        assert!(body.chars().count() <= 60, "{t}");
        // Palabra completa: lo que queda es un prefijo del mensaje que termina antes de un espacio.
        assert_eq!(body, "¿Qué opinas de Apple tras los resultados del último");
    }

    #[test]
    fn title_never_splits_a_word() {
        let msg = format!("{} {}", "a".repeat(59), "bbbb");
        assert_eq!(auto_title(&msg), format!("{}…", "a".repeat(59)));
        // Si el corte cae justo al final de una palabra se conserva entera.
        let msg = format!("{} {}", "a".repeat(60), "bbbb");
        assert_eq!(auto_title(&msg), format!("{}…", "a".repeat(60)));
    }

    #[test]
    fn unbroken_text_is_hard_cut() {
        let t = auto_title(&"x".repeat(200));
        assert_eq!(t.chars().count(), 61);
    }
}
