//! Perfil de riesgo de OPV sobre extractos REALES de folletos de EDGAR (424B4 de 2025-2026:
//! TRXB, LYNX, OIG, CRWV, SNSC) y sobre los fixtures HTML recortados del módulo de folletos.
//! Sin red. Los extractos conservan íntegras las secciones que usa el perfil (recortadas a un
//! máximo de caracteres) y declaran en `missing` las que se quitaron.

use chrono::{DateTime, NaiveDate, Utc};
use domain::{IpoEntry, IpoIndicatorStatus, IpoRiskIndicator, IpoRiskProfile, IpoStatus, Prospectus};
use market::{
    ipo_risk::{ProfileInput, build_profile, compute_base_rate},
    prospectus::extract,
};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-16T00:00:00Z").unwrap().with_timezone(&Utc)
}

fn load(json: &str) -> Prospectus {
    serde_json::from_str(json).expect("fixture válido")
}

fn profile(p: &Prospectus, entry: Option<&IpoEntry>) -> IpoRiskProfile {
    build_profile(ProfileInput { symbol: "TEST", entry, prospectus: Some(p), base_rate: None, now: now() })
}

fn ind<'a>(pr: &'a IpoRiskProfile, key: &str) -> &'a IpoRiskIndicator {
    pr.indicators.iter().find(|i| i.key == key).unwrap_or_else(|| panic!("falta {key}"))
}

fn has_flag(i: &IpoRiskIndicator, code: &str) -> bool {
    i.flags.iter().any(|f| f.code == code)
}

fn approx(a: Option<f64>, b: f64) {
    let a = a.expect("hay valor numérico");
    assert!((a - b).abs() < 0.06, "{a} != {b}");
}

fn trxb() -> Prospectus {
    load(include_str!("fixtures/sec_prospectus_trxb_excerpt.json"))
}
fn lynx() -> Prospectus {
    load(include_str!("fixtures/sec_prospectus_lynx_excerpt.json"))
}
fn oig() -> Prospectus {
    load(include_str!("fixtures/sec_prospectus_oig_excerpt.json"))
}
fn crwv() -> Prospectus {
    load(include_str!("fixtures/sec_prospectus_crwv_excerpt.json"))
}
fn snsc() -> Prospectus {
    load(include_str!("fixtures/sec_prospectus_snsc_excerpt.json"))
}

#[test]
fn trxb_pure_primary_biotech() {
    let pr = profile(&trxb(), None);
    let s = ind(&pr, "secondary_sale");
    assert_eq!(s.status, IpoIndicatorStatus::Extracted);
    assert_eq!(s.numeric, Some(0.0));
    approx(ind(&pr, "float_offered").numeric, 30.36);
    let l = ind(&pr, "lock_up");
    assert_eq!(l.numeric, Some(180.0));
    assert!(l.evidence.as_deref().unwrap().contains("180 days after the date of this prospectus"));
    let u = ind(&pr, "underwriters");
    assert_eq!(u.numeric, Some(5.0));
    assert!(u.value.as_deref().unwrap().starts_with("J.P. Morgan Securities LLC (37 %)"), "{:?}", u.value);
    approx(ind(&pr, "dilution").numeric, 7.04);
    assert!(ind(&pr, "voting").flags.is_empty());
    let n = ind(&pr, "net_result");
    assert!(has_flag(n, "net_loss"), "{:?}", n.value);
    // «Collaboration revenue» no es una fila de ingresos reconocida: no se rellena.
    let r = ind(&pr, "revenue");
    assert_eq!(r.status, IpoIndicatorStatus::NotExtracted);
    assert!(r.value.is_none() && r.note.is_some());
    // Destino de los fondos: ni deuda ni vendedores.
    let u = ind(&pr, "use_of_proceeds");
    assert!(has_flag(u, "general_corporate") && has_flag(u, "rnd") && !has_flag(u, "debt_repayment") && !has_flag(u, "selling_holders"));
}

