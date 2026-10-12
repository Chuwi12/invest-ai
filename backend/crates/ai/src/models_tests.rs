use std::{collections::HashMap, sync::Mutex, time::Duration};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::testutil::MockOllama;

#[derive(Default)]
pub struct MemSettings(pub Mutex<HashMap<String, String>>);

#[async_trait]
impl SettingsStore for MemSettings {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    async fn set(&self, key: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

fn manager(m: &MockOllama, store: Arc<MemSettings>) -> ModelManager {
    let cfg = AiConfig { base_url: m.url.clone(), model: "qwen2.5:7b".into(), ..AiConfig::default() };
    ModelManager::new(OllamaClient::new(&m.url, Duration::from_secs(5)).unwrap(), cfg, store)
}

fn tags_with(names: &[&str]) -> serde_json::Value {
    json!({"models": names.iter().map(|n| json!({"name": n, "size": 1_000_000_000u64,
        "details": {"family": "x", "parameter_size": "3B", "quantization_level": "Q4_K_M"}})).collect::<Vec<_>>()})
}

#[tokio::test]
async fn lists_curated_and_uncurated_installed_models() {
    let m = MockOllama::start().await;
    *m.state.tags.lock().unwrap() = tags_with(&["qwen2.5:7b", "mimodelo:1b"]);
    let mgr = manager(&m, Arc::new(MemSettings::default()));
    let r = mgr.list().await;
    assert!(r.ollama_reachable);
    assert_eq!(r.active, "qwen2.5:7b");
    let q = r.models.iter().find(|x| x.id == "qwen2.5:7b").unwrap();
    assert!(q.curated && q.installed && q.active);
    assert!(q.requirements.as_ref().unwrap().recommended_vram_gb > 4.0);
    let other = r.models.iter().find(|x| x.id == "qwen3:8b").unwrap();
    assert!(other.curated && !other.installed && !other.active);
    let un = r.models.iter().find(|x| x.id == "mimodelo:1b").unwrap();
    assert!(!un.curated && un.installed && un.tier.is_none());
    assert!(un.requirements.is_some(), "requisitos desde /api/show");
    // Orden: curados por nivel y tamaño, luego los no curados.
    let pos = |id: &str| r.models.iter().position(|x| x.id == id).unwrap();
    assert!(pos("qwen3:4b") < pos("qwen3:14b") && pos("qwen3:14b") < pos("qwen3:32b") && pos("qwen3:32b") < pos("mimodelo:1b"));
}

#[tokio::test]
async fn unreachable_ollama_still_lists_the_catalog() {
    let cfg = AiConfig::default();
    let client = OllamaClient::new("http://127.0.0.1:9", Duration::from_secs(1)).unwrap();
    let mgr = ModelManager::new(client, cfg, Arc::new(MemSettings::default()));
    let r = mgr.list().await;
    assert!(!r.ollama_reachable);
    assert!(r.models.len() >= 15 && r.models.iter().all(|x| !x.installed));
}

#[tokio::test]
async fn active_model_persists_and_validates() {
    let m = MockOllama::start().await;
    *m.state.tags.lock().unwrap() = tags_with(&["qwen2.5:7b", "qwen3:4b"]);
    let store = Arc::new(MemSettings::default());
    let mgr = manager(&m, store.clone());
    assert_eq!(mgr.active().await, "qwen2.5:7b", "por defecto LLM_MODEL");
    assert!(matches!(mgr.set_active("qwen3:8b").await, Err(DomainError::InvalidInput(_))), "no instalado");
    assert!(matches!(mgr.set_active("").await, Err(DomainError::InvalidInput(_))));
    mgr.set_active("qwen3:4b").await.unwrap();
    assert_eq!(mgr.active().await, "qwen3:4b");
    assert_eq!(store.get(ACTIVE_MODEL_KEY).await.unwrap().as_deref(), Some("qwen3:4b"));
    assert!(mgr.list().await.models.iter().find(|x| x.id == "qwen3:4b").unwrap().active);
    // Un gestor nuevo (reinicio) recupera el guardado.
    let again = manager(&m, store);
    assert_eq!(again.active().await, "qwen3:4b");
}

#[tokio::test]
async fn delete_refuses_active_and_pull_refuses_unknown() {
    let m = MockOllama::start().await;
    *m.state.tags.lock().unwrap() = tags_with(&["qwen2.5:7b", "qwen3:4b"]);
    let mgr = manager(&m, Arc::new(MemSettings::default()));
    assert!(matches!(mgr.delete("qwen2.5:7b").await, Err(DomainError::InvalidInput(_))));
    mgr.delete("qwen3:4b").await.unwrap();
    assert_eq!(m.state.deleted.lock().unwrap().as_slice(), ["qwen3:4b"]);
    assert!(matches!(mgr.delete("fantasma:1b").await, Err(DomainError::NotFound(_))));
    assert!(matches!(mgr.pull("evil/model:latest").await, Err(DomainError::InvalidInput(_))));
}

#[tokio::test]
async fn only_one_pull_per_model_and_released_afterwards() {
    let m = MockOllama::start().await;
    *m.state.pull_lines.lock().unwrap() = vec![json!({"status": "success"}).to_string()];
    let mgr = manager(&m, Arc::new(MemSettings::default()));
    let first = mgr.pull("qwen3:4b").await.unwrap();
    assert!(matches!(mgr.pull("qwen3:4b").await, Err(DomainError::Unavailable(_))), "segunda descarga simultánea");
    // Otro modelo sí se puede descargar a la vez.
    drop(mgr.pull("qwen3:8b").await.unwrap());
    let events: Vec<PullEvent> = first.collect().await;
    assert!(matches!(events.last(), Some(PullEvent::Done)));
    // Terminada (y soltada), se puede volver a pedir.
    assert!(mgr.pull("qwen3:4b").await.is_ok());
}

#[tokio::test]
async fn caps_come_from_show_and_fall_back_to_catalog() {
    let m = MockOllama::start().await;
    let mgr = manager(&m, Arc::new(MemSettings::default()));
    let c = mgr.caps("qwen2.5:7b").await;
    assert!(c.tools && !c.thinking);
    assert_eq!(c.tier, Some(ModelTier::Balanced));
    assert!((c.params_b.unwrap() - 7.6).abs() < 0.2);
    let dead = ModelManager::new(OllamaClient::new("http://127.0.0.1:9", Duration::from_secs(1)).unwrap(), AiConfig::default(), Arc::new(MemSettings::default()));
    let c = dead.caps("qwen3:8b").await;
    assert!(c.tools && c.thinking, "del catálogo");
    let c = dead.caps("desconocido:13b").await;
    assert!(!c.tools);
    assert_eq!(c.params_b, Some(13.0));
}
