//! Escenarios sintéticos sin red: un factor predice y los demás son ruido; muestra pequeña;
//! cambio de régimen. Los números impresos (`--nocapture`) son los de `docs/notes/learning.md`.

use domain::AssetClass;

use crate::{
    stats::Rng,
    store::{FACTOR_NAMES, N_FACTORS},
    weights::{Decision, LearnRow, ONES, WeightState, WeightsConfig, attempt, evaluate_gate},
};

const WEEK_MS: i64 = 7 * 86_400_000;

/// `corr(week)`: correlación entre el factor señal (estandarizado) y el exceso de ese momento.
fn make_rows(
    seed: u64,
    weeks: usize,
    per_week: usize,
    assets: usize,
    signal: usize,
    corr: &dyn Fn(usize) -> f64,
) -> Vec<LearnRow> {
    let mut r = Rng::new(seed);
    let mut rows = Vec::new();
    for w in 0..weeks {
        for k in 0..per_week {
            let mut f = [None; N_FACTORS];
            let mut z_sig = 0.0;
            for (j, slot) in f.iter_mut().enumerate() {
                let z = r.normal();
                if j == signal {
                    z_sig = z;
                }
                *slot = Some((50.0 + 15.0 * z).clamp(0.0, 100.0));
            }
            let rho = corr(w);
            let excess = 0.07 * (rho * z_sig + (1.0 - rho * rho).sqrt() * r.normal());
            let asset = (w * per_week + k) % assets;
            rows.push(LearnRow {
                asset_id: format!("stock:S{asset}"),
                class: AssetClass::Stock,
                t_ms: w as i64 * WEEK_MS + (k as i64 * 3_600_000),
                excess,
                factors: f,
            });
        }
    }
    rows
}

fn run(
    rows: &[LearnRow],
    as_of_week: usize,
    cfg: &WeightsConfig,
) -> (Option<crate::weights::GateReport>, WeightState) {
    let mut st = WeightState::default();
    let rep = attempt(rows, as_of_week as i64 * WEEK_MS, &mut st, cfg);
    (rep, st)
}

const MOM: usize = 1;

fn detect(corr: f64, seeds: u64) -> (u64, f64, f64, f64, f64) {
    let cfg = WeightsConfig::default();
    let (mut adopted, mut mom, mut others, mut ic_i, mut ic_c) = (0u64, 0.0, 0.0f64, 0.0, 0.0);
    for s in 0..seeds {
        let rows = make_rows(100 + s, 156, 12, 60, MOM, &|_| corr);
        let (rep, st) = run(&rows, 170, &cfg);
        let rep = rep.unwrap();
        if rep.decision == Decision::Adopt {
            adopted += 1;
            mom += st.incumbent[MOM];
            others += st
                .incumbent
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != MOM)
                .map(|(_, m)| *m)
                .fold(0.0, f64::max);
            ic_i += rep.ic_incumbent.unwrap();
            ic_c += rep.ic_candidate.unwrap();
        }
    }
    let d = adopted.max(1) as f64;
    (adopted, mom / d, others / d, ic_i / d, ic_c / d)
}

#[test]
fn a_detects_the_predictive_factor_with_enough_data() {
    let (adopted, mom, others, ic_i, ic_c) = detect(0.30, 4);
    println!(
        "(a, rápido) corr 0,30: adopta {adopted}/4; multiplicador {mom:.2} (resto máx. {others:.2}); IC {ic_i:.3} -> {ic_c:.3}"
    );
    assert!(adopted >= 3 && mom > 1.5 && mom > others);
}

#[test]
#[ignore = "Montecarlo: cargo test --release -- --ignored --nocapture"]
fn a_montecarlo_power() {
    for corr in [0.30, 0.15, 0.08] {
        let (adopted, mom, others, ic_i, ic_c) = detect(corr, 40);
        println!(
            "(a) factor señal = {} con corr {corr:.2}, 1872 filas, 40 semillas: adopta {adopted}/40; multiplicador medio {mom:.2} \
             (mayor de los demás {others:.2}); IC de validación {ic_i:+.3} -> {ic_c:+.3}",
            FACTOR_NAMES[MOM]
        );
        if corr >= 0.15 {
            assert!(adopted >= 20 && mom > 1.5 && mom > others);
        }
    }
}

#[test]
fn b_small_sample_never_adopts() {
    let cfg = WeightsConfig::default();
    // Aunque el factor sea MUY predictivo (corr 0,4), con 100 filas independientes no se adopta.
    let mut adopted = 0;
    for s in 0..50 {
        let rows = make_rows(500 + s, 26, 4, 40, MOM, &|_| 0.4);
        let (rep, _) = run(&rows, 40, &cfg);
        let rep = rep.unwrap();
        assert_eq!(rep.decision, Decision::Keep);
        assert!(rep.reason.contains("insuficiente"), "{}", rep.reason);
        adopted += usize::from(rep.decision == Decision::Adopt);
    }
    println!("(b) corr 0,40 con ~100 filas, 50 semillas: adopta {adopted}/50");
    assert_eq!(adopted, 0);
}

