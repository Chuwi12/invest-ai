//! Estudio de OPV: reconstrucción del perfil con fecha correcta, resultados, estratificación,
//! bootstrap reproducible (semilla), conglomerados por mes, Holm y matriz phi. Sin red.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use domain::{Candle, IpoEntry, IpoStatus, Prospectus};
use market::{
    ipo_study::*,
    providers::yahoo::DailySeries,
};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn now() -> DateTime<Utc> {
    d(2026, 10, 19).and_hms_opt(0, 0, 0).unwrap().and_utc()
}

fn ts(date: NaiveDate) -> i64 {
    date.and_hms_opt(14, 30, 0).unwrap().and_utc().timestamp()
}

/// Un cierre por día laborable desde `start`, que cambia `step` cada día.
fn series(start: NaiveDate, days: i64, first: f64, step: f64) -> DailySeries {
    let mut candles = Vec::new();
    let (mut day, mut px) = (start, first);
    for _ in 0..days {
        if day.weekday().number_from_monday() < 6 {
            candles.push(Candle { t: ts(day), o: px, h: px * 1.05, l: px * 0.95, c: px, v: Some(1.0) });
            px += step;
        }
        day += chrono::Duration::days(1);
    }
    let adj_close = candles.iter().map(|c| c.c).collect();
    DailySeries { candles, adj_close, splits: vec![] }
}

fn entry(date: NaiveDate, shares: u64, price: f64) -> IpoEntry {
    IpoEntry {
        date,
        symbol: Some("TRXB".into()),
        name: "TRex Bio, Inc.".into(),
        exchange: None,
        status: IpoStatus::Priced,
        price_low: Some(price),
        price_high: Some(price),
        shares: Some(shares),
        total_value: Some(shares as f64 * price),
        spac_likely: false,
        source: "finnhub".into(),
        source_url: None,
        as_of: now(),
    }
}

fn trxb() -> Prospectus {
    serde_json::from_str(include_str!("fixtures/sec_prospectus_trxb_excerpt.json")).unwrap()
}

#[test]
fn prospectus_must_be_definitive_and_filed_around_the_ipo() {
    let mut p = trxb();
    let ipo = d(2026, 10, 9);
    p.filed_on = ipo;
    assert!(prospectus_valid_at(&p, ipo));
    p.filed_on = d(2026, 10, 12); // +3
    assert!(prospectus_valid_at(&p, ipo));
    p.filed_on = d(2026, 10, 13); // +4: posterior
    assert!(!prospectus_valid_at(&p, ipo));
    p.filed_on = d(2026, 9, 29); // −10
    assert!(prospectus_valid_at(&p, ipo));
    p.filed_on = d(2026, 9, 28);
    assert!(!prospectus_valid_at(&p, ipo), "un folleto de una oferta anterior no vale");
    p.filed_on = ipo;
    p.preliminary = true;
    assert!(!prospectus_valid_at(&p, ipo));
}

#[test]
fn profile_is_rebuilt_only_from_the_ipo_prospectus_and_flags_follow_the_thresholds() {
    let ipo = d(2025, 1, 10);
    let mut p = trxb();
    p.filed_on = ipo;
    let stock = series(ipo, 400, 14.0, 0.01);
    let spy = series(ipo - chrono::Duration::days(20), 450, 500.0, 0.2);
    // TRXB: 8.333.334 acciones a 14 USD = 116,7 M (tramo 50–250 M), sin venta secundaria, lock-up 180 d,
    // capital ofrecido 30,4 %, dilución 7,04 USD (50 % del precio), pérdidas.
    let o = build_obs(&entry(ipo, 8_333_334, 14.0), Some(&p), Some(&stock), &spy, now()).unwrap();
    assert_eq!(o.flags[0], Some(false), "V1 venta secundaria > 50 %");
    assert_eq!(o.flags[1], Some(false), "V2 deuda");
    assert_eq!(o.flags[2], Some(false), "V3 voto múltiple");
    assert_eq!(o.flags[4], Some(false), "V5 dilución 50 % < 75 %");
    assert_eq!(o.flags[5], Some(false), "V6 flotante 30 % ≥ 15 %");
    assert_eq!(o.flags[6], Some(true), "V7 pérdidas");
    assert_eq!(o.flags[7], Some(false), "V8 lock-up 180 no es < 180");
    assert_eq!((o.flags[8], o.flags[9]), (Some(false), Some(false)), "116,7 M: ni micro ni grande");
    // Con otro tamaño de oferta cambian solo las banderas de tamaño.
    let big = build_obs(&entry(ipo, 20_000_000, 14.0), Some(&p), Some(&stock), &spy, now()).unwrap();
    assert_eq!((big.flags[8], big.flags[9]), (Some(false), Some(true)));
    let micro = build_obs(&entry(ipo, 2_000_000, 14.0), Some(&p), Some(&stock), &spy, now()).unwrap();
    assert_eq!((micro.flags[8], micro.flags[9]), (Some(true), Some(false)));
    // Folleto fuera de ventana o ausente: exclusión con motivo, no un perfil con datos posteriores.
    let mut late = p.clone();
    late.filed_on = ipo + chrono::Duration::days(30);
    assert_eq!(build_obs(&entry(ipo, 8_333_334, 14.0), Some(&late), Some(&stock), &spy, now()).unwrap_err(), Exclusion::ProspectusOutOfWindow);
    assert_eq!(build_obs(&entry(ipo, 8_333_334, 14.0), None, Some(&stock), &spy, now()).unwrap_err(), Exclusion::NoProspectus);
    assert_eq!(build_obs(&entry(ipo, 8_333_334, 14.0), Some(&p), None, &spy, now()).unwrap_err(), Exclusion::NoYahooSeries);
}

