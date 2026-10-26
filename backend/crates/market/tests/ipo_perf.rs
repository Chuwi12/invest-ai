//! Coste de leer folletos manipulados. Un folleto de EDGAR trae texto de terceros: con una entrada
//! hostil de unos 400 KB, el análisis debe costar lo mismo que con una normal (milisegundos), no
//! segundos o minutos con un worker de Tokio ocupado. Las entradas son las medidas en la auditoría
//! de seguridad (hallazgo B1) más otros bucles del mismo estilo hallados al revisar los módulos.
//!
//! Sin red y sin dependencias de reloj externas.
//!
//! # Criterio de medida
//!
//! - **Lo que se compara con el límite es el tiempo de CPU del hilo del caso**
//!   (`CLOCK_THREAD_CPUTIME_ID`, leído dentro del propio hilo antes y después), no el reloj de pared.
//!   Con la máquina cargada (varios agentes compilando, el portátil con batería) el reloj de pared de un
//!   caso se infla varias veces sin que el código haya cambiado y el test fallaba sin que el coste
//!   hubiera vuelto a ser cuadrático (los casos de 3,2 MB llegaban al 76 % de su límite de 3 s). El
//!   tiempo de CPU solo cuenta lo que ese hilo ejecutó, aunque compita con otros procesos por el núcleo.
//! - Los límites siguen siendo absolutos y generosos (en segundos de CPU, compilado en debug): entre 5 y
//!   40 veces lo que cuesta hoy (en reposo, de 23 ms a 0,55 s de CPU según el caso) y muy por debajo de
//!   lo que costaba el código cuadrático (segundos o minutos). El tiempo de CPU sí sube algo si el núcleo
//!   baja de frecuencia o comparte núcleo físico con otro hilo (SMT), pero en un factor pequeño, no en
//!   múltiplos: de ahí el margen.
//! - Comprobado: con el binario y tres bucles ocupados fijados al mismo núcleo (`taskset`), el reloj de
//!   pared de los casos de 3,2 MB pasó de 0,5 s a 2,0-2,1 s (el 70 % del antiguo límite de 3 s) y su CPU
//!   se quedó en 0,53-0,58 s.
//! - Sin esa medida (plataformas que no sean Linux de 64 bits) se vuelve al reloj de pared y se dice en
//!   la salida de cada caso (`reloj de pared`).
//! - Cada caso corre en su hilo y los casos se serializan para no competir entre sí.
//! - Tope duro de 30 s de reloj de pared por caso: si el coste vuelve a ser cuadrático, el test FALLA en
//!   vez de colgarse. Un hilo no se puede matar y seguiría gastando CPU (y falseando los casos que vienen
//!   detrás), así que al saltar el tope se escribe el diagnóstico y se termina el proceso de pruebas.

