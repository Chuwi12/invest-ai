//! Búsqueda léxica (FTS5/BM25) y fusión con la vectorial.
//!
//! Los embeddings flojean con identificadores exactos (tickers como `BRK.B`, «Item 1A», cifras);
//! BM25 los encuentra tal cual y además funciona sin embedder. Las dos listas se fusionan con
//! Reciprocal Rank Fusion (Cormack et al., 2009) ANTES del reranking heurístico de `kb.rs`.
//!
//! Este módulo es puro (sin SQLite): sanea la consulta y calcula la fusión.

use std::collections::HashSet;

use crate::text::fold_key;

/// Constante de RRF. 60 es el valor del artículo original y el que usan por defecto los motores
/// que lo implementan; no hay datos propios con los que ajustarlo (ver `docs/notes/rag.md`).
pub const RRF_K: f32 = 60.0;
/// Longitud máxima (caracteres) de la consulta que se considera para FTS5.
pub const MAX_QUERY_CHARS: usize = 300;
/// Máximo de términos en la expresión MATCH.
pub const MAX_TERMS: usize = 16;

/// Contribución de una lista a la fusión para un elemento en la posición `rank` (0 = el mejor).
pub fn rrf_term(rank: usize) -> f32 {
    1.0 / (RRF_K + rank as f32 + 1.0)
}

/// Mayor puntuación posible de la fusión de dos listas (primero en ambas).
pub fn rrf_max() -> f32 {
    2.0 * rrf_term(0)
}

/// Palabras vacías muy frecuentes en español e inglés: con `OR` solo añaden ruido y coste.
const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "de", "del", "el", "en", "es", "for", "from", "in", "is", "it", "la", "las", "lo", "los", "of", "on", "or", "por", "que", "se",
    "the", "to", "un", "una", "y", "with", "con", "para", "al", "su", "sus",
];

/// Convierte texto libre en una expresión MATCH de FTS5 que NO puede romper la sintaxis.
///
/// Cada término va entre comillas dobles (una cadena entre comillas es siempre una frase literal:
/// `AND`, `NOT`, `NEAR`, `*`, `^`, `:`, `-`, `(` pierden su significado) y se unen con `OR`; BM25
/// ordena por cuántos términos y cuán raros. `BRK.B` pasa a la frase `"BRK.B"`, que el
/// tokenizador parte en `brk b` adyacentes y casa con el texto original. Las comillas internas se
/// eliminan. Devuelve `None` si no queda ningún término útil.
pub fn fts_query(raw: &str) -> Option<String> {
    let limited: String = raw.chars().take(MAX_QUERY_CHARS).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut terms: Vec<String> = Vec::new();
    for tok in limited.split_whitespace() {
        let cleaned: String = tok.chars().filter(|c| *c != '"' && !c.is_control()).collect();
        let cleaned = cleaned.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
        if cleaned.is_empty() || !cleaned.chars().any(char::is_alphanumeric) {
            continue;
        }
        let key = fold_key(&cleaned);
        if key.is_empty() || (cleaned.chars().all(char::is_alphabetic) && STOP.contains(&key.as_str())) {
            continue;
        }
        if seen.insert(key) {
            terms.push(format!("\"{cleaned}\""));
        }
        if terms.len() >= MAX_TERMS {
            break;
        }
    }
    if terms.is_empty() { None } else { Some(terms.join(" OR ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_every_term_and_drops_stopwords() {
        assert_eq!(fts_query("riesgos de la OPV").as_deref(), Some("\"riesgos\" OR \"OPV\""));
        assert_eq!(fts_query("BRK.B").as_deref(), Some("\"BRK.B\""));
        assert_eq!(fts_query("Item 1A.").as_deref(), Some("\"Item\" OR \"1A\""));
        assert_eq!(fts_query("the of and"), None);
        assert_eq!(fts_query("  ---  ** "), None);
    }

    #[test]
    fn hostile_input_cannot_inject_operators() {
        let q = fts_query("foo\" OR \"bar NEAR(a b) AND NOT x* ^y col:z (-w) \"unclosed").unwrap();
        // Todo término está entre comillas dobles balanceadas y sin comillas internas.
        for t in q.split(" OR ") {
            assert!(t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"'), "{t} en {q}");
        }
        assert!(!q.contains("NEAR(") || q.contains("\"NEAR"), "{q}");
    }

    #[test]
    fn caps_length_and_terms() {
        let long = (0..200).map(|i| format!("palabra{i}")).collect::<Vec<_>>().join(" ");
        let q = fts_query(&long).unwrap();
        assert!(q.split(" OR ").count() <= MAX_TERMS);
        assert!(q.chars().count() < 400);
    }

    #[test]
    fn rrf_prefers_agreement() {
        assert!(rrf_term(0) > rrf_term(1));
        assert!(rrf_term(0) + rrf_term(5) > rrf_term(0));
        assert!((rrf_max() - 2.0 / 61.0).abs() < 1e-6);
    }
}
