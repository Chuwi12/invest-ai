//! WebSocket `/ws`: un socket por pestaña, multiplexado por suscripciones.
//!
//! Por conexión se mantiene el conjunto de activos suscritos; cada vez que cambia se vuelve a
//! llamar a `market.subscribe(ids)` y se sustituye el stream anterior (soltarlo lo cancela).
//! Los ticks se reenvían con un throttle por activo: el primero sale al instante y, si llegan
//! más dentro de la ventana, solo se envía el último cuando ésta termina.

use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use domain::{AssetId, PriceTick};
use futures::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::{error::ApiError, state::AppState};

/// Ventana de throttle por activo (≈ 4 ticks por segundo).
const THROTTLE: Duration = Duration::from_millis(250);
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Sin recibir nada del cliente (ni siquiera pongs) durante este tiempo se cierra.
const IDLE_LIMIT: Duration = Duration::from_secs(90);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SUBSCRIPTIONS: usize = 100;
const MAX_INCOMING_BYTES: usize = 16 * 1024;
/// Espera antes de volver a suscribirse si el stream del mercado termina solo.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Subscribe { ids: Vec<String> },
    Unsubscribe { ids: Vec<String> },
    Ping,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    Tick { tick: &'a PriceTick },
    Pong,
    Error { message: String },
}

pub async fn handler(State(state): State<AppState>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !origin_allowed(&headers, &state.settings.allowed_origins) {
        return ApiError::Http {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: "origen no permitido".into(),
        }
        .into_response();
    }
    ws.max_message_size(MAX_INCOMING_BYTES).on_upgrade(move |socket| connection(socket, state))
}

/// Los navegadores no aplican CORS a los WebSockets: sin esta comprobación cualquier web abierta
/// en la misma máquina podría leer el socket. Sin cabecera `Origin` (clientes que no son
/// navegadores) se permite.
fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let origin = origin.trim_end_matches('/');
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(origin)) {
        return true;
    }
    let Some((_, authority)) = origin.split_once("://") else { return false };
    // Mismo origen que el propio servidor (despliegue de un único binario).
    if headers.get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(|h| h.eq_ignore_ascii_case(authority)) {
        return true;
    }
    // Cualquier puerto de loopback (Vite, previews...).
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or_default(),
        |rest| rest.split(']').next().unwrap_or_default(),
    );
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

struct Throttle {
    last_sent: Instant,
    /// El último tick recibido dentro de la ventana, pendiente de enviar.
    pending: Option<PriceTick>,
}

struct Connection {
    socket: WebSocket,
    state: AppState,
    subs: BTreeSet<AssetId>,
    ticks: Option<BoxStream<'static, PriceTick>>,
    throttle: HashMap<AssetId, Throttle>,
    /// Cuándo reintentar la suscripción si el stream del mercado terminó.
    resubscribe_at: Option<Instant>,
}

type SendResult = Result<(), ()>;

async fn connection(socket: WebSocket, state: AppState) {
    let mut conn = Connection {
        socket,
        state,
        subs: BTreeSet::new(),
        ticks: None,
        throttle: HashMap::new(),
        resubscribe_at: None,
    };
    conn.run().await;
}

impl Connection {
    async fn run(&mut self) {
        let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        let mut last_seen = Instant::now();
        let shutdown = self.state.shutdown.clone();

        loop {
            let next_flush = self
                .throttle
                .values()
                .filter(|t| t.pending.is_some())
                .map(|t| t.last_sent + THROTTLE)
                .min();

            tokio::select! {
                () = shutdown.cancelled() => {
                    self.close(1001, "servidor deteniéndose").await;
                    return;
                }
                incoming = self.socket.recv() => {
                    last_seen = Instant::now();
                    match incoming {
                        None | Some(Err(_)) => return,
                        Some(Ok(Message::Close(_))) => {
                            // Responde al cierre del cliente y termina limpiamente.
                            let _ = self.socket.send(Message::Close(None)).await;
                            return;
                        }
                        Some(Ok(Message::Text(text))) => {
                            if self.on_text(text.as_str()).await.is_err() { return; }
                        }
                        Some(Ok(Message::Binary(_))) => {
                            if self.send_error("los mensajes binarios no están soportados").await.is_err() { return; }
                        }
                        // Los ping/pong de protocolo los gestiona la librería.
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    }
                }
                tick = next_tick(&mut self.ticks) => match tick {
                    Some(tick) => {
                        if self.on_tick(tick).await.is_err() { return; }
                    }
                    None => {
                        // El proveedor cerró el stream: se reintenta pasado un rato.
                        self.ticks = None;
                        if !self.subs.is_empty() {
                            self.resubscribe_at = Some(Instant::now() + RESUBSCRIBE_DELAY);
                        }
                    }
                },
                () = sleep_until_opt(next_flush) => {
                    if self.flush_pending().await.is_err() { return; }
                }
                () = sleep_until_opt(self.resubscribe_at) => {
                    self.resubscribe_at = None;
                    self.resubscribe();
                }
                _ = heartbeat.tick() => {
                    if last_seen.elapsed() > IDLE_LIMIT {
                        self.close(1001, "sin actividad").await;
                        return;
                    }
                    if self.send_raw(Message::Ping(Vec::new().into())).await.is_err() { return; }
                }
            }
        }
    }

