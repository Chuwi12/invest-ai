//! Informes periódicos con documentos REALES de EDGAR (13-nov-2026), recortados: 10-K de Apple, Exxon
//! Mobil y JPMorgan, 10-Q de Apple y Caterpillar, EX-99.1 de resultados de Apple, 20-F de SAP y de
//! ASML y la página de índice de un 8-K. Sin red ni claves. El texto es el del documento; solo se
//! recortó el cuerpo de cada apartado (el índice, las cabeceras y el principio de cada sección se
//! conservan tal cual).

use chrono::NaiveDate;
use domain::ProspectusSection;
use market::{
    company_filing::{CompanyExtract, Report, extract},
    providers::sec::{FilingRow, parse_company_submissions, parse_filing_rows, pick_exhibit_99, plan_company_reports},
};

const AAPL_10K: &str = include_str!("fixtures/sec_company_10k_aapl_excerpt.htm");
const XOM_10K: &str = include_str!("fixtures/sec_company_10k_xom_excerpt.htm");
const JPM_10K: &str = include_str!("fixtures/sec_company_10k_jpm_excerpt.htm");
const AAPL_10Q: &str = include_str!("fixtures/sec_company_10q_aapl_excerpt.htm");
const CAT_10Q: &str = include_str!("fixtures/sec_company_10q_cat_excerpt.htm");
const AAPL_EX99: &str = include_str!("fixtures/sec_company_ex99_aapl_excerpt.htm");
const SAP_20F: &str = include_str!("fixtures/sec_company_20f_sap_excerpt.htm");
const ASML_20F: &str = include_str!("fixtures/sec_company_20f_asml_excerpt.htm");

fn get<'a>(e: &'a CompanyExtract, key: &str) -> &'a ProspectusSection {
    e.sections.iter().find(|s| s.key == key).unwrap_or_else(|| panic!("falta {key}: {:?}", e.missing))
}

fn keys(e: &CompanyExtract) -> Vec<&str> {
    e.sections.iter().map(|s| s.key.as_str()).collect()
}

#[test]
fn apple_10k_skips_the_index_and_finds_four_sections() {
    let e = extract(Report::TenK, AAPL_10K, 150_000);
    assert_eq!(keys(&e), ["business", "risk_factors", "mdna", "market_risk"]);
    assert!(e.missing.is_empty(), "{:?}", e.missing);
    // El índice trae «Item 1A. | Risk Factors | 5»: la sección es el cuerpo, no esa fila.
    let risk = get(&e, "risk_factors");
    assert_eq!(risk.title, "Item 1A. Risk Factors");
    assert!(risk.text.starts_with("The following summarizes factors that could have a material adverse effect"), "{}", risk.text);
    assert!(!risk.text.contains("Unresolved Staff Comments"), "no arrastra el apartado siguiente");
    assert!(!risk.text.contains("Form 10-K |"), "el pie de página no es texto");
    assert!(risk.text.chars().count() > 5000 && !risk.truncated);
    assert!(get(&e, "mdna").text.starts_with("The following discussion should be read in conjunction"));
    assert!(get(&e, "market_risk").text.starts_with("The Company is exposed to economic risk from interest rates"));
    assert!(get(&e, "business").text.contains("designs, manufactures and markets smartphones"));
    assert!(e.sections.iter().all(|s| s.parent.is_none() && s.original_chars > 0));
}

#[test]
fn a_cross_reference_instead_of_a_section_goes_to_missing() {
    // Exxon: «Item 7. … Reference is made to the section entitled…» (160 caracteres, sin cuerpo).
    let e = extract(Report::TenK, XOM_10K, 150_000);
    assert_eq!(keys(&e), ["business", "risk_factors"]);
    assert_eq!(e.missing, ["mdna", "market_risk"]);
    assert!(get(&e, "risk_factors").text.starts_with("ExxonMobil’s financial and operating results are subject to a variety of risks"));
    // JPMorgan: cabeceras sin negrita y con punto final, y el índice con rangos de páginas («9-31»).
    let e = extract(Report::TenK, JPM_10K, 150_000);
    assert_eq!(keys(&e), ["business", "risk_factors"]);
    assert_eq!(e.missing, ["mdna", "market_risk"]);
    assert_eq!(get(&e, "risk_factors").title, "Item 1A. Risk Factors.");
    assert!(get(&e, "risk_factors").text.starts_with("The following discussion sets forth the material risk factors"));
}

