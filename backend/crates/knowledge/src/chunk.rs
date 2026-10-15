//! Troceado por secciones y oraciones: ~300–600 tokens estimados con solape.
//!
//! Reglas: (1) nunca se mezcla texto de dos secciones; (2) los cortes caen en fin de oración
//! (salvo una oración más larga que `max_tokens`, que se parte por palabras); (3) cada fragmento
//! repite al principio las últimas oraciones del anterior hasta `overlap_tokens`; (4) una cola
//! diminuta se funde con el fragmento anterior si cabe; (5) es determinista.

use crate::text::{estimate_tokens, normalize_text};

#[derive(Debug, Clone)]
pub struct ChunkConfig {
    pub target_tokens: usize,
    pub max_tokens: usize,
    pub overlap_tokens: usize,
    /// Una cola nueva por debajo de esto se funde con el fragmento anterior (si cabe en `max`).
    pub min_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self { target_tokens: 400, max_tokens: 600, overlap_tokens: 60, min_tokens: 80 }
    }
}

impl ChunkConfig {
    /// Corrige combinaciones incoherentes (objetivo ≤ máximo, solape < objetivo).
    pub fn sanitized(mut self) -> Self {
        self.max_tokens = self.max_tokens.clamp(32, 4000);
        self.target_tokens = self.target_tokens.clamp(16, self.max_tokens);
        self.overlap_tokens = self.overlap_tokens.min(self.target_tokens / 2);
        self.min_tokens = self.min_tokens.min(self.target_tokens);
        self
    }
}

/// Sección de un documento (p. ej. «Risk Factors»).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: Option<String>,
    pub text: String,
}

