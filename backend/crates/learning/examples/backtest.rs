//! Backtest de los factores de precio de la puntuación (diseño: `docs/notes/backtest.md`).
//!
//! ```text
//! cargo run --release -p learning --example backtest -- download   # 1 sola vez, ~110 peticiones a Yahoo
//! cargo run --release -p learning --example backtest -- run        # sin red: lee el fichero congelado
//! cargo run --release -p learning --example backtest -- posthoc    # análisis POST HOC ajustado por beta
//! ```
//!
//! Datos congelados en `$BACKTEST_DATA` o `~/.cache/invest-ai/backtest/prices.json` (fuera del repo).

use std::{path::PathBuf, time::Duration};

use chrono::{NaiveDate, Utc};
use domain::AssetId;
use learning::backtest::*;
use market::providers::yahoo::{Yahoo, to_yahoo_symbol};

const ETF: &str = "QQQ IWM DIA MDY IJR EFA EEM VGK EWJ EWZ FXI EWG IWD IWF USMV XLK XLF XLE XLV XLY XLP XLI XLB XLU XLRE XLC \
SMH XBI KRE XHB IBB GDX XME XOP VNQ ITA";
const STOCK: &str = "AAPL MSFT NVDA AMZN GOOGL META TSLA BRK-B JPM V UNH XOM JNJ PG HD MA CVX LLY KO PEP COST WMT DIS NFLX ADBE CRM \
CSCO INTC IBM BA GE F GM PFE MRK ABBV T VZ CMCSA NKE MCD ORCL AMD QCOM TXN CAT MMM KHC PYPL SBUX CVS \
ASML SAP NVO SHEL AZN UL DEO BP TTE SNY GSK NVS SAN BBVA ING";
const CRYPTO: &str = "BTC ETH XRP LTC ADA DOGE BNB LINK XLM TRX BCH";

fn universe_ids() -> Vec<String> {
    let mut v = vec!["etf:SPY".to_string()];
    v.extend(ETF.split_whitespace().map(|s| format!("etf:{s}")));
    v.extend(STOCK.split_whitespace().map(|s| format!("stock:{s}")));
    v.extend(CRYPTO.split_whitespace().map(|s| format!("crypto:{s}")));
    v
}

fn data_path() -> PathBuf {
    if let Ok(p) = std::env::var("BACKTEST_DATA") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/invest-ai/backtest/prices.json")
}

