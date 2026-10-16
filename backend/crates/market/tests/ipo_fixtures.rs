//! Pruebas sin red con respuestas REALES de Finnhub, SEC EDGAR y Yahoo (14/15-oct-2026) para el
//! calendario de OPV, la extracción de folletos y el rendimiento posterior.

use chrono::{DateTime, NaiveDate, Utc};
use domain::IpoStatus;
use market::{
    html::html_to_lines,
    ipo::{PerfInput, compute},
    prospectus::extract,
    providers::{
        finnhub::{IPO_PAGE_CAP, parse_ipo_calendar},
        sec::{ProspectusPick, parse_filing_rows, parse_fts, pick_prospectus},
        yahoo::parse_daily_series,
    },
};
use serde_json::Value;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-15T00:00:00Z").unwrap().with_timezone(&Utc)
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

// ---- Finnhub: calendario ---------------------------------------------------------------------

#[test]
fn finnhub_calendar_priced_and_filed_entries() {
    let v = json(include_str!("fixtures/finnhub_ipo_calendar_sep_oct_2026.json"));
    let rows = parse_ipo_calendar(&v, now());
    assert_eq!(rows.len(), v["ipoCalendar"].as_array().unwrap().len(), "no se pierde ninguna fila");
    let trxb = rows.iter().find(|e| e.symbol.as_deref() == Some("TRXB")).unwrap();
    assert_eq!(trxb.status, IpoStatus::Priced);
    assert_eq!(trxb.date, d(2026, 10, 9));
    assert_eq!((trxb.price_low, trxb.price_high), (Some(14.0), Some(14.0)));
    assert_eq!(trxb.shares, Some(8_333_334));
    assert_eq!(trxb.total_value, Some(116_666_676.0));
    assert_eq!(trxb.exchange.as_deref(), Some("NASDAQ Global Select"));
    assert!(!trxb.spac_likely);
    // Presentaciones sin precio ni acciones: todo a None, no a 0.
    let filed = rows.iter().find(|e| e.name.starts_with("Oura")).unwrap();
    assert_eq!(filed.status, IpoStatus::Filed);
    assert!(filed.price_low.is_none() && filed.shares.is_none());
    // Símbolo vacío o nulo -> None.
    assert!(rows.iter().filter(|e| e.status == IpoStatus::Withdrawn).all(|e| e.symbol.is_none()));
    // SPAC reconocida por nombre y precio de 10 USD.
    let spac = rows.iter().find(|e| e.name.starts_with("Haymaker")).unwrap();
    assert!(spac.spac_likely);
    // Turbogen: «priced» sin precio (OPV sin precio en el calendario).
    let tb = rows.iter().find(|e| e.symbol.as_deref() == Some("TRBG")).unwrap();
    assert_eq!(tb.status, IpoStatus::Priced);
    assert!(tb.price_low.is_none() && tb.total_value.is_none());
}

#[test]
fn finnhub_calendar_expected_has_price_range() {
    let rows = parse_ipo_calendar(&json(include_str!("fixtures/finnhub_ipo_calendar_expected.json")), now());
    assert!(rows.len() >= 2 && rows.iter().all(|e| e.status == IpoStatus::Expected));
    let cty = rows.iter().find(|e| e.symbol.as_deref() == Some("CTY")).unwrap();
    assert_eq!(cty.status, IpoStatus::Expected);
    assert_eq!((cty.price_low, cty.price_high), (Some(17.0), Some(19.0)));
    // Importe de `expected` = límite alto x acciones x 1,15 (medido en los tres casos del calendario).
    let max = 19.0 * cty.shares.unwrap() as f64 * 1.15;
    assert!((cty.total_value.unwrap() - max).abs() < 10.0, "{:?} vs {max}", cty.total_value);
}

#[test]
fn finnhub_calendar_wide_windows_hit_the_page_cap() {
    // Una ventana de un año devuelve exactamente el tope: por eso `ipo_calendar` parte la ventana.
    let v = json(include_str!("fixtures/finnhub_ipo_calendar_capped_200.json"));
    assert_eq!(parse_ipo_calendar(&v, now()).len(), IPO_PAGE_CAP);
}

// ---- SEC: elección del folleto ---------------------------------------------------------------

