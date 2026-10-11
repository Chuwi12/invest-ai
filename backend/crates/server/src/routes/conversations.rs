//! CRUD de conversaciones.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use domain::{AssetId, ChatMessage, Conversation};
use serde::Serialize;

use crate::{
    error::{ApiError, ApiResult},
    extract::{AppJson, OptionalJson},
    models::{CreateConversation, PatchConversation},
    repo::conversations::{self as repo, Patch},
    state::AppState,
};

const MAX_TITLE_CHARS: usize = 200;
/// Activos que pueden colgar de una conversación.
pub const MAX_CONVERSATION_ASSETS: usize = 20;
const LIST_LIMIT: i64 = 500;

pub fn not_found(cid: &str) -> ApiError {
    ApiError::not_found(format!("conversación {cid}"))
}

fn clean_title(title: &str) -> ApiResult<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(ApiError::invalid(format!("title no puede superar {MAX_TITLE_CHARS} caracteres")));
    }
    Ok(title)
}

/// Sin duplicados y conservando el orden.
pub fn dedupe_assets(ids: Vec<AssetId>) -> ApiResult<Vec<AssetId>> {
    let mut out: Vec<AssetId> = Vec::with_capacity(ids.len());
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_CONVERSATION_ASSETS {
        return Err(ApiError::invalid(format!(
            "una conversación admite como máximo {MAX_CONVERSATION_ASSETS} activos"
        )));
    }
    Ok(out)
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<Conversation>>> {
    Ok(Json(repo::list(&state.db, LIST_LIMIT).await?))
}

pub async fn create(
    State(state): State<AppState>,
    OptionalJson(body): OptionalJson<CreateConversation>,
) -> ApiResult<Json<Conversation>> {
    let title = clean_title(body.title.as_deref().unwrap_or_default())?;
    let assets = dedupe_assets(body.asset_ids.unwrap_or_default())?;
    Ok(Json(repo::create(&state.db, &title, &assets).await?))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationWithMessages {
    conversation: Conversation,
    messages: Vec<ChatMessage>,
}

pub async fn get(
    State(state): State<AppState>,
    Path(cid): Path<String>,
) -> ApiResult<Json<ConversationWithMessages>> {
    let conversation = repo::get(&state.db, &cid).await?.ok_or_else(|| not_found(&cid))?;
    let messages = repo::messages(&state.db, &cid).await?;
    Ok(Json(ConversationWithMessages { conversation, messages }))
}

pub async fn patch(
    State(state): State<AppState>,
    Path(cid): Path<String>,
    AppJson(body): AppJson<PatchConversation>,
) -> ApiResult<Json<Conversation>> {
    let patch = Patch {
        title: body.title.as_deref().map(clean_title).transpose()?,
        asset_ids: body.asset_ids.map(dedupe_assets).transpose()?,
        pinned: body.pinned,
    };
    let updated = repo::update(&state.db, &cid, patch).await?.ok_or_else(|| not_found(&cid))?;
    Ok(Json(updated))
}

pub async fn delete(State(state): State<AppState>, Path(cid): Path<String>) -> ApiResult<StatusCode> {
    if repo::delete(&state.db, &cid).await? { Ok(StatusCode::NO_CONTENT) } else { Err(not_found(&cid)) }
}
