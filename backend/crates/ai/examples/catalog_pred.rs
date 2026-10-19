//! Predicción de VRAM del catálogo de la app para unos modelos (num_ctx y factor de caché KV como en la app):
//!   (el tipo de caché KV se pasa como `LLM_KV_CACHE_TYPE`, igual que la app)
//!   LLM_KV_CACHE_TYPE=q8_0 cargo run --release -p ai --example catalog_pred -- 16384 qwen2.5:7b qwen3:8b qwen3.5:4b qwen3.5:9b
use ai::{AiConfig, catalog};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ctx: u32 = args[0].parse().unwrap();
    let kv = AiConfig::from_env().kv_cache_factor;
    for id in &args[1..] {
        match catalog::find(id) {
            Some(e) => {
                let r = catalog::entry_requirements(e, ctx, kv);
                println!(
                    "{}",
                    serde_json::json!({"model": id, "numCtx": ctx, "kvFactor": kv, "diskGb": r.disk_gb, "recommendedVramGb": r.recommended_vram_gb, "minVramGb": r.min_vram_gb, "minRamGb": r.min_ram_gb,
                        "kvBytesPerTokenFull": e.arch.kv_bytes_per_token_full, "kvBytesPerTokenSwa": e.arch.kv_bytes_per_token_swa, "slidingWindow": e.arch.sliding_window})
                );
            }
            None => println!("{}", serde_json::json!({"model": id, "error": "no está en el catálogo"})),
        }
    }
}