#[test]
fn lynx_secondary_sale_and_debt_repayment() {
    let pr = profile(&lynx(), None);
    let s = ind(&pr, "secondary_sale");
    approx(s.numeric, 66.4);
    assert!(has_flag(s, "secondary"));
    let u = ind(&pr, "use_of_proceeds");
    assert!(has_flag(u, "debt_repayment") && has_flag(u, "selling_holders"));
    let debt = u.flags.iter().find(|f| f.code == "debt_repayment").unwrap();
    assert!(debt.evidence.contains("repay approximately $60.0 million"), "{}", debt.evidence);
    // Lock-up escalonado: se listan los plazos, sin elegir uno.
    let l = ind(&pr, "lock_up");
    assert!(l.numeric.is_none());
    let v = l.value.as_deref().unwrap();
    assert!(v.contains("180 días") && v.contains("720 días"), "{v}");
    approx(ind(&pr, "float_offered").numeric, 14.66);
}

#[test]
fn oig_dual_class_and_mixed_results() {
    let pr = profile(&oig(), None);
    let v = ind(&pr, "voting");
    assert_eq!(v.numeric, Some(10.0));
    assert!(has_flag(v, "multi_vote"));
    assert!(has_flag(ind(&pr, "use_of_proceeds"), "debt_repayment"));
    // «Net income (loss) | 13,547 | (3,042) | 16,314 | (288)»: signos mezclados.
    let n = ind(&pr, "net_result");
    assert!(n.value.as_deref().unwrap().starts_with("Resultado mixto"), "{:?}", n.value);
    assert!(n.flags.is_empty());
    assert_eq!(ind(&pr, "revenue").status, IpoIndicatorStatus::Extracted);
    approx(ind(&pr, "dilution").numeric, 11.60);
}

#[test]
fn crwv_multiple_classes_total_row_and_many_underwriters() {
    let pr = profile(&crwv(), None);
    approx(ind(&pr, "secondary_sale").numeric, 2.4);
    // Usa la fila «Total Class A, Class B, and Class C...» (464 M), no suma clases a mano.
    approx(ind(&pr, "float_offered").numeric, 8.08);
    assert_eq!(ind(&pr, "voting").numeric, Some(10.0));
    let u = ind(&pr, "underwriters");
    assert_eq!(u.numeric, Some(18.0));
    assert!(u.value.as_deref().unwrap().ends_with("y 15 más"));
    approx(ind(&pr, "dilution").numeric, 36.74);
}

#[test]
fn snsc_small_ipo_with_two_lockups_and_odd_prospectus_price() {
    let p = snsc();
    // La portada de este folleto da 0,0001 (valor nominal) como «precio»: no se usa para la dilución.
    assert!(p.offer_price.is_some_and(|v| v < 0.5));
    let pr = profile(&p, None);
    let d = ind(&pr, "dilution");
    assert_eq!(d.numeric, Some(4.3801));
    assert!(!d.value.as_deref().unwrap().contains('%'), "{:?}", d.value);
    let l = ind(&pr, "lock_up");
    assert_eq!(l.value.as_deref(), Some("Varios plazos en el texto: 3 meses, 6 meses"));
    assert_eq!(ind(&pr, "underwriters").numeric, Some(2.0));
    // Sin la sección de resumen financiero: no se inventa nada.
    assert_eq!(ind(&pr, "net_result").status, IpoIndicatorStatus::NotExtracted);
    approx(ind(&pr, "float_offered").numeric, 8.14);
}

