//! Estimación barata de tokens. Sin tokenizador real: ~4 caracteres por token.
//!
//! Es solo para dimensionar presupuestos (contexto, historial); el recuento real lo devuelve
//! Ollama (`prompt_eval_count`) y se contrasta en el benchmark.

pub const CHARS_PER_TOKEN: usize = 4;

pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// Recorta `s` para que quepa en `max_tokens` (estimados), cortando en un límite de línea o de
/// palabra cuando se puede y añadiendo `…` si hubo recorte.
pub fn truncate_to_tokens(s: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let cut: String = s.chars().take(keep).collect();
    // Prefiere cortar en salto de línea; si no hay uno razonablemente cerca, en espacio.
    let min_keep = cut.len() * 3 / 4;
    let end = cut
        .rfind('\n')
        .filter(|&i| i >= min_keep)
        .or_else(|| cut.rfind(' ').filter(|&i| i >= min_keep))
        .unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end())
}

/// Recorta a un número de caracteres (para resúmenes de noticias).
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    let end = cut.rfind(' ').filter(|&i| i >= cut.len() / 2).unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_four_chars_per_token() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens(&"x".repeat(400)), 100);
    }

    #[test]
    fn truncation_respects_budget_and_marks_cut() {
        let text = "linea uno\nlinea dos\nlinea tres\nlinea cuatro\nlinea cinco\n".repeat(20);
        let out = truncate_to_tokens(&text, 30);
        assert!(estimate_tokens(&out) <= 31, "{}", estimate_tokens(&out));
        assert!(out.ends_with('…'));
        assert_eq!(truncate_to_tokens("corto", 30), "corto");
    }

    #[test]
    fn truncation_is_utf8_safe() {
        let text = "ñandú€ ".repeat(100);
        let out = truncate_to_tokens(&text, 10);
        assert!(out.ends_with('…'));
        let out = truncate_chars(&text, 25);
        assert!(out.chars().count() <= 26);
    }
}
