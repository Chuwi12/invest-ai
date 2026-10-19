//! Estudio histórico de las banderas del perfil de riesgo de una OPV (ver `docs/notes/ipo-study.md`).
//!
//! `cargo run --release -p market --example ipo_study -- download [--retry]`  (red; reanudable)
//! `cargo run --release -p market --example ipo_study -- run`                  (sin red; determinista)
//!
//! Necesita `FINNHUB_API_KEY` y `SEC_USER_AGENT` en el entorno solo para `download`. Los datos se
//! congelan en `~/.cache/invest-ai/ipo-study/` (o `IPO_STUDY_DATA`), fuera del repo.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Months, NaiveDate, Utc};
use domain::{IpoEntry, IpoStatus, Prospectus};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use market::{
    MarketConfig, MarketService,
    ipo_study::{self, Exclusion, FLAGS, FrozenSeries, HORIZONS, N_FLAGS, Obs},
    providers::yahoo::{DailySeries, Yahoo},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Meta {
    /// Fecha F de la descarga (UTC): ancla de la ventana y de los horizontes.
    freeze_date: NaiveDate,
    downloaded_at: String,
}

fn data_dir() -> PathBuf {
    std::env::var("IPO_STUDY_DATA").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").expect("HOME")).join(".cache/invest-ai/ipo-study")
    })
}