#[test]
fn calendar_price_gives_the_dilution_percentage_and_the_offer_size() {
    let entry = IpoEntry {
        date: NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
        symbol: Some("TRXB".into()),
        name: "TRex Bio, Inc.".into(),
        exchange: Some("NASDAQ Global Select".into()),
        status: IpoStatus::Priced,
        price_low: Some(14.0),
        price_high: Some(14.0),
        shares: Some(8_333_334),
        total_value: Some(116_666_676.0),
        spac_likely: false,
        source: "finnhub".into(),
        source_url: Some("https://finnhub.io/".into()),
        as_of: now(),
    };
    let pr = profile(&trxb(), Some(&entry));
    assert!(ind(&pr, "dilution").value.as_deref().unwrap().contains("50 % del precio"));
    let o = ind(&pr, "offer_size");
    assert_eq!(o.numeric, Some(116_666_676.0));
    assert_eq!(o.source.kind, "calendar");
    assert!(o.value.as_deref().unwrap().contains("116,7 M USD"), "{:?}", o.value);
    assert_eq!(pr.name.as_deref(), Some("TRex Bio, Inc."));
    // Solo avisa de la tasa base ausente (no hay folleto preliminar, SPAC ni OPV sin cotizar).
    assert_eq!(pr.warnings.len(), 1, "{:?}", pr.warnings);
}

#[test]
fn without_prospectus_every_prospectus_indicator_is_not_extracted() {
    let pr = build_profile(ProfileInput { symbol: "zzz", entry: None, prospectus: None, base_rate: None, now: now() });
    assert_eq!(pr.symbol, "ZZZ");
    assert_eq!(pr.indicators.len(), 11);
    for i in &pr.indicators {
        assert_eq!(i.status, IpoIndicatorStatus::NotExtracted, "{}", i.key);
        assert!(i.value.is_none() && i.numeric.is_none() && i.flags.is_empty() && i.note.is_some(), "{}", i.key);
    }
    assert!(pr.warnings.iter().any(|w| w.contains("No se pudo obtener el folleto")));
    assert!(pr.disclaimer.contains("no una recomendación"));
}

#[test]
fn sections_with_unrelated_text_give_not_extracted_not_guesses() {
    let mut p = trxb();
    for s in &mut p.sections {
        s.text = "Lorem ipsum dolor sit amet. No hay cifras aquí.\nOtra línea | sin números".to_string();
    }
    let pr = profile(&p, None);
    for key in ["secondary_sale", "float_offered", "lock_up", "underwriters", "dilution", "net_result", "revenue"] {
        assert_eq!(ind(&pr, key).status, IpoIndicatorStatus::NotExtracted, "{key}");
    }
    // Estos dos buscan palabras: sin coincidencias dicen «no se detectó» y avisan de que no prueba nada.
    assert!(ind(&pr, "voting").flags.is_empty() && ind(&pr, "controlled").flags.is_empty());
    assert!(ind(&pr, "voting").note.as_deref().unwrap().contains("no prueba"));
}

#[test]
fn preliminary_prospectus_and_spac_warn() {
    let mut p = trxb();
    p.preliminary = true;
    p.form = "S-1/A".into();
    let entry = IpoEntry {
        date: NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
        symbol: Some("AAAU".into()),
        name: "Alpha Acquisition Corp".into(),
        exchange: None,
        status: IpoStatus::Expected,
        price_low: Some(10.0),
        price_high: Some(10.0),
        shares: Some(10_000_000),
        total_value: Some(115_000_000.0),
        spac_likely: true,
        source: "finnhub".into(),
        source_url: None,
        as_of: now(),
    };
    let pr = profile(&p, Some(&entry));
    let w = pr.warnings.join(" | ");
    assert!(w.contains("preliminar") && w.contains("SPAC") && w.contains("aún no cotiza"), "{w}");
}

