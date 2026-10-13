//! Niveles de operación calculados en Rust a partir del snapshot: nada de lo que sale de aquí lo
//! inventa el modelo.
//!
//! - **Zona de entrada**: entre el soporte más cercano por debajo del precio y el precio (o, si el
//!   veredicto es esperar, justo sobre ese soporte).
//! - **Stop**: soporte de referencia − 1·ATR14 (`STOP_ATR_MULT`). Sin soporte o sin ATR: `None`.
//! - **Objetivos**: las resistencias (y otros niveles de los datos) por encima del precio.
//! - **Escenarios**: precio objetivo de cada escenario = un nivel concreto de los datos.
//! - **Tamaño de posición**: riesgo fijo por operación (1 % de la cartera) / distancia al stop,
//!   con un tope según la puntuación de riesgo.

use domain::{AssetSnapshot, KeyLevels, PositionSizing, Verdict};

use crate::{
    fmt,
    lang::{Lang, tr},
    scoring::Scorecard,
};

pub const STOP_ATR_MULT: f64 = 1.0;
const RISK_PER_TRADE_PCT: f64 = 1.0;

/// Nivel con su origen, para poder explicar de dónde sale cada número.
#[derive(Debug, Clone)]
pub struct Level {
    pub value: f64,
    /// Nombre del nivel («SMA50», «soporte»…) o, si es derivado, la fórmula completa.
    pub basis: String,
    /// Cifras de las que sale (para validar que un texto solo cita las de este nivel).
    pub refs: Vec<f64>,
    /// `basis` ya lleva sus cifras (nivel derivado): no hay que añadir el valor.
    pub derived: bool,
}

