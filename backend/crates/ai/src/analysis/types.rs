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
    /// Área del hallazgo (opcional; el digest la antepone si viene).
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
    /// Tesis en tres campos (un párrafo cada uno): fuerza la estructura en modelos pequeños.
    #[serde(default)]
    pub thesis_valuation: String,
    #[serde(default)]
    pub thesis_technical: String,
    #[serde(default)]
    pub thesis_balance: String,
    /// Tesis en un solo texto (modelos antiguos o respuestas parciales).
    #[serde(default)]
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

impl JudgeOut {
    /// Tesis final: los tres párrafos, o el texto único si el modelo lo dio así.
    pub fn full_thesis(&self) -> String {
        let parts: Vec<&str> = [&self.thesis_valuation, &self.thesis_technical, &self.thesis_balance]
            .into_iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        let joined = if parts.is_empty() { self.thesis.trim().to_string() } else { parts.join("\n\n") };
        super::lint::dedupe_sentences(&joined)
    }
}

fn s() -> Value {
    json!({"type": "string"})
}

fn claim_schema() -> Value {
    json!({"type": "object", "properties": { "text": s() }, "required": ["text"]})
}

fn list_of(item: Value, max: usize) -> Value {
    json!({"type": "array", "items": item, "maxItems": max})
}

pub fn stage_schema(max_findings: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "stance": {"type": "string", "enum": ["bullish", "bearish", "neutral", "mixed"]},
            "confidence": {"type": "number"},
            "findings": list_of(claim_schema(), max_findings)
        },
        "required": ["stance", "confidence", "findings"]
    })
}

pub fn debate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "arguments": list_of(claim_schema(), 4),
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
        "thesis_valuation": s(),
        "thesis_technical": s(),
        "thesis_balance": s(),
        "bull_case": list_of(claim_schema(), 4),
        "bear_case": list_of(claim_schema(), 4),
        "risks": list_of(claim_schema(), 5),
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
        "verdict", "confidence", "horizon", "horizon_note", "summary", "thesis_valuation", "thesis_technical", "thesis_balance", "bull_case", "bear_case", "risks",
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

/// Esquema del juez de `quick`: sin horizonte comentado, catalizadores, estrategia ni escenarios, con
/// listas cortas. Lo que no pide queda vacío/`None` en `Analysis` (no se inventa).
pub fn quick_judge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["buy", "accumulate", "hold", "wait", "reduce", "avoid"]},
            "confidence": {"type": "number"},
            "horizon": {"type": "string", "enum": ["short", "medium", "long"]},
            "summary": s(),
            "thesis_valuation": s(),
            "thesis_technical": s(),
            "thesis_balance": s(),
            "bull_case": list_of(claim_schema(), 1),
            "bear_case": list_of(claim_schema(), 1),
            "risks": list_of(claim_schema(), 3),
            "would_change_if": list_of(s(), 2),
            "uncertainty": s()
        },
        "required": [
            "verdict", "confidence", "horizon", "summary", "thesis_valuation", "thesis_technical", "thesis_balance",
            "bull_case", "bear_case", "risks", "would_change_if", "uncertainty"
        ]
    })
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