#[test]
fn html_fixture_end_to_end_through_the_extractor() {
    let html = include_str!("fixtures/sec_prospectus_dfin_424b4.html");
    let e = extract(html, 0);
    let p = Prospectus {
        issuer: "TRex Bio, Inc.".into(),
        cik: 1,
        ticker: Some("TRXB".into()),
        form: "424B4".into(),
        preliminary: false,
        filed_on: NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
        accession: "0000000000-26-000000".into(),
        url: "https://www.sec.gov/Archives/edgar/data/1/x.htm".into(),
        index_url: "https://www.sec.gov/Archives/edgar/data/1/".into(),
        offer_price: e.offer_price,
        sections: e.sections,
        missing: e.missing,
        document_chars: e.document_chars,
        source: "sec".into(),
        fetched_at: now(),
    };
    let pr = profile(&p, None);
    // El fixture HTML está recortado (primeras líneas de cada sección): lo que sí conserva se
    // lee y lo que no, queda como «no extraído».
    assert_eq!(ind(&pr, "use_of_proceeds").status, IpoIndicatorStatus::Extracted);
    assert!(pr.indicators.iter().all(|i| i.status == IpoIndicatorStatus::NotExtracted || i.value.is_some()));
    assert_eq!(pr.prospectus.as_ref().unwrap().form, "424B4");
}

#[test]
fn base_rate_uses_medians_and_reports_the_sample_sizes() {
    use domain::{IpoPerformance, IpoReturnPoint};
    let d = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    let mk = |sym: &str, d1: f64, r3: Option<f64>, x3: Option<f64>| IpoPerformance {
        symbol: sym.into(),
        ipo_date: d,
        offer_price: 10.0,
        offer_price_source: "calendar".into(),
        first_day_close: 10.0,
        first_day_return_pct: d1,
        first_day_high_pct: d1,
        points: vec![IpoReturnPoint {
            horizon: "3m".into(),
            target_date: d,
            price_date: r3.map(|_| d),
            price: None,
            return_pct: r3,
            since_first_close_pct: None,
            benchmark_return_pct: None,
            excess_pct: x3,
        }],
        latest_price: 10.0,
        latest_date: d,
        latest_return_pct: 0.0,
        max_drawdown_pct: 0.0,
        benchmark: "SPY".into(),
        source: "yahoo".into(),
        source_url: None,
        warnings: vec![],
        as_of: now(),
    };
    let perfs = vec![mk("A", 20.0, Some(10.0), Some(5.0)), mk("B", -10.0, Some(-30.0), Some(-35.0)), mk("C", 40.0, Some(50.0), Some(44.0)), mk("D", 4.0, None, None)];
    let b = compute_base_rate(&perfs, 12, "SPY", now());
    assert_eq!((b.sample, b.n_day1, b.n_3m), (4, 4, 3));
    assert_eq!(b.median_day1_pct, Some(12.0)); // (4 + 20) / 2
    assert_eq!(b.median_3m_pct, Some(10.0));
    assert_eq!(b.median_3m_excess_pct, Some(5.0));
    assert_eq!(b.positive_day1_pct, Some(75.0));
    assert!((b.beat_benchmark_3m_pct.unwrap() - 66.666).abs() < 0.01);
    let empty = compute_base_rate(&[], 12, "SPY", now());
    assert!(empty.median_day1_pct.is_none() && empty.median_3m_pct.is_none() && empty.sample == 0);
    assert!(!b.caveats.is_empty());
}

fn section(key: &str, text: &str) -> domain::ProspectusSection {
    domain::ProspectusSection { key: key.into(), title: key.into(), text: text.into(), parent: None, truncated: false, original_chars: text.chars().count() }
}