impl Section {
    pub fn new(heading: Option<&str>, text: impl Into<String>) -> Self {
        Self { heading: heading.map(str::to_string), text: text.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDraft {
    pub ordinal: usize,
    pub section: Option<String>,
    pub text: String,
    pub tokens: usize,
}

#[derive(Debug, Clone)]
struct Unit {
    text: String,
    tokens: usize,
    /// Empieza un párrafo nuevo (se une con salto de línea en vez de espacio).
    para: bool,
}

pub fn chunk_sections(sections: &[Section], cfg: &ChunkConfig) -> Vec<ChunkDraft> {
    let cfg = cfg.clone().sanitized();
    let mut out: Vec<ChunkDraft> = Vec::new();
    for s in sections {
        let units = units_of(&normalize_text(&s.text), &cfg);
        for text in pack(units, &cfg) {
            out.push(ChunkDraft { ordinal: out.len(), section: s.heading.clone(), tokens: estimate_tokens(&text), text });
        }
    }
    out
}

fn units_of(text: &str, cfg: &ChunkConfig) -> Vec<Unit> {
    let mut units = Vec::new();
    for para in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        let mut first = true;
        for sentence in split_sentences(para) {
            for piece in split_long(&sentence, cfg.max_tokens) {
                units.push(Unit { tokens: estimate_tokens(&piece), text: piece, para: first });
                first = false;
            }
        }
    }
    units
}

/// Parte por palabras una oración que no cabe en `max_tokens`.
fn split_long(s: &str, max_tokens: usize) -> Vec<String> {
    if estimate_tokens(s) <= max_tokens {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        // Una «palabra» gigante (sin espacios) se corta por caracteres.
        let pieces: Vec<String> = if estimate_tokens(w) > max_tokens {
            let step = (max_tokens * 7 / 2).max(1);
            w.chars().collect::<Vec<_>>().chunks(step).map(|c| c.iter().collect()).collect()
        } else {
            vec![w.to_string()]
        };
        for p in pieces {
            let candidate_len = if cur.is_empty() { p.clone() } else { format!("{cur} {p}") };
            if !cur.is_empty() && estimate_tokens(&candidate_len) > max_tokens {
                out.push(std::mem::take(&mut cur));
                cur = p;
            } else {
                cur = candidate_len;
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

const ABBREVIATIONS: &[&str] = &[
    "inc", "corp", "co", "ltd", "llc", "plc", "sa", "sl", "nv", "ag", "no", "nos", "mr", "mrs", "ms", "dr", "sr", "sra", "srta", "dna",
    "vs", "etc", "approx", "est", "fig", "al", "jr", "st", "u.s", "e.g", "i.e", "p.ej", "ej", "pág", "pag", "núm", "num", "vol", "ee.uu",
    "cía", "cia", "ud", "uds", "av", "dept", "gen", "gov", "sec", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct",
    "nov", "dec",
];

fn is_abbrev(prefix: &str) -> bool {
    let word = prefix.split_whitespace().next_back().unwrap_or("").trim_start_matches(['(', '"', '“', '\'']);
    let w = word.to_lowercase();
    if w.is_empty() {
        return false;
    }
    let single_letter = w.chars().count() == 1 && w.chars().all(char::is_alphabetic);
    single_letter || ABBREVIATIONS.contains(&w.as_str()) || (w.contains('.') && w.chars().any(char::is_alphabetic))
}

fn starts_sentence(c: char) -> bool {
    c.is_uppercase() || c.is_ascii_digit() || matches!(c, '"' | '“' | '(' | '¿' | '¡' | '\'' | '«')
}

pub(crate) fn split_sentences(p: &str) -> Vec<String> {
    let chars: Vec<(usize, char)> = p.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (bi, c) = chars[i];
        if matches!(c, '.' | '!' | '?' | '…') {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '"' | '\'' | ')' | '”' | '»' | '’') {
                j += 1;
            }
            if j < chars.len() && chars[j].1.is_whitespace() {
                let mut k = j;
                while k < chars.len() && chars[k].1.is_whitespace() {
                    k += 1;
                }
                if k < chars.len() && starts_sentence(chars[k].1) && !(c == '.' && is_abbrev(&p[start..bi])) {
                    let s = p[start..chars[j].0].trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                    }
                    start = chars[k].0;
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    let tail = p[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn assemble(units: &[Unit]) -> String {
    let mut s = String::new();
    for (i, u) in units.iter().enumerate() {
        if i > 0 {
            s.push(if u.para { '\n' } else { ' ' });
        }
        s.push_str(&u.text);
    }
    s
}

fn pack(units: Vec<Unit>, cfg: &ChunkConfig) -> Vec<String> {
    // (unidades, cuántas son solape heredado)
    let mut chunks: Vec<(Vec<Unit>, usize)> = Vec::new();
    let mut cur: Vec<Unit> = Vec::new();
    let mut cur_tok = 0usize;
    let mut seed = 0usize;
    for u in units {
        if !cur.is_empty() && cur.len() > seed && cur_tok + u.tokens > cfg.target_tokens {
            let mut ov: Vec<Unit> = Vec::new();
            let mut t = 0usize;
            for x in cur.iter().rev() {
                if t + x.tokens > cfg.overlap_tokens {
                    break;
                }
                t += x.tokens;
                ov.push(x.clone());
            }
            ov.reverse();
            chunks.push((std::mem::take(&mut cur), seed));
            seed = ov.len();
            cur_tok = t;
            cur = ov;
        }
        // El solape heredado se sacrifica antes que pasarse del máximo.
        while !cur.is_empty() && cur_tok + u.tokens > cfg.max_tokens && seed > 0 {
            let r = cur.remove(0);
            cur_tok -= r.tokens;
            seed -= 1;
        }
        cur_tok += u.tokens;
        cur.push(u);
    }
    if cur.len() > seed {
        chunks.push((cur, seed));
    }
    // Cola diminuta: se funde con el fragmento anterior si cabe.
    if chunks.len() >= 2 {
        let (last, last_seed) = chunks.last().unwrap();
        let new_part: Vec<Unit> = last[*last_seed..].to_vec();
        let new_tok: usize = new_part.iter().map(|u| u.tokens).sum();
        let prev_tok: usize = chunks[chunks.len() - 2].0.iter().map(|u| u.tokens).sum();
        if new_tok < cfg.min_tokens && prev_tok + new_tok <= cfg.max_tokens {
            chunks.pop();
            chunks.last_mut().unwrap().0.extend(new_part);
        }
    }
    chunks.iter().map(|(u, _)| assemble(u)).collect()
}

/// Cabeceras típicas de un folleto (S-1/F-1/424B) en mayúsculas.
const KNOWN_HEADINGS: &[&str] = &[
    "PROSPECTUS SUMMARY", "SUMMARY", "THE OFFERING", "RISK FACTORS", "USE OF PROCEEDS", "DIVIDEND POLICY", "CAPITALIZATION", "DILUTION",
    "BUSINESS", "MANAGEMENT", "EXECUTIVE COMPENSATION", "PRINCIPAL STOCKHOLDERS", "PRINCIPAL AND SELLING STOCKHOLDERS", "UNDERWRITING",
    "DESCRIPTION OF CAPITAL STOCK", "SHARES ELIGIBLE FOR FUTURE SALE", "LEGAL MATTERS", "EXPERTS", "SELECTED FINANCIAL DATA",
    "MANAGEMENT'S DISCUSSION AND ANALYSIS OF FINANCIAL CONDITION AND RESULTS OF OPERATIONS", "CERTAIN RELATIONSHIPS AND RELATED PARTY TRANSACTIONS",
];

fn is_heading(line: &str) -> bool {
    let t = line.trim();
    let n = t.chars().count();
    if !(3..=110).contains(&n) || t.ends_with(['.', ',', ';']) && !t.to_uppercase().starts_with("ITEM") {
        return false;
    }
    if let Some(rest) = t.strip_prefix('#') {
        return !rest.trim_start_matches('#').trim().is_empty();
    }
    let up = t.to_uppercase();
    if KNOWN_HEADINGS.contains(&up.trim_end_matches(':')) && t == up {
        return true;
    }
    let item = (up.starts_with("ITEM ") || up.starts_with("PART ")) && up.split_whitespace().nth(1).is_some_and(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit() || "IVX".contains(c)));
    if item {
        return true;
    }
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 6 && t.split_whitespace().count() >= 2 && letters.iter().all(|c| c.is_uppercase())
}

/// Parte un texto plano en secciones por cabeceras reconocibles (`ITEM 1A. RISK FACTORS`, líneas
/// en mayúsculas, `# Título`). Sin cabeceras devuelve una sola sección sin título. Si el origen
/// ya trae secciones estructuradas, es preferible pasarlas directamente.
pub fn sections_from_text(text: &str) -> Vec<Section> {
    let norm = normalize_text(text);
    let mut sections: Vec<Section> = Vec::new();
    let mut heading: Option<String> = None;
    let mut buf: Vec<&str> = Vec::new();
    let flush = |heading: &Option<String>, buf: &mut Vec<&str>, sections: &mut Vec<Section>| {
        let body = buf.join("\n");
        buf.clear();
        if !body.trim().is_empty() {
            sections.push(Section { heading: heading.clone(), text: body });
        }
    };
    for line in norm.lines() {
        if is_heading(line) {
            flush(&heading, &mut buf, &mut sections);
            heading = Some(line.trim().trim_start_matches('#').trim().to_string());
        } else {
            buf.push(line);
        }
    }
    flush(&heading, &mut buf, &mut sections);
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_text(sentences: usize) -> String {
        (1..=sentences)
            .map(|i| format!("La compañía informó de que el ingreso trimestral número {i} alcanzó {} millones de dólares en el periodo.", 100 + i))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn short_text_is_one_chunk() {
        let c = chunk_sections(&[Section::new(None, "Apple sube un 3% tras resultados.")], &ChunkConfig::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].ordinal, 0);
        assert_eq!(c[0].text, "Apple sube un 3% tras resultados.");
    }

    #[test]
    fn long_text_respects_limits_overlap_and_loses_nothing() {
        let cfg = ChunkConfig::default();
        let text = long_text(120);
        let chunks = chunk_sections(&[Section::new(Some("Resultados"), text.clone())], &cfg);
        assert!(chunks.len() >= 4, "{} fragmentos", chunks.len());
        for (i, c) in chunks.iter().enumerate() {
            assert!(c.tokens <= cfg.max_tokens, "fragmento {i}: {} tokens", c.tokens);
            assert_eq!(c.ordinal, i);
            assert_eq!(c.section.as_deref(), Some("Resultados"));
        }
        // Solape: la última oración de cada fragmento reaparece al principio del siguiente.
        for w in chunks.windows(2) {
            let first = split_sentences(&w[1].text).remove(0);
            assert!(w[0].text.ends_with(&first) || w[0].text.contains(&first), "sin solape:\n{}\n---\n{}", w[0].text, w[1].text);
            let last = split_sentences(&w[0].text).pop().unwrap();
            assert!(w[1].text.contains(&last), "la última oración no se repite");
        }
        // Todas las oraciones aparecen en algún fragmento y los cortes son de oración.
        for s in split_sentences(&text) {
            assert!(chunks.iter().any(|c| c.text.contains(&s)), "se perdió: {s}");
        }
    }

    #[test]
    fn sections_never_mix() {
        let cfg = ChunkConfig::default();
        let c = chunk_sections(&[Section::new(Some("A"), "Texto de A."), Section::new(Some("B"), "Texto de B."), Section::new(Some("C"), "  ")], &cfg);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].section.as_deref(), c[1].section.as_deref()), (Some("A"), Some("B")));
        assert!(!c[0].text.contains('B'));
    }

    #[test]
    fn abbreviations_and_decimals_do_not_split() {
        let s = split_sentences("Apple Inc. reported revenue of $3.5 billion in the U.S. market. Shares rose 2%. Dr. Smith agreed.");
        assert_eq!(s, vec!["Apple Inc. reported revenue of $3.5 billion in the U.S. market.", "Shares rose 2%.", "Dr. Smith agreed."]);
    }

    #[test]
    fn oversized_sentence_is_split_by_words_and_utf8_is_safe() {
        let cfg = ChunkConfig { target_tokens: 50, max_tokens: 60, overlap_tokens: 10, min_tokens: 10 };
        let huge = format!("{} fin.", "ñandú€ ".repeat(300));
        let c = chunk_sections(&[Section::new(None, huge)], &cfg);
        assert!(c.len() > 3);
        assert!(c.iter().all(|c| c.tokens <= 60));
        let giant = "界".repeat(5000);
        let c = chunk_sections(&[Section::new(None, giant.clone())], &cfg);
        assert_eq!(c.iter().map(|c| c.text.chars().count()).sum::<usize>(), 5000);
    }

    #[test]
    fn tiny_tail_is_merged_when_it_fits() {
        let cfg = ChunkConfig { target_tokens: 100, max_tokens: 160, overlap_tokens: 0, min_tokens: 40 };
        // ~100 tokens por bloque + una cola de 1 oración.
        let body = format!("{} Fin.", "Palabra ".repeat(40).trim_end().to_string() + ".");
        let c = chunk_sections(&[Section::new(None, format!("{body} {body}"))], &cfg);
        assert!(c.iter().all(|c| c.tokens <= 160));
        assert!(!c.last().unwrap().text.trim().eq("Fin."));
    }

    #[test]
    fn chunking_is_deterministic() {
        let t = long_text(60);
        let a = chunk_sections(&[Section::new(None, t.clone())], &ChunkConfig::default());
        let b = chunk_sections(&[Section::new(None, t)], &ChunkConfig::default());
        assert_eq!(a, b);
    }

    #[test]
    fn detects_sec_style_headings() {
        let text = "PROSPECTUS SUMMARY\nWe are a company.\nITEM 1A. RISK FACTORS\nRisks exist.\nUSE OF PROCEEDS\nWe will use funds.\nThis sentence mentions NASDAQ and ends.";
        let s = sections_from_text(text);
        let heads: Vec<_> = s.iter().map(|s| s.heading.as_deref().unwrap_or("-")).collect();
        assert_eq!(heads, vec!["PROSPECTUS SUMMARY", "ITEM 1A. RISK FACTORS", "USE OF PROCEEDS"]);
        assert!(s[2].text.contains("NASDAQ"));
        let none = sections_from_text("Solo un párrafo normal sin cabeceras.");
        assert_eq!(none.len(), 1);
        assert!(none[0].heading.is_none());
    }
}