use std::{
    io::Write,
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
/// Si un caso tarda más que esto (reloj de pared), se da por colgado (coste cuadrático) y el test falla.
const HARD_CAP: Duration = Duration::from_secs(30);

/// Los casos miden tiempo: de uno en uno.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tiempo de CPU consumido hasta ahora por el hilo que llama (`CLOCK_THREAD_CPUTIME_ID`).
/// `clock_gettime` la aporta la libc que `std` ya enlaza: no hace falta ninguna dependencia.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn thread_cpu_time() -> Option<Duration> {
    /// `struct timespec` de Linux de 64 bits (`time_t` y `long` de 64 bits).
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock_id: i32, tp: *mut Timespec) -> i32;
    }
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` es un `timespec` válido y escribible durante toda la llamada.
    if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
        return None;
    }
    Some(Duration::new(u64::try_from(ts.tv_sec).ok()?, u32::try_from(ts.tv_nsec).ok()?))
}

#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
fn thread_cpu_time() -> Option<Duration> {
    None
}

/// Lo que costó un caso: CPU del hilo (si la plataforma la da) y reloj de pared.
#[derive(Clone, Copy)]
struct Cost {
    cpu: Option<Duration>,
    wall: Duration,
}

impl Cost {
    /// Lo que se compara con el límite: CPU del hilo, o el reloj de pared si no hay forma de medirla.
    fn measured(self) -> Duration {
        self.cpu.unwrap_or(self.wall)
    }
}

/// Ejecuta `f` en un hilo con tope duro y devuelve lo que costó y su resultado.
fn timed<T: Send + 'static>(label: &str, f: impl FnOnce() -> T + Send + 'static) -> (Cost, T) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // La CPU se lee dentro del hilo del caso: es la de ESTE hilo, no la del proceso.
        let cpu0 = thread_cpu_time();
        let t0 = Instant::now();
        let out = f();
        let wall = t0.elapsed();
        let cpu = cpu0.zip(thread_cpu_time()).map(|(a, b)| b.saturating_sub(a));
        let _ = tx.send((Cost { cpu, wall }, out));
    });
    match rx.recv_timeout(HARD_CAP) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // `std::io::stderr()` directo: `eprintln!` lo captura el arnés y no se vería.
            let _ = writeln!(
                std::io::stderr(),
                "ipo_perf: {label}: sin terminar tras {HARD_CAP:?} (¿coste cuadrático?). El hilo no se puede matar y seguiría \
                 gastando CPU: se termina el proceso de pruebas."
            );
            std::process::exit(101);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{label}: el análisis hizo panic"),
    }
}

fn within(label: &str, cost: Cost, limit_ms: u64) {
    let took = cost.measured();
    let clock = if cost.cpu.is_some() { "CPU del hilo" } else { "reloj de pared" };
    println!("{label}: {took:?} de {clock} (reloj de pared {:?}; límite {limit_ms} ms)", cost.wall);
    assert!(took < Duration::from_millis(limit_ms), "{label}: {took:?} de {clock} (límite {limit_ms} ms)");
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
    let (cost, pr) = timed("sentences", move || profile_of(prospectus_with("use_of_proceeds", text)));
    within("líneas de un carácter (use_of_proceeds)", cost, 1_500);
    assert_eq!(indicator(&pr, "use_of_proceeds").status, IpoIndicatorStatus::Extracted);
}

/// `duration_candidates()`: cada «month» retrocedía por todas las letras previas.
#[test]
fn a_run_of_month_keeps_the_lockup_scan_linear() {
    let _g = serial();
    let text = "month".repeat(KB400 / 5);
    let (cost, pr) = timed("duration_candidates", move || profile_of(prospectus_with("lock_up", text)));
    within("«month» repetido (lock_up)", cost, 800);
    assert_eq!(indicator(&pr, "lock_up").status, IpoIndicatorStatus::NotExtracted);
}

/// `html_to_lines()`: cada `<style>` pasaba a minúsculas todo el resto del documento.
#[test]
fn empty_style_blocks_keep_html_parsing_linear() {
    let _g = serial();
    let html = "<style></style>".repeat(KB400 / 15);
    let (cost, lines) = timed("html_to_lines", move || html_to_lines(&html));
    within("<style></style> repetido", cost, 800);
    assert!(lines.is_empty());
}

// ---- Otros bucles del mismo estilo --------------------------------------------------------------

/// `strip_footnote()`: copiaba la etiqueta entera por cada nota «(1)» recortada.
#[test]
fn long_footnote_chains_keep_the_financial_rows_linear() {
    let _g = serial();
    let text = format!("Net loss{} | 5\nRevenue | 10\n", "(1)".repeat(KB400 / 3));
    let (cost, pr) = timed("strip_footnote", move || profile_of(prospectus_with("summary_financial_data", text)));
    within("«Net loss(1)(1)…» en una etiqueta", cost, 900);
    // Ninguna fila útil: la etiqueta hostil no es «net loss» y «Revenue» sí se lee.
    assert_eq!(indicator(&pr, "revenue").status, IpoIndicatorStatus::Extracted);
}

/// `Parser::end()`: cada cierre sin apertura recorría la pila (hasta 2 000 marcos).
#[test]
fn stray_closing_tags_do_not_walk_the_whole_open_stack() {
    let _g = serial();
    let html = format!("{}{}", "<i>".repeat(1_990), "</u>".repeat(KB400 / 4));
    let (cost, lines) = timed("end", move || html_to_lines(&html));
    within("cierres huérfanos con la pila llena", cost, 900);
    assert!(lines.is_empty());
}

/// `Headings::next_term()`: recorrido lineal de los títulos por cada candidato.
#[test]
fn thousands_of_bold_headings_keep_section_detection_linear() {
    let _g = serial();
    let html = "<p><b>DILUTION</b></p>".repeat(2 * KB400 / 22);
    let (cost, ex) = timed("next_term", move || prospectus::extract(&html, 0));
    within("«DILUTION» en negrita repetido (800 KB)", cost, 1_500);
    assert!(ex.sections.is_empty(), "sin cuerpo no hay sección");
}

/// `fund_prospectus::extract`: `end_of` recorría todas las cabeceras por cada una.
#[test]
fn thousands_of_fund_headings_keep_extraction_linear() {
    let _g = serial();
    let html = "<p><b>Investment Objective</b></p>".repeat(8 * KB400 / 34);
    let (cost, ex) = timed("fund end_of", move || fund_prospectus::extract(&html, "SPY", 0));
    within("«Investment Objective» repetido (3,2 MB)", cost, 3_000);
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
    let (cost, pr) = timed("float_offered", move || profile_of(prospectus_with("offering", text)));
    within("miles de clases «outstanding after this offering» (3,2 MB)", cost, 3_000);
    assert_eq!(indicator(&pr, "float_offered").status, IpoIndicatorStatus::Extracted);
}

/// Control: texto normal del mismo tamaño que el hostil (marca la línea base del coste lineal).
#[test]
fn normal_text_of_the_same_size_stays_within_the_same_budget() {
    let _g = serial();
    let text = "We intend to use the net proceeds for general corporate purposes. ".repeat(KB400 / 66);
    let (cost, pr) = timed("control", move || profile_of(prospectus_with("use_of_proceeds", text)));
    within("texto normal de 400 KB", cost, 1_500);
    assert_eq!(indicator(&pr, "use_of_proceeds").status, IpoIndicatorStatus::Extracted);
}
