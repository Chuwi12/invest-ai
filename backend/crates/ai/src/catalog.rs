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
//! ## Overhead medido (solo 4 modelos)
//!
//! El `overhead` de 0.6 GiB es una constante prudente. Medido con nvidia-smi (VRAM del modelo ya cargado
//! entero en GPU, restando la línea base sin modelo; `num_ctx` 16384, caché KV q8_0, Ollama 0.40.4/0.40.6,
//! RTX 4070 Laptop 8 GB, 16 y 19-oct-2026, `docs/notes/model-eval.md`) el overhead real fue
//! qwen2.5:7b 0.02, qwen3:8b −0.04, qwen3.5:9b 0.05 y qwen3.5:4b 0.63 GiB: la constante sobreestimaba
//! 0.55–0.68 GiB en tres de ellos y acertaba en el cuarto. Para esos CUATRO modelos se usa el valor medido
//! (redondeado hacia arriba a 0.05, ver `MEASURED_OVERHEAD_GIB`); el resto sigue con 0.6: no se extrapola
//! (el overhead depende de la arquitectura y no se midió en otras). El mismo valor se aplica a cualquier
//! `num_ctx` y tipo de caché KV: se asume que el overhead no depende de ellos (solo medido a 16384/q8_0).
//!
//! Es una estimación: no contempla KV cuantizada (`OLLAMA_KV_CACHE_TYPE`), flash-attention ni
//! lotes grandes. Los modelos con atención por ventana o recurrente (gemma3/4, gpt-oss, qwen3.5/3.6/3.8,
//! nemotron-h) cuentan solo las capas globales a `ctx` completo y las de ventana a `min(ctx, ventana)`;
//! `tools/gen_catalog.py` decide capa a capa (cabecera GGUF) cuáles guardan KV: las recurrentes/SSM y las
//! que comparten KV (gemma4 E2B/E4B) no suman. Quedan fuera, por pequeños y sin medir: el estado recurrente
//! (≈50 MiB en qwen3.5:4b, ≈150 MiB en un 27B) y el margen que llama.cpp añade a la ventana (`n_ubatch`).
//! `disk_bytes` suma todas las capas del manifest (pesos, proyector de visión y borrador MTP si lo hay).

use std::sync::OnceLock;

use domain::{ModelRequirements, ModelTier};
use serde::Deserialize;

use crate::ollama::ShowInfo;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const OVERHEAD_GIB: f64 = 0.6;
/// Overhead (GiB) MEDIDO para modelos concretos (ver la documentación del módulo). Solo estos cuatro.
const MEASURED_OVERHEAD_GIB: &[(&str, f64)] = &[("qwen2.5:7b", 0.05), ("qwen3:8b", 0.0), ("qwen3.5:4b", 0.65), ("qwen3.5:9b", 0.1)];

/// Overhead de un modelo del catálogo: el medido si existe, si no la constante prudente.
pub fn overhead_gib(id: &str) -> f64 {
    MEASURED_OVERHEAD_GIB.iter().find(|(m, _)| *m == id).map_or(OVERHEAD_GIB, |(_, o)| *o)
}
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
/// `kv_factor`: tamaño de la caché KV frente a f16 (`AiConfig::kv_cache_factor`).
pub fn requirements(disk_bytes: u64, kv: &KvProfile, active_fraction: f64, num_ctx: u32, kv_factor: f64) -> ModelRequirements {
    requirements_with_overhead(disk_bytes, kv, active_fraction, num_ctx, kv_factor, OVERHEAD_GIB)
}

