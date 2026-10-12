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
    #[serde(default)]
    pub missing: Vec<String>,
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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LevelsOut {
    #[serde(default)]
    pub supports: Vec<f64>,
    #[serde(default)]
    pub resistances: Vec<f64>,
    #[serde(default)]
    pub entry_zone: Vec<f64>,
    #[serde(default)]
    pub stop_loss: Option<f64>,
    #[serde(default)]
    pub targets: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JudgeOut {
    pub verdict: Verdict,
    pub confidence: f32,
    pub horizon: Horizon,
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
    pub key_levels: LevelsOut,
    #[serde(default)]
    pub entry_strategy: String,
    #[serde(default)]
    pub exit_strategy: String,
    #[serde(default)]
    pub position_sizing: String,
    #[serde(default)]
    pub missing_data: Vec<String>,
    #[serde(default)]
    pub would_change_if: Vec<String>,
    #[serde(default)]
    pub uncertainty: String,
}

fn s() -> Value {
    json!({"type": "string"})
}

fn claim_schema(with_area: bool) -> Value {
    let mut props = json!({
        "text": s(),
        "figures": {"type": "array", "items": s(), "maxItems": 8}
    });
    if with_area {
        props["area"] = json!({"type": "string", "enum": ["technical", "fundamental", "sentiment", "macro_risk"]});
    }
    json!({"type": "object", "properties": props, "required": ["text", "figures"]})
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
            "findings": list_of(claim_schema(with_area), max_findings),
            "missing": list_of(s(), 6)
        },
        "required": ["stance", "confidence", "findings", "missing"]
    })
}

pub fn debate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "arguments": list_of(claim_schema(false), 5),
            "opponent_weakness": s(),
            "would_prove_me_wrong": s()
        },
        "required": ["arguments", "opponent_weakness", "would_prove_me_wrong"]
    })
}

pub fn judge_schema() -> Value {
    let num_list = |max| list_of(json!({"type": "number"}), max);
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["buy", "accumulate", "hold", "wait", "reduce", "avoid"]},
            "confidence": {"type": "number"},
            "horizon": {"type": "string", "enum": ["short", "medium", "long"]},
            "summary": s(),
            "thesis": s(),
            "bull_case": list_of(claim_schema(false), 5),
            "bear_case": list_of(claim_schema(false), 5),
            "risks": list_of(claim_schema(false), 6),
            "catalysts": list_of(json!({
                "type": "object",
                "properties": {"date": s(), "description": s(), "impact": {"type": "number"}},
                "required": ["description"]
            }), 5),
            "key_levels": {
                "type": "object",
                "properties": {
                    "supports": num_list(4),
                    "resistances": num_list(4),
                    "entry_zone": num_list(2),
                    "stop_loss": {"type": "number"},
                    "targets": num_list(3)
                }
            },
            "entry_strategy": s(),
            "exit_strategy": s(),
            "position_sizing": s(),
            "missing_data": list_of(s(), 8),
            "would_change_if": list_of(s(), 5),
            "uncertainty": s()
        },
        "required": ["verdict", "confidence", "horizon", "summary", "thesis", "bull_case", "bear_case",
            "risks", "entry_strategy", "exit_strategy", "position_sizing", "missing_data", "would_change_if", "uncertainty"]
    })
}