#[test]
fn quarterly_report_keeps_a_risk_update_with_content() {
    let e = extract(Report::TenQ, AAPL_10Q, 150_000);
    assert_eq!(keys(&e), ["mdna", "risk_factors"]);
    assert!(get(&e, "mdna").text.starts_with("This Item and other sections of this Quarterly Report on Form 10-Q"));
    let risk = get(&e, "risk_factors");
    assert!(risk.text.starts_with("The Company’s business, reputation, results of operations"), "{}", risk.text);
    // Part II, Item 2 («Unregistered Sales…») tiene el mismo número que el Item 2 de MD&A: no se confunden.
    assert!(!get(&e, "mdna").text.contains("Unregistered Sales"));
}

#[test]
fn a_quarterly_no_material_changes_statement_goes_to_missing() {
    // Caterpillar: «There have been no material changes to the risk factors we previously disclosed…».
    let e = extract(Report::TenQ, CAT_10Q, 150_000);
    assert_eq!(keys(&e), ["mdna"]);
    assert_eq!(e.missing, ["risk_factors"]);
    assert!(get(&e, "mdna").text.starts_with("The following Management’s Discussion and Analysis"));
}

#[test]
fn earnings_release_flattens_tables_into_readable_lines() {
    let e = extract(Report::Release, AAPL_EX99, 150_000);
    let r = get(&e, "earnings_release");
    assert_eq!(r.title, "Apple reports fourth quarter results");
    assert!(r.text.contains("today announced financial results for its fiscal 2026 fourth quarter"));
    assert!(r.text.contains("Total net sales (1) | 114,477 | 102,466 | 478,834 | 416,161"), "{}", r.text);
    assert!(r.text.contains("Diluted | $2.10 | $1.85 | $8.98 | $7.46"));
    assert!(!r.text.contains("Exhibit 99.1") && e.missing.is_empty());
    assert!(extract(Report::Release, "<html><body><p>Exhibit 99.1</p></body></html>", 1000).missing == ["earnings_release"]);
}

#[test]
fn sap_20f_finds_risk_factors_inside_item_3_and_the_operating_review() {
    let e = extract(Report::TwentyF, SAP_20F, 150_000);
    assert_eq!(keys(&e), ["risk_factors", "operating_review"]);
    assert!(e.missing.is_empty());
    let risk = get(&e, "risk_factors");
    assert_eq!(risk.title, "Risk Factors");
    assert!(risk.text.starts_with("Our operations and financial results are subject to various risks"), "{}", risk.text);
    assert!(!risk.text.contains("INFORMATION ABOUT SAP"));
    // El índice del 20-F lista «ITEM 5. … | 43» con sus subentradas debajo («Overview | 43»…): no es la sección.
    let op = get(&e, "operating_review");
    assert_eq!(op.title, "ITEM 5. OPERATING AND FINANCIAL REVIEW AND PROSPECTS");
    assert!(op.text.starts_with("Overview\nFor information on our principal sources of revenue"), "{}", op.text);
    assert!(!op.text.contains(" | 43"));
}

#[test]
fn an_integrated_annual_report_without_item_headings_is_not_split() {
    // ASML: el 20-F es la memoria anual; solo hay una tabla de equivalencias al final.
    let e = extract(Report::TwentyF, ASML_20F, 150_000);
    assert!(e.sections.is_empty(), "{:?}", keys(&e));
    assert_eq!(e.missing, ["risk_factors", "operating_review"]);
    assert!(e.document_chars > 10_000);
}

#[test]
fn sections_are_truncated_and_flagged() {
    let e = extract(Report::TenK, AAPL_10K, 300);
    let r = get(&e, "risk_factors");
    assert!(r.truncated && r.text.chars().count() == 300 && r.original_chars > 5000);
    // `business` se recorta siempre a 20 000 caracteres, aunque el tope general sea mayor o no exista.
    let long = format!(
        "<p><b>Item 1. Business</b></p>{}<p><b>Item 1A. Risk Factors</b></p><p>{}</p>",
        "<p>Our business is selling widgets worldwide to many customers.</p>".repeat(600),
        "Risk text. ".repeat(100)
    );
    for max in [150_000, 0] {
        let e = extract(Report::TenK, &long, max);
        let b = get(&e, "business");
        assert!(b.truncated && b.text.chars().count() == 20_000 && b.original_chars > 30_000, "{max}");
        assert!(!get(&e, "risk_factors").truncated);
    }
}

