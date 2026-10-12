//! Validación en Rust de lo que dice el modelo: cifras, citas de noticias, niveles de precio y
//! calibración de la confianza. Nada de lo que sale del LLM llega al `Analysis` sin pasar por aquí.

use domain::{AssetSnapshot, SourceRef, Verdict};

use super::types::Claim;
use crate::{
    context::{NewsRegistry, extract_citations, news_source},
    fidelity::{NumberIndex, Segment, extract_numbers},
    lang::{Lang, tr},
    scoring::{Scorecard, verdict_rank},
    tokens::truncate_chars,
};

const MAX_CLAIM_CHARS: usize = 420;
const MAX_PROSE_CHARS: usize = 1800;

/// Afirmación aprobada: texto ya con su cita de fuente y marca de tiempo.
#[derive(Debug, Clone)]
pub struct Checked {
    pub text: String,
    /// El texto sin la cita (para pasarlo a la siguiente etapa sin gastar tokens).
    pub plain: String,
    pub area: String,
}

pub struct Verifier<'a> {
    pub idx: NumberIndex,
    snap: &'a AssetSnapshot,
    registry: &'a NewsRegistry,
    lang: Lang,
    pub warnings: Vec<String>,
    /// Afirmaciones descartadas por citar cifras o noticias inexistentes.
    pub dropped: usize,
    /// Cifras sin verificar que se dejaron marcadas con ⚠ en prosa.
    pub marked: usize,
    used: Vec<Segment>,
}

impl<'a> Verifier<'a> {
    pub fn new(context_text: &str, snap: &'a AssetSnapshot, registry: &'a NewsRegistry, lang: Lang) -> Self {
        Self {
            idx: NumberIndex::from_context(context_text),
            snap,
            registry,
            lang,
            warnings: Vec::new(),
            dropped: 0,
            marked: 0,
            used: Vec::new(),
        }
    }

    fn warn(&mut self, w: String) {
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    /// ¿Respalda la noticia `n` lo que dice `text`? Sí si alguna cifra del texto sale de esa noticia,
    /// o si comparte al menos dos palabras significativas con su titular/resumen (sin contar el
    /// nombre del propio activo). Un modelo pequeño pega `[N2]` por costumbre: eso no es una fuente.
    fn news_supports(&self, n: usize, text: &str) -> bool {
        let tag = format!("N{n}");
        let Some(item) = self.registry.get(&tag) else { return false };
        let nums: Vec<_> = extract_numbers(text).into_iter().filter(|x| !x.exempt).collect();
        if nums.iter().any(|x| self.idx.find(x).contains(&Segment::News(tag.clone()))) {
            return true;
        }
        let own = format!("{} {}", self.snap.summary.name, self.snap.summary.symbol).to_lowercase();
        let words = |t: &str| -> Vec<String> {
            t.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.chars().count() >= 4 && !own.contains(*w))
                .map(str::to_string)
                .collect()
        };
        let body = format!("{} {}", item.title, item.summary.as_deref().unwrap_or(""));
        let news_words = words(&body);
        let mut shared: Vec<String> = words(text).into_iter().filter(|w| news_words.contains(w)).collect();
        shared.sort();
        shared.dedup();
        shared.len() >= 2
    }

    /// Quita las citas `[N#]` (una sola noticia por corchete) que la noticia no respalda.
    pub fn clean_citations(&self, text: &str) -> String {
        let mut out = text.to_string();
        for n in extract_citations(text) {
            if !self.news_supports(n, text) {
                out = out.replace(&format!(" [N{n}]"), "").replace(&format!("[N{n}]"), "");
            }
        }
        out
    }

