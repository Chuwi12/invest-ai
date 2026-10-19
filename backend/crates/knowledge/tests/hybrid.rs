//! Búsqueda híbrida (FTS5/BM25 + vectorial con RRF). Sin red ni Ollama: `FakeEmbedder` no es
//! semántico (un hash del texto), así que un identificador exacto solo puede encontrarlo BM25.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use knowledge::{DocKind, EmbedError, EmbedKind, Embedder, FakeEmbedder, KnowledgeBase, KnowledgeConfig, NewDocument, SearchRequest, Section, testutil};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
}

fn doc(key: &str, kind: DocKind, title: &str, sections: &[(Option<&str>, &str)], assets: &[&str], age_days: i64) -> NewDocument {
    NewDocument {
        doc_key: key.into(),
        kind,
        subtype: (kind == DocKind::Filing).then(|| "S-1".to_string()),
        source: if kind == DocKind::Filing { "sec".into() } else { "yahoo".into() },
        publisher: None,
        title: title.into(),
        url: Some(format!("https://example.test/{key}")),
        issuer: None,
        language: Some("en".into()),
        published_at: now() - Duration::days(age_days),
        assets: assets.iter().map(|s| s.to_string()).collect(),
        sections: sections.iter().map(|(h, t)| Section { heading: h.map(str::to_string), text: (*t).to_string() }).collect(),
        meta: serde_json::json!({}),
    }
}

fn req(q: &str, k: usize) -> SearchRequest {
    SearchRequest { now: Some(now()), k, ..SearchRequest::new(q) }
}

struct Down;
#[async_trait]
impl Embedder for Down {
    fn model_id(&self) -> String {
        FakeEmbedder::default().model_id()
    }
    async fn embed(&self, _: &[String], _: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unavailable("apagado".into()))
    }
}

async fn kb_with(cfg: KnowledgeConfig, embedder: Arc<dyn Embedder>) -> Arc<KnowledgeBase> {
    KnowledgeBase::open(testutil::memory_pool().await, embedder, cfg).await.unwrap()
}

/// Palabras de relleno distintas (`w0 w1 …`): diluyen el vector de un fragmento largo, que es justo
/// donde un embedding flojea con un identificador suelto.
fn filler(from: usize, n: usize) -> String {
    (from..from + n).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")
}

