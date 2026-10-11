//! Configuración del analista. Todo se puede fijar por entorno (`AiConfig::from_env`).

use std::time::Duration;

/// Modelo por defecto. Se cambia solo si otro gana en el benchmark (ver `docs/LLM.md`).
pub const DEFAULT_MODEL: &str = "qwen2.5:7b";

#[derive(Debug, Clone)]
pub struct AiConfig {
    /// URL base de Ollama, sin barra final.
    pub base_url: String,
    pub model: String,
    /// Ventana de contexto que se pide a Ollama. Debe ser constante entre peticiones: si cambia,
    /// Ollama recarga el modelo.
    pub num_ctx: u32,
    pub temperature: f32,
    /// Tiempo máximo **sin recibir datos** del modelo (incluye la carga del modelo y el
    /// procesado del prompt). No limita la duración total de una respuesta larga.
    pub timeout: Duration,
    /// Idioma de respuesta (BCP-47), p. ej. "es".
    pub language: String,
    /// Cuánto mantiene Ollama el modelo cargado tras la última petición.
    pub keep_alive: String,
    /// Rondas máximas de tool calling por mensaje de chat.
    pub max_tool_rounds: usize,
    /// Tope de tokens generados por petición (`num_predict`). `None` = el del modelo.
    pub max_output_tokens: Option<u32>,
    /// Si se activa el razonamiento en modelos que lo soportan (qwen3, gpt-oss…). Alarga mucho
    /// la latencia en una GPU de 8 GB, así que por defecto está apagado.
    pub think: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434".into(),
            model: DEFAULT_MODEL.into(),
            // 8 GB de VRAM: con un 7–8B en Q4 el KV-cache de 8192 tokens ocupa ~0,5 GB.
            num_ctx: 8192,
            temperature: 0.2,
            timeout: Duration::from_secs(180),
            language: "es".into(),
            keep_alive: "30m".into(),
            max_tool_rounds: 6,
            max_output_tokens: Some(2048),
            think: false,
        }
    }
}

impl AiConfig {
    /// Lee `LLM_BASE_URL`, `LLM_MODEL`, `LLM_NUM_CTX`, `LLM_TEMPERATURE`, `LLM_TIMEOUT_SECS`,
    /// `LLM_LANGUAGE` (y, opcionales, `LLM_KEEP_ALIVE`, `LLM_MAX_OUTPUT_TOKENS`, `LLM_THINK`).
    /// Los valores vacíos o ilegibles se ignoran y se usa el valor por defecto.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Igual que `from_env` pero con una fuente de variables inyectable (para tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let text = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if let Some(v) = text("LLM_BASE_URL") {
            c.base_url = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = text("LLM_MODEL") {
            c.model = v;
        }
        if let Some(v) = text("LLM_NUM_CTX").and_then(|v| v.parse::<u32>().ok()) {
            c.num_ctx = v.clamp(1024, 262_144);
        }
        if let Some(v) = text("LLM_TEMPERATURE").and_then(|v| v.parse::<f32>().ok()) {
            c.temperature = v.clamp(0.0, 2.0);
        }
        if let Some(v) = text("LLM_TIMEOUT_SECS").and_then(|v| v.parse::<u64>().ok()) {
            c.timeout = Duration::from_secs(v.clamp(5, 3600));
        }
        if let Some(v) = text("LLM_LANGUAGE") {
            c.language = v;
        }
        if let Some(v) = text("LLM_KEEP_ALIVE") {
            c.keep_alive = v;
        }
        if let Some(v) = text("LLM_MAX_OUTPUT_TOKENS").and_then(|v| v.parse::<u32>().ok()) {
            c.max_output_tokens = (v > 0).then_some(v);
        }
        if let Some(v) = text("LLM_THINK") {
            c.think = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "si" | "sí");
        }
        c
    }

    /// Código de idioma primario en minúsculas ("es-ES" → "es").
    pub fn lang_code(&self) -> String {
        lang_code(&self.language)
    }
}

pub(crate) fn lang_code(language: &str) -> String {
    language.split(['-', '_']).next().unwrap_or("es").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn defaults_without_env() {
        let c = AiConfig::from_lookup(env(&[]));
        assert_eq!(c.model, DEFAULT_MODEL);
        assert_eq!(c.base_url, "http://127.0.0.1:11434");
        assert_eq!(c.language, "es");
    }

    #[test]
    fn reads_and_clamps_values() {
        let c = AiConfig::from_lookup(env(&[
            ("LLM_BASE_URL", " http://gpu-box:11434/ "),
            ("LLM_MODEL", "qwen3:8b"),
            ("LLM_NUM_CTX", "16384"),
            ("LLM_TEMPERATURE", "9"),
            ("LLM_TIMEOUT_SECS", "90"),
            ("LLM_LANGUAGE", "en-US"),
            ("LLM_THINK", "true"),
        ]));
        assert_eq!(c.base_url, "http://gpu-box:11434");
        assert_eq!(c.model, "qwen3:8b");
        assert_eq!(c.num_ctx, 16384);
        assert_eq!(c.temperature, 2.0);
        assert_eq!(c.timeout, Duration::from_secs(90));
        assert_eq!(c.lang_code(), "en");
        assert!(c.think);
    }

    #[test]
    fn ignores_garbage() {
        let c = AiConfig::from_lookup(env(&[("LLM_NUM_CTX", "abc"), ("LLM_MODEL", "  ")]));
        assert_eq!(c.num_ctx, AiConfig::default().num_ctx);
        assert_eq!(c.model, DEFAULT_MODEL);
    }
}
