//! Coste del archivado encendido por defecto (`RAG_ARCHIVE=1`): sin Ollama el texto se guarda y los
//! embeddings quedan pendientes sin martillear al servidor, y la retención acota el disco. Sin red.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use knowledge::{
    DocKind, EmbedError, EmbedKind, Embedder, FakeEmbedder, KnowledgeBase, KnowledgeConfig, NewDocument, SearchRequest, Section, testutil,
};

fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
}

fn news(key: &str, title: &str, text: &str, published: DateTime<Utc>) -> NewDocument {
    NewDocument {
        doc_key: key.into(),
        kind: DocKind::News,
        subtype: None,
        source: "yahoo".into(),
        publisher: None,
        title: title.into(),
        url: Some(format!("https://example.test/{key}")),
        issuer: None,
        language: Some("en".into()),
        published_at: published,
        assets: vec!["stock:AAPL".into()],
        sections: vec![Section { heading: None, text: text.into() }],
        meta: serde_json::json!({}),
    }
}

/// Embedder que siempre falla con el error dado y cuenta las llamadas que recibe.
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

async fn archive_with_failing_embedder(err: fn() -> EmbedError) -> (Arc<KnowledgeBase>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let kb = KnowledgeBase::open(testutil::memory_pool().await, Arc::new(Failing(err, calls.clone())), KnowledgeConfig::default()).await.unwrap();
    (kb, calls)
}

/// Archiva 30 noticias, y luego simula 5 pasadas de mantenimiento, con un embedder que falla.
async fn archive_while_down(err: fn() -> EmbedError) {
    let (kb, calls) = archive_with_failing_embedder(err).await;
    for i in 0..30 {
        let r = kb
            .ingest(news(&format!("n:{i}"), &format!("Headline number {i} about copper"), &format!("Headline number {i} about copper prices and demand."), base()))
            .await
            .expect("el archivado no falla si el embedder está caído");
        assert_eq!((r.chunks, r.embedded, r.pending), (1, 0, 1), "el texto se guarda y el vector queda pendiente");
    }
    for _ in 0..5 {
        assert_eq!(kb.embed_pending(64).await.expect("embed_pending tampoco falla"), 0, "sin embedder no se completa nada");
    }
    let s = kb.stats().await.unwrap();
    assert_eq!((s.documents, s.chunks, s.pending, s.indexed), (30, 30, 30, 0), "{s:?}");
    // Una sola llamada en 35 intentos: tras el primer fallo el embedder queda en pausa y no se le vuelve a llamar.
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no se martillea al servidor de embeddings");
    // La búsqueda léxica (híbrida, por defecto) sigue funcionando con el texto archivado.
    let hits = kb.search(&SearchRequest { now: Some(base()), ..SearchRequest::new("copper demand") }).await.expect("BM25 sin embedder").hits;
    assert!(!hits.is_empty(), "lo archivado sin Ollama ya se puede buscar por texto");
}

#[tokio::test]
async fn ollama_down_stores_text_and_leaves_embeddings_pending_without_hammering() {
    archive_while_down(|| EmbedError::Unavailable("conexión rechazada".into())).await;
}

#[tokio::test]
async fn embedding_model_not_installed_behaves_the_same() {
    // El caso más probable tras activar el archivado por defecto: Ollama funciona pero falta `ollama pull`.
    archive_while_down(|| EmbedError::ModelMissing("embeddinggemma:300m-qat-q8_0".into())).await;
    let msg = EmbedError::ModelMissing("m".into()).to_string();
    assert!(msg.contains("ollama pull m"), "el mensaje dice qué hacer: {msg}");
}

#[tokio::test]
async fn retention_and_cap_keep_the_database_file_bounded() {
    // 50 «días» de 20 noticias de ~0,9 KB con vectores de 768 dim; retención de 10 días y tope de 120 fragmentos.
    // Sin límites habría 1 000 fragmentos; con ellos el archivo debe estabilizarse (SQLite reutiliza las páginas libres).
    let dir = std::env::temp_dir().join(format!("knowledge-archive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("k.db");
    let _ = std::fs::remove_file(&path);
    let cfg = KnowledgeConfig { retention_news_days: 10, max_chunks: 120, ..KnowledgeConfig::default() };
    let kb = KnowledgeBase::open(testutil::file_pool(&path).await, Arc::new(FakeEmbedder { dim: 768 }), cfg).await.unwrap();
    let filler = "Apple shares moved after the company reported quarterly revenue, services growth and guidance for the holiday quarter. ".repeat(8);
    let (mut size_mid, mut size_end, mut max_chunks_seen) = (0i64, 0i64, 0usize);
    for day in 0..50i64 {
        let now = base() + Duration::days(day);
        for n in 0..20 {
            let key = format!("n:{day}:{n}");
            let d = news(&key, &format!("Headline {day}-{n}"), &format!("Headline {day}-{n}. {filler}"), now);
            kb.ingest(d).await.unwrap();
        }
        kb.purge(now).await.unwrap();
        let s = kb.stats().await.unwrap();
        max_chunks_seen = max_chunks_seen.max(s.chunks);
        assert!(s.chunks <= 120, "el tope se respeta tras cada purga: {s:?}");
        let size = db_bytes(&kb).await;
        match day {
            24 => size_mid = size,
            49 => size_end = size,
            _ => {}
        }
    }
    println!("chunks máx. tras purga = {max_chunks_seen}; archivo día 25 = {size_mid} B; día 50 = {size_end} B");
    assert!(size_mid > 0 && size_end > 0);
    assert!(size_end as f64 <= size_mid as f64 * 1.10, "el archivo se estabiliza: {size_mid} -> {size_end} bytes");
    drop(kb);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tamaño del fichero de la BD según SQLite (páginas × tamaño de página).
async fn db_bytes(kb: &KnowledgeBase) -> i64 {
    sqlx::query_scalar("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()").fetch_one(kb.pool_for_tests()).await.unwrap()
}
