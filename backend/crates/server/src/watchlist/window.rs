//! Ventana de la pasada diaria: el día de bolsa D se analiza con el cierre de D, en cualquier momento
//! entre el cierre de EE. UU. de D y la apertura siguiente.
//!
//! - **Cierre**: 16:00 hora de Nueva York del día D. **Apertura siguiente**: 9:30 de Nueva York del
//!   siguiente día laborable (D+1, o el lunes si D es viernes). Se calcula en hora de Nueva York y no
//!   como «22:00 / 15:30 de Madrid»: los cambios de hora de EE. UU. (2.º domingo de marzo, 1.er domingo
//!   de noviembre) y de Europa (últimos domingos de marzo y octubre) no coinciden, y en esas semanas la
//!   diferencia es de 5 h y no de 6: cierre a las 21:00 y apertura a las 14:30 en Madrid.
//! - **Margen**: la pasada no empieza hasta [`CLOSE_MARGIN`] después del cierre (la vela diaria de los
//!   proveedores tarda en consolidarse).
//! - **Fines de semana**: la pasada del viernes vale hasta la apertura del lunes. **Festivos de EE. UU.:
//!   no hay calendario**; un festivo se trata como un día laborable más (la ventana de la víspera acaba
//!   a las 9:30 del festivo y la «pasada» del festivo analiza el último cierre real, que se repite).
//!   Las sesiones cortas (13:00) cierran antes de las 16:00, así que el margen es conservador.
//! - **Cripto** opera 24/7: su precio es el del momento de la pasada (que puede ser horas después del
//!   cierre de la bolsa), no el de las 16:00 de Nueva York.
//!
//! Todo son instantes UTC: la zona del usuario solo interviene en la hora preferida (`WATCHLIST_TIME`).

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveTime, Offset, TimeDelta, TimeZone, Utc, Weekday};

use super::Clock;

/// Cuánto después del cierre puede empezar la pasada.
pub const CLOSE_MARGIN: TimeDelta = TimeDelta::minutes(30);

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or(NaiveTime::MIN)
}

fn nth_sunday(year: i32, month: u32, n: u8) -> Option<NaiveDate> {
    NaiveDate::from_weekday_of_month_opt(year, month, Weekday::Sun, n)
}

fn eastern(dst: bool) -> FixedOffset {
    FixedOffset::west_opt(if dst { 4 } else { 5 } * 3600).unwrap_or_else(|| Utc.fix())
}

/// ¿Rige el horario de verano de EE. UU. (EDT, UTC-4) en esa fecha de Nueva York? Del 2.º domingo de
/// marzo al 1.er domingo de noviembre (reglas vigentes desde 2007). Vale para las horas de sesión
/// (9:30 y 16:00): ninguna cae en las 2:00 que se saltan o se repiten.
fn us_dst_on(date: NaiveDate) -> bool {
    let year = date.year();
    match (nth_sunday(year, 3, 2), nth_sunday(year, 11, 1)) {
        (Some(start), Some(end)) => date >= start && date < end,
        _ => false,
    }
}

/// Desfase de Nueva York en un instante UTC (para convertir «ahora» a su fecha y hora de allí).
fn us_offset_at(t: DateTime<Utc>) -> FixedOffset {
    let year = t.year();
    let at = |d: NaiveDate, hour: u32| d.and_hms_opt(hour, 0, 0).map(|l| Utc.from_utc_datetime(&l));
    // Empieza a las 2:00 EST (07:00 UTC) y acaba a las 2:00 EDT (06:00 UTC).
    let dst = match (nth_sunday(year, 3, 2).and_then(|d| at(d, 7)), nth_sunday(year, 11, 1).and_then(|d| at(d, 6))) {
        (Some(start), Some(end)) => t >= start && t < end,
        _ => false,
    };
    eastern(dst)
}

/// Instante UTC de una hora local de Nueva York.
fn et_instant(date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let local = date.and_time(time);
    eastern(us_dst_on(date)).from_local_datetime(&local).single().map_or_else(|| Utc.from_utc_datetime(&local), |d| d.with_timezone(&Utc))
}