/// Texto abreviado del 424B1 real de CHWM (Chilwa Minerals, oct-2026): ADS, un solo colocador sin
/// fila «Total», plazo de lock-up en letra («six months») y segunda mención como «180th day».
#[test]
fn small_adr_ipo_single_underwriter_and_spelled_out_months() {
    let mut p = trxb();
    p.sections = vec![
        section(
            "offering",
            "Securities offered by us | 625,000 ADSs and Warrants\n\
             Ordinary shares outstanding immediately after this offering | 104,256,092 ordinary shares\n\
             Lock-up agreements | Our directors, executive officers, and its major shareholder have agreed with the Representative not to offer for sale, issue, sell, contract to sell, pledge or otherwise dispose of any of our ordinary shares for a period of six months from the date of this prospectus.",
        ),
        section(
            "underwriting",
            "We are offering the ADSs through Maxim Group LLC as the sole representative.\n\
             Underwriter | Number of ADSs | Number of Warrants\n\
             Maxim Group LLC | 625,000 | 625,000\n\
             The underwriting agreement provides that the underwriters must buy all of the ADSs.",
        ),
    ];
    let pr = profile(&p, None);
    // ADS frente a acciones ordinarias: unidades no comparables, no se calcula el porcentaje.
    assert_eq!(ind(&pr, "float_offered").status, IpoIndicatorStatus::NotExtracted);
    assert!(ind(&pr, "float_offered").note.as_deref().unwrap().contains("ADS"));
    let l = ind(&pr, "lock_up");
    assert_eq!(l.value.as_deref(), Some("6 meses"));
    assert!(l.numeric.is_none(), "no se convierte meses a días");
    // Sin «Total»: se acepta porque la suma (625.000) coincide con lo ofrecido.
    let u = ind(&pr, "underwriters");
    assert_eq!(u.status, IpoIndicatorStatus::Extracted, "{:?}", u.note);
    assert_eq!(u.numeric, Some(1.0));
    assert!(u.value.as_deref().unwrap().starts_with("Maxim Group LLC (100 %)"));
}

#[test]
fn rule_144_waiting_period_is_not_taken_as_a_lockup() {
    let mut p = trxb();
    p.sections = vec![section(
        "shares_eligible",
        "Lock-up agreements are described below. Holders of restricted shares are entitled to sell upon the expiration of the lock-up agreements, \
         within any three-month period beginning 90 days after the date of this prospectus, a number of shares that does not exceed the greater of 1% of the shares outstanding.",
    )];
    let pr = profile(&p, None);
    assert_eq!(ind(&pr, "lock_up").status, IpoIndicatorStatus::NotExtracted);
}

// ---- Tasa base por tramos de tamaño ------------------------------------------------------------

fn perf(sym: &str, d1: f64, r3: Option<f64>) -> domain::IpoPerformance {
    let d = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    domain::IpoPerformance {
        symbol: sym.into(),
        ipo_date: d,
        offer_price: 10.0,
        offer_price_source: "calendar".into(),
        first_day_close: 10.0,
        first_day_return_pct: d1,
        first_day_high_pct: d1,
        points: vec![domain::IpoReturnPoint {
            horizon: "3m".into(),
            target_date: d,
            price_date: r3.map(|_| d),
            price: None,
            return_pct: r3,
            since_first_close_pct: None,
            benchmark_return_pct: None,
            excess_pct: r3,
        }],
        latest_price: 10.0,
        latest_date: d,
        latest_return_pct: 0.0,
        max_drawdown_pct: 0.0,
        benchmark: "SPY".into(),
        source: "yahoo".into(),
        source_url: None,
        warnings: vec![],
        as_of: now(),
    }
}

fn sized(n: usize, size: f64, d1: f64, tag: &str) -> Vec<market::ipo_risk::SizedPerf> {
    (0..n).map(|k| market::ipo_risk::SizedPerf { size_usd: Some(size), perf: perf(&format!("{tag}{k}"), d1, Some(d1 / 2.0)) }).collect()
}

#[test]
fn tier_assignment_edges_and_offer_size() {
    use market::ipo_risk::{offer_size_usd, tier_index};
    assert_eq!(tier_index(1.0), 0);
    assert_eq!(tier_index(49_999_999.99), 0);
    assert_eq!(tier_index(50_000_000.0), 1, "el corte pertenece al tramo de arriba");
    assert_eq!(tier_index(249_999_999.0), 1);
    assert_eq!(tier_index(250_000_000.0), 2);
    assert_eq!(tier_index(5e9), 2);
    let mut e = IpoEntry {
        date: NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
        symbol: Some("X".into()),
        name: "X".into(),
        exchange: None,
        status: IpoStatus::Priced,
        price_low: Some(14.0),
        price_high: Some(14.0),
        shares: Some(1_000_000),
        total_value: Some(14_000_000.0),
        spac_likely: false,
        source: "finnhub".into(),
        source_url: None,
        as_of: now(),
    };
    assert_eq!(offer_size_usd(&e), Some(14_000_000.0));
    e.total_value = None;
    assert_eq!(offer_size_usd(&e), Some(14_000_000.0), "acciones × precio");
    e.status = IpoStatus::Expected;
    e.price_low = Some(12.0);
    e.price_high = Some(16.0);
    assert_eq!(offer_size_usd(&e), Some(14_000_000.0), "punto medio, sin el 15 % de sobreasignación");
    e.shares = None;
    assert_eq!(offer_size_usd(&e), None);
}

