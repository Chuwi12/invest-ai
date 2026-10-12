//! Análisis profundo de si invertir: pipeline de etapas sobre el snapshot en vivo y la
//! puntuación determinista.
//!
//! ```text
//!  snapshot ─▶ saneado ─▶ puntuación (Rust) ─▶ contexto <market_data> (presupuesto de tokens)
//!      ├─ quick:    [analista combinado] ───────────────────────────────▶ juez
//!      ├─ standard: [técnico][fundamental][sentimiento][macro/riesgo] ──▶ juez
//!      └─ deep:     [técnico][fundamental][sentimiento][macro/riesgo] ─▶ [abogado alcista][abogado bajista] ─▶ juez
//! ```
//!
//! Cada etapa devuelve JSON con esquema (`format` de Ollama). Entre etapa y etapa, Rust comprueba
//! cada cifra contra el contexto (`fidelity`) y descarta lo que no esté: el juez solo ve
//! afirmaciones verificadas, y el `Analysis` final lleva fuente y marca de tiempo en cada una.

mod prompts;
mod types;
mod verify;

use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
};

use chrono::{NaiveDate, Utc};
use domain::{
    Analysis, AnalysisDepth, AssetSnapshot, Catalyst, DomainError, Horizon, KeyLevels, ModelTier, Result,
    ScoreBreakdown, Verdict,
};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use self::{
    types::{Claim, DebateReport, JudgeOut, StageReport, debate_schema, judge_schema, stage_schema},
    verify::{Checked, Verifier, calibrate},
};
use crate::{
    config::AiConfig,
    context::{ContextOptions, build_context, NewsRegistry},
    lang::{Lang, tr},
    models::{ModelCaps, ModelManager},
    ollama::{ChatParams, OllamaClient, WireMessage},
    safety::sanitize_snapshot,
    scoring::{self, Scorecard, verdict_name},
    tokens::truncate_chars,
};

pub use prompts::{data_block, language_name, system_prompt};

/// Profundidad por defecto según el modelo activo: ligeros → etapas fusionadas; pesados → todas.
pub fn default_depth(caps: &ModelCaps) -> AnalysisDepth {
    match caps.tier {
        Some(ModelTier::Light) => AnalysisDepth::Quick,
        Some(ModelTier::Balanced) => AnalysisDepth::Standard,
        Some(ModelTier::Heavy | ModelTier::Max) => AnalysisDepth::Deep,
        None => match caps.effective_params_b() {
            Some(p) if p < 5.0 => AnalysisDepth::Quick,
            Some(p) if p > 14.0 => AnalysisDepth::Deep,
            _ => AnalysisDepth::Standard,
        },
    }
}

/// Errores internos de una llamada estructurada.
#[derive(Debug)]
enum CallError {
    /// Hay que abortar el análisis (Ollama caído, modelo inexistente, timeout…).
    Fatal(DomainError),
    /// El modelo respondió, pero no con un JSON válido ni tras la reparación.
    Invalid(String),
}

#[derive(Default)]
struct Usage {
    prompt: AtomicU64,
    completion: AtomicU64,
    calls: AtomicU64,
}

pub struct AnalysisEngine {
    client: OllamaClient,
    models: Arc<ModelManager>,
    cfg: AiConfig,
}

impl AnalysisEngine {
    pub fn new(client: OllamaClient, models: Arc<ModelManager>, cfg: AiConfig) -> Self {
        Self { client, models, cfg }
    }

    fn lang(&self) -> Lang {
        Lang::from_code(&self.cfg.lang_code())
    }

    /// Valor del parámetro `think` para este modelo: solo si tiene la capacidad; gpt-oss no puede
    /// apagarse (admite low/medium/high).
    pub(crate) fn think_param(&self, caps: &ModelCaps) -> Option<Value> {
        if !caps.thinking {
            return None;
        }
        if caps.id.starts_with("gpt-oss") {
            return Some(json!(if self.cfg.think { "medium" } else { "low" }));
        }
        Some(json!(self.cfg.think))
    }

