//! Calibración de `MIN_CLAIM_SIMILARITY` con el embedder REAL (Ollama con `embeddinggemma:300m-qat-q8_0` en 127.0.0.1:11434).
//! `cargo test -p knowledge --test live_citations -- --ignored --nocapture`
//!
//! `fixtures/claim_pairs.json`: pares reales del 19-nov-2026. `supported` = la frase cita el fragmento y este la respalda;
//! `filler` = frase de un análisis sin RAG emparejada con un fragmento de la misma empresa. `cos` = coseno medido entonces.

use std::sync::Arc;

use knowledge::{DEFAULT_EMBED_MODEL, KnowledgeBase, KnowledgeConfig, KnowledgeSearch, OllamaEmbedConfig, OllamaEmbedder, citation::MIN_CLAIM_SIMILARITY, testutil};
use serde::Deserialize;

#[derive(Deserialize)]
struct Pair {
    claim: String,
    fragment: String,
    cos: f32,
}

#[derive(Deserialize)]
struct Fixture {
    supported: Vec<Pair>,
    filler: Vec<Pair>,
}

/// Cuántos pares puntúan `>= MIN_CLAIM_SIMILARITY` con la ruta de producción (`KnowledgeBase::similarities`).
async fn accepted(kb: &KnowledgeBase, label: &str, pairs: &[Pair]) -> usize {
    let asked: Vec<(String, String)> = pairs.iter().map(|p| (p.claim.clone(), p.fragment.clone())).collect();
    let scores = kb.similarities(&asked).await.expect("Ollama con el modelo de embeddings (¿`ollama pull embeddinggemma:300m-qat-q8_0`?)");
    assert_eq!(scores.len(), pairs.len());
    for (p, s) in pairs.iter().zip(&scores) {
        println!("{label}: {s:.3} (medido {:.3}) {}", p.cos, p.claim.chars().take(70).collect::<String>());
        // Misma calibración: si esto se separa, los textos no se están embebiendo como la medición (p. ej. como `Document`).
        assert!((s - p.cos).abs() < 0.05, "{label}: {s} frente a {} medido", p.cos);
    }
    scores.iter().filter(|&&s| s >= MIN_CLAIM_SIMILARITY).count()
}

#[tokio::test]
#[ignore = "necesita Ollama con embeddinggemma"]
async fn the_threshold_keeps_real_citations_and_drops_filler_with_the_real_embedder() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/claim_pairs.json")).unwrap();
    assert_eq!((fixture.supported.len(), fixture.filler.len()), (14, 14));
    let embedder = OllamaEmbedder::new(OllamaEmbedConfig::new("http://127.0.0.1:11434", DEFAULT_EMBED_MODEL)).unwrap();
    let kb = KnowledgeBase::open(testutil::memory_pool().await, Arc::new(embedder), KnowledgeConfig::default()).await.unwrap();
    let supported = accepted(&kb, "supported", &fixture.supported).await;
    let filler = accepted(&kb, "filler", &fixture.filler).await;
    println!("aceptadas: {supported}/14 citas reales y {filler}/14 de relleno (umbral {MIN_CLAIM_SIMILARITY})");
    assert!(supported >= 12, "solo {supported} de 14 citas reales aceptadas");
    assert!(filler <= 3, "{filler} de 14 citas de relleno aceptadas");
}
