//! Tiempo real: una conexión WebSocket por proveedor (reconexión con backoff exponencial y
//! jitter) y un hub que reparte los ticks por `broadcast` con cuenta de referencias.
//!
//! - Cada activo con suscriptores tiene una entrada en el hub; al llegar el primer suscriptor se
//!   pide el símbolo al feed WS que lo cubra y se arranca un sondeo de respaldo que solo consulta
//!   cuando ese feed no está conectado (o si ningún feed cubre el activo).
//! - Los ticks de un mismo activo se limitan a ~4/s: el último valor nunca se pierde (se emite
//!   con retardo si llegó dentro de la ventana).
//! - Al soltar el stream devuelto por `subscribe` se decrementa el contador; en 0 se desuscribe
//!   del feed y se cancela el sondeo.

use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use domain::{AssetClass, AssetId, MarketState, PriceTick, Quote};
use futures::{
    Stream, StreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};
use tokio_util::sync::CancellationToken;

/// Separación mínima entre ticks emitidos de un mismo activo (~4 por segundo).
pub const MIN_TICK_GAP: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Sin ningún mensaje durante este tiempo se considera la conexión muerta y se reconecta.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_WS_MESSAGE: usize = 1 << 20;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Las altas/bajas que llegan juntas (varios activos suscritos a la vez) se agrupan en una sola
/// tanda antes de hablar con el proveedor.
const COALESCE: Duration = Duration::from_millis(40);

// ---------------------------------------------------------------------------------------------
// Feed WS genérico
// ---------------------------------------------------------------------------------------------

/// Tick tal como lo da el proveedor, con su símbolo propio (`BTCUSDT`, `AAPL`).
#[derive(Debug, Clone, PartialEq)]
pub struct RawTick {
    pub symbol: String,
    pub price: f64,
    /// Precio de referencia de 24 h si el feed lo da (para calcular el cambio).
    pub open24h: Option<f64>,
    pub volume: Option<f64>,
    /// Marca de tiempo del proveedor (o la local si el feed no la da).
    pub ts: DateTime<Utc>,
}

pub trait WsFeed: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    /// URL de conexión. Puede contener una clave: **nunca** se registra.
    fn url(&self) -> String;
    /// Mensajes de (des)suscripción para estos símbolos.
    fn subscribe_msgs(&self, symbols: &[String], on: bool) -> Vec<String>;
    /// Separación mínima entre mensajes de control enviados al proveedor. Binance admite 5
    /// mensajes por segundo y conexión y, al pasarse, ignora los siguientes sin avisar y acaba
    /// cerrando la conexión (medido: 10 `SUBSCRIBE` seguidos -> 5 acks, ningún tick, cierre).
    fn min_msg_gap(&self) -> Duration {
        Duration::ZERO
    }
    /// Ticks contenidos en un mensaje de texto (vacío si es un ack u otro tipo de mensaje).
    fn parse(&self, text: &str) -> Vec<RawTick>;
}

enum Cmd {
    Sub(String),
    Unsub(String),
}

/// Manejador de un worker WS en segundo plano.
pub struct WsHandle {
    name: &'static str,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    connected: Arc<AtomicBool>,
}

impl WsHandle {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// Arranca el worker. Es perezoso: no abre la conexión hasta que haya algún símbolo suscrito y
/// la cierra cuando se queda sin ninguno.
pub fn spawn_ws<F: WsFeed>(
    feed: F,
    ingest: mpsc::Sender<(usize, RawTick)>,
    feed_idx: usize,
    cancel: CancellationToken,
) -> WsHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let connected = Arc::new(AtomicBool::new(false));
    let handle = WsHandle {
        name: feed.name(),
        cmd_tx,
        connected: connected.clone(),
    };
    tokio::spawn(ws_worker(feed, cmd_rx, ingest, feed_idx, connected, cancel));
    handle
}

/// Espera con backoff exponencial y jitter (50–100 % del valor): evita que varias conexiones
/// caídas a la vez reconecten sincronizadas.
pub fn backoff(attempt: u32) -> Duration {
    let exp = BACKOFF_BASE
        .saturating_mul(1u32 << attempt.min(6))
        .min(BACKOFF_MAX);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let jitter = 0.5 + f64::from(nanos % 1000) / 2000.0;
    exp.mul_f64(jitter)
}

fn apply(desired: &mut HashSet<String>, cmd: Cmd) -> Option<(String, bool)> {
    match cmd {
        Cmd::Sub(s) => desired.insert(s.clone()).then_some((s, true)),
        Cmd::Unsub(s) => desired.remove(&s).then_some((s, false)),
    }
}