/// 150 noticias cortas de relleno + una noticia larga con el ticker BRK.B + un folleto largo con «Item 1A».
async fn corpus(kb: &KnowledgeBase) {
    for i in 0..150 {
        let t = format!("Market wrap {i} {}", filler(1000 + i * 3, 9));
        kb.ingest(doc(&format!("n:{i}"), DocKind::News, &t, &[(None, &t)], &[], i as i64 % 5)).await.unwrap();
    }
    let brk = format!("Berkshire Hathaway (BRK.B) adds to its stake after the quarterly filing. {}", filler(0, 110));
    kb.ingest(doc("n:brk", DocKind::News, "Berkshire adds to its stake", &[(None, &brk)], &["stock:BRK-B"], 3)).await.unwrap();
    let risk = format!("We depend on a single supplier for actuators and any disruption would harm our business. {}", filler(200, 110));
    kb.ingest(doc(
        "f:1",
        DocKind::Filing,
        "Acme Robotics S-1",
        &[(Some("ITEM 1A. RISK FACTORS"), &risk), (Some("USE OF PROCEEDS"), "We intend to use the net proceeds for working capital.")],
        &["stock:ACME"],
        10,
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn exact_identifiers_are_found_only_with_the_lexical_part() {
    let vec_only = kb_with(KnowledgeConfig { hybrid: false, ..KnowledgeConfig::default() }, Arc::new(FakeEmbedder::default())).await;
    let hybrid = kb_with(KnowledgeConfig::default(), Arc::new(FakeEmbedder::default())).await;
    corpus(&vec_only).await;
    corpus(&hybrid).await;
    for (q, key) in [("BRK.B", "n:brk"), ("Item 1A", "f:1")] {
        let v = vec_only.search(&req(q, 3)).await.unwrap();
        if q == "BRK.B" {
            // Un ticker suelto en un fragmento largo: el vector simulado lo diluye y no lo recupera.
            assert!(!v.hits.iter().any(|h| h.doc.doc_key == key), "el embedder simulado no entiende «{q}»: {:?}", v.hits.iter().map(|h| &h.doc.doc_key).collect::<Vec<_>>());
        }
        let h = hybrid.search(&req(q, 3)).await.unwrap();
        assert_eq!(h.hits[0].doc.doc_key, key, "«{q}» debe ser el primero con la híbrida");
        assert!(h.hits[0].lexical);
    }
    assert!(hybrid.stats().await.unwrap().lexical);
}

#[tokio::test]
async fn hostile_queries_never_break_match_syntax() {
    let kb = kb_with(KnowledgeConfig::default(), Arc::new(FakeEmbedder::default())).await;
    corpus(&kb).await;
    for q in [
        "\"", "\"\"\"", "foo\" OR \"bar", "NEAR(a b)", "a AND NOT b", "x*", "^y", "col:z", "(-w)", "title:\"Acme\" -supplier", "BRK.B\"; DROP TABLE chunks; --", "'", "\\", "*** ---", &"palabra ".repeat(500),
        "ünïcödé ñ 日本語",
    ] {
        match kb.search(&req(q, 3)).await {
            Ok(_) => {}
            // Una consulta sin términos útiles solo puede fallar como consulta vacía/sin términos, no con un error de SQLite.
            Err(e) => assert!(!e.to_string().to_lowercase().contains("fts5") && !e.to_string().contains("syntax"), "{q:?}: {e}"),
        }
    }
    // La tabla de verdad sigue intacta.
    assert!(kb.stats().await.unwrap().chunks >= 150);
    let r = kb.search(&req("foo\" OR \"supplier", 3)).await.unwrap();
    assert!(r.hits.iter().any(|h| h.doc.doc_key == "f:1"));
}

#[tokio::test]
async fn lexical_part_works_without_embedder_and_for_unembedded_chunks() {
    let kb = kb_with(KnowledgeConfig::default(), Arc::new(Down)).await;
    let r = kb.ingest(doc("n:brk", DocKind::News, "Berkshire adds to its stake", &[(None, "Berkshire Hathaway (BRK.B) adds to its stake.")], &["stock:BRK-B"], 1)).await.unwrap();
    assert_eq!((r.embedded, r.pending), (0, 1));
    let hits = kb.search(&req("BRK.B", 3)).await.unwrap().hits;
    assert_eq!(hits.len(), 1, "sin embedder ni vector se recupera por BM25");
    assert!(hits[0].lexical && hits[0].similarity == 0.0);
    // Los filtros siguen valiendo para la parte léxica.
    let other = SearchRequest { assets: vec!["stock:MSFT".into()], ..req("BRK.B", 3) };
    assert!(kb.search(&other).await.unwrap().hits.is_empty());
    let own = SearchRequest { assets: vec!["stock:BRK-B".into()], kinds: vec![DocKind::News], ..req("BRK.B", 3) };
    assert_eq!(kb.search(&own).await.unwrap().hits.len(), 1);
    let wrong_kind = SearchRequest { kinds: vec![DocKind::Filing], ..req("BRK.B", 3) };
    assert!(kb.search(&wrong_kind).await.unwrap().hits.is_empty());
    // Una consulta sin términos útiles y sin embedder sigue fallando en voz alta.
    assert!(kb.search(&req("the of and", 3)).await.is_err());
}

#[tokio::test]
async fn degrades_to_vectors_when_fts5_is_unusable() {
    let pool = testutil::memory_pool().await;
    // Simula un FTS5 roto: ya existe una tabla NORMAL con ese nombre, así que «CREATE VIRTUAL TABLE IF NOT EXISTS» no hace
    // nada y la sonda MATCH falla.
    sqlx::query("CREATE TABLE chunks_fts (x TEXT)").execute(&pool).await.unwrap();
    let kb = KnowledgeBase::open(pool, Arc::new(FakeEmbedder::default()), KnowledgeConfig::default()).await.expect("el arranque no se rompe");
    assert!(!kb.lexical_enabled());
    corpus(&kb).await;
    let s = kb.stats().await.unwrap();
    assert!(!s.lexical && s.chunks >= 150);
    let r = kb.search(&req("single supplier actuators", 3)).await.unwrap();
    assert_eq!(r.hits.len(), 3, "sigue buscando solo con vectores");
    assert!(r.hits.iter().all(|h| !h.lexical));
    assert_eq!(kb.backfill_lexical().await.unwrap(), 0);
}

#[tokio::test]
async fn fts_failure_while_ingesting_does_not_lose_the_document() {
    let kb = kb_with(KnowledgeConfig::default(), Arc::new(FakeEmbedder::default())).await;
    assert!(kb.lexical_enabled());
    // Rompe la tabla a mitad de sesión.
    sqlx::query("DROP TABLE chunks_fts").execute(kb_pool(&kb)).await.unwrap();
    let r = kb.ingest(doc("n:1", DocKind::News, "Gold hits record high", &[(None, "Gold hits record high as the dollar weakens.")], &[], 0)).await.unwrap();
    assert_eq!(r.chunks, 1);
    assert!(!kb.lexical_enabled(), "se degrada a solo vectores");
    assert_eq!(kb.search(&req("gold record", 3)).await.unwrap().hits.len(), 1);
}

fn kb_pool(kb: &KnowledgeBase) -> &sqlx::SqlitePool {
    kb.pool_for_tests()
}

#[tokio::test]
async fn delete_update_and_purge_keep_the_lexical_index_coherent() {
    let cfg = KnowledgeConfig { retention_news_days: 30, ..KnowledgeConfig::default() };
    let kb = kb_with(cfg, Arc::new(FakeEmbedder::default())).await;
    kb.ingest(doc("n:a", DocKind::News, "Copper rallies", &[(None, "Copper rallies on supply cuts at Zanzibarium mines.")], &[], 1)).await.unwrap();
    kb.ingest(doc("n:old", DocKind::News, "Ancient wheat story", &[(None, "Wheat story about Quuxland harvest.")], &[], 60)).await.unwrap();
    kb.ingest(doc("n:b", DocKind::News, "Oil slides", &[(None, "Oil slides as Frobnia raises output.")], &[], 1)).await.unwrap();
    let total = |kb: Arc<KnowledgeBase>| async move { kb.stats().await.unwrap() };
    let s = total(kb.clone()).await;
    assert_eq!((s.chunks, s.lexical_indexed), (3, 3));
    // Purga por antigüedad: el fragmento viejo desaparece también del índice léxico.
    assert_eq!(kb.purge(now()).await.unwrap().documents_by_age, 1);
    assert!(kb.search(&req("Quuxland", 3)).await.unwrap().hits.iter().all(|h| h.doc.doc_key != "n:old"));
    assert_eq!(total(kb.clone()).await.lexical_indexed, 2);
    // Borrado explícito.
    assert!(kb.delete_by_key("n:a").await.unwrap());
    assert!(!kb.search(&req("Zanzibarium", 3)).await.unwrap().hits.iter().any(|h| h.doc.doc_key == "n:a"));
    assert_eq!(total(kb.clone()).await.lexical_indexed, 1);
    // Actualización (misma clave, otro contenido): lo viejo sale, lo nuevo entra.
    kb.ingest(doc("n:b", DocKind::News, "Oil slides again", &[(None, "Oil slides again as Plinthia cuts output.")], &[], 1)).await.unwrap();
    assert!(!kb.search(&req("Frobnia", 3)).await.unwrap().hits.iter().any(|h| h.doc.doc_key == "n:b" && h.lexical));
    assert!(kb.search(&req("Plinthia", 3)).await.unwrap().hits.iter().any(|h| h.doc.doc_key == "n:b" && h.lexical));
    let s = total(kb.clone()).await;
    assert_eq!((s.chunks, s.lexical_indexed), (1, 1));
}

#[tokio::test]
async fn existing_chunks_are_backfilled_the_first_time() {
    let dir = std::env::temp_dir().join(format!("knowledge-hybrid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("k.db");
    {
        // Archivo creado sin búsqueda léxica (RAG_HYBRID=0): no existe la tabla FTS5.
        let kb = KnowledgeBase::open(testutil::file_pool(&path).await, Arc::new(FakeEmbedder::default()), KnowledgeConfig { hybrid: false, ..KnowledgeConfig::default() }).await.unwrap();
        corpus(&kb).await;
        assert!(!kb.stats().await.unwrap().lexical);
    }
    let kb = KnowledgeBase::open(testutil::file_pool(&path).await, Arc::new(FakeEmbedder::default()), KnowledgeConfig::default()).await.unwrap();
    // El relleno en segundo plano y esta llamada son idempotentes y se coordinan con la puerta de escritura.
    kb.backfill_lexical().await.unwrap();
    let s = kb.stats().await.unwrap();
    assert!(s.lexical && s.lexical_indexed == s.chunks && s.chunks >= 150, "{s:?}");
    assert_eq!(kb.search(&req("BRK.B", 3)).await.unwrap().hits[0].doc.doc_key, "n:brk");
    assert_eq!(kb.backfill_lexical().await.unwrap(), 0, "ya está todo");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn hybrid_off_is_the_old_vector_only_behaviour() {
    let kb = kb_with(KnowledgeConfig { hybrid: false, ..KnowledgeConfig::default() }, Arc::new(FakeEmbedder::default())).await;
    corpus(&kb).await;
    let s = kb.stats().await.unwrap();
    assert!((s.lexical, s.lexical_indexed) == (false, 0));
    let r = kb.search(&req("BRK.B", 5)).await.unwrap();
    assert!(r.hits.iter().all(|h| !h.lexical));
}

#[tokio::test]
async fn config_reads_rag_hybrid() {
    let get = |v: &'static str| KnowledgeConfig::from_lookup(move |k| (k == "RAG_HYBRID").then(|| v.to_string())).hybrid;
    assert!(KnowledgeConfig::from_lookup(|_| None).hybrid, "activada por defecto");
    assert!(!get("0") && !get("false") && !get("off"));
    assert!(get("1"));
}
