//! Análisis de IA por activo: el último guardado, generar uno nuevo, histórico y snapshot.

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use axum::{
    Json,
    http::StatusCode,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use domain::{Analysis, AnalysisDepth, AnalysisEvent, AssetId, AssetSnapshot, DomainError};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::{
    db::new_id,
    error::{ApiError, ApiResult},
    extract::{AppQuery, AssetPath},
    repo,
    resolve::cache_summaries,
    state::AppState,
};

const HISTORY_LIMIT: i64 = 20;
/// Tope de seguridad: el cliente HTTP del analista ya tiene su propio timeout (`LLM_TIMEOUT_SECS`).
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(900);

/// `?depth=quick|standard|deep` (opcional: por defecto según el modelo activo).
#[derive(Deserialize, Default)]
pub struct GenerateQuery {
    pub depth: Option<String>,
}

pub async fn latest(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<Option<Analysis>>> {
    Ok(Json(repo::analyses::latest(&state.db, &id).await?))
}

pub async fn history(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<Vec<Analysis>>> {
    Ok(Json(repo::analyses::list_for_asset(&state.db, &id, HISTORY_LIMIT).await?))
}

pub async fn snapshot(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
) -> ApiResult<Json<AssetSnapshot>> {
    let mut snapshot = state.market.snapshot(&id).await?;
    (state.annotate_news)(&mut snapshot.news);
    Ok(Json(snapshot))
}

/// Progreso compartido de un análisis en curso: cualquier número de clientes (SSE o POST) se
/// engancha al mismo. El trabajo en sí vive en `AppState::analysis_flights` (single-flight).
struct Entry {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<AnalysisEvent>,
}

#[derive(Default)]
struct Inner {
    /// Última etapa publicada (para quien se engancha tarde).
    last_stage: Option<AnalysisEvent>,
    /// `Done` o `Error`.
    terminal: Option<AnalysisEvent>,
}

impl Entry {
    fn publish(&self, ev: AnalysisEvent) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match ev {
            AnalysisEvent::Stage { .. } => inner.last_stage = Some(ev.clone()),
            _ => inner.terminal = Some(ev.clone()),
        }
        let _ = self.tx.send(ev);
    }

    /// Eventos ya ocurridos que el nuevo cliente debe ver primero + receptor del resto. Bajo el
    /// mismo cerrojo que `publish`, así no se pierde ni se duplica ninguno.
    fn subscribe(&self) -> (VecDeque<AnalysisEvent>, broadcast::Receiver<AnalysisEvent>) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut initial = VecDeque::new();
        initial.extend(inner.last_stage.clone());
        initial.extend(inner.terminal.clone());
        (initial, self.tx.subscribe())
    }
}

type HubKey = (usize, AssetId);
static HUB: LazyLock<Mutex<HashMap<HubKey, Arc<Entry>>>> = LazyLock::new(Default::default);

fn hub_key(state: &AppState, id: &AssetId) -> HubKey {
    // El single-flight es único por estado de aplicación: sirve de espacio de nombres.
    (Arc::as_ptr(&state.analysis_flights) as usize, id.clone())
}

fn is_terminal(ev: &AnalysisEvent) -> bool {
    !matches!(ev, AnalysisEvent::Stage { .. })
}

/// Engancha al análisis en curso de `id` o lo lanza. El trabajo sigue aunque todos los clientes
/// se desconecten, guarda en BD y se cancela con el apagado.
fn join_or_start(state: &AppState, id: &AssetId, depth: Option<AnalysisDepth>, origin: &'static str) -> Arc<Entry> {
    let key = hub_key(state, id);
    let mut hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = hub.get(&key) {
        tracing::debug!(%id, "análisis ya en curso: se comparte");
        return e.clone();
    }
    let entry = Arc::new(Entry { inner: Mutex::default(), tx: broadcast::channel(64).0 });
    hub.insert(key.clone(), entry.clone());
    drop(hub);

    let (job_state, job_id, job_entry) = (state.clone(), id.clone(), entry.clone());
    state.tasks.spawn(async move {
        let flights = job_state.analysis_flights.clone();
        let (st, jid, je) = (job_state.clone(), job_id.clone(), job_entry.clone());
        let (result, _) = flights.run(job_id.clone(), move || async move { run_analysis(st, jid, depth, je, origin).await.map(Arc::new) }).await;
        job_entry.publish(match result {
            Ok(a) => AnalysisEvent::Done { analysis: Analysis::clone(&a) },
            Err(e) => AnalysisEvent::Error { code: e.code().to_string(), message: e.to_string() },
        });
        // Quien se enganche en este instante aún recibe el evento final desde `terminal`.
        let mut hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
        if hub.get(&key).is_some_and(|e| Arc::ptr_eq(e, &job_entry)) {
            hub.remove(&key);
        }
    });
    entry
}

/// Genera un análisis por el mismo camino que la API (single-flight por activo, límite de
/// generaciones simultáneas, guardado en BD) y espera al resultado. Lo usa la lista de seguimiento
/// con `origin = "watchlist"`. Si ya había uno en curso para el activo, se suma a ese (y se guarda
/// con el origen de quien lo lanzó).
pub async fn run_to_completion(
    state: &AppState,
    id: &AssetId,
    depth: Option<AnalysisDepth>,
    origin: &'static str,
) -> Result<Analysis, DomainError> {
    let entry = join_or_start(state, id, depth, origin);
    let (mut initial, mut rx) = entry.subscribe();
    loop {
        let ev = match initial.pop_front() {
            Some(ev) => ev,
            None => match rx.recv().await {
                Ok(ev) => ev,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(DomainError::Internal("el análisis terminó sin resultado".into()));
                }
            },
        };
        match ev {
            AnalysisEvent::Done { analysis } => return Ok(analysis),
            AnalysisEvent::Error { code, message } => return Err(DomainError::from_code(&code, message)),
            AnalysisEvent::Stage { .. } => {}
        }
    }
}

