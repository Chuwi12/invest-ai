//! Gestión de modelos: catálogo curado + modelos instalados, modelo activo en caliente,
//! descargas (una por modelo) y borrado. Implementa `domain::ModelService`.
//!
//! El usuario elige el modelo; la app solo informa de requisitos y del hardware detectado.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use domain::{
    DomainError, LlmModel, LoadedModel, ModelRequirements, ModelService, ModelTier, ModelsResponse, PullEvent, Result,
    SettingsStore,
};
use futures::{StreamExt, stream::BoxStream};
use tokio::sync::{OnceCell, RwLock};

use crate::{
    catalog::{self, CatalogEntry},
    config::{ALL_GPU_LAYERS, AiConfig},
    hardware::HardwareProbe,
    ollama::{OllamaClient, ShowInfo, TagModel},
};

/// Clave en la tabla `settings`.
pub const ACTIVE_MODEL_KEY: &str = "llm.active_model";
/// Prefijo de la clave por modelo de «forzar todas las capas en GPU» (`llm.force_gpu.<id>` = `1`).
pub const FORCE_GPU_KEY_PREFIX: &str = "llm.force_gpu.";
const SHOW_TTL: Duration = Duration::from_secs(120);

/// Lo que el pipeline necesita saber del modelo activo.
#[derive(Debug, Clone)]
pub struct ModelCaps {
    pub id: String,
    pub tools: bool,
    pub thinking: bool,
    /// Parámetros totales y activos (miles de millones) si se conocen.
    pub params_b: Option<f64>,
    pub active_params_b: Option<f64>,
    pub tier: Option<ModelTier>,
    pub context_length: Option<u32>,
    /// Capas que se piden en GPU (`options.num_gpu`): `None` = decide Ollama (por defecto).
    pub num_gpu: Option<i32>,
}

impl ModelCaps {
    /// Tamaño «efectivo» para decidir la profundidad: en un MoE pesan más los activos que el total.
    pub fn effective_params_b(&self) -> Option<f64> {
        match (self.params_b, self.active_params_b) {
            (Some(t), Some(a)) => Some((t * 0.35 + a * 0.65).max(a)),
            (Some(t), None) => Some(t),
            _ => None,
        }
    }
}

pub struct ModelManager {
    client: OllamaClient,
    cfg: AiConfig,
    settings: Arc<dyn SettingsStore>,
    hardware: HardwareProbe,
    active: RwLock<Option<String>>,
    loaded: OnceCell<()>,
    pulls: Arc<Mutex<HashSet<String>>>,
    show_cache: Mutex<HashMap<String, (Instant, ShowInfo)>>,
}

impl ModelManager {
    pub fn new(client: OllamaClient, cfg: AiConfig, settings: Arc<dyn SettingsStore>) -> Self {
        let hardware = HardwareProbe::new(cfg.models_dir.clone());
        Self {
            client,
            cfg,
            settings,
            hardware,
            active: RwLock::new(None),
            loaded: OnceCell::new(),
            pulls: Arc::default(),
            show_cache: Mutex::default(),
        }
    }

