//! Análisis profundo de si invertir: pipeline de etapas sobre el snapshot en vivo y la
//! puntuación determinista.
//!
//! ```text
//!  snapshot ─▶ saneado ─▶ puntuación + niveles (Rust) ─▶ contexto por etapa (solo lo que necesita)
//!      ├─ quick:    [técnico][fundamental+noticias+riesgo] (cortas) ─────▶ juez breve ─▶ verificación
//!      ├─ standard: [técnico][fundamental][sentimiento][macro/riesgo] ───▶ juez ─▶ verificación
//!      └─ deep:     [técnico][fundamental][sentimiento][macro/riesgo] ─▶ [abogado alcista][abogado bajista] ─▶ juez ─▶ verificación
//! ```
//!
//! Cada etapa devuelve JSON con esquema (`format` de Ollama). Entre etapa y etapa, Rust comprueba
//! cada cifra contra el contexto (`fidelity`) y descarta lo que no esté: el juez solo ve
//! afirmaciones verificadas. Entrada, stop, objetivos, escenarios (precios) y tamaño de posición
//! los calcula Rust (`levels`), no el modelo. Tras el juez: tesis específica (si es genérica, un
//! reintento), verificación de cifras y revisión de redacción (`lint`; el modelo solo corrige si
//! hay algo sospechoso).

mod gaps;
pub(crate) mod levels;
pub mod lint;
mod prompts;
mod types;
mod verify;

use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{NaiveDate, Utc};
use knowledge::{Citations, KnowledgeSearch};
use domain::{
    Analysis, AnalysisDepth, AnalysisEvent, AssetSnapshot, Catalyst, DomainError, Horizon, ModelTier, Result,
    Scenario, ScoreBreakdown, Verdict,
};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use self::{
    levels::LevelPlan,
    types::{Claim, DebateReport, JudgeOut, Rewrite, StageReport, debate_schema, judge_schema, quick_judge_schema, rewrite_schema, stage_schema},
    verify::{Checked, Verifier, calibrate},
};
use crate::{
    config::AiConfig,
    context::{ContextOptions, NewsRegistry, Sections, build_context, build_context_sections},
    fidelity::{NumberIndex, Segment, extract_numbers},
    fmt,
    lang::{Lang, tr},
    models::{ModelCaps, ModelManager},
    ollama::{ChatParams, OllamaClient, WireMessage},
    safety::sanitize_snapshot,
    scoring::{self, Scorecard, verdict_name},
    tokens::truncate_chars,
};

pub use prompts::{data_block, language_name, system_prompt};

/// Profundidad por defecto según el modelo activo: ligeros → `quick` (2 etapas cortas); pesados → todas.
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

/// Ids estables de las etapas del progreso (contrato de `AnalysisEvent::Stage`).
fn steps(depth: AnalysisDepth) -> Vec<&'static str> {
    match depth {
        AnalysisDepth::Quick => vec!["snapshot", "technical", "fundamental", "judge", "verify"],
        AnalysisDepth::Standard => vec!["snapshot", "technical", "fundamental", "sentiment", "macro_risk", "judge", "verify"],
        AnalysisDepth::Deep => {
            vec!["snapshot", "technical", "fundamental", "sentiment", "macro_risk", "debate", "judge", "verify"]
        }
    }
}

fn step_label(id: &str, depth: AnalysisDepth, lang: Lang) -> &'static str {
    match id {
        "snapshot" => lang.t("Preparando datos, puntuación y niveles", "Preparing data, scores and levels"),
        "fundamental" if depth == AnalysisDepth::Quick => {
            lang.t("Fundamentales, noticias y riesgo", "Fundamentals, news and risk")
        }
        "technical" => lang.t("Análisis técnico", "Technical analysis"),
        "fundamental" => lang.t("Análisis fundamental y valoración", "Fundamental and valuation analysis"),
        "sentiment" => lang.t("Noticias y sentimiento", "News and sentiment"),
        "macro_risk" => lang.t("Entorno macro y riesgo", "Macro environment and risk"),
        "debate" => lang.t("Debate: caso alcista contra caso bajista", "Debate: bull case against bear case"),
        "judge" => lang.t("Veredicto del juez", "Judge's verdict"),
        _ => lang.t("Verificando cifras y redacción", "Verifying figures and wording"),
    }
}

/// Emisor de progreso (no hace nada si nadie escucha).
#[derive(Clone)]
struct Progress {
    tx: Option<UnboundedSender<AnalysisEvent>>,
    steps: Vec<&'static str>,
    depth: AnalysisDepth,
    lang: Lang,
}

impl Progress {
    fn at(&self, id: &str) {
        let Some(tx) = &self.tx else { return };
        let Some(pos) = self.steps.iter().position(|s| *s == id) else { return };
        let _ = tx.send(AnalysisEvent::Stage {
            stage: id.to_string(),
            label: step_label(id, self.depth, self.lang).to_string(),
            index: pos as u32 + 1,
            total: self.steps.len() as u32,
        });
    }
}

pub struct AnalysisEngine {
    client: OllamaClient,
    models: Arc<ModelManager>,
    cfg: AiConfig,
    /// Archivo de conocimiento local (RAG). `None` (por defecto, `RAG_ENABLED=0`): el análisis es
    /// idéntico al que no conoce el RAG; ver `crate::rag`.
    knowledge: Option<Arc<dyn KnowledgeSearch>>,
    /// Datos de redes sociales (opinión). `None` (por defecto, sin credenciales): el análisis es
    /// idéntico byte a byte al que no conoce las redes; ver `crate::social`.
    social: Option<Arc<dyn domain::SocialData>>,
}

/// `quick` no reintenta la tesis genérica: en el 7B el reintento (otra generación completa del juez)
/// era lo que se comía el ahorro de tiempo. Se mantiene la nota de calidad y el −0,05 de confianza.
/// Poner a `true` para la ablación «quick con reintento».
const QUICK_RETRY_GENERIC: bool = false;

/// Tope de salida del juez de `quick` (esperado ~600 tokens; holgado para no truncar el JSON, que
/// costaría una reparación).
const QUICK_JUDGE_MAX_TOKENS: u32 = 900;
/// Tope de salida de cada etapa de `quick` (2 hallazgos de ≤20 palabras ≈ 130–200 tokens).
const QUICK_STAGE_MAX_TOKENS: u32 = 300;

/// Cifras distintas mínimas que debe citar la tesis según la profundidad.
fn min_figures(depth: AnalysisDepth) -> usize {
    match depth {
        AnalysisDepth::Quick => 5,
        AnalysisDepth::Standard => 7,
        AnalysisDepth::Deep => 9,
    }
}

impl AnalysisEngine {
    pub fn new(client: OllamaClient, models: Arc<ModelManager>, cfg: AiConfig) -> Self {
        Self { client, models, cfg, knowledge: None, social: None }
    }

