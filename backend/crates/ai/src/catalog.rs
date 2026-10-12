//! Catálogo curado de modelos (`models.json`, generado por `tools/gen_catalog.py` a partir del
//! registro de Ollama) y estimación de requisitos de hardware.
//!
//! ## Fórmula de requisitos (todo en GiB = 2^30 bytes)
//!
//! ```text
//! pesos     = tamaño del GGUF en disco (Ollama lo mapea en memoria tal cual)
//! kv        = (kv_full·ctx + kv_swa·min(ctx, ventana)) / 2^30      (caché f16)
//! kv_full   = capas_globales · cabezas_kv · (dim_k + dim_v) · 2 bytes   (por token)
//! kv_swa    = capas_ventana  · cabezas_kv · (dim_k + dim_v) · 2 bytes
//! overhead  = 0.6   (contexto CUDA/ROCm + buffers de cómputo)
//!
//! recommended_vram_gb = pesos + kv + overhead      (todo en GPU: velocidad máxima)
//! min_vram_gb         = 0.4·pesos + kv + overhead  (≥ 40 % de las capas en GPU; por debajo la
//!                                                    GPU apenas ayuda y el resto va a RAM)
//! min_ram_gb          = pesos + kv + 1.5           (ejecución solo-CPU: 1.5 = SO + runtime)
//! disk_gb             = pesos (+ manifest)
//! cpu_only_ok         = pesos · (activos/total) ≤ 6 GiB   (≈ lo que lee la CPU por token; con
//!                       ~50 GB/s de RAM da ≥ 5 tok/s. En un MoE solo cuentan los expertos activos)
//! ```
//!
//! Es una estimación: no contempla KV cuantizada (`OLLAMA_KV_CACHE_TYPE`), flash-attention ni
//! lotes grandes. Los modelos con atención por ventana o recurrente (gemma3, gpt-oss, qwen3.5)
//! cuentan solo las capas globales a `ctx` completo.

use std::sync::OnceLock;

use domain::{ModelRequirements, ModelTier};
use serde::Deserialize;

use crate::ollama::ShowInfo;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const OVERHEAD_GIB: f64 = 0.6;
const RAM_OVERHEAD_GIB: f64 = 1.5;
const CPU_ONLY_MAX_ACTIVE_GIB: f64 = 6.0;

static RAW: &str = include_str!("../models.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub family: String,
    pub parameters: String,
    pub active_parameters: Option<String>,
    pub quantization: String,
    pub context_window: u32,
    pub tier: ModelTier,
    pub supports_tools: bool,
    pub reasoning: bool,
    pub description: String,
    pub disk_bytes: u64,
    pub arch: KvProfile,
}

/// Lo necesario para estimar el KV cache.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvProfile {
    pub sliding_window: Option<u32>,
    pub kv_bytes_per_token_full: u64,
    pub kv_bytes_per_token_swa: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogFile {
    models: Vec<CatalogEntry>,
}

/// Catálogo embebido. Si el JSON estuviera corrupto (lo impide un test) devuelve uno vacío.
pub fn catalog() -> &'static [CatalogEntry] {
    static CAT: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    CAT.get_or_init(|| match serde_json::from_str::<CatalogFile>(RAW) {
        Ok(f) => f.models,
        Err(e) => {
            tracing::error!(error = %e, "models.json no es válido: catálogo vacío");
            Vec::new()
        }
    })
}

pub fn find(id: &str) -> Option<&'static CatalogEntry> {
    catalog().iter().find(|m| m.id == id)
}

/// «8.2B» → 8.2e9, «566M» → 5.66e8.
pub fn parse_params(s: &str) -> Option<f64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        'B' | 'b' => (&s[..s.len() - 1], 1e9),
        'M' | 'm' => (&s[..s.len() - 1], 1e6),
        'K' | 'k' => (&s[..s.len() - 1], 1e3),
        _ => (s, 1.0),
    };
    num.trim().parse::<f64>().ok().map(|n| n * mult)
}

fn round1(x: f64) -> f32 {
    ((x * 10.0).round() / 10.0) as f32
}

/// Aplica la fórmula del módulo. `active_fraction` ∈ (0, 1] (1 = denso).
pub fn requirements(disk_bytes: u64, kv: &KvProfile, active_fraction: f64, num_ctx: u32) -> ModelRequirements {
    let weights = disk_bytes as f64 / GIB;
    let ctx = f64::from(num_ctx);
    let swa_ctx = kv.sliding_window.map_or(ctx, |w| ctx.min(f64::from(w)));
    let kv_gib = (kv.kv_bytes_per_token_full as f64 * ctx + kv.kv_bytes_per_token_swa as f64 * swa_ctx) / GIB;
    ModelRequirements {
        min_vram_gb: round1(0.4 * weights + kv_gib + OVERHEAD_GIB),
        recommended_vram_gb: round1(weights + kv_gib + OVERHEAD_GIB),
        min_ram_gb: round1(weights + kv_gib + RAM_OVERHEAD_GIB),
        disk_gb: round1(weights),
        cpu_only_ok: weights * active_fraction.clamp(0.0, 1.0) <= CPU_ONLY_MAX_ACTIVE_GIB,
    }
}