#[test]
fn picks_the_final_prospectus_of_a_fresh_ipo() {
    let v = json(include_str!("fixtures/sec_submissions_etra.json"));
    let rows = parse_filing_rows(&v["filings"]["recent"]);
    let (r, prelim) = pick_prospectus(&rows, ProspectusPick::Initial).unwrap();
    assert_eq!((r.form.as_str(), r.filed, prelim), ("424B4", d(2026, 9, 18), false));
    assert_eq!(r.document, "d61940d424b4.htm");
    // Sin el 424B4 se cae al último borrador y se marca como preliminar.
    let no_final: Vec<_> = rows.iter().filter(|r| r.form != "424B4").cloned().collect();
    let (r, prelim) = pick_prospectus(&no_final, ProspectusPick::Initial).unwrap();
    assert_eq!((r.form.as_str(), r.filed, prelim), ("S-1/A", d(2026, 9, 14), true));
}

#[test]
fn old_ipo_prospectus_lives_in_the_historic_file_not_in_recent() {
    let main = json(include_str!("fixtures/sec_submissions_crwv.json"));
    let recent = parse_filing_rows(&main["filings"]["recent"]);
    assert!(pick_prospectus(&recent, ProspectusPick::Initial).is_none_or(|(_, p)| p) || recent.iter().all(|r| r.form != "424B4"));
    assert!(main["filings"]["files"][0]["name"].as_str().unwrap().contains("submissions-001"));
    let mut all = recent;
    all.extend(parse_filing_rows(&json(include_str!("fixtures/sec_submissions_crwv_files001.json"))));
    let (r, prelim) = pick_prospectus(&all, ProspectusPick::Initial).unwrap();
    assert_eq!((r.form.as_str(), r.filed, prelim), ("424B4", d(2025, 3, 31), false));
    assert_eq!(r.accession, "0001193125-25-067651");
}

#[test]
fn fts_hits_give_cik_name_and_accession() {
    let hits = parse_fts(include_str!("fixtures/sec_fts_trex_424b4.json")).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].cik, hits[0].name.as_str(), hits[0].form.as_str()), (2_139_097, "TRex Bio, Inc.", "424B4"));
    assert_eq!(hits[0].accession, "0001193125-26-419203");
    let window = parse_fts(include_str!("fixtures/sec_fts_424b4_window.json")).unwrap();
    assert!(window.len() >= 6 && window.iter().all(|h| h.filed >= d(2026, 10, 8)));
    assert!(parse_fts("{}").is_err());
}

// ---- Folleto: HTML real y fixtures recortados ------------------------------------------------

#[test]
fn real_html_head_cover_and_toc() {
    let lines = html_to_lines(include_str!("fixtures/sec_prospectus_dfin_424b4_head.html"));
    let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(text.contains(&"Filed pursuant to Rule 424(b)(4)"));
    assert!(text.iter().any(|t| t.contains("initial public offering price is $14.00 per share")));
    assert!(text.contains(&"RISK FACTORS | 11"), "el índice sale como filas «título | página»");
    assert!(!text.iter().any(|t| t.contains("<") || t.contains("&#")), "sin restos de HTML");
    // Cabecera SGML («<TYPE>424B4…») descartada.
    assert!(!text.iter().take(3).any(|t| t.contains("project_dino")));
}

fn section<'a>(e: &'a market::prospectus::Extracted, key: &str) -> &'a domain::ProspectusSection {
    e.sections.iter().find(|s| s.key == key).unwrap_or_else(|| panic!("falta {key}: {:?}", e.missing))
}

#[test]
fn dfin_ipo_has_every_key_section() {
    let e = extract(include_str!("fixtures/sec_prospectus_dfin_424b4.html"), 0);
    assert_eq!(e.offer_price, Some(14.0));
    assert!(e.missing.is_empty(), "{:?}", e.missing);
    assert!(section(&e, "use_of_proceeds").text.contains("net proceeds from this offering will be approximately $103.5 million"));
    assert!(section(&e, "dilution").text.starts_with("If you invest in our common stock in this offering"));
    assert!(section(&e, "principal_stockholders").text.contains("beneficial ownership"));
    assert!(section(&e, "underwriting").text.contains("underwriters"));
    assert!(section(&e, "risk_factors").text.starts_with("Investing in our common stock involves a high degree of risk"));
    // El «Use of proceeds» del resumen no se confunde con la sección.
    assert!(section(&e, "summary").text.contains("Use of proceeds") || section(&e, "offering").text.contains("proceeds"));
    let lock = section(&e, "lock_up");
    assert_eq!((lock.title.as_str(), lock.parent.as_deref()), ("Lock-Up Agreements", Some("shares_eligible")));
    assert!(lock.text.contains("lock-up") || lock.text.contains("lock‑up"));
    // Las secciones no se pisan entre sí (salvo subsecciones con `parent`).
    assert!(!section(&e, "dilution").text.contains("USE OF PROCEEDS"));
}

