//! Detección de hardware, solo informativa (la app no elige el modelo por el usuario).
//!
//! Sin dependencias pesadas: `/proc`, `nvidia-smi`, `rocm-smi`, sysfs y `df`. Cada fuente que
//! falle se ignora (el valor queda vacío), nunca se propaga un error. El resultado se cachea unos
//! segundos porque `/api/models` se consulta con frecuencia desde la UI.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use domain::{GpuInfo, HardwareInfo};
use serde_json::Value;
use tokio::{process::Command, sync::Mutex};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const CACHE_TTL: Duration = Duration::from_secs(5);
const CMD_TIMEOUT: Duration = Duration::from_secs(3);

pub struct HardwareProbe {
    models_dir: Option<PathBuf>,
    cache: Mutex<Option<(Instant, HardwareInfo)>>,
}

impl HardwareProbe {
    /// `models_dir`: directorio de modelos de Ollama; `None` = autodetectar.
    pub fn new(models_dir: Option<PathBuf>) -> Self {
        Self { models_dir, cache: Mutex::new(None) }
    }

    pub async fn get(&self) -> HardwareInfo {
        let mut guard = self.cache.lock().await;
        if let Some((at, info)) = guard.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return info.clone();
        }
        let info = self.detect().await;
        *guard = Some((Instant::now(), info.clone()));
        info
    }

    async fn detect(&self) -> HardwareInfo {
        let cpuinfo = tokio::fs::read_to_string("/proc/cpuinfo").await.unwrap_or_default();
        let meminfo = tokio::fs::read_to_string("/proc/meminfo").await.unwrap_or_default();
        let (cpu_model, cpu_threads) = parse_cpuinfo(&cpuinfo);
        let cpu_threads = if cpu_threads > 0 {
            cpu_threads
        } else {
            std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0)
        };
        let (total, avail) = parse_meminfo(&meminfo);
        let dir = self.models_dir.clone().or_else(default_models_dir);
        let (gpus, disk) = tokio::join!(detect_gpus(), async {
            match &dir {
                Some(d) => run("df", &["-B1", "--output=avail", &d.to_string_lossy()]).await.and_then(|o| parse_df(&o)),
                None => None,
            }
        });
        HardwareInfo {
            cpu_model,
            cpu_threads,
            ram_total_gb: gib(total),
            ram_available_gb: gib(avail),
            gpus,
            disk_free_gb: disk.map(gib),
        }
    }
}

fn gib(bytes: u64) -> f32 {
    ((bytes as f64 / GIB) * 100.0).round() as f32 / 100.0
}

/// Directorio de modelos de Ollama: `OLLAMA_MODELS`, el del usuario o el del servicio systemd.
fn default_models_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(d) = std::env::var("OLLAMA_MODELS") {
        candidates.push(PathBuf::from(d));
    }
    if let Ok(h) = std::env::var("HOME") {
        candidates.push(Path::new(&h).join(".ollama/models"));
    }
    candidates.push(PathBuf::from("/usr/share/ollama/.ollama/models"));
    candidates.push(PathBuf::from("/var/lib/ollama"));
    candidates.into_iter().find(|p| p.exists())
}

async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let fut = Command::new(cmd).args(args).kill_on_drop(true).output();
    let out = tokio::time::timeout(CMD_TIMEOUT, fut).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn detect_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    if let Some(out) = run(
        "nvidia-smi",
        &["--query-gpu=name,memory.total,memory.free", "--format=csv,noheader,nounits"],
    )
    .await
    {
        gpus.extend(parse_nvidia_csv(&out));
    }
    // nvidia-smi puede fallar con el driver a medio actualizar («Driver/library version mismatch»)
    // aunque la GPU funcione: se lee al menos el nombre de /proc y la VRAM queda como 0 (desconocida).
    if !gpus.iter().any(|g| g.vendor == "NVIDIA") {
        gpus.extend(nvidia_proc_gpus(Path::new("/proc/driver/nvidia/gpus")).await);
    }
    if let Some(out) = run("rocm-smi", &["--showproductname", "--showmeminfo", "vram", "--json"]).await {
        gpus.extend(parse_rocm_json(&out));
    }
    // Sysfs: cubre AMD sin rocm-smi y Intel; NVIDIA no expone VRAM por sysfs.
    let have_amd = gpus.iter().any(|g| g.vendor == "AMD");
    gpus.extend(sysfs_gpus(have_amd).await);
    gpus
}