async fn ws_worker<F: WsFeed>(
    feed: F,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    ingest: mpsc::Sender<(usize, RawTick)>,
    idx: usize,
    connected: Arc<AtomicBool>,
    cancel: CancellationToken,
) {
    let name = feed.name();
    let mut desired: HashSet<String> = HashSet::new();
    let mut attempt: u32 = 0;
    let cfg = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE));
    loop {
        // Reposo hasta que haya algo que escuchar.
        while desired.is_empty() {
            tokio::select! {
                () = cancel.cancelled() => return,
                c = cmds.recv() => match c {
                    Some(c) => { apply(&mut desired, c); }
                    None => return,
                },
            }
        }

        let url = feed.url();
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async_with_config(url, Some(cfg), false),
        )
        .await;
        let mut ws = match conn {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                // El error de tungstenite puede incluir la URL de la petición: solo se registra su tipo.
                tracing::warn!(feed = name, error = %ws_error_kind(&e), "WS: no se pudo conectar");
                if !wait_backoff(&mut attempt, &mut desired, &mut cmds, &cancel).await {
                    return;
                }
                continue;
            }
            Err(_) => {
                tracing::warn!(feed = name, "WS: timeout al conectar");
                if !wait_backoff(&mut attempt, &mut desired, &mut cmds, &cancel).await {
                    return;
                }
                continue;
            }
        };

        // Suscribe todo lo que se quiere escuchar.
        let all: Vec<String> = desired.iter().cloned().collect();
        let mut last_control: Option<tokio::time::Instant> = None;
        let ok = send_paced(&mut ws, feed.subscribe_msgs(&all, true), feed.min_msg_gap(), &mut last_control).await;
        if !ok {
            if !wait_backoff(&mut attempt, &mut desired, &mut cmds, &cancel).await {
                return;
            }
            continue;
        }
        connected.store(true, Ordering::Relaxed);
        tracing::info!(feed = name, symbols = all.len(), "WS conectado");
        let mut got_message = false;

        let reason = loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    connected.store(false, Ordering::Relaxed);
                    let _ = ws.close(None).await;
                    return;
                }
                c = cmds.recv() => {
                    let Some(c) = c else { return };
                    // Una ráfaga de altas (un cliente que se suscribe a 10 activos) se agrupa en
                    // una tanda: un mensaje por activo desbordaría el límite del proveedor.
                    let before = desired.clone();
                    apply(&mut desired, c);
                    let settle = tokio::time::sleep(COALESCE);
                    tokio::pin!(settle);
                    loop {
                        tokio::select! {
                            () = cancel.cancelled() => {
                                connected.store(false, Ordering::Relaxed);
                                let _ = ws.close(None).await;
                                return;
                            }
                            () = &mut settle => break,
                            c = cmds.recv() => match c {
                                Some(c) => { apply(&mut desired, c); }
                                None => return,
                            },
                        }
                    }
                    let mut added: Vec<String> = desired.difference(&before).cloned().collect();
                    let mut removed: Vec<String> = before.difference(&desired).cloned().collect();
                    added.sort_unstable();
                    removed.sort_unstable();
                    let gap = feed.min_msg_gap();
                    let mut sent = true;
                    if !added.is_empty() {
                        sent = send_paced(&mut ws, feed.subscribe_msgs(&added, true), gap, &mut last_control).await;
                    }
                    if sent && !removed.is_empty() {
                        sent = send_paced(&mut ws, feed.subscribe_msgs(&removed, false), gap, &mut last_control).await;
                    }
                    if !sent { break "error al enviar"; }
                    if desired.is_empty() {
                        let _ = ws.close(None).await;
                        break "sin suscripciones";
                    }
                }
                msg = tokio::time::timeout(IDLE_TIMEOUT, ws.next()) => {
                    match msg {
                        Err(_) => break "sin datos (idle)",
                        Ok(None) => break "cerrada por el servidor",
                        Ok(Some(Err(e))) => {
                            tracing::debug!(feed = name, error = %ws_error_kind(&e), "WS: error de lectura");
                            break "error de lectura";
                        }
                        Ok(Some(Ok(Message::Text(t)))) => {
                            if !got_message { got_message = true; attempt = 0; }
                            for tick in feed.parse(t.as_str()) {
                                // Cola llena = consumidor lento: se descarta el tick (llegará otro).
                                let _ = ingest.try_send((idx, tick));
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) => break "cerrada por el servidor",
                        // Ping/Pong los gestiona tungstenite; binarios no se usan.
                        Ok(Some(Ok(_))) => {}
                    }
                }
            }
        };
        connected.store(false, Ordering::Relaxed);
        tracing::info!(feed = name, reason, "WS desconectado");
        if desired.is_empty() {
            attempt = 0;
            continue;
        }
        if !wait_backoff(&mut attempt, &mut desired, &mut cmds, &cancel).await {
            return;
        }
    }
}

