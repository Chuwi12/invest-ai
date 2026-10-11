//! Chat por SSE: `POST /conversations/{cid}/messages`.
//!
//! Un SSE nunca habla directamente con el analista. Una tarea aparte consume el stream del LLM,
//! acumula la respuesta, la guarda en BD y reenvía los eventos por un canal; el SSE solo lee del
//! canal. Así el mensaje del asistente se guarda aunque el cliente cierre la pestaña a medias.

use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Path, State},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use domain::{AssetId, ChatEvent, ChatMessage, ChatRequest, Role, ToolCallRecord};
use futures::{Stream, StreamExt, stream::BoxStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    db::{new_id, now_ms},
    error::{ApiError, ApiResult},
    extract::AppJson,
    models::SendMessage,
    repo::{
        self,
        conversations::{self as convs, NewMessage, auto_title},
    },
    routes::conversations::{MAX_CONVERSATION_ASSETS, dedupe_assets, not_found},
    state::AppState,
};

const MAX_CONTENT_CHARS: usize = 8_000;
/// Mensajes anteriores que se pasan al analista (el resto no cabría en el contexto del LLM).
const HISTORY_LIMIT: i64 = 40;
/// Activos que se precargan en el contexto del modelo.
const MAX_FOCUS: usize = 10;
/// Si el analista no emite nada durante este tiempo se da por colgado.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Si el cliente no consume el SSE durante este tiempo se le da por desconectado.
const CLIENT_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const CHANNEL_CAPACITY: usize = 256;

pub async fn send_message(
    State(state): State<AppState>,
    Path(cid): Path<String>,
    AppJson(body): AppJson<SendMessage>,
) -> ApiResult<Response> {
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return Err(ApiError::invalid("content no puede estar vacío"));
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(ApiError::invalid(format!("content no puede superar {MAX_CONTENT_CHARS} caracteres")));
    }
    let body_assets = dedupe_assets(body.asset_ids)?;

    // Los errores de aquí en adelante hasta abrir el stream son HTTP normales (404, 400...).
    let conversation = convs::get(&state.db, &cid).await?.ok_or_else(|| not_found(&cid))?;

    // Foco = activos del mensaje ∪ los de la conversación. Los del mensaje pasan a formar parte de
    // la conversación para que las respuestas siguientes no pierdan el contexto.
    let mut conversation_assets = conversation.asset_ids.clone();
    for id in &body_assets {
        if !conversation_assets.contains(id) {
            conversation_assets.push(id.clone());
        }
    }
    if conversation_assets.len() > MAX_CONVERSATION_ASSETS {
        // Se conservan los más recientes.
        let excess = conversation_assets.len() - MAX_CONVERSATION_ASSETS;
        conversation_assets.drain(..excess);
    }
    if conversation_assets != conversation.asset_ids {
        convs::set_asset_ids(&state.db, &cid, &conversation_assets).await?;
    }
    let focus = focus_assets(&body_assets, &conversation.asset_ids);

    // Historial *antes* de guardar el mensaje nuevo (el contrato del analista lo pide sin él).
    let history: Vec<ChatMessage> = convs::recent_messages(&state.db, &cid, HISTORY_LIMIT).await?;

    let user_created_ms = now_ms();
    let user_id = new_id();
    convs::insert_message(
        &state.db,
        &NewMessage {
            id: &user_id,
            conversation_id: &cid,
            role: Role::User,
            content: &content,
            tool_calls: &[],
            analysis: None,
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            created_at_ms: user_created_ms,
        },
    )
    .await?;
    // Atómico y solo si sigue sin título: el primer mensaje gana.
    convs::set_title_if_empty(&state.db, &cid, &auto_title(&content)).await?;

    let events = state.analyst.chat(ChatRequest {
        conversation_id: cid.clone(),
        history,
        user_message: content,
        focus,
        market: state.market.clone(),
        language: state.settings.language.clone(),
    });

    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let job = ChatJob {
        state: state.clone(),
        conversation_id: cid,
        assistant_id: new_id(),
        // El mensaje del asistente se ordena justo detrás del del usuario.
        created_at_ms: now_ms().max(user_created_ms + 1),
    };
    state.tasks.spawn(job.run(events, tx));

    let stream = ReceiverStream::new(rx).map(|ev| Ok::<_, Infallible>(to_sse(&ev)));
    Ok(sse_response(stream))
}

