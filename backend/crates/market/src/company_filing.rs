//! Informes periódicos de una empresa cotizada (10-K, 10-Q, 20-F) y su nota de resultados (EX-99.1
//! de un 8-K con Item 2.02) en secciones de texto limpio. Reutiliza `html::html_to_lines` y
//! `prospectus::normalize`; mismo patrón que `fund_prospectus.rs` (extractor puro, sin red).
//!
//! Un informe se parte por sus cabeceras `Item N`. La clave del extractor es el índice: un 10-K lista
//! «Item 1A. Risk Factors … 12» en su tabla de contenidos, y esa línea no es la sección. Cada
//! cabecera termina en la siguiente de OTRO número de Item (las cabeceras repetidas en cada página,
//! «Item 7 (continued)», no cortan), así que la entrada del índice, seguida de otra entrada, queda sin
//! cuerpo y se descarta; vale la primera aparición con cuerpo sustancial. Lo que no se localiza
//! se declara en `missing`; nunca se inventa texto. Todo el texto es dato no fiable: se extrae,
//! no se interpreta.
//!
//! Costes: una pasada por las líneas, prefijos de longitudes y un vector de finales calculado hacia
//! atrás; ninguna cabecera repetida un millón de veces da coste cuadrático (ver B1 en
//! docs/notes/security.md).
//!
//! Límites conocidos: las memorias integradas sin cabeceras `Item` (el 20-F de ASML) y los 10-K que
//! sustituyen el cuerpo por un índice de referencias cruzadas no se parten: salen en `missing`.

use domain::ProspectusSection;

use crate::{
    html::{Line, html_to_lines},
    prospectus::normalize,
};

/// Tope de caracteres por sección que usa el servicio (el archivo RAG las trocea de todos modos).
pub const MAX_SECTION_CHARS: usize = 150_000;
/// `business` es una descripción larga que el resto del informe ya resume: solo se guarda el principio.
const MAX_BUSINESS_CHARS: usize = 20_000;
/// Cuerpo mínimo (caracteres) para tomar una cabecera por la sección y no por una remisión
/// («Reference is made to…», 160-210 caracteres en XOM y JPM).
const MIN_BODY: usize = 500;
/// Un 10-Q casi siempre remite al 10-K en su Item 1A («no material changes», «refer to…», 150-1300
/// caracteres): solo cuenta con contenido propio si añade factores de riesgo, que van con título en
/// negrita (o, sin formato, son muy largos).
const MIN_QUARTERLY_RISK: usize = 1000;
const LONG_BODY: usize = 4000;
/// Una nota de resultados más corta que esto no es una nota de resultados.
const MIN_RELEASE: usize = 200;
/// Cabeceras de una sola línea: más largo es una frase que empieza por «Item 7A…».
const MAX_HEAD_BYTES: usize = 400;
const MAX_UNFORMATTED_HEAD_BYTES: usize = 160;

/// Qué tipo de documento es.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// 10-K: informe anual de un emisor de EE. UU.
    TenK,
    /// 10-Q: informe trimestral.
    TenQ,
    /// 20-F: informe anual de un emisor extranjero.
    TwentyF,
    /// EX-99.1 de un 8-K con Item 2.02: nota de prensa de resultados.
    Release,
}

impl Report {
    /// Del formulario de EDGAR (`10-K`, `10-Q`, `20-F`, `8-K`).
    pub fn from_form(form: &str) -> Option<Self> {
        match form {
            "10-K" => Some(Self::TenK),
            "10-Q" => Some(Self::TenQ),
            "20-F" => Some(Self::TwentyF),
            "8-K" => Some(Self::Release),
            _ => None,
        }
    }