fn write_gz<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut enc = GzEncoder::new(f, Compression::default());
    enc.write_all(&serde_json::to_vec(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn read_gz<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> {
    let mut s = Vec::new();
    GzDecoder::new(std::fs::File::open(path).map_err(|e| e.to_string())?).read_to_end(&mut s).map_err(|e| e.to_string())?;
    serde_json::from_slice(&s).map_err(|e| e.to_string())
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn key_of(e: &IpoEntry) -> String {
    format!("{}_{}", e.symbol.clone().unwrap_or_default().replace(['/', '.'], "-"), e.date)
}

/// Universo del diseño: precio definitivo ≥ 1 USD, símbolo, sin SPAC, primera cotización entre F−36 m y F−3 m.
fn candidates(all: &[IpoEntry], f: NaiveDate) -> Vec<IpoEntry> {
    let lo = f.checked_sub_months(Months::new(36)).unwrap();
    let hi = f.checked_sub_months(Months::new(3)).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut v: Vec<IpoEntry> = all
        .iter()
        .filter(|e| {
            e.status == IpoStatus::Priced
                && e.symbol.as_deref().is_some_and(|s| !s.is_empty())
                && e.price_low.is_some_and(|p| p >= 1.0)
                && e.price_low == e.price_high
                && !e.spac_likely
                && e.date >= lo
                && e.date <= hi
        })
        .filter(|e| seen.insert(key_of(e)))
        .cloned()
        .collect();
    v.sort_by(|a, b| a.date.cmp(&b.date).then(a.symbol.cmp(&b.symbol)));
    v
}

// ---------------------------------------------------------------------------------------------
// download
// ---------------------------------------------------------------------------------------------

async fn download(retry: bool) -> Result<(), String> {
    let dir = data_dir();
    for sub in ["prospectus", "series"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(|e| e.to_string())?;
    }
    let meta_path = dir.join("meta.json");
    let meta: Meta = if meta_path.exists() {
        serde_json::from_slice(&std::fs::read(&meta_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?
    } else {
        let m = Meta { freeze_date: Utc::now().date_naive(), downloaded_at: Utc::now().to_rfc3339() };
        write_json(&meta_path, &m)?;
        m
    };
    let f = meta.freeze_date;
    println!("F = {f} (descarga iniciada {})", meta.downloaded_at);
    let svc = MarketService::new(MarketConfig::from_env()).map_err(|e| e.to_string())?;

    // 1. Calendario: tramos de 180 días entre F−36 m y F.
    let cal_path = dir.join("calendar.json");
    if !cal_path.exists() {
        let mut all: Vec<IpoEntry> = Vec::new();
        let mut from = f.checked_sub_months(Months::new(36)).unwrap();
        while from <= f {
            let to = (from + chrono::Duration::days(179)).min(f);
            let rows = svc.ipo_calendar(from, to).await.map_err(|e| format!("calendario {from}..{to}: {e}"))?;
            println!("calendario {from}..{to}: {} filas", rows.len());
            all.extend(rows);
            from = to + chrono::Duration::days(1);
        }
        if all.iter().all(|e| e.source != "finnhub") {
            return Err("el calendario no vino de Finnhub (¿falta FINNHUB_API_KEY?): sin precio ni símbolo no hay estudio".into());
        }
        write_json(&cal_path, &all)?;
    }
    let all: Vec<IpoEntry> = serde_json::from_slice(&std::fs::read(&cal_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let cands = candidates(&all, f);
    println!("{} OPV candidatas", cands.len());

    // 2. Series: Yahoo con 0,5 s entre peticiones. SPY una vez.
    let yahoo = Yahoo::new(market::http::build_client(Duration::from_secs(30)).map_err(|e| e.to_string())?);
    let to_ts = Utc::now().timestamp();
    let ts = |d: NaiveDate| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    let fetch_series = |sym: String, from: NaiveDate| {
        let yahoo = &yahoo;
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            yahoo.daily_between(&sym, ts(from), to_ts).await.map_err(|e| e.to_string())
        }
    };
    let spy_path = dir.join("series/SPY.json.gz");
    if !spy_path.exists() {
        let s = fetch_series("SPY".into(), f.checked_sub_months(Months::new(38)).unwrap()).await?;
        write_gz(&spy_path, &FrozenSeries::from(&s))?;
        println!("SPY: {} velas", s.candles.len());
    }
    for (i, e) in cands.iter().enumerate() {
        let key = key_of(e);
        let sym = e.symbol.clone().unwrap();
        // Folleto (SEC): por símbolo y, si falla, por nombre.
        let (pg, pn) = (dir.join(format!("prospectus/{key}.json.gz")), dir.join(format!("prospectus/{key}.none")));
        if retry {
            let _ = std::fs::remove_file(&pn);
        }
        if !pg.exists() && !pn.exists() {
            let r = match svc.ipo_prospectus(&sym, false, 0).await {
                Ok(p) => Ok(p),
                Err(_) => svc.ipo_prospectus(&e.name, false, 0).await,
            };
            match r {
                Ok(p) => write_gz(&pg, &p)?,
                Err(err) => std::fs::write(&pn, err.to_string()).map_err(|e| e.to_string())?,
            }
        }
        // Serie del símbolo (Yahoo).
        let (sg, sn) = (dir.join(format!("series/{key}.json.gz")), dir.join(format!("series/{key}.none")));
        if retry {
            let _ = std::fs::remove_file(&sn);
        }
        if !sg.exists() && !sn.exists() {
            match fetch_series(sym.clone(), e.date - chrono::Duration::days(7)).await {
                Ok(s) => write_gz(&sg, &FrozenSeries::from(&s))?,
                Err(err) => std::fs::write(&sn, err).map_err(|e| e.to_string())?,
            }
        }
        if i % 20 == 0 {
            println!("{}/{} ({key})", i + 1, cands.len());
        }
    }

    // 3. Manifiesto con sha256 y fecha de cada fichero.
    let mut files = BTreeMap::new();
    for sub in ["", "prospectus", "series"] {
        for ent in std::fs::read_dir(dir.join(sub)).map_err(|e| e.to_string())? {
            let ent = ent.map_err(|e| e.to_string())?;
            let name = ent.file_name().to_string_lossy().to_string();
            if ent.path().is_dir() || name == "manifest.json" || name == "results.md" || name.ends_with(".tmp") {
                continue;
            }
            let bytes = std::fs::read(ent.path()).map_err(|e| e.to_string())?;
            let at: chrono::DateTime<Utc> = ent.metadata().and_then(|m| m.modified()).map_err(|e| e.to_string())?.into();
            let rel = if sub.is_empty() { name } else { format!("{sub}/{name}") };
            files.insert(rel, (hex(&Sha256::digest(&bytes)), at.to_rfc3339()));
        }
    }
    write_json(&dir.join("manifest.json"), &files)?;
    println!("manifiesto: {} ficheros", files.len());
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------------------------

fn run() -> Result<(), String> {
    let dir = data_dir();
    let manifest: BTreeMap<String, (String, String)> =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).map_err(|e| format!("falta manifest.json (¿download?): {e}"))?).map_err(|e| e.to_string())?;
    for (rel, (sha, _)) in &manifest {
        let got = hex(&Sha256::digest(std::fs::read(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?));
        if &got != sha {
            return Err(format!("sha256 distinto en {rel}: los datos congelados cambiaron"));
        }
    }
    let meta: Meta = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let all: Vec<IpoEntry> = serde_json::from_slice(&std::fs::read(dir.join("calendar.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let cands = candidates(&all, meta.freeze_date);
    let spy: DailySeries = read_gz::<FrozenSeries>(&dir.join("series/SPY.json.gz"))?.into();
    // Reloj fijo: el resultado no depende de la hora de ejecución.
    let now = meta.freeze_date.and_hms_opt(0, 0, 0).unwrap().and_utc();

    let mut obs: Vec<Obs> = Vec::new();
    let mut excl: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut missing_by_tier: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for e in &cands {
        let key = key_of(e);
        let p: Option<Prospectus> = read_gz(&dir.join(format!("prospectus/{key}.json.gz"))).ok();
        let s: Option<DailySeries> = read_gz::<FrozenSeries>(&dir.join(format!("series/{key}.json.gz"))).ok().map(Into::into);
        let tier = match market::ipo_risk::offer_size_usd(e) {
            Some(x) if x < ipo_study::SIZE_MICRO_USD => "< 50 M",
            Some(x) if x < ipo_study::SIZE_LARGE_USD => "50–250 M",
            Some(_) => "≥ 250 M",
            None => "sin tamaño",
        };
        let t = missing_by_tier.entry(tier).or_default();
        t.0 += 1;
        match ipo_study::build_obs(e, p.as_ref(), s.as_ref(), &spy, now) {
            Ok(o) => obs.push(o),
            Err(x) => {
                if x == Exclusion::NoYahooSeries {
                    t.1 += 1;
                }
                let name = match x {
                    Exclusion::NoProspectus => "sin folleto (SEC)",
                    Exclusion::ProspectusOutOfWindow => "folleto fuera de fecha o preliminar",
                    Exclusion::NoYahooSeries => "sin serie en Yahoo (supervivencia)",
                    Exclusion::BadPerformance => "rendimiento no calculable",
                };
                excl.entry(name).or_default().push(e.symbol.clone().unwrap_or_default());
            }
        }
    }
    let mut out = String::new();
    let w = |out: &mut String, s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    w(&mut out, format!("# Resultados del estudio de OPV (F = {}, datos congelados)\n", meta.freeze_date));
    w(&mut out, format!("Candidatas del calendario: {}; en el estudio: **{}**.\n", cands.len(), obs.len()));
    w(&mut out, "## Desgaste del universo\n\n| motivo | n | símbolos |\n|---|---|---|".into());
    for (k, v) in &excl {
        let list = v.iter().take(40).cloned().collect::<Vec<_>>().join(", ");
        w(&mut out, format!("| {k} | {} | {list}{} |", v.len(), if v.len() > 40 { " …" } else { "" }));
    }
    w(&mut out, "\nSin serie en Yahoo por tramo de tamaño (n candidatas / sin serie):".into());
    for (k, (n, m)) in &missing_by_tier {
        w(&mut out, format!("- {k}: {n} / {m}"));
    }

    w(&mut out, "\n## Cobertura de las banderas (OPV con el indicador extraído) y prevalencia\n\n| bandera | n con dato | % de las OPV | n con bandera | prevalencia |\n|---|---|---|---|---|".into());
    for (f, (id, name)) in FLAGS.iter().enumerate() {
        let known = obs.iter().filter(|o| o.flags[f].is_some()).count();
        let yes = obs.iter().filter(|o| o.flags[f] == Some(true)).count();
        w(&mut out, format!("| {id} {name} | {known} | {:.0} % | {yes} | {:.0} % |", 100.0 * known as f64 / obs.len().max(1) as f64, 100.0 * yes as f64 / known.max(1) as f64));
    }

    w(&mut out, "\n## Correlación entre banderas (phi; n del solape entre paréntesis)\n".into());
    let head: String = (0..N_FLAGS).map(|k| format!(" {} |", FLAGS[k].0)).collect();
    w(&mut out, format!("| |{head}\n|---|{}", "---|".repeat(N_FLAGS)));
    for (a, (id, _)) in FLAGS.iter().enumerate() {
        let cells: String = (0..N_FLAGS)
            .map(|b| match ipo_study::phi(&obs, a, b) {
                (Some(v), n) => format!(" {v:+.2} ({n}) |"),
                (None, n) => format!(" — ({n}) |"),
            })
            .collect();
        w(&mut out, format!("| {id} |{cells}"));
    }

    type Pick = fn(&Obs) -> Option<f64>;
    let outcomes: [(&str, Pick); 6] = [
        ("PRINCIPAL: exceso a 6 m desde la colocación", |o| o.outcomes.placement[1]),
        ("exceso a 3 m desde la colocación", |o| o.outcomes.placement[0]),
        ("exceso a 12 m desde la colocación", |o| o.outcomes.placement[2]),
        ("exceso a 3 m desde el cierre del día 1", |o| o.outcomes.day1[0]),
        ("exceso a 6 m desde el cierre del día 1", |o| o.outcomes.day1[1]),
        ("exceso a 12 m desde el cierre del día 1", |o| o.outcomes.day1[2]),
    ];
    let mut tables = Vec::new();
    for (label, pick) in &outcomes {
        let ys: Vec<f64> = obs.iter().filter_map(pick).collect();
        w(&mut out, format!("\n## {label}\n\nOPV con dato: {}; mediana global {:.1} pp; σ robusta {:.1} pp.\n", ys.len(), ipo_study::median(&ys).unwrap_or(f64::NAN), ipo_study::robust_sigma(&ys).unwrap_or(f64::NAN)));
        w(&mut out, "| bandera | análisis | n con | n sin | diferencia | IC 95 % | IC conglom. | p | p Holm | MDE | sin estratificar: n con/sin, mediana con/sin, dif., IC, p |\n|---|---|---|---|---|---|---|---|---|---|---|".into());
        let c = ipo_study::contrasts(&obs, *pick, ipo_study::SEED);
        for r in &c {
            let f1 = |x: Option<f64>| x.map_or("—".to_string(), |v| format!("{v:+.1}"));
            let ci = |c: Option<(f64, f64)>| c.map_or("—".to_string(), |(a, b)| format!("[{a:+.1}, {b:+.1}]"));
            let pp = |x: Option<f64>| x.map_or("—".to_string(), |v| format!("{v:.3}"));
            let note = if r.testable { "" } else { " (no contrastable)" };
            w(&mut out, format!(
                "| {}{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}/{}, {}/{}, {}, {}, {} |",
                FLAGS[r.flag].0, note, if r.stratified { "estratificado" } else { "directo" }, r.n1, r.n0, f1(r.diff), ci(r.ci), ci(r.ci_cluster), pp(r.p), pp(r.p_holm),
                r.mde.map_or("—".to_string(), |v| format!("{v:.0} pp")), r.raw_n1, r.raw_n0, f1(r.raw_median1), f1(r.raw_median0), f1(r.raw_diff), ci(r.raw_ci), pp(r.raw_p)
            ));
        }
        tables.push(c);
    }

    w(&mut out, "\n## Regla de decisión a priori\n".into());
    let mut any = false;
    for f in 0..N_FLAGS {
        let (assoc, usable) = ipo_study::decide(&tables[0][f], &tables[1][f], &tables[2][f], &tables[4][f]);
        any |= assoc;
        w(&mut out, format!("- {} {}: se asocia = {assoc}; aprovechable por un inversor normal = {usable}", FLAGS[f].0, FLAGS[f].1));
    }
    w(&mut out, if any { "\nAlguna bandera cumple la regla (ver arriba).".into() } else { "\n**Ninguna bandera cumple la regla: no se distingue del azar.**".to_string() });

    let placebo = ipo_study::placebo_rate(&obs, outcomes[0].1, ipo_study::SEED ^ 0x5151);
    w(&mut out, format!("\n## Referencia de azar\n\nBanderas aleatorias con la misma prevalencia ({} réplicas de familia, {} permutaciones): proporción con p Holm < 0,05 = {:.1} % (esperable ≤ 5 %).", ipo_study::PLACEBOS, ipo_study::PLACEBO_REPS, 100.0 * placebo));
    let _ = HORIZONS;
    print!("{out}");
    std::fs::write(dir.join("results.md"), &out).map_err(|e| e.to_string())?;
    write_json(&dir.join("obs.json"), &obs)?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("download") => download(args.iter().any(|a| a == "--retry")).await,
        Some("run") => run(),
        _ => Err("uso: ipo_study download [--retry] | run".into()),
    };
    if let Err(e) = res {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