fn focus_assets(body: &[AssetId], conversation: &[AssetId]) -> Vec<AssetId> {
    let mut focus: Vec<AssetId> = Vec::new();
    for id in body.iter().chain(conversation) {
        if !focus.contains(id) {
            focus.push(id.clone());
        }
    }
    focus.truncate(MAX_FOCUS);
    focus
}

fn to_sse(ev: &ChatEvent) -> Event {
    Event::default().json_data(ev).unwrap_or_else(|e| {
        tracing::error!(error = %e, "no se pudo serializar un ChatEvent");
        Event::default().data(r#"{"type":"error","message":"error interno de serialización"}"#)
    })
}

fn sse_response(stream: impl Stream<Item = Result<Event, Infallible>> + Send + 'static) -> Response {
    let sse = Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"));
    // Evita que un proxy inverso (nginx) acumule el stream en lugar de reenviarlo.
    ([("x-accel-buffering", "no")], sse).into_response()
}

// ---------------------------------------------------------------------------------------------
// Tarea de fondo
// ---------------------------------------------------------------------------------------------

struct ChatJob {
    state: AppState,
    conversation_id: String,
    assistant_id: String,
    created_at_ms: i64,
}

/// Lo que se va acumulando de la respuesta para guardarla al final.
#[derive(Default)]
struct Accumulated {
    content: String,
    tool_calls: Vec<ToolCallRecord>,
    analysis: Option<domain::Analysis>,
    model: Option<String>,
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

impl Accumulated {
    fn has_output(&self) -> bool {
        !self.content.trim().is_empty() || self.analysis.is_some() || !self.tool_calls.is_empty()
    }

    fn tool_call_started(&mut self, id: String, name: String, arguments: serde_json::Value) {
        self.tool_calls.push(ToolCallRecord { id, name, arguments, result_summary: None, ok: None });
    }

    fn tool_call_finished(&mut self, id: &str, name: &str, ok: bool, summary: String) {
        match self.tool_calls.iter_mut().find(|r| r.id == id && r.ok.is_none()) {
            Some(rec) => {
                rec.ok = Some(ok);
                rec.result_summary = Some(summary);
            }
            // Resultado sin llamada previa: se conserva igualmente.
            None => self.tool_calls.push(ToolCallRecord {
                id: id.to_string(),
                name: name.to_string(),
                arguments: serde_json::Value::Null,
                result_summary: Some(summary),
                ok: Some(ok),
            }),
        }
    }
}

/// Canal hacia el SSE que tolera que el cliente se haya ido.
struct ClientSink {
    tx: mpsc::Sender<ChatEvent>,
    open: bool,
}

impl ClientSink {
    async fn send(&mut self, ev: ChatEvent) {
        if !self.open {
            return;
        }
        match tokio::time::timeout(CLIENT_SEND_TIMEOUT, self.tx.send(ev)).await {
            Ok(Ok(())) => {}
            // Receptor soltado o cliente atascado: se sigue consumiendo al LLM solo para guardar.
            _ => {
                tracing::debug!("cliente SSE desconectado: la respuesta se guardará igualmente");
                self.open = false;
            }
        }
    }
}

impl ChatJob {
    async fn run(self, mut events: BoxStream<'static, ChatEvent>, tx: mpsc::Sender<ChatEvent>) {
        let mut sink = ClientSink { tx, open: true };
        let mut acc = Accumulated::default();
        // El id del mensaje lo pone el servidor (no depende de lo que genere el analista) y se
        // anuncia ya: el cliente puede pintar la burbuja antes del primer token.
        sink.send(ChatEvent::Start { message_id: self.assistant_id.clone() }).await;

        let mut terminal: Option<ChatEvent> = None;
        loop {
            let next = tokio::select! {
                biased;
                () = self.state.shutdown.cancelled() => {
                    terminal = Some(ChatEvent::Error { message: "el servidor se está deteniendo".into() });
                    break;
                }
                next = tokio::time::timeout(IDLE_TIMEOUT, events.next()) => next,
            };
            let Ok(next) = next else {
                terminal = Some(ChatEvent::Error { message: "el modelo no responde (tiempo de espera agotado)".into() });
                break;
            };
            let Some(event) = next else { break };
            match event {
                ChatEvent::Start { .. } => {}
                ChatEvent::Thinking { .. } => sink.send(event).await,
                ChatEvent::Token { ref text } => {
                    acc.content.push_str(text);
                    sink.send(event).await;
                }
                ChatEvent::ToolCall { ref id, ref name, ref arguments } => {
                    acc.tool_call_started(id.clone(), name.clone(), arguments.clone());
                    sink.send(event).await;
                }
                ChatEvent::ToolResult { ref id, ref name, ok, ref summary } => {
                    acc.tool_call_finished(id, name, ok, summary.clone());
                    sink.send(event).await;
                }
                ChatEvent::Analysis { ref analysis } => {
                    self.save_analysis(analysis).await;
                    acc.analysis = Some(analysis.clone());
                    sink.send(event).await;
                }
                ChatEvent::Done { model, prompt_tokens, completion_tokens, .. } => {
                    acc.model = Some(model.clone());
                    acc.prompt_tokens = prompt_tokens;
                    acc.completion_tokens = completion_tokens;
                    terminal = Some(ChatEvent::Done {
                        message_id: self.assistant_id.clone(),
                        model,
                        prompt_tokens,
                        completion_tokens,
                    });
                    break;
                }
                ChatEvent::Error { message } => {
                    terminal = Some(ChatEvent::Error { message });
                    break;
                }
            }
        }

        // Guardar *antes* de emitir el evento final: si el cliente recarga la conversación al
        // recibir `done`, el mensaje ya tiene que estar.
        let terminal = match (terminal, acc.has_output()) {
            // El stream se cerró sin `done` ni `error`: se trata como terminado si hay contenido.
            (None, true) => ChatEvent::Done {
                message_id: self.assistant_id.clone(),
                model: acc.model.clone().unwrap_or_default(),
                prompt_tokens: None,
                completion_tokens: None,
            },
            (None, false) => ChatEvent::Error { message: "el modelo no devolvió ninguna respuesta".into() },
            (Some(ChatEvent::Done { .. }), false) => {
                ChatEvent::Error { message: "el modelo devolvió una respuesta vacía".into() }
            }
            (Some(ev), _) => ev,
        };
        if acc.has_output() {
            self.save_message(&acc).await;
        }
        sink.send(terminal).await;
    }

    async fn save_analysis(&self, analysis: &domain::Analysis) {
        if let Err(e) = repo::analyses::insert(&self.state.db, analysis, "chat").await {
            tracing::error!(error = %e, "no se pudo guardar el análisis del chat");
        }
    }

    async fn save_message(&self, acc: &Accumulated) {
        let res = convs::insert_message(
            &self.state.db,
            &NewMessage {
                id: &self.assistant_id,
                conversation_id: &self.conversation_id,
                role: Role::Assistant,
                content: &acc.content,
                tool_calls: &acc.tool_calls,
                analysis: acc.analysis.as_ref(),
                model: acc.model.as_deref(),
                prompt_tokens: acc.prompt_tokens,
                completion_tokens: acc.completion_tokens,
                created_at_ms: self.created_at_ms,
            },
        )
        .await;
        if let Err(e) = res {
            // Típico: la conversación se borró mientras se generaba la respuesta.
            tracing::warn!(conversation = %self.conversation_id, error = %e, "no se pudo guardar la respuesta del asistente");
        }
    }
}