#[test]
fn outcomes_from_placement_and_from_day1_close() {
    let ipo = d(2025, 1, 10); // viernes
    let mut p = trxb();
    p.filed_on = ipo;
    // Acción: abre a 14 (precio de OPV 10) y sube 0,02/día; SPY sube 0,5/día desde 500.
    let stock = series(ipo, 400, 14.0, 0.02);
    let spy = series(ipo - chrono::Duration::days(10), 450, 500.0, 0.5);
    let o = build_obs(&entry(ipo, 8_333_334, 10.0), Some(&p), Some(&stock), &spy, now()).unwrap();
    let perf = market::ipo::compute(&market::ipo::PerfInput {
        symbol: "TRXB",
        ipo_date: ipo,
        offer_price: 10.0,
        offer_price_source: "calendar",
        series: &stock,
        bench: &spy,
        bench_name: "SPY",
        source_url: None,
        now: now(),
    })
    .unwrap();
    let p6 = perf.points.iter().find(|x| x.horizon == "6m").unwrap();
    assert_eq!(o.outcomes.placement[1], p6.excess_pct, "desde la colocación = lo que calcula la app");
    // Desde el cierre del día 1: la acción parte de 14, no de 10, así que rinde mucho menos.
    let since = p6.since_first_close_pct.unwrap();
    assert!(since < p6.return_pct.unwrap() - 30.0);
    let spy_base = spy.candles.iter().zip(&spy.adj_close).find(|(c, _)| c.t == ts(ipo)).unwrap().1;
    let end = p6.price_date.unwrap();
    let spy_end = spy.candles.iter().zip(&spy.adj_close).rfind(|(c, _)| c.t <= ts(end)).unwrap().1;
    let want = since - (spy_end / spy_base - 1.0) * 100.0;
    assert!((o.outcomes.day1[1].unwrap() - want).abs() < 1e-9);
    // 12 meses aún no alcanzados con 400 días de serie desde la OPV... sí se alcanzan (365 < 400): ambos presentes.
    assert!(o.outcomes.placement[2].is_some());
    // Con una serie corta, el horizonte no alcanzado queda en None (nunca se extrapola).
    let short = series(ipo, 120, 14.0, 0.02);
    let o2 = build_obs(&entry(ipo, 8_333_334, 10.0), Some(&p), Some(&short), &spy, now()).unwrap();
    assert!(o2.outcomes.placement[0].is_some() && o2.outcomes.placement[1].is_none() && o2.outcomes.day1[2].is_none());
}

fn row(flag: bool, y: f64, tier: usize, month: i32) -> Row {
    Row { flag, y, tier, month }
}

fn obs_with(flag: bool, y: f64, size: f64, month: u32) -> Obs {
    let mut flags = [None; N_FLAGS];
    flags[0] = Some(flag);
    flags[8] = Some(flag);
    Obs {
        symbol: "X".into(),
        ipo_date: d(2024, month, 10),
        size_usd: Some(size),
        flags,
        outcomes: Outcomes { placement: [Some(y), Some(y), Some(y)], day1: [Some(y), Some(y), Some(y)] },
    }
}