#[test]
fn workiva_style_with_title_in_caps_and_tax_sections_in_between() {
    let e = extract(include_str!("fixtures/sec_prospectus_workiva_424b4.html"), 0);
    assert_eq!(e.offer_price, Some(18.0));
    assert!(e.missing.is_empty(), "{:?}", e.missing);
    assert_eq!(section(&e, "principal_stockholders").title, "PRINCIPAL AND SELLING STOCKHOLDERS");
    // Tras «Dilution» viene «UNAUDITED CONSOLIDATED PRO FORMA…», que no pertenece a la sección.
    assert!(!section(&e, "dilution").text.contains("PRO FORMA BALANCE SHEET"));
    assert!(section(&e, "dilution").original_chars < 10_000);
    assert_eq!(section(&e, "lock_up").title, "Lock-Up Agreements");
}

#[test]
fn small_f1_ipo_reports_missing_sections_instead_of_inventing_them() {
    let e = extract(include_str!("fixtures/sec_prospectus_f1_424b1.html"), 0);
    assert_eq!(e.offer_price, Some(5.6));
    assert_eq!(e.missing, ["summary_financial_data", "shares_eligible"]);
    assert!(section(&e, "dilution").text.contains("ADSs"));
    assert_eq!(section(&e, "lock_up").parent.as_deref(), Some("underwriting"));
}

#[test]
fn spac_prospectus_and_section_truncation() {
    let e = extract(include_str!("fixtures/sec_prospectus_spac_424b4.html"), 0);
    assert_eq!(e.offer_price, Some(10.0));
    assert!(section(&e, "use_of_proceeds").text.contains("20,000,000 units"));
    assert!(section(&e, "dilution").text.contains("public offering price per unit"));
    let cut = extract(include_str!("fixtures/sec_prospectus_spac_424b4.html"), 500);
    let s = section(&cut, "risk_factors");
    assert!(s.truncated && s.text.chars().count() == 500 && s.original_chars > 500);
    assert!(!section(&cut, "capitalization").truncated || section(&cut, "capitalization").original_chars > 500);
}

// ---- Rendimiento posterior -------------------------------------------------------------------

#[test]
fn etra_first_weeks_against_spy() {
    let s = parse_daily_series(include_str!("fixtures/yahoo_daily_etra_ipo.json")).unwrap();
    let b = parse_daily_series(include_str!("fixtures/yahoo_daily_spy_sep_oct_2026.json")).unwrap();
    assert_eq!(s.candles.len(), s.adj_close.len());
    let p = compute(&PerfInput {
        symbol: "ETRA",
        ipo_date: d(2026, 9, 18),
        offer_price: 15.0,
        offer_price_source: "calendar",
        series: &s,
        bench: &b,
        bench_name: "SPY",
        source_url: None,
        now: now(),
    })
    .unwrap();
    // Cierre del primer día 13,25 frente a 15,00 de salida.
    assert!((p.first_day_close - 13.25).abs() < 1e-6);
    assert!((p.first_day_return_pct - (13.25 / 15.0 - 1.0) * 100.0).abs() < 1e-6);
    assert!((p.first_day_high_pct - (15.5 / 15.0 - 1.0) * 100.0).abs() < 1e-6, "máximo del día 15,50");
    assert!(p.points[0].price.is_none(), "el mes aún no se ha cumplido el 15-oct");
    assert!(p.latest_return_pct < 0.0 && p.max_drawdown_pct < 0.0);
    assert_eq!(p.latest_date, d(2026, 10, 14));
}

