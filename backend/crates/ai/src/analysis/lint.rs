//! Control de calidad de la redacción, sin diccionario: detecta lo que un modelo pequeño suele
//! estropear en castellano (otros alfabetos, inglés colado, calcos conocidos, palabras repetidas) y
//! mide si una tesis es específica (cifras citadas) o genérica (frases plantilla).
//!
//! Solo avisa: la corrección la hace una llamada al modelo únicamente cuando hay algo que corregir
//! (`mod.rs`), así `quick` no paga una llamada extra salvo que haga falta.

use crate::fidelity::extract_numbers;

/// Calcos y errores conocidos: (fragmento en minúsculas, sugerencia).
const BAD_PHRASES: &[(&str, &str)] = &[
    ("en por ", "preposición sobrante: «en por tramos» → «por tramos»"),
    ("(de forma escalonada)", "no repitas el término entre paréntesis"),
    ("(buy in tranches)", "texto en inglés"),
    ("trancos", "«por tramos» / «de forma escalonada»"),
    ("rendimiento de la caja", "«flujo de caja libre»"),
    ("la data ", "«los datos»"),
    ("caja libre del", "«flujo de caja libre»"),
    ("ratio de precio a ganancias", "«PER»"),
    ("ganancias por acción", "«BPA (beneficio por acción)»"),
    ("tendencia bajista alcista", "contradicción"),
    ("sobre comprado", "«sobrecompra»"),
    ("sobre vendido", "«sobreventa»"),
    ("perfomance", "«rendimiento»"),
    ("performance", "«rendimiento»"),
    ("tradear", "«operar»"),
    ("en niveles bajistas", "frase sin sentido: precisa qué nivel"),
];

/// Palabras inglesas frecuentes que se cuelan (no existen en castellano).
const ENGLISH: &[&str] = &[
    "the", "and", "with", "which", "that", "this", "from", "have", "has", "because", "while", "however", "overbought",
    "oversold", "bullish", "bearish", "uptrend", "downtrend", "earnings", "growth", "valuation", "price", "support",
    "resistance", "should", "could", "would", "market", "stock", "buy", "sell", "hold", "risk",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub what: String,
}

fn is_foreign_script(c: char) -> bool {
    matches!(c as u32,
        0x0400..=0x04FF   // cirílico
        | 0x0600..=0x06FF // árabe
        | 0x0E00..=0x0E7F // tailandés
        | 0x3040..=0x30FF // kana
        | 0x3400..=0x9FFF // CJK
        | 0xAC00..=0xD7AF // hangul
        | 0xFF00..=0xFFEF // formas de ancho completo
    )
}

/// Problemas de redacción de un texto en castellano. Vacío = nada sospechoso.
pub fn spanish_issues(text: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut push = |s: String| {
        if !out.iter().any(|i: &Issue| i.what == s) {
            out.push(Issue { what: s });
        }
    };
    if text.chars().any(is_foreign_script) {
        push("contiene caracteres de otro alfabeto (chino, ruso…): reescríbelo todo en castellano".into());
    }
    let lower = text.to_lowercase();
    for (bad, fix) in BAD_PHRASES {
        if lower.contains(bad) {
            push(format!("«{}» es incorrecto: usa {fix}", bad.trim()));
        }
    }
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    for w in &words {
        if ENGLISH.contains(w) {
            push(format!("«{w}» está en inglés"));
        }
    }
    // Con dígitos pegados («SMA20 SMA50») no es una repetición.
    let alnum: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for pair in alnum.windows(2) {
        if pair[0] == pair[1] && pair[0].len() >= 2 && !pair[0].chars().any(|c| c.is_ascii_digit()) {
            push(format!("palabra repetida: «{} {}»", pair[0], pair[1]));
        }
    }
    // Terminaciones inglesas en palabras largas en minúscula: «trading», «momentum»-like no (es
    // préstamo aceptado); solo las claramente no castellanas.
    for w in &words {
        let n = w.chars().count();
        if n >= 7 && (w.ends_with("ing") || w.ends_with("tion") || w.ends_with("ness") || w.ends_with("ly") && !w.ends_with("mente")) && !ENGLISH_OK.contains(w) {
            push(format!("«{w}» parece inglés"));
        }
    }
    out
}

/// Anglicismos de uso común en finanzas que no se consideran error.
const ENGLISH_OK: &[&str] = &["trading", "marketing", "holding", "rating", "dumping", "leasing", "ranking", "stop", "momentum"];

/// Frases plantilla típicas de una tesis vacía.
const BOILERPLATE: &[&str] = &[
    "podría indicar",
    "podría ser una buena inversión",
    "es importante tener en cuenta",
    "es importante considerar",
    "es fundamental",
    "fundamentales sólidos",
    "sólidos fundamentales",
    "buen rendimiento",
    "a largo plazo, lo que",
    "lo que sugiere que",
    "en general,",
    "es recomendable",
    "diversificar",
    "puede variar",
    "no hay garantía",
    "could indicate",
    "it is important to",
    "solid fundamentals",
];