    /// Modelo activo: el guardado en `settings`, o `LLM_MODEL` si no hay ninguno.
    pub async fn active(&self) -> String {
        self.loaded
            .get_or_init(|| async {
                match self.settings.get(ACTIVE_MODEL_KEY).await {
                    Ok(Some(v)) if !v.trim().is_empty() => *self.active.write().await = Some(v.trim().to_string()),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "no se pudo leer el modelo activo de la BD"),
                }
            })
            .await;
        self.active.read().await.clone().unwrap_or_else(|| self.cfg.model.clone())
    }

    async fn show_cached(&self, id: &str) -> Option<ShowInfo> {
        if let Some((at, s)) = self.show_cache.lock().unwrap().get(id)
            && at.elapsed() < SHOW_TTL
        {
            return Some(s.clone());
        }
        let info = self.client.show(id).await.ok()?;
        self.show_cache.lock().unwrap().insert(id.to_string(), (Instant::now(), info.clone()));
        Some(info)
    }

    /// Capacidades del modelo: `/api/show` si responde; si no, el catálogo; si no, lo prudente.
    pub async fn caps(&self, id: &str) -> ModelCaps {
        let cat = catalog::find(id);
        let show = self.show_cached(id).await;
        let (tools, thinking) = match &show {
            Some(s) if !s.capabilities.is_empty() => (s.has("tools"), s.has("thinking")),
            _ => (cat.is_some_and(|c| c.supports_tools), cat.is_some_and(|c| c.reasoning)),
        };
        let params_b = cat
            .and_then(|c| catalog::parse_params(&c.parameters))
            .or_else(|| show.as_ref().and_then(|s| catalog::parse_params(&s.details.parameter_size)))
            .or_else(|| params_from_name(id))
            .map(|p| if p > 1e6 { p / 1e9 } else { p });
        let active_params_b = cat.and_then(|c| c.active_parameters.as_deref()).and_then(catalog::parse_params).map(|p| p / 1e9);
        ModelCaps {
            id: id.to_string(),
            tools,
            thinking,
            params_b,
            active_params_b,
            tier: cat.map(|c| c.tier),
            context_length: show
                .as_ref()
                .and_then(|s| s.arch_u64("context_length"))
                .map(|c| c.min(u64::from(u32::MAX)) as u32)
                .or(cat.map(|c| c.context_window)),
            num_gpu: self.num_gpu_for(id).await,
        }
    }

    /// ¿Tiene el usuario activado «forzar todas las capas en GPU» para este modelo?
    pub async fn force_gpu(&self, id: &str) -> bool {
        match self.settings.get(&format!("{FORCE_GPU_KEY_PREFIX}{id}")).await {
            Ok(Some(v)) => v.trim() == "1",
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(error = %e, "no se pudo leer el ajuste de GPU del modelo");
                false
            }
        }
    }

    /// `num_gpu` efectivo para un modelo: nada si forzar la GPU falló por memoria hace poco (reparto
    /// automático), todas las capas si el usuario lo fuerza en ese modelo, `LLM_NUM_GPU` si está definido,
    /// y si no `None` (decide Ollama, como antes).
    pub async fn num_gpu_for(&self, id: &str) -> Option<i32> {
        if self.client.gpu_fallback(id).is_some() {
            return None;
        }
        if self.force_gpu(id).await {
            return Some(ALL_GPU_LAYERS);
        }
        self.cfg.num_gpu
    }

    pub fn client(&self) -> &OllamaClient {
        &self.client
    }

    pub fn hardware(&self) -> &HardwareProbe {
        &self.hardware
    }

    async fn build_model(&self, entry: &CatalogEntry, installed: bool, active: &str) -> LlmModel {
        LlmModel {
            id: entry.id.clone(),
            name: entry.name.clone(),
            family: entry.family.clone(),
            parameters: entry.parameters.clone(),
            active_parameters: entry.active_parameters.clone(),
            quantization: entry.quantization.clone(),
            context_window: entry.context_window,
            tier: Some(entry.tier),
            supports_tools: entry.supports_tools,
            reasoning: entry.reasoning,
            description: entry.description.clone(),
            requirements: Some(catalog::entry_requirements(entry, self.cfg.num_ctx, self.cfg.kv_cache_factor)),
            curated: true,
            installed,
            active: entry.id == active,
            force_gpu: false,
            gpu_fallback: None,
        }
    }

    /// Modelo instalado que no está en el catálogo: datos de `/api/tags` y `/api/show`.
    async fn build_uncurated(&self, tag: &TagModel, active: &str) -> LlmModel {
        let show = self.show_cached(&tag.name).await;
        let d = show.as_ref().map(|s| &s.details).unwrap_or(&tag.details);
        let requirements: Option<ModelRequirements> = show
            .as_ref()
            .and_then(catalog::profile_from_show)
            .map(|kv| catalog::requirements(tag.size, &kv, 1.0, self.cfg.num_ctx, self.cfg.kv_cache_factor));
        LlmModel {
            id: tag.name.clone(),
            name: tag.name.clone(),
            family: d.family.clone(),
            parameters: d.parameter_size.clone(),
            active_parameters: None,
            quantization: d.quantization_level.clone(),
            context_window: show.as_ref().and_then(|s| s.arch_u64("context_length")).unwrap_or(0).min(u64::from(u32::MAX)) as u32,
            tier: None,
            supports_tools: show.as_ref().is_some_and(|s| s.has("tools")),
            reasoning: show.as_ref().is_some_and(|s| s.has("thinking")),
            description: String::new(),
            requirements,
            curated: false,
            installed: true,
            active: tag.name == active,
            force_gpu: false,
            gpu_fallback: None,
        }
    }
}

/// «qwen3:8b», «llama3.1:70b-instruct-q4» → miles de millones (heurística para modelos sin metadatos).
fn params_from_name(id: &str) -> Option<f64> {
    let tag = id.split(':').nth(1)?;
    tag.split(['-', '_']).find_map(|part| {
        let p = part.strip_suffix('b')?;
        p.parse::<f64>().ok().map(|n| n * 1e9)
    })
}

