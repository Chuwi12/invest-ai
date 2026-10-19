//! `OllamaAnalyst`: la implementación de `domain::Analyst` sobre Ollama. Es el único tipo que
//! necesita el cableado (`server/src/main.rs`).

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use domain::{
    Analysis, AnalysisDepth, AnalysisEvent, Analyst, AssetSnapshot, ChatEvent, ChatRequest, DomainError, LlmStatus, ModelService,
    Result, SettingsStore,
};
use futures::{StreamExt, stream::BoxStream};
use knowledge::KnowledgeSearch;

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
        Self::with_knowledge(cfg, settings, None)
    }

    /// Como `new`, con el archivo de conocimiento local (RAG). Con `None` es exactamente `new`.
    pub fn with_knowledge(cfg: AiConfig, settings: Arc<dyn SettingsStore>, knowledge: Option<Arc<dyn KnowledgeSearch>>) -> Result<Self> {
        let client = OllamaClient::new(&cfg.base_url, cfg.timeout)?;
        let models = Arc::new(ModelManager::new(client.clone(), cfg.clone(), settings));
        let engine = Arc::new(AnalysisEngine::new(client.clone(), models.clone(), cfg.clone()).with_knowledge(knowledge));
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

    fn analyze_stream(self: Arc<Self>, snapshot: AssetSnapshot, depth: Option<AnalysisDepth>) -> BoxStream<'static, AnalysisEvent> {
        /// Aborta el análisis si se suelta el stream (el cliente HTTP de Ollama cancela la generación).
        struct AbortOnDrop(tokio::task::JoinHandle<()>);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = self.engine.clone();
        let handle = tokio::spawn(async move {
            let last = match engine.analyze_progress(&snapshot, depth, Some(tx.clone())).await {
                Ok(analysis) => AnalysisEvent::Done { analysis },
                Err(e) => AnalysisEvent::Error { code: e.code().to_string(), message: e.to_string() },
            };
            let _ = tx.send(last);
        });
        async_stream::stream! {
            let _guard = AbortOnDrop(handle);
            while let Some(ev) = rx.recv().await {
                let end = !matches!(ev, AnalysisEvent::Stage { .. });
                yield ev;
                if end {
                    break;
                }
            }
        }
        .boxed()
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
