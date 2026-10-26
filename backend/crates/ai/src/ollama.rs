//! Cliente HTTP de Ollama: `/api/chat` (streaming NDJSON, tools, salida estructurada, thinking),
//! `/api/tags`, `/api/show`, `/api/ps`, `/api/pull` y `/api/delete`.
//!
//! Cancelación: todos los streams se construyen con `async_stream`; al soltar el stream se suelta
//! la respuesta de `reqwest`, se cierra la conexión TCP y Ollama aborta la generación (o la
//! descarga). No hace falta ningún token aparte.
//!
//! Timeouts: la conexión tiene 5 s; las llamadas cortas (`tags`, `show`, `ps`, `delete`) 10 s en
//! total; las largas (chat, pull) solo tienen «tiempo máximo sin recibir bytes» (`read_timeout`),
//! porque una respuesta de varios minutos es normal pero un servidor mudo no.
//!
//! Tamaños: Ollama no comprime, así que el cliente no descomprime nunca (sin bombas gzip/brotli) y todo
//! cuerpo se lee por trozos con tope: `MAX_BODY_BYTES` en las respuestas no streaming y, en los streams NDJSON,
//! `MAX_LINE_BYTES` por línea y `MAX_CHAT_STREAM_BYTES` en total (el pull no lo lleva: no acumula nada).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use domain::{DomainError, PullEvent};
use futures::{Stream, StreamExt, stream::BoxStream};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::think::{Piece, ThinkSplitter};

const SHORT_TIMEOUT: Duration = Duration::from_secs(10);
/// Una línea NDJSON de Ollama nunca debería acercarse a esto; protege la memoria.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Tope de un cuerpo no streaming (`tags`, `show`, `ps` y mensajes de error). Lo real son KB (`show` sin `verbose`, con
/// licencia y plantilla, ronda 1 MB); aunque incluyera el vocabulario del tokenizador (varios MB) cabría de sobra.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Tope total de un chat en streaming. Cada token viaja en una línea de ~150 B; con el máximo de `num_ctx` (262 144 tokens,
/// `config.rs`) una generación entera pesa unos 40 MB. 128 MiB deja más de 3× de margen y acota lo que `chat()` agrega.
const MAX_CHAT_STREAM_BYTES: usize = 128 * 1024 * 1024;

/// Cuánto dura la desconfianza hacia «forzar GPU» de un modelo tras un fallo de memoria: durante este
/// tiempo se pide el reparto automático sin volver a intentarlo (cada intento fallido cuesta una carga).
pub const GPU_FALLBACK_TTL: Duration = Duration::from_secs(30 * 60);

/// Un intento de cargar un modelo con `num_gpu` forzado que falló por falta de memoria.
#[derive(Debug, Clone)]
struct GpuFallback {
    at: Instant,
    /// Mensaje de Ollama (acortado).
    reason: String,
    /// ¿Ya se avisó en algún análisis?
    notified: bool,
}

#[derive(Clone)]
pub struct OllamaClient {
    http: Client,
    base: String,
    /// Modelos cuyo `num_gpu` forzado falló por memoria (compartido entre clones del cliente).
    gpu_fallbacks: Arc<Mutex<HashMap<String, GpuFallback>>>,
}

// ---------------------------------------------------------------------------------------------
// Tipos de la petición
// ---------------------------------------------------------------------------------------------