    /// Conecta el archivo de conocimiento (RAG). Con `None` no cambia nada.
    pub fn with_knowledge(mut self, knowledge: Option<Arc<dyn KnowledgeSearch>>) -> Self {
        self.knowledge = knowledge;
        self
    }

    /// Conecta los datos de redes sociales. Con `None` no cambia nada.
    pub fn with_social(mut self, social: Option<Arc<dyn domain::SocialData>>) -> Self {
        self.social = social;
        self
    }

    pub(crate) fn knowledge(&self) -> Option<Arc<dyn KnowledgeSearch>> {
        self.knowledge.clone()
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
            num_gpu: caps.num_gpu,
            ..ChatParams::default()
        }
    }

    /// Llamada con salida estructurada desde un system y un user.
    async fn structured<T: DeserializeOwned>(
        &self,
        caps: &ModelCaps,
        system: &str,
        user: String,
        schema: &Value,
        max_tokens: u32,
        usage: &Usage,
    ) -> std::result::Result<T, CallError> {
        let messages = vec![WireMessage::system(system), WireMessage::user(user)];
        self.structured_from(caps, messages, schema, max_tokens, usage).await.map(|(v, _)| v)
    }

    /// Llamada con salida estructurada: valida el JSON y, si falla, reintenta una vez pidiéndole
    /// al modelo que lo repare. Devuelve también el texto crudo de la respuesta válida.
    async fn structured_from<T: DeserializeOwned>(
        &self,
        caps: &ModelCaps,
        mut messages: Vec<WireMessage>,
        schema: &Value,
        max_tokens: u32,
        usage: &Usage,
    ) -> std::result::Result<(T, String), CallError> {
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
                Ok(v) => return Ok((v, out.content)),
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
        self.analyze_progress(snapshot, depth, None).await
    }

    /// Como `analyze`, enviando el progreso por `events` (si hay receptor).
    pub async fn analyze_progress(
        &self,
        snapshot: &AssetSnapshot,
        depth: Option<AnalysisDepth>,
        events: Option<UnboundedSender<AnalysisEvent>>,
    ) -> Result<Analysis> {
        let model = self.models.active().await;
        let caps = self.models.caps(&model).await;
        let depth = depth.unwrap_or_else(|| default_depth(&caps));
        let lang = self.lang();
        let started = std::time::Instant::now();
        let prog = Progress { tx: events, steps: steps(depth), depth, lang };

        // --- Datos, puntuación y niveles (todo en Rust) ---
        prog.at("snapshot");
        let snap = sanitize_snapshot(snapshot);
        let card = scoring::score(&snap, lang);
        let mut registry = NewsRegistry::new();
        // Cada contexto ocupa como mucho ~40 % de la ventana: el resto es para el system, las
        // etapas previas (resumen) y la respuesta.
        let max_tokens = (self.cfg.num_ctx as usize * 40 / 100).clamp(1200, 16_000);
        let opts = ContextOptions { max_tokens, lang };
        // Contexto completo: fija la numeración de noticias y es el índice de verificación.
        let built = build_context(&snap, &card, &opts, &mut registry);
        // Contexto de cada etapa. TODOS se construyen con el MISMO registro: si el completo tuvo que
        // reducirse, una etapa de solo noticias puede mostrar más (N9, N10…) y el verificador debe
        // conocerlas y poder comprobar sus cifras (R-11).
        let stages = stage_plan(depth);
        let mut ctx_for = |sec: &Sections| build_context_sections(&snap, &card, &opts, &mut registry, sec).text;
        let stage_texts: Vec<String> = stages
            .iter()
            .map(|st| match st {
                Stage::Technical | Stage::QuickTechnical => ctx_for(&Sections::technical()),
                Stage::Fundamental => ctx_for(&Sections::fundamental()),
                Stage::QuickFundamental => ctx_for(&quick_fundamental_sections()),
                Stage::Sentiment => ctx_for(&Sections::sentiment()),
                Stage::MacroRisk => ctx_for(&Sections::macro_risk()),
            })
            .collect();
        // Compacto (cotización + puntuaciones): lo demás ya está en el digest. Sin noticias.
        let judge_text = ctx_for(&Sections::judge());
        let lang_code = self.cfg.lang_code();
        let system = system_prompt(&lang_code);
        // RAG: una recuperación por activo, antes de las etapas y del juez. Un fallo no detiene el análisis.
        let (knowledge, rag_warning) = match &self.knowledge {
            Some(kb) => match crate::rag::retrieve_for_analysis(kb.as_ref(), &snap, depth).await {
                Ok(c) => (c, None),
                Err(e) => {
                    tracing::warn!(error = %e, "archivo de conocimiento no disponible: el análisis sigue sin él");
                    (Citations::default(), Some(crate::rag::unavailable_warning(lang)))
                }
            },
            None => (Citations::default(), None),
        };
        let system = if knowledge.is_empty() { system } else { format!("{system}\n{}", crate::rag::ANALYSIS_RULES) };
        // Redes sociales: opinión, en su propio bloque y SIN pasar por el NumberIndex. Un fallo o la
        // falta de datos dejan el prompt idéntico al de siempre (solo un aviso si falló).
        let (social_block, social_warning) = match &self.social {
            Some(s) => match tokio::time::timeout(std::time::Duration::from_secs(20), s.digest(&snap.summary)).await {
                Ok(Ok(Some(d))) if d.metrics.mentions_window > 0 => (Some(crate::social::render_block(&d, lang)), None),
                Ok(Ok(_)) => (None, None),
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "redes sociales no disponibles: el análisis sigue sin ellas");
                    (None, Some(crate::social::unavailable_warning(lang)))
                }
                Err(_) => {
                    tracing::warn!("redes sociales: tiempo de espera agotado; el análisis sigue sin ellas");
                    (None, Some(crate::social::unavailable_warning(lang)))
                }
            },
            None => (None, None),
        };
        let system = match &social_block {
            Some(_) => format!("{system}\n{}", crate::social::SOCIAL_RULES),
            None => system,
        };
        // Niveles: dos planes porque dependen del veredicto final (esperar ≠ comprar).
        let plan_now = levels::plan(&snap, Verdict::Accumulate, lang);
        let plan_wait = levels::plan(&snap, Verdict::Wait, lang);
        let levels_block = levels_block(plan_now.as_ref(), plan_wait.as_ref(), lang);

        let mut v = Verifier::new(&built.text, &snap, &registry, lang);
        for text in stage_texts.iter().chain([&judge_text]) {
            v.idx.add_context(text);
        }
        v.idx.add_text(&levels_block, Segment::Levels);
        // Una cifra citada de un fragmento recuperado cuenta como respaldada (solo el texto del
        // fragmento: ni fechas ni URL de los metadatos). Se indexa EXACTAMENTE lo que el modelo ve
        // (`rendered_texts`: saneado y recortado como en `<knowledge_base>`): la cola recortada del
        // fragmento no puede avalar una cifra que el modelo no tuvo delante.
        for text in knowledge.rendered_texts() {
            v.idx.add_text(&text, Segment::Other);
        }
        let usage = Usage::default();
        let mut stage_notes: Vec<String> = Vec::new();

        // --- Etapas de análisis (independientes entre sí, cada una con su contexto) ---
        let par = self.cfg.parallel.max(1);
        let futs: Vec<BoxFuture<'_, (usize, std::result::Result<StageReport, CallError>)>> = stages
            .iter()
            .enumerate()
            .map(|(i, st)| {
                let user = format!("{}{}", data_block(&stage_texts[i]), prompts::with_language(st.task(), &lang_code));
                let schema = stage_schema(st.max_findings());
                let (system, caps, usage, prog) = (&system, &caps, &usage, &prog);
                let id = st.id();
                async move {
                    prog.at(id);
                    (i, self.structured::<StageReport>(caps, system, user, &schema, st.max_tokens(), usage).await)
                }
                .boxed()
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
        for (st, rep) in stages.iter().zip(&reports) {
            let Some(rep) = rep else { continue };
            let checked: Vec<Checked> = rep.findings.iter().filter_map(|c| v.claim(c)).collect();
            digest.push_str(&render_stage(st.label(lang), rep, &checked, lang));
        }

        // --- Abogados del diablo (solo deep) ---
        let debated = depth == AnalysisDepth::Deep && !digest.is_empty();
        if debated {
            prog.at("debate");
            let tasks = [prompts::bull_task(&digest), prompts::bear_task(&digest)];
            let debate_ctx = &judge_text;
            let futs: Vec<BoxFuture<'_, (usize, std::result::Result<DebateReport, CallError>)>> = tasks
                .iter()
                .enumerate()
                .map(|(i, task)| {
                    let user = format!("{}{}", data_block(debate_ctx), prompts::with_language(task.clone(), &lang_code));
                    let (system, caps, usage) = (&system, &caps, &usage);
                    async move { (i, self.structured::<DebateReport>(caps, system, user, &debate_schema(), 450, usage).await) }.boxed()
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
        prog.at("judge");
        let deep = depth == AnalysisDepth::Deep;
        let quick = depth == AnalysisDepth::Quick;
        // quick: el juez no escribe estrategia ni usa los niveles (los calcula Rust y salen en `levelsNote`).
        let (judge_ctx, judge_task) = if quick {
            (judge_text.clone(), prompts::quick_judge_task(lang, &digest, card.verdict, &lang_code, &prompts::quick_hints(&snap, lang)))
        } else {
            (format!("{judge_text}\n{levels_block}"), prompts::judge_task(lang, &digest, card.verdict, debated, deep, &lang_code))
        };
        let knowledge_block = if knowledge.is_empty() { String::new() } else { format!("{}\n", knowledge.render(&lang_code)) };
        let social_block = social_block.map(|b| format!("{b}\n")).unwrap_or_default();
        let judge_user = format!("{}{knowledge_block}{social_block}{}", data_block(&judge_ctx), prompts::with_language(judge_task, &lang_code));
        let judge_max = if quick {
            self.cfg.max_output_tokens.map_or(QUICK_JUDGE_MAX_TOKENS, |m| m.min(QUICK_JUDGE_MAX_TOKENS))
        } else {
            self.cfg.max_output_tokens.unwrap_or(2048).max(if deep { 2600 } else { 1800 })
        };
        let schema = if quick { quick_judge_schema() } else { judge_schema(deep) };
        let msgs = vec![WireMessage::system(&system), WireMessage::user(judge_user)];
        let mut quality_notes: Vec<String> = Vec::new();
        let mut generic_penalty = 0usize;
        let mut judged = self.structured_from::<JudgeOut>(&caps, msgs.clone(), &schema, judge_max, &usage).await;
        if let Ok((j, raw)) = &judged {
            let min = min_figures(depth);
            let spec = lint::specificity(&j.full_thesis());
            if spec.is_generic(min) && quick && !QUICK_RETRY_GENERIC {
                tracing::info!(figures = spec.figures, "tesis genérica en quick: sin reintento");
                generic_penalty = 1;
                quality_notes.push(tr!(
                    lang,
                    "La tesis es poco específica (modo rápido, sin reintento): se redujo la confianza.",
                    "The thesis is not specific enough (quick mode, no retry): confidence was reduced."
                ));
            } else if spec.is_generic(min) {
                tracing::info!(figures = spec.figures, boilerplate = spec.boilerplate.len(), "tesis genérica: se pide reescribirla");
                let mut retry = msgs;
                retry.push(WireMessage::assistant(truncate_chars(raw, 7000)));
                retry.push(WireMessage::user(prompts::specificity_request(&spec.feedback(min), min)));
                match self.structured_from::<JudgeOut>(&caps, retry, &schema, judge_max, &usage).await {
                    Err(CallError::Fatal(e)) => return Err(e),
                    Err(CallError::Invalid(_)) => generic_penalty = 1,
                    Ok((j2, raw2)) => {
                        let spec2 = lint::specificity(&j2.full_thesis());
                        if spec2.figures >= spec.figures {
                            if spec2.is_generic(min) {
                                generic_penalty = 1;
                            }
                            judged = Ok((j2, raw2));
                        } else {
                            generic_penalty = 1;
                        }
                    }
                }
                if generic_penalty > 0 {
                    quality_notes.push(tr!(
                        lang,
                        "La tesis sigue siendo poco específica tras un reintento: se redujo la confianza.",
                        "The thesis is still not specific enough after a retry: confidence was reduced."
                    ));
                }
            }
        }

        // --- Verificación: redacción (solo llama al modelo si hay algo sospechoso) ---
        prog.at("verify");
        let judged = match judged {
            Ok((mut j, _)) => {
                if let Some(note) = self.fix_wording(&caps, &system, &lang_code, &mut j, &usage).await? {
                    quality_notes.push(note);
                }
                if !knowledge.is_empty() {
                    let mut issues = Vec::new();
                    for t in judge_texts_mut(&mut j) {
                        let (clean, found) = knowledge.clean(t);
                        *t = clean;
                        issues.extend(found);
                    }
                    quality_notes.extend(crate::rag::issues_note(lang, &issues));
                }
                Ok(j)
            }
            Err(e) => Err(e),
        };

        let (mut analysis, used_fallback) = match judged {
            Ok(j) => (assemble(j, &snap, &card, &mut v, lang, deep), false),
            Err(CallError::Fatal(e)) => return Err(e),
            Err(CallError::Invalid(e)) => {
                tracing::warn!(error = %e, "el juez no produjo JSON válido: se usa la puntuación determinista");
                (fallback(&snap, &card, &mut v, lang), true)
            }
        };
        v.dropped += generic_penalty;
        if generic_penalty > 0 && !used_fallback {
            analysis.confidence = (analysis.confidence - 0.05).clamp(0.05, 0.85);
        }

        // --- Niveles y tamaño de posición: Rust, con el veredicto definitivo ---
        let plan = levels::plan(&snap, analysis.verdict, lang);
        if let Some(plan) = &plan {
            analysis.key_levels = plan.key_levels(&snap);
            analysis.levels_note = Some(plan.note());
            analysis.position_sizing = levels::sizing(plan, &snap, &card, analysis.verdict, lang);
            if let Some(sc) = analysis.scenarios.as_mut() {
                price_scenarios(sc, plan, analysis_price(&snap), lang);
                quality_notes.extend(explain_scenarios(sc, plan, &snap, lang));
                analysis.expected_return_pct = expected_return(sc);
            }
        } else {
            analysis.levels_note = Some(tr!(
                lang,
                "Sin precio actual en los datos: no se calculan entrada, stop ni objetivos.",
                "No current price in the data: entry, stop and targets are not computed."
            ));
        }
        if let Some(ps) = &analysis.position_sizing {
            let line = format!("{}: {}", lang.t("Tamaño de posición (orientativo)", "Position size (indicative)"), ps.rationale);
            analysis.strategy = Some(match analysis.strategy.take() {
                Some(s) if !s.is_empty() => format!("{s}\n{line}"),
                _ => line,
            });
        }

        // --- Avisos y metadatos ---
        if self.client.take_gpu_fallback_notice(&caps.id).is_some() {
            quality_notes.push(tr!(
                lang,
                "Se pidió cargar todas las capas del modelo en la GPU y Ollama no tenía memoria suficiente: se usó el reparto automático (más lento). El ajuste se vuelve a probar en 30 minutos o al cambiarlo en Modelos.",
                "All of the model's layers were requested on the GPU but Ollama did not have enough memory: automatic placement was used (slower). The setting is retried after 30 minutes or when changed in Models."
            ));
        }
        let mut notes: Vec<String> = Vec::new();
        let citation_note = v.citation_note();
        for w in v.warnings.iter().chain(&stage_notes).chain(&built.trimmed).chain(&quality_notes).chain(citation_note.iter()) {
            if !w.trim().is_empty() && !notes.contains(w) {
                notes.push(w.clone());
            }
        }
        if used_fallback {
            notes.push(tr!(lang, "El modelo no devolvió un JSON válido ni tras reintentarlo: se muestra el veredicto de la puntuación cuantitativa, sin análisis del LLM.", "The model did not return valid JSON even after a retry: showing the quantitative score's verdict, without LLM analysis."));
        }
        analysis.data_warnings = gaps::real_gaps(&snap, &card, lang);
        analysis.data_warnings.extend(rag_warning);
        analysis.data_warnings.extend(social_warning);
        analysis.quality_notes = Some(notes);
        analysis.method = Some(tr!(
            lang,
            "Modelo {}, profundidad {}, {} llamadas, {} tokens de entrada y {} de salida, {:.0} s. Análisis automatizado con un modelo de IA local: no es asesoramiento financiero y puede contener errores.",
            "Model {}, depth {}, {} calls, {} input and {} output tokens, {:.0} s. Automated analysis by a local AI model: not financial advice, it may contain errors.",
            caps.id,
            depth_name(depth),
            usage.calls.load(Ordering::Relaxed),
            usage.prompt.load(Ordering::Relaxed),
            usage.completion.load(Ordering::Relaxed),
            started.elapsed().as_secs_f32()
        ));
        analysis.depth = Some(depth);
        analysis.duration_ms = Some(started.elapsed().as_millis() as u64);
        analysis.llm_calls = Some(usage.calls.load(Ordering::Relaxed) as u32);
        analysis.prompt_tokens = Some(usage.prompt.load(Ordering::Relaxed) as u32);
        analysis.completion_tokens = Some(usage.completion.load(Ordering::Relaxed) as u32);
        analysis.sources = v.sources();
        if !knowledge.is_empty() {
            // Solo las citas que sobrevivieron a la verificación y están en el texto final.
            let mut texts: Vec<&str> = vec![&analysis.summary, &analysis.thesis];
            texts.extend(analysis.bull_case.iter().chain(&analysis.bear_case).chain(&analysis.risks).map(String::as_str));
            texts.extend(analysis.strategy.iter().chain(analysis.horizon_note.iter()).map(String::as_str));
            texts.extend(analysis.would_change_if.iter().flatten().map(String::as_str));
            texts.extend(analysis.catalysts.iter().map(|c| c.description.as_str()));
            texts.extend(analysis.scenarios.iter().flatten().map(|s| s.description.as_str()));
            let mut tags: Vec<String> = texts.iter().flat_map(|t| knowledge::citation::extract_tags(t)).map(|n| format!("D{n}")).collect();
            tags.sort_by_key(|t| t[1..].parse::<usize>().unwrap_or(0));
            tags.dedup();
            for r in knowledge.source_refs(tags.iter().map(String::as_str)) {
                if !analysis.sources.iter().any(|o| o.title == r.title && o.provider == r.provider) {
                    analysis.sources.push(r);
                }
            }
        }
        analysis.model = caps.id.clone();
        analysis.asset = snap.summary.id.clone();
        analysis.price_at_analysis = snap.quote.as_ref().map(|q| q.price).filter(|p| p.is_finite());
        analysis.scores = card.scores.clone();
        analysis.id = uuid::Uuid::now_v7().to_string();
        analysis.generated_at = Utc::now();
        Ok(analysis)
    }

    /// Revisa la redacción del juez; si hay palabras inexistentes, otro alfabeto, inglés colado…
    /// pide al modelo (una sola llamada) que corrija solo esos textos. Si no hay nada sospechoso
    /// no cuesta ninguna llamada. Devuelve un aviso si quedan problemas sin corregir.
    async fn fix_wording(
        &self,
        caps: &ModelCaps,
        system: &str,
        lang_code: &str,
        j: &mut JudgeOut,
        usage: &Usage,
    ) -> Result<Option<String>> {
        if lang_code != "es" {
            return Ok(None);
        }
        let flagged: Vec<(usize, Vec<String>)> = judge_texts_mut(j)
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let issues: Vec<String> = lint::spanish_issues(t).into_iter().map(|x| x.what).collect();
                (!issues.is_empty()).then_some((i, issues))
            })
            .take(8)
            .collect();
        if flagged.is_empty() {
            return Ok(None);
        }
        let items: Vec<(String, Vec<String>)> = {
            let texts = judge_texts_mut(j);
            flagged.iter().map(|(i, iss)| (texts[*i].clone(), iss.clone())).collect()
        };
        tracing::info!(n = items.len(), "redacción sospechosa: se pide corregirla");
        let user = prompts::rewrite_task(lang_code, &items);
        let fixed = self
            .structured::<Rewrite>(caps, system, user, &rewrite_schema(items.len()), 1400, usage)
            .await;
        let mut remaining = Vec::new();
        match fixed {
            Err(CallError::Fatal(e)) => return Err(e),
            Err(CallError::Invalid(_)) => remaining.extend(items.iter().flat_map(|(_, iss)| iss.clone())),
            Ok(r) => {
                let mut texts = judge_texts_mut(j);
                for (k, (idx, _)) in flagged.iter().enumerate() {
                    let (old, _) = &items[k];
                    let Some(new) = r.texts.get(k) else { continue };
                    let new = new.trim();
                    // Solo se acepta si las cifras y citas no cambian y mejora la redacción.
                    let same_numbers = numbers_of(old) == numbers_of(new);
                    let better = lint::spanish_issues(new).len() < lint::spanish_issues(old).len();
                    if same_numbers && better && !new.is_empty() {
                        *texts[*idx] = new.to_string();
                    }
                }
                for (idx, _) in &flagged {
                    remaining.extend(lint::spanish_issues(texts[*idx]).into_iter().map(|x| x.what));
                }
            }
        }
        remaining.dedup();
        Ok((!remaining.is_empty()).then(|| {
            format!(
                "Posibles errores de redacción que no se pudieron corregir: {}.",
                remaining.into_iter().take(4).collect::<Vec<_>>().join("; ")
            )
        }))
    }
}

/// Cifras y citas de un texto, para comprobar que una reescritura no las toca.
fn numbers_of(t: &str) -> Vec<String> {
    let mut v: Vec<String> = extract_numbers(t).into_iter().filter(|n| !n.exempt).map(|n| n.raw).collect();
    v.extend(crate::context::extract_citations(t).into_iter().map(|n| format!("N{n}")));
    v.extend(knowledge::citation::extract_tags(t).into_iter().map(|n| format!("D{n}")));
    v.sort();
    v
}

/// Todos los textos redactados por el juez, en un orden estable.
fn judge_texts_mut(j: &mut JudgeOut) -> Vec<&mut String> {
    let mut v: Vec<&mut String> = vec![&mut j.summary, &mut j.thesis, &mut j.thesis_valuation, &mut j.thesis_technical, &mut j.thesis_balance, &mut j.horizon_note, &mut j.entry_strategy, &mut j.exit_strategy, &mut j.uncertainty];
    v.extend(j.bull_case.iter_mut().map(|c| &mut c.text));
    v.extend(j.bear_case.iter_mut().map(|c| &mut c.text));
    v.extend(j.risks.iter_mut().map(|c| &mut c.text));
    v.extend(j.would_change_if.iter_mut());
    v.extend(j.scenarios.iter_mut().map(|s| &mut s.description));
    v.extend(j.catalysts.iter_mut().map(|c| &mut c.description));
    v
}

fn levels_block(now: Option<&LevelPlan>, wait: Option<&LevelPlan>, lang: Lang) -> String {
    let Some(now) = now else { return String::new() };
    let mut s = now.prompt_block(lang);
    if let Some(w) = wait
        && let Some((a, b)) = w.entry_zone
    {
        s.push_str(&tr!(
            lang,
            "Si el veredicto es esperar: zona de entrada {} – {}{}.\n",
            "If the verdict is wait: entry zone {} – {}{}.\n",
            crate::fmt::price(a),
            crate::fmt::price(b),
            w.stop_loss.map(|x| format!(", stop {}", crate::fmt::price(x))).unwrap_or_default()
        ));
    }
    s.push_str(lang.t(
        "Con reducir o evitar no hay entrada, stop ni objetivos.\nUmbrales de referencia: RSI14 sobrecompra 70, sobreventa 30.\n",
        "With reduce or avoid there is no entry, stop or targets.\nReference thresholds: RSI14 overbought 70, oversold 30.\n",
    ));
    s
}

fn analysis_price(snap: &AssetSnapshot) -> Option<f64> {
    snap.quote.as_ref().map(|q| q.price).filter(|p| p.is_finite() && *p > 0.0)
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
    /// quick: técnico corto (2 hallazgos).
    QuickTechnical,
    /// quick: fundamental + noticias + macro/riesgo en un informe corto (2 hallazgos).
    QuickFundamental,
}

impl Stage {
    fn task(self) -> String {
        match self {
            Self::Technical => prompts::technical_task(),
            Self::Fundamental => prompts::fundamental_task(),
            Self::Sentiment => prompts::sentiment_task(),
            Self::MacroRisk => prompts::macro_task(),
            Self::QuickTechnical => prompts::quick_technical_task(),
            Self::QuickFundamental => prompts::quick_fundamental_task(),
        }
    }
    fn quick(self) -> bool {
        matches!(self, Self::QuickTechnical | Self::QuickFundamental)
    }
    fn max_findings(self) -> usize {
        if self.quick() { 2 } else { 4 }
    }
    fn max_tokens(self) -> u32 {
        if self.quick() { QUICK_STAGE_MAX_TOKENS } else { 600 }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Technical | Self::QuickTechnical => "technical",
            Self::Fundamental | Self::QuickFundamental => "fundamental",
            Self::Sentiment => "sentiment",
            Self::MacroRisk => "macro_risk",
        }
    }
    fn label(self, l: Lang) -> &'static str {
        match self {
            Self::Technical | Self::QuickTechnical => l.t("Analista técnico", "Technical analyst"),
            Self::Fundamental => l.t("Analista fundamental", "Fundamental analyst"),
            Self::QuickFundamental => l.t("Fundamentales, noticias y riesgo", "Fundamentals, news and risk"),
            Self::Sentiment => l.t("Sentimiento y noticias", "Sentiment and news"),
            Self::MacroRisk => l.t("Macro y riesgo", "Macro and risk"),
        }
    }
}