/// Genera un análisis nuevo. Single-flight por activo: si ya hay uno en curso, espera a ese.
pub async fn generate(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
    AppQuery(q): AppQuery<GenerateQuery>,
) -> ApiResult<Json<Analysis>> {
    let depth: Option<AnalysisDepth> = q.depth.as_deref().map(str::parse).transpose()?;
    let entry = join_or_start(&state, &id, depth, "analyze");
    let (mut initial, mut rx) = entry.subscribe();
    loop {
        let ev = match initial.pop_front() {
            Some(ev) => ev,
            None => match rx.recv().await {
                Ok(ev) => ev,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(DomainError::Internal("el análisis terminó sin resultado".into()).into());
                }
            },
        };
        match ev {
            AnalysisEvent::Done { analysis } => return Ok(Json(analysis)),
            AnalysisEvent::Error { code, message } if code == "rate_limited" => {
                return Err(ApiError::Http { status: StatusCode::TOO_MANY_REQUESTS, code: "rate_limited", message });
            }
            AnalysisEvent::Error { code, message } => return Err(DomainError::from_code(&code, message).into()),
            AnalysisEvent::Stage { .. } => {}
        }
    }
}

/// `POST /assets/{id}/analysis/stream`: SSE de `AnalysisEvent` (etapas y, al final, `Done` o `Error`).
pub async fn generate_stream(
    State(state): State<AppState>,
    AssetPath(id): AssetPath,
    AppQuery(q): AppQuery<GenerateQuery>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let depth: Option<AnalysisDepth> = q.depth.as_deref().map(str::parse).transpose()?;
    let entry = join_or_start(&state, &id, depth, "analyze");
    let (initial, rx) = entry.subscribe();
    let shutdown = state.shutdown.clone();
    // (eventos pendientes, receptor, terminado)
    let events = futures::stream::unfold((initial, rx, false), |(mut initial, mut rx, done)| async move {
        if done {
            return None;
        }
        let ev = match initial.pop_front() {
            Some(ev) => ev,
            None => loop {
                match rx.recv().await {
                    Ok(ev) => break ev,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        break AnalysisEvent::Error { code: "internal".into(), message: "el análisis terminó sin resultado".into() };
                    }
                }
            },
        };
        let end = is_terminal(&ev);
        Some((ev, (initial, rx, end)))
    })
    .take_until(async move { shutdown.cancelled().await })
    .map(|ev| {
        Ok(Event::default().json_data(&ev).unwrap_or_else(|_| {
            Event::default().data(r#"{"type":"error","code":"internal","message":"error interno de serialización"}"#)
        }))
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive")))
}

async fn run_analysis(
    state: AppState,
    id: AssetId,
    depth: Option<AnalysisDepth>,
    entry: Arc<Entry>,
    origin: &'static str,
) -> Result<Analysis, DomainError> {
    // Límite de análisis simultáneos: solo lo consume quien lanza la generación (el líder del
    // single-flight); los que se suman no. El permiso vive hasta terminar, aunque el cliente se vaya.
    let _permit = state.limits.analyses.clone().try_acquire_owned().map_err(|_| {
        DomainError::RateLimited { provider: "análisis simultáneos de IA: espera a que termine alguno".into() }
    })?;
    let snapshot: AssetSnapshot = state.market.snapshot(&id).await?;
    cache_summaries(&state, std::slice::from_ref(&snapshot.summary)).await;

    let mut stream = state.analyst.clone().analyze_stream(snapshot, depth);
    let deadline = tokio::time::sleep(ANALYZE_TIMEOUT);
    tokio::pin!(deadline);
    let mut analysis: Option<Analysis> = None;
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => {
                return Err(DomainError::Unavailable("análisis cancelado: el servidor se está apagando".into()));
            }
            () = &mut deadline => return Err(DomainError::Unavailable("el análisis tardó demasiado".into())),
            ev = stream.next() => match ev {
                Some(stage @ AnalysisEvent::Stage { .. }) => entry.publish(stage),
                Some(AnalysisEvent::Done { analysis: a }) => { analysis = Some(a); break; }
                Some(AnalysisEvent::Error { code, message }) => return Err(DomainError::from_code(&code, message)),
                None => break,
            }
        }
    }
    let mut analysis = analysis.ok_or_else(|| DomainError::Internal("el analista terminó sin resultado".into()))?;
    if analysis.id.is_empty() {
        analysis.id = new_id();
    }
    // Si guardar falla, el análisis (caro de generar) se devuelve igualmente.
    if let Err(e) = repo::analyses::insert(&state.db, &analysis, origin).await {
        tracing::error!(%id, error = %e, "no se pudo guardar el análisis");
    }
    Ok(analysis)
}