#[test]
fn crwv_full_year_horizons() {
    let s = parse_daily_series(include_str!("fixtures/yahoo_daily_crwv_ipo.json")).unwrap();
    let b = parse_daily_series(include_str!("fixtures/yahoo_daily_spy_2025_2026.json")).unwrap();
    let p = compute(&PerfInput {
        symbol: "CRWV",
        ipo_date: d(2025, 3, 28),
        offer_price: 40.0,
        offer_price_source: "calendar",
        series: &s,
        bench: &b,
        bench_name: "SPY",
        source_url: None,
        now: now(),
    })
    .unwrap();
    assert!(p.points.iter().all(|x| x.price.is_some() && x.benchmark_return_pct.is_some()), "{:?}", p.points);
    let m12 = &p.points[3];
    assert_eq!(m12.target_date, d(2026, 3, 28));
    assert!(m12.price_date.unwrap() <= m12.target_date);
    let (r, bench, ex) = (m12.return_pct.unwrap(), m12.benchmark_return_pct.unwrap(), m12.excess_pct.unwrap());
    assert!((ex - (r - bench)).abs() < 1e-9);
    // El primer día CoreWeave cerró cerca del precio de salida (40 USD).
    assert!(p.first_day_return_pct.abs() < 10.0, "{}", p.first_day_return_pct);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
}

// ---- Heurística de SPAC ------------------------------------------------------------------------

#[test]
fn spac_heuristic_on_real_calendar_names() {
    use market::providers::finnhub::is_spac_likely as spac;
    let p = Some(10.0);
    let yes = [
        ("General Catalyst Global Resilience Merger Corp", "GCGRU"),
        ("West Enclave Merger Corp.", "WENCU"),
        ("B&R Technology Merger Corp.", "BRTMU"),
        ("Xsolla SPAC 1", "XSLLU"),
        ("Archimedes Tech SPAC Partners III Co.", "ARCIU"),
        ("Gores Holdings XI, Inc.", "GHXIU"),
        ("Churchill Capital Corp XIII", "XIIIU"),
        ("Churchill Capital Corp XII", "CXIIU"),
        ("Churchill Capital Corp XI", "CCXIU"),
        ("SilverBox Corp V", "SBXEU"),
        ("Aldabra 4 Liquidity Opportunity Vehicle, Inc.", "ALOVU"),
        ("Cantor Equity Partners VII, Inc.", "CAES"),
        ("Cantor Equity Partners VI, Inc.", "CEPS"),
        ("Research Alliance Corp III", "RACC"),
        ("Research Alliance Corp IV", "RACD"),
        ("Helix Quarry Acquisition Corp.", "HQACU"),
        ("Foo Industries Corp 4", "FOOX"),
        ("Bar Capital Four", "BARX"),
        ("Baz Partners Inc. III", "BAZX"),
    ];
    for (name, sym) in yes {
        assert!(spac(name, Some(sym), p, p), "{name} debería marcarse");
    }
    // Un símbolo de unidades a 10 USD basta por sí solo.
    assert!(spac("Nombre Neutro", Some("ZZZZU"), p, p));
    // Casos negativos: conversión de caja de ahorros a 10 USD y empresas operativas sin palabras clave.
    for (name, sym) in [("Hoyne Bancorp, Inc.", "HYNE"), ("Lumen Harbor Therapeutics", "LHTX"), ("Nimbus Row Foods, Inc.", "NRWF")] {
        assert!(!spac(name, Some(sym), p, p), "{name} NO debe marcarse");
    }
    // Precio distinto de 10: las reglas nuevas no se aplican (aunque el nombre sea de SPAC).
    assert!(!spac("Churchill Capital Corp XIII", Some("XIIIU"), Some(18.0), Some(18.0)));
    assert!(!spac("Gores Holdings XI, Inc.", Some("GHXIU"), Some(9.0), Some(11.0)));
    // Regla antigua: «Acquisition» con símbolo de unidades aunque el precio falte.
    assert!(spac("Foo Acquisition Corp", Some("FOOAU"), None, None));
}

#[test]
fn edgar_fallback_flags_blank_checks_by_sic_6770() {
    let mk = |sic: &str| {
        format!(
            r#"{{"hits":{{"hits":[{{"_source":{{"ciks":["0001"],"display_names":["X Corp  (CIK 0000000001)"],"form":"424B4","file_date":"2026-10-01","adsh":"0000000001-26-000001","sics":["{sic}"]}}}}]}}}}"#
        )
    };
    assert!(parse_fts(&mk("6770")).unwrap()[0].blank_check);
    assert!(!parse_fts(&mk("2834")).unwrap()[0].blank_check);
}