impl Level {
    /// «SMA50 81.059,79» (o la fórmula completa si es derivado), con formato para el usuario.
    pub fn describe(&self, lang: Lang) -> String {
        if self.derived { self.basis.clone() } else { format!("{} {}", self.basis, fmt::price_l(lang, self.value)) }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LevelPlan {
    pub price: f64,
    pub atr: Option<f64>,
    /// Por debajo del precio, de más cerca a más lejos.
    pub supports: Vec<Level>,
    /// Por encima del precio, de más cerca a más lejos.
    pub upside: Vec<Level>,
    pub entry_zone: Option<(f64, f64)>,
    pub stop_loss: Option<f64>,
    pub stop_basis: Option<String>,
    pub targets: Vec<Level>,
    pub reward_risk: Option<f64>,
    /// Frases sobre lo que falta o cómo se calculó, para el usuario (cifras en formato castellano).
    pub notes: Vec<String>,
    /// Las mismas frases con punto decimal, para el prompt del juez (se compara textualmente).
    pub prompt_notes: Vec<String>,
    /// «Binance, 12 oct 2026»: proveedor y fecha del precio sobre el que se calcula todo.
    pub source: String,
}

/// «Binance, 12 oct 2026»: de dónde y de cuándo es el precio sobre el que se calcula.
pub fn source_label(snap: &AssetSnapshot, lang: Lang) -> String {
    match &snap.quote {
        Some(q) => format!("{}, {}", fmt::provider_name(&q.source), fmt::date_l(lang, q.timestamp.date_naive())),
        None => fmt::date_l(lang, snap.generated_at.date_naive()),
    }
}

impl LevelPlan {
    /// Añade una frase a las notas de usuario (cifras en castellano) y del prompt (punto decimal).
    fn note_both(&mut self, f: impl Fn(&dyn Fn(f64) -> String) -> String, lang: Lang) {
        self.notes.push(f(&|x| fmt::price_l(lang, x)));
        self.prompt_notes.push(f(&|x| fmt::price(x)));
    }
    fn note_plain(&mut self, s: String) {
        self.notes.push(s.clone());
        self.prompt_notes.push(s);
    }
}

fn price_of(snap: &AssetSnapshot) -> Option<f64> {
    snap.quote
        .as_ref()
        .map(|q| q.price)
        .or_else(|| snap.indicators.as_ref().and_then(|i| i.last_close))
        .filter(|p| p.is_finite() && *p > 0.0)
}

fn dedupe(mut v: Vec<Level>, price: f64, ascending: bool) -> Vec<Level> {
    v.retain(|l| l.value.is_finite() && l.value > 0.0);
    v.sort_by(|a, b| a.value.total_cmp(&b.value));
    if !ascending {
        v.reverse();
    }
    let mut out: Vec<Level> = Vec::new();
    for l in v {
        // Dos niveles a menos del 0,4 % del precio son el mismo para operar.
        if out.iter().all(|o| (o.value - l.value).abs() > price * 0.004) {
            out.push(l);
        }
    }
    out
}

/// Soportes (debajo del precio) y niveles al alza (encima) presentes en los datos.
fn collect(snap: &AssetSnapshot, price: f64, lang: Lang) -> (Vec<Level>, Vec<Level>) {
    let mut sup = Vec::new();
    let mut up = Vec::new();
    let mut add = |value: Option<f64>, basis: &str| {
        let Some(v) = value.filter(|v| v.is_finite() && *v > 0.0) else { return };
        let l = Level { value: v, basis: basis.to_string(), refs: vec![v], derived: false };
        if v < price * 0.999 {
            sup.push(l);
        } else if v > price * 1.001 {
            up.push(l);
        }
    };
    if let Some(i) = &snap.indicators {
        for s in &i.supports {
            add(Some(*s), lang.t("soporte", "support"));
        }
        for r in &i.resistances {
            add(Some(*r), lang.t("resistencia", "resistance"));
        }
        add(i.sma20, "SMA20");
        add(i.sma50, "SMA50");
        add(i.sma200, "SMA200");
        add(i.bb_lower, lang.t("banda de Bollinger inferior", "lower Bollinger band"));
        add(i.bb_upper, lang.t("banda de Bollinger superior", "upper Bollinger band"));
        add(i.low52w, lang.t("mínimo de 52 semanas", "52-week low"));
        add(i.high52w, lang.t("máximo de 52 semanas", "52-week high"));
    }
    if let Some(f) = &snap.fundamentals {
        add(f.analyst_target_mean, lang.t("precio objetivo medio de analistas", "mean analyst target"));
        add(f.analyst_target_high, lang.t("precio objetivo máximo de analistas", "highest analyst target"));
        add(f.all_time_high, lang.t("máximo histórico", "all-time high"));
    }
    (dedupe(sup, price, false), dedupe(up, price, true))
}

/// Calcula el plan. `None` solo si no hay precio.
pub fn plan(snap: &AssetSnapshot, verdict: Verdict, lang: Lang) -> Option<LevelPlan> {
    let price = price_of(snap)?;
    let atr = snap.indicators.as_ref().and_then(|i| i.atr14).filter(|a| a.is_finite() && *a > 0.0);
    let (supports, upside) = collect(snap, price, lang);
    let mut p = LevelPlan { price, atr, supports: supports.clone(), upside: upside.clone(), source: source_label(snap, lang), ..Default::default() };

    let actionable = matches!(verdict, Verdict::Buy | Verdict::Accumulate | Verdict::Hold | Verdict::Wait);
    if !actionable {
        p.note_plain(tr!(
            lang,
            "Con un veredicto de reducir o evitar no se proponen zona de entrada, stop ni objetivos.",
            "With a reduce/avoid verdict no entry zone, stop or targets are proposed."
        ));
        return Some(p);
    }

    // --- Zona de entrada ---
    let s1 = supports.first().cloned();
    p.entry_zone = match (verdict, atr, &s1) {
        (Verdict::Wait, Some(a), Some(s)) => Some((s.value, s.value + 0.5 * a)),
        (Verdict::Wait, None, Some(s)) => Some((s.value, s.value * 1.005)),
        (Verdict::Wait, _, None) => None,
        (_, Some(a), Some(s)) => Some((s.value.max(price - a), price)),
        (_, Some(a), None) => Some((price - a, price)),
        (_, None, Some(s)) if (price - s.value) / price <= 0.08 => Some((s.value, price)),
        _ => None,
    };
    match (&p.entry_zone, verdict) {
        (None, Verdict::Wait) => p.note_plain(tr!(
            lang,
            "Sin zona de entrada: los datos no muestran ningún soporte por debajo del precio sobre el que esperar.",
            "No entry zone: the data shows no support below the price to wait for."
        )),
        (None, _) => p.note_plain(tr!(
            lang,
            "Sin zona de entrada: faltan el ATR y un soporte cercano en los datos.",
            "No entry zone: ATR and a nearby support are missing from the data."
        )),
        (&Some((lo, hi)), Verdict::Wait) => {
            let sv = s1.as_ref().map(|s| s.value).unwrap_or(lo);
            let src = p.source.clone();
            p.note_both(
                |px| {
                    tr!(
                        lang,
                        "Zona de entrada {} – {} = sobre el soporte más cercano {}, con un margen de 0,5×ATR14 ({}).",
                        "Entry zone {} – {} = just above the nearest support {}, with 0.5×ATR14 of room ({}).",
                        px(lo),
                        px(hi),
                        px(sv),
                        src
                    )
                },
                lang,
            );
        }
        (&Some((lo, hi)), _) => {
            let src = p.source.clone();
            p.note_both(
                |px| {
                    tr!(
                        lang,
                        "Zona de entrada {} – {} = desde el soporte más cercano o 1×ATR14 bajo el precio (el más cercano de los dos) hasta el precio actual ({}).",
                        "Entry zone {} – {} = from the nearest support or 1×ATR14 below the price (whichever is closer) up to the current price ({}).",
                        px(lo),
                        px(hi),
                        src
                    )
                },
                lang,
            );
        }
    }

    // --- Stop ---
    let ref_price = p.entry_zone.map(|z| z.0);
    if let (Some(a), Some(low)) = (atr, ref_price) {
        if let Some(s) = supports.iter().find(|s| s.value <= low * 1.0005) {
            let stop = s.value - STOP_ATR_MULT * a;
            if stop > 0.0 {
                p.stop_loss = Some(stop);
                let (sb, sv, src) = (s.basis.clone(), s.value, p.source.clone());
                p.stop_basis = Some(format!("{sb} {} − {STOP_ATR_MULT}×ATR14 {}", fmt::price_l(lang, sv), fmt::price_l(lang, a)));
                p.note_both(
                    |px| {
                        tr!(
                            lang,
                            "Stop {} = {} {} − {}×ATR14 {} ({}).",
                            "Stop {} = {} {} − {}×ATR14 {} ({}).",
                            px(stop),
                            sb,
                            px(sv),
                            STOP_ATR_MULT,
                            px(a),
                            src
                        )
                    },
                    lang,
                );
            }
        }
        if p.stop_loss.is_none() {
            p.note_plain(tr!(
                lang,
                "Sin stop: los datos no tienen ningún soporte en o bajo la zona de entrada sobre el que calcularlo.",
                "No stop: the data has no support at or below the entry zone to base it on."
            ));
        }
    } else if p.entry_zone.is_some() {
        p.note_plain(tr!(
            lang,
            "Sin stop: falta el ATR14 en los datos.",
            "No stop: ATR14 is missing from the data."
        ));
    }

    // --- Objetivos ---
    p.targets = upside.iter().take(3).cloned().collect();
    if p.targets.is_empty() {
        p.note_plain(tr!(
            lang,
            "Sin objetivos: el precio está por encima de todas las resistencias y máximos de los datos.",
            "No targets: the price is above every resistance and high in the data."
        ));
    } else {
        let (targets, src) = (p.targets.clone(), p.source.clone());
        p.note_both(
            |px| {
                let list: Vec<String> = targets.iter().map(|t| format!("{} = {}", px(t.value), t.basis)).collect();
                tr!(lang, "Objetivos: {} ({}).", "Targets: {} ({}).", list.join("; "), src)
            },
            lang,
        );
    }

    // --- Recompensa/riesgo ---
    if let (Some(stop), Some(t1), Some((lo, hi))) = (p.stop_loss, p.targets.first(), p.entry_zone) {
        let entry = (lo + hi) / 2.0;
        let risk = entry - stop;
        if risk > 0.0 && t1.value > entry {
            let rr = (t1.value - entry) / risk;
            p.reward_risk = Some((rr * 10.0).round() / 10.0);
            p.note_both(
                |px| {
                    tr!(
                        lang,
                        "Relación recompensa/riesgo hasta el primer objetivo: {:.1} (entrada en el punto medio de la zona, {}).",
                        "Reward/risk to the first target: {:.1} (entry at the zone midpoint, {}).",
                        rr,
                        px(entry)
                    )
                },
                lang,
            );
        }
    }
    Some(p)
}

impl LevelPlan {
    pub fn key_levels(&self, snap: &AssetSnapshot) -> KeyLevels {
        let ind = snap.indicators.as_ref();
        let mut supports: Vec<f64> = self.supports.iter().take(4).map(|l| l.value).collect();
        let mut resistances: Vec<f64> = self.upside.iter().take(4).map(|l| l.value).collect();
        // Los de `indicators.supports/resistances` son los «oficiales»; los demás niveles (medias,
        // máximos) ya están en el plan.
        if supports.is_empty() {
            supports = ind.map_or_else(Vec::new, |i| i.supports.iter().copied().filter(|x| *x < self.price).take(3).collect());
        }
        if resistances.is_empty() {
            resistances = ind.map_or_else(Vec::new, |i| i.resistances.iter().copied().filter(|x| *x > self.price).take(3).collect());
        }
        KeyLevels {
            supports,
            resistances,
            entry_zone: self.entry_zone,
            stop_loss: self.stop_loss,
            targets: self.targets.iter().map(|t| t.value).collect(),
        }
    }

    /// Bloque de texto para el juez (sus cifras se añaden al índice de verificación).
    pub fn prompt_block(&self, lang: Lang) -> String {
        let mut s = String::new();
        s.push_str(lang.t(
            "## Niveles calculados por el sistema (NO los cambies ni inventes otros)\n",
            "## Levels computed by the system (do NOT change them or invent others)\n",
        ));
        let none = lang.t("sin datos", "n/a");
        s.push_str(&format!(
            "{}: {}\n",
            lang.t("Zona de entrada", "Entry zone"),
            self.entry_zone.map_or(none.to_string(), |(a, b)| format!("{} – {}", fmt::price(a), fmt::price(b)))
        ));
        s.push_str(&format!("Stop: {}\n", self.stop_loss.map_or(none.to_string(), fmt::price)));
        let t: Vec<String> = self.targets.iter().map(|t| fmt::price(t.value)).collect();
        s.push_str(&format!("{}: {}\n", lang.t("Objetivos", "Targets"), if t.is_empty() { none.to_string() } else { t.join(" / ") }));
        if let Some(rr) = self.reward_risk {
            s.push_str(&format!("{}: {rr:.1}\n", lang.t("Recompensa/riesgo", "Reward/risk")));
        }
        for n in &self.prompt_notes {
            s.push_str(&format!("- {n}\n"));
        }
        s
    }

    pub fn note(&self) -> String {
        self.notes.join(" ")
    }
}

/// Tamaño de posición orientativo. `None` si no hay ATR ni volatilidad con que medirlo.
pub fn sizing(plan: &LevelPlan, snap: &AssetSnapshot, card: &Scorecard, verdict: Verdict, lang: Lang) -> Option<PositionSizing> {
    let ind = snap.indicators.as_ref();
    let atr_pct = plan.atr.map(|a| a / plan.price * 100.0);
    let vol = ind.and_then(|i| i.volatility30d).filter(|v| v.is_finite());
    if atr_pct.is_none() && vol.is_none() {
        return None;
    }
    if matches!(verdict, Verdict::Reduce | Verdict::Avoid) {
        return Some(PositionSizing {
            max_portfolio_pct: 0.0,
            risk_per_trade_pct: None,
            stop_distance_pct: None,
            atr_pct,
            volatility30d: vol,
            rationale: tr!(lang, "Con este veredicto no se recomienda abrir ni ampliar posición.", "With this verdict no new or larger position is advised."),
        });
    }
    let stop_dist_pct = match (plan.stop_loss, plan.entry_zone) {
        (Some(s), Some((lo, hi))) => Some(((lo + hi) / 2.0 - s) / ((lo + hi) / 2.0) * 100.0),
        (Some(s), None) => Some((plan.price - s) / plan.price * 100.0),
        _ => atr_pct.map(|a| 2.0 * a),
    }
    .filter(|d| d.is_finite() && *d > 0.0)?;
    let has_stop = plan.stop_loss.is_some();
    let risk_score = card.scores.risk.unwrap_or(50.0);
    let cap = if risk_score > 70.0 || vol.is_some_and(|v| v > 0.8) {
        3.0
    } else if risk_score > 55.0 || vol.is_some_and(|v| v > 0.5) {
        5.0
    } else if risk_score > 40.0 {
        8.0
    } else {
        10.0
    };
    let raw = RISK_PER_TRADE_PCT / stop_dist_pct * 100.0;
    let pct = ((raw.min(cap)) * 2.0).floor() / 2.0;
    let rationale = tr!(
        lang,
        "Con un riesgo máximo de {:.0} % de la cartera por operación y {} a {:.1} % de la entrada, el cálculo daría {:.1} % de la cartera; se limita a {:.0} % por el riesgo del activo (puntuación de riesgo {:.0}/100). Orientativo, no es asesoramiento.",
        "With a maximum risk of {:.0}% of the portfolio per trade and {} {:.1}% from entry, the calculation gives {:.1}% of the portfolio; it is capped at {:.0}% for the asset's risk (risk score {:.0}/100). Indicative, not advice.",
        RISK_PER_TRADE_PCT,
        if has_stop { lang.t("el stop", "the stop") } else { lang.t("una distancia de 2×ATR", "a 2×ATR distance") },
        stop_dist_pct,
        raw,
        cap,
        risk_score
    );
    Some(PositionSizing {
        max_portfolio_pct: pct,
        risk_per_trade_pct: Some(RISK_PER_TRADE_PCT),
        stop_distance_pct: Some((stop_dist_pct * 10.0).round() / 10.0),
        atr_pct: atr_pct.map(|a| (a * 100.0).round() / 100.0),
        volatility30d: vol,
        rationale,
    })
}

/// Precios de los tres escenarios, cada uno un nivel concreto de los datos.
#[derive(Debug, Clone, Default)]
pub struct ScenarioLevels {
    pub bullish: Option<Level>,
    pub base: Option<Level>,
    pub bearish: Option<Level>,
}

pub fn scenario_levels(plan: &LevelPlan, lang: Lang) -> ScenarioLevels {
    let bullish = match plan.upside.as_slice() {
        [] => None,
        [a] => Some(a.clone()),
        [_, b, ..] => Some(b.clone()),
    };
    let base = match (plan.supports.first(), plan.upside.first()) {
        (Some(s), Some(r)) => Some(Level {
            value: (s.value + r.value) / 2.0,
            basis: tr!(
                lang,
                "punto medio entre el soporte {} y la resistencia {}",
                "midpoint between support {} and resistance {}",
                fmt::price_l(lang, s.value),
                fmt::price_l(lang, r.value)
            ),
            refs: vec![s.value, r.value, (s.value + r.value) / 2.0],
            derived: true,
        }),
        _ => None,
    };
    let bearish = match plan.supports.as_slice() {
        [] => None,
        [_, b, ..] => Some(b.clone()),
        [a] => plan.atr.map(|atr| Level {
            value: a.value - atr,
            basis: tr!(lang, "{} {} − 1×ATR14 {}", "{} {} − 1×ATR14 {}", a.basis, fmt::price_l(lang, a.value), fmt::price_l(lang, atr)),
            refs: vec![a.value, atr, a.value - atr],
            derived: true,
        }),
    };
    ScenarioLevels { bullish, base, bearish }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixture, scoring};

    #[test]
    fn plan_is_derived_only_from_data_levels_and_atr() {
        let snap = fixture::stock_snapshot();
        let ind = snap.indicators.as_ref().unwrap();
        let atr = ind.atr14.unwrap();
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        let (lo, hi) = p.entry_zone.expect("zona de entrada");
        assert!(lo < hi && hi == p.price);
        let stop = p.stop_loss.expect("stop");
        // stop = soporte de los datos − 1·ATR
        assert!(
            p.supports.iter().any(|s| ((s.value - atr * STOP_ATR_MULT) - stop).abs() < 1e-9),
            "stop {stop} no sale de ningún soporte − ATR"
        );
        assert!(stop < lo);
        // Los objetivos son exactamente niveles de los datos por encima del precio.
        assert!(!p.targets.is_empty());
        for t in &p.targets {
            assert!(t.value > p.price);
            let known = ind.resistances.iter().chain(&ind.supports).chain([&ind.high52w.unwrap_or(0.0)]).any(|r| (r - t.value).abs() < 1e-9)
                || [ind.sma20, ind.sma50, ind.sma200, ind.bb_upper, ind.bb_lower, ind.low52w].into_iter().flatten().any(|r| (r - t.value).abs() < 1e-9)
                || snap.fundamentals.as_ref().is_some_and(|f| [f.analyst_target_mean, f.analyst_target_high, f.all_time_high].into_iter().flatten().any(|r| (r - t.value).abs() < 1e-9));
            assert!(known, "objetivo {} inventado", t.value);
        }
        assert!(p.reward_risk.is_some_and(|r| r > 0.0));
        let kl = p.key_levels(&snap);
        assert_eq!(kl.stop_loss, Some(stop));
    }

    #[test]
    fn no_support_or_atr_means_none_with_explanation() {
        let mut snap = fixture::stock_snapshot();
        let ind = snap.indicators.as_mut().unwrap();
        ind.atr14 = None;
        let p = plan(&snap, Verdict::Buy, Lang::Es).unwrap();
        assert!(p.stop_loss.is_none());
        assert!(p.note().contains("Sin stop"), "{}", p.note());

        let mut snap = fixture::stock_snapshot();
        let ind = snap.indicators.as_mut().unwrap();
        ind.supports.clear();
        ind.sma20 = None;
        ind.sma50 = None;
        ind.sma200 = None;
        ind.bb_lower = None;
        ind.low52w = None;
        let p = plan(&snap, Verdict::Wait, Lang::Es).unwrap();
        assert!(p.entry_zone.is_none() && p.stop_loss.is_none());
        assert!(p.note().contains("Sin zona de entrada"));

        let p = plan(&fixture::stock_snapshot(), Verdict::Avoid, Lang::Es).unwrap();
        assert!(p.entry_zone.is_none() && p.stop_loss.is_none() && p.targets.is_empty());
        assert!(p.note().contains("reducir o evitar"));
    }

    #[test]
    fn no_resistance_above_price_gives_no_targets() {
        let mut snap = fixture::stock_snapshot();
        let price = snap.quote.as_ref().unwrap().price;
        let ind = snap.indicators.as_mut().unwrap();
        ind.resistances.clear();
        ind.high52w = Some(price * 0.9);
        ind.bb_upper = None;
        if let Some(f) = snap.fundamentals.as_mut() {
            f.analyst_target_mean = None;
            f.analyst_target_high = None;
            f.all_time_high = None;
        }
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        assert!(p.targets.is_empty());
        assert!(p.note().contains("Sin objetivos"));
    }

    #[test]
    fn sizing_follows_stop_distance_and_risk_cap() {
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::Es);
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        let s = sizing(&p, &snap, &card, Verdict::Accumulate, Lang::Es).unwrap();
        assert!(s.max_portfolio_pct > 0.0 && s.max_portfolio_pct <= 10.0);
        let d = s.stop_distance_pct.unwrap();
        assert!(s.max_portfolio_pct <= 100.0 / d + 0.5, "{} vs {}", s.max_portfolio_pct, d);
        let none = sizing(&p, &snap, &card, Verdict::Avoid, Lang::Es).unwrap();
        assert_eq!(none.max_portfolio_pct, 0.0);
    }

