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
    /// Peticiones simultáneas que se envían a Ollama en las etapas independientes del análisis.
    /// Debe coincidir con `OLLAMA_NUM_PARALLEL` del servidor de Ollama (1 por defecto: si es
    /// mayor de lo que Ollama admite, las peticiones esperan en su cola y se alarga la latencia).
    pub parallel: usize,
    /// Directorio de modelos de Ollama (solo para medir el disco libre). `None` = autodetectar.
    pub models_dir: Option<std::path::PathBuf>,
    /// Tamaño relativo de la caché KV frente a f16, según el `OLLAMA_KV_CACHE_TYPE` con el que corre
    /// Ollama (`LLM_KV_CACHE_TYPE`): f16 = 1, q8_0 = 34/64 ≈ 0,53, q4_0 = 18/64 ≈ 0,28 (bloques de
    /// 32 valores de llama.cpp). Solo afecta a los requisitos de VRAM que muestra el catálogo.
    pub kv_cache_factor: f64,
    /// Capas del modelo que se piden en GPU (`options.num_gpu` de Ollama) para TODOS los modelos
    /// (`LLM_NUM_GPU`). `None` = lo decide Ollama (comportamiento por defecto). `Some(999)` = todas.
    /// Un ajuste por modelo en la app (`ModelManager::set_force_gpu`) tiene prioridad sobre este valor.
    pub num_gpu: Option<i32>,
}

/// Capas que se piden con «forzar todas las capas en GPU»: más que las de cualquier modelo del catálogo
/// (Ollama recorta al número real de capas).
pub const ALL_GPU_LAYERS: i32 = 999;

/// `LLM_NUM_GPU`: vacío/`auto` = Ollama decide (`None`); `all`/`max`/`gpu` = todas las capas; un entero
/// 0..=999 = ese número de capas (0 = solo CPU). Cualquier otra cosa se ignora.
pub fn parse_num_gpu(v: &str) -> Option<i32> {
    match v.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "default" => None,
        "all" | "max" | "gpu" | "todas" => Some(ALL_GPU_LAYERS),
        n => n.parse::<i32>().ok().filter(|n| (0..=ALL_GPU_LAYERS).contains(n)),
    }
}

/// Factor de tamaño de la caché KV para un `OLLAMA_KV_CACHE_TYPE`; desconocido = f16.
pub fn kv_cache_factor(kind: &str) -> f64 {
    match kind.trim().to_ascii_lowercase().as_str() {
        "q8_0" => 34.0 / 64.0,
        "q4_0" => 18.0 / 64.0,
        _ => 1.0,
    }
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
            parallel: 1,
            models_dir: None,
            kv_cache_factor: 1.0,
            num_gpu: None,
        }
    }
}

impl AiConfig {
    /// Lee `LLM_BASE_URL`, `LLM_MODEL`, `LLM_NUM_CTX`, `LLM_TEMPERATURE`, `LLM_TIMEOUT_SECS`,
    /// `LLM_LANGUAGE` (y, opcionales, `LLM_KEEP_ALIVE`, `LLM_MAX_OUTPUT_TOKENS`, `LLM_THINK`, `LLM_PARALLEL`, `OLLAMA_MODELS`).
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
        if let Some(v) = text("LLM_KV_CACHE_TYPE") {
            c.kv_cache_factor = kv_cache_factor(&v);
        }
        if let Some(v) = text("LLM_NUM_GPU") {
            c.num_gpu = parse_num_gpu(&v);
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
            c.think = matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "si" | "sí"
            );
        }
        if let Some(v) = text("LLM_PARALLEL").and_then(|v| v.parse::<usize>().ok()) {
            c.parallel = v.clamp(1, 8);
        }
        if let Some(v) = text("OLLAMA_MODELS") {
            c.models_dir = Some(v.into());
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

    #[test]
    fn num_gpu_is_opt_in_and_validated() {
        assert_eq!(AiConfig::default().num_gpu, None);
        for (v, want) in [("all", Some(999)), ("ALL", Some(999)), ("999", Some(999)), ("33", Some(33)), ("0", Some(0)), ("auto", None), ("", None), ("-1", None), ("1000", None), ("muchas", None)] {
            let c = AiConfig::from_lookup(env(&[("LLM_NUM_GPU", v)]));
            assert_eq!(c.num_gpu, want, "LLM_NUM_GPU={v:?}");
        }
    }

    #[test]
    fn kv_cache_type_sets_the_factor() {
        assert_eq!(AiConfig::default().kv_cache_factor, 1.0);
        let q8 = AiConfig::from_lookup(env(&[("LLM_KV_CACHE_TYPE", "Q8_0")]));
        assert!((q8.kv_cache_factor - 0.53125).abs() < 1e-9);
        let unknown = AiConfig::from_lookup(env(&[("LLM_KV_CACHE_TYPE", "nope")]));
        assert_eq!(unknown.kv_cache_factor, 1.0);
    }
}