    /// Comprueba una afirmación. `None` = descartada (y anotada en `warnings`).
    pub fn claim(&mut self, c: &Claim) -> Option<Checked> {
        let text = truncate_chars(c.text.trim(), MAX_CLAIM_CHARS);
        if text.is_empty() {
            return None;
        }
        let l = self.lang;
        // Citas de noticias: todas deben existir.
        for n in extract_citations(&text) {
            if self.registry.get(&format!("N{n}")).is_none() {
                self.dropped += 1;
                let w = tr!(l, "Se descartó una afirmación que citaba la noticia [N{}], que no existe: «{}»", "A claim citing news [N{}], which does not exist, was dropped: «{}»", n, truncate_chars(&text, 90));
                self.warn(w);
                return None;
            }
        }
        let text = self.clean_citations(&text);
        let mut segs = match self.idx.check_figures(&c.figures) {
            Ok(s) => s,
            Err(bad) => return self.reject(&text, &bad),
        };
        // Las cifras del propio texto también deben existir aunque el modelo no las listara.
        let in_text = self.idx.unverified(&text);
        if !in_text.is_empty() {
            let bad: Vec<String> = in_text.into_iter().map(|n| n.raw).collect();
            return self.reject(&text, &bad);
        }
        for n in extract_numbers(&text).iter().filter(|n| !n.exempt) {
            for s in self.idx.find(n) {
                if !segs.contains(&s) {
                    segs.push(s);
                }
            }
        }
        // Las cifras de la puntuación repiten las de cotización/indicadores/fundamentales: se cita
        // la fuente primaria, y como mucho dos, para que la cita sea legible.
        if segs.iter().any(|s| !matches!(s, Segment::Scores)) {
            segs.retain(|s| !matches!(s, Segment::Scores));
        }
        segs.truncate(2);
        let suffix = self.cite(&segs, &text);
        Some(Checked { text: format!("{text}{suffix}"), plain: text, area: c.area.clone() })
    }

    fn reject(&mut self, text: &str, bad: &[String]) -> Option<Checked> {
        self.dropped += 1;
        let l = self.lang;
        let w = tr!(
            l,
            "Se descartó una afirmación porque citaba cifras que no aparecen en los datos ({}): «{}»",
            "A claim was dropped because it cited figures that are not in the data ({}): «{}»",
            bad.join(", "),
            truncate_chars(text, 90)
        );
        self.warn(w);
        None
    }

    /// Prosa libre (resumen, tesis, estrategia): se conserva, pero cada cifra sin verificar se
    /// marca con ⚠ y se avisa una vez.
    pub fn prose(&mut self, text: &str, field: &str) -> String {
        let text = self.clean_citations(&truncate_chars(text.trim(), MAX_PROSE_CHARS));
        let (marked, bad) = self.idx.mark_unverified(&text);
        if !bad.is_empty() {
            self.marked += bad.len();
            let l = self.lang;
            let w = tr!(
                l,
                "Cifras no verificables en los datos (marcadas con ⚠) en «{}»: {}",
                "Figures not verifiable in the data (marked with ⚠) in «{}»: {}",
                field,
                bad.join(", ")
            );
            self.warn(w);
        }
        marked
    }

