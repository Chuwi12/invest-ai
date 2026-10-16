//! Herramienta de desarrollo: `ipo_profile <prospectus.json>...` calcula el perfil de riesgo de
//! `Prospectus` guardados con `ipo_dump` y lo imprime (sin red).
fn main() {
    for path in std::env::args().skip(1) {
        let p: domain::Prospectus = serde_json::from_str(&std::fs::read_to_string(&path).expect("leer")).expect("json");
        let prof = market::ipo_risk::build_profile(market::ipo_risk::ProfileInput {
            symbol: p.ticker.as_deref().unwrap_or("?"),
            entry: None,
            prospectus: Some(&p),
            base_rate: None,
            now: chrono::Utc::now(),
        });
        println!("===== {} ({})", p.issuer, path);
        for i in &prof.indicators {
            let st = if i.status == domain::IpoIndicatorStatus::Extracted { "OK " } else { "-- " };
            println!("{st}{:34} {}", i.label, i.value.clone().or(i.note.clone()).unwrap_or_default());
            if std::env::var("VERBOSE").is_ok()
                && let Some(e) = &i.evidence
            {
                println!("      evidencia: {e}");
            }
            for f in &i.flags {
                println!("      · {}{}", f.label, if f.attention { " [!]" } else { "" });
                if std::env::var("VERBOSE").is_ok() {
                    println!("          «{}»", f.evidence);
                }
            }
        }
    }
}