async fn download() -> Result<(), String> {
    let path = data_path();
    let client = market::http::build_client(Duration::from_secs(30)).map_err(|e| e.to_string())?;
    let yahoo = Yahoo::new(client);
    let now = Utc::now().timestamp();
    let from = NaiveDate::from_ymd_opt(2016, 9, 1).map_or(0, |d| day_of(d) * DAY);
    let mut series = Vec::new();
    let ids = universe_ids();
    for (i, s) in ids.iter().enumerate() {
        let id: AssetId = s.parse().map_err(|e| format!("{s}: {e}"))?;
        let Some(sym) = to_yahoo_symbol(&id) else { continue };
        let mut tries = 0;
        loop {
            tries += 1;
            match yahoo.daily_between(&sym, from, now).await {
                Ok(d) => {
                    println!("[{}/{}] {s}: {} velas", i + 1, ids.len(), d.candles.len());
                    series.push(SeriesFile { id: s.clone(), candles: d.candles, adj_close: d.adj_close });
                    break;
                }
                Err(e) if tries < 3 => {
                    eprintln!("{s}: {e}; reintento en 30 s");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                Err(e) => {
                    eprintln!("{s}: OMITIDO ({e})");
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let ds = DatasetFile { fetched_at: now, from, series };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&ds).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    let candles: usize = ds.series.iter().map(|s| s.candles.len()).sum();
    println!("guardado {}: {} símbolos, {candles} velas", path.display(), ds.series.len());
    Ok(())
}

fn p(x: f64) -> String {
    format!("{:+.3}", x)
}
fn pc(x: f64) -> String {
    format!("{:+.1}%", x * 100.0)
}

fn ics(panel: &Panel, u: Universe, hi: usize, s: usize) -> Vec<(usize, i64, f64)> {
    ic_series(panel, u, hi, &move |_, o| o.sig[s])
}

fn run() -> Result<(), String> {
    let path = data_path();
    let (ds, series) = load_dataset(&path)?;
    let start = day_of(NaiveDate::from_ymd_opt(2018, 1, 1).unwrap_or_default());
    let end = ds.fetched_at.div_euclid(DAY);
    let cfg = PanelConfig { start_day: start, end_day: end, now_s: ds.fetched_at };
    let panel = build_panel(&series, &cfg)?;
    let candles: usize = series.iter().map(|s| s.raw.len()).sum();
    println!("# Resultados del backtest\n");
    println!(
        "Datos: {} símbolos descargados, {candles} velas, descarga {}. Panel: {} viernes ({} a {}), {} activos en la sección cruzada.\n",
        series.len(),
        date_of(end),
        panel.weeks.len(),
        panel.weeks.first().map_or(String::new(), |w| date_of(w.day).to_string()),
        panel.weeks.last().map_or(String::new(), |w| date_of(w.day).to_string()),
        panel.assets.len()
    );
    for u in [Universe::Equity, Universe::Etf, Universe::Stock, Universe::Crypto] {
        let n_assets = panel.assets.iter().filter(|a| u.contains(a.group)).count();
        let usable = panel
            .weeks
            .iter()
            .filter(|w| w.obs.iter().filter(|o| u.contains(panel.assets[o.asset].group)).count() >= u.min_n())
            .count();
        let avg = panel
            .weeks
            .iter()
            .map(|w| w.obs.iter().filter(|o| u.contains(panel.assets[o.asset].group)).count())
            .filter(|n| *n >= u.min_n())
            .sum::<usize>() as f64
            / usable.max(1) as f64;
        println!("- Universo `{}`: {n_assets} activos, {usable} semanas usables, {avg:.1} activos de media por semana.", u.name());
    }

    // ---- 1. IC por factor y horizonte ----
    for u in [Universe::Equity, Universe::Etf, Universe::Stock, Universe::Crypto] {
        println!("\n## IC de Spearman, universo `{}`\n", u.name());
        println!("| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | IC 95 % (bloques) | 2018-20 | 2021-23 | 2024-hoy |");
        println!("|---|---|---|---|---|---|---|---|---|---|");
        for (s, name) in SIGNALS.iter().enumerate() {
            for (hi, h) in HORIZONS.iter().enumerate() {
                let all = ics(&panel, u, hi, s);
                let pts: Vec<(usize, f64)> = all.iter().map(|x| (x.0, x.2)).collect();
                let Some(sm) = summarize(&pts, *h as usize, true) else { continue };
                let per: Vec<String> = (0..3)
                    .map(|k| {
                        let v: Vec<(usize, f64)> =
                            all.iter().filter(|x| in_period(x.1, k)).map(|x| (x.0, x.2)).collect();
                        summarize(&v, *h as usize, false)
                            .map_or("n/d".into(), |q| format!("{} (t {:.1}, n {})", p(q.mean), q.t, q.n))
                    })
                    .collect();
                let b = sm.boot.map_or("n/d".into(), |(l, hh)| format!("[{}, {}]", p(l), p(hh)));
                println!(
                    "| {} | {h} | {} | {} | {:.2} | [{}, {}] | {b} | {} | {} | {} |",
                    name, sm.n, p(sm.mean), sm.t, p(sm.lo), p(sm.hi), per[0], per[1], per[2]
                );
            }
        }
    }

    // ---- 2. Aleatorio ----
    println!("\n## Puntuación aleatoria (200 semillas), universo `equity`\n");
    println!("| h | IC medio (media de semillas) | sd entre semillas | p2,5 / p97,5 | semillas con \\|t\\|>1,96 | \\|t\\|>2,87 | LS bruto anual | LS neto anual (sd) |");
    println!("|---|---|---|---|---|---|---|---|");
    let cost = CostModel::default_for(Universe::Equity);
    for (hi, h) in HORIZONS.iter().enumerate() {
        if let Some(r) = random_reference(&panel, Universe::Equity, hi, 1..=200, cost) {
            println!(
                "| {h} | {} | {:.3} | {} / {} | {:.1} % | {:.1} % | {} ({}) | {} ({}) |",
                p(r.ic_mean), r.ic_sd, p(r.ic_p025), p(r.ic_p975), r.frac_t196 * 100.0, r.frac_t287 * 100.0,
                pc(r.gross_mean), pc(r.gross_sd), pc(r.net_mean), pc(r.net_sd)
            );
        }
    }

    // ---- 3. Regla de decisión a priori (universo equity, 4 factores x 3 horizontes) ----
    println!("\n## Regla de decisión a priori (universo `equity`)\n");
    println!("(i) |t_NW| ≥ 2,87; (ii) IC medio > 0 en los tres subperiodos; (iii) IC medio > 0; (iv) cartera LS neta > 0.\n");
    println!("| factor | h | IC | t | (i) | (ii) | (iii) | LS neto anual | (iv) | ¿señal? |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    let mut n_signal = 0;
    for (s, name) in SIGNALS.iter().enumerate().take(4) {
        for (hi, h) in HORIZONS.iter().enumerate() {
            let all = ics(&panel, Universe::Equity, hi, s);
            let pts: Vec<(usize, f64)> = all.iter().map(|x| (x.0, x.2)).collect();
            let Some(sm) = summarize(&pts, *h as usize, false) else { continue };
            let per_ok = (0..3).all(|k| {
                let v: Vec<f64> = all.iter().filter(|x| in_period(x.1, k)).map(|x| x.2).collect();
                !v.is_empty() && v.iter().sum::<f64>() > 0.0
            });
            let f = move |_: &Week, o: &Obs| o.sig[s];
            let ls = long_short(&panel, Universe::Equity, hi, &f, cost);
            let net: Vec<f64> = ls.iter().map(|x| x.net).collect();
            let netm = net.iter().sum::<f64>() / net.len().max(1) as f64 * 52.0 / f64::from(*h);
            let (c1, c2, c3, c4) = (sm.t.abs() >= 2.87, per_ok, sm.mean > 0.0, netm > 0.0);
            let sig = c1 && c2 && c3 && c4;
            n_signal += usize::from(sig);
            let yn = |b: bool| if b { "sí" } else { "no" };
            println!(
                "| {} | {h} | {} | {:.2} | {} | {} | {} | {} | {} | {} |",
                name, p(sm.mean), sm.t, yn(c1), yn(c2), yn(c3), pc(netm), yn(c4), if sig { "SEÑAL" } else { "no" }
            );
        }
    }
    println!("\nPruebas de la familia que cumplen todo: {n_signal} de 12.");

    // ---- 4. Carteras ----
    for u in [Universe::Equity, Universe::Etf, Universe::Stock, Universe::Crypto] {
        let cost = CostModel::default_for(u);
        println!(
            "\n## Cartera quintil superior − inferior, universo `{}` (coste {} pb por operación, préstamo {:.1} %/año)\n",
            u.name(), cost.one_way_bps, cost.borrow_annual * 100.0
        );
        println!("| factor | h | cohortes | bruto anual | neto anual | vol anual | Sharpe neto | t neto (NW) | rotación | solo largos neto vs índice | neto 2018-20 | 2021-23 | 2024-hoy |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for s in [SIG_TECHNICAL, SIG_MOMENTUM, SIG_RISK, SIG_COMPOSITE, SIG_MOM12_1] {
            for (hi, h) in HORIZONS.iter().enumerate() {
                let f = move |_: &Week, o: &Obs| o.sig[s];
                let ls = long_short(&panel, u, hi, &f, cost);
                let g: Vec<f64> = ls.iter().map(|x| x.gross).collect();
                let n: Vec<f64> = ls.iter().map(|x| x.net).collect();
                let lo: Vec<f64> = ls.iter().map(|x| x.long_only_net).collect();
                let turn = ls.iter().map(|x| x.turnover).sum::<f64>() / ls.len().max(1) as f64;
                let (Some(gs), Some(ns), Some(los)) =
                    (summarize_port(&g, turn, *h), summarize_port(&n, turn, *h), summarize_port(&lo, turn, *h))
                else { continue };
                let per: Vec<String> = (0..3)
                    .map(|k| {
                        let v: Vec<f64> = ls.iter().filter(|x| in_period(x.day, k)).map(|x| x.net).collect();
                        summarize_port(&v, turn, *h).map_or("n/d".into(), |q| format!("{} (t {:.1})", pc(q.ann), q.t))
                    })
                    .collect();
                println!(
                    "| {} | {h} | {} | {} | {} | {:.1}% | {:.2} | {:.2} | {:.0}% | {} | {} | {} | {} |",
                    SIGNALS[s], ns.n, pc(gs.ann), pc(ns.ann), ns.ann_vol * 100.0, ns.sharpe, ns.t, turn * 100.0,
                    pc(los.ann), per[0], per[1], per[2]
                );
            }
        }
    }

    // ---- 5. Sensibilidad a costes del compuesto ----
    println!("\n## Sensibilidad a costes, compuesto, universo `equity` (LS neto anual)\n");
    println!("| h | 0 pb | 5 pb | 10 pb | 20 pb |");
    println!("|---|---|---|---|---|");
    for (hi, h) in HORIZONS.iter().enumerate() {
        let f = |_: &Week, o: &Obs| o.sig[SIG_COMPOSITE];
        let cells: Vec<String> = [0.0, 5.0, 10.0, 20.0]
            .iter()
            .map(|bps| {
                let c = CostModel { one_way_bps: *bps, borrow_annual: 0.005 };
                let v: Vec<f64> = long_short(&panel, Universe::Equity, hi, &f, c).iter().map(|x| x.net).collect();
                pc(v.iter().sum::<f64>() / v.len().max(1) as f64 * 52.0 / f64::from(*h))
            })
            .collect();
        println!("| {h} | {} |", cells.join(" | "));
    }

    // ---- 6. Quintiles ----
    println!("\n## Exceso medio por quintil (Q1 = peor puntuación), universo `equity`, por semana de h semanas\n");
    println!("| factor | h | Q1 | Q2 | Q3 | Q4 | Q5 |");
    println!("|---|---|---|---|---|---|---|");
    for s in [SIG_COMPOSITE, SIG_MOM12_1] {
        for (hi, h) in HORIZONS.iter().enumerate() {
            let f = move |_: &Week, o: &Obs| o.sig[s];
            if let Some(q) = quintile_means(&panel, Universe::Equity, hi, &f) {
                println!("| {} | {h} | {} |", SIGNALS[s], q.iter().map(|x| pc(*x)).collect::<Vec<_>>().join(" | "));
            }
        }
    }

    // ---- 7. Veredictos ----
    for (hi, h) in HORIZONS.iter().enumerate() {
        let Some((n, base)) = base_rate_outperform(&panel, Universe::Equity, hi) else { continue };
        println!("\n## Acierto por veredicto, h = {h} semanas, universo `equity` (n = {n}, tasa base de superar al índice {:.1} %)\n", base * 100.0);
        println!("| veredicto | n | acierto | acierto de referencia | diferencia | IC 95 % de la diferencia (bloques) |");
        println!("|---|---|---|---|---|---|");
        for r in verdict_table(&panel, Universe::Equity, hi, 600, 20_261_015) {
            let ci = r.lift_ci.map_or("n/d".into(), |(l, hh)| format!("[{}, {}]", pc(l), pc(hh)));
            println!(
                "| {} | {} | {:.1}% | {:.1}% | {} | {ci} |",
                learning::rules::verdict_str(r.verdict), r.n, r.hit * 100.0, r.base * 100.0, pc(r.lift)
            );
        }
    }
    Ok(())
}

fn mean_of(v: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = v.filter(|x| x.is_finite()).fold((0.0, 0usize), |a, x| (a.0 + x, a.1 + 1));
    if n == 0 { f64::NAN } else { s / n as f64 }
}

/// Análisis POST HOC (declarado después de ver los resultados): exceso ajustado por beta.
fn posthoc() -> Result<(), String> {
    let (ds, series) = load_dataset(&data_path())?;
    let start = day_of(NaiveDate::from_ymd_opt(2018, 1, 1).unwrap_or_default());
    let cfg = PanelConfig { start_day: start, end_day: ds.fetched_at.div_euclid(DAY), now_s: ds.fetched_at };
    let raw = build_panel(&series, &cfg)?;
    let adj = raw.with_beta_adjusted_excess();
    let u = Universe::Equity;
    let cost = CostModel::default_for(u);
    println!("# Análisis post hoc: exceso ajustado por beta\n");

    // ---- A. Diagnóstico de la beta ----
    let obs: Vec<&Obs> = raw
        .weeks
        .iter()
        .flat_map(|w| w.obs.iter())
        .filter(|o| u.contains(raw.assets[o.asset].group))
        .collect();
    let with_beta: Vec<&&Obs> = obs.iter().filter(|o| o.beta.is_some()).collect();
    let lo = with_beta.iter().filter(|o| o.beta_clipped && o.beta == Some(0.0)).count();
    let hi = with_beta.iter().filter(|o| o.beta_clipped && o.beta == Some(BETA_MAX)).count();
    println!("## A. Beta (104 semanas anteriores, mín. 52, recortada a [0, 3]), universo `equity`\n");
    println!(
        "- Observaciones activo-semana: {}; con beta: {}; sin beta: {}.\n- Recortadas: {} a 0 ({:.2} %) y {} a 3 ({:.2} %).\n- Beta media: {:.2}; ETF {:.2}; acciones/ADR {:.2}.",
        obs.len(),
        with_beta.len(),
        obs.len() - with_beta.len(),
        lo,
        100.0 * lo as f64 / with_beta.len().max(1) as f64,
        hi,
        100.0 * hi as f64 / with_beta.len().max(1) as f64,
        mean_of(with_beta.iter().filter_map(|o| o.beta)),
        mean_of(with_beta.iter().filter(|o| raw.assets[o.asset].group == Group::Etf).filter_map(|o| o.beta)),
        mean_of(with_beta.iter().filter(|o| raw.assets[o.asset].group == Group::Stock).filter_map(|o| o.beta)),
    );
    if let Some(spy) = series.iter().find(|s| s.id.to_string() == "etf:SPY") {
        let d0 = start;
        let first = spy.adj.iter().find(|c| c.t.div_euclid(DAY) >= d0).map(|c| (c.t, c.c));
        if let (Some((t0, p0)), Some(last)) = (first, spy.adj.last()) {
            let years = (last.t - t0) as f64 / (365.25 * DAY as f64);
            println!("- SPY (rentabilidad total) en el periodo: {:+.1} %/año.", 100.0 * ((last.c / p0).powf(1.0 / years) - 1.0));
        }
    }
    println!("\nBeta media por quintil del factor de riesgo-precio (Q1 = más riesgo, Q5 = menos riesgo), por semana y promediado:\n");
    println!("| quintil | beta media |");
    println!("|---|---|");
    let mut sums = [0.0f64; 5];
    let mut cnt = [0usize; 5];
    for w in &raw.weeks {
        let mut c: Vec<(f64, f64)> = w
            .obs
            .iter()
            .filter(|o| u.contains(raw.assets[o.asset].group))
            .filter_map(|o| o.beta.map(|b| (o.sig[SIG_RISK], b)))
            .collect();
        if c.len() < u.min_n() {
            continue;
        }
        c.sort_by(|a, b| a.0.total_cmp(&b.0));
        let n = c.len();
        let mut tmp = ([0.0; 5], [0usize; 5]);
        for (i, x) in c.iter().enumerate() {
            let q = (i * 5 / n).min(4);
            tmp.0[q] += x.1;
            tmp.1[q] += 1;
        }
        for q in 0..5 {
            sums[q] += tmp.0[q] / tmp.1[q].max(1) as f64;
            cnt[q] += 1;
        }
    }
    for q in 0..5 {
        println!("| Q{} | {:.2} |", q + 1, sums[q] / cnt[q].max(1) as f64);
    }

    // ---- B. IC ajustado ----
    println!("\n## B. IC de Spearman con exceso ajustado por beta, universo `equity`\n");
    println!("| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | 2018-20 | 2021-23 | 2024-hoy | IC sin ajustar | t sin ajustar |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (s, name) in SIGNALS.iter().enumerate() {
        for (hi, h) in HORIZONS.iter().enumerate() {
            let a = ics(&adj, u, hi, s);
            let r = ics(&raw, u, hi, s);
            let pa: Vec<(usize, f64)> = a.iter().map(|x| (x.0, x.2)).collect();
            let pr: Vec<(usize, f64)> = r.iter().map(|x| (x.0, x.2)).collect();
            let (Some(sa), Some(sr)) = (summarize(&pa, *h as usize, false), summarize(&pr, *h as usize, false)) else { continue };
            let per: Vec<String> = (0..3)
                .map(|k| {
                    let v: Vec<(usize, f64)> = a.iter().filter(|x| in_period(x.1, k)).map(|x| (x.0, x.2)).collect();
                    summarize(&v, *h as usize, false).map_or("n/d".into(), |q| format!("{} (t {:.1})", p(q.mean), q.t))
                })
                .collect();
            println!(
                "| {name} | {h} | {} | {} | {:.2} | [{}, {}] | {} | {} | {} | {} | {:.2} |",
                sa.n, p(sa.mean), sa.t, p(sa.lo), p(sa.hi), per[0], per[1], per[2], p(sr.mean), sr.t
            );
        }
    }

    // ---- C. Carteras ----
    println!("\n## C. Cartera quintil superior − inferior con exceso ajustado, universo `equity` (10 pb, 0,5 %/año)\n");
    println!("| factor | h | bruto ajustado | neto ajustado | t neto | bruto sin ajustar | neto sin ajustar | beta larga | beta corta | beta neta (L−C) | ajustado 2018-20 | 2021-23 | 2024-hoy |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for s in [SIG_RISK, SIG_COMPOSITE, SIG_TECHNICAL, SIG_MOMENTUM, SIG_MOM12_1] {
        for (hi, h) in HORIZONS.iter().enumerate() {
            let f = move |_: &Week, o: &Obs| o.sig[s];
            let la = long_short(&adj, u, hi, &f, cost);
            let lr = long_short(&raw, u, hi, &f, cost);
            let col = |v: &[PortPoint], g: fn(&PortPoint) -> f64| v.iter().map(g).collect::<Vec<f64>>();
            let turn = la.iter().map(|x| x.turnover).sum::<f64>() / la.len().max(1) as f64;
            let (Some(ga), Some(na), Some(gr), Some(nr)) = (
                summarize_port(&col(&la, |x| x.gross), turn, *h),
                summarize_port(&col(&la, |x| x.net), turn, *h),
                summarize_port(&col(&lr, |x| x.gross), turn, *h),
                summarize_port(&col(&lr, |x| x.net), turn, *h),
            ) else { continue };
            let (bl, bs) = (mean_of(la.iter().map(|x| x.beta_long)), mean_of(la.iter().map(|x| x.beta_short)));
            let per: Vec<String> = (0..3)
                .map(|k| {
                    let v: Vec<f64> = la.iter().filter(|x| in_period(x.day, k)).map(|x| x.net).collect();
                    summarize_port(&v, turn, *h).map_or("n/d".into(), |q| format!("{} (t {:.1})", pc(q.ann), q.t))
                })
                .collect();
            println!(
                "| {} | {h} | {} | {} | {:.2} | {} | {} | {:.2} | {:.2} | {:+.2} | {} | {} | {} |",
                SIGNALS[s], pc(ga.ann), pc(na.ann), na.t, pc(gr.ann), pc(nr.ann), bl, bs, bl - bs, per[0], per[1], per[2]
            );
        }
    }

    // ---- D. Riesgo-precio en otros universos ----
    println!("\n## D. IC del riesgo-precio y del compuesto por universo: sin ajustar frente a ajustado\n");
    println!("| universo | factor | h | IC sin ajustar (t) | IC ajustado (t) |");
    println!("|---|---|---|---|---|");
    for uu in [Universe::Etf, Universe::Stock] {
        for s in [SIG_RISK, SIG_COMPOSITE] {
            for (hi, h) in HORIZONS.iter().enumerate() {
                let f = |panel: &Panel| {
                    let v: Vec<(usize, f64)> = ics(panel, uu, hi, s).iter().map(|x| (x.0, x.2)).collect();
                    summarize(&v, *h as usize, false)
                };
                if let (Some(r), Some(a)) = (f(&raw), f(&adj)) {
                    println!("| {} | {} | {h} | {} ({:.2}) | {} ({:.2}) |", uu.name(), SIGNALS[s], p(r.mean), r.t, p(a.mean), a.t);
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let r = match std::env::args().nth(1).as_deref() {
        Some("download") => download().await,
        Some("run") => run(),
        Some("posthoc") => posthoc(),
        _ => Err("uso: backtest download | run | posthoc".into()),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
