//! `OllamaAnalyst`: la implementación de `domain::Analyst` sobre Ollama. Es el único tipo que
//! necesita el cableado (`server/src/main.rs`).

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use domain::{
    Analysis, AnalysisDepth, Analyst, AssetSnapshot, ChatEvent, ChatRequest, DomainError, LlmStatus, ModelService,
    Result, SettingsStore,
};
use futures::stream::BoxStream;

use crate::{analysis::AnalysisEngine, chat::ChatEngine, config::AiConfig, models::ModelManager, ollama::OllamaClient};

pub struct OllamaAnalyst {
    cfg: AiConfig,
    client: OllamaClient,
    models: Arc<ModelManager>,
    engine: Arc<AnalysisEngine>,
    chat: Arc<ChatEngine>,
}

impl OllamaAnalyst {
    /// `settings`: almacén de ajustes persistentes (modelo activo). El acceso a datos de mercado
    /// no hace falta aquí: `analyze` recibe el snapshot y `chat` recibe `market` en `ChatRequest`.
    pub fn new(cfg: AiConfig, settings: Arc<dyn SettingsStore>) -> Result<Self> {
        let client = OllamaClient::new(&cfg.base_url, cfg.timeout)?;
        let models = Arc::new(ModelManager::new(client.clone(), cfg.clone(), settings));
        let engine = Arc::new(AnalysisEngine::new(client.clone(), models.clone(), cfg.clone()));
        let chat = Arc::new(ChatEngine::new(client.clone(), models.clone(), engine.clone(), cfg.clone()));
        Ok(Self { cfg, client, models, engine, chat })
    }

    /// Gestión de modelos para las rutas `/models` (`AppState::with_models`).
    pub fn models(&self) -> Arc<dyn ModelService> {
        self.models.clone()
    }

    pub fn model_manager(&self) -> Arc<ModelManager> {
        self.models.clone()
    }
}

#[async_trait]
impl Analyst for OllamaAnalyst {
    async fn analyze(&self, snapshot: &AssetSnapshot) -> Result<Analysis> {
        self.engine.analyze(snapshot, None).await
    }

    async fn analyze_with(&self, snapshot: &AssetSnapshot, depth: Option<AnalysisDepth>) -> Result<Analysis> {
        self.engine.analyze(snapshot, depth).await
    }

    fn chat(&self, req: ChatRequest) -> BoxStream<'static, ChatEvent> {
        self.chat.clone().run(req)
    }

    async fn status(&self) -> LlmStatus {
        let model = self.models.active().await;
        let tags = tokio::time::timeout(Duration::from_secs(4), self.client.tags()).await;
        match tags {
            Ok(Ok(tags)) => {
                let installed: Vec<String> = tags.into_iter().map(|t| t.name).collect();
                let present = installed.contains(&model);
                let caps = self.models.caps(&model).await;
                LlmStatus {
                    available: present,
                    base_url: self.cfg.base_url.clone(),
                    supports_tools: present && caps.tools,
                    error: (!present).then(|| format!("el modelo «{model}» no está instalado en Ollama (elige otro en Ajustes o ejecuta `ollama pull {model}`)")),
                    model,
                    installed_models: installed,
                }
            }
            other => {
                let error = match other {
                    Ok(Err(DomainError::Unavailable(m))) => m,
                    Ok(Err(e)) => e.to_string(),
                    _ => "Ollama no respondió a tiempo".to_string(),
                };
                LlmStatus {
                    available: false,
                    base_url: self.cfg.base_url.clone(),
                    model,
                    installed_models: vec![],
                    supports_tools: false,
                    error: Some(error),
                }
            }
        }
    }
}