fn noise_adoptions(seeds: u64) -> (usize, usize) {
    let cfg = WeightsConfig::default();
    let (mut single, mut sequence) = (0, 0);
    for s in 0..seeds {
        let rows = make_rows(900 + s, 156, 12, 60, MOM, &|_| 0.0);
        let (rep, _) = run(&rows, 170, &cfg);
        single += usize::from(rep.unwrap().decision == Decision::Adopt);
        let mut st = WeightState::default();
        let mut any = false;
        for wk in (60..=170).step_by(8) {
            if let Some(r) = attempt(&rows, wk as i64 * WEEK_MS, &mut st, &cfg) {
                any |= r.decision == Decision::Adopt;
            }
        }
        sequence += usize::from(any);
    }
    (single, sequence)
}

#[test]
fn b2_pure_noise_rarely_adopts() {
    let (single, sequence) = noise_adoptions(6);
    println!("(b2, rápido) solo ruido, 6 semillas: un intento {single}/6, secuencia {sequence}/6");
    assert!(single <= 1 && sequence <= 2);
}

#[test]
#[ignore = "Montecarlo: cargo test --release -- --ignored --nocapture"]
fn b2_montecarlo_false_adoption() {
    let seeds = 200;
    let (single, sequence) = noise_adoptions(seeds);
    println!(
        "(b2) solo ruido, {seeds} semillas, 1872 filas: adopción falsa en un intento {single}/{seeds}; \
         en una secuencia de intentos cada 8 semanas {sequence}/{seeds} (nominal: α = 0,05 en total)"
    );
    assert!(single <= seeds as usize / 10 && sequence <= seeds as usize / 10);
}

fn regime(seeds: u64) -> (f64, f64, f64, String) {
    // Régimen 1 (semanas 0-103): el momentum predice (corr 0,2). Régimen 2 (104-259): ruido.
    let cfg = WeightsConfig {
        half_life_weeks: 13.0,
        ..Default::default()
    };
    let (mut sum_peak, mut sum_end, mut sum_change) = (0.0, 0.0, 0.0);
    let mut traj = String::new();
    for s in 0..seeds {
        let mut line = String::new();
        let rows = make_rows(2000 + s, 260, 12, 60, MOM, &|w| {
            if w < 104 { 0.2 } else { 0.0 }
        });
        let mut st = WeightState::default();
        let (mut peak, mut at_change) = (1.0f64, 1.0);
        for wk in (50..=262).step_by(4) {
            let _ = attempt(&rows, wk as i64 * WEEK_MS, &mut st, &cfg);
            peak = peak.max(st.incumbent[MOM]);
            if wk == 110 {
                at_change = st.incumbent[MOM];
            }
            if (90..=210).contains(&wk) && wk % 8 == 2 {
                line.push_str(&format!("sem {wk}: {:.2} | ", st.incumbent[MOM]));
            }
        }
        if traj.is_empty() && peak > 1.5 {
            traj = format!("semilla {s}: {line}");
        }
        sum_peak += peak;
        sum_change += at_change;
        sum_end += st.incumbent[MOM];
    }
    let n = seeds as f64;
    (sum_change / n, sum_peak / n, sum_end / n, traj)
}

#[test]
fn c_regime_change_degrades_the_weight_over_time() {
    let (change, peak, end, traj) = regime(2);
    println!(
        "(c, rápido) 2 semillas: multiplicador del momentum en la semana 110 {change:.2}, pico {peak:.2}, final {end:.2}; {traj}"
    );
    assert!(peak > 1.5, "no llegó a aprender nada");
    assert!(end < peak - 0.5, "no se degradó");
}

#[test]
#[ignore = "Montecarlo: cargo test --release -- --ignored --nocapture"]
fn c_montecarlo_regime_change() {
    let (change, peak, end, traj) = regime(20);
    println!(
        "(c) cambio de régimen en la semana 104, 20 semillas: multiplicador del momentum en la semana 110 {change:.2}, pico {peak:.2}, \
         al final (semana 262) {end:.2}\n    trayectoria ({traj})"
    );
    assert!(peak > 1.5 && end < peak - 0.5);
}

#[test]
fn gate_rejects_identical_candidate_and_uses_posterior_validation() {
    let cfg = WeightsConfig::default();
    let rows = make_rows(7, 156, 12, 60, MOM, &|_| 0.15);
    let rep = evaluate_gate(&rows, &ONES, None, cfg.alpha, &cfg);
    // Validación estrictamente posterior al entrenamiento (con purga de horizonte).
    assert!(rep.val_start_ms.unwrap() >= rep.train_end_ms.unwrap() + 4 * WEEK_MS);
    // Con los pesos ya adoptados, el candidato igual no vuelve a ser «adoptado» (misma solución).
    if let Some(c) = &rep.candidate {
        let again = evaluate_gate(&rows, &c.multipliers, rep.val_end_ms, cfg.alpha, &cfg);
        assert_eq!(again.decision, Decision::Keep);
    }
}