fn stage_plan(depth: AnalysisDepth) -> Vec<Stage> {
    match depth {
        AnalysisDepth::Quick => vec![Stage::QuickTechnical, Stage::QuickFundamental],
        AnalysisDepth::Standard | AnalysisDepth::Deep => {
            vec![Stage::Technical, Stage::Fundamental, Stage::Sentiment, Stage::MacroRisk]
        }
    }
}

/// Contexto de la segunda etapa de `quick`: lo fundamental, las noticias y el entorno macro/riesgo,
/// sin el bloque de indicadores técnicos (ya lo ve la etapa técnica) ni los suyos en la tabla.
fn quick_fundamental_sections() -> Sections {
    let mut s = Sections::fundamental();
    s.news = true;
    s.macro_ctx = true;
    s.scores |= Sections::S_SENTIMENT | Sections::S_MACRO | Sections::S_RISK;
    s
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

/// Normaliza probabilidades a enteros que suman exactamente 100 (método del mayor resto).
fn normalize_probabilities(p: &[f32]) -> Vec<f32> {
    let clean: Vec<f64> = p.iter().map(|x| if x.is_finite() { f64::from(x.max(0.0)) } else { 0.0 }).collect();
    let total: f64 = clean.iter().sum();
    if total <= 0.0 {
        return vec![];
    }
    let exact: Vec<f64> = clean.iter().map(|x| x / total * 100.0).collect();
    let mut floors: Vec<f64> = exact.iter().map(|x| x.floor()).collect();
    let mut left = 100.0 - floors.iter().sum::<f64>();
    let mut order: Vec<usize> = (0..exact.len()).collect();
    order.sort_by(|a, b| (exact[*b] - floors[*b]).total_cmp(&(exact[*a] - floors[*a])));
    for i in order {
        if left < 0.5 {
            break;
        }
        floors[i] += 1.0;
        left -= 1.0;
    }
    floors.into_iter().map(|x| x as f32).collect()
}

fn price_scenarios(sc: &mut [Scenario], plan: &LevelPlan, price: Option<f64>, lang: Lang) {
    let lv = levels::scenario_levels(plan, lang);
    for s in sc.iter_mut() {
        let l = match s.kind.as_str() {
            "bullish" => lv.bullish.clone(),
            "base" => lv.base.clone(),
            _ => lv.bearish.clone(),
        };
        if let Some(l) = l {
            s.target_price = Some(l.value);
            s.target_basis = Some(l.describe(lang));
            s.return_pct = price.map(|p| ((l.value / p - 1.0) * 1000.0).round() / 10.0);
        }
    }
}

/// Antepone a cada escenario la frase de base del precio, generada en Rust (de dónde sale el
/// número, con proveedor y fecha), y conserva del texto del modelo solo las frases cuyas cifras son
/// las de ESE escenario (su precio objetivo, los niveles de los que sale, el precio de partida, la
/// variación calculada o su probabilidad): el modelo redacta el porqué, no los números.
/// Devuelve las notas de calidad de lo descartado.
fn explain_scenarios(sc: &mut [Scenario], plan: &LevelPlan, snap: &AssetSnapshot, lang: Lang) -> Vec<String> {
    let src = levels::source_label(snap, lang);
    let start = analysis_price(snap);
    let lv = levels::scenario_levels(plan, lang);
    let mut notes = Vec::new();
    for s in sc.iter_mut() {
        let level = match s.kind.as_str() {
            "bullish" => lv.bullish.clone(),
            "base" => lv.base.clone(),
            _ => lv.bearish.clone(),
        };
        let (Some(target), Some(level)) = (s.target_price, level) else { continue };
        let allowed: Vec<f64> = level
            .refs
            .iter()
            .copied()
            .chain([target, f64::from(s.probability)])
            .chain(start)
            .chain(s.return_pct)
            .chain(s.return_pct.map(f64::abs))
            .collect();
        let matches = |v: f64| allowed.iter().any(|a| (a - v).abs() <= (a.abs() * 0.0006).max(0.06));
        let base = if level.derived {
            tr!(lang, "Precio {} = {} ({}).", "Price {} = {} ({}).", fmt::price_l(lang, target), level.basis, src)
        } else {
            tr!(lang, "Precio = {} ({}).", "Price = {} ({}).", level.describe(lang), src)
        };
        let mut kept = Vec::new();
        let mut dropped = 0;
        for sent in s.description.split_inclusive(". ").map(str::trim).filter(|x| !x.is_empty()) {
            // Los números de una fecha escrita («30 de octubre») ya los revisó `prose` como fecha.
            let dates = crate::dates::extract_dates(sent);
            let in_date = |n: &crate::fidelity::Num| dates.iter().any(|d| n.span.start >= d.span.start && n.span.end <= d.span.end);
            if extract_numbers(sent).iter().filter(|n| !n.exempt && !in_date(n)).all(|n| n.values().any(|v| matches(v.abs()) || matches(v))) {
                kept.push(sent);
            } else {
                dropped += 1;
            }
        }
        if dropped > 0 {
            let kind = match s.kind.as_str() {
                "bullish" => lang.t("alcista", "bullish"),
                "base" => lang.t("base", "base"),
                _ => lang.t("bajista", "bearish"),
            };
            notes.push(tr!(
                lang,
                "Escenario {}: {} frase(s) del modelo descartada(s) por citar cifras que no son las de este escenario (precio {}).",
                "Scenario {}: {} model sentence(s) dropped for citing figures that are not this scenario's (price {}).",
                kind,
                dropped,
                fmt::price_l(lang, target)
            ));
        }
        s.description = if kept.is_empty() { base } else { format!("{base} {}", kept.join(" ")) };
        s.target_basis = Some(level.describe(lang));
    }
    notes
}

fn expected_return(sc: &[Scenario]) -> Option<f64> {
    if sc.is_empty() || sc.iter().any(|s| s.return_pct.is_none()) {
        return None;
    }
    let e: f64 = sc.iter().map(|s| f64::from(s.probability) / 100.0 * s.return_pct.unwrap_or(0.0)).sum();
    Some((e * 10.0).round() / 10.0)
}

/// Todas las fechas que traen los datos: resultados, máximo histórico, vencimiento, cualquier fecha
/// ISO de `fundamentals.extra` (p. ej. `exDividendDate` de Yahoo), publicación de noticias, marcas
/// de tiempo de cotización, indicadores y macro, y la fecha del propio snapshot.
fn dates_in_data(snap: &AssetSnapshot) -> std::collections::HashSet<NaiveDate> {
    let mut d = std::collections::HashSet::new();
    d.insert(snap.generated_at.date_naive());
    d.extend(snap.news.iter().map(|n| n.published_at.date_naive()));
    d.extend(snap.quote.iter().map(|q| q.timestamp.date_naive()));
    d.extend(snap.indicators.iter().filter_map(|i| i.as_of).map(|t| t.date_naive()));
    if let Some(m) = &snap.macro_context {
        d.extend(m.as_of.map(|t| t.date_naive()));
        d.extend(m.indicators.iter().map(|i| i.as_of.date_naive()));
    }
    if let Some(f) = &snap.fundamentals {
        d.extend([f.next_earnings_date, f.all_time_high_date, f.maturity_date].into_iter().flatten());
        d.extend(f.as_of.map(|t| t.date_naive()));
        d.extend(f.extra.values().filter_map(|v| v.as_str()).filter_map(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()));
    }
    d
}

/// Resultado de re-verificar un texto ya generado (ver `audit_figures`).
#[doc(hidden)]
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditRow {
    pub field: String,
    pub labeled_ok: usize,
    pub unlabeled_ok: usize,
    pub not_in_data: usize,
    /// (cifra, palabra del texto, rótulo de los datos, fragmento)
    pub wrong_label: Vec<(String, String, String, String)>,
    pub unknown_dates: Vec<(String, String)>,
}

/// Re-verifica textos ya generados (campo, texto) contra el contexto que habría visto el modelo, con el
/// mismo índice que el motor (contexto completo, el de cada etapa, el del juez y el bloque de niveles).
/// Sirve para medir el efecto de cambios en la verificación sobre salidas guardadas (ejemplo
/// `compare_models reverify`).
#[doc(hidden)]
pub fn audit_figures(snapshot: &AssetSnapshot, lang_code: &str, num_ctx: u32, texts: &[(String, String)]) -> Vec<AuditRow> {
    let lang = Lang::from_code(lang_code);
    let snap = sanitize_snapshot(snapshot);
    let card = scoring::score(&snap, lang);
    let mut registry = NewsRegistry::new();
    let max_tokens = (num_ctx as usize * 40 / 100).clamp(1200, 16_000);
    let opts = ContextOptions { max_tokens, lang };
    let built = build_context(&snap, &card, &opts, &mut registry);
    let mut idx = NumberIndex::with_dates(dates_in_data(&snap));
    idx.add_context(&built.text);
    for sec in [Sections::technical(), Sections::fundamental(), Sections::sentiment(), Sections::macro_risk(), quick_fundamental_sections(), Sections::judge()] {
        idx.add_context(&build_context_sections(&snap, &card, &opts, &mut registry, &sec).text);
    }
    let plan_now = levels::plan(&snap, Verdict::Accumulate, lang);
    let plan_wait = levels::plan(&snap, Verdict::Wait, lang);
    idx.add_text(&levels_block(plan_now.as_ref(), plan_wait.as_ref(), lang), Segment::Levels);
    let snippet = |t: &str, at: usize| {
        let a = t.char_indices().map(|(i, _)| i).rev().find(|i| *i <= at.saturating_sub(70)).unwrap_or(0);
        let b = t.char_indices().map(|(i, _)| i).find(|i| *i >= at + 50).unwrap_or(t.len());
        t[a..b].replace('\n', " ")
    };
    texts
        .iter()
        .map(|(field, text)| {
            let r = idx.review(text);
            let mut row = AuditRow { field: field.clone(), labeled_ok: r.labeled_ok, unlabeled_ok: r.unlabeled_ok, not_in_data: 0, wrong_label: vec![], unknown_dates: vec![] };
            for f in &r.findings {
                match &f.kind {
                    crate::fidelity::FindingKind::NotInData => row.not_in_data += 1,
                    crate::fidelity::FindingKind::WrongLabel { said, actual } => row.wrong_label.push((f.raw.clone(), said.clone(), actual.clone(), snippet(text, f.start))),
                    crate::fidelity::FindingKind::UnknownDate => row.unknown_dates.push((f.raw.clone(), snippet(text, f.start))),
                }
            }
            row
        })
        .collect()
}

/// El modelo a veces inventa la fecha de un catalizador (qwen3 pone fechas sueltas como 25-oct que no
/// figuran en ningún sitio): se conserva solo si está entre las fechas de los datos.
fn catalyst_date(date: Option<NaiveDate>, allowed: &std::collections::HashSet<NaiveDate>) -> Option<NaiveDate> {
    date.filter(|d| allowed.contains(d))
}

/// Convierte la salida del juez (ya sin confiar en ella) en un `Analysis` parcial: el llamador
/// completa id, fuentes, avisos, niveles, modelo y marcas de tiempo.
fn assemble(j: JudgeOut, snap: &AssetSnapshot, card: &Scorecard, v: &mut Verifier<'_>, lang: Lang, deep: bool) -> Analysis {
    let claims = |list: &[Claim], v: &mut Verifier<'_>| -> Vec<Checked> { list.iter().filter_map(|c| v.claim(c)).collect() };
    let mut bull_case = claims(&j.bull_case, v);
    let mut bear_case = claims(&j.bear_case, v);
    let mut risks = claims(&j.risks, v);
    // El mismo punto repetido (dentro de una lista o entre ellas) no aporta: se compara sin la cita de fuente.
    let removed = lint::dedupe_bullets(&mut [&mut bull_case, &mut bear_case, &mut risks], |c| c.plain.as_str());
    if removed > 0 {
        tracing::info!(removed, "viñetas repetidas quitadas de bullCase/bearCase/risks");
    }
    let texts = |l: Vec<Checked>| -> Vec<String> { l.into_iter().map(|c| c.text).collect() };
    let (bull_case, bear_case, risks) = (texts(bull_case), texts(bear_case), texts(risks));

    let summary = v.prose(&j.summary, lang.t("resumen", "summary"));
    let mut thesis = v.prose_cited(&j.full_thesis(), lang.t("tesis", "thesis"));
    if !j.uncertainty.trim().is_empty() {
        let u = v.prose(&j.uncertainty, lang.t("incertidumbre", "uncertainty"));
        thesis.push_str(&format!("\n\n{}: {u}", lang.t("Incertidumbre", "Uncertainty")));
    }
    let change: Vec<String> = j.would_change_if.iter().filter(|s| !s.trim().is_empty()).map(|s| v.prose(s, lang.t("qué cambiaría", "what would change"))).collect();
    let horizon_note = (!j.horizon_note.trim().is_empty()).then(|| v.prose(&j.horizon_note, lang.t("horizonte", "horizon")));

    let mut strategy_parts = Vec::new();
    for (label, text, field) in [
        (lang.t("Entrada", "Entry"), &j.entry_strategy, "entrada"),
        (lang.t("Salida", "Exit"), &j.exit_strategy, "salida"),
    ] {
        if !text.trim().is_empty() {
            strategy_parts.push(format!("{label}: {}", v.prose(text, field)));
        }
    }

    let mut undated = 0usize;
    let allowed_dates = dates_in_data(snap);
    let catalysts = j
        .catalysts
        .iter()
        .filter(|c| !c.description.trim().is_empty())
        .map(|c| Catalyst {
            date: {
                let parsed = c.date.as_deref().and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok());
                let known = catalyst_date(parsed, &allowed_dates);
                undated += usize::from(parsed.is_some() && known.is_none());
                known
            },
            description: v.prose(&c.description, lang.t("catalizador", "catalyst")),
            impact: c.impact.filter(|i| i.is_finite()).map(|i| i.clamp(-1.0, 1.0)),
        })
        .collect();

    if undated > 0 {
        let w = tr!(
            lang,
            "Se quitó la fecha de {} catalizador(es): no figura en los datos.",
            "The date of {} catalyst(s) was removed: it is not in the data.",
            undated
        );
        if !v.warnings.contains(&w) {
            v.warnings.push(w);
        }
    }

    // Escenarios (deep): tres tipos distintos, probabilidades normalizadas a 100.
    let scenarios = if deep { build_scenarios(&j, v, lang) } else { None };

    let confidence = calibrate(j.confidence, card, j.verdict, v.dropped, v.marked);

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
        key_levels: Default::default(),
        strategy: (!strategy_parts.is_empty()).then(|| strategy_parts.join("\n")),
        data_warnings: vec![],
        sources: vec![],
        price_at_analysis: None,
        model: String::new(),
        generated_at: Utc::now(),
        levels_note: None,
        scenarios,
        expected_return_pct: None,
        would_change_if: Some(change),
        horizon_note,
        position_sizing: None,
        method: None,
        quality_notes: None,
        depth: None,
        duration_ms: None,
        llm_calls: None,
        prompt_tokens: None,
        completion_tokens: None,
    }
}

