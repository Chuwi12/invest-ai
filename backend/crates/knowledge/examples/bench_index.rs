//! Mide memoria y latencia del índice en memoria (CPU, sin Ollama ni GPU).
//! Uso: `cargo run --release -p knowledge --example bench_index -- [n_vectores] [dim]...`

use std::time::Instant;

use knowledge::{
    DocKind,
    index::{DocMeta, Filter, VectorIndex},
};

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let n = args.first().copied().unwrap_or(50_000);
    let dims: Vec<usize> = if args.len() > 1 { args[1..].to_vec() } else { vec![256, 768, 1024] };
    for dim in dims {
        let before = rss_kb();
        let mut ix = VectorIndex::new();
        let docs = n / 4;
        for d in 0..docs {
            ix.upsert_doc(DocMeta {
                id: d as i64,
                kind: if d % 10 == 0 { DocKind::Filing } else { DocKind::News },
                source: "bench".into(),
                issuer_key: None,
                published_ms: d as i64,
                assets: vec![format!("stock:T{}", d % 200)],
            });
        }
        let mut s: u64 = 42;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32 - 0.5
        };
        for i in 0..n {
            let mut v: Vec<f32> = (0..dim).map(|_| next()).collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= norm);
            ix.add(i as i64, (i % docs) as i64, i as u128, &v).unwrap();
        }
        let after = rss_kb();
        let q: Vec<f32> = {
            let mut v: Vec<f32> = (0..dim).map(|_| next()).collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= norm);
            v
        };
        let time = |f: &Filter| {
            let mut xs: Vec<f64> = (0..30)
                .map(|_| {
                    let t = Instant::now();
                    let (hits, scanned) = ix.search(&q, f, 30, None);
                    std::hint::black_box((&hits, scanned));
                    t.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            xs.sort_by(f64::total_cmp);
            (xs[xs.len() / 2], xs[xs.len() - 1])
        };
        let (p50, max) = time(&Filter::default());
        let asset = Filter { assets: vec!["stock:T7".into()], ..Default::default() };
        let (a50, _) = time(&asset);
        println!(
            "n={n} dim={dim}: vectores {:.1} MiB (teórico {:.1}), RSS +{:.1} MiB | top-30 sin filtro p50 {p50:.1} ms (máx {max:.1}) | filtro por activo p50 {a50:.2} ms",
            (n * dim * 4) as f64 / 1048576.0,
            (n * dim * 4) as f64 / 1048576.0,
            (after.saturating_sub(before)) as f64 / 1024.0,
        );
    }
}