/// Mensaje en el formato de Ollama.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Nombre de la herramienta cuando `role == "tool"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl WireMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), ..Self::default() }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }
    pub fn tool(name: &str, content: impl Into<String>) -> Self {
        Self { tool_name: Some(name.into()), ..Self::new("tool", content) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Parámetros de una petición de chat. `think`: `Some(Bool)` o `Some("low"|"medium"|"high")`;
/// solo debe enviarse a modelos con la capacidad «thinking» (si no, Ollama responde 400).
#[derive(Debug, Clone, Default)]
pub struct ChatParams {
    pub model: String,
    pub messages: Vec<WireMessage>,
    /// Definiciones de herramientas (formato OpenAI `{"type":"function","function":{...}}`).
    pub tools: Vec<Value>,
    /// JSON Schema de la salida (salida estructurada).
    pub format: Option<Value>,
    pub think: Option<Value>,
    pub num_ctx: Option<u32>,
    pub temperature: Option<f32>,
    pub num_predict: Option<u32>,
    pub keep_alive: Option<String>,
    pub seed: Option<i64>,
    /// Capas en GPU (`options.num_gpu`). `None` = lo decide Ollama. Si se fuerza y la carga falla por
    /// memoria, `OllamaClient::chat_stream` reintenta sin forzar (ver `gpu_fallback`).
    pub num_gpu: Option<i32>,
}

impl ChatParams {
    fn body(&self) -> Value {
        let mut options = serde_json::Map::new();
        if let Some(v) = self.num_ctx {
            options.insert("num_ctx".into(), json!(v));
        }
        if let Some(v) = self.temperature {
            options.insert("temperature".into(), json!(v));
        }
        if let Some(v) = self.num_predict {
            options.insert("num_predict".into(), json!(v));
        }
        if let Some(v) = self.seed {
            options.insert("seed".into(), json!(v));
        }
        if let Some(v) = self.num_gpu {
            options.insert("num_gpu".into(), json!(v));
        }
        let mut body = json!({
            "model": self.model,
            "messages": self.messages,
            "stream": true,
            "options": options,
        });
        let obj = body.as_object_mut().expect("objeto");
        if !self.tools.is_empty() {
            obj.insert("tools".into(), json!(self.tools));
        }
        if let Some(f) = &self.format {
            obj.insert("format".into(), f.clone());
        }
        if let Some(t) = &self.think {
            obj.insert("think".into(), t.clone());
        }
        if let Some(k) = &self.keep_alive {
            obj.insert("keep_alive".into(), json!(k));
        }
        body
    }
}

// ---------------------------------------------------------------------------------------------
// Salida del chat
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatChunk {
    Thinking(String),
    Content(String),
    ToolCalls(Vec<ToolCall>),
    /// Último fragmento: uso y motivo de parada (`stop`, `length`, …).
    Done { usage: Usage, reason: Option<String> },
}

/// Respuesta completa ya agregada.
#[derive(Debug, Clone, Default)]
pub struct ChatOutput {
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub done_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    message: Option<WireMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
    #[serde(default)]
    error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Otros endpoints
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelDetails {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub parameter_size: String,
    #[serde(default)]
    pub quantization_level: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagModel {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub details: ModelDetails,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShowInfo {
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub details: ModelDetails,
    /// Metadatos del GGUF (`<arch>.block_count`, `<arch>.context_length`, …).
    #[serde(default)]
    pub model_info: serde_json::Map<String, Value>,
}

impl ShowInfo {
    pub fn has(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(cap))
    }

    /// Metadato numérico de la arquitectura, p. ej. `arch_u64("block_count")`.
    pub fn arch_u64(&self, key: &str) -> Option<u64> {
        let arch = self.model_info.get("general.architecture")?.as_str()?;
        self.model_info.get(&format!("{arch}.{key}"))?.as_u64()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunningModel {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub size_vram: u64,
}

impl OllamaClient {
    pub fn new(base_url: &str, read_timeout: Duration) -> Result<Self, DomainError> {
        let http = Client::builder()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(read_timeout)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| DomainError::Internal(format!("cliente HTTP de Ollama: {e}")))?;
        Ok(Self { http, base: base_url.trim_end_matches('/').to_string(), gpu_fallbacks: Arc::default() })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    // --- chat -----------------------------------------------------------------------------

    /// Stream de fragmentos. Las etiquetas `<think>…</think>` en `content` se separan en
    /// `Thinking`. Soltar el stream cancela la generación en Ollama.
    pub fn chat_stream(&self, params: ChatParams) -> BoxStream<'static, Result<ChatChunk, DomainError>> {
        if params.num_gpu.is_none() {
            return self.chat_stream_once(params);
        }
        // `num_gpu` forzado: si la carga falla por falta de memoria ANTES de recibir nada, se reintenta
        // sin forzar (el reparto automático de Ollama siempre cabe) y se recuerda para avisar.
        let this = self.clone();
        let stream = async_stream::try_stream! {
            let mut forced = this.chat_stream_once(params.clone());
            let mut yielded = false;
            let mut retry = false;
            while let Some(item) = forced.next().await {
                match item {
                    Ok(chunk) => {
                        yielded = true;
                        yield chunk;
                    }
                    Err(e) if !yielded && is_gpu_memory_error(&e) => {
                        this.note_gpu_fallback(&params.model, &e);
                        retry = true;
                        break;
                    }
                    Err(e) => Err(e)?,
                }
            }
            if retry {
                let mut plain = this.chat_stream_once(ChatParams { num_gpu: None, ..params });
                while let Some(item) = plain.next().await {
                    yield item?;
                }
            }
        };
        Box::pin(stream)
    }

    fn note_gpu_fallback(&self, model: &str, e: &DomainError) {
        let reason: String = e.to_string().chars().take(300).collect();
        tracing::warn!(model, error = %reason, "num_gpu forzado sin memoria suficiente: se reintenta con el reparto automático");
        self.gpu_fallbacks.lock().unwrap().insert(model.to_string(), GpuFallback { at: Instant::now(), reason, notified: false });
    }

    /// Motivo del último fallo de memoria al forzar GPU en este modelo, si fue hace menos de `GPU_FALLBACK_TTL`.
    pub fn gpu_fallback(&self, model: &str) -> Option<String> {
        let map = self.gpu_fallbacks.lock().unwrap();
        map.get(model).filter(|f| f.at.elapsed() < GPU_FALLBACK_TTL).map(|f| f.reason.clone())
    }

    /// Como `gpu_fallback`, pero solo la primera vez tras cada fallo (para avisar una vez en un análisis).
    pub fn take_gpu_fallback_notice(&self, model: &str) -> Option<String> {
        let mut map = self.gpu_fallbacks.lock().unwrap();
        let f = map.get_mut(model)?;
        if f.at.elapsed() >= GPU_FALLBACK_TTL || f.notified {
            return None;
        }
        f.notified = true;
        Some(f.reason.clone())
    }

    /// Olvida el fallo (p. ej. cuando el usuario cambia el ajuste de GPU del modelo).
    pub fn clear_gpu_fallback(&self, model: &str) {
        self.gpu_fallbacks.lock().unwrap().remove(model);
    }

    fn chat_stream_once(&self, params: ChatParams) -> BoxStream<'static, Result<ChatChunk, DomainError>> {
        let this = self.clone();
        let stream = async_stream::try_stream! {
            let resp = this
                .http
                .post(this.url("/api/chat"))
                .json(&params.body())
                .send()
                .await
                .map_err(map_reqwest)?;
            let resp = check_status(resp, &params.model).await?;
            let mut lines = ndjson(resp.bytes_stream(), MAX_CHAT_STREAM_BYTES);
            let mut splitter = ThinkSplitter::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            while let Some(line) = lines.next().await {
                let line = line?;
                let chunk: WireChunk = serde_json::from_slice(&line)
                    .map_err(|e| DomainError::Upstream { provider: "ollama".into(), message: format!("línea no válida: {e}") })?;
                if let Some(err) = chunk.error {
                    Err(upstream(err))?;
                }
                if let Some(m) = chunk.message {
                    if let Some(t) = m.thinking.filter(|t| !t.is_empty()) {
                        yield ChatChunk::Thinking(t);
                    }
                    if !m.content.is_empty() {
                        for p in splitter.push(&m.content) {
                            yield match p {
                                Piece::Thinking(t) => ChatChunk::Thinking(t),
                                Piece::Text(t) => ChatChunk::Content(t),
                            };
                        }
                    }
                    tool_calls.extend(m.tool_calls);
                }
                if chunk.done {
                    for p in splitter.finish() {
                        yield match p {
                            Piece::Thinking(t) => ChatChunk::Thinking(t),
                            Piece::Text(t) => ChatChunk::Content(t),
                        };
                    }
                    if !tool_calls.is_empty() {
                        yield ChatChunk::ToolCalls(std::mem::take(&mut tool_calls));
                    }
                    yield ChatChunk::Done {
                        usage: Usage { prompt_tokens: chunk.prompt_eval_count, completion_tokens: chunk.eval_count },
                        reason: chunk.done_reason,
                    };
                    return;
                }
            }
            // El servidor cerró sin `done`: respuesta truncada.
            Err(upstream("la respuesta del modelo se cortó antes de terminar".into()))?;
        };
        Box::pin(stream)
    }

    /// Ejecuta un chat completo y agrega el resultado.
    pub async fn chat(&self, params: ChatParams) -> Result<ChatOutput, DomainError> {
        let mut out = ChatOutput::default();
        let mut s = self.chat_stream(params);
        while let Some(chunk) = s.next().await {
            match chunk? {
                ChatChunk::Thinking(t) => out.thinking.push_str(&t),
                ChatChunk::Content(t) => out.content.push_str(&t),
                ChatChunk::ToolCalls(c) => out.tool_calls.extend(c),
                ChatChunk::Done { usage, reason } => {
                    out.usage = usage;
                    out.done_reason = reason;
                }
            }
        }
        Ok(out)
    }

    // --- gestión de modelos ---------------------------------------------------------------

    /// Modelos instalados. Error de conexión => `Unavailable`.
    pub async fn tags(&self) -> Result<Vec<TagModel>, DomainError> {
        #[derive(Deserialize)]
        struct Tags {
            #[serde(default)]
            models: Vec<TagModel>,
        }
        let resp = self.http.get(self.url("/api/tags")).timeout(SHORT_TIMEOUT).send().await.map_err(map_reqwest)?;
        let resp = check_status(resp, "").await?;
        Ok(read_json::<Tags>(resp).await?.models)
    }

    pub async fn show(&self, model: &str) -> Result<ShowInfo, DomainError> {
        let resp = self
            .http
            .post(self.url("/api/show"))
            .timeout(SHORT_TIMEOUT)
            .json(&json!({ "model": model }))
            .send()
            .await
            .map_err(map_reqwest)?;
        let resp = check_status(resp, model).await?;
        read_json(resp).await
    }

    /// Modelos cargados en memoria ahora mismo.
    pub async fn ps(&self) -> Result<Vec<RunningModel>, DomainError> {
        #[derive(Deserialize)]
        struct Ps {
            #[serde(default)]
            models: Vec<RunningModel>,
        }
        let resp = self.http.get(self.url("/api/ps")).timeout(SHORT_TIMEOUT).send().await.map_err(map_reqwest)?;
        let resp = check_status(resp, "").await?;
        Ok(read_json::<Ps>(resp).await?.models)
    }

    pub async fn delete(&self, model: &str) -> Result<(), DomainError> {
        let resp = self
            .http
            .delete(self.url("/api/delete"))
            .timeout(SHORT_TIMEOUT)
            .json(&json!({ "model": model }))
            .send()
            .await
            .map_err(map_reqwest)?;
        check_status(resp, model).await?;
        Ok(())
    }

    /// Descarga con progreso. Termina con `Done` o `Error`. Soltar el stream aborta la descarga
    /// (Ollama conserva las capas parciales y reanuda en la siguiente).
    pub fn pull_stream(&self, model: &str) -> BoxStream<'static, PullEvent> {
        let this = self.clone();
        let model = model.to_string();
        let stream = async_stream::stream! {
            let sent = this
                .http
                .post(this.url("/api/pull"))
                .json(&json!({ "model": model, "stream": true }))
                .send()
                .await;
            let resp = match sent {
                Ok(r) => match check_status(r, &model).await {
                    Ok(r) => r,
                    Err(e) => { yield PullEvent::Error { message: e.to_string() }; return; }
                },
                Err(e) => { yield PullEvent::Error { message: map_reqwest(e).to_string() }; return; }
            };
            // Sin tope total: no acumula nada y una descarga larga emite ~17 líneas/s; solo importa el tope por línea.
            let mut lines = ndjson(resp.bytes_stream(), usize::MAX);
            while let Some(line) = lines.next().await {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => { yield PullEvent::Error { message: e.to_string() }; return; }
                };
                let v: Value = match serde_json::from_slice(&line) {
                    Ok(v) => v,
                    Err(e) => { yield PullEvent::Error { message: format!("línea no válida: {e}") }; return; }
                };
                if let Some(err) = v.get("error").and_then(Value::as_str) {
                    yield PullEvent::Error { message: err.to_string() };
                    return;
                }
                let status = v.get("status").and_then(Value::as_str).unwrap_or_default().to_string();
                if status == "success" {
                    yield PullEvent::Done;
                    return;
                }
                yield PullEvent::Progress {
                    status,
                    completed: v.get("completed").and_then(Value::as_u64),
                    total: v.get("total").and_then(Value::as_u64),
                };
            }
            yield PullEvent::Error { message: "la descarga se interrumpió antes de terminar".into() };
        };
        Box::pin(stream)
    }
}

// ---------------------------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------------------------

/// ¿Es un fallo de memoria de la GPU/del reparto al cargar el modelo? Ollama 0.40.x lo devuelve como HTTP 500
/// `{"error": "..."}` (o como línea `error` del stream) con textos de llama.cpp/CUDA que varían entre versiones:
/// «cudaMalloc failed: out of memory», «unable to allocate CUDA0 buffer», «model requires more system memory
/// (X GiB) than is available (Y GiB)», «memory layout cannot be allocated». No se pudo capturar uno real en el
/// equipo de desarrollo (no hubo ningún fallo), así que se reconocen por fragmentos y sin distinguir mayúsculas.
pub fn is_gpu_memory_error(e: &DomainError) -> bool {
    let DomainError::Upstream { message, .. } = e else { return false };
    let m = message.to_ascii_lowercase();
    [
        "out of memory",
        "cudamalloc",
        "cuda error",
        "unable to allocate",
        "failed to allocate",
        "not enough memory",
        "insufficient memory",
        "memory layout cannot be allocated",
        "requires more system memory",
        "than is available",
        "alloc_failed",
        "oom",
    ]
    .iter()
    .any(|p| if *p == "oom" { m.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "oom") } else { m.contains(p) })
}

fn upstream(message: String) -> DomainError {
    DomainError::Upstream { provider: "ollama".into(), message }
}

/// Errores de red => `Unavailable` (Ollama apagado o inalcanzable) o `Upstream` (timeout, etc.).
fn map_reqwest(e: reqwest::Error) -> DomainError {
    if e.is_connect() {
        return DomainError::Unavailable("no se pudo conectar con Ollama (¿está en marcha?)".into());
    }
    if e.is_timeout() {
        return upstream("Ollama no respondió a tiempo".into());
    }
    // `to_string` de reqwest puede incluir la URL: aquí es local y sin secretos.
    upstream(e.to_string())
}

/// Convierte respuestas no 2xx en errores con el mensaje de Ollama (`{"error": "..."}`).
async fn check_status(resp: reqwest::Response, model: &str) -> Result<reqwest::Response, DomainError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    // Un cuerpo ilegible o enorme va en el mensaje, pero manda el código HTTP (404 → NotFound, 400 → InvalidInput).
    let text = match read_limited(resp, MAX_BODY_BYTES).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => e.to_string(),
    };
    let msg = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| text.chars().take(300).collect());
    Err(match status {
        StatusCode::NOT_FOUND => DomainError::NotFound(if model.is_empty() { msg } else { format!("modelo «{model}»: {msg}") }),
        StatusCode::BAD_REQUEST => DomainError::InvalidInput(msg),
        s if s.is_server_error() => upstream(format!("{s}: {msg}")),
        s => upstream(format!("{s}: {msg}")),
    })
}