    /// Prosa por párrafos: como `prose`, y cada párrafo termina con las fuentes de las cifras que
    /// usa (proveedor y hora), para que la tesis sea comprobable igual que las afirmaciones.
    pub fn prose_cited(&mut self, text: &str, field: &str) -> String {
        let text = self.clean_citations(&truncate_chars(text.trim(), MAX_PROSE_CHARS));
        let mut out = Vec::new();
        for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            let (marked, bad) = self.idx.mark_unverified(para);
            if !bad.is_empty() {
                self.marked += bad.len();
                let l = self.lang;
                let w = tr!(
                    l,
                    "Cifras no verificables en los datos (marcadas con ⚠) en «{}»: {}",
                    "Figures not verifiable in the data (marked with ⚠) in «{}»: {}",
                    field,
                    bad.join(", ")
                );
                self.warn(w);
            }
            let mut segs: Vec<Segment> = Vec::new();
            for n in extract_numbers(para).iter().filter(|n| !n.exempt) {
                for s in self.idx.find(n) {
                    if !segs.contains(&s) {
                        segs.push(s);
                    }
                }
            }
            if segs.iter().any(|s| !matches!(s, Segment::Scores)) {
                segs.retain(|s| !matches!(s, Segment::Scores));
            }
            segs.truncate(3);
            let suffix = self.cite(&segs, para);
            out.push(format!("{marked}{suffix}"));
        }
        out.join("\n\n")
    }

    /// Sufijo « (fuente: …)» con proveedor y marca de tiempo de cada tramo de datos usado.
    fn cite(&mut self, segs: &[Segment], text: &str) -> String {
        let mut parts = Vec::new();
        let mut tags = Vec::new();
        for s in segs {
            if let Segment::News(t) = s {
                if !text.contains(&format!("[{t}]")) && !text.contains(t.as_str()) && !tags.contains(t) {
                    tags.push(t.clone());
                }
                if !self.used.contains(s) {
                    self.used.push(s.clone());
                }
                continue;
            }
            if let Some((label, _)) = self.source_of(s) {
                parts.push(label);
                if !self.used.contains(s) {
                    self.used.push(s.clone());
                }
            }
        }
        let mut out = String::new();
        if !tags.is_empty() {
            out.push_str(&format!(" [{}]", tags.join("] [")));
        }
        if !parts.is_empty() {
            let l = self.lang;
            out.push_str(&format!(" ({}: {})", l.t("fuente", "source"), parts.join("; ")));
        }
        out
    }

    fn ts(t: chrono::DateTime<chrono::Utc>) -> String {
        t.format("%Y-%m-%d %H:%M UTC").to_string()
    }

    /// (etiqueta legible con marca de tiempo, `SourceRef`) de un tramo del contexto.
    fn source_of(&self, seg: &Segment) -> Option<(String, SourceRef)> {
        let l = self.lang;
        let snap = self.snap;
        let id = &snap.summary.id;
        match seg {
            Segment::Quote => {
                let q = snap.quote.as_ref()?;
                Some((
                    format!("{} {} {}", l.t("cotización", "quote"), q.source, Self::ts(q.timestamp)),
                    SourceRef {
                        title: format!("{} {id}", l.t("Cotización", "Quote")),
                        url: None,
                        provider: q.source.clone(),
                        published_at: Some(q.timestamp),
                    },
                ))
            }
            Segment::Indicators => {
                let at = snap.indicators.as_ref()?.as_of.unwrap_or(snap.generated_at);
                Some((
                    format!("{} {}", l.t("indicadores técnicos", "technical indicators"), Self::ts(at)),
                    SourceRef {
                        title: format!("{} {id}", l.t("Indicadores técnicos (velas diarias)", "Technical indicators (daily candles)")),
                        url: None,
                        provider: "market".into(),
                        published_at: Some(at),
                    },
                ))
            }
            Segment::Fundamentals => {
                let f = snap.fundamentals.as_ref()?;
                let at = f.as_of.unwrap_or(snap.generated_at);
                let provider = if f.sources.is_empty() { "market".to_string() } else { f.sources.join(", ") };
                Some((
                    format!("{} {provider} {}", l.t("fundamentales", "fundamentals"), Self::ts(at)),
                    SourceRef {
                        title: format!("{} {id}", l.t("Fundamentales", "Fundamentals")),
                        url: None,
                        provider,
                        published_at: Some(at),
                    },
                ))
            }
            Segment::Macro => {
                let m = snap.macro_context.as_ref()?;
                let at = m.as_of.unwrap_or(snap.generated_at);
                let mut srcs: Vec<&str> = m.indicators.iter().map(|i| i.source.as_str()).collect();
                srcs.sort_unstable();
                srcs.dedup();
                let provider = if srcs.is_empty() { "market".to_string() } else { srcs.join(", ") };
                Some((
                    format!("macro {provider} {}", Self::ts(at)),
                    SourceRef {
                        title: l.t("Contexto macroeconómico", "Macro context").to_string(),
                        url: None,
                        provider,
                        published_at: Some(at),
                    },
                ))
            }
            Segment::Levels => Some((
                format!("{} {}", l.t("niveles calculados", "computed levels"), Self::ts(snap.generated_at)),
                SourceRef {
                    title: format!("{} {id}", l.t("Niveles de entrada, stop y objetivos calculados", "Computed entry, stop and target levels")),
                    url: None,
                    provider: "invest-ai".into(),
                    published_at: Some(snap.generated_at),
                },
            )),
            Segment::Scores => Some((
                format!("{} {}", l.t("puntuación cuantitativa", "quantitative score"), Self::ts(snap.generated_at)),
                SourceRef {
                    title: format!("{} {id}", l.t("Puntuación cuantitativa determinista", "Deterministic quantitative score")),
                    url: None,
                    provider: "invest-ai".into(),
                    published_at: Some(snap.generated_at),
                },
            )),
            _ => None,
        }
    }

    /// Fuentes usadas por las afirmaciones aprobadas (cotización siempre que exista).
    pub fn sources(&mut self) -> Vec<SourceRef> {
        if !self.used.contains(&Segment::Quote) && self.snap.quote.is_some() {
            self.used.insert(0, Segment::Quote);
        }
        let mut out: Vec<SourceRef> = Vec::new();
        for seg in self.used.clone() {
            let r = match &seg {
                Segment::News(t) => self.registry.get(t).map(news_source),
                s => self.source_of(s).map(|(_, r)| r),
            };
            if let Some(r) = r
                && !out.iter().any(|o| o.title == r.title && o.provider == r.provider)
            {
                out.push(r);
            }
        }
        out
    }
}