    pub(crate) fn base_params(&self, caps: &ModelCaps, messages: Vec<WireMessage>, max_tokens: Option<u32>) -> ChatParams {
        let thinking_on = self.cfg.think || caps.id.starts_with("gpt-oss");
        ChatParams {
            model: caps.id.clone(),
            messages,
            num_ctx: Some(self.cfg.num_ctx),
            temperature: Some(self.cfg.temperature),
            // Con razonamiento el modelo gasta muchos tokens antes de la respuesta.
            num_predict: max_tokens.map(|m| if caps.thinking && thinking_on { m.saturating_mul(4) } else { m }),
            keep_alive: Some(self.cfg.keep_alive.clone()),
            think: self.think_param(caps),
            ..ChatParams::default()
        }
    }

    /// Llamada con salida estructurada: valida el JSON y, si falla, reintenta una vez pidiéndole
    /// al modelo que lo repare.
    async fn structured<T: DeserializeOwned>(
        &self,
        caps: &ModelCaps,
        system: &str,
        user: String,
        schema: &Value,
        max_tokens: u32,
        usage: &Usage,
    ) -> std::result::Result<T, CallError> {
        let mut messages = vec![WireMessage::system(system), WireMessage::user(user)];
        let mut last_err = String::new();
        for attempt in 0..2 {
            let mut params = self.base_params(caps, messages.clone(), Some(max_tokens));
            params.format = Some(schema.clone());
            let out = match self.client.chat(params.clone()).await {
                Ok(o) => o,
                // Modelo sin capacidad de razonamiento pese a lo que dijera /api/show.
                Err(DomainError::InvalidInput(m)) if params.think.is_some() && m.to_lowercase().contains("think") => {
                    params.think = None;
                    self.client.chat(params).await.map_err(CallError::Fatal)?
                }
                Err(e) => return Err(CallError::Fatal(e)),
            };
            usage.calls.fetch_add(1, Ordering::Relaxed);
            usage.prompt.fetch_add(u64::from(out.usage.prompt_tokens.unwrap_or(0)), Ordering::Relaxed);
            usage.completion.fetch_add(u64::from(out.usage.completion_tokens.unwrap_or(0)), Ordering::Relaxed);
            match parse_json::<T>(&out.content) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::debug!(attempt, error = %e, "JSON del modelo no válido");
                    last_err = e;
                    if attempt == 0 {
                        messages.push(WireMessage::assistant(truncate_chars(&out.content, 1500)));
                        messages.push(WireMessage::user(prompts::repair_request(&last_err)));
                    }
                }
            }
        }
        Err(CallError::Invalid(last_err))
    }

    /// Ejecuta el análisis completo.
    pub async fn analyze(&self, snapshot: &AssetSnapshot, depth: Option<AnalysisDepth>) -> Result<Analysis> {
        let model = self.models.active().await;
        let caps = self.models.caps(&model).await;
        let depth = depth.unwrap_or_else(|| default_depth(&caps));
        let lang = self.lang();
        let started = std::time::Instant::now();

        let snap = sanitize_snapshot(snapshot);
        let card = scoring::score(&snap, lang);
        let mut registry = NewsRegistry::new();
        // El contexto ocupa ~40 % de la ventana: el resto es para el system, las etapas previas
        // (resumen) y la respuesta.
        let max_tokens = (self.cfg.num_ctx as usize * 40 / 100).clamp(1200, 16_000);
        let built = build_context(&snap, &card, &ContextOptions { max_tokens, lang }, &mut registry);
        let lang_code = self.cfg.lang_code();
        let system = system_prompt(&lang_code);
        let prefix = data_block(&built.text);
        let mut v = Verifier::new(&built.text, &snap, &built.news, lang);
        let usage = Usage::default();
        let mut stage_notes: Vec<String> = Vec::new();

        // --- Etapas de análisis (independientes entre sí) ---
        let stages = stage_plan(depth);
        let par = self.cfg.parallel.max(1);
        let futs: Vec<BoxFuture<'_, (usize, std::result::Result<StageReport, CallError>)>> = stages
            .iter()
            .enumerate()
            .map(|(i, st)| {
                let user = format!("{prefix}{}", prompts::with_language(st.task(), &lang_code));
                let schema = stage_schema(st.combined(), if st.combined() { 8 } else { 5 });
                let (system, caps, usage) = (&system, &caps, &usage);
                async move { (i, self.structured::<StageReport>(caps, system, user, &schema, 900, usage).await) }.boxed()
            })
            .collect();
        let results: Vec<_> = stream::iter(futs).buffer_unordered(par).collect().await;
        let mut reports: Vec<Option<StageReport>> = (0..stages.len()).map(|_| None).collect();
        for (i, r) in results {
            match r {
                Ok(rep) => reports[i] = Some(rep),
                Err(CallError::Fatal(e)) => return Err(e),
                Err(CallError::Invalid(e)) => {
                    tracing::warn!(stage = ?stages[i], error = %e, "etapa sin JSON válido");
                    stage_notes.push(tr!(
                        lang,
                        "La etapa «{}» no produjo un resultado válido y se omitió.",
                        "Stage «{}» did not produce a valid result and was skipped.",
                        stages[i].label(lang)
                    ));
                }
            }
        }

        // Verificación de cada hallazgo y digest para el juez.
        let mut digest = String::new();
        let mut analyst_missing: Vec<String> = Vec::new();
        for (st, rep) in stages.iter().zip(&reports) {
            let Some(rep) = rep else { continue };
            let checked: Vec<Checked> = rep.findings.iter().filter_map(|c| v.claim(c)).collect();
            analyst_missing.extend(rep.missing.iter().map(|m| truncate_chars(m, 160)));
            digest.push_str(&render_stage(st.label(lang), rep, &checked, lang));
        }

        // --- Abogados del diablo (solo deep) ---
        let debated = depth == AnalysisDepth::Deep && !digest.is_empty();
        if debated {
            let tasks = [("bull", prompts::bull_task(&digest)), ("bear", prompts::bear_task(&digest))];
            let futs: Vec<BoxFuture<'_, (usize, std::result::Result<DebateReport, CallError>)>> = tasks
                .iter()
                .enumerate()
                .map(|(i, (_, task))| {
                    let user = format!("{prefix}{}", prompts::with_language(task.clone(), &lang_code));
                    let (system, caps, usage) = (&system, &caps, &usage);
                    async move { (i, self.structured::<DebateReport>(caps, system, user, &debate_schema(), 800, usage).await) }.boxed()
                })
                .collect();
            let results: Vec<_> = stream::iter(futs).buffer_unordered(par).collect().await;
            let mut sorted = results;
            sorted.sort_by_key(|(i, _)| *i);
            for (i, r) in sorted {
                let label = if i == 0 { tr!(lang, "Abogado alcista", "Bull advocate") } else { tr!(lang, "Abogado bajista", "Bear advocate") };
                match r {
                    Ok(rep) => {
                        let checked: Vec<Checked> = rep.arguments.iter().filter_map(|c| v.claim(c)).collect();
                        digest.push_str(&format!("\n## {label}\n"));
                        for c in &checked {
                            digest.push_str(&format!("- {}\n", c.plain));
                        }
                        if !rep.opponent_weakness.trim().is_empty() {
                            digest.push_str(&format!("- {}: {}\n", tr!(lang, "punto débil del contrario", "opponent's weak point"), truncate_chars(&rep.opponent_weakness, 220)));
                        }
                        if !rep.would_prove_me_wrong.trim().is_empty() {
                            digest.push_str(&format!("- {}: {}\n", tr!(lang, "me equivoco si", "I am wrong if"), truncate_chars(&rep.would_prove_me_wrong, 220)));
                        }
                    }
                    Err(CallError::Fatal(e)) => return Err(e),
                    Err(CallError::Invalid(_)) => stage_notes.push(tr!(
                        lang,
                        "La etapa «{}» no produjo un resultado válido y se omitió.",
                        "Stage «{}» did not produce a valid result and was skipped.",
                        label
                    )),
                }
            }
        }
        if digest.is_empty() {
            digest = tr!(lang, "(ninguna etapa previa produjo resultados; decide solo con los datos y la puntuación)", "(no previous stage produced results; decide from the data and the scores only)");
        }

        // --- Juez ---
        let judge_user = format!("{prefix}{}", prompts::with_language(prompts::judge_task(lang, &digest, card.verdict, debated), &lang_code));
        let judge_max = self.cfg.max_output_tokens.unwrap_or(2048).max(1500);
        let judged = self.structured::<JudgeOut>(&caps, &system, judge_user, &judge_schema(), judge_max, &usage).await;

        let (mut analysis, used_fallback) = match judged {
            Ok(j) => (assemble(j, &snap, &card, &mut v, lang), false),
            Err(CallError::Fatal(e)) => return Err(e),
            Err(CallError::Invalid(e)) => {
                tracing::warn!(error = %e, "el juez no produjo JSON válido: se usa la puntuación determinista");
                (fallback(&snap, &card, &mut v, lang), true)
            }
        };

        // --- Avisos y metadatos ---
        let mut warnings: Vec<String> = Vec::new();
        let push = |w: String, out: &mut Vec<String>| {
            if !w.trim().is_empty() && !out.contains(&w) {
                out.push(w);
            }
        };
        for m in &snap.missing {
            push(m.clone(), &mut warnings);
        }
        for g in &card.gaps {
            push(g.clone(), &mut warnings);
        }
        for m in analyst_missing.iter().chain(analysis.data_warnings.clone().iter()) {
            push(m.clone(), &mut warnings);
        }
        for w in v.warnings.iter().chain(&stage_notes).chain(&built.trimmed) {
            push(w.clone(), &mut warnings);
        }
        if used_fallback {
            push(
                tr!(lang, "El modelo no devolvió un JSON válido ni tras reintentarlo: se muestra el veredicto de la puntuación cuantitativa, sin análisis del LLM.", "The model did not return valid JSON even after a retry: showing the quantitative score's verdict, without LLM analysis."),
                &mut warnings,
            );
        }
        push(
            tr!(
                lang,
                "Método: modelo {}, profundidad {}, {} llamadas, {} tokens de entrada y {} de salida, {:.0} s.",
                "Method: model {}, depth {}, {} calls, {} input and {} output tokens, {:.0} s.",
                caps.id,
                depth_name(depth),
                usage.calls.load(Ordering::Relaxed),
                usage.prompt.load(Ordering::Relaxed),
                usage.completion.load(Ordering::Relaxed),
                started.elapsed().as_secs_f32()
            ),
            &mut warnings,
        );
        push(
            tr!(lang, "Análisis automatizado con un modelo de IA local: no es asesoramiento financiero. Puede contener errores.", "Automated analysis by a local AI model: not financial advice. It may contain errors."),
            &mut warnings,
        );
        analysis.data_warnings = warnings;
        analysis.sources = v.sources();
        analysis.model = caps.id.clone();
        analysis.asset = snap.summary.id.clone();
        analysis.price_at_analysis = snap.quote.as_ref().map(|q| q.price).filter(|p| p.is_finite());
        analysis.scores = card.scores.clone();
        analysis.id = uuid::Uuid::now_v7().to_string();
        analysis.generated_at = Utc::now();
        Ok(analysis)
    }
}