#[test]
fn headings_are_read_by_shape_not_by_position() {
    let body = "A risk factor sentence long enough to count as content. ".repeat(12);
    let html = format!(
        "<p><b>Item 1A.</b></p><p><b>Risk Factors</b></p><p>{body}</p>\
         <p>Item 8 of this Annual Report describes the financial statements and the audit.</p>\
         <p><b>Item 1A. Risk Factors (continued)</b></p><p>{body}</p>\
         <p>42</p><p>Table of Contents</p><p>Acme Corp | 2026 Form 10-K | 17</p><p>More text after the page break.</p>\
         <p><b>Item 1B. Unresolved Staff Comments</b></p><p>None.</p>"
    );
    let e = extract(Report::TenK, &html, 0);
    let r = get(&e, "risk_factors");
    // Título en dos líneas, cabecera repetida en cada página y frase que empieza por «Item 8…»: nada corta la sección.
    assert_eq!(r.title, "Item 1A. Risk Factors");
    assert!(r.text.starts_with("A risk factor sentence"), "el título no se repite en el cuerpo");
    assert!(r.text.contains("Item 8 of this Annual Report describes") && r.text.contains("More text after the page break."));
    assert!(!r.text.contains("Table of Contents") && !r.text.contains("Form 10-K |") && !r.text.contains("\n42\n"));
    assert!(!r.text.contains("Unresolved") && !r.text.contains("None."));
}

#[test]
fn hostile_documents_cost_linear_time() {
    let t = std::time::Instant::now();
    // Cien mil cabeceras idénticas y cien mil que se alternan, sin cuerpo.
    let same = "<p><b>Item 1A. Risk Factors</b></p>".repeat(100_000);
    let e = extract(Report::TenK, &same, 150_000);
    assert!(e.sections.len() <= 1);
    let alt = "<p><b>Item 1A. Risk Factors</b></p><p><b>Item 7. Management’s Discussion</b></p>".repeat(50_000);
    assert!(extract(Report::TenK, &alt, 150_000).sections.is_empty());
    let sap = "<p><b>ITEM 3. KEY INFORMATION</b></p><p><b>Risk Factors</b></p>".repeat(50_000);
    assert!(extract(Report::TwentyF, &sap, 150_000).sections.len() <= 1);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

fn rows(json: &str) -> Vec<FilingRow> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    parse_filing_rows(&v["filings"]["recent"])
}