/// Lee el cuerpo por trozos y aborta en cuanto pasa de `max` bytes (mismo patrón que `market::http::read_limited`;
/// `market` no es dependencia de este crate). Sin descompresión, `max` cuenta lo que llega por el cable.
async fn read_limited(mut resp: reqwest::Response, max: usize) -> Result<Vec<u8>, DomainError> {
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(upstream("respuesta demasiado grande".into()));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(map_reqwest)? {
        if buf.len() + chunk.len() > max {
            return Err(upstream("respuesta demasiado grande".into()));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// JSON de una respuesta no streaming, con tope de tamaño.
async fn read_json<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, DomainError> {
    serde_json::from_slice(&read_limited(resp, MAX_BODY_BYTES).await?).map_err(|e| upstream(format!("respuesta no válida: {e}")))
}

/// Parte un stream de bytes en líneas NDJSON no vacías. Aborta si una línea pasa de `MAX_LINE_BYTES` o el stream
/// entero de `max_total`: un servidor roto u hostil no puede alargarlo sin fin.
fn ndjson(
    bytes: impl Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
    max_total: usize,
) -> BoxStream<'static, Result<Vec<u8>, DomainError>> {
    let s = async_stream::try_stream! {
        let mut bytes = Box::pin(bytes);
        let mut buf: Vec<u8> = Vec::new();
        let mut total = 0usize;
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(map_reqwest)?;
            total = total.saturating_add(chunk.len());
            if total > max_total {
                Err(upstream("respuesta demasiado grande".into()))?;
            }
            // Solo se recorre el trozo nuevo (una línea enorme en trozos pequeños no cuesta O(n²)) y el tope se
            // comprueba antes de copiar, así que `buf` nunca pasa de `MAX_LINE_BYTES`.
            for part in chunk.split_inclusive(|&b| b == b'\n') {
                if buf.len() + part.len() > MAX_LINE_BYTES {
                    Err(upstream("línea de respuesta demasiado grande".into()))?;
                }
                buf.extend_from_slice(part);
                if buf.ends_with(b"\n") {
                    let line = std::mem::take(&mut buf);
                    let trimmed = line.trim_ascii();
                    if !trimmed.is_empty() {
                        yield trimmed.to_vec();
                    }
                }
            }
        }
        let rest = buf.trim_ascii();
        if !rest.is_empty() {
            yield rest.to_vec();
        }
    };
    Box::pin(s)
}

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