pub fn depth_name(d: AnalysisDepth) -> &'static str {
    match d {
        AnalysisDepth::Quick => "quick",
        AnalysisDepth::Standard => "standard",
        AnalysisDepth::Deep => "deep",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Technical,
    Fundamental,
    Sentiment,
    MacroRisk,
    Combined,
}

impl Stage {
    fn task(self) -> String {
        match self {
            Self::Technical => prompts::technical_task(),
            Self::Fundamental => prompts::fundamental_task(),
            Self::Sentiment => prompts::sentiment_task(),
            Self::MacroRisk => prompts::macro_task(),
            Self::Combined => prompts::combined_task(),
        }
    }
    fn combined(self) -> bool {
        self == Self::Combined
    }
    fn label(self, l: Lang) -> &'static str {
        match self {
            Self::Technical => l.t("Analista técnico", "Technical analyst"),
            Self::Fundamental => l.t("Analista fundamental", "Fundamental analyst"),
            Self::Sentiment => l.t("Sentimiento y noticias", "Sentiment and news"),
            Self::MacroRisk => l.t("Macro y riesgo", "Macro and risk"),
            Self::Combined => l.t("Analista combinado", "Combined analyst"),
        }
    }
}

fn stage_plan(depth: AnalysisDepth) -> Vec<Stage> {
    match depth {
        AnalysisDepth::Quick => vec![Stage::Combined],
        AnalysisDepth::Standard | AnalysisDepth::Deep => {
            vec![Stage::Technical, Stage::Fundamental, Stage::Sentiment, Stage::MacroRisk]
        }
    }
}

