//! Secciones clave del folleto resumido (497K) o del folleto (485BPOS, 497) de un ETF en texto
//! limpio: objetivo de inversión, comisiones y gastos, estrategia principal, riesgos principales y
//! rotación de cartera. Reutiliza `html::html_to_lines` y `prospectus::normalize`.
//!
//! No es el extractor de OPV (`prospectus.rs`): los folletos de fondos tienen otro catálogo de
//! títulos y casi siempre un índice o el encabezado de varios fondos. Una sección que no se localiza
//! se declara en `missing`; nunca se inventa texto.

use domain::ProspectusSection;

use crate::{
    html::{Line, html_to_lines},
    prospectus::normalize,
};

/// Claves en el orden habitual del documento.
pub const KEYS: &[&str] = &["objective", "fees", "strategy", "risks", "turnover"];

/// Caracteres mínimos del cuerpo para no confundir un título con una entrada del índice.
const MIN_BODY: usize = 40;
/// Líneas que se toman de la rotación cuando solo hay un párrafo que empieza por «Portfolio Turnover».
const TURNOVER_LINES: usize = 3;

#[derive(Debug, Clone)]
pub struct FundExtract {
    pub sections: Vec<ProspectusSection>,
    pub missing: Vec<String>,
    pub document_chars: usize,
    /// El ticker aparece en el documento como palabra completa.
    pub mentions_ticker: bool,
    /// Título del fondo (primera línea con «ETF»/«Fund»/«Trust» del encabezado), si se ve.
    pub fund_name: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Head {
    Key(&'static str),
    /// Título de otra sección que termina la anterior.
    Boundary,
}

fn is_page_number(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || t.len() > 8 {
        return false;
    }
    let body = t.strip_prefix(['S', 's', 'A', 'a', 'F', 'f']).map_or(t, |r| r.trim_start_matches('-'));
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit())
}

fn classify(l: &Line, norm: &str) -> Option<Head> {
    if l.table || l.text.chars().count() > 110 {
        return None;
    }
    let words = norm.split(' ').count();
    let key = if norm.starts_with("investment objective") || norm == "investment goal" || norm == "fund s investment objective" || norm == "the fund s investment objective" {
        Some("objective")
    } else if (norm.starts_with("fees and expenses") || norm.starts_with("fund fees and expenses")) && words <= 10 {
        Some("fees")
    } else if (norm.contains("investments and portfolio turnover") || norm.starts_with("principal investment strateg") || norm.starts_with("principal strateg") || norm == "investment strategies" || norm == "investment strategy") && words <= 9 {
        Some("strategy")
    } else if (norm.starts_with("principal risk") || norm == "summary of principal risks" || norm == "principal risk factors") && words <= 10 {
        Some("risks")
    } else if norm.starts_with("portfolio turnover") && words <= 3 {
        Some("turnover")
    } else {
        None
    };
    if let Some(k) = key {
        return Some(Head::Key(k));
    }
    const STARTS: &[&str] = &[
        "performance",
        "annual total return",
        "average annual total return",
        "management",
        "investment adviser",
        "investment advisor",
        "portfolio manager",
        "purchase and sale",
        "tax information",
        "payments to broker",
        "financial highlights",
        "additional information",
        "more information",
        "buying and selling",
        "premium discount",
        "important information",
        "index provider",
        "disclosure of portfolio holdings",
        "creation and redemption",
        "dividends and distributions",
        "distribution plan",
        "the trust",
        "summary information",
        "fund summary",
    ];
    // «The Trust», «Performance» y similares solo cuentan como título si la línea es corta.
    (words <= 8 && STARTS.iter().any(|s| norm.strip_prefix(s).is_some_and(|rest| rest.is_empty() || rest.starts_with(' ')))).then_some(Head::Boundary)
}

/// Palabra completa (sin letras ni dígitos pegados) en `hay`.
pub fn contains_word(hay: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(word) {
        let s = from + i;
        let e = s + word.len();
        let before = hay[..s].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = hay[e..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = e;
    }
    false
}

/// Extrae las secciones. `ticker` en mayúsculas; `max_chars` acota cada sección (0 = sin tope).
pub fn extract(html: &str, ticker: &str, max_chars: usize) -> FundExtract {
    let lines: Vec<Line> = html_to_lines(html)
        .into_iter()
        .map(|mut l| {
            l.text = l.text.trim().to_string();
            l
        })
        .filter(|l| !l.text.is_empty() && !is_page_number(&l.text))
        .collect();
    let document_chars = lines.iter().map(|l| l.text.chars().count() + 1).sum();
    let mentions_ticker = lines.iter().any(|l| contains_word(&l.text, ticker));
    let norms: Vec<String> = lines.iter().map(|l| normalize(&l.text)).collect();
    let heads: Vec<(usize, Head)> = lines.iter().zip(&norms).enumerate().filter_map(|(i, (l, n))| classify(l, n).map(|h| (i, h))).collect();
    // «Portfolio Turnover. The Fund may pay…»: párrafo con el título al principio.
    let lead: Vec<usize> = lines
        .iter()
        .zip(&norms)
        .enumerate()
        .filter(|(_, (l, n))| !l.table && l.text.chars().count() > 110 && n.starts_with("portfolio turnover"))
        .map(|(i, _)| i)
        .collect();

    // `heads` va ordenado por posición: búsqueda binaria en vez de recorrerlo entero en cada cabecera
    // (con miles de «Investment Objective» sin cuerpo era O(n²)).
    let end_of = |from: usize| {
        let k = heads.partition_point(|(i, _)| *i <= from);
        heads.get(k).map_or(lines.len(), |(i, _)| *i)
    };
    let body = |s: usize, e: usize| lines[s..e].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");

    let mut sections = Vec::new();
    for key in KEYS {
        // Primer candidato con cuerpo suficiente: salta las entradas del índice.
        let mut found: Option<(String, String)> = None;
        for (i, h) in &heads {
            if *h != Head::Key(key) {
                continue;
            }
            let text = body(i + 1, end_of(*i));
            if text.chars().count() >= MIN_BODY {
                found = Some((lines[*i].text.clone(), text));
                break;
            }
        }
        if found.is_none() && *key == "turnover" {
            found = lead.first().map(|&i| {
                let stop = end_of(i).min(i + TURNOVER_LINES);
                ("Portfolio Turnover".to_string(), body(i, stop))
            });
        }
        if found.is_none() && *key == "turnover" {
            // Fideicomisos como SPY: la frase de la rotación va dentro de la sección de inversiones.
            found = norms.iter().position(|n| n.contains("portfolio turnover rate was")).map(|i| ("Portfolio Turnover".to_string(), lines[i].text.clone()));
        }
        let Some((title, text)) = found else { continue };
        let original_chars = text.chars().count();
        let (text, truncated) = if max_chars > 0 && original_chars > max_chars { (text.chars().take(max_chars).collect(), true) } else { (text, false) };
        sections.push(ProspectusSection { key: (*key).to_string(), title, text, parent: None, truncated, original_chars });
    }
    let missing = KEYS.iter().filter(|k| !sections.iter().any(|s| s.key == **k)).map(|k| (*k).to_string()).collect();
    let fund_name = lines
        .iter()
        .take(25)
        .find(|l| !l.table && !l.text.contains(':') && l.text.chars().count() <= 90 && (l.text.contains("ETF") || l.text.contains("Fund") || l.text.contains("Trust")) && !l.text.to_lowercase().starts_with("form "))
        .map(|l| l.text.clone());
    FundExtract { sections, missing, document_chars, mentions_ticker, fund_name }
}

impl FundExtract {
    /// Lo mínimo para archivar: objetivo y al menos otra sección.
    pub fn usable(&self) -> bool {
        self.sections.iter().any(|s| s.key == "objective") && self.sections.len() >= 2
    }
}