#[test]
fn tiers_with_enough_sample_stay_separate() {
    let mut items = sized(12, 20e6, -2.0, "S");
    items.extend(sized(11, 100e6, 5.0, "M"));
    items.extend(sized(10, 400e6, 9.0, "L"));
    let b = market::ipo_risk::compute_base_rate_sized(&items, 12, "SPY", now());
    assert_eq!(b.tiers.len(), 3);
    assert!(b.tier_notes.is_empty());
    assert_eq!(b.tiers[0].label, "< 50 M USD");
    assert_eq!(b.tiers[1].label, "50–250 M USD");
    assert_eq!(b.tiers[2].label, "≥ 250 M USD");
    assert_eq!((b.tiers[0].sample, b.tiers[1].sample, b.tiers[2].sample), (12, 11, 10));
    assert_eq!(b.tiers[2].median_day1_pct, Some(9.0));
    assert_eq!(b.tiers[2].max_usd, None);
    assert_eq!(b.sample, 33, "la global incluye todo");
}

#[test]
fn small_tiers_are_merged_with_the_adjacent_one_and_say_so() {
    // Micro con 4 (< 10): se fusiona con el tramo contiguo (50–250).
    let mut items = sized(4, 20e6, -3.0, "S");
    items.extend(sized(15, 100e6, 5.0, "M"));
    items.extend(sized(12, 400e6, 9.0, "L"));
    let b = market::ipo_risk::compute_base_rate_sized(&items, 12, "SPY", now());
    assert_eq!(b.tiers.len(), 2);
    assert_eq!(b.tiers[0].label, "< 250 M USD");
    assert_eq!(b.tiers[0].merged_from, vec!["< 50 M USD", "50–250 M USD"]);
    assert_eq!(b.tiers[0].sample, 19);
    assert_eq!(b.tiers[1].label, "≥ 250 M USD");
    assert!(b.tier_notes[0].contains("fusionado"), "{:?}", b.tier_notes);
    // El tramo grande con poca muestra se fusiona con el anterior (más pequeño en n entre vecinos).
    let mut items = sized(15, 20e6, -3.0, "S");
    items.extend(sized(15, 100e6, 5.0, "M"));
    items.extend(sized(3, 400e6, 9.0, "L"));
    let b = market::ipo_risk::compute_base_rate_sized(&items, 12, "SPY", now());
    assert_eq!(b.tiers.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), vec!["< 50 M USD", "≥ 50 M USD"]);
    // Todo pequeño: acaba en un único tramo «todos los tamaños».
    let items = sized(5, 20e6, 1.0, "S");
    let b = market::ipo_risk::compute_base_rate_sized(&items, 12, "SPY", now());
    assert_eq!(b.tiers.len(), 1);
    assert_eq!(b.tiers[0].label, "todos los tamaños");
    // Sin tamaños: sin tramos.
    let b = compute_base_rate(&[perf("A", 1.0, None)], 12, "SPY", now());
    assert!(b.tiers.is_empty());
}

