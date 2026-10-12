//! Tipos de las etapas del pipeline y sus JSON Schemas (se envían a Ollama en `format`).
//!
//! Los schemas son deliberadamente simples (tipos, `required`, `enum`, `maxItems`): el
//! decodificador con gramática de Ollama los cumple siempre, y lo que no puede expresar (rangos,
//! coherencia entre campos) se valida y se recorta en Rust.

use domain::{Horizon, Verdict};
use serde::Deserialize;
use serde_json::{Value, json};

/// Afirmación del modelo con las cifras exactas en las que se apoya.
#[derive(Debug, Clone, Deserialize)]
pub struct Claim {
    pub text: String,
    #[serde(default)]
    pub figures: Vec<String>,
    /// Solo en la etapa combinada (modo rápido): técnico, fundamental, sentimiento, macro.
    #[serde(default)]
    pub area: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StageReport {
    #[serde(default)]
    pub stance: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub findings: Vec<Claim>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DebateReport {
    #[serde(default)]
    pub arguments: Vec<Claim>,
    #[serde(default)]
    pub opponent_weakness: String,
    #[serde(default)]
    pub would_prove_me_wrong: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalystOut {
    #[serde(default)]
    pub date: Option<String>,
    pub description: String,
    #[serde(default)]
    pub impact: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioOut {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub probability: f32,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JudgeOut {
    pub verdict: Verdict,
    pub confidence: f32,
    pub horizon: Horizon,
    #[serde(default)]
    pub horizon_note: String,
    pub summary: String,
    pub thesis: String,
    #[serde(default)]
    pub bull_case: Vec<Claim>,
    #[serde(default)]
    pub bear_case: Vec<Claim>,
    #[serde(default)]
    pub risks: Vec<Claim>,
    #[serde(default)]
    pub catalysts: Vec<CatalystOut>,
    #[serde(default)]
    pub entry_strategy: String,
    #[serde(default)]
    pub exit_strategy: String,
    #[serde(default)]
    pub would_change_if: Vec<String>,
    #[serde(default)]
    pub uncertainty: String,
    #[serde(default)]
    pub scenarios: Vec<ScenarioOut>,
}

fn s() -> Value {
    json!({"type": "string"})
}

fn claim_schema(with_area: bool) -> Value {
    let mut props = json!({ "text": s() });
    if with_area {
        props["area"] = json!({"type": "string", "enum": ["technical", "fundamental", "sentiment", "macro_risk"]});
    }
    json!({"type": "object", "properties": props, "required": ["text"]})
}

fn list_of(item: Value, max: usize) -> Value {
    json!({"type": "array", "items": item, "maxItems": max})
}

pub fn stage_schema(with_area: bool, max_findings: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "stance": {"type": "string", "enum": ["bullish", "bearish", "neutral", "mixed"]},
            "confidence": {"type": "number"},
            "findings": list_of(claim_schema(with_area), max_findings)
        },
        "required": ["stance", "confidence", "findings"]
    })
}

pub fn debate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "arguments": list_of(claim_schema(false), 4),
            "opponent_weakness": s(),
            "would_prove_me_wrong": s()
        },
        "required": ["arguments", "opponent_weakness", "would_prove_me_wrong"]
    })
}

pub fn judge_schema(deep: bool) -> Value {
    let mut props = json!({
        "verdict": {"type": "string", "enum": ["buy", "accumulate", "hold", "wait", "reduce", "avoid"]},
        "confidence": {"type": "number"},
        "horizon": {"type": "string", "enum": ["short", "medium", "long"]},
        "horizon_note": s(),
        "summary": s(),
        "thesis": s(),
        "bull_case": list_of(claim_schema(false), 4),
        "bear_case": list_of(claim_schema(false), 4),
        "risks": list_of(claim_schema(false), 5),
        "catalysts": list_of(json!({
            "type": "object",
            "properties": {"date": s(), "description": s(), "impact": {"type": "number"}},
            "required": ["description"]
        }), 4),
        "entry_strategy": s(),
        "exit_strategy": s(),
        "would_change_if": list_of(s(), 4),
        "uncertainty": s()
    });
    let mut required = vec![
        "verdict", "confidence", "horizon", "horizon_note", "summary", "thesis", "bull_case", "bear_case", "risks",
        "entry_strategy", "exit_strategy", "would_change_if", "uncertainty",
    ];
    if deep {
        props["scenarios"] = json!({
            "type": "array",
            "minItems": 3,
            "maxItems": 3,
            "items": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": ["bullish", "base", "bearish"]},
                    "probability": {"type": "number"},
                    "description": s()
                },
                "required": ["kind", "probability", "description"]
            }
        });
        required.push("scenarios");
    }
    json!({"type": "object", "properties": props, "required": required})
}

/// Esquema de la pasada de corrección de redacción: la misma lista de textos corregidos.
pub fn rewrite_schema(n: usize) -> Value {
    json!({
        "type": "object",
        "properties": {"texts": {"type": "array", "items": s(), "minItems": n, "maxItems": n}},
        "required": ["texts"]
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rewrite {
    pub texts: Vec<String>,
}