    async fn on_text(&mut self, text: &str) -> SendResult {
        let msg: ClientMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => return self.send_error(&format!("mensaje no válido: {e}")).await,
        };
        match msg {
            ClientMessage::Ping => self.send_json(&ServerMessage::Pong).await,
            ClientMessage::Subscribe { ids } => {
                let (ids, invalid) = parse_ids(&ids);
                let new: Vec<AssetId> = ids.into_iter().filter(|id| !self.subs.contains(id)).collect();
                if self.subs.len() + new.len() > MAX_SUBSCRIPTIONS {
                    return self
                        .send_error(&format!("demasiadas suscripciones (máximo {MAX_SUBSCRIPTIONS} por conexión)"))
                        .await;
                }
                if !new.is_empty() {
                    self.subs.extend(new);
                    self.resubscribe();
                }
                if invalid.is_empty() {
                    Ok(())
                } else {
                    self.send_error(&format!("ids no válidos: {}", invalid.join(", "))).await
                }
            }
            ClientMessage::Unsubscribe { ids } => {
                let (ids, invalid) = parse_ids(&ids);
                let mut changed = false;
                for id in ids {
                    changed |= self.subs.remove(&id);
                    self.throttle.remove(&id);
                }
                if changed {
                    self.resubscribe();
                }
                if invalid.is_empty() {
                    Ok(())
                } else {
                    self.send_error(&format!("ids no válidos: {}", invalid.join(", "))).await
                }
            }
        }
    }

    /// Sustituye el stream por uno nuevo con los activos actuales (soltar el viejo lo cancela).
    fn resubscribe(&mut self) {
        self.ticks = if self.subs.is_empty() {
            None
        } else {
            let ids: Vec<AssetId> = self.subs.iter().cloned().collect();
            Some(self.state.market.subscribe(&ids))
        };
    }

    async fn on_tick(&mut self, tick: PriceTick) -> SendResult {
        // Puede llegar algún tick de un activo recién dado de baja mientras se cambiaba el stream.
        if !self.subs.contains(&tick.id) {
            return Ok(());
        }
        let now = Instant::now();
        match self.throttle.get_mut(&tick.id) {
            Some(t) if now.duration_since(t.last_sent) < THROTTLE => {
                // Dentro de la ventana: solo cuenta el más reciente.
                t.pending = Some(tick);
                Ok(())
            }
            slot => {
                match slot {
                    Some(t) => {
                        t.last_sent = now;
                        t.pending = None;
                    }
                    None => {
                        self.throttle.insert(tick.id.clone(), Throttle { last_sent: now, pending: None });
                    }
                }
                self.send_json(&ServerMessage::Tick { tick: &tick }).await
            }
        }
    }

    async fn flush_pending(&mut self) -> SendResult {
        let now = Instant::now();
        let due: Vec<PriceTick> = self
            .throttle
            .values_mut()
            .filter(|t| t.pending.is_some() && now >= t.last_sent + THROTTLE)
            .filter_map(|t| {
                t.last_sent = now;
                t.pending.take()
            })
            .collect();
        for tick in due {
            self.send_json(&ServerMessage::Tick { tick: &tick }).await?;
        }
        Ok(())
    }

    async fn send_error(&mut self, message: &str) -> SendResult {
        self.send_json(&ServerMessage::Error { message: message.to_string() }).await
    }

    async fn send_json(&mut self, msg: &ServerMessage<'_>) -> SendResult {
        let Ok(text) = serde_json::to_string(msg) else { return Ok(()) };
        self.send_raw(Message::Text(text.into())).await
    }

    /// Un cliente que no lee no puede bloquear la conexión indefinidamente.
    async fn send_raw(&mut self, msg: Message) -> SendResult {
        match tokio::time::timeout(SEND_TIMEOUT, self.socket.send(msg)).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(()),
        }
    }

    async fn close(&mut self, code: u16, reason: &'static str) {
        let frame = CloseFrame { code, reason: reason.into() };
        let _ = self.send_raw(Message::Close(Some(frame))).await;
    }
}

fn parse_ids(raw: &[String]) -> (Vec<AssetId>, Vec<String>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for s in raw {
        match s.parse::<AssetId>() {
            Ok(id) if !ok.contains(&id) => ok.push(id),
            Ok(_) => {}
            Err(_) => bad.push(s.clone()),
        }
    }
    (ok, bad)
}

/// Siguiente tick, o espera eternamente si no hay suscripciones.
async fn next_tick(ticks: &mut Option<BoxStream<'static, PriceTick>>) -> Option<PriceTick> {
    match ticks {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
        }
        h
    }

    #[test]
    fn origin_rules() {
        let allowed = vec!["http://localhost:5180".to_string()];
        let h = |o| headers(o, "127.0.0.1:8787");
        assert!(origin_allowed(&h(None), &allowed));
        assert!(origin_allowed(&h(Some("http://localhost:5180")), &allowed));
        assert!(origin_allowed(&h(Some("http://127.0.0.1:5180")), &allowed));
        assert!(origin_allowed(&h(Some("http://[::1]:3000")), &allowed));
        assert!(origin_allowed(&headers(Some("https://app.example.com"), "app.example.com"), &allowed));
        assert!(!origin_allowed(&h(Some("https://evil.example")), &allowed));
        assert!(!origin_allowed(&h(Some("null")), &allowed));
        assert!(!origin_allowed(&h(Some("http://localhost.evil.example")), &allowed));
    }

    #[test]
    fn parse_ids_splits_valid_and_invalid() {
        let (ok, bad) = parse_ids(&["crypto:BTC".into(), "crypto:BTC".into(), "BTC".into(), "stock:AAPL".into()]);
        assert_eq!(ok.len(), 2);
        assert_eq!(bad, vec!["BTC".to_string()]);
    }
}