/// Duerme el backoff atendiendo a los comandos. `false` = hay que terminar el worker.
async fn wait_backoff(
    attempt: &mut u32,
    desired: &mut HashSet<String>,
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
    cancel: &CancellationToken,
) -> bool {
    let wait = backoff(*attempt);
    *attempt = attempt.saturating_add(1);
    let sleep = tokio::time::sleep(wait);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return false,
            () = &mut sleep => return true,
            c = cmds.recv() => match c {
                Some(c) => { apply(desired, c); }
                None => return false,
            },
        }
    }
}

fn ws_error_kind(e: &tokio_tungstenite::tungstenite::Error) -> &'static str {
    use tokio_tungstenite::tungstenite::Error as E;
    match e {
        E::ConnectionClosed | E::AlreadyClosed => "conexión cerrada",
        E::Io(_) => "E/S",
        E::Tls(_) => "TLS",
        E::Capacity(_) => "capacidad",
        E::Protocol(_) => "protocolo",
        E::Utf8(_) => "UTF-8",
        E::Url(_) => "URL",
        E::Http(r) if r.status().as_u16() == 429 => "HTTP 429",
        E::Http(r) if matches!(r.status().as_u16(), 401 | 403) => "HTTP 401/403 (clave rechazada)",
        E::Http(_) | E::HttpFormat(_) => "HTTP",
        _ => "otro",
    }
}

