//! Folletos de ETF con documentos REALES de EDGAR (19-oct-2026), recortados: iShares 20+ Year
//! Treasury Bond ETF (TLT, 497K), Invesco QQQ Trust (QQQ, 497K) y SPDR S&P 500 ETF Trust (SPY,
//! 485BPOS; fideicomiso de unidades, ausente de `company_tickers_mf.json`). Sin red ni claves.

use market::{
    fund_prospectus::{contains_word, extract},
    providers::sec::{parse_atom_name, parse_filing_atom, parse_fund_tickers, pick_main_document},
};

const TLT: &str = include_str!("fixtures/sec_fund_497k_tlt.htm");
const QQQ: &str = include_str!("fixtures/sec_fund_497k_qqq.htm");
const SPY: &str = include_str!("fixtures/sec_fund_485bpos_spy_excerpt.htm");

fn section<'a>(e: &'a market::fund_prospectus::FundExtract, key: &str) -> &'a str {
    &e.sections.iter().find(|s| s.key == key).unwrap_or_else(|| panic!("falta {key}: {:?}", e.missing)).text
}

#[test]
fn tlt_summary_prospectus_has_the_five_key_sections() {
    let e = extract(TLT, "TLT", 6000);
    assert!(e.usable() && e.mentions_ticker, "{:?}", e.missing);
    assert!(e.missing.is_empty(), "{:?}", e.missing);
    assert!(section(&e, "objective").contains("seeks to track the investment results"), "{}", section(&e, "objective"));
    assert!(section(&e, "fees").contains("Management Fee") && section(&e, "fees").contains("0.15"));
    assert!(section(&e, "strategy").contains("indexing approach"));
    assert!(section(&e, "risks").to_lowercase().contains("risk"));
    assert!(section(&e, "turnover").to_lowercase().contains("turnover"));
    // El objetivo no arrastra la sección siguiente.
    assert!(!section(&e, "objective").contains("Management Fee"));
    assert!(e.fund_name.as_deref().is_some_and(|n| n.to_uppercase().contains("20+ YEAR TREASURY")), "{:?}", e.fund_name);
}

#[test]
fn qqq_summary_prospectus_is_usable_and_names_the_ticker() {
    let e = extract(QQQ, "QQQ", 6000);
    assert!(e.usable() && e.mentions_ticker, "{:?}", e.missing);
    assert!(e.missing.is_empty(), "«Fund Fees and Expenses» también es la sección de comisiones: {:?}", e.missing);
    assert!(section(&e, "fees").contains("Management Fees"));
    assert!(section(&e, "objective").to_lowercase().contains("nasdaq-100"), "{}", section(&e, "objective"));
}

#[test]
fn a_supplement_for_other_funds_is_not_taken_for_the_etf() {
    let html = "<html><body><p>SUPPLEMENT DATED JUNE 10, 2026</p><p>Invesco NASDAQ 100 ETF (QQQM)</p><p>The Board approved a change in the fee waiver of the funds listed above.</p></body></html>";
    let e = extract(html, "QQQ", 6000);
    assert!(!e.usable() && !e.mentions_ticker);
}

#[test]
fn spy_unit_trust_prospectus_skips_the_table_of_contents() {
    let e = extract(SPY, "SPY", 6000);
    assert!(e.usable(), "{:?}", e.missing);
    assert!(section(&e, "objective").contains("S&P 500"), "{}", section(&e, "objective"));
    assert!(section(&e, "strategy").contains("common stocks"));
    assert!(section(&e, "turnover").contains("3%"), "{}", section(&e, "turnover"));
}

#[test]
fn sections_are_truncated_and_flagged() {
    let e = extract(TLT, "TLT", 300);
    let r = e.sections.iter().find(|s| s.key == "risks").unwrap();
    assert!(r.truncated && r.text.chars().count() <= 300 && r.original_chars > 300);
}

#[test]
fn ticker_matches_whole_words_only() {
    assert!(contains_word("Invesco QQQ Trust", "QQQ"));
    assert!(contains_word("shares (TLT)", "TLT"));
    assert!(!contains_word("Invesco NASDAQ 100 ETF (QQQM)", "QQQ"));
    assert!(!contains_word("", "QQQ"));
}

#[test]
fn mutual_fund_ticker_file_maps_ticker_to_series_and_class() {
    let m = parse_fund_tickers(include_str!("fixtures/sec_company_tickers_mf_excerpt.json")).unwrap();
    let tlt = &m["TLT"];
    assert_eq!((tlt.cik, tlt.series_id.as_str(), tlt.class_id.as_str()), (1100663, "S000004360", "C000012090"));
    assert_eq!(m["QQQ"].cik, 1067839);
    assert!(!m.contains_key("SPY"), "SPY es un fideicomiso de unidades: no figura");
    assert!(parse_fund_tickers("{\"fields\":[\"cik\"],\"data\":[]}").is_err());
}

#[test]
fn atom_feed_gives_the_newest_filings_without_duplicates() {
    let (cik, f) = parse_filing_atom(include_str!("fixtures/sec_fund_atom_497k_tlt.xml"));
    assert_eq!(cik, Some(1100663));
    assert_eq!(f[0].accession, "0001193125-26-287948");
    assert_eq!(f[0].form, "497K");
    assert!(f.windows(2).all(|w| w[0].filed >= w[1].filed));
    let mut acc: Vec<_> = f.iter().map(|x| &x.accession).collect();
    acc.sort();
    acc.dedup();
    assert_eq!(acc.len(), f.len());
    let (cik, f) = parse_filing_atom(include_str!("fixtures/sec_fund_atom_497k_qqq.xml"));
    assert_eq!(cik, Some(1067839));
    assert!(f.iter().any(|x| x.accession == "0001104659-25-123275"));
    assert_eq!(parse_filing_atom("<feed></feed>"), (None, vec![]));
    assert_eq!(parse_atom_name(include_str!("fixtures/sec_fund_atom_497k_qqq.xml")).as_deref(), Some("INVESCO QQQ TRUST, SERIES 1"));
}

#[test]
fn main_document_is_the_largest_html_of_the_filing() {
    assert_eq!(pick_main_document(include_str!("fixtures/sec_fund_index_tlt.json"), 1_500_000).as_deref(), Some("d128154d497k.htm"));
    assert_eq!(pick_main_document(include_str!("fixtures/sec_fund_index_qqq.json"), 1_500_000).as_deref(), Some("tm2529177d5_497k.htm"));
    assert_eq!(pick_main_document(include_str!("fixtures/sec_fund_index_tlt.json"), 1000), None, "por encima del tope");
    assert_eq!(pick_main_document("no es json", 1000), None);
}