    #[test]
    fn scenario_levels_come_from_plan() {
        let snap = fixture::stock_snapshot();
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        let sc = scenario_levels(&p, Lang::Es);
        let bull = sc.bullish.unwrap();
        let bear = sc.bearish.unwrap();
        assert!(bull.value > p.price && bear.value < p.price);
        assert!(sc.base.is_some());
    }

    #[test]
    fn the_source_in_the_notes_uses_the_provider_display_name() {
        // TODO 4: «(binance, 13 oct 2026)» → «(Binance, 13 oct 2026)».
        let mut snap = fixture::stock_snapshot();
        {
            let q = snap.quote.as_mut().unwrap();
            q.source = "binance".into();
            q.timestamp = "2026-10-13T10:00:00Z".parse().unwrap();
        }
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        assert_eq!(p.source, "Binance, 13 oct 2026");
        assert!(p.note().contains("(Binance, 13 oct 2026)") && !p.note().contains("binance"), "{}", p.note());
        snap.quote.as_mut().unwrap().source = "yahoo".into();
        assert_eq!(source_label(&snap, Lang::Es), "Yahoo Finance, 13 oct 2026");
    }

    #[test]
    fn level_notes_state_basis_and_source_in_spanish_but_the_prompt_keeps_dots() {
        let mut snap = fixture::stock_snapshot();
        snap.quote.as_mut().unwrap().price = 81_900.0;
        let p = plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        let note = p.note();
        assert!(note.contains(&format!("({})", p.source)) && p.source.contains(", "), "{note}");
        assert!(note.contains("Stop ") && note.contains("ATR14"), "{note}");
        let stop = p.stop_loss.unwrap();
        assert!(note.contains(&fmt::price_l(Lang::Es, stop)));
        // El prompt del juez conserva el formato con punto (se compara textualmente).
        assert!(p.prompt_block(Lang::Es).contains(&fmt::price(stop)));
    }
}