/// Apertura de la sesión regular (9:30 de Nueva York) del día `date`.
pub fn session_open(date: NaiveDate) -> DateTime<Utc> {
    et_instant(date, hm(9, 30))
}

/// Cierre de la sesión regular (16:00 de Nueva York) del día `date`.
pub fn session_close(date: NaiveDate) -> DateTime<Utc> {
    et_instant(date, hm(16, 0))
}

fn is_weekday(d: NaiveDate) -> bool {
    !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Siguiente día laborable (sin calendario de festivos).
pub fn next_trading_day(d: NaiveDate) -> NaiveDate {
    let mut n = d;
    for _ in 0..7 {
        n = n.succ_opt().unwrap_or(n);
        if is_weekday(n) {
            break;
        }
    }
    n
}

/// Día laborable anterior (sin calendario de festivos).
pub fn prev_trading_day(d: NaiveDate) -> NaiveDate {
    let mut n = d;
    for _ in 0..7 {
        n = n.pred_opt().unwrap_or(n);
        if is_weekday(n) {
            break;
        }
    }
    n
}

/// Intervalo en el que se puede hacer la pasada del día de bolsa `day`: `[start, end)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Día de bolsa D (el que se guarda en `local_date`).
    pub day: NaiveDate,
    /// Cierre de EE. UU. de D.
    pub start: DateTime<Utc>,
    /// Apertura de EE. UU. siguiente: desde aquí la pasada de D ya no vale.
    pub end: DateTime<Utc>,
}

/// Ventana de un día laborable `day`.
pub fn window_of(day: NaiveDate) -> Window {
    Window { day, start: session_close(day), end: session_open(next_trading_day(day)) }
}

/// Dónde cae un instante respecto a las ventanas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// El mercado está cerrado y la pasada de `Window::day` puede hacerse.
    Window(Window),
    /// El mercado está abierto: `ended` es la ventana que acaba de cerrarse a la apertura de hoy y
    /// `next` la que se abrirá al cierre de hoy.
    MarketOpen { ended: Window, next: Window },
}

pub fn phase_at(t: DateTime<Utc>) -> Phase {
    let et = t.with_timezone(&us_offset_at(t));
    let (date, time) = (et.date_naive(), et.time());
    if !is_weekday(date) {
        return Phase::Window(window_of(prev_trading_day(date)));
    }
    if time >= hm(16, 0) {
        Phase::Window(window_of(date))
    } else if time < hm(9, 30) {
        Phase::Window(window_of(prev_trading_day(date)))
    } else {
        Phase::MarketOpen { ended: window_of(prev_trading_day(date)), next: window_of(date) }
    }
}

/// Instante preferido de la pasada dentro de `w`, dada la hora local `time` (`WATCHLIST_TIME`): la
/// primera vez que el reloj local marca esa hora a partir de `start + CLOSE_MARGIN`. Si cae fuera de la
/// ventana (en plena sesión), se ajusta a `start + CLOSE_MARGIN` y se devuelve `true` para que quien
/// llama avise. La ventana, en hora local, depende de la fecha (22:00-15:30 en Madrid, 21:00-14:30 en
/// las semanas en que los cambios de hora no coinciden), así que un mismo valor puede valer un día y
/// otro no.
pub fn preferred_instant(clock: &dyn Clock, w: &Window, time: NaiveTime) -> (DateTime<Utc>, bool) {
    let earliest = w.start + CLOSE_MARGIN;
    let local = clock.utc_to_local(earliest);
    let mut candidate = local.date().and_time(time);
    if candidate < local {
        candidate += TimeDelta::days(1);
    }
    let at = clock.local_to_utc(candidate).max(earliest);
    if at < w.end { (at, false) } else { (earliest, true) }
}

