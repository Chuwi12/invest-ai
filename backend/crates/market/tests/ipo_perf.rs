//! Coste de leer folletos manipulados. Un folleto de EDGAR trae texto de terceros: con una entrada
//! hostil de unos 400 KB, el análisis debe costar lo mismo que con una normal (milisegundos), no
//! segundos o minutos con un worker de Tokio ocupado. Las entradas son las medidas en la auditoría
//! de seguridad (hallazgo B1) más otros bucles del mismo estilo hallados al revisar los módulos.
//!
//! Sin red y sin dependencias de reloj externas. Cada caso corre en un hilo con un tope duro de
//! 30 s: si el coste vuelve a ser cuadrático, el test FALLA en vez de colgarse. Los límites son
//! generosos (entre 5 y 30 veces lo que tarda hoy en debug, y por debajo de lo que tardaba el código
//! cuadrático: segundos o minutos), y los casos se serializan para no competir entre sí por CPU.

use std::{
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, Utc};
use domain::{IpoIndicatorStatus, IpoRiskProfile, Prospectus, ProspectusSection};
use market::{
    fund_prospectus,
    html::html_to_lines,
    ipo_risk::{ProfileInput, build_profile},
    prospectus,
};

/// Tamaño de la entrada hostil de la auditoría.
const KB400: usize = 400 * 1024;
/// Si un caso tarda más que esto, se da por colgado (coste cuadrático) y el test falla.
const HARD_CAP: Duration = Duration::from_secs(30);

/// Los casos miden tiempo: de uno en uno.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ejecuta `f` en un hilo con tope duro y devuelve lo que tardó y su resultado.
fn timed<T: Send + 'static>(label: &str, f: impl FnOnce() -> T + Send + 'static) -> (Duration, T) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let out = f();
        let _ = tx.send((t0.elapsed(), out));
    });
    match rx.recv_timeout(HARD_CAP) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{label}: sin terminar tras {HARD_CAP:?} (¿coste cuadrático?)"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{label}: el análisis hizo panic"),
    }
}

fn within(label: &str, took: Duration, limit_ms: u64) {
    println!("{label}: {took:?} (límite {limit_ms} ms)");
    assert!(took < Duration::from_millis(limit_ms), "{label}: {took:?} (límite {limit_ms} ms)");
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-20T00:00:00Z").unwrap().with_timezone(&Utc)
}

fn prospectus_with(key: &str, text: String) -> Prospectus {
    Prospectus {
        issuer: "Hostile Corp.".into(),
        cik: 1,
        ticker: None,
        form: "424B4".into(),
        preliminary: false,
        filed_on: NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
        accession: "0000000000-26-000001".into(),
        url: "https://www.sec.gov/Archives/edgar/data/1/x.htm".into(),
        index_url: "https://www.sec.gov/Archives/edgar/data/1/".into(),
        offer_price: None,
        sections: vec![ProspectusSection { key: key.into(), title: key.into(), original_chars: text.chars().count(), text, parent: None, truncated: false }],
        missing: Vec::new(),
        document_chars: 0,
        source: "sec".into(),
        fetched_at: now(),
    }
}

fn profile_of(p: Prospectus) -> IpoRiskProfile {
    build_profile(ProfileInput { symbol: "HOST", entry: None, prospectus: Some(&p), base_rate: None, now: now() })
}

fn indicator<'a>(p: &'a IpoRiskProfile, key: &str) -> &'a domain::IpoRiskIndicator {
    p.indicators.iter().find(|i| i.key == key).unwrap_or_else(|| panic!("falta {key}"))
}

// ---- Los tres casos de la auditoría (B1) --------------------------------------------------------

/// `sentences()`: cada línea corta sin puntuación se pegaba a la anterior y se releía la línea
/// lógica entera en busca de « | ». 0,4 s a 200 KB y 1,2 s a 400 KB en release (minutos en debug).
#[test]
fn one_char_lines_keep_sentence_splitting_linear() {
    let _g = serial();
    let text = "a\n".repeat(KB400 / 2);
    let (took, pr) = timed("sentences", move || profile_of(prospectus_with("use_of_proceeds", text)));
    within("líneas de un carácter (use_of_proceeds)", took, 1_500);
    assert_eq!(indicator(&pr, "use_of_proceeds").status, IpoIndicatorStatus::Extracted);
}

