//! Reglas A PRIORI de benchmark, entrada/salida y acierto. Se fijan antes de ver resultados; cambiarlas
//! exige subir `RULE_VERSION`, y esa subida hace que el resolutor vuelva a etiquetar el histórico con la
//! regla nueva (las métricas solo leen filas de la versión vigente: nunca se mezclan versiones).

use chrono::{DateTime, Datelike, NaiveDate, Weekday};
use domain::{AssetClass, AssetId, Candle, Verdict};

/// Versión de las reglas de resolución. Historial y motivos en `docs/notes/learning.md`.
/// - 1: entrada = cierre de la sesión del día UTC de `generated_at` (o la siguiente).
/// - 2 (2026-10-23): entrada = cierre de la última sesión ya abierta cuando el análisis vio los datos.
pub const RULE_VERSION: i64 = 2;

/// Horizontes de resolución, en semanas desde `generated_at`.
pub const HORIZONS_WEEKS: [u32; 3] = [1, 4, 12];

const DAY: i64 = 86_400;
/// Huecos de datos tolerados, en días (un puente largo de EE. UU. son 4).
const TOL: i64 = 5;

/// Banda neutra de «mantener/esperar»: 2 % · √semanas (1 s = 2 %, 4 s = 4 %, 12 s = 6,9 %), del
/// orden de medio σ del exceso de rentabilidad de una acción grande frente a SPY.
pub fn neutral_band(horizon_weeks: u32) -> f64 {
    0.02 * f64::from(horizon_weeks).sqrt()
}

/// ¿Acertó el veredicto dado el exceso de rentabilidad (simple, no anualizado)?
/// comprar/acumular: exceso > 0; reducir/evitar: exceso < 0; mantener/esperar: |exceso| < banda.
pub fn is_hit(verdict: Verdict, excess: f64, horizon_weeks: u32) -> bool {
    match verdict {
        Verdict::Buy | Verdict::Accumulate => excess > 0.0,
        Verdict::Reduce | Verdict::Avoid => excess < 0.0,
        Verdict::Hold | Verdict::Wait => excess.abs() < neutral_band(horizon_weeks),
    }
}

pub fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Buy => "buy",
        Verdict::Accumulate => "accumulate",
        Verdict::Hold => "hold",
        Verdict::Wait => "wait",
        Verdict::Reduce => "reduce",
        Verdict::Avoid => "avoid",
    }
}