fn render_stage(label: &str, rep: &StageReport, checked: &[Checked], lang: Lang) -> String {
    let mut s = format!(
        "\n## {label} ({}: {}, {} {:.2})\n",
        lang.t("sesgo", "stance"),
        truncate_chars(&rep.stance, 12),
        lang.t("confianza", "confidence"),
        if rep.confidence.is_finite() { rep.confidence.clamp(0.0, 1.0) } else { 0.0 }
    );
    if checked.is_empty() {
        s.push_str(&format!("- ({})\n", lang.t("sin hallazgos verificados", "no verified findings")));
    }
    for c in checked {
        let area = if c.area.is_empty() { String::new() } else { format!("[{}] ", c.area) };
        s.push_str(&format!("- {area}{}\n", c.plain));
    }
    s
}

/// Convierte la salida del juez (ya sin confiar en ella) en un `Analysis` parcial: el llamador
/// completa id, fuentes, avisos, modelo y marcas de tiempo.
fn assemble(j: JudgeOut, snap: &AssetSnapshot, card: &Scorecard, v: &mut Verifier<'_>, lang: Lang) -> Analysis {
    let claims = |list: &[Claim], v: &mut Verifier<'_>| -> Vec<String> { list.iter().filter_map(|c| v.claim(c)).map(|c| c.text).collect() };
    let bull_case = claims(&j.bull_case, v);
    let bear_case = claims(&j.bear_case, v);
    let risks = claims(&j.risks, v);

    let summary = v.prose(&j.summary, lang.t("resumen", "summary"));
    let mut thesis = v.prose(&j.thesis, lang.t("tesis", "thesis"));
    if !j.uncertainty.trim().is_empty() {
        let u = v.prose(&j.uncertainty, lang.t("incertidumbre", "uncertainty"));
        thesis.push_str(&format!("\n\n{}: {u}", lang.t("Incertidumbre", "Uncertainty")));
    }
    let change: Vec<String> = j.would_change_if.iter().filter(|s| !s.trim().is_empty()).map(|s| v.prose(s, lang.t("qué cambiaría", "what would change"))).collect();
    if !change.is_empty() {
        thesis.push_str(&format!("\n\n{}:\n", lang.t("La conclusión cambiaría si", "The conclusion would change if")));
        thesis.push_str(&change.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n"));
    }

    let mut strategy_parts = Vec::new();
    for (label, text, field) in [
        (lang.t("Entrada", "Entry"), &j.entry_strategy, "entrada"),
        (lang.t("Salida", "Exit"), &j.exit_strategy, "salida"),
        (lang.t("Tamaño de posición (orientativo)", "Position size (indicative)"), &j.position_sizing, "tamaño"),
    ] {
        if !text.trim().is_empty() {
            strategy_parts.push(format!("{label}: {}", v.prose(text, field)));
        }
    }

    let catalysts = j
        .catalysts
        .iter()
        .filter(|c| !c.description.trim().is_empty())
        .map(|c| Catalyst {
            date: c.date.as_deref().and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok()),
            description: v.prose(&c.description, lang.t("catalizador", "catalyst")),
            impact: c.impact.filter(|i| i.is_finite()).map(|i| i.clamp(-1.0, 1.0)),
        })
        .collect();

    let key_levels = v.levels(&j.key_levels, j.verdict);
    let confidence = calibrate(j.confidence, card, j.verdict, v.dropped, v.marked);
    let missing: Vec<String> = j.missing_data.iter().map(|m| truncate_chars(m, 200)).collect();

    Analysis {
        id: String::new(),
        asset: snap.summary.id.clone(),
        verdict: j.verdict,
        confidence,
        horizon: j.horizon,
        scores: ScoreBreakdown::default(),
        summary,
        thesis,
        bull_case,
        bear_case,
        risks,
        catalysts,
        key_levels,
        strategy: (!strategy_parts.is_empty()).then(|| strategy_parts.join("\n")),
        data_warnings: missing,
        sources: vec![],
        price_at_analysis: None,
        model: String::new(),
        generated_at: Utc::now(),
    }
}