/// GPUs NVIDIA según `/proc/driver/nvidia/gpus/*/information` (línea `Model:`). Sin VRAM: se
/// informa `vram_total_gb = 0` (desconocida) y `vram_free_gb = None`.
pub(crate) async fn nvidia_proc_gpus(dir: &Path) -> Vec<GpuInfo> {
    let mut out = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return out };
    let mut names = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Ok(info) = tokio::fs::read_to_string(e.path().join("information")).await
            && let Some(m) = info.lines().find_map(|l| l.strip_prefix("Model:"))
        {
            names.push(m.trim().to_string());
        }
    }
    names.sort();
    for name in names {
        out.push(GpuInfo { name, vendor: "NVIDIA".into(), vram_total_gb: 0.0, vram_free_gb: None });
    }
    out
}

async fn sysfs_gpus(skip_amd: bool) -> Vec<GpuInfo> {
    let mut out = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir("/sys/class/drm").await else { return out };
    let mut seen = std::collections::HashSet::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        // `card0`, `card1`… (no `card0-DP-1`).
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        let read = |f: &str| {
            let p = dev.join(f);
            async move { tokio::fs::read_to_string(p).await.ok().map(|s| s.trim().to_string()) }
        };
        let vendor = read("vendor").await.unwrap_or_default();
        let total: Option<u64> = read("mem_info_vram_total").await.and_then(|s| s.parse().ok());
        let used: Option<u64> = read("mem_info_vram_used").await.and_then(|s| s.parse().ok());
        let (vendor_name, is_amd) = match vendor.as_str() {
            "0x1002" => ("AMD", true),
            "0x8086" => ("Intel", false),
            _ => continue,
        };
        if is_amd && skip_amd {
            continue;
        }
        let Some(total) = total.filter(|t| *t > 0) else { continue };
        if !seen.insert(dev.canonicalize().unwrap_or(dev.clone())) {
            continue;
        }
        let product = read("product_name").await.unwrap_or_else(|| format!("{vendor_name} GPU ({name})"));
        out.push(GpuInfo {
            name: product,
            vendor: vendor_name.into(),
            vram_total_gb: gib(total),
            vram_free_gb: used.map(|u| gib(total.saturating_sub(u))),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Parsers (puros, con tests)
// ---------------------------------------------------------------------------------------------

/// (modelo de CPU, hilos lógicos).
pub(crate) fn parse_cpuinfo(s: &str) -> (String, u32) {
    let mut model = String::new();
    let mut threads = 0;
    for line in s.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim() {
            "model name" if model.is_empty() => model = v.trim().to_string(),
            "processor" => threads += 1,
            _ => {}
        }
    }
    (model, threads)
}

/// (total, disponible) en bytes.
pub(crate) fn parse_meminfo(s: &str) -> (u64, u64) {
    let kb = |key: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|r| r.trim_start_matches(':').split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
            .map_or(0, |n| n * 1024)
    };
    (kb("MemTotal"), kb("MemAvailable"))
}

/// `nvidia-smi --format=csv,noheader,nounits`: `NVIDIA GeForce RTX 4060 Laptop GPU, 8188, 1927` (MiB).
pub(crate) fn parse_nvidia_csv(s: &str) -> Vec<GpuInfo> {
    s.lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split(',').map(str::trim).collect();
            let [name, total, free] = parts[..] else { return None };
            let total: f64 = total.parse().ok()?;
            let free: Option<f64> = free.parse().ok();
            Some(GpuInfo {
                name: name.to_string(),
                vendor: "NVIDIA".into(),
                vram_total_gb: gib((total * 1024.0 * 1024.0) as u64),
                vram_free_gb: free.map(|f| gib((f * 1024.0 * 1024.0) as u64)),
            })
        })
        .collect()
}

