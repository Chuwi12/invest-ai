//! Comparación reproducible de modelos: ejecuta el pipeline de análisis sobre un `AssetSnapshot`
//! CONGELADO (JSON) con un modelo y una profundidad fijos, y guarda `Analysis` + métricas.
//!
//!   curl -s localhost:8774/api/assets/stock:AAPL/snapshot > aapl.json
//!   cargo run --release -p ai --example compare_models -- aapl.json qwen3:8b deep out.json
//!
//! `LLM_THINK=1` activa el razonamiento (por defecto apagado: ver `AiConfig::think`). El resto de
//! variables `LLM_*` se respetan (`num_ctx`, temperatura…). Captura los eventos `tracing` del
//! pipeline (reparaciones de JSON, reintentos de tesis genérica, correcciones de redacción) y
//! aplica el lint de castellano a todos los textos del resultado.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use ai::{AiConfig, OllamaAnalyst, analysis::lint, models::ACTIVE_MODEL_KEY};
use async_trait::async_trait;
use domain::{Analysis, AnalysisDepth, Analyst, AssetSnapshot, SettingsStore};
use serde_json::json;
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

#[derive(Default)]
struct Mem(Mutex<HashMap<String, String>>);

#[async_trait]
impl SettingsStore for Mem {
    async fn get(&self, k: &str) -> domain::Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(k).cloned())
    }
    async fn set(&self, k: &str, v: &str) -> domain::Result<()> {
        self.0.lock().unwrap().insert(k.into(), v.into());
        Ok(())
    }
}

