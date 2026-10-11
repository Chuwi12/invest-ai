//! Single-flight: si ya hay un trabajo en curso para una clave, los demás esperan a ese en vez
//! de lanzar otro (p. ej. dos pestañas pidiendo el análisis del mismo activo).

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Mutex},
};

use domain::DomainError;
use tokio_util::task::TaskTracker;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};

use crate::error::clone_domain_error;

type Flight<V> = Shared<BoxFuture<'static, Result<V, Arc<DomainError>>>>;

pub struct SingleFlight<K, V> {
    inflight: Arc<Mutex<HashMap<K, Flight<V>>>>,
    /// Los trabajos se lanzan aquí para poder esperarlos en el apagado.
    tasks: TaskTracker,
}

/// Quita la clave aunque el trabajo haga panic.
struct Cleanup<K: Hash + Eq, V> {
    map: Arc<Mutex<HashMap<K, Flight<V>>>>,
    key: Option<K>,
}

impl<K: Hash + Eq, V> Drop for Cleanup<K, V> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        }
    }
}

impl<K, V> SingleFlight<K, V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(tasks: TaskTracker) -> Self {
        Self { inflight: Arc::default(), tasks }
    }

    /// Ejecuta `make()` salvo que ya haya un trabajo para `key`, en cuyo caso espera a su resultado.
    ///
    /// El trabajo corre en su propia tarea: si todos los que esperan se desconectan, termina
    /// igualmente (y su efecto, p. ej. guardar el análisis en BD, no se pierde).
    /// Devuelve también si esta llamada fue la que lo lanzó.
    pub async fn run<F, Fut>(&self, key: K, make: F) -> (Result<V, DomainError>, bool)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, DomainError>> + Send + 'static,
    {
        let (flight, leader) = {
            let mut map = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            match map.get(&key) {
                Some(existing) => (existing.clone(), false),
                None => {
                    let work = make();
                    let cleanup = Cleanup { map: self.inflight.clone(), key: Some(key.clone()) };
                    let handle = self.tasks.spawn(async move {
                        let _cleanup = cleanup;
                        work.await
                    });
                    let flight: Flight<V> = async move {
                        match handle.await {
                            Ok(res) => res.map_err(Arc::new),
                            Err(e) => Err(Arc::new(DomainError::Internal(format!("la tarea falló: {e}")))),
                        }
                    }
                    .boxed()
                    .shared();
                    map.insert(key, flight.clone());
                    (flight, true)
                }
            }
        };
        let res = flight.await.map_err(|e| clone_domain_error(&e));
        (res, leader)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn concurrent_calls_share_one_execution() {
        let sf = Arc::new(SingleFlight::<&'static str, u32>::new(TaskTracker::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let (sf, runs) = (sf.clone(), runs.clone());
            handles.push(tokio::spawn(async move {
                sf.run("k", || async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(7)
                })
                .await
            }));
        }
        let mut leaders = 0;
        for h in handles {
            let (res, leader) = h.await.unwrap();
            assert_eq!(res.unwrap(), 7);
            leaders += usize::from(leader);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(leaders, 1);
        // Terminado el trabajo, la siguiente llamada lanza otro.
        let (res, leader) = sf.run("k", || async { Ok(8) }).await;
        assert_eq!(res.unwrap(), 8);
        assert!(leader);
    }

    #[tokio::test]
    async fn errors_are_shared_and_work_survives_dropped_waiters() {
        let sf = Arc::new(SingleFlight::<&'static str, u32>::new(TaskTracker::new()));
        let done = Arc::new(AtomicUsize::new(0));
        let d = done.clone();
        let first = tokio::spawn({
            let sf = sf.clone();
            async move {
                sf.run("k", || async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    d.fetch_add(1, Ordering::SeqCst);
                    Err(DomainError::Unavailable("caído".into()))
                })
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        first.abort(); // el cliente que lo lanzó se desconecta
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(done.load(Ordering::SeqCst), 1);
        let (res, _) = sf.run("k", || async { Err(DomainError::NotFound("x".into())) }).await;
        assert!(matches!(res, Err(DomainError::NotFound(_))));
    }
}