pub fn active_fraction(total: &str, active: Option<&str>) -> f64 {
    match (parse_params(total), active.and_then(parse_params)) {
        (Some(t), Some(a)) if t > 0.0 => (a / t).min(1.0),
        _ => 1.0,
    }
}

pub fn entry_requirements(e: &CatalogEntry, num_ctx: u32) -> ModelRequirements {
    requirements(e.disk_bytes, &e.arch, active_fraction(&e.parameters, e.active_parameters.as_deref()), num_ctx)
}

/// Perfil de KV de un modelo no curado a partir de `/api/show` (atención global en todas las
/// capas: sobrestima los modelos con ventana, que es el lado seguro).
pub fn profile_from_show(show: &ShowInfo) -> Option<KvProfile> {
    let blocks = show.arch_u64("block_count")?;
    let heads = show.arch_u64("attention.head_count")?;
    let kv_heads = show.arch_u64("attention.head_count_kv").unwrap_or(heads);
    let key = show
        .arch_u64("attention.key_length")
        .or_else(|| show.arch_u64("embedding_length").map(|e| e / heads.max(1)))?;
    let val = show.arch_u64("attention.value_length").unwrap_or(key);
    Some(KvProfile {
        sliding_window: None,
        kv_bytes_per_token_full: blocks * kv_heads * (key + val) * 2,
        kv_bytes_per_token_swa: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid_and_complete() {
        let c = catalog();
        assert!(c.len() >= 15, "catálogo vacío o corrupto: {}", c.len());
        let mut ids = std::collections::HashSet::new();
        for m in c {
            assert!(ids.insert(&m.id), "id duplicado {}", m.id);
            assert!(m.supports_tools, "{} sin tools no debería estar", m.id);
            assert!(m.disk_bytes > 100_000_000, "{}", m.id);
            assert!(m.arch.kv_bytes_per_token_full > 0, "{}", m.id);
            assert!(m.id.contains(':'), "{} sin tag explícito", m.id);
        }
        for tier in [ModelTier::Light, ModelTier::Balanced, ModelTier::Heavy, ModelTier::Max] {
            assert!(c.iter().any(|m| m.tier == tier), "falta el nivel {tier:?}");
        }
        assert!(find("qwen2.5:7b").is_some() && find("gpt-oss:20b").is_some());
    }

    #[test]
    fn parses_parameter_counts() {
        assert!((parse_params("8.2B").unwrap() - 8.2e9).abs() < 1.0);
        assert!((parse_params("566M").unwrap() - 566e6).abs() < 1.0);
        assert_eq!(parse_params("raro"), None);
        assert!((active_fraction("30.5B", Some("3.3B")) - 0.108).abs() < 0.001);
        assert_eq!(active_fraction("8B", None), 1.0);
    }

    #[test]
    fn requirements_match_hand_calculation_for_qwen2_5_7b() {
        // qwen2.5:7b: 28 capas, 4 cabezas KV, dim 128 => 28·4·256·2 = 57344 B/token.
        // ctx 8192 => 0.4375 GiB. Pesos 4.68e9 B = 4.36 GiB.
        let kv = KvProfile { sliding_window: None, kv_bytes_per_token_full: 57_344, kv_bytes_per_token_swa: 0 };
        let r = requirements(4_683_087_332, &kv, 1.0, 8192);
        assert_eq!(r.recommended_vram_gb, 5.4); // 4.36 + 0.44 + 0.6
        assert_eq!(r.min_vram_gb, 2.8); // 1.74 + 0.44 + 0.6
        assert_eq!(r.min_ram_gb, 6.3); // 4.36 + 0.44 + 1.5
        assert_eq!(r.disk_gb, 4.4);
        assert!(r.cpu_only_ok);
    }

    #[test]
    fn sliding_window_caps_swa_layers_and_moe_counts_only_active_experts() {
        let kv = KvProfile { sliding_window: Some(128), kv_bytes_per_token_full: 1000, kv_bytes_per_token_swa: 1000 };
        let a = requirements(1 << 30, &kv, 1.0, 8192);
        let full = KvProfile { sliding_window: None, ..kv.clone() };
        let b = requirements(1 << 30, &full, 1.0, 8192);
        assert!(a.recommended_vram_gb <= b.recommended_vram_gb);
        // 13 GiB de pesos con 3.6/21 activos => 2.2 GiB leídos por token => CPU viable.
        let moe = requirements(13 << 30, &kv, 3.6 / 21.0, 8192);
        assert!(moe.cpu_only_ok);
        let dense = requirements(13 << 30, &kv, 1.0, 8192);
        assert!(!dense.cpu_only_ok);
    }

    #[test]
    fn profile_from_show_uses_model_info() {
        let show: ShowInfo = serde_json::from_value(serde_json::json!({
            "model_info": {"general.architecture": "llama", "llama.block_count": 32,
                "llama.attention.head_count": 32, "llama.attention.head_count_kv": 8,
                "llama.embedding_length": 4096}
        }))
        .unwrap();
        let p = profile_from_show(&show).unwrap();
        assert_eq!(p.kv_bytes_per_token_full, 32 * 8 * 256 * 2);
    }
}