/// `duration_candidates()`: cada «month» retrocedía por todas las letras previas.
#[test]
fn a_run_of_month_keeps_the_lockup_scan_linear() {
    let _g = serial();
    let text = "month".repeat(KB400 / 5);
    let (took, pr) = timed("duration_candidates", move || profile_of(prospectus_with("lock_up", text)));
    within("«month» repetido (lock_up)", took, 800);
    assert_eq!(indicator(&pr, "lock_up").status, IpoIndicatorStatus::NotExtracted);
}

/// `html_to_lines()`: cada `<style>` pasaba a minúsculas todo el resto del documento.
#[test]
fn empty_style_blocks_keep_html_parsing_linear() {
    let _g = serial();
    let html = "<style></style>".repeat(KB400 / 15);
    let (took, lines) = timed("html_to_lines", move || html_to_lines(&html));
    within("<style></style> repetido", took, 800);
    assert!(lines.is_empty());
}

// ---- Otros bucles del mismo estilo --------------------------------------------------------------

/// `strip_footnote()`: copiaba la etiqueta entera por cada nota «(1)» recortada.
#[test]
fn long_footnote_chains_keep_the_financial_rows_linear() {
    let _g = serial();
    let text = format!("Net loss{} | 5\nRevenue | 10\n", "(1)".repeat(KB400 / 3));
    let (took, pr) = timed("strip_footnote", move || profile_of(prospectus_with("summary_financial_data", text)));
    within("«Net loss(1)(1)…» en una etiqueta", took, 900);
    // Ninguna fila útil: la etiqueta hostil no es «net loss» y «Revenue» sí se lee.
    assert_eq!(indicator(&pr, "revenue").status, IpoIndicatorStatus::Extracted);
}

/// `Parser::end()`: cada cierre sin apertura recorría la pila (hasta 2 000 marcos).
#[test]
fn stray_closing_tags_do_not_walk_the_whole_open_stack() {
    let _g = serial();
    let html = format!("{}{}", "<i>".repeat(1_990), "</u>".repeat(KB400 / 4));
    let (took, lines) = timed("end", move || html_to_lines(&html));
    within("cierres huérfanos con la pila llena", took, 900);
    assert!(lines.is_empty());
}

/// `Headings::next_term()`: recorrido lineal de los títulos por cada candidato.
#[test]
fn thousands_of_bold_headings_keep_section_detection_linear() {
    let _g = serial();
    let html = "<p><b>DILUTION</b></p>".repeat(2 * KB400 / 22);
    let (took, ex) = timed("next_term", move || prospectus::extract(&html, 0));
    within("«DILUTION» en negrita repetido (800 KB)", took, 1_500);
    assert!(ex.sections.is_empty(), "sin cuerpo no hay sección");
}

/// `fund_prospectus::extract`: `end_of` recorría todas las cabeceras por cada una.
#[test]
fn thousands_of_fund_headings_keep_extraction_linear() {
    let _g = serial();
    let html = "<p><b>Investment Objective</b></p>".repeat(8 * KB400 / 34);
    let (took, ex) = timed("fund end_of", move || fund_prospectus::extract(&html, "SPY", 0));
    within("«Investment Objective» repetido (3,2 MB)", took, 3_000);
    assert!(ex.sections.is_empty());
}

/// `float_offered()`: buscaba cada etiqueta de clase en la lista ya reunida.
#[test]
fn thousands_of_share_class_rows_keep_float_offered_linear() {
    let _g = serial();
    let mut text = String::from("Common stock offered by us | 5,000,000 shares\n");
    let mut i = 0u64;
    while text.len() < 8 * KB400 {
        text.push_str(&format!("Class {i:06} shares outstanding after this offering | {} shares\n", 1_000 + i));
        i += 1;
    }
    let (took, pr) = timed("float_offered", move || profile_of(prospectus_with("offering", text)));
    within("miles de clases «outstanding after this offering» (3,2 MB)", took, 3_000);
    assert_eq!(indicator(&pr, "float_offered").status, IpoIndicatorStatus::Extracted);
}

/// Control: texto normal del mismo tamaño que el hostil (marca la línea base del coste lineal).
#[test]
fn normal_text_of_the_same_size_stays_within_the_same_budget() {
    let _g = serial();
    let text = "We intend to use the net proceeds for general corporate purposes. ".repeat(KB400 / 66);
    let (took, pr) = timed("control", move || profile_of(prospectus_with("use_of_proceeds", text)));
    within("texto normal de 400 KB", took, 1_500);
    assert_eq!(indicator(&pr, "use_of_proceeds").status, IpoIndicatorStatus::Extracted);
}
