//! Mide la búsqueda léxica (FTS5/BM25) sobre un archivo sintético. Solo CPU: sin Ollama ni GPU.
//! Uso: `cargo run --release -p knowledge --example bench_lexical -- [fragmentos] [directorio]`
//!
//! Genera `fragmentos` fragmentos (~1,4 KB, vocabulario de 30 000 palabras con distribución tipo Zipf, 10 por documento),
//! SIN embeddings (embedder siempre caído), mide la creación del índice léxico (relleno), y la
//! latencia de consultas léxicas con y sin filtro de activo. Todos los bucles están acotados.

use std::{path::PathBuf, sync::Arc, time::Instant};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use knowledge::{DocKind, EmbedError, EmbedKind, Embedder, KnowledgeBase, KnowledgeConfig, NewDocument, SearchRequest, Section, testutil};

struct Down;
#[async_trait]
impl Embedder for Down {
    fn model_id(&self) -> String {
        "bench|down".into()
    }
    async fn embed(&self, _: &[String], _: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unavailable("sin embedder (benchmark léxico)".into()))
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
    }
}

const VOCAB: usize = 30_000;
const CHUNKS_PER_DOC: usize = 10;
const WORDS_PER_CHUNK: usize = 210;

fn word(i: usize) -> String {
    // Palabras pronunciables y distintas: sílabas a partir del índice.
    const S: [&str; 16] = ["ba", "ce", "di", "fo", "gu", "ha", "je", "ki", "lo", "mu", "na", "pe", "qui", "ro", "su", "te"];
    let (mut n, mut w) = (i + 17, String::new());
    for _ in 0..4 {
        w.push_str(S[n % 16]);
        n /= 16;
    }
    format!("{w}{}", i % 7)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let chunks: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(50_000).clamp(1000, 200_000);
    let dir: PathBuf = args.get(1).map_or_else(|| std::env::temp_dir().join(format!("bench-lexical-{}", std::process::id())), PathBuf::from);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("k.db");
    let _ = std::fs::remove_file(&path);
    let docs = chunks / CHUNKS_PER_DOC;
    let mut rng = Lcg(42);

    // 1) Archivo sin índice léxico (RAG_HYBRID=0): mide solo la creación del texto.
    let cfg_off = KnowledgeConfig { hybrid: false, max_chunks: 1_000_000, max_chunks_per_doc: 100, ..KnowledgeConfig::default() };
    let t = Instant::now();
    {
        let kb = KnowledgeBase::open(testutil::file_pool(&path).await, Arc::new(Down), cfg_off).await.unwrap();
        for d in 0..docs {
            let mut text = String::new();
            for c in 0..CHUNKS_PER_DOC {
                for _ in 0..WORDS_PER_CHUNK {
                    // Distribución sesgada: pocas palabras muy frecuentes, cola larga.
                    let r = rng.next();
                    text.push_str(&word(((r * r * r) * VOCAB as f64) as usize));
                    text.push(' ');
                }
                if c == 0 && d % 50 == 0 {
                    text.push_str(&format!("TKR{d} "));
                }
                text.push_str(". ");
            }
            let n = NewDocument {
                doc_key: format!("bench:{d}"),
                kind: DocKind::News,
                subtype: None,
                source: "bench".into(),
                publisher: None,
                title: format!("Documento sintético {d}"),
                url: None,
                issuer: None,
                language: Some("es".into()),
                published_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
                assets: vec![format!("stock:S{}", d % 200)],
                // Un documento = CHUNKS_PER_DOC secciones (el troceado respeta las secciones).
                sections: text.split(". ").filter(|s| !s.trim().is_empty()).map(|s| Section { heading: None, text: s.to_string() }).collect(),
                meta: serde_json::json!({}),
            };
            kb.ingest(n).await.unwrap();
        }
        let s = kb.stats().await.unwrap();
        println!("generado: {} documentos, {} fragmentos, {:.1} MiB de texto, en {:.1} s", s.documents, s.chunks, s.text_bytes as f64 / 1048576.0, t.elapsed().as_secs_f64());
    }
    let db_before = std::fs::metadata(&path).unwrap().len();

    // 2) Se abre con la búsqueda léxica: relleno del índice FTS5 (primera vez).
    let cfg = KnowledgeConfig { max_chunks: 1_000_000, ..KnowledgeConfig::default() };
    let t = Instant::now();
    let kb = KnowledgeBase::open(testutil::file_pool(&path).await, Arc::new(Down), cfg).await.unwrap();
    kb.backfill_lexical().await.unwrap();
    let fill = t.elapsed().as_secs_f64();
    let s = kb.stats().await.unwrap();
    let db_after = std::fs::metadata(&path).unwrap().len();
    println!("índice léxico: {} / {} fragmentos en {:.1} s; BD {:.0} MiB -> {:.0} MiB (+{:.0} MiB)", s.lexical_indexed, s.chunks, fill, db_before as f64 / 1048576.0, db_after as f64 / 1048576.0, (db_after - db_before) as f64 / 1048576.0);

    // 3) Latencia de búsqueda (solo léxica: el embedder está caído).
    let common = format!("{} {} {}", word(1), word(5), word(20));
    let natural = format!("{} {} {} {} {} {}", word(300), word(900), word(2500), word(70), word(4000), word(150));
    let cases: Vec<(String, String, Option<String>)> = vec![
        ("ticker raro (TKR100)".into(), "TKR100".into(), None),
        ("6 términos medios".into(), natural.clone(), None),
        ("3 términos muy frecuentes".into(), common.clone(), None),
        ("6 términos + filtro de activo".into(), natural.clone(), Some("stock:S7".into())),
        ("3 frecuentes + filtro de activo".into(), common.clone(), Some("stock:S7".into())),
    ];
    for (name, q, asset) in cases {
        let req = SearchRequest { k: 6, assets: asset.into_iter().collect(), ..SearchRequest::new(q) };
        let mut lat = Vec::new();
        let mut hits = 0;
        for _ in 0..30 {
            let t = Instant::now();
            let r = kb.search(&req).await.unwrap();
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            hits = r.hits.len();
        }
        println!("{name:34} p50 {:7.2} ms  p95 {:7.2} ms  máx {:7.2} ms  ({hits} resultados)", percentile(&mut lat.clone(), 0.5), percentile(&mut lat.clone(), 0.95), percentile(&mut lat, 1.0));
    }
    drop(kb);
    std::fs::remove_dir_all(&dir).ok();
}
