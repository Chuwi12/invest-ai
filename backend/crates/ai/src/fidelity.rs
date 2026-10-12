//! Anti-alucinación numérica: toda cifra que cite el modelo debe existir en el contexto que se le
//! dio. Se indexan los números del contexto (con el tramo del que salen: cotización, indicadores,
//! fundamentales, puntuaciones, macro o una noticia concreta) y se comprueban las cifras de la
//! salida contra ese índice.
//!
//! Tolerancia: dos cifras coinciden si difieren menos que media unidad del decimal menos preciso
//! (el contexto muestra `12.3%`; el modelo puede escribir `12.3%` o `12%`, no `13%`). Cifras
//! con sufijo (`3.21T`, `450M`) se comparan ya escaladas. Los porcentajes se comparan como el
//! número escrito (12.3), porque así los presenta el contexto.
//!
//! Exentos (no son datos): enteros pequeños sin unidad (≤ 12), años, números pegados a letras
//! (`SMA200`, `RSI14`, `52w`, `N3`), y los de fechas/horas.

use std::ops::Range;

/// De qué parte del contexto sale una cifra.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    Header,
    Quote,
    Indicators,
    Fundamentals,
    Scores,
    Macro,
    /// Noticia con su etiqueta (`N3`).
    News(String),
    Gaps,
    /// Tramo sin clasificar (p. ej. resultados de herramientas en el chat).
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Num {
    pub raw: String,
    /// Valor ya escalado por el sufijo K/M/B/T.
    pub value: f64,
    /// Decimales escritos en la mantisa.
    pub decimals: u32,
    pub scale: f64,
    /// Posición en el texto (bytes) del token completo.
    pub span: Range<usize>,
    /// `true` si es un entero pequeño, un año o un fragmento de fecha/código: no es una cifra
    /// «de datos» y no se exige que esté en el contexto.
    pub exempt: bool,
}

impl Num {
    fn tolerance(&self) -> f64 {
        0.5 * 10f64.powi(-(self.decimals as i32)) * self.scale
    }
}

/// Extrae los números de un texto.
pub fn extract_numbers(text: &str) -> Vec<Num> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        // Signo (solo si está pegado y no es un guion entre palabras/fechas).
        let prev = (i > 0).then(|| chars[i - 1].1);
        let mut start = pos;
        let mut negative = false;
        if matches!(prev, Some('-' | '−' | '+')) {
            let before = (i > 1).then(|| chars[i - 2].1);
            if !before.is_some_and(|b| b.is_alphanumeric() || b == ')') {
                negative = matches!(prev, Some('-' | '−'));
                start = chars[i - 1].0;
            }
        }
        // Dígitos, con comas de millares (grupos exactos de 3) y parte decimal.
        let mut j = i;
        let mut mantissa = String::new();
        while j < chars.len() && chars[j].1.is_ascii_digit() {
            mantissa.push(chars[j].1);
            j += 1;
        }
        while j + 3 < chars.len()
            && chars[j].1 == ','
            && chars[j + 1..=j + 3].iter().all(|(_, d)| d.is_ascii_digit())
            && !chars.get(j + 4).is_some_and(|(_, d)| d.is_ascii_digit())
        {
            mantissa.extend(chars[j + 1..=j + 3].iter().map(|(_, d)| *d));
            j += 4;
        }
        let mut decimals = 0u32;
        if j + 1 < chars.len() && chars[j].1 == '.' && chars[j + 1].1.is_ascii_digit() {
            mantissa.push('.');
            j += 1;
            while j < chars.len() && chars[j].1.is_ascii_digit() {
                mantissa.push(chars[j].1);
                decimals += 1;
                j += 1;
            }
        }
        // Sufijo de escala pegado (K/M/B/T), solo si no continúa una palabra.
        let mut scale = 1.0;
        let mut suffix_len = 0;
        if let Some(&(_, s)) = chars.get(j) {
            let next_alnum = chars.get(j + 1).is_some_and(|(_, n)| n.is_alphanumeric());
            let sc = match s {
                'K' => 1e3,
                'M' => 1e6,
                'B' => 1e9,
                'T' => 1e12,
                _ => 0.0,
            };
            if sc > 0.0 && !next_alnum {
                scale = sc;
                suffix_len = 1;
                j += 1;
            }
        }
        let end = chars.get(j).map_or(text.len(), |(p, _)| *p);
        let has_percent = chars.get(j).is_some_and(|(_, p)| *p == '%');
        let attached_before = prev.is_some_and(|p| p.is_alphabetic());
        let attached_after = suffix_len == 0 && chars.get(j).is_some_and(|(_, n)| n.is_alphabetic());
        let base: f64 = mantissa.parse().unwrap_or(f64::NAN);
        let mut value = base * scale;
        if negative {
            value = -value;
        }
        let in_date_or_time = prev.is_some_and(|p| p == ':' || p == '/')
            || chars.get(j).is_some_and(|(_, n)| *n == ':' || *n == '/')
            || (matches!(prev, Some('-')) && !negative)
            || (chars.get(j).is_some_and(|(_, n)| *n == '-')
                && chars.get(j + 1).is_some_and(|(_, d)| d.is_ascii_digit()));
        let small_int = decimals == 0 && scale == 1.0 && !has_percent && base <= 12.0;
        let year = decimals == 0 && scale == 1.0 && !has_percent && (1990.0..=2100.0).contains(&base);
        let exempt = attached_before || attached_after || in_date_or_time || small_int || year;
        if value.is_finite() {
            out.push(Num {
                raw: text[start..end].to_string(),
                value,
                decimals,
                scale,
                span: start..end,
                exempt,
            });
        }
        i = j.max(i + 1);
    }
    out
}