/// Envía mensajes de control separados al menos `gap` entre sí (y del último enviado antes).
/// `false` si el envío falla.
async fn send_paced<W: SendText>(
    ws: &mut W,
    msgs: Vec<String>,
    gap: Duration,
    last: &mut Option<tokio::time::Instant>,
) -> bool {
    for m in msgs {
        if let Some(prev) = *last {
            let wait = gap.saturating_sub(prev.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        if ws.send_text(m).await.is_err() {
            return false;
        }
        *last = Some(tokio::time::Instant::now());
    }
    true
}

trait SendText {
    async fn send_text(&mut self, text: String) -> Result<(), ()>;
}

impl<S> SendText for tokio_tungstenite::WebSocketStream<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    async fn send_text(&mut self, text: String) -> Result<(), ()> {
        use futures::SinkExt;
        self.send(Message::text(text)).await.map_err(|_| ())
    }
}

// ---------------------------------------------------------------------------------------------
// Hub
// ---------------------------------------------------------------------------------------------

/// Obtiene una cotización (con caché) para el sondeo y la referencia de cambio.
pub type QuoteFn = Arc<dyn Fn(AssetId) -> BoxFuture<'static, Option<Quote>> + Send + Sync>;

type ToProvider = Box<dyn Fn(&AssetId) -> Option<String> + Send + Sync>;
/// Un símbolo del feed puede corresponder a varias clases (`SPY` como acción o ETF).
type FromProvider = Box<dyn Fn(&str) -> Vec<AssetId> + Send + Sync>;

struct FeedSlot {
    handle: WsHandle,
    to_provider: ToProvider,
    from_provider: FromProvider,
}

struct Entry {
    tx: broadcast::Sender<PriceTick>,
    refs: usize,
    feed: Option<usize>,
    provider_symbol: Option<String>,
    poller: CancellationToken,
    /// Cierre del día anterior (o apertura 24 h) para calcular el cambio si el feed no lo da.
    reference: Option<f64>,
    last_sent: Option<Instant>,
    pending: Option<PriceTick>,
    flush_scheduled: bool,
    last_tick: Option<PriceTick>,
}

struct Inner {
    entries: Mutex<HashMap<AssetId, Entry>>,
    feeds: Vec<FeedSlot>,
    quote_fn: QuoteFn,
    cancel: CancellationToken,
    poll_every: Duration,
}

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

/// Construye el hub con sus feeds WS. Debe llamarse dentro de un runtime de Tokio.
type Spawner =
    Box<dyn FnOnce(usize, mpsc::Sender<(usize, RawTick)>, CancellationToken) -> WsHandle + Send>;

pub struct HubBuilder {
    feeds: Vec<(Spawner, ToProvider, FromProvider)>,
}

impl Default for HubBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HubBuilder {
    pub fn new() -> Self {
        Self { feeds: Vec::new() }
    }

    /// Añade un feed. `to_provider` traduce el `AssetId` al símbolo del feed (o `None` si no lo
    /// cubre); `from_provider` hace el camino inverso.
    pub fn feed<F: WsFeed>(
        mut self,
        feed: F,
        to_provider: impl Fn(&AssetId) -> Option<String> + Send + Sync + 'static,
        from_provider: impl Fn(&str) -> Vec<AssetId> + Send + Sync + 'static,
    ) -> Self {
        self.feeds.push((
            Box::new(move |idx, tx, cancel| spawn_ws(feed, tx, idx, cancel)),
            Box::new(to_provider),
            Box::new(from_provider),
        ));
        self
    }

    pub fn build(self, quote_fn: QuoteFn, cancel: CancellationToken) -> Hub {
        let (ingest_tx, mut ingest_rx) = mpsc::channel::<(usize, RawTick)>(4096);
        let feeds = self
            .feeds
            .into_iter()
            .enumerate()
            .map(|(i, (spawn, to_provider, from_provider))| FeedSlot {
                handle: spawn(i, ingest_tx.clone(), cancel.clone()),
                to_provider,
                from_provider,
            })
            .collect();
        let inner = Arc::new(Inner {
            entries: Mutex::new(HashMap::new()),
            feeds,
            quote_fn,
            cancel: cancel.clone(),
            poll_every: Duration::from_secs(5),
        });
        let hub = Hub {
            inner: inner.clone(),
        };
        let weak = Arc::downgrade(&inner);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    item = ingest_rx.recv() => {
                        let Some((idx, raw)) = item else { break };
                        let Some(inner) = weak.upgrade() else { break };
                        inner.on_raw(idx, raw);
                    }
                }
            }
        });
        hub
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<AssetId, Entry>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn on_raw(self: &Arc<Self>, idx: usize, raw: RawTick) {
        let Some(slot) = self.feeds.get(idx) else {
            return;
        };
        let ids = (slot.from_provider)(&raw.symbol);
        let source = slot.handle.name();
        let mut map = self.lock();
        for id in ids {
            let Some(e) = map.get_mut(&id) else { continue };
            let reference = raw.open24h.or(e.reference);
            let tick = PriceTick {
                id: id.clone(),
                price: raw.price,
                change_pct: reference
                    .filter(|r| *r > 0.0)
                    .map(|r| (raw.price / r - 1.0) * 100.0),
                volume: raw.volume,
                ts: raw.ts,
                source: source.into(),
            };
            self.emit_locked(&mut map, tick);
        }
    }

    /// Emite respetando el límite de frecuencia. Debe llamarse con el mapa bloqueado.
    fn emit_locked(self: &Arc<Self>, map: &mut HashMap<AssetId, Entry>, tick: PriceTick) {
        let Some(e) = map.get_mut(&tick.id) else {
            return;
        };
        let now = Instant::now();
        if let Some(last) = e.last_sent {
            let since = now.duration_since(last);
            if since < MIN_TICK_GAP {
                let id = tick.id.clone();
                e.pending = Some(tick);
                if !e.flush_scheduled {
                    e.flush_scheduled = true;
                    let inner = self.clone();
                    let wait = MIN_TICK_GAP - since;
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        let mut map = inner.lock();
                        if let Some(e) = map.get_mut(&id) {
                            e.flush_scheduled = false;
                            if let Some(t) = e.pending.take() {
                                e.last_sent = Some(Instant::now());
                                e.last_tick = Some(t.clone());
                                let _ = e.tx.send(t);
                            }
                        }
                    });
                }
                return;
            }
        }
        e.last_sent = Some(now);
        e.pending = None;
        e.last_tick = Some(tick.clone());
        let _ = e.tx.send(tick);
    }

    fn release(self: &Arc<Self>, id: &AssetId) {
        let mut map = self.lock();
        let Some(e) = map.get_mut(id) else { return };
        e.refs = e.refs.saturating_sub(1);
        if e.refs > 0 {
            return;
        }
        if let Some(e) = map.remove(id) {
            e.poller.cancel();
            if let (Some(i), Some(sym)) = (e.feed, e.provider_symbol) {
                let _ = self.feeds[i].handle.cmd_tx.send(Cmd::Unsub(sym));
            }
        }
    }
}

impl Hub {
    /// Hub sin feeds WS (todo se sondea). Útil cuando el tiempo real está desactivado.
    pub fn polling_only(quote_fn: QuoteFn, cancel: CancellationToken) -> Self {
        HubBuilder::new().build(quote_fn, cancel)
    }

    /// Estado de cada feed WS (nombre, conectado).
    pub fn feed_status(&self) -> Vec<(&'static str, bool)> {
        self.inner
            .feeds
            .iter()
            .map(|f| (f.handle.name(), f.handle.is_connected()))
            .collect()
    }