#[test]
fn profile_uses_the_tier_of_its_own_offer_size() {
    let mut items = sized(12, 20e6, -2.0, "S");
    items.extend(sized(11, 100e6, 5.0, "M"));
    items.extend(sized(10, 400e6, 9.0, "L"));
    let base = market::ipo_risk::compute_base_rate_sized(&items, 12, "SPY", now());
    let mk = |total: f64| IpoEntry {
        date: NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
        symbol: Some("ETRA".into()),
        name: "Electra".into(),
        exchange: None,
        status: IpoStatus::Priced,
        price_low: Some(15.0),
        price_high: Some(15.0),
        shares: Some(23_333_334),
        total_value: Some(total),
        spac_likely: false,
        source: "finnhub".into(),
        source_url: None,
        as_of: now(),
    };
    let tier_of = |e: &IpoEntry| {
        build_profile(ProfileInput { symbol: "ETRA", entry: Some(e), prospectus: None, base_rate: Some(base.clone()), now: now() }).base_rate_tier.map(|t| t.label)
    };
    assert_eq!(tier_of(&mk(350_000_000.0)).as_deref(), Some("≥ 250 M USD"));
    assert_eq!(tier_of(&mk(249_000_000.0)).as_deref(), Some("50–250 M USD"));
    assert_eq!(tier_of(&mk(3_500_000.0)).as_deref(), Some("< 50 M USD"));
    // Sin tamaño: sin tramo y con aviso; la global sigue disponible.
    let mut e = mk(1.0);
    e.total_value = None;
    e.shares = None;
    let pr = build_profile(ProfileInput { symbol: "ETRA", entry: Some(&e), prospectus: None, base_rate: Some(base), now: now() });
    assert!(pr.base_rate_tier.is_none() && pr.base_rate.is_some());
    assert!(pr.warnings.iter().any(|w| w.contains("solo la tasa base global")));
}

#[test]
fn lockup_scan_does_not_panic_on_non_ascii_before_a_spelled_out_number() {
    // Regresión: comillas tipográficas justo antes de «six months» partían un carácter multibyte.
    let mut p = trxb();
    p.sections = vec![section(
        "lock_up",
        "Our officers have agreed with the underwriters, “six months from the date of this prospectus”, not to sell. Il y a «three months» aussi.",
    )];
    let pr = profile(&p, None);
    assert_eq!(ind(&pr, "lock_up").value.as_deref(), Some("6 meses"));
}

// ---- float_offered: clases repetidas ---------------------------------------------------------

#[test]
fn repeated_share_class_rows_count_once_in_the_float() {
    // Las filas con la misma etiqueta (p. ej. la tabla repetida en el resumen y en «The Offering»)
    // cuentan una sola vez: 1 000 000 de la clase A y 500 000 de la B, no 3 000 000.
    let mut p = trxb();
    p.sections = vec![section(
        "offering",
        "Class A common stock offered by us | 500,000 shares\n\
         Class A shares outstanding immediately after this offering | 1,000,000 shares\n\
         Class A shares outstanding immediately after this offering | 1,000,000 shares\n\
         Class B shares outstanding immediately after this offering | 500,000 shares\n\
         Class A shares outstanding immediately after this offering | 1,000,000 shares\n",
    )];
    let pr = profile(&p, None);
    let f = ind(&pr, "float_offered");
    assert_eq!(f.status, IpoIndicatorStatus::Extracted, "{:?}", f.note);
    approx(f.numeric, 500_000.0 / 1_500_000.0 * 100.0);
}

// ---- Caché de perfiles -----------------------------------------------------------------------

mod profile_cache {
    use std::{sync::Arc, time::Duration};

    use market::ipo_risk::ProfileCache;

    use super::*;

    fn build(cache: &ProfileCache, p: &Prospectus, entry: Option<&IpoEntry>, base: Option<domain::IpoBaseRate>) -> Arc<IpoRiskProfile> {
        cache.get_or_build(ProfileInput { symbol: "trxb", entry, prospectus: Some(p), base_rate: base, now: now() })
    }