#[test]
fn stratification_removes_pure_size_confounding() {
    // La bandera es más frecuente entre micro-OPV, y las micro rinden peor: sin estratificar
    // parece que la bandera «causa» −20 pp; dentro de cada tramo no hay efecto.
    let mut obs = Vec::new();
    for k in 0..40 {
        let noise = (k % 5) as f64;
        obs.push(obs_with(k % 4 != 0, -20.0 + noise, 20e6, 1 + (k % 12) as u32)); // micro: 75 % con bandera
        obs.push(obs_with(k % 4 == 0, 0.0 + noise, 400e6, 1 + (k % 12) as u32)); // grande: 25 % con bandera
    }
    let c = contrasts(&obs, |o| o.outcomes.placement[1], SEED);
    let v1 = &c[0];
    assert!(v1.stratified);
    let raw = v1.raw_diff.unwrap();
    assert!(raw < -10.0, "sin estratificar, la confusión da un efecto grande: {raw}");
    assert!(v1.diff.unwrap().abs() < 1.5, "estratificado ≈ 0: {:?}", v1.diff);
    assert!(v1.p_holm.unwrap() > 0.05, "{:?}", v1.p_holm);
    // Un tramo con n < 5 en un grupo no cuenta en el estadístico.
    let few = vec![row(true, 5.0, 0, 1), row(false, 1.0, 0, 1), row(true, 7.0, 1, 1), row(true, 7.0, 1, 1), row(true, 7.0, 1, 1), row(true, 7.0, 1, 1), row(true, 7.0, 1, 1), row(false, 1.0, 1, 1), row(false, 1.0, 1, 1), row(false, 1.0, 1, 1), row(false, 1.0, 1, 1), row(false, 1.0, 1, 1)];
    let (diff, n1, n0) = strat_stat_for_tests(&few);
    assert_eq!((n1, n0), (5, 5), "solo el tramo 1 tiene ≥ 5 en ambos grupos");
    assert!((diff - 6.0).abs() < 1e-12);
    // El tamaño mismo (V9) no se estratifica.
    assert!(!c[8].stratified && c[8].n1 + c[8].n0 == 80);
}

/// Estadístico estratificado vía la API pública (`strat_perm_p` exige el mismo cálculo): se
/// reconstruye aquí con las mismas reglas para comprobar el ponderado.
fn strat_stat_for_tests(rows: &[Row]) -> (f64, usize, usize) {
    let (mut num, mut den, mut n1, mut n0) = (0.0, 0.0, 0, 0);
    for t in 0..3 {
        let a: Vec<f64> = rows.iter().filter(|r| r.tier == t && r.flag).map(|r| r.y).collect();
        let b: Vec<f64> = rows.iter().filter(|r| r.tier == t && !r.flag).map(|r| r.y).collect();
        if a.len() >= MIN_STRATUM_GROUP && b.len() >= MIN_STRATUM_GROUP {
            let w = (a.len() + b.len()) as f64;
            num += w * median_diff(&a, &b).unwrap();
            den += w;
            n1 += a.len();
            n0 += b.len();
        }
    }
    (num / den, n1, n0)
}

#[test]
fn stratified_permutation_and_bootstrap_are_reproducible_and_detect_a_real_effect() {
    // Efecto real de +10 pp en los dos tramos.
    let mut rows = Vec::new();
    let mut r = Rng::new(3);
    for t in 0..2 {
        for k in 0..40 {
            let base = (r.next_u64() % 1000) as f64 / 100.0;
            rows.push(row(k % 2 == 0, base + if k % 2 == 0 { 10.0 } else { 0.0 }, t, 1 + (k % 9)));
        }
    }
    let p1 = strat_perm_p(&rows, 5, 2000, 9).unwrap();
    assert_eq!(p1, strat_perm_p(&rows, 5, 2000, 9).unwrap());
    assert!(p1 < 0.01, "{p1}");
    let ci = strat_boot_ci(&rows, 5, 2000, 9).unwrap();
    assert_eq!(ci, strat_boot_ci(&rows, 5, 2000, 9).unwrap());
    assert!(ci.0 > 5.0 && ci.1 < 15.0, "{ci:?}");
    // Por conglomerados de mes: reproducible, más ancho o igual de ancho que el ordinario y con el efecto dentro.
    let cc = cluster_boot_ci(&rows, 5, 2000, 9).unwrap();
    assert_eq!(cc, cluster_boot_ci(&rows, 5, 2000, 9).unwrap());
    assert!(cc.0 < 10.5 && cc.1 > 9.5, "{cc:?}");
    assert!(cluster_boot_ci(&[row(true, 1.0, 0, 1), row(false, 2.0, 0, 1)], 1, 100, 1).is_none(), "menos de 3 meses no admite conglomerados");
}