fn build_scenarios(j: &JudgeOut, v: &mut Verifier<'_>, lang: Lang) -> Option<Vec<Scenario>> {
    const KINDS: [&str; 3] = ["bullish", "base", "bearish"];
    if j.scenarios.len() < 3 {
        tracing::warn!(n = j.scenarios.len(), "el juez no dio los tres escenarios");
        return None;
    }
    // Lo normal es una de cada tipo; si el modelo repite o inventa el tipo, se asigna por orden.
    let distinct = KINDS.iter().all(|k| j.scenarios.iter().filter(|s| s.kind == *k).count() == 1);
    let picked: Vec<(&str, &types::ScenarioOut)> = if distinct {
        KINDS.iter().map(|k| (*k, j.scenarios.iter().find(|s| s.kind == *k).expect("tipo presente"))).collect()
    } else {
        KINDS.iter().copied().zip(j.scenarios.iter()).collect()
    };
    let probs = normalize_probabilities(&picked.iter().map(|(_, s)| s.probability).collect::<Vec<_>>());
    if probs.len() != 3 {
        tracing::warn!("probabilidades de escenarios no válidas");
        return None;
    }
    Some(
        picked
            .into_iter()
            .zip(probs)
            .map(|((kind, s), probability)| Scenario {
                description: v.prose(&s.description, lang.t("escenario", "scenario")),
                kind: kind.to_string(),
                probability,
                target_price: None,
                target_basis: None,
                return_pct: None,
            })
            .collect(),
    )
}