    #[test]
    fn identical_inputs_are_built_once_and_served_from_the_cache() {
        let cache = ProfileCache::default();
        let p = trxb();
        let a = build(&cache, &p, None, None);
        let b = build(&cache, &p, None, None);
        assert_eq!(cache.builds(), 1);
        assert!(Arc::ptr_eq(&a, &b), "la segunda lectura es la guardada");
        assert_eq!(a.symbol, "TRXB");
        // El resultado es el mismo que el de construirlo directamente.
        let direct = build_profile(ProfileInput { symbol: "trxb", entry: None, prospectus: Some(&p), base_rate: None, now: now() });
        assert_eq!(serde_json::to_value(&*a).unwrap(), serde_json::to_value(&direct).unwrap());
    }

    #[test]
    fn downloading_the_same_prospectus_again_does_not_invalidate_the_profile() {
        let cache = ProfileCache::default();
        let p = trxb();
        let a = build(&cache, &p, None, None);
        let mut again = p.clone();
        again.fetched_at += chrono::Duration::hours(3);
        let b = build(&cache, &again, None, None);
        assert_eq!(cache.builds(), 1);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn any_change_in_the_inputs_builds_a_new_profile() {
        let cache = ProfileCache::default();
        let p = trxb();
        build(&cache, &p, None, None);
        // Otro texto en una sección del folleto (misma accession).
        let mut edited = p.clone();
        edited.sections[0].text.push_str(" Cambio mínimo.");
        build(&cache, &edited, None, None);
        // Otra entrada del calendario.
        let entry = IpoEntry {
            date: NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
            symbol: Some("TRXB".into()),
            name: "TRex Bio".into(),
            exchange: None,
            status: IpoStatus::Priced,
            price_low: Some(14.0),
            price_high: Some(14.0),
            shares: Some(8_333_334),
            total_value: Some(116_666_676.0),
            spac_likely: false,
            source: "finnhub".into(),
            source_url: None,
            as_of: now(),
        };
        build(&cache, &p, Some(&entry), None);
        let mut moved = entry.clone();
        moved.price_low = Some(15.0);
        moved.price_high = Some(15.0);
        build(&cache, &p, Some(&moved), None);
        // Con tasa base.
        let base = compute_base_rate(&[], 12, "SPY", now());
        build(&cache, &p, None, Some(base));
        assert_eq!(cache.builds(), 5);
        // Y todas siguen guardadas: repetir cualquiera no construye de nuevo.
        build(&cache, &p, Some(&entry), None);
        build(&cache, &edited, None, None);
        assert_eq!(cache.builds(), 5);
    }

    #[test]
    fn simultaneous_requests_share_one_build() {
        let cache = Arc::new(ProfileCache::default());
        let p = Arc::new(trxb());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (cache, p) = (cache.clone(), p.clone());
                std::thread::spawn(move || build(&cache, &p, None, None))
            })
            .collect();
        let profiles: Vec<_> = handles.into_iter().map(|h| h.join().expect("sin panic")).collect();
        assert_eq!(cache.builds(), 1);
        assert!(profiles.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
    }

    #[test]
    fn the_cache_is_bounded_in_bytes() {
        // Cada perfil pesa unos kilobytes: con 40 KB caben muy pocos de los 30 distintos.
        let cache = ProfileCache::with_limits(40 * 1024, Duration::from_secs(600));
        let p = trxb();
        for k in 0..30 {
            let mut other = p.clone();
            other.sections[0].text.push_str(&format!(" variante {k}"));
            build(&cache, &other, None, None);
        }
        assert_eq!(cache.builds(), 30);
        let (entries, bytes) = cache.stats();
        assert!(bytes <= 40 * 1024, "{bytes} bytes");
        assert!(entries < 30, "{entries} perfiles guardados");
    }

    #[test]
    fn entries_expire() {
        let cache = ProfileCache::with_limits(1 << 20, Duration::from_millis(100));
        let p = trxb();
        build(&cache, &p, None, None);
        std::thread::sleep(Duration::from_millis(400));
        build(&cache, &p, None, None);
        assert_eq!(cache.builds(), 2);
    }
}
