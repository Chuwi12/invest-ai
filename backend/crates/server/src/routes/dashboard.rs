//! `GET /dashboard`: todo lo que pinta la portada, en una petición y tolerando fallos parciales.

use std::{collections::HashMap, future::Future, time::Duration};

use axum::{Json, extract::State};
use domain::{AssetId, NewsItem, Quote};

use crate::{
    error::ApiResult,
    models::{Dashboard, FavoriteWithQuote, ViewWithQuote},
    repo,
    state::AppState,
};

/// Panel fijo de mercado: ^GSPC, ^IXIC, ^STOXX50E, ^IBEX, BTC, ETH, oro, WTI, EUR/USD, T-note
/// a 10 años y VIX. Las clases siguen la convención de `domain::AssetId`.
pub const MARKET_PANEL: [&str; 11] = [
    "index:^GSPC",
    "index:^IXIC",
    "index:^STOXX50E",
    "index:^IBEX",
    "crypto:BTC",
    "crypto:ETH",
    "commodity:GC=F",
    "commodity:CL=F",
    "forex:EURUSD=X",
    "bond:^TNX",
    "index:^VIX",
];
const MOST_VIEWED: usize = 8;
const RECENT_SEARCHES: i64 = 10;
const RECENT_CONVERSATIONS: i64 = 5;
const LATEST_ANALYSES: i64 = 5;
const NEWS: usize = 10;
/// Un proveedor lento no puede bloquear la portada entera.
const MARKET_TIMEOUT: Duration = Duration::from_secs(8);

/// Espera `fut` con tope de tiempo; si falla o se agota, lo registra y devuelve `None`.
async fn soft<T, E: std::fmt::Display>(what: &str, fut: impl Future<Output = Result<T, E>>) -> Option<T> {
    match tokio::time::timeout(MARKET_TIMEOUT, fut).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "dashboard: {what} no disponible");
            None
        }
        Err(_) => {
            tracing::warn!("dashboard: {what} tardó más de {MARKET_TIMEOUT:?}");
            None
        }
    }
}

fn by_id(quotes: Vec<Quote>) -> HashMap<String, Quote> {
    quotes.into_iter().map(|q| (q.id.to_string(), q)).collect()
}

pub async fn dashboard(State(state): State<AppState>) -> ApiResult<Json<Dashboard>> {
    let panel_ids: Vec<AssetId> = MARKET_PANEL.iter().map(|s| s.parse().expect("panel válido")).collect();

    // Parte local (BD) y, cuando se sabe qué activos del usuario hay, sus cotizaciones.
    let user_part = async {
        let (favorites, viewed, searches, conversations, analyses) = tokio::join!(
            soft("favoritos", repo::favorites::list(&state.db)),
            soft("más vistos", repo::history::most_viewed(&state.db, MOST_VIEWED)),
            soft("búsquedas", repo::history::recent_searches(&state.db, RECENT_SEARCHES)),
            soft("conversaciones", repo::conversations::list(&state.db, RECENT_CONVERSATIONS)),
            soft("análisis", repo::analyses::latest_per_asset(&state.db, LATEST_ANALYSES)),
        );
        let (favorites, viewed) = (favorites.unwrap_or_default(), viewed.unwrap_or_default());
        let mut ids: Vec<AssetId> = favorites.iter().map(|f| f.id.clone()).collect();
        for v in &viewed {
            if !ids.contains(&v.id) {
                ids.push(v.id.clone());
            }
        }
        let quotes = if ids.is_empty() {
            Some(Vec::new())
        } else {
            soft("cotizaciones de favoritos y más vistos", state.market.quotes(&ids)).await
        };
        (favorites, viewed, searches, conversations, analyses, quotes)
    };

    // Panel de mercado y titulares: no dependen de la BD, así que corren a la vez.
    let (user_part, panel, news) = tokio::join!(
        user_part,
        soft("panel de mercado", state.market.quotes(&panel_ids)),
        soft("titulares", state.market.news(None, NEWS)),
    );
    let (favorites, viewed, searches, conversations, analyses, user_quotes) = user_part;

    let user_quotes = by_id(user_quotes.unwrap_or_default());
    let mut panel = by_id(panel.unwrap_or_default());
    let mut news: Vec<NewsItem> = news.unwrap_or_default();
    (state.annotate_news)(&mut news);

    Ok(Json(Dashboard {
        favorites: favorites
            .into_iter()
            .map(|favorite| {
                let quote = user_quotes.get(&favorite.id.to_string()).cloned();
                FavoriteWithQuote { favorite, quote }
            })
            .collect(),
        most_viewed: viewed
            .into_iter()
            .map(|stat| {
                let quote = user_quotes.get(&stat.id.to_string()).cloned();
                ViewWithQuote { stat, quote }
            })
            .collect(),
        recent_searches: searches.unwrap_or_default(),
        recent_conversations: conversations.unwrap_or_default(),
        latest_analyses: analyses.unwrap_or_default(),
        // En el orden fijo del panel, omitiendo lo que el proveedor no devolvió.
        market: panel_ids.iter().filter_map(|id| panel.remove(&id.to_string())).collect(),
        news,
    }))
}