/// Subscriber mínimo: guarda «nivel mensaje campos» de los eventos de este crate.
struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields(String);
impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        if f.name() == "message" {
            self.0 = format!("{v:?}{}", self.0);
        } else {
            self.0 = format!("{} {}={v:?}", self.0, f.name());
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        m.target().starts_with("ai")
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, e: &Event<'_>) {
        let mut f = Fields(String::new());
        e.record(&mut f);
        self.0.lock().unwrap().push(format!("{} {}", e.metadata().level(), f.0));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn texts(a: &Analysis) -> Vec<(&'static str, String)> {
    let mut v = vec![("summary", a.summary.clone()), ("thesis", a.thesis.clone())];
    for (k, xs) in [("bull", &a.bull_case), ("bear", &a.bear_case), ("risk", &a.risks)] {
        v.extend(xs.iter().map(|x| (k, x.clone())));
    }
    v.extend(a.catalysts.iter().map(|c| ("catalyst", c.description.clone())));
    if let Some(s) = &a.strategy {
        v.push(("strategy", s.clone()));
    }
    if let Some(w) = &a.would_change_if {
        v.extend(w.iter().map(|x| ("wouldChangeIf", x.clone())));
    }
    if let Some(s) = &a.scenarios {
        v.extend(s.iter().map(|x| ("scenario", x.description.clone())));
    }
    v
}

/// Textos redactados por el modelo (sin la cita de fuente ni las marcas ⚠ que añade Rust, ni las frases
/// calculadas en Rust de estrategia y escenarios).
fn model_texts(a: &Analysis) -> Vec<(String, String)> {
    let clean = |t: &str| {
        let mut t = t.replace('⚠', "");
        while let Some(i) = t.find(" (fuente:") {
            let end = t[i..].find(')').map_or(t.len(), |e| i + e + 1);
            t.replace_range(i..end, "");
        }
        t.trim().to_string()
    };
    let mut v = vec![("summary".to_string(), clean(&a.summary))];
    v.extend(a.thesis.split("\n\n").map(|p| ("thesis".to_string(), clean(p))));
    for (k, xs) in [("bull", &a.bull_case), ("bear", &a.bear_case), ("risk", &a.risks)] {
        v.extend(xs.iter().map(|x| (k.to_string(), clean(x))));
    }
    v.extend(a.catalysts.iter().map(|c| ("catalyst".to_string(), clean(&c.description))));
    if let Some(s) = &a.strategy {
        v.extend(s.lines().filter(|l| l.starts_with("Entrada:") || l.starts_with("Salida:")).map(|l| ("strategy".to_string(), clean(l))));
    }
    if let Some(w) = &a.would_change_if {
        v.extend(w.iter().map(|x| ("wouldChangeIf".to_string(), clean(x))));
    }
    if let Some(h) = &a.horizon_note {
        v.push(("horizonNote".to_string(), clean(h)));
    }
    if let Some(s) = &a.scenarios {
        // La primera frase («Precio = … (Binance, 14 oct 2026).») la escribe Rust.
        v.extend(s.iter().map(|x| ("scenario".to_string(), clean(x.description.split_once(". ").map_or("", |(_, rest)| rest)))));
    }
    v
}

fn issues_of(a: &Analysis) -> Vec<serde_json::Value> {
    texts(a)
        .into_iter()
        .flat_map(|(k, t)| lint::spanish_issues(&t).into_iter().map(move |i| json!({"field": k, "issue": i.what})))
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `relint <salida.json>...`: recalcula `spanishIssuesFinal` de resultados ya guardados.
    if args.first().map(String::as_str) == Some("relint") {
        for path in &args[1..] {
            let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let a: Analysis = serde_json::from_value(v["analysis"].clone())?;
            v["metrics"]["spanishIssuesFinal"] = issues_of(&a).into();
            std::fs::write(path, serde_json::to_string_pretty(&v)?)?;
        }
        return Ok(());
    }
    // `reverify <snapshot.json> <salida.json>...`: re-verifica las cifras y fechas de resultados ya
    // guardados con la verificación actual (valor + etiqueta + fechas) y resume los cambios.
    if args.first().map(String::as_str) == Some("reverify") {
        let snap: AssetSnapshot = serde_json::from_str(&std::fs::read_to_string(&args[1])?)?;
        let mut report = Vec::new();
        for path in &args[2..] {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let a: Analysis = serde_json::from_value(v["analysis"].clone())?;
            let rows = ai::analysis::audit_figures(&snap, "es", 16384, &model_texts(&a));
            report.push(json!({ "file": path, "rows": rows }));
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let [snap_path, model, depth, out] = args.as_slice() else {
        anyhow::bail!("uso: compare_models <snapshot.json> <modelo> <quick|standard|deep> <salida.json>");
    };
    let snap: AssetSnapshot = serde_json::from_str(&std::fs::read_to_string(snap_path)?)?;
    let depth: AnalysisDepth = depth.parse()?;
    let events = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(Capture(events.clone()))?;

    let mut cfg = AiConfig::from_env();
    cfg.model = model.clone();
    let settings = Arc::new(Mem::default());
    settings.set(ACTIVE_MODEL_KEY, model).await?;
    let think = cfg.think;
    let (num_ctx, temperature) = (cfg.num_ctx, cfg.temperature);
    let analyst = OllamaAnalyst::new(cfg, settings)?;

    let t = std::time::Instant::now();
    let a = analyst.analyze_with(&snap, Some(depth)).await?;
    let wall = t.elapsed().as_secs_f64();

    let issues = issues_of(&a);
    let events = events.lock().unwrap().clone();
    let count = |needle: &str| events.iter().filter(|e| e.contains(needle)).count();
    let metrics = json!({
        "model": model, "depth": format!("{depth:?}").to_lowercase(), "think": think, "numCtx": num_ctx, "temperature": temperature,
        "wallSeconds": wall,
        "jsonRepairs": count("JSON del modelo no válido"),
        "genericThesisRetries": count("tesis genérica"),
        "wordingFixCalls": count("redacción sospechosa"),
        "spanishIssuesFinal": issues,
        "events": events,
    });
    std::fs::write(out, serde_json::to_string_pretty(&json!({ "metrics": metrics, "analysis": a }))?)?;
    eprintln!("{model} {depth:?}: {wall:.1} s, {} llamadas -> {out}", a.llm_calls.unwrap_or(0));
    Ok(())
}
