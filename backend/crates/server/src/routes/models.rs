//! Modelos de IA elegibles por el usuario: listar (con requisitos y hardware), cambiar el activo,
//! descargar (SSE) y borrar. La lógica vive en `ai` detrás de `domain::ModelService`.

use std::{
    sync::Mutex,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use domain::{
    DomainError, GpuInfo, HardwareInfo, LlmModel, LoadedModel, ModelRequirements, ModelService, ModelTier, ModelsResponse,
    PullEvent,
};
use futures::{Stream, StreamExt, stream::{self, BoxStream}};
use serde::Deserialize;

use crate::{error::ApiResult, extract::AppJson, state::AppState};

pub async fn list(State(state): State<AppState>) -> Json<ModelsResponse> {
    Json(state.models.list().await)
}

#[derive(Deserialize)]
pub struct SetActive {
    pub id: String,
}

pub async fn set_active(State(state): State<AppState>, AppJson(body): AppJson<SetActive>) -> ApiResult<Json<ModelsResponse>> {
    state.models.set_active(&body.id).await?;
    Ok(Json(state.models.list().await))
}

#[derive(Deserialize)]
pub struct ForceGpu {
    pub force: bool,
}

/// `PUT /models/{id}/gpu` `{force}`: activa o quita «forzar todas las capas en GPU» para ese modelo.
pub async fn set_force_gpu(State(state): State<AppState>, Path(id): Path<String>, AppJson(body): AppJson<ForceGpu>) -> ApiResult<Json<ModelsResponse>> {
    state.models.set_force_gpu(&id, body.force).await?;
    Ok(Json(state.models.list().await))
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    state.models.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// SSE de `PullEvent`. Si el cliente se va, soltar el stream cancela la descarga en Ollama.
pub async fn pull(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let events = state.models.pull(&id).await?;
    let shutdown = state.shutdown.clone();
    let stream = events
        .take_until(async move { shutdown.cancelled().await })
        .map(|ev| {
            Ok(Event::default().json_data(&ev).unwrap_or_else(|_| {
                Event::default().data(r#"{"type":"error","message":"error interno de serialización"}"#)
            }))
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive")))
}

// ---------------------------------------------------------------------------------------------
// Implementación de mentira (INVEST_AI_FAKE y tests): estado en memoria, sin Ollama.
// ---------------------------------------------------------------------------------------------

pub struct FakeModelService {
    state: Mutex<FakeModels>,
}

struct FakeModels {
    active: String,
    installed: Vec<String>,
    pulling: Vec<String>,
    force_gpu: Vec<String>,
}

impl Default for FakeModelService {
    fn default() -> Self {
        Self {
            state: Mutex::new(FakeModels {
                active: "qwen2.5:7b".into(),
                installed: vec!["qwen2.5:7b".into(), "mi-modelo:3b".into()],
                pulling: vec![],
                force_gpu: vec![],
            }),
        }
    }
}

fn fake_model(id: &str, name: &str, tier: Option<ModelTier>, params: &str, vram: f32, curated: bool) -> LlmModel {
    LlmModel {
        id: id.into(),
        name: name.into(),
        family: id.split(':').next().unwrap_or(id).into(),
        parameters: params.into(),
        active_parameters: None,
        quantization: "Q4_K_M".into(),
        context_window: 32768,
        tier,
        supports_tools: true,
        reasoning: id.starts_with("qwen3"),
        description: if curated { format!("{name} (dato simulado)") } else { String::new() },
        requirements: Some(ModelRequirements {
            min_vram_gb: (vram * 0.5).round(),
            recommended_vram_gb: vram,
            min_ram_gb: vram + 1.5,
            disk_gb: vram - 1.0,
            cpu_only_ok: vram < 7.0,
        }),
        curated,
        installed: false,
        active: false,
        force_gpu: false,
        gpu_fallback: None,
    }
}

#[async_trait]
impl ModelService for FakeModelService {
    async fn list(&self) -> ModelsResponse {
        let st = self.state.lock().unwrap();
        let mut models = vec![
            fake_model("qwen3:4b", "Qwen3 4B", Some(ModelTier::Light), "4.0B", 3.7, true),
            fake_model("qwen2.5:7b", "Qwen2.5 7B", Some(ModelTier::Balanced), "7.6B", 5.4, true),
            fake_model("qwen3:14b", "Qwen3 14B", Some(ModelTier::Heavy), "14.8B", 10.9, true),
            fake_model("llama3.3:70b", "Llama 3.3 70B", Some(ModelTier::Max), "70.6B", 43.7, true),
            fake_model("mi-modelo:3b", "mi-modelo:3b", None, "3B", 2.5, false),
        ];
        for m in &mut models {
            m.installed = st.installed.contains(&m.id);
            m.active = m.id == st.active;
            m.force_gpu = st.force_gpu.contains(&m.id);
        }
        // Simulado: el modelo activo está cargado; sin forzar queda al 89 % en GPU (el caso que se ve en una tarjeta de 8 GB).
        let loaded = vec![LoadedModel {
            id: st.active.clone(),
            size: 6_732_182_977,
            size_vram: if st.force_gpu.contains(&st.active) { 6_732_182_977 } else { 6_004_639_006 },
            gpu_share: if st.force_gpu.contains(&st.active) { 1.0 } else { 0.892 },
        }];
        ModelsResponse {
            active: st.active.clone(),
            ollama_reachable: true,
            models,
            hardware: HardwareInfo {
                cpu_model: "CPU simulada".into(),
                cpu_threads: 16,
                ram_total_gb: 31.0,
                ram_available_gb: 20.0,
                gpus: vec![GpuInfo { name: "GPU simulada".into(), vendor: "NVIDIA".into(), vram_total_gb: 8.0, vram_free_gb: Some(6.5) }],
                disk_free_gb: Some(400.0),
            },
            loaded,
            num_gpu: None,
        }
    }

    async fn set_active(&self, id: &str) -> domain::Result<()> {
        let mut st = self.state.lock().unwrap();
        if !st.installed.iter().any(|m| m == id) {
            return Err(DomainError::InvalidInput(format!("el modelo «{id}» no existe o no está instalado")));
        }
        st.active = id.into();
        Ok(())
    }

    async fn set_force_gpu(&self, id: &str, force: bool) -> domain::Result<()> {
        let id = id.trim();
        if id.is_empty() || id.len() > 200 {
            return Err(DomainError::InvalidInput("id de modelo no válido".into()));
        }
        let mut st = self.state.lock().unwrap();
        st.force_gpu.retain(|m| m != id);
        if force {
            st.force_gpu.push(id.into());
        }
        Ok(())
    }

    async fn pull(&self, id: &str) -> domain::Result<BoxStream<'static, PullEvent>> {
        if !["qwen3:4b", "qwen3:14b", "llama3.3:70b", "qwen2.5:7b"].contains(&id) {
            return Err(DomainError::InvalidInput(format!("«{id}» no está en el catálogo: solo se descargan modelos curados")));
        }
        let mut st = self.state.lock().unwrap();
        if st.pulling.iter().any(|m| m == id) {
            return Err(DomainError::Unavailable(format!("«{id}» ya se está descargando")));
        }
        st.pulling.push(id.into());
        if !st.installed.iter().any(|m| m == id) {
            st.installed.push(id.into());
        }
        let id = id.to_string();
        let steps = (1..=4u64).map(|i| PullEvent::Progress { status: format!("pulling {id}"), completed: Some(i * 25), total: Some(100) });
        let all: Vec<PullEvent> = steps.chain([PullEvent::Done]).collect();
        let s = stream::iter(all).then(|e| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            e
        });
        Ok(Box::pin(s))
    }

    async fn delete(&self, id: &str) -> domain::Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.active == id {
            return Err(DomainError::InvalidInput("no se puede borrar el modelo activo: elige otro antes".into()));
        }
        let before = st.installed.len();
        st.installed.retain(|m| m != id);
        st.pulling.retain(|m| m != id);
        if st.installed.len() == before {
            return Err(DomainError::NotFound(format!("modelo «{id}»")));
        }
        Ok(())
    }
}
