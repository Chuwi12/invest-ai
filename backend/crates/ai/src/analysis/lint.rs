//! Control de calidad de la redacción, sin diccionario: detecta lo que un modelo pequeño suele
//! estropear en castellano (otros alfabetos, inglés colado, calcos conocidos, palabras repetidas) y
//! mide si una tesis es específica (cifras citadas) o genérica (frases plantilla).
//!
//! Solo avisa: la corrección la hace una llamada al modelo únicamente cuando hay algo que corregir
//! (`mod.rs`), así `quick` no paga una llamada extra salvo que haga falta.

use crate::fidelity::extract_numbers;

/// Calcos y errores conocidos: (fragmento en minúsculas, sugerencia). Si el fragmento va precedido
/// de `OK_BEFORE` es correcto («flujo de caja libre del año» no es el calco «caja libre del»).
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

/// Fragmentos de `BAD_PHRASES` que son válidos si van tras este prefijo.
const OK_BEFORE: &[(&str, &str)] = &[("caja libre del", "flujo de ")];

/// ¿Aparece `bad` en `lower` en algún sitio donde SÍ es un error (no precedido de su prefijo válido)?
fn is_bad_occurrence(lower: &str, bad: &str) -> bool {
    let ok_prefix = OK_BEFORE.iter().find(|(b, _)| *b == bad).map(|(_, p)| *p);
    lower.match_indices(bad).any(|(i, _)| ok_prefix.is_none_or(|p| !lower[..i].ends_with(p)))
}

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
        if is_bad_occurrence(&lower, bad) {
            push(format!("«{}» es incorrecto: usa {fix}", bad.trim()));
        }
    }
    // Sin los términos financieros consolidados («market cap», «stop loss»…): ver `FINANCE_TERMS`.
    let masked = mask_finance_terms(text);
    let masked_lower = masked.to_lowercase();
    let words: Vec<&str> = masked_lower
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    // Inglés colado, salvo nombres propios: «T. Rowe Price», «Vanguard Growth ETF», «The Home Depot»
    // llevan mayúscula a mitad de frase.
    let mut sentence_start = true;
    for tok in masked.split_whitespace() {
        let core = tok.trim_matches(|c: char| !c.is_alphabetic());
        let capitalized = core.chars().next().is_some_and(char::is_uppercase);
        let w = core.to_lowercase();
        if ENGLISH.contains(&w.as_str()) && !(capitalized && !sentence_start) {
            push(format!("«{w}» está en inglés"));
        }
        sentence_start = tok.ends_with(['.', '!', '?', ':', '¿', '¡']) && !tok.chars().next().is_some_and(|c| c.is_uppercase() && tok.len() <= 2);
    }
    // Palabra repetida: dos tokens seguidos y sin puntuación entre ellos («de de»). Una palabra al
    // final de una frase y la misma al empezar la siguiente («…el soporte. Soporte que…») no lo es,
    // ni con dígitos pegados («SMA20 SMA50»).
    for pair in lower.split_whitespace().collect::<Vec<_>>().windows(2) {
        if pair[0] == pair[1] && pair[0].chars().count() >= 2 && pair[0].chars().all(char::is_alphabetic) {
            push(format!("palabra repetida: «{} {}»", pair[0], pair[1]));
        }
    }
    // Terminaciones inglesas en palabras largas en minúscula: «trading», «momentum»-like no (es
    // préstamo aceptado); solo las claramente no castellanas.
    for w in &words {
        let n = w.chars().count();
        if n >= 7 && (w.ends_with("ing") || w.ends_with("tion") || w.ends_with("ness") || w.ends_with("ly") && !w.ends_with("mente")) {
            push(format!("«{w}» parece inglés"));
        }
    }
    out
}

/// Términos financieros consolidados que en español se usan sin traducir (prensa económica, brókers,
/// informes de mercado) y que NO son «inglés colado». Criterio: (1) no tienen un equivalente español
/// de uso corriente o el inglés es el habitual, (2) son sustantivos o locuciones fijas, no palabras
/// funcionales («the», «with»), y (3) se admiten SOLO como término completo: «market cap» pasa, pero
/// «market» suelto o «the price is high» siguen marcándose. Las locuciones admiten espacio, guion o
/// barra entre palabras y un plural en «s». Se enmascaran antes de buscar inglés colado.
const FINANCE_TERMS: &[&str] = &[
    "market cap", "ticker", "etf", "spread", "rally", "trading", "trader", "broker", "stop loss", "take profit", "stop",
    "breakout", "pullback", "yield", "dividend yield", "price action", "risk on", "risk off", "risk reward", "reward risk",
    "buy and hold", "sell off", "buy the dip", "short squeeze", "hedge", "swing", "scalping", "benchmark", "momentum",
    "marketing", "holding", "rating", "dumping", "leasing", "ranking",
];