    /// Número de activos con suscriptores (para métricas y tests).
    pub fn active_assets(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn subscribe(&self, ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        let mut seen = HashSet::new();
        let mut streams: Vec<BoxStream<'static, PriceTick>> = Vec::new();
        let mut held: Vec<AssetId> = Vec::new();
        for id in ids.iter().filter(|i| seen.insert((*i).clone())).take(200) {
            let (rx, last) = self.acquire(id);
            held.push(id.clone());
            if let Some(t) = last {
                streams.push(stream::iter([t]).boxed());
            }
            streams.push(
                BroadcastStream::new(rx)
                    .filter_map(|r| async move { r.ok() })
                    .boxed(),
            );
        }
        let guard = SubGuard {
            hub: self.inner.clone(),
            ids: held,
        };
        Guarded {
            inner: stream::select_all(streams).boxed(),
            _guard: guard,
        }
        .boxed()
    }

    fn acquire(&self, id: &AssetId) -> (broadcast::Receiver<PriceTick>, Option<PriceTick>) {
        let inner = &self.inner;
        let mut map = inner.lock();
        if let Some(e) = map.get_mut(id) {
            e.refs += 1;
            return (e.tx.subscribe(), e.last_tick.clone());
        }
        let (tx, rx) = broadcast::channel(64);
        let feed = inner
            .feeds
            .iter()
            .enumerate()
            .find_map(|(i, f)| (f.to_provider)(id).map(|sym| (i, sym)));
        let poller = inner.cancel.child_token();
        if let Some((i, sym)) = &feed {
            let _ = inner.feeds[*i].handle.cmd_tx.send(Cmd::Sub(sym.clone()));
        }
        map.insert(
            id.clone(),
            Entry {
                tx,
                refs: 1,
                feed: feed.as_ref().map(|(i, _)| *i),
                provider_symbol: feed.map(|(_, s)| s),
                poller: poller.clone(),
                reference: None,
                last_sent: None,
                pending: None,
                flush_scheduled: false,
                last_tick: None,
            },
        );
        drop(map);
        tokio::spawn(poll_task(inner.clone(), id.clone(), poller));
        (rx, None)
    }
}

/// Intervalo del sondeo de un activo con el mercado cerrado y sin cambios desde el sondeo anterior.
const CLOSED_POLL: Duration = Duration::from_secs(60);

/// Cada cuánto se renueva el precio de referencia (cierre previo) mientras el WS está conectado y
/// el sondeo de respaldo en pausa: sin esto, el cambio de los ticks se mediría contra un cierre
/// antiguo si la pestaña sigue abierta al cambiar de sesión.
const REFERENCE_REFRESH: Duration = Duration::from_secs(600);

/// Sondeo de respaldo. Además fija el precio de referencia (cierre previo) para calcular el
/// cambio de los ticks WS que no lo traen.
async fn poll_task(inner: Arc<Inner>, id: AssetId, cancel: CancellationToken) {
    let every = if id.class == AssetClass::Crypto {
        inner.poll_every.mul_f64(0.6)
    } else {
        inner.poll_every
    };
    let mut last: Option<(f64, DateTime<Utc>)> = None;
    let mut first = true;
    let mut last_query = tokio::time::Instant::now();
    let mut wait = every;
    loop {
        let feed_ok = {
            let map = inner.lock();
            map.get(&id)
                .and_then(|e| e.feed)
                .is_some_and(|i| inner.feeds[i].handle.is_connected())
        };
        // La primera vuelta siempre consulta: fija la referencia y da un primer valor. Con el WS
        // conectado solo se consulta de vez en cuando para renovar la referencia.
        let refresh_ref = feed_ok && last_query.elapsed() >= REFERENCE_REFRESH;
        if first || !feed_ok || refresh_ref {
            last_query = tokio::time::Instant::now();
            let fut = (inner.quote_fn)(id.clone());
            let q = tokio::select! {
                () = cancel.cancelled() => return,
                q = fut => q,
            };
            wait = every;
            if let Some(q) = q {
                // Fuera de la sesión regular (cerrado, pre o post) el precio y su hora de la quote no
                // cambian: si no hay novedad, no tiene sentido preguntar cada pocos segundos.
                if matches!(q.market_state, MarketState::Closed | MarketState::Pre | MarketState::Post)
                    && last.is_some_and(|(p, t)| p == q.price && t == q.timestamp)
                {
                    wait = every.max(CLOSED_POLL);
                }
                let mut map = inner.lock();
                if let Some(e) = map.get_mut(&id)
                    && let Some(prev) = q.prev_close.filter(|p| *p > 0.0)
                {
                    e.reference = Some(prev);
                }
                let changed = last.is_none_or(|(p, t)| p != q.price || t != q.timestamp);
                if changed && (!feed_ok || first) {
                    last = Some((q.price, q.timestamp));
                    let tick = PriceTick {
                        id: id.clone(),
                        price: q.price,
                        change_pct: q.change_pct,
                        volume: q.volume,
                        ts: q.timestamp,
                        source: q.source.clone(),
                    };
                    inner.emit_locked(&mut map, tick);
                }
            }
            first = false;
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
    }
}

struct SubGuard {
    hub: Arc<Inner>,
    ids: Vec<AssetId>,
}

impl Drop for SubGuard {
    fn drop(&mut self) {
        for id in &self.ids {
            self.hub.release(id);
        }
    }
}

struct Guarded {
    inner: BoxStream<'static, PriceTick>,
    _guard: SubGuard,
}

impl Stream for Guarded {
    type Item = PriceTick;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<PriceTick>> {
        self.inner.poll_next_unpin(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use futures::FutureExt;

    struct DummyFeed;
    impl WsFeed for DummyFeed {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn url(&self) -> String {
            "ws://127.0.0.1:1/".into()
        }
        fn subscribe_msgs(&self, _: &[String], _: bool) -> Vec<String> {
            vec![]
        }
        fn parse(&self, _: &str) -> Vec<RawTick> {
            vec![]
        }
    }

    fn quote(sym: &str, price: f64, secs: i64) -> Quote {
        Quote {
            id: AssetId::new(AssetClass::Stock, sym),
            price,
            change: None,
            change_pct: Some(1.0),
            open: None,
            high: None,
            low: None,
            prev_close: Some(100.0),
            volume: None,
            market_cap: None,
            currency: None,
            market_state: domain::MarketState::Regular,
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            source: "test".into(),
            delay_secs: Some(0),
            source_url: None,
            timestamp_origin: None,
        }
    }

    fn polling_hub(counter: Arc<std::sync::atomic::AtomicU64>) -> (Hub, CancellationToken) {
        let cancel = CancellationToken::new();
        let qf: QuoteFn = Arc::new(move |id: AssetId| {
            let n = counter.fetch_add(1, Ordering::SeqCst) as i64;
            async move { Some(quote(&id.symbol, 100.0 + n as f64, 1000 + n)) }.boxed()
        });
        (Hub::polling_only(qf, cancel.clone()), cancel)
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        for a in 0..12 {
            let d = backoff(a);
            assert!(d <= BACKOFF_MAX);
            assert!(
                d >= BACKOFF_BASE.saturating_mul(1 << a.min(6)).min(BACKOFF_MAX) / 2
                    - Duration::from_millis(1)
            );
        }
        assert!(backoff(5) > backoff(0));
    }

    #[tokio::test(start_paused = true)]
    async fn polling_feeds_subscribers_and_refcounts() {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (hub, cancel) = polling_hub(counter.clone());
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let mut a = hub.subscribe(std::slice::from_ref(&id));
        let mut b = hub.subscribe(std::slice::from_ref(&id));
        assert_eq!(hub.active_assets(), 1);
        let t = a.next().await.unwrap();
        assert_eq!(t.id, id);
        assert_eq!(t.price, 100.0);
        // Cambio calculado por el proveedor (1.0), tiempo = el del proveedor.
        assert_eq!(t.ts.timestamp(), 1000);
        assert_eq!(b.next().await.unwrap().price, 100.0);
        // Solo hubo una consulta compartida para los dos suscriptores.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        drop(a);
        assert_eq!(hub.active_assets(), 1);
        drop(b);
        assert_eq!(hub.active_assets(), 0);
        cancel.cancel();
    }

    /// Hub de sondeo cuyo proveedor devuelve siempre la misma cotización.
    fn frozen_hub(state: domain::MarketState, counter: Arc<std::sync::atomic::AtomicU64>) -> (Hub, CancellationToken) {
        let cancel = CancellationToken::new();
        let qf: QuoteFn = Arc::new(move |id: AssetId| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                let mut q = quote(&id.symbol, 100.0, 1000);
                q.market_state = state;
                Some(q)
            }
            .boxed()
        });
        (Hub::polling_only(qf, cancel.clone()), cancel)
    }

    #[tokio::test(start_paused = true)]
    async fn closed_market_without_changes_is_polled_rarely_and_emits_one_tick() {
        // Pre y post tampoco mueven la quote regular: mismo trato que cerrado.
        for state in [domain::MarketState::Closed, domain::MarketState::Pre, domain::MarketState::Post] {
            closed_case(state).await;
        }
    }

    async fn closed_case(state: domain::MarketState) {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (hub, cancel) = frozen_hub(state, counter.clone());
        let mut s = hub.subscribe(&[AssetId::new(AssetClass::Stock, "AAPL")]);
        assert_eq!(s.next().await.unwrap().price, 100.0);
        tokio::time::sleep(Duration::from_secs(300)).await;
        // Con 5 s serían 60 consultas; con el sondeo espaciado a 60 s, unas 6.
        let n = counter.load(Ordering::SeqCst);
        assert!(n <= 8, "demasiadas consultas con el mercado cerrado: {n}");
        assert!(tokio::time::timeout(Duration::from_millis(10), s.next()).await.is_err(), "no debe repetir el tick");
        cancel.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn open_market_keeps_the_fast_poll_but_does_not_repeat_identical_ticks() {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (hub, cancel) = frozen_hub(domain::MarketState::Regular, counter.clone());
        let mut s = hub.subscribe(&[AssetId::new(AssetClass::Stock, "AAPL")]);
        assert_eq!(s.next().await.unwrap().price, 100.0);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(counter.load(Ordering::SeqCst) >= 10);
        assert!(tokio::time::timeout(Duration::from_millis(10), s.next()).await.is_err());
        cancel.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn late_subscriber_gets_last_tick() {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (hub, cancel) = polling_hub(counter);
        let id = AssetId::new(AssetClass::Stock, "MSFT");
        let mut a = hub.subscribe(std::slice::from_ref(&id));
        let first = a.next().await.unwrap();
        let mut b = hub.subscribe(std::slice::from_ref(&id));
        let replay = b.next().await.unwrap();
        assert_eq!(replay.price, first.price);
        cancel.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn throttles_ws_ticks_but_keeps_the_last_one() {
        let cancel = CancellationToken::new();
        let qf: QuoteFn = Arc::new(|_| async { None }.boxed());
        let hub = HubBuilder::new()
            .feed(
                DummyFeed,
                |id: &AssetId| Some(id.symbol.clone()),
                |s: &str| vec![AssetId::new(AssetClass::Stock, s)],
            )
            .build(qf, cancel.clone());
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let mut s = hub.subscribe(std::slice::from_ref(&id));
        let ts = Utc.timestamp_opt(5, 0).unwrap();
        let raw = |p: f64| RawTick {
            symbol: "AAPL".into(),
            price: p,
            open24h: Some(100.0),
            volume: None,
            ts,
        };
        // Ráfaga de 20 ticks en el mismo instante.
        for i in 0..20 {
            hub.inner.on_raw(0, raw(101.0 + f64::from(i)));
        }
        let first = s.next().await.unwrap();
        assert_eq!(first.price, 101.0);
        assert!((first.change_pct.unwrap() - 1.0).abs() < 1e-9);
        // El resto se colapsa en el último valor, emitido tras la ventana.
        let last = s.next().await.unwrap();
        assert_eq!(last.price, 120.0);
        cancel.cancel();
    }

    /// Feed contra un servidor WS local: cada mensaje de control lleva hasta 3 símbolos.
    struct LocalFeed {
        url: String,
    }
    impl WsFeed for LocalFeed {
        fn name(&self) -> &'static str {
            "local"
        }
        fn url(&self) -> String {
            self.url.clone()
        }
        fn subscribe_msgs(&self, symbols: &[String], on: bool) -> Vec<String> {
            symbols
                .chunks(3)
                .map(|c| format!("{}:{}", if on { "SUB" } else { "UNSUB" }, c.join(",")))
                .collect()
        }
        fn min_msg_gap(&self) -> Duration {
            Duration::from_millis(60)
        }
        fn parse(&self, _: &str) -> Vec<RawTick> {
            vec![]
        }
    }

    /// Medido con Binance real: 10 `SUBSCRIBE` en ráfaga (1 al conectar + 9 de otros activos) pasan
    /// de 5 mensajes/s; solo se atienden 5, no llega ningún tick y la conexión se cierra. Un cliente
    /// que se suscribe a varios activos a la vez debe producir pocos mensajes y espaciados.
    #[tokio::test]
    async fn burst_of_subscriptions_is_batched_and_paced() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let received: Arc<Mutex<Vec<(Instant, String)>>> = Arc::default();
        let log = received.clone();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            while let Some(Ok(m)) = ws.next().await {
                if let Message::Text(t) = m {
                    log.lock().unwrap().push((Instant::now(), t.to_string()));
                }
            }
        });
        let cancel = CancellationToken::new();
        let (ingest, _rx) = mpsc::channel(16);
        let h = spawn_ws(LocalFeed { url }, ingest, 0, cancel.clone());
        h.cmd_tx.send(Cmd::Sub("S0".into())).unwrap();
        while !h.is_connected() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Ráfaga de 9 altas más, una a una, como hace el hub.
        for i in 1..10 {
            h.cmd_tx.send(Cmd::Sub(format!("S{i}"))).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        let msgs = received.lock().unwrap().clone();
        let symbols: HashSet<String> = msgs
            .iter()
            .flat_map(|(_, m)| m.strip_prefix("SUB:").unwrap_or_default().split(',').map(str::to_owned).collect::<Vec<_>>())
            .collect();
        assert_eq!(symbols.len(), 10, "llegan los 10 símbolos: {msgs:?}");
        // 1 mensaje inicial + 9 símbolos en bloques de 3 = 4 (antes: 10).
        assert!(msgs.len() <= 5, "demasiados mensajes de control: {msgs:?}");
        for w in msgs.windows(2) {
            assert!(w[1].0 - w[0].0 >= Duration::from_millis(50), "mensajes sin espaciar: {msgs:?}");
        }
        cancel.cancel();
    }

    /// Bajas en ráfaga (el último cliente se va): una tanda de `UNSUB`, no una por activo, y la
    /// conexión se cierra al quedarse sin símbolos.
    #[tokio::test]
    async fn burst_of_unsubscriptions_is_batched() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let received: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = received.clone();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            while let Some(Ok(m)) = ws.next().await {
                if let Message::Text(t) = m {
                    log.lock().unwrap().push(t.to_string());
                }
            }
        });
        let cancel = CancellationToken::new();
        let (ingest, _rx) = mpsc::channel(16);
        let h = spawn_ws(LocalFeed { url }, ingest, 0, cancel.clone());
        for i in 0..3 {
            h.cmd_tx.send(Cmd::Sub(format!("S{i}"))).unwrap();
        }
        // Espera a que el proveedor haya recibido las 3 altas (algunas van en una segunda tanda).
        for _ in 0..200 {
            let n: usize = received.lock().unwrap().iter().filter(|m| m.starts_with("SUB:")).map(|m| m.matches(',').count() + 1).sum();
            if n >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for i in 0..3 {
            h.cmd_tx.send(Cmd::Unsub(format!("S{i}"))).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let msgs = received.lock().unwrap().clone();
        let unsubs: Vec<_> = msgs.iter().filter(|m| m.starts_with("UNSUB:")).collect();
        assert_eq!(unsubs.len(), 1, "una sola tanda de bajas, no una por activo: {msgs:?}");
        assert_eq!(unsubs[0].matches(',').count(), 2, "con los 3 símbolos: {msgs:?}");
        assert!(!h.is_connected(), "sin suscripciones se cierra la conexión");
        cancel.cancel();
    }

    /// R-06: con el WS conectado el sondeo se detiene y la referencia del cambio quedaba fija en
    /// el cierre de la primera quote, aunque pasaran días. Debe renovarse periódicamente.
    #[tokio::test(start_paused = true)]
    async fn reference_is_refreshed_while_the_feed_is_connected() {
        let cancel = CancellationToken::new();
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let c2 = calls.clone();
        let qf: QuoteFn = Arc::new(move |id: AssetId| {
            let n = c2.fetch_add(1, Ordering::SeqCst);
            // Primera consulta: cierre previo 100. Las siguientes (otro día de mercado): 200.
            let mut q = quote(&id.symbol, 100.0, 1000 + n as i64);
            q.prev_close = Some(if n == 0 { 100.0 } else { 200.0 });
            async move { Some(q) }.boxed()
        });
        let hub = HubBuilder::new()
            .feed(
                DummyFeed,
                |id: &AssetId| Some(id.symbol.clone()),
                |s: &str| vec![AssetId::new(AssetClass::Stock, s)],
            )
            .build(qf, cancel.clone());
        hub.inner.feeds[0].handle.connected.store(true, Ordering::Relaxed);
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let mut s = hub.subscribe(std::slice::from_ref(&id));
        let _ = s.next().await.unwrap(); // tick del sondeo inicial
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        let raw = RawTick {
            symbol: "AAPL".into(),
            price: 210.0,
            open24h: None,
            volume: None,
            ts: Utc.timestamp_opt(90_000, 0).unwrap(),
        };
        hub.inner.on_raw(0, raw);
        let t = s.next().await.unwrap();
        assert_eq!(t.price, 210.0);
        assert!((t.change_pct.unwrap() - 5.0).abs() < 1e-9, "cambio contra el cierre vigente: {:?}", t.change_pct);
        cancel.cancel();
    }
}
