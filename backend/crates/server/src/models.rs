//! Tipos JSON propios del servidor (los de `docs/API.md` que no viven en `domain`) y cuerpos de
//! petición. Camel case, igual que el resto del contrato.

use chrono::{DateTime, Utc};
use domain::{Analysis, AssetId, AssetSummary, Conversation, Fundamentals, NewsItem, Quote};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub id: AssetId,
    pub summary: AssetSummary,
    pub note: Option<String>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEntry {
    pub query: String,
    pub selected: Option<AssetId>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewStat {
    pub id: AssetId,
    pub summary: AssetSummary,
    pub views: u64,
    pub last_viewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetail {
    pub summary: AssetSummary,
    pub quote: Option<Quote>,
    pub fundamentals: Option<Fundamentals>,
    pub is_favorite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteWithQuote {
    pub favorite: Favorite,
    pub quote: Option<Quote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewWithQuote {
    pub stat: ViewStat,
    pub quote: Option<Quote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub favorites: Vec<FavoriteWithQuote>,
    pub most_viewed: Vec<ViewWithQuote>,
    pub recent_searches: Vec<SearchEntry>,
    pub recent_conversations: Vec<Conversation>,
    pub latest_analyses: Vec<Analysis>,
    pub market: Vec<Quote>,
    pub news: Vec<NewsItem>,
}

// ---------------------------------------------------------------------------------------------
// Cuerpos de petición
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversation {
    pub title: Option<String>,
    pub asset_ids: Option<Vec<AssetId>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchConversation {
    pub title: Option<String>,
    pub asset_ids: Option<Vec<AssetId>>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessage {
    pub content: String,
    #[serde(default)]
    pub asset_ids: Vec<AssetId>,
}

/// `PUT /favorites/{id}`: `note` ausente = conservar la nota actual; `null` o `""` = borrarla.
#[derive(Debug, Default, Deserialize)]
pub struct PutFavorite {
    #[serde(default, deserialize_with = "present_or_null")]
    pub note: Option<Option<String>>,
}

/// Distingue «campo ausente» (`None`) de «campo null» (`Some(None)`).
fn present_or_null<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct RecordSearch {
    pub query: String,
    pub selected: AssetId,
}