/// Análisis construido solo con la puntuación determinista (si el LLM no da JSON válido).
fn fallback(snap: &AssetSnapshot, card: &Scorecard, v: &mut Verifier<'_>, lang: Lang) -> Analysis {
    let reasons: Vec<String> = card.verdict_reasons.iter().take(4).cloned().collect();
    let summary = tr!(
        lang,
        "Veredicto cuantitativo provisional: {} (puntuación global {:.0}/100, confianza {:.2}). {}",
        "Provisional quantitative verdict: {} (overall score {:.0}/100, confidence {:.2}). {}",
        verdict_name(lang, card.verdict),
        card.scores.overall,
        card.confidence,
        reasons.join("; ")
    );
    Analysis {
        id: String::new(),
        asset: snap.summary.id.clone(),
        verdict: card.verdict,
        confidence: (card.confidence * 0.8 * 100.0).round() / 100.0,
        horizon: Horizon::Medium,
        scores: ScoreBreakdown::default(),
        thesis: summary.clone(),
        summary,
        bull_case: vec![],
        bear_case: vec![],
        risks: vec![],
        catalysts: vec![],
        key_levels: v.levels(&types::LevelsOut::default(), Verdict::Wait).into_levels_only(),
        strategy: None,
        data_warnings: vec![],
        sources: vec![],
        price_at_analysis: None,
        model: String::new(),
        generated_at: Utc::now(),
    }
}

trait LevelsOnly {
    fn into_levels_only(self) -> KeyLevels;
}
impl LevelsOnly for KeyLevels {
    /// Sin juez no se proponen entrada, stop ni objetivos: solo los niveles que están en los datos.
    fn into_levels_only(self) -> KeyLevels {
        KeyLevels { entry_zone: None, stop_loss: None, targets: vec![], ..self }
    }
}

/// Extrae y valida un JSON de la respuesta del modelo (tolera vallas ```json y texto alrededor).
pub(crate) fn parse_json<T: DeserializeOwned>(content: &str) -> std::result::Result<T, String> {
    let t = content.trim();
    if t.is_empty() {
        return Err("respuesta vacía".into());
    }
    let first = serde_json::from_str::<T>(t);
    if let Ok(v) = first {
        return Ok(v);
    }
    let err = first.err().map(|e| e.to_string()).unwrap_or_default();
    if let (Some(a), Some(b)) = (t.find('{'), t.rfind('}'))
        && a < b
        && let Ok(v) = serde_json::from_str::<T>(&t[a..=b])
    {
        return Ok(v);
    }
    Err(err)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