fn day(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

#[test]
fn plan_takes_the_latest_annual_and_a_later_quarterly_and_a_recent_earnings_8k() {
    // Apple: el 10-K (3-nov) es posterior al último 10-Q (31-jul): sin trimestral. 8-K de resultados del 2-nov.
    let r = rows(include_str!("fixtures/sec_company_submissions_aapl.json"));
    let p = plan_company_reports(&r, day("2026-11-13"));
    assert_eq!(p.annual.as_ref().map(|f| (f.form.as_str(), f.accession.as_str())), Some(("10-K", "0000320193-26-000026")));
    assert!(p.quarterly.is_none());
    let e = p.earnings.unwrap();
    assert_eq!((e.accession.as_str(), e.filed, e.items.as_str()), ("0000320193-26-000024", day("2026-11-02"), "2.02,9.01"));
    // El 8-K más reciente (9-nov, Items 8.01 y 9.01) no es de resultados.
    assert!(r.iter().any(|f| f.form == "8-K" && f.filed == day("2026-11-09")));
    // Pasados 120 días ya no hay nota de resultados reciente.
    assert!(plan_company_reports(&r, day("2027-03-04")).earnings.is_none());
    assert!(plan_company_reports(&r, day("2027-03-02")).earnings.is_some());
    // Coca-Cola: 10-K de febrero y 10-Q de octubre (posterior), más el 8-K 2.02 del 27-oct.
    let p = plan_company_reports(&rows(include_str!("fixtures/sec_company_submissions_ko.json")), day("2026-11-13"));
    assert_eq!(p.annual.unwrap().accession, "0001628280-26-010047");
    assert_eq!(p.quarterly.unwrap().accession, "0001628280-26-067844");
    assert_eq!(p.earnings.unwrap().accession, "0001628280-26-067403");
    // SAP presenta 20-F y 6-K: sin 10-Q ni 8-K.
    let p = plan_company_reports(&rows(include_str!("fixtures/sec_company_submissions_sap.json")), day("2026-11-13"));
    assert_eq!(p.annual.as_ref().map(|f| (f.form.as_str(), f.accession.as_str())), Some(("20-F", "0001104659-26-020058")));
    assert!(p.quarterly.is_none() && p.earnings.is_none());
}

#[test]
fn item_2_02_is_matched_exactly_and_amendments_are_ignored() {
    let row = |form: &str, filed: &str, items: &str, acc: &str| FilingRow {
        accession: acc.into(),
        form: form.into(),
        filed: day(filed),
        document: "x.htm".into(),
        items: items.into(),
    };
    let rows = vec![
        row("8-K", "2026-11-10", "5.02,12.02", "0000000001-26-000001"),
        row("8-K/A", "2026-11-09", "2.02", "0000000001-26-000002"),
        row("10-K/A", "2026-11-08", "", "0000000001-26-000003"),
        row("8-K", "2026-11-05", "7.01, 2.02 ,9.01", "0000000001-26-000004"),
        row("8-K", "2026-11-01", "2.02", "0000000001-26-000005"),
        row("10-Q", "2026-08-01", "", "0000000001-26-000006"),
    ];
    let p = plan_company_reports(&rows, day("2026-11-13"));
    assert_eq!(p.earnings.unwrap().accession, "0000000001-26-000004");
    // Sin 10-K en las filas, el 10-Q cuenta.
    assert!(p.annual.is_none() && p.quarterly.unwrap().accession == "0000000001-26-000006");
}

#[test]
fn the_press_release_is_the_ex_99_1_row_of_the_index_page() {
    let index = include_str!("fixtures/sec_company_index_aapl_8k.htm");
    assert_eq!(pick_exhibit_99(index).as_deref(), Some("a8-kex991q4202609262026.htm"));
    assert_eq!(pick_exhibit_99("<html><body>sin tabla</body></html>"), None);
    // Sin anexo EX-99.1 (solo el 8-K) o con una ruta que no es un fichero HTML de la presentación: nada.
    let no99 = index.replace("EX-99.1", "EX-99.2");
    assert_eq!(pick_exhibit_99(&no99), None);
    let evil = |href: &str| format!("<table><tr><td>2</td><td>EX-99.1</td><td><a href=\"{href}\">x</a></td><td>EX-99.1</td></tr></table>");
    assert_eq!(pick_exhibit_99(&evil("/Archives/edgar/data/1/2/ex991.htm")).as_deref(), Some("ex991.htm"));
    for bad in ["../../etc/passwd", "/a/b c.htm", "/a/b.pdf", "/a/..htm"] {
        assert_eq!(pick_exhibit_99(&evil(bad)), None, "{bad}");
    }
}

#[test]
fn submissions_without_recent_filings_are_an_error_not_an_empty_company() {
    // Un `Ok` vacío haría que el servidor borrase del archivo los informes que ya tenía.
    let today = day("2026-11-13");
    for bad in ["{}", "[]", "null", r#"{"name":"Acme","filings":{}}"#, r#"{"name":"Acme","filings":{"recent":{}}}"#, r#"{"filings":{"recent":{"form":[],"accessionNumber":[]}}}"#, "no es json"] {
        assert!(parse_company_submissions(bad, "Acme", today).is_err(), "{bad}");
    }
    let (name, plan) = parse_company_submissions(include_str!("fixtures/sec_company_submissions_aapl.json"), "x", today).unwrap();
    assert_eq!(name, "Apple Inc.");
    assert_eq!(plan.annual.unwrap().accession, "0000320193-26-000026");
    // Sin `name` se usa el del fichero de tickers; con filas pero sin 10-K/10-Q/8-K es una empresa sin informes: `Ok`.
    let quiet = r#"{"filings":{"recent":{"accessionNumber":["0000000001-26-000001"],"filingDate":["2026-11-01"],"form":["4"],"items":[""],"primaryDocument":["x.xml"]}}}"#;
    let (name, plan) = parse_company_submissions(quiet, "Acme Corp", today).unwrap();
    assert_eq!(name, "Acme Corp");
    assert_eq!(plan, Default::default());
}