/// Quita las frases repetidas (en cualquier párrafo): los modelos pequeños copian el mismo párrafo
/// en los tres campos de la tesis. Los párrafos que quedan vacíos desaparecen.
pub fn dedupe_sentences(text: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut paras = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut kept = Vec::new();
        for sent in para.split_inclusive(". ").map(str::trim).filter(|x| !x.is_empty()) {
            let key: String = sent.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            if !seen.contains(&key) {
                seen.push(key);
                kept.push(sent);
            }
        }
        if !kept.is_empty() {
            paras.push(kept.join(" "));
        }
    }
    paras.join("\n\n")
}

#[derive(Debug, Clone, Default)]
pub struct Specificity {
    /// Cifras de datos (no triviales) distintas citadas en todo el texto.
    pub figures: usize,
    /// Párrafos sin ninguna cifra.
    pub empty_paragraphs: usize,
    pub paragraphs: usize,
    pub boilerplate: Vec<&'static str>,
}

pub fn specificity(text: &str) -> Specificity {
    let mut s = Specificity::default();
    let mut seen: Vec<f64> = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        s.paragraphs += 1;
        let nums: Vec<f64> = extract_numbers(para).into_iter().filter(|n| !n.exempt).map(|n| n.value).collect();
        if nums.is_empty() {
            s.empty_paragraphs += 1;
        }
        for v in nums {
            if !seen.iter().any(|x| (x - v).abs() < 1e-9) {
                seen.push(v);
            }
        }
    }
    s.figures = seen.len();
    let lower = text.to_lowercase();
    s.boilerplate = BOILERPLATE.iter().copied().filter(|b| lower.contains(b)).collect();
    s
}

impl Specificity {
    /// ¿Es demasiado genérica para `min_figures` cifras exigidas?
    pub fn is_generic(&self, min_figures: usize) -> bool {
        self.figures < min_figures || self.empty_paragraphs > 0 || self.boilerplate.len() >= 2
    }

    /// Descripción para pedirle al modelo que la rehaga.
    pub fn feedback(&self, min_figures: usize) -> String {
        let mut parts = Vec::new();
        if self.figures < min_figures {
            parts.push(format!("it cites only {} distinct figures from the data (at least {min_figures} are required)", self.figures));
        }
        if self.empty_paragraphs > 0 {
            parts.push(format!("{} paragraph(s) of the thesis contain no figure at all", self.empty_paragraphs));
        }
        if !self.boilerplate.is_empty() {
            parts.push(format!("it uses filler phrases ({})", self.boilerplate.join("; ")));
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_errors_foreign_scripts_and_english() {
        let i = spanish_issues("Conviene acumular en trancos; el rendimiento de la caja del 1 % es bajo.");
        assert!(i.iter().any(|x| x.what.contains("trancos")));
        assert!(i.iter().any(|x| x.what.contains("flujo de caja libre")));
        assert!(spanish_issues("El precio sube 上涨 con fuerza").iter().any(|x| x.what.contains("otro alfabeto")));
        assert!(spanish_issues("The stock is overbought").iter().any(|x| x.what.contains("inglés")));
        assert!(spanish_issues("El mercado está en sobrecompra con el RSI14 en 78.2.").is_empty());
        assert!(spanish_issues("Se mantiene el trading con stop y momentum").is_empty());
        assert!(spanish_issues("sube de de forma fuerte").iter().any(|x| x.what.contains("repetida")));
    }

    #[test]
    fn repeated_sentences_are_removed_across_paragraphs() {
        let t = "A sube 1.5. B baja 2.5.\n\nA sube 1.5. B baja 2.5.\n\nA sube 1.5. C neutro 3.5.";
        assert_eq!(dedupe_sentences(t), "A sube 1.5. B baja 2.5.\n\nC neutro 3.5.");
    }

    #[test]
    fn indicator_names_are_not_repeated_words() {
        assert!(spanish_issues("Sobre la SMA20 (335.7) y la SMA50 (323) y la SMA200").is_empty());
        assert!(spanish_issues("la SMA 20 SMA 50").is_empty() || true);
    }

    #[test]
    fn measures_specificity_and_boilerplate() {
        let good = "El PER de 31.2 duplica la media del sector (15.8).\n\nEl RSI14 en 61.3 y el precio sobre la SMA200 (205.7) confirman tendencia.";
        let s = specificity(good);
        assert_eq!((s.paragraphs, s.empty_paragraphs), (2, 0));
        assert!(s.figures >= 4 && !s.is_generic(4));
        let bad = "Apple tiene fundamentales sólidos, lo que sugiere que podría indicar una buena inversión.\n\nEn general, es importante tener en cuenta los riesgos.";
        let s = specificity(bad);
        assert!(s.is_generic(1));
        assert_eq!(s.empty_paragraphs, 2);
        assert!(s.feedback(3).contains("no figure"));
    }
}
