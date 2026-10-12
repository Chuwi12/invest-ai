//! Validación en Rust de lo que dice el modelo: cifras, citas de noticias, niveles de precio y
//! calibración de la confianza. Nada de lo que sale del LLM llega al `Analysis` sin pasar por aquí.

use domain::{AssetSnapshot, KeyLevels, SourceRef, Verdict};

use super::types::{Claim, LevelsOut};
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
        let text = truncate_chars(text.trim(), MAX_PROSE_CHARS);
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

    /// Niveles de precio: soportes/resistencias deben ser los de los datos; entrada, stop y
    /// objetivos son derivados por el modelo y solo se comprueba que tengan sentido respecto al
    /// precio. Sin niveles válidos del modelo se usan los calculados en código.
    pub fn levels(&mut self, raw: &LevelsOut, verdict: Verdict) -> KeyLevels {
        let price = self
            .snap
            .quote
            .as_ref()
            .map(|q| q.price)
            .or_else(|| self.snap.indicators.as_ref().and_then(|i| i.last_close))
            .filter(|p| p.is_finite() && *p > 0.0);
        let l = self.lang;
        // Niveles que existen de verdad en los datos del snapshot.
        let known = data_levels(self.snap);
        let in_data = |_: &NumberIndex, x: f64| known.iter().any(|k| (k - x).abs() <= k.abs() * 0.0005 + 1e-9);
        let mut pick = |model: &[f64], from_data: &[f64], supports: bool| -> Vec<f64> {
            let mut v: Vec<f64> = model.iter().copied().filter(|x| x.is_finite() && *x > 0.0 && in_data(&self.idx, *x)).collect();
            if v.len() < model.len() {
                let w = tr!(l, "Se descartaron niveles que no figuran en los datos.", "Levels that are not in the data were dropped.");
                if !self.warnings.contains(&w) {
                    self.warnings.push(w);
                }
            }
            if v.is_empty() {
                v = from_data.iter().copied().filter(|x| x.is_finite() && *x > 0.0).take(3).collect();
            }
            if let Some(p) = price {
                v.sort_by(|a, b| (a - p).abs().total_cmp(&(b - p).abs()));
                // Un soporte no puede estar por encima del precio ni una resistencia por debajo.
                v.retain(|x| if supports { *x <= p * 1.001 } else { *x >= p * 0.999 });
            }
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            v.truncate(4);
            v
        };
        let ind = self.snap.indicators.as_ref();
        let supports = pick(&raw.supports, ind.map_or(&[][..], |i| &i.supports), true);
        let resistances = pick(&raw.resistances, ind.map_or(&[][..], |i| &i.resistances), false);

        let mut out = KeyLevels { supports, resistances, ..KeyLevels::default() };
        let enters = matches!(verdict, Verdict::Buy | Verdict::Accumulate | Verdict::Hold | Verdict::Wait);
        let mut derived = false;
        if let (true, Some(p)) = (enters, price) {
            let sane = |x: f64| x.is_finite() && x >= p * 0.3 && x <= p * 3.0;
            if let [lo, hi] = raw.entry_zone[..]
                && sane(lo)
                && sane(hi)
                && lo < hi
                && hi <= p * 1.15
            {
                out.entry_zone = Some((lo, hi));
                derived = true;
            }
            let floor = out.entry_zone.map_or(p, |z| z.0);
            if let Some(s) = raw.stop_loss.filter(|s| sane(*s) && *s < floor) {
                out.stop_loss = Some(s);
                derived = true;
            }
            let mut t: Vec<f64> = raw.targets.iter().copied().filter(|x| sane(*x) && *x > p).collect();
            t.sort_by(f64::total_cmp);
            t.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            t.truncate(3);
            derived |= !t.is_empty();
            out.targets = t;
        }
        if derived {
            self.warn(tr!(
                l,
                "La zona de entrada, el stop y los objetivos son orientativos y los deriva el modelo de los niveles de los datos; no son datos de mercado.",
                "Entry zone, stop and targets are indicative and derived by the model from the data's levels; they are not market data."
            ));
        }
        out
    }
}

/// Todos los niveles de precio presentes en el snapshot (medias, bandas, extremos, soportes…).
fn data_levels(snap: &AssetSnapshot) -> Vec<f64> {
    let mut v = Vec::new();
    if let Some(i) = &snap.indicators {
        v.extend(i.supports.iter().chain(&i.resistances).copied());
        v.extend([i.sma20, i.sma50, i.sma200, i.ema12, i.ema26, i.bb_upper, i.bb_middle, i.bb_lower, i.high52w, i.low52w, i.last_close].into_iter().flatten());
    }
    if let Some(q) = &snap.quote {
        v.extend([Some(q.price), q.open, q.high, q.low, q.prev_close].into_iter().flatten());
    }
    if let Some(f) = &snap.fundamentals {
        v.extend([f.analyst_target_mean, f.analyst_target_high, f.analyst_target_low, f.all_time_high].into_iter().flatten());
    }
    v.retain(|x| x.is_finite() && *x > 0.0);
    v
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
    fn prose_marks_unverified_figures() {
        let f = fx();
        let mut v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        let out = v.prose("Esperamos un 77.7 % de subida", "tesis");
        assert!(out.contains('⚠'), "{out}");
        assert_eq!(v.marked, 1);
        assert!(v.warnings[0].contains("tesis"));
    }

    #[test]
    fn levels_use_data_levels_and_sanity_check_the_rest() {
        let f = fx();
        let p = f.snap.quote.as_ref().unwrap().price;
        let ind = f.snap.indicators.as_ref().unwrap();
        let real_support = ind.supports[0];
        let mut v = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es);
        let raw = LevelsOut {
            supports: vec![real_support, 1.23],
            resistances: vec![],
            entry_zone: vec![p * 0.97, p * 0.995],
            stop_loss: Some(p * 0.9),
            targets: vec![p * 1.1, p * 0.5, p * 50.0],
        };
        let k = v.levels(&raw, Verdict::Accumulate);
        assert_eq!(k.supports, vec![real_support]);
        assert!(!k.resistances.is_empty(), "sin resistencias del modelo se usan las de los datos");
        assert!(k.entry_zone.is_some() && k.stop_loss.is_some());
        assert_eq!(k.targets.len(), 1, "solo el objetivo por encima del precio y razonable");
        // Con veredicto de salida no se proponen entradas.
        let k = Verifier::new(&f.text, &f.snap, &f.reg, Lang::Es).levels(&raw, Verdict::Avoid);
        assert!(k.entry_zone.is_none() && k.stop_loss.is_none() && k.targets.is_empty());
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