/// `rocm-smi --json`: `{"card0": {"Card series": "...", "VRAM Total Memory (B)": "...", "VRAM Total Used Memory (B)": "..."}}`.
pub(crate) fn parse_rocm_json(s: &str) -> Vec<GpuInfo> {
    let Ok(Value::Object(cards)) = serde_json::from_str::<Value>(s) else { return vec![] };
    let num = |c: &Value, k: &str| c.get(k).and_then(|v| v.as_str()).and_then(|v| v.trim().parse::<u64>().ok());
    cards
        .iter()
        .filter(|(k, _)| k.starts_with("card"))
        .filter_map(|(k, c)| {
            let total = num(c, "VRAM Total Memory (B)").filter(|t| *t > 0)?;
            let used = num(c, "VRAM Total Used Memory (B)");
            let name = ["Card Series", "Card series", "Card Model"]
                .iter()
                .find_map(|n| c.get(*n).and_then(Value::as_str))
                .map_or_else(|| format!("AMD GPU ({k})"), str::to_string);
            Some(GpuInfo {
                name,
                vendor: "AMD".into(),
                vram_total_gb: gib(total),
                vram_free_gb: used.map(|u| gib(total.saturating_sub(u))),
            })
        })
        .collect()
}

/// Salida de `df -B1 --output=avail <dir>`: cabecera y un número.
pub(crate) fn parse_df(s: &str) -> Option<u64> {
    s.lines().nth(1)?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        let cpu = "processor\t: 0\nmodel name\t: AMD Ryzen 9 7940HS\nprocessor\t: 1\nmodel name\t: AMD Ryzen 9 7940HS\n";
        assert_eq!(parse_cpuinfo(cpu), ("AMD Ryzen 9 7940HS".to_string(), 2));
        let mem = "MemTotal:       32000000 kB\nMemFree:  100 kB\nMemAvailable:   16000000 kB\n";
        assert_eq!(parse_meminfo(mem), (32_000_000 * 1024, 16_000_000 * 1024));
        assert_eq!(parse_meminfo(""), (0, 0));
    }

    #[test]
    fn parses_nvidia_smi() {
        let g = parse_nvidia_csv("NVIDIA GeForce RTX 4060 Laptop GPU, 8188, 1927\n");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].vendor, "NVIDIA");
        assert!((g[0].vram_total_gb - 8.0).abs() < 0.01);
        assert!((g[0].vram_free_gb.unwrap() - 1.88).abs() < 0.01);
        assert!(parse_nvidia_csv("basura\n[N/A], x, y").is_empty());
    }

    #[test]
    fn parses_rocm_smi() {
        let j = r#"{"card0": {"Card Series": "Radeon RX 7900 XTX", "VRAM Total Memory (B)": "25753026560", "VRAM Total Used Memory (B)": "1073741824"}, "system": {"x": "1"}}"#;
        let g = parse_rocm_json(j);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].name, "Radeon RX 7900 XTX");
        assert!((g[0].vram_total_gb - 23.98).abs() < 0.01);
        assert!(parse_rocm_json("no json").is_empty());
    }

    #[tokio::test]
    async fn nvidia_without_smi_falls_back_to_proc_name_with_unknown_vram() {
        let dir = std::env::temp_dir().join(format!("ai-hw-{}", std::process::id()));
        let gpu = dir.join("0000:01:00.0");
        std::fs::create_dir_all(&gpu).unwrap();
        std::fs::write(gpu.join("information"), "Model: \t\t NVIDIA GeForce RTX 4060 Laptop GPU\nIRQ:   \t\t 185\n").unwrap();
        let g = nvidia_proc_gpus(&dir).await;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 4060 Laptop GPU");
        assert_eq!((g[0].vram_total_gb, g[0].vram_free_gb), (0.0, None));
        // Directorio inexistente (sin driver NVIDIA): lista vacía, sin error.
        assert!(nvidia_proc_gpus(Path::new("/no/existe")).await.is_empty());
        // Salida de error de nvidia-smi: no produce GPUs fantasma.
        assert!(parse_nvidia_csv("Failed to initialize NVML: Driver/library version mismatch\n").is_empty());
    }

    #[test]
    fn parses_df() {
        assert_eq!(parse_df("      Avail\n 123456789\n"), Some(123_456_789));
        assert_eq!(parse_df("Avail\n"), None);
    }

    #[tokio::test]
    async fn detects_something_on_this_machine_and_caches() {
        let p = HardwareProbe::new(None);
        let a = p.get().await;
        assert!(a.ram_total_gb > 0.0);
        assert!(a.cpu_threads > 0);
        let b = p.get().await;
        assert_eq!(a.ram_available_gb, b.ram_available_gb, "segunda lectura desde la caché");
    }
}