#[async_trait]
impl ModelService for ModelManager {
    async fn list(&self) -> ModelsResponse {
        let active = self.active().await;
        let (tags, hardware, running) = tokio::join!(self.client.tags(), self.hardware.get(), self.client.ps());
        let (reachable, installed) = match tags {
            Ok(t) => (true, t),
            Err(e) => {
                tracing::debug!(error = %e, "Ollama no responde al listar modelos");
                (false, Vec::new())
            }
        };
        let installed_ids: HashSet<&str> = installed.iter().map(|t| t.name.as_str()).collect();
        let mut curated: Vec<&CatalogEntry> = catalog::catalog().iter().collect();
        curated.sort_by_key(|e| (tier_rank(e.tier), e.disk_bytes));
        let mut models = Vec::with_capacity(curated.len());
        for e in curated {
            models.push(self.build_model(e, installed_ids.contains(e.id.as_str()), &active).await);
        }
        let extra: Vec<&TagModel> = installed.iter().filter(|t| catalog::find(&t.name).is_none()).collect();
        let built = futures::future::join_all(extra.iter().map(|t| self.build_uncurated(t, &active))).await;
        models.extend(built);
        for m in &mut models {
            m.force_gpu = self.force_gpu(&m.id).await;
            m.gpu_fallback = self.client.gpu_fallback(&m.id);
        }
        let loaded = running
            .unwrap_or_default()
            .into_iter()
            .map(|r| LoadedModel {
                gpu_share: if r.size > 0 { (r.size_vram as f64 / r.size as f64).clamp(0.0, 1.0) as f32 } else { 0.0 },
                id: r.name,
                size: r.size,
                size_vram: r.size_vram,
            })
            .collect();
        ModelsResponse { active, ollama_reachable: reachable, models, hardware, loaded, num_gpu: self.cfg.num_gpu }
    }

    async fn set_active(&self, id: &str) -> Result<()> {
        let id = id.trim();
        if id.is_empty() || id.len() > 200 {
            return Err(DomainError::InvalidInput("id de modelo no válido".into()));
        }
        let tags = self.client.tags().await?;
        if !tags.iter().any(|t| t.name == id) {
            return Err(DomainError::InvalidInput(format!(
                "el modelo «{id}» no existe o no está instalado en Ollama"
            )));
        }
        self.settings.set(ACTIVE_MODEL_KEY, id).await?;
        // Garantiza que `active()` no sobrescriba este valor con el de la BD en una carga tardía.
        self.active().await;
        *self.active.write().await = Some(id.to_string());
        tracing::info!(model = id, "modelo activo cambiado");
        Ok(())
    }

    async fn pull(&self, id: &str) -> Result<BoxStream<'static, PullEvent>> {
        let entry = catalog::find(id)
            .ok_or_else(|| DomainError::InvalidInput(format!("«{id}» no está en el catálogo: solo se descargan modelos curados")))?;
        if !self.pulls.lock().unwrap().insert(entry.id.clone()) {
            return Err(DomainError::Unavailable(format!("«{id}» ya se está descargando")));
        }
        struct Release(Arc<Mutex<HashSet<String>>>, String);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.lock().unwrap().remove(&self.1);
            }
        }
        let release = Release(self.pulls.clone(), entry.id.clone());
        let mut inner = self.client.pull_stream(&entry.id);
        let stream = async_stream::stream! {
            let _release = release;
            while let Some(ev) = inner.next().await {
                let end = matches!(ev, PullEvent::Done | PullEvent::Error { .. });
                yield ev;
                if end {
                    break;
                }
            }
        };
        Ok(Box::pin(stream))
    }

    async fn set_force_gpu(&self, id: &str, force: bool) -> Result<()> {
        let id = id.trim();
        if id.is_empty() || id.len() > 200 {
            return Err(DomainError::InvalidInput("id de modelo no válido".into()));
        }
        self.settings.set(&format!("{FORCE_GPU_KEY_PREFIX}{id}"), if force { "1" } else { "0" }).await?;
        // Un cambio explícito del usuario borra el fallo anterior: se vuelve a intentar.
        self.client.clear_gpu_fallback(id);
        tracing::info!(model = id, force, "ajuste de GPU del modelo cambiado");
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        if id == self.active().await {
            return Err(DomainError::InvalidInput("no se puede borrar el modelo activo: elige otro antes".into()));
        }
        // Solo se borra lo que está instalado: nada de nombres arbitrarios hacia Ollama.
        let installed = self.client.tags().await?;
        if !installed.iter().any(|t| t.name == id) {
            return Err(DomainError::NotFound(format!("modelo «{id}» no instalado")));
        }
        self.client.delete(id).await?;
        self.show_cache.lock().unwrap().remove(id);
        Ok(())
    }
}

fn tier_rank(t: ModelTier) -> u8 {
    match t {
        ModelTier::Light => 0,
        ModelTier::Balanced => 1,
        ModelTier::Heavy => 2,
        ModelTier::Max => 3,
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