    /// Claves que se intentan extraer, en el orden del documento.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Self::TenK => &["business", "risk_factors", "mdna", "market_risk"],
            Self::TenQ => &["mdna", "risk_factors"],
            Self::TwentyF => &["risk_factors", "operating_review"],
            Self::Release => &["earnings_release"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompanyExtract {
    pub sections: Vec<ProspectusSection>,
    pub missing: Vec<String>,
    /// Caracteres del texto limpio del documento completo.
    pub document_chars: usize,
}

/// Qué cabecera `Item` es cada sección: número de Item, inicio del título (normalizado) y cuerpo mínimo.
struct Spec {
    key: &'static str,
    item: &'static str,
    title: &'static str,
    min: usize,
    /// El cuerpo debe traer algún subtítulo en negrita o pasar de `LONG_BODY` (descarta las remisiones largas).
    titled: bool,
}

const fn spec(key: &'static str, item: &'static str, title: &'static str, min: usize) -> Spec {
    Spec { key, item, title, min, titled: false }
}

fn specs(report: Report) -> Vec<Spec> {
    match report {
        Report::TenK => vec![
            spec("business", "1", "business", MIN_BODY),
            spec("risk_factors", "1a", "risk factors", MIN_BODY),
            spec("mdna", "7", "management s discussion", MIN_BODY),
            spec("market_risk", "7a", "quantitative and qualitative", MIN_BODY),
        ],
        Report::TenQ => vec![
            spec("mdna", "2", "management s discussion", MIN_BODY),
            Spec { titled: true, ..spec("risk_factors", "1a", "risk factors", MIN_QUARTERLY_RISK) },
        ],
        Report::TwentyF => vec![spec("operating_review", "5", "operating and financial review", MIN_BODY)],
        Report::Release => Vec::new(),
    }
}

/// Cabecera `Item N` (en una línea o en una fila de tabla).
struct Head {
    line: usize,
    /// Número en minúsculas: `1`, `1a`, `7a`, `16`.
    num: String,
    /// Texto tras el número, sin separadores (`Risk Factors | 12`); vacío si el título va en la línea siguiente.
    rest: String,
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.as_bytes().get(..prefix.len()).is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// `Item 1A. Risk Factors` -> `("1a", "Risk Factors")`. Admite el prefijo `Part II,`. Rechaza una frase
/// que empieza por «Item 7A of the…» (tras el número, un título empieza por mayúscula, cifra o `[`).
fn parse_item(text: &str) -> Option<(String, &str)> {
    let mut s = text;
    if starts_with_ci(s, "part ") {
        let r = &s[5..];
        let end = r.find(|c: char| !matches!(c.to_ascii_lowercase(), 'i' | 'v' | 'x'))?;
        if end == 0 {
            return None;
        }
        s = r[end..].trim_start_matches(|c: char| c.is_whitespace() || ",.:-–—|".contains(c));
    }
    if !starts_with_ci(s, "item") {
        return None;
    }
    let r = &s[4..];
    let r2 = r.trim_start();
    if r2.len() == r.len() {
        return None;
    }
    let digits = r2.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let mut num = r2[..digits].to_string();
    let mut tail = &r2[digits..];
    // Letra del Item (`1A`, `7A`, `3D`): solo si no es la inicial de la palabra siguiente.
    if let Some(c) = tail.chars().next()
        && matches!(c, 'a'..='e' | 'A'..='E')
        && tail[1..].chars().next().is_none_or(|n| !n.is_alphanumeric())
    {
        num.push(c.to_ascii_lowercase());
        tail = &tail[1..];
    }
    if tail.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }
    let rest = tail.trim_start_matches(|c: char| c.is_whitespace() || ".:-–—|".contains(c));
    if rest.chars().next().is_some_and(char::is_lowercase) {
        return None;
    }
    Some((num, rest))
}

fn is_page_number(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || t.len() > 5 {
        return false;
    }
    let body = t.strip_prefix(['F', 'f', 'A', 'a', 'S', 's']).map_or(t, |r| r.trim_start_matches('-'));
    (!body.is_empty() && body.len() <= 3 && body.bytes().all(|b| b.is_ascii_digit()))
        || (t.len() <= 4 && t.bytes().all(|b| matches!(b.to_ascii_lowercase(), b'i' | b'v' | b'x')))
}

/// `12`, `202`, `9-31`: la columna de páginas de un índice.
fn is_page_ref(cell: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    let cell = cell.trim();
    digits(cell) || cell.split_once(['-', '–', ',']).is_some_and(|(a, b)| digits(a.trim()) && digits(b.trim()))
}

/// Fila de un índice: «Item 1A. | Risk Factors | 12», «Overview | 43». Un 10-K lo tiene al principio
/// y los 20-F y 10-Q grandes añaden las entradas de cada apartado, que suman cuerpo de sobra.
fn is_toc_row(l: &Line) -> bool {
    l.table && l.text.rsplit_once(" | ").is_some_and(|(_, last)| is_page_ref(last))
}

/// Paginación que el HTML repite en cada página: número suelto, «Table of Contents» y el pie
/// «Apple Inc. | 2026 Form 10-K | 5».
fn is_noise(l: &Line) -> bool {
    let t = l.text.as_str();
    if t.is_empty() || (!l.table && is_page_number(t)) {
        return true;
    }
    if t.len() > 100 {
        return false;
    }
    let low = t.to_ascii_lowercase();
    low.starts_with("table of contents")
        || (low.contains(" | ")
            && (low.contains("form 10-k") || low.contains("form 10-q") || low.contains("form 20-f"))
            && low.rsplit(" | ").next().is_some_and(|last| last.len() <= 3 && last.bytes().all(|b| b.is_ascii_digit())))
}

/// Primera línea con letra tras `Item N`: el título cuando la cabecera va en dos líneas.
fn title_matches(h: &Head, lines: &[Line], title: &str) -> Option<usize> {
    let starts = |s: &str| {
        let n = normalize(s);
        n.strip_prefix(title).is_some_and(|r| r.is_empty() || r.starts_with(' '))
    };
    if !h.rest.is_empty() {
        return starts(&h.rest).then_some(0);
    }
    // «Item 1A.» solo en una línea y «Risk Factors» en la siguiente.
    lines.get(h.line + 1).filter(|l| starts(&l.text)).map(|_| 1)
}

fn clip(text: String, max: usize) -> (String, bool, usize) {
    let original = text.chars().count();
    if max > 0 && original > max {
        (text.chars().take(max).collect(), true, original)
    } else {
        (text, false, original)
    }
}

fn section(key: &str, title: String, text: String, max: usize) -> ProspectusSection {
    let (text, truncated, original_chars) = clip(text, max);
    ProspectusSection { key: key.to_string(), title, text, parent: None, truncated, original_chars }
}

fn join(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// Extrae las secciones de `html`. `max_chars` acota cada sección (0 = sin tope); `business` nunca
/// pasa de 20 000.
pub fn extract(report: Report, html: &str, max_chars: usize) -> CompanyExtract {
    let lines: Vec<Line> = html_to_lines(html).into_iter().filter(|l| !is_noise(l)).collect();
    // cum[i] = caracteres (con el salto de línea) de lines[..i].
    let mut cum = Vec::with_capacity(lines.len() + 1);
    cum.push(0usize);
    for l in &lines {
        cum.push(cum[cum.len() - 1] + l.text.chars().count() + 1);
    }
    let document_chars = cum[lines.len()];
    let mut sections = Vec::new();
    if report == Report::Release {
        sections.extend(release(&lines, max_chars));
    } else {
        let heads: Vec<Head> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.text.len() <= MAX_HEAD_BYTES && (l.bold || l.emph || l.table || l.text.len() <= MAX_UNFORMATTED_HEAD_BYTES)
            })
            .filter_map(|(i, l)| parse_item(&l.text).map(|(num, rest)| Head { line: i, num, rest: rest.to_string() }))
            .collect();
        // Dónde acaba la sección que abre cada cabecera: la siguiente de otro número de Item.
        let mut ends = vec![lines.len(); heads.len()];
        for k in (0..heads.len()).rev() {
            ends[k] = match heads.get(k + 1) {
                None => lines.len(),
                Some(next) if next.num != heads[k].num => next.line,
                Some(_) => ends[k + 1],
            };
        }
        for sp in specs(report) {
            let found = heads.iter().zip(&ends).find_map(|(h, &end)| {
                if h.num != sp.item {
                    return None;
                }
                let skip = title_matches(h, &lines, sp.title)?;
                let start = h.line + 1 + skip;
                // Entrada del índice (con sus subentradas debajo) y no la sección.
                let toc = is_toc_row(&lines[h.line]) || lines.get(start).is_some_and(is_toc_row);
                let size = cum[end].saturating_sub(cum[start.min(end)]);
                let own = !sp.titled || size >= LONG_BODY || lines[start.min(end)..end].iter().any(|l| l.bold || l.emph);
                (!toc && start < end && size >= sp.min && own).then_some((h.line, start, end))
            });
            if let Some((title_line, start, end)) = found {
                let max = if sp.key == "business" && (max_chars == 0 || max_chars > MAX_BUSINESS_CHARS) { MAX_BUSINESS_CHARS } else { max_chars };
                let title = lines[title_line..start].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
                sections.push(section(sp.key, title, join(&lines[start..end]), max));
            }
        }
        if report == Report::TwentyF {
            sections.extend(twenty_f_risk_factors(&lines, &heads, &ends, &cum, max_chars));
            sections.sort_by_key(|s| report.keys().iter().position(|k| *k == s.key));
        }
    }
    let missing = report.keys().iter().filter(|k| !sections.iter().any(|s| s.key == **k)).map(|k| (*k).to_string()).collect();
    CompanyExtract { sections, missing, document_chars }
}

/// 20-F, Item 3.D: «Risk Factors» es una subcabecera dentro del Item 3 («Key Information») y llega
/// hasta el Item 4. También se acepta la cabecera directa «Item 3.D. Risk Factors».
fn twenty_f_risk_factors(lines: &[Line], heads: &[Head], ends: &[usize], cum: &[usize], max_chars: usize) -> Option<ProspectusSection> {
    let is_sub = |l: &Line| {
        l.text.len() <= 60 && matches!(normalize(&l.text).as_str(), "risk factors" | "d risk factors" | "3 d risk factors")
    };
    // Una sola pasada por las líneas; por cabecera solo se miran los primeros candidatos (búsqueda binaria).
    let subs: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| is_sub(l)).map(|(i, _)| i).collect();
    let substantive = |start: usize, end: usize| start < end && cum[end] - cum[start] >= MIN_BODY;
    for (h, &end) in heads.iter().zip(ends) {
        let direct = h.num == "3d" && normalize(&h.rest).starts_with("risk factors");
        let direct = direct || (h.num == "3" && normalize(&h.rest).starts_with("d risk factors"));
        if direct && !is_toc_row(&lines[h.line]) && substantive(h.line + 1, end) {
            return Some(section("risk_factors", lines[h.line].text.clone(), join(&lines[h.line + 1..end]), max_chars));
        }
        let key_info = h.num == "3" && !is_toc_row(&lines[h.line]) && title_matches(h, lines, "key information").is_some();
        if !key_info {
            continue;
        }
        let first = subs.partition_point(|&i| i <= h.line);
        for &i in subs[first..].iter().take(3).take_while(|&&i| i < end) {
            if substantive(i + 1, end) {
                return Some(section("risk_factors", lines[i].text.clone(), join(&lines[i + 1..end]), max_chars));
            }
        }
    }
    None
}

/// EX-99.1: el documento entero (las tablas ya son líneas «concepto | cifra | cifra»).
fn release(lines: &[Line], max_chars: usize) -> Option<ProspectusSection> {
    let body: Vec<&Line> = lines.iter().filter(|l| !(l.text.len() <= 20 && normalize(&l.text) == "exhibit 99 1")).collect();
    let text = body.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    if text.chars().count() < MIN_RELEASE {
        return None;
    }
    let title = body.iter().find(|l| !l.table && l.text.len() <= 200).map_or_else(|| "Exhibit 99.1".to_string(), |l| l.text.clone());
    Some(section("earnings_release", title, text, max_chars))
}