/// Próxima hora preferida estrictamente posterior a `now` (para `GET /api/watchlist`). No sabe si la
/// pasada de ahora está pendiente: si la preferida de la ventana en curso ya pasó, da la de la siguiente.
pub fn next_due(clock: &dyn Clock, now: DateTime<Utc>, time: NaiveTime) -> DateTime<Utc> {
    let (current, upcoming) = match phase_at(now) {
        Phase::Window(w) => {
            let next = window_of(next_trading_day(w.day));
            (w, next)
        }
        Phase::MarketOpen { next, .. } => {
            let after = window_of(next_trading_day(next.day));
            (next, after)
        }
    };
    let first = preferred_instant(clock, &current, time).0;
    if first > now { first } else { preferred_instant(clock, &upcoming, time).0 }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::watchlist::ManualClock;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn utc(y: i32, m: u32, day: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, day, h, mi, 0).unwrap()
    }

    /// Hora de pared de Madrid para un instante, con el desfase que se indica (+1 invierno, +2 verano).
    fn madrid(t: DateTime<Utc>, offset_h: i32) -> (u32, u32) {
        let l = t.with_timezone(&FixedOffset::east_opt(offset_h * 3600).unwrap());
        (chrono::Timelike::hour(&l), chrono::Timelike::minute(&l))
    }

    #[test]
    fn horario_de_verano_de_ee_uu_segun_la_fecha() {
        // 2026: del 8-mar al 1-nov. 2025: del 9-mar al 2-nov.
        for (date, dst) in [
            (d(2026, 3, 7), false),
            (d(2026, 3, 8), true),
            (d(2026, 7, 1), true),
            (d(2026, 10, 31), true),
            (d(2026, 11, 1), false),
            (d(2026, 12, 25), false),
            (d(2025, 3, 8), false),
            (d(2025, 3, 9), true),
            (d(2025, 11, 1), true),
            (d(2025, 11, 2), false),
        ] {
            assert_eq!(us_dst_on(date), dst, "{date}");
        }
        // Y el desfase de un instante cambia en el instante exacto (2:00 locales).
        assert_eq!(us_offset_at(utc(2026, 3, 8, 6, 59)).local_minus_utc(), -5 * 3600);
        assert_eq!(us_offset_at(utc(2026, 3, 8, 7, 0)).local_minus_utc(), -4 * 3600);
        assert_eq!(us_offset_at(utc(2026, 11, 1, 5, 59)).local_minus_utc(), -4 * 3600);
        assert_eq!(us_offset_at(utc(2026, 11, 1, 6, 0)).local_minus_utc(), -5 * 3600);
    }

    #[test]
    fn cierre_y_apertura_en_hora_de_madrid_incluidas_las_semanas_desfasadas() {
        // (martes D, desfase de Madrid, cierre de D en UTC, apertura de D+1 en UTC, cierre y apertura en Madrid)
        let cases = [
            // Invierno en los dos lados: 22:00 y 15:30.
            (d(2026, 3, 3), 1, (21, 0), (14, 30), (22, 0), (15, 30)),
            // EE. UU. ya en verano (8-mar) y Madrid aún en invierno (hasta el 29-mar): 21:00 y 14:30.
            (d(2026, 3, 10), 1, (20, 0), (13, 30), (21, 0), (14, 30)),
            (d(2026, 3, 24), 1, (20, 0), (13, 30), (21, 0), (14, 30)),
            // Verano en los dos lados: 22:00 y 15:30.
            (d(2026, 7, 14), 2, (20, 0), (13, 30), (22, 0), (15, 30)),
            (d(2026, 10, 20), 2, (20, 0), (13, 30), (22, 0), (15, 30)),
            // Madrid ya en invierno (25-oct) y EE. UU. aún en verano (hasta el 1-nov): 21:00 y 14:30.
            (d(2026, 10, 27), 1, (20, 0), (13, 30), (21, 0), (14, 30)),
            // Invierno en los dos lados otra vez.
            (d(2026, 11, 3), 1, (21, 0), (14, 30), (22, 0), (15, 30)),
        ];
        for (day, off, close_utc, open_utc, close_madrid, open_madrid) in cases {
            let w = window_of(day);
            let next = day.succ_opt().unwrap();
            assert_eq!(w.start, utc(day.year(), day.month(), day.day(), close_utc.0, close_utc.1), "cierre de {day}");
            assert_eq!(w.end, utc(next.year(), next.month(), next.day(), open_utc.0, open_utc.1), "apertura tras {day}");
            assert_eq!(madrid(w.start, off), close_madrid, "cierre de {day} en Madrid");
            assert_eq!(madrid(w.end, off), open_madrid, "apertura tras {day} en Madrid");
        }
    }

    #[test]
    fn la_ventana_del_viernes_llega_hasta_la_apertura_del_lunes_tambien_con_cambio_de_hora() {
        // Viernes 16-oct-2026 -> lunes 19-oct (sin cambios de hora en medio).
        let w = window_of(d(2026, 10, 16));
        assert_eq!((w.start, w.end), (utc(2026, 10, 16, 20, 0), utc(2026, 10, 19, 13, 30)));
        // El fin de semana del 8-mar-2026 EE. UU. adelanta el reloj: cierra en EST y abre en EDT.
        let w = window_of(d(2026, 3, 6));
        assert_eq!((w.start, w.end), (utc(2026, 3, 6, 21, 0), utc(2026, 3, 9, 13, 30)));
        // El del 1-nov-2026 lo retrasa: cierra en EDT y abre en EST.
        let w = window_of(d(2026, 10, 30));
        assert_eq!((w.start, w.end), (utc(2026, 10, 30, 20, 0), utc(2026, 11, 2, 14, 30)));
        assert_eq!(next_trading_day(d(2026, 10, 16)), d(2026, 10, 19));
        assert_eq!(prev_trading_day(d(2026, 10, 19)), d(2026, 10, 16));
        assert_eq!(prev_trading_day(d(2026, 10, 18)), d(2026, 10, 16));
    }

    #[test]
    fn fase_segun_el_instante() {
        let w14 = window_of(d(2026, 10, 14));
        let w15 = window_of(d(2026, 10, 15));
        let w16 = window_of(d(2026, 10, 16));
        // Jueves 15-oct (EDT): abre 13:30 UTC y cierra 20:00 UTC.
        assert_eq!(phase_at(utc(2026, 10, 15, 7, 0)), Phase::Window(w14.clone()), "09:00 de Madrid: pasada de ayer");
        assert_eq!(phase_at(utc(2026, 10, 15, 13, 29)), Phase::Window(w14.clone()));
        assert_eq!(phase_at(utc(2026, 10, 15, 13, 30)), Phase::MarketOpen { ended: w14.clone(), next: w15.clone() });
        assert_eq!(phase_at(utc(2026, 10, 15, 19, 59)), Phase::MarketOpen { ended: w14, next: w15.clone() });
        assert_eq!(phase_at(utc(2026, 10, 15, 20, 0)), Phase::Window(w15.clone()));
        // Fin de semana: sigue la ventana del viernes hasta la apertura del lunes.
        for t in [utc(2026, 10, 16, 20, 0), utc(2026, 10, 17, 12, 0), utc(2026, 10, 18, 23, 59), utc(2026, 10, 19, 13, 29)] {
            assert_eq!(phase_at(t), Phase::Window(w16.clone()), "{t}");
        }
        assert!(matches!(phase_at(utc(2026, 10, 19, 13, 30)), Phase::MarketOpen { ended, .. } if ended == w16));
    }

    #[test]
    fn fase_en_la_semana_en_que_los_cambios_de_hora_no_coinciden() {
        // Miércoles 28-oct-2026: Madrid en invierno (+1) y EE. UU. aún en verano. La apertura es a las
        // 14:30 de Madrid (13:30 UTC): a las 14:45 el mercado ya está abierto aunque «15:30» no haya llegado.
        let w27 = window_of(d(2026, 10, 27));
        assert_eq!(phase_at(utc(2026, 10, 28, 13, 29)), Phase::Window(w27.clone()));
        assert!(matches!(phase_at(utc(2026, 10, 28, 13, 45)), Phase::MarketOpen { ended, .. } if ended == w27));
        // Y cierra a las 21:00 de Madrid: a las 21:10 ya se puede.
        assert!(matches!(phase_at(utc(2026, 10, 28, 19, 59)), Phase::MarketOpen { .. }));
        assert_eq!(phase_at(utc(2026, 10, 28, 20, 10)), Phase::Window(window_of(d(2026, 10, 28))));
    }

    #[test]
    fn hora_preferida_dentro_de_la_ventana_o_ajustada() {
        let hm = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        // Semana normal en Madrid de verano (+2): ventana 22:00-15:30 locales (20:00-13:30 UTC).
        let clock = ManualClock::new(utc(2026, 10, 15, 0, 0), 2 * 3600);
        let w = window_of(d(2026, 10, 15));
        let pref = |t| preferred_instant(clock.as_ref(), &w, t);
        assert_eq!(pref(hm(23, 0)), (utc(2026, 10, 15, 21, 0), false), "23:00 de esa noche");
        assert_eq!(pref(hm(9, 15)), (utc(2026, 10, 16, 7, 15), false), "09:15 de la mañana siguiente");
        assert_eq!(pref(hm(22, 30)), (utc(2026, 10, 15, 20, 30), false), "justo el margen tras el cierre");
        // Fuera de la ventana (sesión abierta): se ajusta al cierre + margen.
        assert_eq!(pref(hm(16, 0)), (utc(2026, 10, 15, 20, 30), true));
        assert_eq!(pref(hm(21, 30)), (utc(2026, 10, 15, 20, 30), true), "antes del margen: la siguiente es en plena sesión");
        assert_eq!(pref(hm(22, 10)), (utc(2026, 10, 15, 20, 30), true), "22:10 cae antes del margen y la siguiente es en sesión");
        // Viernes: la primera vez que marca las 09:15 es el sábado.
        let fri = window_of(d(2026, 10, 16));
        assert_eq!(preferred_instant(clock.as_ref(), &fri, hm(9, 15)), (utc(2026, 10, 17, 7, 15), false));
        // Semana desfasada (Madrid +1, ventana 21:00-14:30 locales): 21:30 vale; en la normal no valía.
        let winter = ManualClock::new(utc(2026, 10, 27, 0, 0), 3600);
        let w = window_of(d(2026, 10, 27));
        assert_eq!(preferred_instant(winter.as_ref(), &w, hm(21, 30)), (utc(2026, 10, 27, 20, 30), false));
        assert_eq!(preferred_instant(winter.as_ref(), &w, hm(14, 45)), (utc(2026, 10, 27, 20, 30), true), "14:45 ya es sesión abierta esa semana");
        assert_eq!(preferred_instant(winter.as_ref(), &w, hm(14, 15)), (utc(2026, 10, 28, 13, 15), false));
    }

    #[test]
    fn next_due_es_estrictamente_posterior() {
        let hm = NaiveTime::from_hms_opt(23, 0, 0).unwrap();
        let clock = ManualClock::new(utc(2026, 10, 15, 0, 0), 2 * 3600);
        // Antes de las 23:00 locales: la de esa noche. Pasada: la de la noche siguiente.
        assert_eq!(next_due(clock.as_ref(), utc(2026, 10, 15, 12, 0), hm), utc(2026, 10, 15, 21, 0));
        assert_eq!(next_due(clock.as_ref(), utc(2026, 10, 15, 21, 0), hm), utc(2026, 10, 16, 21, 0));
        // Viernes por la noche, pasada la preferida: la siguiente es la del lunes (23:00 del lunes).
        assert_eq!(next_due(clock.as_ref(), utc(2026, 10, 16, 21, 30), hm), utc(2026, 10, 19, 21, 0));
        // Sábado: la del lunes.
        assert_eq!(next_due(clock.as_ref(), utc(2026, 10, 17, 12, 0), hm), utc(2026, 10, 19, 21, 0));
    }
}
