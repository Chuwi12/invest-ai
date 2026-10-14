//! Cifras derivadas que un analista calcularía a mano (potencial hasta el objetivo, distancia a un
//! soporte, recompensa/riesgo…), calculadas en Rust y entregadas al LLM en su propia sección del
//! contexto («Cifras derivadas»). El modelo interpreta, no calcula: las evaluaciones mostraron que el
//! 7B y qwen3 inventaban «recompensa/riesgo 6,7», tomaban la distancia al máximo como «corrección
//! esperada» o leían como ganancia un objetivo de analistas por debajo del precio.
//!
//! Reglas:
//! - Sin dato de base → la fila no existe (nunca se inventa ni se rellena).
//! - Porcentajes con signo explícito (`+9.0%` / `-3.2%`) y su fórmula en la tercera columna.
//! - Distancias a máximos, mínimos y medias son DESCRIPTIVAS, no previsiones: la cabecera lo dice.
//! - Las cuentas (FCF) y la capitalización solo se dividen si están en la misma moneda (R-05).
//! - La recompensa/riesgo y los niveles son los del plan de `levels.rs` que ve el juez (veredicto
//!   «acumular»), así que la cifra coincide con la del informe.

use domain::{AssetClass, AssetSnapshot, Fundamentals, Verdict};

use crate::{
    analysis::levels,
    fmt,
    lang::Lang,
};

/// Grupos de filas (máscara de bits en `Sections::derived`).
pub const ANALYST: u8 = 1;
pub const LEVELS: u8 = 2;
pub const RANGE: u8 = 4;
pub const TECH: u8 = 8;
pub const VALUATION: u8 = 16;
/// Detalle de niveles: soportes y resistencias de los datos, zona de entrada y mínimo de 52 semanas
/// (la etapa técnica y el contexto completo; el juez ya recibe el plan y va justo de espacio).
pub const DETAIL: u8 = 32;
pub const ALL: u8 = ANALYST | LEVELS | RANGE | TECH | VALUATION | DETAIL;

/// Una cifra derivada: `| label | value | basis |`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub group: u8,
    /// Imprescindible: se conserva aunque el contexto se reduzca por presupuesto.
    pub core: bool,
    pub label: String,
    pub value: String,
    pub basis: String,
}