fn requirements_with_overhead(disk_bytes: u64, kv: &KvProfile, active_fraction: f64, num_ctx: u32, kv_factor: f64, overhead: f64) -> ModelRequirements {
    let weights = disk_bytes as f64 / GIB;
    let ctx = f64::from(num_ctx);
    let swa_ctx = kv.sliding_window.map_or(ctx, |w| ctx.min(f64::from(w)));
    let kv_gib = kv_factor * (kv.kv_bytes_per_token_full as f64 * ctx + kv.kv_bytes_per_token_swa as f64 * swa_ctx) / GIB;
    ModelRequirements {
        min_vram_gb: round1(0.4 * weights + kv_gib + overhead),
        recommended_vram_gb: round1(weights + kv_gib + overhead),
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

pub fn entry_requirements(e: &CatalogEntry, num_ctx: u32, kv_factor: f64) -> ModelRequirements {
    requirements_with_overhead(e.disk_bytes, &e.arch, active_fraction(&e.parameters, e.active_parameters.as_deref()), num_ctx, kv_factor, overhead_gib(&e.id))
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
        let r = requirements(4_683_087_332, &kv, 1.0, 8192, 1.0);
        assert_eq!(r.recommended_vram_gb, 5.4); // 4.36 + 0.44 + 0.6
        assert_eq!(r.min_vram_gb, 2.8); // 1.74 + 0.44 + 0.6
        assert_eq!(r.min_ram_gb, 6.3); // 4.36 + 0.44 + 1.5
        assert_eq!(r.disk_gb, 4.4);
        assert!(r.cpu_only_ok);
    }

    #[test]
    fn a_quantised_kv_cache_lowers_the_vram_requirement() {
        // qwen3:8b (36 capas, 8 cabezas KV, dim 128 => 147456 B/token) con 16k de contexto: f16 da los
        // 7,7 GB medidos el 14-oct-2026 (7,8 GB en `ollama ps`, 12 % en CPU con 8 GB); con
        // OLLAMA_KV_CACHE_TYPE=q8_0 la caché pasa de 2,25 a ~1,2 GiB.
        let kv = KvProfile { sliding_window: None, kv_bytes_per_token_full: 147_456, kv_bytes_per_token_swa: 0 };
        let f16 = requirements(5_225_388_164, &kv, 1.0, 16_384, 1.0);
        let q8 = requirements(5_225_388_164, &kv, 1.0, 16_384, crate::config::kv_cache_factor("q8_0"));
        assert_eq!(f16.recommended_vram_gb, 7.7);
        assert_eq!(q8.recommended_vram_gb, 6.7);
        assert_eq!(f16.disk_gb, q8.disk_gb);
    }

    #[test]
    fn sliding_window_caps_swa_layers_and_moe_counts_only_active_experts() {
        let kv = KvProfile { sliding_window: Some(128), kv_bytes_per_token_full: 1000, kv_bytes_per_token_swa: 1000 };
        let a = requirements(1 << 30, &kv, 1.0, 8192, 1.0);
        let full = KvProfile { sliding_window: None, ..kv.clone() };
        let b = requirements(1 << 30, &full, 1.0, 8192, 1.0);
        assert!(a.recommended_vram_gb <= b.recommended_vram_gb);
        // 13 GiB de pesos con 3.6/21 activos => 2.2 GiB leídos por token => CPU viable.
        let moe = requirements(13 << 30, &kv, 3.6 / 21.0, 8192, 1.0);
        assert!(moe.cpu_only_ok);
        let dense = requirements(13 << 30, &kv, 1.0, 8192, 1.0);
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

    /// Etiquetas añadidas al regenerar el catálogo el 14-oct-2026 (verificadas contra el registro).
    const NEW_TAGS: &[&str] = &[
        "qwen3.5:0.8b",
        "qwen3.5:2b",
        "qwen3.5:4b",
        "qwen3.5:9b",
        "qwen3.5:27b",
        "qwen3.5:35b-a3b",
        "qwen3.5:122b-a10b",
        "qwen3.6:27b",
        "qwen3.6:35b-a3b",
        "qwen3.8:27b",
        "gemma4:e2b",
        "gemma4:e4b",
        "gemma4:12b",
        "gemma4:26b-a4b",
        "gemma4:31b",
        "muse-glimmer:30b",
        "nemotron-3.5-lightning:30b-a3b",
    ];

    fn vram(id: &str, ctx: u32, kv_factor: f64) -> ModelRequirements {
        entry_requirements(find(id).unwrap_or_else(|| panic!("{id} no está en el catálogo")), ctx, kv_factor)
    }

    #[test]
    fn new_tags_have_kv_profile_and_coherent_requirements() {
        for id in NEW_TAGS {
            let m = find(id).unwrap_or_else(|| panic!("{id} no está en el catálogo"));
            assert!(m.supports_tools && m.reasoning, "{id}: tools y razonamiento verificados en el registro");
            assert!(m.arch.kv_bytes_per_token_full > 0, "{id} sin perfil de KV");
            assert_eq!(m.arch.sliding_window.is_some(), m.arch.kv_bytes_per_token_swa > 0, "{id}: ventana y bytes SWA deben ir juntos");
            assert!(!m.description.trim().is_empty() && m.context_window >= 128_000, "{id}");
            let f16 = vram(id, 16_384, 1.0);
            // pesos (disk_gb) + KV + 0,6 de overhead; la RAM solo-CPU añade 1,5.
            assert!(f16.recommended_vram_gb >= f16.disk_gb + 0.5, "{id}: la VRAM recomendada no cubre los pesos");
            assert!(f16.min_vram_gb < f16.recommended_vram_gb && f16.recommended_vram_gb < f16.min_ram_gb, "{id}");
            // KV q8_0 ahorra VRAM (con 128k, para que el ahorro supere el redondeo a 0,1 GiB incluso en los híbridos).
            let (long16, long8) = (vram(id, 131_072, 1.0), vram(id, 131_072, crate::config::kv_cache_factor("q8_0")));
            assert!(long8.recommended_vram_gb < long16.recommended_vram_gb && long8.disk_gb == long16.disk_gb, "{id}");
            // El KV de 16k no puede ser mayor que los pesos en ninguno de estos modelos (sí lo sería sin tratar
            // las capas recurrentes/de ventana): detecta un perfil que cuente todas las capas como globales.
            let kv_gib = f64::from(f16.recommended_vram_gb - f16.disk_gb) - overhead_gib(id);
            assert!(kv_gib < f64::from(f16.disk_gb), "{id}: KV de 16k = {kv_gib:.2} GiB, desproporcionado");
        }
    }

    #[test]
    fn measured_overhead_applies_only_to_the_four_measured_models() {
        // Medido (nvidia-smi, modelo entero en GPU, 16384 + q8_0): 4.85, 6.02, 3.99 y 6.42 GiB.
        let q8 = crate::config::kv_cache_factor("q8_0");
        let measured: [(&str, f32, f64); 4] = [("qwen2.5:7b", 4.85, 0.05), ("qwen3:8b", 6.02, 0.0), ("qwen3.5:4b", 3.99, 0.65), ("qwen3.5:9b", 6.42, 0.1)];
        for (id, gib, over) in measured {
            assert_eq!(overhead_gib(id), over, "{id}");
            let r = vram(id, 16_384, q8);
            // Lo recomendado no queda por debajo de lo medido y la sobreestimación es de redondeo (< 0,2 GiB), no de 0,6.
            assert!(r.recommended_vram_gb >= gib - 0.05 && r.recommended_vram_gb - gib < 0.2, "{id}: {} frente a {gib} medidos", r.recommended_vram_gb);
        }
        // Resto del catálogo: sigue la constante prudente (sin extrapolar).
        for m in catalog().iter().filter(|m| !measured.iter().any(|(id, _, _)| *id == m.id)) {
            assert_eq!(overhead_gib(&m.id), OVERHEAD_GIB, "{}", m.id);
        }
        let kv = KvProfile { sliding_window: None, kv_bytes_per_token_full: 147_456, kv_bytes_per_token_swa: 0 };
        let generic = requirements(5_225_388_164, &kv, 1.0, 16_384, q8);
        assert_eq!(generic.recommended_vram_gb, 6.7, "la fórmula genérica no cambia");
        assert_eq!(vram("qwen3:8b", 16_384, q8).recommended_vram_gb, 6.1);
        // El mínimo baja lo mismo que la recomendada (mismo overhead).
        assert!(vram("qwen3:8b", 16_384, q8).min_vram_gb < generic.min_vram_gb);
    }

    #[test]
    fn qwen3_5_4b_needs_less_vram_than_qwen2_5_7b_at_every_context() {
        for ctx in [8_192, 16_384, 32_768, 131_072] {
            let (small, big) = (vram("qwen3.5:4b", ctx, 1.0), vram("qwen2.5:7b", ctx, 1.0));
            assert!(small.recommended_vram_gb < big.recommended_vram_gb, "ctx {ctx}: {small:?} vs {big:?}");
            assert!(small.min_vram_gb < big.min_vram_gb && small.min_ram_gb < big.min_ram_gb, "ctx {ctx}");
        }
        // Y la brecha crece con el contexto: 8 de 32 capas guardan KV (4 cabezas·512 B) frente a 28 de 28 en qwen2.5.
        let gap = |ctx| vram("qwen2.5:7b", ctx, 1.0).recommended_vram_gb - vram("qwen3.5:4b", ctx, 1.0).recommended_vram_gb;
        assert!(gap(131_072) > gap(8_192));
    }

    #[test]
    fn hybrid_models_grow_slower_with_context_than_dense_ones() {
        let slope = |id: &str| vram(id, 32_768, 1.0).recommended_vram_gb - vram(id, 8_192, 1.0).recommended_vram_gb;
        // qwen3:8b: 147456 B/token; qwen3.5:9b: 32768 B/token (8 capas de atención de 32).
        assert!(slope("qwen3.5:9b") < slope("qwen3:8b") / 3.0, "{} vs {}", slope("qwen3.5:9b"), slope("qwen3:8b"));
        assert!(slope("qwen3.5:27b") < slope("qwen3:32b") / 3.0);
        // MTP activa por defecto (qwen3.6/3.8): +1 capa de atención propia, algo más de KV que qwen3.5:27b.
        let (q35, q36, q38) = (find("qwen3.5:27b").unwrap(), find("qwen3.6:27b").unwrap(), find("qwen3.8:27b").unwrap());
        assert_eq!(q36.arch.kv_bytes_per_token_full, q35.arch.kv_bytes_per_token_full + 4096);
        assert_eq!(q38.arch.kv_bytes_per_token_full, q36.arch.kv_bytes_per_token_full);
    }

    #[test]
    fn qwen3_5_family_requirements_grow_with_size() {
        let order = ["qwen3.5:0.8b", "qwen3.5:2b", "qwen3.5:4b", "qwen3.5:9b", "qwen3.5:27b", "qwen3.5:35b-a3b", "qwen3.5:122b-a10b"];
        for w in order.windows(2) {
            let (a, b) = (vram(w[0], 16_384, 1.0), vram(w[1], 16_384, 1.0));
            assert!(a.recommended_vram_gb < b.recommended_vram_gb && a.disk_gb < b.disk_gb, "{} < {}", w[0], w[1]);
        }
        // 8 GB de VRAM (RTX 4070): el 9B con 16k cabe entero; el 27B no.
        assert!(vram("qwen3.5:9b", 16_384, 1.0).recommended_vram_gb < 8.0);
        assert!(vram("qwen3.5:27b", 16_384, 1.0).recommended_vram_gb > 16.0);
    }

    #[test]
    fn new_moe_models_declare_active_parameters() {
        let moe = ["qwen3.5:35b-a3b", "qwen3.5:122b-a10b", "qwen3.6:35b-a3b", "gemma4:26b-a4b", "nemotron-3.5-lightning:30b-a3b"];
        for id in NEW_TAGS {
            let m = find(id).unwrap();
            assert_eq!(m.active_parameters.is_some(), moe.contains(id), "{id}");
            let f = active_fraction(&m.parameters, m.active_parameters.as_deref());
            assert!(if moe.contains(id) { f < 0.2 } else { f == 1.0 }, "{id}: fracción activa {f}");
        }
        // 35B-A3B: ~1,8 GiB leídos por token → viable solo con CPU; el denso de 27B no.
        assert!(vram("qwen3.5:35b-a3b", 16_384, 1.0).cpu_only_ok && !vram("qwen3.5:27b", 16_384, 1.0).cpu_only_ok);
    }

    #[test]
    fn gemma4_sliding_window_and_shared_kv_are_applied() {
        for id in ["gemma4:e2b", "gemma4:e4b", "gemma4:12b", "gemma4:26b-a4b", "gemma4:31b"] {
            let m = find(id).unwrap();
            assert!(m.arch.sliding_window.is_some() && m.arch.kv_bytes_per_token_swa > 0, "{id}");
            // Con 32k de contexto, contar las capas de ventana como globales daría más KV.
            let as_global = KvProfile { sliding_window: None, ..m.arch.clone() };
            let (win, glob) = (requirements(m.disk_bytes, &m.arch, 1.0, 32_768, 1.0), requirements(m.disk_bytes, &as_global, 1.0, 32_768, 1.0));
            assert!(win.recommended_vram_gb < glob.recommended_vram_gb, "{id}: {win:?} vs {glob:?}");
        }
        // E2B/E4B comparten KV entre las últimas capas: guardan KV 15 de 35 y 24 de 42 capas, no todas.
        let e4b = find("gemma4:e4b").unwrap();
        assert_eq!(e4b.arch.kv_bytes_per_token_full, 4 * 2 * (512 + 512) * 2);
        assert_eq!(e4b.arch.kv_bytes_per_token_swa, 20 * 2 * (256 + 256) * 2);
    }
}