pub fn parse_verdict(s: &str) -> Option<Verdict> {
    Some(match s {
        "buy" => Verdict::Buy,
        "accumulate" => Verdict::Accumulate,
        "hold" => Verdict::Hold,
        "wait" => Verdict::Wait,
        "reduce" => Verdict::Reduce,
        "avoid" => Verdict::Avoid,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchKind {
    /// Exceso = rentabilidad del activo − rentabilidad del índice.
    Relative,
    /// El activo es (o replica) su propio índice: exceso = rentabilidad absoluta (índice = efectivo).
    Absolute,
}

impl BenchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relative => "relative",
            Self::Absolute => "absolute",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Benchmark {
    /// `None` con `Absolute`.
    pub id: Option<AssetId>,
    pub kind: BenchKind,
}

/// Sufijos de bolsa de Yahoo que indican un valor no estadounidense.
const NON_US_SUFFIXES: &[&str] = &[
    "MC", "L", "T", "PA", "DE", "MI", "AS", "TO", "V", "HK", "AX", "SS", "SZ", "SW", "ST", "OL",
    "CO", "HE", "VI", "BR", "LS", "IR", "KS", "KQ", "TW", "SI", "NZ", "SA", "MX", "JK", "NS", "BO",
    "TA", "F", "SG",
];
const US_INDICES: &[&str] = &["^GSPC", "^IXIC", "^DJI", "^RUT", "^NDX"];

/// Regla de benchmark:
/// - acción/ETF/fondo de EE. UU. (símbolo sin sufijo de bolsa extranjera): SPY (relativo);
/// - SPY y los índices de EE. UU.: absoluto (son su propio índice);
/// - cripto: BTC (relativo); BTC: absoluto;
/// - el resto (divisas, materias primas, bonos, futuros, opciones, valores extranjeros): sin
///   benchmark definido → no se resuelve (se anota, no se inventa).
pub fn benchmark_for(id: &AssetId) -> Option<Benchmark> {
    let sym = id.symbol.to_ascii_uppercase();
    let spy = AssetId::new(AssetClass::Etf, "SPY");
    match id.class {
        AssetClass::Stock | AssetClass::Etf | AssetClass::Fund => {
            if let Some((_, suffix)) = sym.rsplit_once('.')
                && NON_US_SUFFIXES.contains(&suffix)
            {
                return None;
            }
            if sym == "SPY" {
                return Some(Benchmark {
                    id: None,
                    kind: BenchKind::Absolute,
                });
            }
            Some(Benchmark {
                id: Some(spy),
                kind: BenchKind::Relative,
            })
        }
        AssetClass::Index if US_INDICES.contains(&sym.as_str()) => Some(Benchmark {
            id: None,
            kind: BenchKind::Absolute,
        }),
        AssetClass::Crypto => {
            if sym == "BTC" {
                Some(Benchmark {
                    id: None,
                    kind: BenchKind::Absolute,
                })
            } else {
                Some(Benchmark {
                    id: Some(AssetId::new(AssetClass::Crypto, "BTC")),
                    kind: BenchKind::Relative,
                })
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Cálculo de la rentabilidad
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub entry_ts: i64,
    pub exit_ts: i64,
    pub entry_price: f64,
    pub exit_price: f64,
    pub bench_entry_price: Option<f64>,
    pub bench_exit_price: Option<f64>,
    pub asset_return: f64,
    pub bench_return: f64,
    pub excess: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Computation {
    Resolved(Resolved),
    /// Todavía no hay cierre completo en la fecha objetivo.
    NotYet,
    /// Los datos no sirven (hueco, historia que no llega…): se reintenta y, tras varios intentos, se descarta.
    NoData(String),
}

fn day(t: i64) -> i64 {
    t.div_euclid(DAY)
}

/// Calendario de operación del activo: decide qué sesión estaba ya abierta cuando el análisis vio los datos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trading {
    /// Bolsa de EE. UU.: sesión regular de 9:30 a 16:00 hora de Nueva York. Solo hay vela los días con
    /// sesión, así que los fines de semana y festivos se saltan solos (no hace falta calendario).
    UsSession,
    /// 24/7 (cripto): cada día UTC es una sesión que abre a las 00:00 UTC.
    Continuous,
}

impl Trading {
    pub fn of(class: AssetClass) -> Self {
        if class == AssetClass::Crypto {
            Self::Continuous
        } else {
            Self::UsSession
        }
    }
}

fn nth_sunday(year: i32, month: u32, n: u8) -> Option<NaiveDate> {
    NaiveDate::from_weekday_of_month_opt(year, month, Weekday::Sun, n)
}

/// ¿Rige el horario de verano de EE. UU. (EDT, UTC-4) el día UTC número `d`? Del 2.º domingo de marzo al
/// 1.er domingo de noviembre (reglas vigentes desde 2007). Vale para la apertura (9:30 hora de Nueva
/// York): el cambio se hace a las 2:00 de un domingo, sin sesión.
fn us_dst_on(d: i64) -> bool {
    let Some(date) = d
        .checked_mul(DAY)
        .and_then(|s| DateTime::from_timestamp(s, 0))
        .map(|t| t.date_naive())
    else {
        return false;
    };
    match (
        nth_sunday(date.year(), 3, 2),
        nth_sunday(date.year(), 11, 1),
    ) {
        (Some(start), Some(end)) => date >= start && date < end,
        _ => false,
    }
}

/// Instante UTC (segundos) en que abre la sesión del día UTC número `d`.
fn session_open_s(trading: Trading, d: i64) -> i64 {
    let start = d.saturating_mul(DAY);
    match trading {
        Trading::Continuous => start,
        // 9:30 EDT = 13:30 UTC; 9:30 EST = 14:30 UTC.
        Trading::UsSession if us_dst_on(d) => start.saturating_add(13 * 3600 + 1800),
        Trading::UsSession => start.saturating_add(14 * 3600 + 1800),
    }
}

/// Vela de entrada (regla v2): la de la última sesión que ya había abierto en `seen_s` (UTC, segundos),
/// como mucho `TOL` días antes. Su cierre es el último precio que el análisis pudo ver si el mercado
/// estaba cerrado (antes de la apertura, fin de semana, festivo o tras el cierre) y, si estaba abierto, el
/// primer cierre posterior: el cierre anterior contendría movimiento que el análisis ya había visto.
fn entry_candle(c: &[Candle], trading: Trading, seen_s: i64) -> Option<&Candle> {
    let i = c.partition_point(|x| session_open_s(trading, day(x.t)) <= seen_s);
    let last = c.get(i.checked_sub(1)?)?;
    (day(seen_s) - day(last.t) <= TOL).then_some(last)
}

/// Primera vela con día UTC ≥ `d`, como máximo `tol` días después.
fn first_from(c: &[Candle], d: i64, tol: i64) -> Option<&Candle> {
    let i = c.partition_point(|x| day(x.t) < d);
    c.get(i).filter(|x| day(x.t) - d <= tol)
}

/// Rentabilidad cierre-a-cierre con la regla vigente (v2). `seen_ms` es el instante en que el análisis
/// vio los datos (`generated_at` menos lo que tardó en generarse). Entrada = cierre de la última sesión
/// ya abierta en ese instante ([`entry_candle`]); salida = primer cierre ≥ 7·semanas días después del día
/// de la entrada. La vela de salida debe estar completa (`t + 1 día <= now`). Para el benchmark se
/// usan los mismos días del activo. `candles` debe venir en orden creciente.
pub fn compute(
    asset: &[Candle],
    bench: Option<&[Candle]>,
    seen_ms: i64,
    trading: Trading,
    horizon_weeks: u32,
    now_s: i64,
) -> Computation {
    let Some(entry) = entry_candle(asset, trading, seen_ms.div_euclid(1000)) else {
        return Computation::NoData("sin cierre del activo en la fecha del análisis".into());
    };
    measure(asset, bench, entry, horizon_weeks, now_s)
}

/// Mismo cálculo con otra entrada: el primer cierre con día UTC ≥ `first_day` (hasta `TOL` días después).
/// Es la entrada de la regla v1 y la que usa el backtest («la señal usa el cierre de ese día, así que se
/// entra en el siguiente»).
pub fn compute_from_day(
    asset: &[Candle],
    bench: Option<&[Candle]>,
    first_day: i64,
    horizon_weeks: u32,
    now_s: i64,
) -> Computation {
    let Some(entry) = first_from(asset, first_day, TOL) else {
        return Computation::NoData("sin cierre del activo en la fecha del análisis".into());
    };
    measure(asset, bench, entry, horizon_weeks, now_s)
}

/// Salida, benchmark y rentabilidades a partir de la vela de entrada.
fn measure(
    asset: &[Candle],
    bench: Option<&[Candle]>,
    entry: &Candle,
    horizon_weeks: u32,
    now_s: i64,
) -> Computation {
    let target = day(entry.t) + 7 * i64::from(horizon_weeks);
    let Some(exit) = first_from(asset, target, TOL) else {
        let last = asset.last().map_or(0, |c| day(c.t));
        return if last < target && now_s < (target + TOL + 1) * DAY {
            Computation::NotYet
        } else {
            Computation::NoData("sin cierre del activo en la fecha objetivo".into())
        };
    };
    if exit.t + DAY > now_s {
        return Computation::NotYet;
    }
    if entry.c <= 0.0 || exit.c <= 0.0 || !entry.c.is_finite() || !exit.c.is_finite() {
        return Computation::NoData("precio no válido".into());
    }
    let asset_return = exit.c / entry.c - 1.0;
    let (mut be, mut bx, mut bench_return) = (None, None, 0.0);
    if let Some(b) = bench {
        let (Some(b0), Some(b1)) = (
            first_from(b, day(entry.t), 4),
            first_from(b, day(exit.t), 4),
        ) else {
            return Computation::NoData("sin cierre del benchmark en las fechas".into());
        };
        if b0.c <= 0.0 || b1.c <= 0.0 {
            return Computation::NoData("precio del benchmark no válido".into());
        }
        bench_return = b1.c / b0.c - 1.0;
        be = Some(b0.c);
        bx = Some(b1.c);
    }
    Computation::Resolved(Resolved {
        entry_ts: entry.t,
        exit_ts: exit.t,
        entry_price: entry.c,
        exit_price: exit.c,
        bench_entry_price: be,
        bench_exit_price: bx,
        asset_return,
        bench_return,
        excess: asset_return - bench_return,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn candles(start_day: i64, closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Candle {
                t: (start_day + i as i64) * DAY + 13 * 3600,
                o: c,
                h: c,
                l: c,
                c,
                v: None,
            })
            .collect()
    }

    #[test]
    fn hit_rules() {
        assert!(is_hit(Verdict::Buy, 0.01, 4));
        assert!(!is_hit(Verdict::Accumulate, -0.01, 4));
        assert!(is_hit(Verdict::Avoid, -0.001, 1));
        assert!(!is_hit(Verdict::Reduce, 0.0, 1));
        assert!(is_hit(Verdict::Hold, 0.039, 4));
        assert!(!is_hit(Verdict::Wait, 0.041, 4));
        assert!(is_hit(Verdict::Wait, -0.039, 4));
        assert!((neutral_band(4) - 0.04).abs() < 1e-12);
    }

    #[test]
    fn benchmark_rules() {
        let b = |c, s: &str| benchmark_for(&AssetId::new(c, s));
        assert_eq!(
            b(AssetClass::Stock, "AAPL").unwrap().id,
            Some(AssetId::new(AssetClass::Etf, "SPY"))
        );
        assert_eq!(
            b(AssetClass::Stock, "BRK.B").unwrap().kind,
            BenchKind::Relative
        );
        assert!(b(AssetClass::Stock, "SAN.MC").is_none());
        assert_eq!(b(AssetClass::Etf, "SPY").unwrap().kind, BenchKind::Absolute);
        assert_eq!(
            b(AssetClass::Crypto, "ETH").unwrap().id,
            Some(AssetId::new(AssetClass::Crypto, "BTC"))
        );
        assert_eq!(
            b(AssetClass::Crypto, "BTC").unwrap().kind,
            BenchKind::Absolute
        );
        assert_eq!(
            b(AssetClass::Index, "^GSPC").unwrap().kind,
            BenchKind::Absolute
        );
        assert!(b(AssetClass::Index, "^N225").is_none());
        assert!(b(AssetClass::Forex, "EURUSD=X").is_none());
        assert!(b(AssetClass::Commodity, "GC=F").is_none());
    }

    // --- Cálculo con velas sintéticas, sin calendario: `Continuous`, un día UTC = una sesión ---

    #[test]
    fn computes_excess_on_the_same_days() {
        let a = candles(100, &(0..40).map(|i| 100.0 + i as f64).collect::<Vec<_>>());
        let b = candles(100, &vec![50.0; 40]);
        let t0 = (100 * DAY + 10 * 3600) * 1000;
        let now = 200 * DAY;
        match compute(&a, Some(&b), t0, Trading::Continuous, 4, now) {
            Computation::Resolved(r) => {
                assert!((r.asset_return - 0.28).abs() < 1e-12); // 128/100 - 1
                assert_eq!(r.bench_return, 0.0);
                assert_eq!(day(r.exit_ts) - day(r.entry_ts), 28);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn not_yet_when_the_exit_candle_is_open_or_missing() {
        let a = candles(100, &vec![10.0; 29]); // días 100..=128
        let t0 = (100 * DAY) * 1000;
        let c = Trading::Continuous;
        // Vela de salida (día 128) sin completar a las 20:00 de ese día.
        assert_eq!(
            compute(&a, None, t0, c, 4, 128 * DAY + 20 * 3600),
            Computation::NotYet
        );
        assert!(matches!(
            compute(&a, None, t0, c, 4, 129 * DAY + 14 * 3600),
            Computation::Resolved(_)
        ));
        // Sin vela en la fecha objetivo y aún dentro de la tolerancia.
        assert_eq!(
            compute(&a[..20], None, t0, c, 4, 129 * DAY),
            Computation::NotYet
        );
        // Mucho después y sin datos: no hay precio.
        assert!(matches!(
            compute(&a[..20], None, t0, c, 4, 200 * DAY),
            Computation::NoData(_)
        ));
    }

    #[test]
    fn compute_from_day_keeps_the_v1_entry_used_by_the_backtest() {
        // Sesiones solo en días pares: sin sesión el día pedido, se entra en la siguiente.
        let a: Vec<Candle> = (0..30)
            .map(|i| Candle {
                t: (100 + 2 * i) * DAY,
                o: 1.0,
                h: 1.0,
                l: 1.0,
                c: 1.0 + i as f64,
                v: None,
            })
            .collect();
        match compute_from_day(&a, None, 101, 1, 300 * DAY) {
            Computation::Resolved(r) => assert_eq!(day(r.entry_ts), 102),
            other => panic!("{other:?}"),
        }
        // Y con la regla v2 el mismo instante entra en la sesión anterior (la última ya abierta).
        match compute(
            &a,
            None,
            101 * DAY * 1000,
            Trading::Continuous,
            1,
            300 * DAY,
        ) {
            Computation::Resolved(r) => assert_eq!(day(r.entry_ts), 100),
            other => panic!("{other:?}"),
        }
    }

    // --- Regla v2 con fechas concretas (octubre-noviembre de 2026) ---
    // 2026-10-12 es lunes. EE. UU. está en horario de verano (EDT) hasta el domingo 1-nov-2026: abre a las
    // 13:30 UTC y cierra a las 20:00 UTC; desde el 2-nov (EST), abre a las 14:30 UTC y cierra a las 21:00.

    fn d(y: i32, m: u32, dd: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, dd, 0, 0, 0)
            .unwrap()
            .timestamp()
            .div_euclid(DAY)
    }

    /// Instante en milisegundos, como `generated_at`.
    fn at(y: i32, m: u32, dd: u32, h: u32, mi: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, dd, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    fn is_weekday(day_no: i64) -> bool {
        // 1970-01-01 fue jueves: (día + 3) mod 7 = 0 lunes … 4 viernes, 5 sábado, 6 domingo.
        !matches!((day_no + 3).rem_euclid(7), 5 | 6)
    }

    /// Velas de bolsa (solo días laborables, sin los festivos dados) con `t` = apertura de la sesión,
    /// como Yahoo, y cierre = nº de día desde 2026-01-01 + 100: el precio de entrada dice qué sesión se eligió.
    fn us_series(from: i64, to: i64, holidays: &[i64]) -> Vec<Candle> {
        let base = d(2026, 1, 1);
        (from..=to)
            .filter(|x| is_weekday(*x) && !holidays.contains(x))
            .map(|x| {
                let c = 100.0 + (x - base) as f64;
                Candle {
                    t: session_open_s(Trading::UsSession, x),
                    o: c,
                    h: c,
                    l: c,
                    c,
                    v: None,
                }
            })
            .collect()
    }

    /// Febrero-diciembre de 2026 sin Labor Day (7-sep) ni Acción de Gracias (26-nov).
    fn stock_series() -> Vec<Candle> {
        us_series(
            d(2026, 2, 1),
            d(2026, 12, 31),
            &[d(2026, 9, 7), d(2026, 11, 26)],
        )
    }

    /// Día de la sesión de entrada que elige la regla v2 para un análisis visto en `seen_ms`.
    fn entry_day(c: &[Candle], trading: Trading, seen_ms: i64) -> i64 {
        match compute(c, None, seen_ms, trading, 1, d(2027, 6, 1) * DAY) {
            Computation::Resolved(r) => day(r.entry_ts),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn analysis_before_the_open_enters_at_the_previous_close() {
        // Martes 13-oct-2026 a las 07:15 UTC (09:15 en Madrid): EE. UU. abre a las 13:30 UTC. Lo que vio
        // el análisis es el cierre del lunes 12, y de ahí se mide.
        let a = stock_series();
        let seen = at(2026, 10, 13, 7, 15);
        assert_eq!(entry_day(&a, Trading::UsSession, seen), d(2026, 10, 12));
        match compute(&a, None, seen, Trading::UsSession, 1, d(2027, 6, 1) * DAY) {
            Computation::Resolved(r) => {
                assert_eq!(
                    r.entry_price,
                    100.0 + (d(2026, 10, 12) - d(2026, 1, 1)) as f64
                );
                // 1 semana desde el cierre del lunes 12: cierre del lunes 19.
                assert_eq!(day(r.exit_ts), d(2026, 10, 19));
            }
            other => panic!("{other:?}"),
        }
        // La regla v1 (cierre del día UTC del análisis) habría entrado en el cierre del martes 13.
        match compute_from_day(&a, None, day(seen / 1000), 1, d(2027, 6, 1) * DAY) {
            Computation::Resolved(r) => assert_eq!(day(r.entry_ts), d(2026, 10, 13)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn weekend_analysis_enters_at_the_friday_close() {
        let a = stock_series();
        let fri = d(2026, 10, 16);
        for seen in [
            at(2026, 10, 17, 10, 0),  // sábado
            at(2026, 10, 18, 23, 59), // domingo
            at(2026, 10, 19, 13, 29), // lunes, un minuto antes de abrir
        ] {
            assert_eq!(entry_day(&a, Trading::UsSession, seen), fri, "{seen}");
        }
    }

    #[test]
    fn analysis_with_the_market_open_enters_at_that_session_close() {
        // Miércoles 14-oct-2026 a las 15:00 UTC (11:00 en Nueva York): la sesión ya abrió. El análisis vio
        // un precio intradía; el cierre del martes contendría el movimiento de la mañana, que el análisis
        // ya conocía, así que se mide desde el cierre de esa misma sesión.
        let a = stock_series();
        let wed = d(2026, 10, 14);
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 14, 15, 0)),
            wed
        );
        // Frontera exacta: a las 13:29 aún no ha abierto (cierre del martes); a las 13:30 sí.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 14, 13, 29)),
            d(2026, 10, 13)
        );
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 14, 13, 30)),
            wed
        );
        // Tras el cierre (20:00 UTC) el precio que vio es el de cierre de esa sesión.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 14, 21, 30)),
            wed
        );
    }

    #[test]
    fn the_open_follows_us_daylight_saving_time() {
        let a = stock_series();
        // Miércoles 4-nov-2026, ya en horario de invierno (EST): abre a las 14:30 UTC, no a las 13:30.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 11, 4, 14, 0)),
            d(2026, 11, 3)
        );
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 11, 4, 14, 30)),
            d(2026, 11, 4)
        );
        // La misma hora UTC en octubre (EDT): ya hay sesión.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 14, 14, 0)),
            d(2026, 10, 14)
        );
        // Lunes 9-mar-2026, primer día con EDT tras el cambio del domingo 8: abre a las 13:30 UTC.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 3, 9, 13, 45)),
            d(2026, 3, 9)
        );
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 3, 6, 14, 15)),
            d(2026, 3, 5)
        ); // vie, aún EST
    }

    #[test]
    fn session_open_matches_the_new_york_time_zone_database() {
        // Verdad de referencia obtenida con `TZ=America/New_York date -d "<día> 09:30"`: segundos del día UTC.
        const EDT: i64 = 48_600; // 13:30
        const EST: i64 = 52_200; // 14:30
        for (y, m, dd, tod) in [
            (2024, 3, 8, EST),
            (2024, 3, 11, EDT),
            (2024, 11, 1, EDT),
            (2024, 11, 4, EST),
            (2025, 3, 7, EST),
            (2025, 3, 10, EDT),
            (2025, 10, 31, EDT),
            (2025, 11, 3, EST),
            (2026, 3, 6, EST),
            (2026, 3, 9, EDT),
            (2026, 10, 14, EDT),
            (2026, 10, 30, EDT),
            (2026, 11, 2, EST),
            (2026, 11, 4, EST),
            (2030, 3, 8, EST),
            (2030, 3, 11, EDT),
            (2030, 11, 1, EDT),
            (2030, 11, 4, EST),
        ] {
            let day_no = d(y, m, dd);
            assert_eq!(
                session_open_s(Trading::UsSession, day_no),
                day_no * DAY + tod,
                "{y}-{m}-{dd}"
            );
        }
        // Cripto: la sesión abre a las 00:00 UTC.
        assert_eq!(
            session_open_s(Trading::Continuous, d(2026, 10, 14)),
            d(2026, 10, 14) * DAY
        );
    }

    #[test]
    fn holidays_need_no_calendar_because_there_is_no_candle() {
        let a = stock_series();
        // Lunes 7-sep-2026 (Labor Day) a las 15:00 UTC: no hubo sesión; lo último visto es el viernes 4.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 9, 7, 15, 0)),
            d(2026, 9, 4)
        );
        // Jueves 26-nov-2026 (Acción de Gracias): el cierre del miércoles 25.
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 11, 26, 15, 0)),
            d(2026, 11, 25)
        );
    }

    #[test]
    fn crypto_enters_at_the_close_of_the_utc_day_of_the_analysis() {
        // Cripto opera 24/7: con velas diarias el precio del momento no existe. El cierre del día UTC del
        // análisis es posterior (no contiene movimiento ya visto); el del día anterior sí lo contendría.
        let daily: Vec<Candle> = candles(
            d(2026, 9, 1),
            &(0..120).map(|i| 100.0 + i as f64).collect::<Vec<_>>(),
        );
        let tue = d(2026, 10, 13);
        for seen in [
            at(2026, 10, 13, 0, 0),
            at(2026, 10, 13, 7, 15), // el pase de la lista de seguimiento
            at(2026, 10, 13, 23, 59),
        ] {
            assert_eq!(entry_day(&daily, Trading::Continuous, seen), tue, "{seen}");
        }
        // Sábado: cripto sí tiene sesión.
        assert_eq!(
            entry_day(&daily, Trading::Continuous, at(2026, 10, 17, 10, 0)),
            d(2026, 10, 17)
        );
        // El mismo instante para una acción: el cierre del lunes, no el del martes.
        let a = stock_series();
        assert_eq!(
            entry_day(&a, Trading::UsSession, at(2026, 10, 13, 7, 15)),
            d(2026, 10, 12)
        );
        assert_eq!(Trading::of(AssetClass::Crypto), Trading::Continuous);
        assert_eq!(Trading::of(AssetClass::Stock), Trading::UsSession);
        assert_eq!(Trading::of(AssetClass::Etf), Trading::UsSession);
    }

    #[test]
    fn a_data_gap_longer_than_the_tolerance_is_no_data() {
        let a = us_series(d(2026, 9, 1), d(2026, 10, 2), &[]); // acaba el viernes 2-oct
        let end = d(2026, 10, 2) * DAY;
        // Martes 6 a las 07:15 UTC: 4 días después de la última vela, dentro de la tolerancia.
        assert!(entry_candle(&a, Trading::UsSession, at(2026, 10, 6, 7, 15) / 1000).is_some());
        // Viernes 9: 7 días de hueco.
        assert!(entry_candle(&a, Trading::UsSession, at(2026, 10, 9, 12, 0) / 1000).is_none());
        assert!(matches!(
            compute(
                &a,
                None,
                at(2026, 10, 9, 12, 0),
                Trading::UsSession,
                1,
                end + 90 * DAY
            ),
            Computation::NoData(_)
        ));
        // Antes de la primera vela: sin entrada.
        assert!(entry_candle(&a, Trading::UsSession, at(2026, 8, 20, 12, 0) / 1000).is_none());
        // Sin velas.
        assert!(entry_candle(&[], Trading::UsSession, 0).is_none());
    }

    #[test]
    fn watchlist_passes_enter_at_the_close_of_their_trading_day() {
        // Coherencia con `local_date` de `watchlist_results`: la pasada del día de bolsa D se hace entre
        // el cierre de D + 30 min y la apertura siguiente (D+1, o el lunes si D es viernes). En todo ese
        // intervalo la regla general da D, sin leer `local_date`. Incluye los cambios de hora de EE. UU.
        // (8-mar y 1-nov de 2026), en los que la ventana pasa de 21:00-14:30 UTC a 20:00-13:30 y al revés.
        let a = stock_series();
        let close = |day_no: i64| session_open_s(Trading::UsSession, day_no) + 6 * 3600 + 1800;
        let next_open = |day_no: i64| {
            let mut n = day_no + 1;
            while !is_weekday(n) {
                n += 1;
            }
            session_open_s(Trading::UsSession, n)
        };
        let days = [
            (2026, 3, 6), // viernes EST -> lunes EDT (el domingo 8 cambia la hora)
            (2026, 3, 9),
            (2026, 10, 13), // martes
            (2026, 10, 16), // viernes -> lunes
            (2026, 10, 30), // viernes EDT -> lunes EST (el domingo 1-nov cambia la hora)
            (2026, 11, 3),
            (2026, 11, 25), // víspera de Acción de Gracias: la ventana acaba a las 9:30 del festivo
        ];
        for (y, m, dd) in days {
            let day_no = d(y, m, dd);
            let (start, end) = (close(day_no) + 30 * 60, next_open(day_no));
            for s in [start, start + 3600, (start + end) / 2, end - 1] {
                assert_eq!(
                    entry_day(&a, Trading::UsSession, s * 1000),
                    day_no,
                    "pasada del {y}-{m}-{dd} hecha en {s}"
                );
            }
        }
    }
}