/// Índice de las cifras presentes en un contexto.
#[derive(Debug, Clone, Default)]
pub struct NumberIndex {
    entries: Vec<(Num, Segment)>,
}

impl NumberIndex {
    /// Indexa el contexto del análisis (formato de `context::build_context`): clasifica cada línea
    /// por su sección `## …` y las noticias por su etiqueta `[N3]`.
    pub fn from_context(text: &str) -> Self {
        let mut idx = Self::default();
        let mut seg = Segment::Header;
        for line in text.lines() {
            if let Some(h) = line.strip_prefix("## ") {
                seg = classify_heading(h);
            }
            let line_seg = news_tag(line).map_or_else(|| seg.clone(), Segment::News);
            idx.add_text(line, line_seg);
        }
        idx
    }

    pub fn add_text(&mut self, text: &str, seg: Segment) {
        // Las exentas (fechas, códigos, enteros pequeños) no son datos: no sirven de aval.
        for n in extract_numbers(text).into_iter().filter(|n| !n.exempt) {
            self.entries.push((n, seg.clone()));
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Segmentos donde aparece una cifra equivalente. Vacío = no está en el contexto.
    pub fn find(&self, n: &Num) -> Vec<Segment> {
        let mut segs: Vec<Segment> = Vec::new();
        for (e, s) in &self.entries {
            let tol = n.tolerance().max(e.tolerance()) * 1.0001;
            if (n.value - e.value).abs() <= tol && !segs.contains(s) {
                segs.push(s.clone());
            }
        }
        segs
    }

    /// Cifras de `text` que no están en el contexto (ignora las exentas).
    pub fn unverified(&self, text: &str) -> Vec<Num> {
        extract_numbers(text).into_iter().filter(|n| !n.exempt && self.find(n).is_empty()).collect()
    }

    /// Cifras citadas (lista `figures` del modelo): `Ok(segmentos)` si todas existen.
    pub fn check_figures(&self, figures: &[String]) -> Result<Vec<Segment>, Vec<String>> {
        let mut segs: Vec<Segment> = Vec::new();
        let mut bad = Vec::new();
        for f in figures {
            for n in extract_numbers(f) {
                if n.exempt {
                    continue;
                }
                let found = self.find(&n);
                if found.is_empty() {
                    bad.push(n.raw.clone());
                }
                for s in found {
                    if !segs.contains(&s) {
                        segs.push(s);
                    }
                }
            }
        }
        if bad.is_empty() { Ok(segs) } else { Err(bad) }
    }

    /// Reescribe `text` marcando con `⚠` cada cifra no verificable. Devuelve el texto y las cifras.
    pub fn mark_unverified(&self, text: &str) -> (String, Vec<String>) {
        let bad = self.unverified(text);
        if bad.is_empty() {
            return (text.to_string(), vec![]);
        }
        let mut out = String::with_capacity(text.len() + bad.len() * 3);
        let mut last = 0;
        for n in &bad {
            // El `%` va pegado al número: la marca se pone después.
            let end = n.span.end + usize::from(text[n.span.end..].starts_with('%'));
            out.push_str(&text[last..end]);
            out.push('⚠');
            last = end;
        }
        out.push_str(&text[last..]);
        (out, bad.into_iter().map(|n| n.raw).collect())
    }
}

fn classify_heading(h: &str) -> Segment {
    let h = h.trim_start();
    let starts = |p: &[&str]| p.iter().any(|x| h.starts_with(x));
    if starts(&["Cotización", "Quote"]) {
        Segment::Quote
    } else if starts(&["Indicadores técnicos", "Technical indicators"]) {
        Segment::Indicators
    } else if starts(&["Fundamentales", "Fundamentals"]) {
        Segment::Fundamentals
    } else if starts(&["Puntuaciones", "Quantitative scores"]) {
        Segment::Scores
    } else if starts(&["Entorno macro", "Macro environment", "Macro"]) {
        Segment::Macro
    } else if starts(&["Noticias", "News"]) {
        Segment::Other
    } else if starts(&["Datos no disponibles", "Missing data"]) {
        Segment::Gaps
    } else {
        Segment::Other
    }
}

/// `[N3] 2026-…` → `N3`.
fn news_tag(line: &str) -> Option<String> {
    let rest = line.strip_prefix("[N")?;
    let end = rest.find(']')?;
    let digits = &rest[..end];
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| format!("N{digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: &str = "# Apple (stock:AAPL) — stock\n\
## Cotización [fuente: yahoo · 2026-10-12 14:03 UTC]\n| Precio | 212.3 USD |\n| Volume | 51.2M |\n\
## Indicadores técnicos (velas diarias)\n| RSI14 | 72.3 |\n| Rendimientos | 1m +4.5%, 1y +18.2% |\n\
## Fundamentales\n| EV | 3.21T |\n\
## Noticias (más recientes primero)\n[N1] 2026-10-10 09:00 · Reuters · +0.40 positive: Apple gana 12% más que lo previsto\n";

    fn find_first(idx: &NumberIndex, s: &str) -> Vec<Segment> {
        idx.find(&extract_numbers(s)[0])
    }

    #[test]
    fn extracts_numbers_with_suffix_decimals_and_sign() {
        let n = extract_numbers("subió -3.5% y 1,234.5 con EV 3.21T, volumen 450M");
        let raws: Vec<&str> = n.iter().map(|x| x.raw.as_str()).collect();
        assert_eq!(raws, ["-3.5", "1,234.5", "3.21T", "450M"]);
        assert_eq!(n[1].value, 1234.5);
        assert_eq!(n[2].value, 3.21e12);
        assert!(n[0].value < 0.0);
    }

    #[test]
    fn exempts_codes_dates_small_ints_and_years() {
        let n = extract_numbers("SMA200 RSI14 52w 3 razones en 2026 el 2026-10-12 a las 14:03 y N3");
        assert!(n.iter().all(|x| x.exempt), "{n:?}");
        let n = extract_numbers("precio 212.3 y 45%");
        assert!(n.iter().all(|x| !x.exempt));
    }

    #[test]
    fn index_tracks_the_segment_of_each_figure() {
        let idx = NumberIndex::from_context(CTX);
        assert_eq!(find_first(&idx, "212.3"), vec![Segment::Quote]);
        assert_eq!(find_first(&idx, "72.3"), vec![Segment::Indicators]);
        assert_eq!(find_first(&idx, "3.21T"), vec![Segment::Fundamentals]);
        assert_eq!(find_first(&idx, "51.2M"), vec![Segment::Quote]);
        assert_eq!(find_first(&idx, "12%"), vec![Segment::News("N1".into())]);
        assert!(find_first(&idx, "999.9").is_empty());
    }

    #[test]
    fn tolerance_is_half_a_unit_of_the_least_precise_figure() {
        let idx = NumberIndex::from_context(CTX);
        assert!(!find_first(&idx, "72").is_empty(), "redondeo de 72.3");
        assert!(find_first(&idx, "73").is_empty(), "73 no es 72.3");
        assert!(!find_first(&idx, "3.2T").is_empty());
        assert!(find_first(&idx, "3.5T").is_empty());
        assert!(!find_first(&idx, "212.30").is_empty());
    }

    #[test]
    fn checks_figure_lists_and_reports_the_bad_ones() {
        let idx = NumberIndex::from_context(CTX);
        let ok = idx.check_figures(&["RSI14 72.3".into(), "212.3 USD".into()]).unwrap();
        assert!(ok.contains(&Segment::Indicators) && ok.contains(&Segment::Quote));
        let bad = idx.check_figures(&["RSI14 72.3".into(), "PER 41.7".into()]).unwrap_err();
        assert_eq!(bad, vec!["41.7".to_string()]);
    }

    #[test]
    fn marks_unverified_numbers_inline() {
        let idx = NumberIndex::from_context(CTX);
        let (t, bad) = idx.mark_unverified("RSI 72.3 con objetivo en 250 y 3 razones; sube un 45%");
        assert_eq!(t, "RSI 72.3 con objetivo en 250⚠ y 3 razones; sube un 45%⚠");
        assert_eq!(bad, ["250", "45"]);
        let (same, none) = idx.mark_unverified("sin cifras raras 212.3");
        assert_eq!(same, "sin cifras raras 212.3");
        assert!(none.is_empty());
    }

    #[test]
    fn works_on_the_real_rendered_context() {
        use crate::{context, fixture, lang::Lang, scoring};
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::Es);
        let built = context::build_context(
            &snap,
            &card,
            &context::ContextOptions { max_tokens: 6000, lang: Lang::Es },
            &mut context::NewsRegistry::new(),
        );
        let idx = NumberIndex::from_context(&built.text);
        assert!(idx.len() > 50);
        let price = snap.quote.as_ref().unwrap().price;
        let n = extract_numbers(&crate::fmt::price(price));
        assert!(idx.find(&n[0]).contains(&Segment::Quote));
        let rsi = snap.indicators.as_ref().unwrap().rsi14.unwrap();
        let n = extract_numbers(&format!("{rsi:.1}"));
        assert!(!idx.find(&n[0]).is_empty());
    }
}
