//! Perfil de ETF en el análisis: sección del contexto, cifras derivadas, etiquetas del verificador,
//! magnitudes del ejemplo de `quick` y la garantía de que la puntuación no cambia. Los perfiles de
//! `tests/fixtures/fund_profile_*.json` son la salida real del parser de Yahoo (19-oct-2026).

use domain::{AssetClass, AssetSnapshot, FundProfile};

use crate::{
    context::{ContextOptions, NewsRegistry, Sections, build_context, build_context_sections},
    derived,
    fidelity::NumberIndex,
    fixture,
    lang::Lang,
    scoring,
};

fn profile(name: &str) -> FundProfile {
    let p = format!("{}/tests/fixtures/fund_profile_{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn etf(name: &str, with_profile: bool) -> AssetSnapshot {
    let mut s = fixture::stock_snapshot();
    s.summary.class = AssetClass::Etf;
    s.summary.name = name.to_uppercase();
    let f = s.fundamentals.as_mut().unwrap();
    // Un ETF no tiene datos de empresa.
    f.pe_trailing = None;
    f.pe_forward = None;
    f.peg = None;
    f.revenue_growth_yoy = None;
    f.operating_margin = None;
    f.free_cash_flow = None;
    f.eps_trailing = None;
    f.eps_forward = None;
    f.sector = None;
    f.industry = None;
    let fp = profile(name);
    f.expense_ratio = fp.expense_ratio;
    f.total_assets = fp.total_assets;
    f.top_holdings = fp.top_holdings.clone();
    f.fund_profile = with_profile.then_some(fp);
    s
}

fn ctx(snap: &AssetSnapshot, lang: Lang, max_tokens: usize) -> String {
    let card = scoring::score(snap, lang);
    build_context(snap, &card, &ContextOptions { max_tokens, lang }, &mut NewsRegistry::new()).text
}

#[test]
fn spy_context_has_the_fund_profile_section_with_source_and_date() {
    let c = ctx(&etf("spy", true), Lang::Es, 8000);
    assert!(c.contains("## Perfil del fondo [fuente: yahoo · 2026-10-19 14:15 UTC]"), "{c}");
    assert!(c.contains("State Street Investment Management") && c.contains("categoría Large Blend") && c.contains("inicio 1993-01-22"), "{c}");
    assert!(c.contains("expense ratio 0.09%") && c.contains("turnover 3.0%"), "{c}");
    assert!(c.contains("total assets 809B") && c.contains("dividend yield 0.99%") && c.contains("beta 3Y 1"), "{c}");
    assert!(c.contains("peso de las 10 primeras posiciones 39."), "{c}");
    assert!(c.contains("NVDA 8.4%") && c.contains("technology 40."), "{c}");
    assert!(c.contains("Valoración de la cartera | P/E 24.2, P/B 5.25, P/S 3.64"), "{c}");
    // Las posiciones se muestran una sola vez (en el perfil, no también en la tabla de fundamentales).
    assert_eq!(c.matches("NVDA 8.4%, AAPL 7.4%").count(), 1, "{c}");
    // Lo que Yahoo no dio no aparece.
    assert!(!c.contains("Renta fija de la cartera") && !c.contains("gross expense") && !c.contains("earnings growth 3Y"), "{c}");
}

#[test]
fn english_context_and_no_profile_means_no_section() {
    let c = ctx(&etf("spy", true), Lang::En, 8000);
    assert!(c.contains("## Fund profile [source: yahoo") && c.contains("weight of the top 10 holdings") && c.contains("Portfolio valuation | P/E 24.2"), "{c}");
    let none = ctx(&etf("spy", false), Lang::Es, 8000);
    assert!(!none.contains("Perfil del fondo") && none.contains("Mayores posiciones | NVDA 8.4%"), "{none}");
    // Una acción nunca recibe la sección, aunque llevara un perfil.
    let mut stock = etf("spy", true);
    stock.summary.class = AssetClass::Stock;
    assert!(!ctx(&stock, Lang::Es, 8000).contains("Perfil del fondo"));
}

#[test]
fn bond_etf_shows_ratings_and_mix_but_no_equity_rows() {
    let c = ctx(&etf("tlt", true), Lang::Es, 8000);
    assert!(c.contains("Perfil del fondo") && c.contains("categoría Long Government") && c.contains("expense ratio 0.15%"), "{c}");
    assert!(c.contains("Tipo de activo | bond 99.6%, cash 0.4%") && c.contains("aa 100.0%, us government 99.6%"), "{c}");
    assert!(!c.contains("Mayores posiciones") && !c.contains("Concentración") && !c.contains("Valoración de la cartera") && !c.contains("Sectores"), "{c}");
}

#[test]
fn the_section_shrinks_with_the_token_budget_but_keeps_costs_and_concentration() {
    let big = ctx(&etf("spy", true), Lang::Es, 8000);
    let small = ctx(&etf("spy", true), Lang::Es, 1900);
    assert!(small.len() < big.len());
    if small.contains("Perfil del fondo") {
        assert!(small.contains("expense ratio 0.09%"), "{small}");
    }
}

#[test]
fn derived_fund_rows_are_computed_in_rust_and_only_with_data() {
    let rows = derived::compute(&etf("spy", true), Lang::Es);
    let get = |needle: &str| rows.iter().find(|r| r.label.contains(needle)).unwrap_or_else(|| panic!("falta {needle}: {rows:?}"));
    let top10: f64 = profile("spy").top_holdings.iter().map(|h| h.weight).sum();
    assert_eq!(get("Concentración (peso de las 10 primeras").value, format!("{:.1}%", top10 * 100.0));
    assert_eq!(get("mayor posición").value, "8.4%");
    assert!(get("primer sector").value.starts_with("technology 40."));
    assert_eq!(get("por cada 10 000").value, "9.45");
    // Sin perfil o sin posiciones: no se inventa ninguna fila.
    assert!(derived::compute(&etf("spy", false), Lang::Es).iter().all(|r| r.group != derived::FUND));
    let tlt = derived::compute(&etf("tlt", true), Lang::Es);
    assert!(tlt.iter().all(|r| !r.label.contains("Concentración") && !r.label.contains("sector")), "{tlt:?}");
    assert!(tlt.iter().any(|r| r.label.contains("por cada 10 000") && r.value == "15"), "{tlt:?}");
    // El juez (cotización + derivadas) las recibe aunque no lleve fundamentales.
    let snap = etf("spy", true);
    let card = scoring::score(&snap, Lang::Es);
    let j = build_context_sections(&snap, &card, &ContextOptions { max_tokens: 4000, lang: Lang::Es }, &mut NewsRegistry::new(), &Sections::judge()).text;
    assert!(j.contains("Concentración (peso de las 10 primeras posiciones)") && !j.contains("## Perfil del fondo"), "{j}");
}

/// B2: el saneado del perfil (Yahoo -> prompt) no toca un perfil normal: los datos y el contexto
/// renderizado son idénticos con y sin `sanitize_snapshot`, en los dos idiomas y a varios presupuestos.
#[test]
fn sanitising_leaves_a_normal_fund_profile_and_its_context_unchanged() {
    for name in ["spy", "qqq", "tlt"] {
        for with_profile in [true, false] {
            let raw = etf(name, with_profile);
            let clean = crate::safety::sanitize_snapshot(&raw);
            let (r, c) = (raw.fundamentals.as_ref().unwrap(), clean.fundamentals.as_ref().unwrap());
            assert_eq!(r.fund_profile, c.fund_profile, "{name}");
            assert_eq!(r.top_holdings, c.top_holdings, "{name}");
            for (lang, budget) in [(Lang::Es, 8000), (Lang::En, 8000), (Lang::Es, 1500)] {
                assert_eq!(ctx(&raw, lang, budget), ctx(&clean, lang, budget), "{name} profile={with_profile} {lang:?} {budget}");
            }
        }
    }
}

#[test]
fn the_score_does_not_change_when_the_fund_profile_is_added() {
    for name in ["spy", "tlt"] {
        let (a, b) = (scoring::score(&etf(name, false), Lang::Es), scoring::score(&etf(name, true), Lang::Es));
        assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap(), "{name}");
    }
}