/// Análisis construido solo con la puntuación determinista (si el LLM no da JSON válido).
fn fallback(snap: &AssetSnapshot, card: &Scorecard, _v: &mut Verifier<'_>, lang: Lang) -> Analysis {
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
        key_levels: Default::default(),
        strategy: None,
        data_warnings: vec![],
        sources: vec![],
        price_at_analysis: None,
        model: String::new(),
        generated_at: Utc::now(),
        levels_note: None,
        scenarios: None,
        expected_return_pct: None,
        would_change_if: None,
        horizon_note: None,
        position_sizing: None,
        method: None,
        quality_notes: None,
        depth: None,
        duration_ms: None,
        llm_calls: None,
        prompt_tokens: None,
        completion_tokens: None,
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
mod scenario_text_tests {
    use super::*;
    use crate::{analysis::levels::Level, fixture};

    /// Plan como el de BTC: soporte 81827.38, 2.º soporte SMA50 81059.79, ATR 2057.45.
    fn btc_plan() -> LevelPlan {
        let lv = |value: f64, basis: &str| Level { value, basis: basis.into(), refs: vec![value], derived: false };
        LevelPlan {
            price: 82_000.0,
            atr: Some(2057.45),
            supports: vec![lv(81_827.38, "banda de Bollinger inferior"), lv(81_059.79, "SMA50")],
            upside: vec![lv(84_078.75, "SMA20"), lv(86_330.11, "banda de Bollinger superior")],
            ..Default::default()
        }
    }

    fn bearish(desc: &str) -> Vec<Scenario> {
        vec![Scenario {
            kind: "bearish".into(),
            probability: 30.0,
            description: desc.into(),
            target_price: Some(81_059.79),
            target_basis: None,
            return_pct: Some(-1.1),
        }]
    }

    fn run(desc: &str) -> (Vec<Scenario>, Vec<String>) {
        let mut snap = fixture::stock_snapshot();
        snap.quote.as_mut().unwrap().price = 82_000.0;
        snap.quote.as_mut().unwrap().source = "Binance".into();
        snap.quote.as_mut().unwrap().timestamp = "2026-10-12T10:00:00Z".parse().unwrap();
        let mut sc = bearish(desc);
        let notes = explain_scenarios(&mut sc, &btc_plan(), &snap, Lang::Es);
        (sc, notes)
    }

    #[test]
    fn btc_case_support_figure_foreign_to_the_scenario_is_dropped_and_noted() {
        // Caso real: el precio objetivo es la SMA50 pero el modelo habla del soporte 79769.92.
        let (sc, notes) = run("El precio podría caer hacia el soporte 79769.92.");
        let d = &sc[0].description;
        assert_eq!(d, "Precio = SMA50 81.059,79 (Binance, 12 oct 2026).", "{d}");
        assert!(!d.contains("79769"));
        assert_eq!(sc[0].target_basis.as_deref(), Some("SMA50 81.059,79"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("bajista") && notes[0].contains("descartada"), "{notes:?}");
    }

    #[test]
    fn coherent_model_text_is_kept_after_the_rust_basis() {
        let (sc, notes) = run("Si el MACD sigue negativo, el precio cedería hasta 81059.79 (-1.1%). Dependería de que la volatilidad persista.");
        let d = &sc[0].description;
        assert!(d.starts_with("Precio = SMA50 81.059,79 (Binance, 12 oct 2026). Si el MACD"), "{d}");
        assert!(d.contains("volatilidad persista"));
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn spanish_formatted_figures_of_the_scenario_are_kept() {
        // TODO 4: el modelo escribió «81.059,79» y «-1,1 %»: antes se leían como 81.059 / 79 / 1 y 1
        // y la frase se descartaba por «citar cifras ajenas».
        let (sc, notes) = run("Si el MACD sigue negativo, el precio cedería hasta 81.059,79 (-1,1 %). Dependería de la volatilidad.");
        let d = &sc[0].description;
        assert!(d.contains("cedería hasta 81.059,79 (-1,1 %)") && d.contains("volatilidad"), "{d}");
        assert!(notes.is_empty(), "{notes:?}");
        // Y una cifra española ajena sí se descarta.
        let (sc, notes) = run("Podría caer hasta el soporte 79.769,92.");
        assert!(!sc[0].description.contains("79.769"), "{}", sc[0].description);
        assert_eq!(notes.len(), 1);
        // Formato inglés intacto.
        let (_, notes) = run("Cedería hasta 81059.79 (-1.1%).");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn only_the_incoherent_sentence_is_dropped() {
        let (sc, notes) = run("Si el MACD sigue negativo, cedería. Caería a 79769.92 sin remedio. Habría ventas por miedo.");
        let d = &sc[0].description;
        assert!(d.contains("MACD") && d.contains("ventas por miedo") && !d.contains("79769"), "{d}");
        assert_eq!(notes.len(), 1);
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
