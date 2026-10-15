//! Reglas A PRIORI (versión 1) de benchmark, ventana de cálculo y acierto. Se fijan antes de ver
//! resultados; cambiarlas exige subir `RULE_VERSION` (las filas ya etiquetadas no se reescriben).

use domain::{AssetClass, AssetId, Candle, Verdict};

pub const RULE_VERSION: i64 = 1;

/// Horizontes de resolución, en semanas desde `generated_at`.
pub const HORIZONS_WEEKS: [u32; 3] = [1, 4, 12];

const DAY: i64 = 86_400;

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

/// Primera vela con día UTC ≥ `d`, como máximo `tol` días después.
fn first_from(c: &[Candle], d: i64, tol: i64) -> Option<&Candle> {
    let i = c.partition_point(|x| day(x.t) < d);
    c.get(i).filter(|x| day(x.t) - d <= tol)
}

/// Rentabilidad cierre-a-cierre: entrada = cierre de la sesión del día del análisis (o la
/// siguiente si ese día no hay sesión); salida = primer cierre ≥ 7·semanas días después de la
/// entrada. La vela de salida debe estar completa (`t + 1 día <= now`). Para el benchmark se
/// usan los mismos días del activo. `candles` debe venir en orden creciente.
pub fn compute(
    asset: &[Candle],
    bench: Option<&[Candle]>,
    t0_ms: i64,
    horizon_weeks: u32,
    now_s: i64,
) -> Computation {
    const TOL: i64 = 5;
    let d0 = day(t0_ms.div_euclid(1000));
    let Some(entry) = first_from(asset, d0, TOL) else {
        return Computation::NoData("sin cierre del activo en la fecha del análisis".into());
    };
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

    #[test]
    fn computes_excess_on_the_same_days() {
        let a = candles(100, &(0..40).map(|i| 100.0 + i as f64).collect::<Vec<_>>());
        let b = candles(100, &vec![50.0; 40]);
        let t0 = (100 * DAY + 10 * 3600) * 1000;
        let now = 200 * DAY;
        match compute(&a, Some(&b), t0, 4, now) {
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
        // Vela de salida (día 128) sin completar a las 20:00 de ese día.
        assert_eq!(
            compute(&a, None, t0, 4, 128 * DAY + 20 * 3600),
            Computation::NotYet
        );
        assert!(matches!(
            compute(&a, None, t0, 4, 129 * DAY + 14 * 3600),
            Computation::Resolved(_)
        ));
        // Sin vela en la fecha objetivo y aún dentro de la tolerancia.
        assert_eq!(
            compute(&a[..20], None, t0, 4, 129 * DAY),
            Computation::NotYet
        );
        // Mucho después y sin datos: no hay precio.
        assert!(matches!(
            compute(&a[..20], None, t0, 4, 200 * DAY),
            Computation::NoData(_)
        ));
    }

    #[test]
    fn weekend_analysis_enters_on_next_session() {
        // Sesiones solo en días pares.
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
        let t0 = (101 * DAY) * 1000;
        match compute(&a, None, t0, 1, 300 * DAY) {
            Computation::Resolved(r) => assert_eq!(day(r.entry_ts), 102),
            other => panic!("{other:?}"),
        }
    }
}
