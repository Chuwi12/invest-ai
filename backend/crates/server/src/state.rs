//! Estado compartido de la aplicación. Todo se maneja a través de los traits de `domain`.

use std::sync::Arc;

use domain::{Analysis, Analyst, AssetId, MarketData, NewsItem};
use sqlx::SqlitePool;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::singleflight::SingleFlight;

/// Ajustes de ejecución que necesitan las rutas.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Idioma de respuesta del asistente.
    pub language: String,
    /// Orígenes del frontend (para validar el `Origin` del WebSocket).
    pub allowed_origins: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { language: "es".into(), allowed_origins: Vec::new() }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub market: Arc<dyn MarketData>,
    pub analyst: Arc<dyn Analyst>,
    pub db: SqlitePool,
    pub settings: Arc<Settings>,
    /// Añade el sentimiento a las noticias de las respuestas (`ai::annotate_sentiment`).
    pub annotate_news: fn(&mut [NewsItem]),
    /// Un único análisis en curso por activo.
    pub analysis_flights: Arc<SingleFlight<AssetId, Arc<Analysis>>>,
    /// Tareas en segundo plano que deben terminar antes de salir (persistir el chat).
    pub tasks: TaskTracker,
    /// Se cancela al iniciar el apagado: cierra WebSockets y SSE.
    pub shutdown: CancellationToken,
}

impl AppState {
    pub fn new(market: Arc<dyn MarketData>, analyst: Arc<dyn Analyst>, db: SqlitePool, settings: Settings) -> Self {
        let tasks = TaskTracker::new();
        Self {
            market,
            analyst,
            db,
            settings: Arc::new(settings),
            annotate_news: |_| {},
            analysis_flights: Arc::new(SingleFlight::new(tasks.clone())),
            tasks,
            shutdown: CancellationToken::new(),
        }
    }

    pub fn with_news_annotator(mut self, f: fn(&mut [NewsItem])) -> Self {
        self.annotate_news = f;
        self
    }
}
