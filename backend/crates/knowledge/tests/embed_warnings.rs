//! N3: una caída del embedder da UN solo aviso (`WARN`), no uno por documento ni por pasada.
//!
//! Va en su propio binario de tests y con un solo `#[test]`: `tracing` guarda en caché el «interés» de
//! cada punto de log, y con suscriptores por hilo y tests en paralelo un test ajeno (sin suscriptor)
//! puede dejarlo apagado y hacer que este no vea nada.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use knowledge::{DocKind, EmbedError, EmbedKind, Embedder, FakeEmbedder, KnowledgeBase, KnowledgeConfig, NewDocument, Section, testutil};

fn news(i: usize) -> NewDocument {
    NewDocument {
        doc_key: format!("w:{i}"),
        kind: DocKind::News,
        subtype: None,
        source: "yahoo".into(),
        publisher: None,
        title: format!("Headline {i} about zinc"),
        url: None,
        issuer: None,
        language: Some("en".into()),
        published_at: Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap(),
        assets: vec![],
        sections: vec![Section { heading: None, text: format!("Headline {i} about zinc prices and demand.") }],
        meta: serde_json::json!({}),
    }
}

/// Embedder que siempre falla con el error dado y cuenta las llamadas.
struct Failing(fn() -> EmbedError, Arc<AtomicUsize>);

#[async_trait]
impl Embedder for Failing {
    fn model_id(&self) -> String {
        FakeEmbedder::default().model_id()
    }
    async fn embed(&self, _: &[String], _: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Err((self.0)())
    }
}

/// Nombre del caso y cómo fabrica el error.
type Case = (&'static str, fn() -> EmbedError);

/// Cuenta los avisos del hilo actual (`#[tokio::test]` usa un runtime de un solo hilo).
struct WarnCounter(Arc<AtomicUsize>);

impl tracing::Subscriber for WarnCounter {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        if *e.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn an_embedder_outage_logs_a_single_warning() {
    let warns = Arc::new(AtomicUsize::new(0));
    let _guard = tracing::subscriber::set_default(WarnCounter(warns.clone()));
    let cases: [Case; 3] = [
        ("Bad", || EmbedError::Bad("HTTP 400 Bad Request".into())),
        ("ModelMissing", || EmbedError::ModelMissing("m".into())),
        ("Unavailable", || EmbedError::Unavailable("conexión rechazada".into())),
    ];
    for (name, err) in cases {
        let calls = Arc::new(AtomicUsize::new(0));
        let kb = KnowledgeBase::open(testutil::memory_pool().await, Arc::new(Failing(err, calls.clone())), KnowledgeConfig::default()).await.unwrap();
        let before = warns.load(Ordering::SeqCst);
        for i in 0..12 {
            kb.ingest(news(i)).await.unwrap();
            kb.embed_pending(64).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{name}: una sola llamada al embedder");
        assert_eq!(warns.load(Ordering::SeqCst) - before, 1, "{name}: un solo aviso por caída, no uno por documento");
    }
}