/// Confianza calibrada (0.05–0.85): mezcla la del modelo con la de la puntuación determinista,
/// y la reduce por falta de cobertura, afirmaciones descartadas, cifras sin verificar y
/// desacuerdo fuerte con el veredicto cuantitativo.
pub fn calibrate(model_conf: f32, card: &Scorecard, verdict: Verdict, dropped: usize, marked: usize) -> f32 {
    let m = if model_conf.is_finite() { model_conf.clamp(0.0, 1.0) } else { 0.5 };
    let mut c = 0.5 * m + 0.5 * card.confidence;
    // Nunca más de 0.15 por encima de la confianza de la puntuación.
    c = c.min(card.confidence + 0.15);
    if card.coverage < 0.5 {
        c = c.min(0.5);
    }
    c -= 0.04 * dropped.min(5) as f32 + 0.02 * marked.min(5) as f32;
    if (verdict_rank(verdict) - verdict_rank(card.verdict)).abs() >= 3 {
        c -= 0.10;
    }
    (c.clamp(0.05, 0.85) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::{ContextOptions, build_context},
        fixture, scoring,
    };

    struct Fx {
        snap: AssetSnapshot,
        card: Scorecard,
        text: String,
        reg: NewsRegistry,
    }

    fn fx() -> Fx {
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::Es);
        let mut reg = NewsRegistry::new();
        let built = build_context(&snap, &card, &ContextOptions { max_tokens: 6000, lang: Lang::Es }, &mut reg);
        Fx { snap, card, text: built.text, reg: built.news }
    }

    fn claim(text: &str, figures: &[&str]) -> Claim {
        Claim { text: text.into(), figures: figures.iter().map(|s| s.to_string()).collect(), area: String::new() }
    }

    #[test]
    fn keeps_verified_claims_with_source_and_timestamp() {
        let f = fx();
        let mut v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        let rsi = format!("{:.1}", f.snap.indicators.as_ref().unwrap().rsi14.unwrap());
        let c = v.claim(&claim(&format!("El RSI14 está en {rsi}"), &[&rsi])).expect("verificada");
        assert!(c.text.contains("fuente: indicadores técnicos 20"), "{}", c.text);
        assert!(c.text.contains("UTC"));
        assert_eq!(v.dropped, 0);
        let srcs = v.sources();
        assert!(srcs.iter().any(|s| s.title.contains("Indicadores")));
        assert!(srcs.iter().all(|s| s.published_at.is_some()));
    }

    #[test]
    fn drops_claims_with_invented_figures_or_unknown_news() {
        let f = fx();
        let mut v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        assert!(v.claim(&claim("El PER es 87.43", &["87.43"])).is_none());
        assert!(v.claim(&claim("Crece fuerte sin decir cifras pero el objetivo es 999.99", &[])).is_none());
        assert!(v.claim(&claim("Según [N42] todo va bien", &[])).is_none());
        assert_eq!(v.dropped, 3);
        assert_eq!(v.warnings.len(), 3);
        assert!(v.claim(&claim("Cualitativa sin números", &[])).is_some());
    }

    #[test]
    fn habitual_news_citations_without_support_are_removed() {
        let f = fx();
        let v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        // Sin relación con la noticia N1: se quita la cita; con cifra de la noticia o texto que la cita bien, se queda.
        let t0 = f.reg.get("N1").unwrap().title.clone();
        assert_eq!(v.clean_citations("La caída máxima es del 13.8 por ciento [N1]."), "La caída máxima es del 13.8 por ciento.");
        let words: Vec<&str> = t0.split_whitespace().filter(|w| w.chars().count() >= 5).take(3).collect();
        let supported = format!("{} [N1]", words.join(" "));
        assert!(v.clean_citations(&supported).contains("[N1]"), "{supported}");
    }

    #[test]
    fn prose_marks_unverified_figures() {
        let f = fx();
        let mut v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        let out = v.prose("Esperamos un 77.7 % de subida", "tesis");
        assert!(out.contains('⚠'), "{out}");
        assert_eq!(v.marked, 1);
        assert!(v.warnings[0].contains("tesis"));
    }

    #[test]
    fn calibration_penalises_and_caps() {
        let f = fx();
        let hi = calibrate(1.0, &f.card, f.card.verdict, 0, 0);
        assert!(hi <= f.card.confidence + 0.151 && hi <= 0.85, "{hi}");
        let lower = calibrate(1.0, &f.card, f.card.verdict, 3, 2);
        assert!(lower < hi);
        let opposite = if verdict_rank(f.card.verdict) >= 3 { Verdict::Avoid } else { Verdict::Buy };
        assert!(calibrate(0.9, &f.card, opposite, 0, 0) < calibrate(0.9, &f.card, f.card.verdict, 0, 0));
        assert!(calibrate(f32::NAN, &f.card, f.card.verdict, 0, 0) >= 0.05);
    }
}