fn good(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

fn price_of(snap: &AssetSnapshot) -> Option<f64> {
    positive(snap.quote.as_ref().map(|q| q.price)).or_else(|| positive(snap.indicators.as_ref().and_then(|i| i.last_close)))
}

/// (a − b)/b como fracción; `None` si la base no es positiva.
fn rel(a: f64, b: f64) -> Option<f64> {
    (a.is_finite() && b.is_finite() && b > 0.0).then(|| (a - b) / b)
}

fn signed(f: f64) -> String {
    fmt::pct_signed(f)
}

/// Monedas de cotización y de las cuentas: `Some(false)` si se sabe que difieren.
fn same_currency(snap: &AssetSnapshot, f: &Fundamentals) -> bool {
    let price_cur = f.currency.as_deref().or(snap.quote.as_ref().and_then(|q| q.currency.as_deref())).or(snap.summary.currency.as_deref());
    let fin_cur = f.extra.get("financialCurrency").and_then(|v| v.as_str()).map(str::trim).filter(|c| !c.is_empty());
    match (fin_cur, price_cur) {
        (Some(fc), Some(pc)) => fc.eq_ignore_ascii_case(pc),
        _ => true,
    }
}

/// Las cifras derivadas disponibles. Vacío si no hay precio.
pub fn compute(snap: &AssetSnapshot, lang: Lang) -> Vec<Row> {
    let Some(price) = price_of(snap) else { return vec![] };
    let mut rows: Vec<Row> = Vec::new();
    let mut add = |group: u8, core: bool, label: String, value: String, basis: String| {
        rows.push(Row { group, core, label, value, basis });
    };
    let join = |v: Vec<String>| v.join(", ");

    let ind = snap.indicators.as_ref();
    let fun = snap.fundamentals.as_ref();

    // --- Analistas ---
    if let Some(f) = fun
        && f.analyst_count != Some(0)
        && f.currency.as_deref().zip(snap.quote.as_ref().and_then(|q| q.currency.as_deref())).is_none_or(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        let parts: Vec<String> = [
            (f.analyst_target_mean, lang.t("medio", "mean")),
            (f.analyst_target_high, lang.t("máximo", "high")),
            (f.analyst_target_low, lang.t("mínimo", "low")),
        ]
        .into_iter()
        .filter_map(|(t, name)| positive(t).and_then(|t| rel(t, price)).map(|r| format!("{name} {}", signed(r))))
        .collect();
        if !parts.is_empty() {
            add(
                ANALYST,
                true,
                lang.t("Potencial hasta el objetivo de analistas", "Upside to the analyst target").into(),
                join(parts),
                lang.t("(objetivo − precio)/precio; negativo = objetivo bajo el precio", "(target − price)/price; negative = target below the price").into(),
            );
        }
    }

    // --- Niveles de operación (los mismos que ve el juez) ---
    if let Some(i) = ind {
        let near = |v: &[f64], below: bool| {
            let mut x: Vec<f64> = v.iter().copied().filter(|x| x.is_finite() && *x > 0.0 && if below { *x < price * 0.999 } else { *x > price * 1.001 }).collect();
            x.sort_by(|a, b| if below { b.total_cmp(a) } else { a.total_cmp(b) });
            x.dedup();
            x.into_iter().take(2).filter_map(|x| rel(x, price)).map(signed).collect::<Vec<_>>()
        };
        let sup = near(&i.supports, true);
        if !sup.is_empty() {
            add(DETAIL, false, lang.t("Distancia a los soportes (cercano primero)", "Distance to supports (nearest first)").into(), join(sup), lang.t("(soporte − precio)/precio", "(support − price)/price").into());
        }
        let res = near(&i.resistances, false);
        if !res.is_empty() {
            add(DETAIL, false, lang.t("Distancia a las resistencias (cercana primero)", "Distance to resistances (nearest first)").into(), join(res), lang.t("(resistencia − precio)/precio", "(resistance − price)/price").into());
        }
    }
    if let Some(plan) = levels::plan(snap, Verdict::Accumulate, lang) {
        if let Some((lo, hi)) = plan.entry_zone {
            let v: Vec<String> = [lo, hi].iter().filter_map(|x| rel(*x, price)).map(signed).collect();
            if v.len() == 2 {
                add(DETAIL, false, lang.t("Distancia a la zona de entrada (inferior, superior)", "Distance to the entry zone (lower, upper)").into(), join(v), lang.t("(entrada − precio)/precio", "(entry − price)/price").into());
            }
        }
        if let Some(s) = plan.stop_loss.and_then(|s| rel(s, price)) {
            add(LEVELS, true, lang.t("Distancia al stop", "Distance to the stop").into(), signed(s), lang.t("(stop − precio)/precio", "(stop − price)/price").into());
        }
        let t: Vec<String> = plan.targets.iter().filter_map(|t| rel(t.value, price)).map(signed).collect();
        if !t.is_empty() {
            add(LEVELS, true, lang.t("Distancia a los objetivos del plan", "Distance to the plan's targets").into(), join(t), lang.t("(objetivo − precio)/precio", "(target − price)/price").into());
        }
        if let Some(rr) = plan.reward_risk {
            add(
                LEVELS,
                true,
                lang.t("Recompensa/riesgo hasta el primer objetivo", "Reward/risk to the first target").into(),
                format!("{rr:.1}"),
                lang.t("(objetivo − entrada)/(entrada − stop)", "(target − entry)/(entry − stop)").into(),
            );
        }
    }

    // --- Rango y máximo histórico (descriptivo, no previsión) ---
    if let Some(i) = ind {
        if let Some(d) = positive(i.high52w).and_then(|h| rel(price, h)) {
            add(RANGE, true, lang.t("Distancia al máximo de 52 semanas", "Distance to the 52-week high").into(), signed(d), lang.t("(precio − máximo)/máximo", "(price − high)/high").into());
        }
        if let Some(d) = positive(i.low52w).and_then(|l| rel(price, l)) {
            add(DETAIL, false, lang.t("Distancia al mínimo de 52 semanas", "Distance to the 52-week low").into(), signed(d), lang.t("(precio − mínimo)/mínimo", "(price − low)/low").into());
        }
    }
    if let Some(f) = fun {
        if let Some(d) = positive(f.all_time_high).and_then(|a| rel(price, a)) {
            add(RANGE, true, lang.t("Distancia al máximo histórico", "Distance to the all-time high").into(), signed(d), lang.t("(precio − máximo)/máximo", "(price − high)/high").into());
        }
        if let (Some(c), Some(m)) = (positive(f.circulating_supply), positive(f.max_supply))
            && c <= m * 1.0001
        {
            add(RANGE, false, lang.t("Oferta circulante sobre oferta máxima", "Circulating supply over max supply").into(), fmt::pct(c / m), lang.t("circulante / máxima", "circulating / max").into());
        }
    }

    // --- Técnico ---
    if let Some(i) = ind {
        let smas: Vec<String> = [("SMA20", i.sma20), ("SMA50", i.sma50), ("SMA200", i.sma200)]
            .into_iter()
            .filter_map(|(n, s)| positive(s).and_then(|s| rel(price, s)).map(|d| format!("{n} {}", signed(d))))
            .collect();
        if !smas.is_empty() {
            add(TECH, true, lang.t("Precio frente a las medias móviles", "Price versus moving averages").into(), join(smas), lang.t("(precio − media)/media", "(price − average)/average").into());
        }
        if let Some(a) = positive(i.atr14) {
            add(TECH, false, "ATR14".into(), fmt::pct(a / price), lang.t("ATR14 / precio", "ATR14 / price").into());
        }
        if let Some(v) = positive(i.volatility30d) {
            add(TECH, false, lang.t("Volatilidad 30d (anualizada)", "Volatility 30d (annualised)").into(), fmt::pct(v), lang.t("σ diaria × √252", "daily σ × √252").into());
        }
        if let Some(r) = positive(i.relative_volume) {
            add(TECH, false, lang.t("Volumen relativo", "Relative volume").into(), format!("{}x", fmt::num(r)), lang.t("vs media de 20 sesiones", "vs 20-session average").into());
        }
    }

    // --- Valoración relativa ---
    if let Some(f) = fun {
        let same = same_currency(snap, f);
        if same
            && let (Some(t), Some(fw)) = (positive(f.pe_trailing), positive(f.pe_forward))
        {
            add(
                VALUATION,
                true,
                lang.t("Crecimiento de beneficios implícito en el P/E", "Earnings growth implied by P/E").into(),
                signed(t / fw - 1.0),
                lang.t("P/E trailing/forward − 1", "P/E trailing/forward − 1").into(),
            );
        }
        let sector = f.sector.as_deref().unwrap_or("").to_lowercase();
        let financial = ["financ", "bank", "insur"].iter().any(|n| sector.contains(n));
        let non_corporate = matches!(snap.summary.class, AssetClass::Etf | AssetClass::Fund | AssetClass::Index | AssetClass::Crypto | AssetClass::Bond);
        if same
            && !financial
            && !non_corporate
            && let (Some(fcf), Some(m)) = (good(f.free_cash_flow), positive(f.market_cap).or(positive(snap.quote.as_ref().and_then(|q| q.market_cap))))
        {
            add(VALUATION, true, lang.t("Rentabilidad por FCF", "FCF yield").into(), signed(fcf / m), lang.t("FCF / capitalización", "FCF / market cap").into());
        }
    }
    rows
}

/// Sección «Cifras derivadas» lista para el contexto. `groups`: qué grupos; `compact`: solo las
/// imprescindibles (presupuesto reducido); `skip_shown`: omite las que el propio contexto ya muestra
/// (ATR en % del precio, volatilidad 30d, volumen relativo cuando la tabla de indicadores está).
/// Vacío si no queda ninguna fila.
pub fn render(snap: &AssetSnapshot, lang: Lang, groups: u8, compact: bool, skip_shown: bool) -> String {
    let rows: Vec<Row> = compute(snap, lang)
        .into_iter()
        .filter(|r| r.group & groups != 0 && (!compact || r.core) && !(skip_shown && r.group == TECH && !r.core))
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    s.push_str(&format!("## {}\n", lang.t("Cifras derivadas (calculadas, no las recalcules)", "Derived figures (computed, do not recompute)")));
    s.push_str(lang.t(
        "Úsalas tal cual; no calcules otras. Distancias a máximos/mínimos/medias: descriptivas, NO previsión.\n",
        "Use as given; compute no others. Distances to highs, lows, averages: descriptive, NOT a forecast.\n",
    ));
    s.push_str(&format!("| {} | {} | {} |\n|---|---|---|\n", lang.t("Cifra", "Figure"), lang.t("Valor", "Value"), lang.t("Base", "Basis")));
    for r in rows {
        s.push_str(&format!("| {} | {} | {} |\n", r.label, r.value, r.basis));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    fn get<'a>(rows: &'a [Row], needle: &str) -> Option<&'a Row> {
        rows.iter().find(|r| r.label.contains(needle))
    }

    #[test]
    fn upside_to_analyst_targets_is_signed_and_based_on_price() {
        let mut snap = fixture::stock_snapshot();
        let price = snap.quote.as_ref().unwrap().price;
        let f = snap.fundamentals.as_mut().unwrap();
        f.analyst_target_mean = Some(price * 1.10);
        f.analyst_target_high = Some(price * 1.30);
        f.analyst_target_low = Some(price * 0.90);
        let rows = compute(&snap, Lang::Es);
        let r = get(&rows, "Potencial hasta el objetivo").unwrap();
        assert_eq!(r.value, "medio +10.0%, máximo +30.0%, mínimo -10.0%");
        assert_eq!(r.basis, "(objetivo − precio)/precio; negativo = objetivo bajo el precio");
    }

    #[test]
    fn a_target_below_the_price_is_a_negative_upside() {
        let mut snap = fixture::stock_snapshot();
        let price = snap.quote.as_ref().unwrap().price;
        snap.fundamentals.as_mut().unwrap().analyst_target_mean = Some(price * 0.968);
        let rows = compute(&snap, Lang::Es);
        let r = get(&rows, "Potencial hasta el objetivo").unwrap();
        assert!(r.value.starts_with("medio -3.2%"), "{r:?}");
        assert!(r.basis.starts_with("(objetivo − precio)/precio"));
    }

    #[test]
    fn no_analysts_or_different_currency_means_no_upside_rows() {
        let mut snap = fixture::stock_snapshot();
        {
            let f = snap.fundamentals.as_mut().unwrap();
            f.analyst_target_mean = None;
            f.analyst_target_high = None;
            f.analyst_target_low = None;
        }
        assert!(get(&compute(&snap, Lang::Es), "Potencial").is_none());
        let mut snap = fixture::stock_snapshot();
        snap.fundamentals.as_mut().unwrap().analyst_count = Some(0);
        assert!(get(&compute(&snap, Lang::Es), "Potencial").is_none());
        let mut snap = fixture::stock_snapshot();
        snap.fundamentals.as_mut().unwrap().currency = Some("EUR".into());
        assert!(get(&compute(&snap, Lang::Es), "Potencial").is_none(), "objetivo en otra moneda que el precio");
    }

    #[test]
    fn level_distances_and_reward_risk_match_the_plan_shown_in_the_report() {
        let snap = fixture::stock_snapshot();
        let rows = compute(&snap, Lang::Es);
        let plan = levels::plan(&snap, Verdict::Accumulate, Lang::Es).unwrap();
        let price = snap.quote.as_ref().unwrap().price;
        let stop = plan.stop_loss.expect("el fixture tiene stop");
        assert_eq!(get(&rows, "al stop").unwrap().value, signed((stop - price) / price));
        let rr = get(&rows, "Recompensa/riesgo").expect("hay recompensa/riesgo");
        assert_eq!(rr.value, format!("{:.1}", plan.reward_risk.unwrap()));
        assert!(get(&rows, "soportes").unwrap().value.starts_with('-'));
        assert!(get(&rows, "resistencias").unwrap().value.starts_with('+'));
        assert!(get(&rows, "zona de entrada").is_some());
    }

    #[test]
    fn without_levels_there_are_no_level_rows() {
        let mut snap = fixture::stock_snapshot();
        snap.indicators = None;
        let rows = compute(&snap, Lang::Es);
        for n in ["soportes", "resistencias", "zona de entrada", "al stop", "Recompensa", "52 semanas", "medias móviles", "ATR14"] {
            assert!(get(&rows, n).is_none(), "{n} no debe aparecer sin indicadores");
        }
        // Sin precio no hay nada.
        let mut snap = fixture::stock_snapshot();
        snap.quote = None;
        snap.indicators.as_mut().unwrap().last_close = None;
        assert!(compute(&snap, Lang::Es).is_empty());
    }

    #[test]
    fn range_and_average_distances_are_signed_descriptions() {
        let mut snap = fixture::stock_snapshot();
        let price = snap.quote.as_ref().unwrap().price;
        {
            let i = snap.indicators.as_mut().unwrap();
            i.high52w = Some(price * 1.25);
            i.low52w = Some(price * 0.8);
            i.sma50 = Some(price * 1.02);
            i.sma20 = Some(price * 0.98);
            i.sma200 = None;
            i.atr14 = Some(price * 0.026);
            i.volatility30d = Some(0.308);
            i.relative_volume = Some(1.3);
        }
        let rows = compute(&snap, Lang::Es);
        assert_eq!(get(&rows, "máximo de 52").unwrap().value, "-20.0%");
        assert_eq!(get(&rows, "mínimo de 52").unwrap().value, "+25.0%");
        let ma = get(&rows, "medias móviles").unwrap();
        assert_eq!(ma.value, "SMA20 +2.0%, SMA50 -2.0%");
        assert_eq!(get(&rows, "ATR14").unwrap().value, "2.6%");
        assert_eq!(get(&rows, "Volatilidad").unwrap().value, "30.8%");
        assert_eq!(get(&rows, "Volumen relativo").unwrap().value, "1.3x");
    }

    #[test]
    fn relative_valuation_needs_both_pes_and_matching_currencies() {
        let mut snap = fixture::stock_snapshot();
        {
            let f = snap.fundamentals.as_mut().unwrap();
            f.pe_trailing = Some(34.8);
            f.pe_forward = Some(29.0);
            f.free_cash_flow = Some(100e9);
            f.market_cap = Some(2.5e12);
        }
        let rows = compute(&snap, Lang::Es);
        assert_eq!(get(&rows, "beneficios implícito").unwrap().value, "+20.0%");
        assert_eq!(get(&rows, "FCF").unwrap().value, "+4.0%");
        // Forward ausente o negativo: sin cifra.
        snap.fundamentals.as_mut().unwrap().pe_forward = None;
        assert!(get(&compute(&snap, Lang::Es), "implícito").is_none());
        snap.fundamentals.as_mut().unwrap().pe_forward = Some(-5.0);
        assert!(get(&compute(&snap, Lang::Es), "implícito").is_none());
        // ADR: cuentas en TWD y capitalización en USD (R-05): ni FCF ni comparación de múltiplos.
        let mut snap = fixture::stock_snapshot();
        snap.fundamentals.as_mut().unwrap().extra.insert("financialCurrency".into(), serde_json::json!("TWD"));
        let rows = compute(&snap, Lang::Es);
        assert!(get(&rows, "FCF").is_none() && get(&rows, "implícito").is_none(), "{rows:?}");
        // Banco: el FCF no significa nada.
        let mut snap = fixture::stock_snapshot();
        snap.fundamentals.as_mut().unwrap().sector = Some("Financial Services".into());
        assert!(get(&compute(&snap, Lang::Es), "FCF").is_none());
    }

    #[test]
    fn crypto_shows_ath_distance_and_supply_share_only_with_data() {
        let snap = fixture::crypto_snapshot();
        let price = snap.quote.as_ref().unwrap().price;
        let rows = compute(&snap, Lang::Es);
        assert_eq!(get(&rows, "máximo histórico").unwrap().value, signed((price - 109_000.0) / 109_000.0));
        assert_eq!(get(&rows, "Oferta circulante").unwrap().value, fmt::pct(19.8e6 / 21e6));
        assert!(get(&rows, "FCF").is_none() && get(&rows, "implícito").is_none());
        let mut snap = fixture::crypto_snapshot();
        {
            let f = snap.fundamentals.as_mut().unwrap();
            f.all_time_high = None;
            f.max_supply = None;
        }
        let rows = compute(&snap, Lang::Es);
        assert!(get(&rows, "máximo histórico").is_none() && get(&rows, "Oferta circulante").is_none());
        // Oferta circulante > máxima: dato incoherente, no se calcula.
        let mut snap = fixture::crypto_snapshot();
        snap.fundamentals.as_mut().unwrap().circulating_supply = Some(30e6);
        assert!(get(&compute(&snap, Lang::Es), "Oferta circulante").is_none());
    }

    #[test]
    fn render_groups_compacts_and_omits_empty_sections() {
        let snap = fixture::stock_snapshot();
        let full = render(&snap, Lang::Es, ALL, false, false);
        assert!(full.starts_with("## Cifras derivadas (calculadas, no las recalcules)\n"), "{full}");
        assert!(full.contains("NO previsión") && full.contains("| Cifra | Valor | Base |"));
        let compact = render(&snap, Lang::Es, ALL, true, false);
        assert!(compact.len() < full.len());
        assert!(compact.contains("Potencial hasta el objetivo") && !compact.contains("Distancia a los soportes"));
        let shown = render(&snap, Lang::Es, TECH, false, true);
        assert!(shown.contains("medias móviles") && !shown.contains("ATR14") && !shown.contains("Volatilidad"), "{shown}");
        assert_eq!(render(&snap, Lang::Es, 0, false, false), "");
        let mut empty = fixture::stock_snapshot();
        empty.quote = None;
        empty.indicators = None;
        assert_eq!(render(&empty, Lang::Es, ALL, false, false), "");
        assert!(render(&snap, Lang::En, ALL, false, false).contains("Upside to the analyst target"));
    }

    /// Presupuesto: la sección es pequeña en cada etapa y el contexto completo apenas crece.
    #[test]
    fn the_section_adds_few_tokens_per_stage() {
        use crate::{
            context::{ContextOptions, NewsRegistry, Sections, build_context_sections},
            scoring,
            tokens::estimate_tokens,
        };
        let opts = ContextOptions { max_tokens: 8000, lang: Lang::Es };
        for snap in [fixture::stock_snapshot(), fixture::crypto_snapshot()] {
            let card = scoring::score(&snap, Lang::Es);
            for (name, sec, max) in [
                ("all", Sections::ALL, 330),
                ("judge", Sections::judge(), 210),
                ("technical", Sections::technical(), 230),
                ("fundamental", Sections::fundamental(), 200),
                ("macro_risk", Sections::macro_risk(), 140),
            ] {
                let with = build_context_sections(&snap, &card, &opts, &mut NewsRegistry::new(), &sec).text;
                let without = build_context_sections(&snap, &card, &opts, &mut NewsRegistry::new(), &Sections { derived: 0, ..sec }).text;
                let added = estimate_tokens(&with) - estimate_tokens(&without);
                eprintln!("{} {name}: +{added} tokens (de {})", snap.summary.id, estimate_tokens(&without));
                assert!(added <= max, "{name} añade {added} tokens (máx. {max})");
            }
        }
    }
}
