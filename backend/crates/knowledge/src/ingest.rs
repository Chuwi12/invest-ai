//! Punto de enganche para noticias: el servicio de mercado entrega lo que YA obtiene; este
//! módulo lo encola, deduplica y embebe en segundo plano sin bloquear nunca la petición.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}},
    time::Duration,
};

use domain::{AssetId, NewsItem};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::kb::KnowledgeBase;

const QUEUE: usize = 256;
const SEEN_CAP: usize = 20_000;

struct Batch {
    asset: Option<AssetId>,
    items: Vec<NewsItem>,
}

/// Manejador barato de clonar. `offer` es síncrono y nunca espera.
#[derive(Clone)]
pub struct NewsSink {
    tx: mpsc::Sender<Batch>,
    seen: Arc<Mutex<HashSet<String>>>,
    dropped: Arc<AtomicU64>,
}

impl NewsSink {
    /// Ofrece noticias ya descargadas. Descarta las ofrecidas antes (mismo id y activo) sin tocar
    /// la BD; si la cola está llena, las pierde y lo cuenta (las volverá a ofrecer la próxima
    /// consulta). Jamás bloquea ni falla.
    pub fn offer(&self, asset: Option<&AssetId>, items: &[NewsItem]) {
        let tag = asset.map(ToString::to_string).unwrap_or_default();
        let fresh: Vec<NewsItem> = {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() > SEEN_CAP {
                seen.clear();
            }
            items.iter().filter(|i| seen.insert(format!("{}|{tag}", i.id))).cloned().collect()
        };
        if fresh.is_empty() {
            return;
        }
        let n = fresh.len() as u64;
        if self.tx.try_send(Batch { asset: asset.cloned(), items: fresh.clone() }).is_err() {
            self.dropped.fetch_add(n, Ordering::Relaxed);
            // Se olvidan para que una próxima oferta las reintente.
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            for i in &fresh {
                seen.remove(&format!("{}|{tag}", i.id));
            }
        }
    }

    /// Noticias descartadas por cola llena desde el arranque.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Lanza el trabajador que ingiere lo que llega por el `NewsSink`. Termina con `shutdown` o al
/// soltarse todos los `NewsSink`.
pub fn spawn_news_worker(kb: Arc<KnowledgeBase>, shutdown: CancellationToken) -> (NewsSink, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<Batch>(QUEUE);
    let sink = NewsSink { tx, seen: Arc::default(), dropped: Arc::default() };
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                batch = rx.recv() => {
                    let Some(b) = batch else { break };
                    let rep = kb.ingest_news(b.asset.as_ref(), &b.items).await;
                    if rep.added + rep.updated > 0 {
                        tracing::debug!(added = rep.added, duplicates = rep.duplicates, failed = rep.failed, "noticias archivadas");
                    }
                    // Completa lo que quedó sin vector (embedder caído antes), a ritmo moderado.
                    let _ = kb.embed_pending(64).await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    });
    (sink, handle)
}
