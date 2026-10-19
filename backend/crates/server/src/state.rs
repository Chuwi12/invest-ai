//! Estado compartido de la aplicación. Todo se maneja a través de los traits de `domain`.

use std::sync::Arc;

use domain::{Analysis, Analyst, AssetId, IpoData, MarketData, ModelService, NewsItem};
use sqlx::SqlitePool;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    singleflight::SingleFlight,
    watchlist::{Clock, PowerProbe, SystemClock, SystemPower, WatchlistConfig},
};

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
    /// Datos de OPV de EE. UU. (calendario, folleto, rendimiento). Por defecto no disponible:
    /// el cableado debe llamar a `with_ipo`.
    pub ipo: Arc<dyn IpoData>,
    pub analyst: Arc<dyn Analyst>,
    pub db: SqlitePool,
    /// Gestión de modelos de IA (`/models`). Por defecto una versión simulada en memoria:
    /// el cableado real debe llamar a `with_models(analyst.models())`.
    pub models: Arc<dyn ModelService>,
    pub settings: Arc<Settings>,
    /// Añade el sentimiento a las noticias de las respuestas (`ai::annotate_sentiment`).
    pub annotate_news: fn(&mut [NewsItem]),
    /// Un único análisis en curso por activo.
    pub analysis_flights: Arc<SingleFlight<AssetId, Arc<Analysis>>>,
    /// Tareas en segundo plano que deben terminar antes de salir (persistir el chat).
    pub tasks: TaskTracker,
    /// Se cancela al iniciar el apagado: cierra WebSockets y SSE.
    pub shutdown: CancellationToken,
    /// Topes de concurrencia (WebSockets, análisis, chats).
    pub limits: Arc<crate::limits::Limits>,
    /// Lista de seguimiento (desactivada por defecto; `main` la activa con `with_watchlist`).
    pub watchlist: Arc<WatchlistConfig>,
    /// Reloj del programador y de `/watchlist` (inyectable en pruebas).
    pub clock: Arc<dyn Clock>,
    /// Fuente de energía (la pasada diaria se aplaza si el equipo va con batería).
    pub power: Arc<dyn PowerProbe>,
    /// Archivo de conocimiento local (RAG). `None` con `RAG_ENABLED=0` o si no pudo abrirse.
    pub knowledge: Option<Arc<knowledge::KnowledgeBase>>,
    /// Redes sociales (Reddit/X; opinión). `None` sin credenciales: la app es idéntica a la que no las conoce.
    pub social: Option<Arc<crate::social::SocialService>>,
    /// Avisos de configuración de redes (p. ej. «X no se activa: falta el presupuesto»).
    pub social_notes: Arc<Vec<String>>,
}

impl AppState {
    pub fn new(market: Arc<dyn MarketData>, analyst: Arc<dyn Analyst>, db: SqlitePool, settings: Settings) -> Self {
        let tasks = TaskTracker::new();
        Self {
            market,
            ipo: Arc::new(crate::routes::ipo::NoIpoData),
            analyst,
            db,
            models: Arc::new(crate::routes::models::FakeModelService::default()),
            settings: Arc::new(settings),
            annotate_news: |_| {},
            analysis_flights: Arc::new(SingleFlight::new(tasks.clone())),
            tasks,
            shutdown: CancellationToken::new(),
            limits: Arc::default(),
            watchlist: Arc::new(WatchlistConfig::disabled()),
            clock: Arc::new(SystemClock),
            power: Arc::new(SystemPower::default()),
            knowledge: None,
            social: None,
            social_notes: Arc::default(),
        }
    }

    pub fn with_ipo(mut self, ipo: Arc<dyn IpoData>) -> Self {
        self.ipo = ipo;
        self
    }

    pub fn with_models(mut self, models: Arc<dyn ModelService>) -> Self {
        self.models = models;
        self
    }

    pub fn with_watchlist(mut self, cfg: WatchlistConfig, clock: Arc<dyn Clock>) -> Self {
        self.watchlist = Arc::new(cfg);
        self.clock = clock;
        self
    }

    pub fn with_knowledge(mut self, knowledge: Option<Arc<knowledge::KnowledgeBase>>) -> Self {
        self.knowledge = knowledge;
        self
    }

    pub fn with_social(mut self, social: Option<Arc<crate::social::SocialService>>, notes: Vec<String>) -> Self {
        self.social = social;
        self.social_notes = Arc::new(notes);
        self
    }

    pub fn with_power(mut self, power: Arc<dyn PowerProbe>) -> Self {
        self.power = power;
        self
    }

    pub fn with_news_annotator(mut self, f: fn(&mut [NewsItem])) -> Self {
        self.annotate_news = f;
        self
    }
}