#[test]
fn clustered_months_widen_the_interval_when_a_hot_month_drives_the_effect() {
    // Todo el «efecto» viene de un mes caliente: el IC por conglomerados lo refleja y deja de excluir 0.
    let mut rows = Vec::new();
    let mut r = Rng::new(5);
    for m in 1..=8 {
        for k in 0..12 {
            let noise = (r.next_u64() % 400) as f64 / 100.0;
            let hot = m == 3;
            let flag = k % 2 == 0;
            rows.push(row(flag, noise + if hot && flag { 40.0 } else { 0.0 }, 0, m));
        }
    }
    let ordinary = strat_boot_ci(&rows, 5, 3000, 2).unwrap();
    let cluster = cluster_boot_ci(&rows, 5, 3000, 2).unwrap();
    assert!(cluster.1 - cluster.0 >= ordinary.1 - ordinary.0, "{ordinary:?} {cluster:?}");
}

#[test]
fn decision_rule_needs_both_intervals_and_holm() {
    let base = Contrast {
        flag: 0,
        stratified: true,
        n1: 30,
        n0: 60,
        diff: Some(-12.0),
        ci: Some((-20.0, -4.0)),
        ci_cluster: Some((-22.0, -3.0)),
        p: Some(0.001),
        p_holm: Some(0.01),
        mde: Some(15.0),
        testable: true,
        raw_n1: 30,
        raw_n0: 60,
        raw_median1: None,
        raw_median0: None,
        raw_diff: None,
        raw_ci: None,
        raw_p: None,
    };
    assert_eq!(decide(&base, &base, &base, &base), (true, true));
    let mut wide = base.clone();
    wide.ci_cluster = Some((-22.0, 3.0));
    assert!(!decide(&wide, &base, &base, &base).0, "el IC por conglomerados incluye 0: no cumple el criterio 2");
    let mut weak = base.clone();
    weak.p_holm = Some(0.2);
    assert!(!decide(&weak, &base, &base, &base).0);
    let mut flip = base.clone();
    flip.diff = Some(5.0);
    assert!(!decide(&base, &flip, &base, &base).0, "signo distinto a 3 meses");
    let mut small = base.clone();
    small.testable = false;
    assert!(decide(&base, &small, &base, &base).0, "un horizonte no contrastable no veta");
    let mut d1 = base.clone();
    d1.ci = Some((-9.0, 2.0));
    assert_eq!(decide(&base, &base, &base, &d1), (true, false), "no aprovechable si desde el cierre del día 1 el IC incluye 0");
}

#[test]
fn phi_matrix_basics() {
    let mk = |a: bool, b: bool| {
        let mut flags = [None; N_FLAGS];
        flags[0] = Some(a);
        flags[1] = Some(b);
        flags[2] = Some(!a);
        Obs { symbol: "X".into(), ipo_date: d(2024, 1, 1), size_usd: None, flags, outcomes: Outcomes::default() }
    };
    let obs: Vec<Obs> = [(true, true), (true, true), (false, false), (false, false), (true, false), (false, true)].iter().map(|&(a, b)| mk(a, b)).collect();
    let (v, n) = phi(&obs, 0, 1);
    assert_eq!(n, 6);
    assert!((v.unwrap() - 1.0 / 3.0).abs() < 1e-12);
    assert!((phi(&obs, 0, 2).0.unwrap() + 1.0).abs() < 1e-12, "una bandera y su negación: phi = −1");
    assert_eq!(phi(&obs, 0, 5).1, 0, "banderas sin datos: n = 0");
    assert_eq!(phi(&obs, 0, 5).0, None);
}

#[test]
fn holm_and_seeded_runs_are_deterministic_end_to_end() {
    let mut obs = Vec::new();
    for k in 0..60 {
        obs.push(obs_with(k % 3 == 0, (k % 7) as f64, if k % 2 == 0 { 20e6 } else { 300e6 }, 1 + (k % 6) as u32));
    }
    let a = contrasts(&obs, |o| o.outcomes.placement[1], SEED);
    let b = contrasts(&obs, |o| o.outcomes.placement[1], SEED);
    assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
    for c in &a {
        if let (Some(p), Some(h)) = (c.p, c.p_holm) {
            assert!(h >= p && h <= 1.0);
        }
    }
}