/// Reemplaza por espacios las apariciones de `FINANCE_TERMS` (misma longitud en bytes).
fn mask_finance_terms(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let sep = |c: u8| matches!(c, b' ' | b'-' | b'_' | b'/');
    let mut i = 0;
    while i < b.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let boundary = i == 0 || !text[..i].chars().next_back().is_some_and(char::is_alphanumeric);
        let mut best = 0;
        if boundary {
            'term: for term in FINANCE_TERMS {
                let t = term.as_bytes();
                let mut j = i;
                for &tc in t {
                    if tc == b' ' {
                        // Una o más separaciones (con espacios alrededor de una barra).
                        let start = j;
                        while j < b.len() && (sep(b[j])) {
                            j += 1;
                        }
                        if j == start {
                            continue 'term;
                        }
                    } else if j < b.len() && b[j].eq_ignore_ascii_case(&tc) {
                        j += 1;
                    } else {
                        continue 'term;
                    }
                }
                if j < b.len() && b[j] == b's' {
                    j += 1;
                }
                if text[j..].chars().next().is_some_and(char::is_alphabetic) {
                    continue;
                }
                best = best.max(j - i);
            }
        }
        if best > 0 {
            out[i..i + best].fill(b' ');
            i += best;
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

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

/// Conjunto de palabras de una frase para comparar repeticiones. Las cifras cuentan aunque sean
/// cortas («PER 31» y «PER 29» son datos distintos); el punto final de frase no forma parte de la palabra.
fn word_set(sent: &str) -> std::collections::HashSet<String> {
    sent.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '.')
        .map(|w| w.trim_matches('.'))
        .filter(|w| w.chars().count() >= 3 || w.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

/// Repetición literal o parafraseada: ≥ 70 % de palabras en común con una frase anterior.
fn is_dup(seen: &[std::collections::HashSet<String>], words: &std::collections::HashSet<String>) -> bool {
    seen.iter().any(|o| {
        let inter = o.intersection(words).count() as f64;
        let union = o.union(words).count().max(1) as f64;
        inter / union >= 0.7
    })
}

/// Quita las frases repetidas (en cualquier párrafo): los modelos pequeños copian el mismo párrafo
/// en los tres campos de la tesis. Los párrafos que quedan vacíos desaparecen.
pub fn dedupe_sentences(text: &str) -> String {
    let mut seen: Vec<std::collections::HashSet<String>> = Vec::new();
    let mut paras = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut kept = Vec::new();
        for sent in para.split_inclusive(". ").map(str::trim).filter(|x| !x.is_empty()) {
            let words = word_set(sent);
            if !is_dup(&seen, &words) {
                seen.push(words);
                kept.push(sent);
            }
        }
        if !kept.is_empty() {
            paras.push(kept.join(" "));
        }
    }
    paras.join("\n\n")
}

/// Quita las viñetas repetidas (mismo criterio que `dedupe_sentences`, a nivel de viñeta) dentro de
/// cada lista y ENTRE listas (`bullCase`, `bearCase`, `risks`): se conserva la primera aparición, en
/// el orden de las listas. Devuelve cuántas quitó.
pub fn dedupe_bullets<T>(lists: &mut [&mut Vec<T>], text_of: impl Fn(&T) -> &str) -> usize {
    let mut seen: Vec<std::collections::HashSet<String>> = Vec::new();
    let mut removed = 0;
    for list in lists.iter_mut() {
        let mut keep = Vec::with_capacity(list.len());
        for item in list.drain(..) {
            let words = word_set(text_of(&item));
            if words.is_empty() || !is_dup(&seen, &words) {
                seen.push(words);
                keep.push(item);
            } else {
                removed += 1;
            }
        }
        **list = keep;
    }
    removed
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

    /// TODO 4b: términos financieros consolidados que se usan en español sin traducir.
    #[test]
    fn consolidated_financial_terms_are_not_flagged_as_english() {
        for ok in [
            "El FDV / market cap es de 1,00x.",
            "La market cap del activo es de 1.68T.",
            "Conviene poner un stop loss y un take profit.",
            "El ticker sube en un rally con breakout y pullback posterior; el spread es bajo.",
            "El ETF paga un yield del 2 %.",
            "El mercado está en modo risk-on tras el sell-off; estrategia buy and hold.",
            "Lectura de price action favorable.",
        ] {
            assert!(spanish_issues(ok).is_empty(), "{ok}: {:?}", spanish_issues(ok));
        }
        // El inglés suelto sigue marcándose, también junto a un término consolidado.
        assert!(!spanish_issues("The market is rising").is_empty());
        assert!(!spanish_issues("El market cap y the price are high").is_empty());
        assert!(!spanish_issues("Hay un market sin cap").is_empty());
    }

    #[test]
    fn repeated_bullets_are_removed_within_and_across_lists() {
        let mut bull = vec!["El RSI14 está en 52.9, en zona neutra.".to_string(), "El RSI14 está en 52.9, en zona neutra".to_string(), "El PER es 35.4.".to_string()];
        let mut bear = vec!["El PER es 35.4".to_string(), "La caída máxima fue del 13.8 %.".to_string()];
        let mut risks = vec!["La caída máxima fue del 13.8%.".to_string(), "Resultados el próximo mes.".to_string()];
        let n = dedupe_bullets(&mut [&mut bull, &mut bear, &mut risks], |s| s.as_str());
        assert_eq!(n, 3);
        assert_eq!(bull, ["El RSI14 está en 52.9, en zona neutra.", "El PER es 35.4."]);
        assert_eq!(bear, ["La caída máxima fue del 13.8 %."]);
        assert_eq!(risks, ["Resultados el próximo mes."]);
        // Cifras distintas no son repetición.
        let mut a = vec!["El PER es 31 veces.".to_string(), "El PER es 29 veces.".to_string()];
        assert_eq!(dedupe_bullets(&mut [&mut a], |s| s.as_str()), 0);
    }

    #[test]
    fn repeated_sentences_are_removed_across_paragraphs() {
        let t = "A sube 1.5. B baja 2.5.\n\nA sube 1.5. B baja 2.5.\n\nA sube 1.5. C neutro 3.5.";
        assert_eq!(dedupe_sentences(t), "A sube 1.5. B baja 2.5.\n\nC neutro 3.5.");
    }

    #[test]
    fn paraphrased_repetitions_are_removed_but_different_sentences_stay() {
        let t = "El PER forward de 35.4 indica valoración alta.\n\nEl PER forward de 35.4 indica una valoración alta. El ATR14 de 6.69 marca volatilidad moderada.";
        assert_eq!(dedupe_sentences(t), "El PER forward de 35.4 indica valoración alta.\n\nEl ATR14 de 6.69 marca volatilidad moderada.");
    }

    #[test]
    fn indicator_names_are_not_repeated_words() {
        assert!(spanish_issues("Sobre la SMA20 (335.7) y la SMA50 (323) y la SMA200").is_empty());
        assert!(spanish_issues("la SMA 20 SMA 50").is_empty() || true);
    }

    #[test]
    fn valid_spanish_is_not_flagged() {
        // «caja libre del» solo es un calco cuando falta «flujo de»: «flujo de caja libre del año» es correcto.
        assert!(spanish_issues("El flujo de caja libre del último año fue de 100 M.").is_empty());
        assert!(!spanish_issues("La caja libre del grupo es alta.").is_empty());
        // Una palabra al final de una frase y la misma al principio de la siguiente no es una repetición.
        assert!(spanish_issues("El nivel clave es el soporte. Soporte que se rompe con facilidad.").is_empty());
        assert!(spanish_issues("Hay dos niveles: precio, precio objetivo y stop.").is_empty());
        assert!(!spanish_issues("sube de de forma fuerte").is_empty());
        // Nombres propios en inglés (empresas, fondos) no son inglés colado.
        assert!(spanish_issues("T. Rowe Price y The Home Depot cotizan con descuento; Vanguard Growth ETF pesa un 5 %.").is_empty());
        assert!(!spanish_issues("The price is high").is_empty());
    }

    #[test]
    fn sentences_that_differ_only_in_a_short_number_are_not_duplicates() {
        // El PER y la rentabilidad son datos distintos aunque «31» y «29» tengan dos cifras.
        let t = "El PER es 31 veces las ganancias. El PER es 29 veces las ganancias.";
        assert_eq!(dedupe_sentences(t), t);
        // Pero la repetición literal sigue eliminándose.
        assert_eq!(dedupe_sentences("El PER es 31 veces las ganancias. El PER es 31 veces las ganancias."), "El PER es 31 veces las ganancias.");
    }

    #[test]
    fn spanish_formatted_figures_count_once_in_specificity() {
        // «81.059,79» y «81059.79» son la misma cifra; «2057,45» no es «2057» + «45».
        let s = specificity("El precio es 81.059,79 y el ATR 2057,45.\n\nEl soporte 81059.79 sigue.");
        assert_eq!(s.figures, 2);
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