#[test]
fn verifier_knows_expense_ratio_ter_net_assets_weight_and_concentration() {
    let snap = etf("spy", true);
    let idx = {
        let mut i = NumberIndex::from_context(&ctx(&snap, Lang::Es, 8000));
        // Las cifras derivadas viajan en su propia sección del contexto.
        i.add_text(&derived::render(&snap, Lang::Es, derived::ALL, false, false), crate::fidelity::Segment::Derived);
        i
    };
    let bad = |t: &str| idx.unverified(t).into_iter().map(|f| f.raw).collect::<Vec<_>>();
    let none: Vec<String> = vec![];
    let top10: f64 = profile("spy").top_holdings.iter().map(|h| h.weight).sum();
    let conc = format!("{:.1}", top10 * 100.0);
    assert_eq!(bad("El ratio de gastos es 0.09% y el TER anual de 0.09% es bajo."), none);
    assert_eq!(bad("El patrimonio del fondo asciende a 809B."), none);
    assert_eq!(bad(&format!("La concentración de las 10 primeras posiciones es del {conc}%.")), none);
    assert_eq!(bad("El peso de NVDA es 8.4% y el del sector tecnológico 40.5%."), none);
    assert_eq!(bad("The expense ratio is 0.09% and net assets are 809B; top holdings weight is 8.4%."), none);
    assert_eq!(bad("Cada 10 000 invertidos cuestan 9.45 al año."), none);
    // Etiquetas equivocadas: 24.2 es el PER de la cartera, no el patrimonio ni el ratio de gastos.
    assert_eq!(bad("El ratio de gastos es 24.2%."), ["24.2"]);
    assert_eq!(bad("El patrimonio es de 24.2."), ["24.2"]);
    assert_eq!(bad("La concentración es 5.25."), ["5.25"]);
    // Y una cifra que no está en ningún sitio sigue sin pasar.
    assert_eq!(bad("El ratio de gastos es 0.35%."), ["0.35"]);
}
