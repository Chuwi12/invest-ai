//! Informes periódicos de empresas contra EDGAR real, sin clave. Ignorada por defecto:
//! `cargo test -p market --test live_company -- --ignored --nocapture --test-threads=1`
//! (`SEC_USER_AGENT` si está en el entorno; si no, un contacto de prueba).

use domain::{AssetClass, AssetId, CompanyDocs};
use market::{MarketConfig, MarketService};

fn svc() -> std::sync::Arc<MarketService> {
    let ua = std::env::var("SEC_USER_AGENT").unwrap_or_else(|_| "invest-ai-test contacto@invest-ai.local".into());
    let cfg = MarketConfig::from_lookup(|k| (k == "SEC_USER_AGENT").then(|| ua.clone()));
    MarketService::new(cfg).expect("servicio")
}

#[tokio::test]
#[ignore = "red real"]
async fn edgar_company_reports() {
    let s = svc();
    for t in ["AAPL", "KO", "XOM", "JPM", "NVDA", "ASML", "SAP"] {
        let id = AssetId::new(AssetClass::Stock, t);
        let t0 = std::time::Instant::now();
        let reports = CompanyDocs::company_reports(s.as_ref(), &id).await.unwrap_or_else(|e| panic!("{t}: {e}"));
        println!("== {t}: {} informes en {:.1} s", reports.len(), t0.elapsed().as_secs_f32());
        for p in &reports {
            let secs: Vec<_> = p.sections.iter().map(|x| format!("{}={}{}", x.key, x.original_chars, if x.truncated { " (recortada)" } else { "" })).collect();
            println!("  {:5} {} {} doc={} chars\n        secciones: {}\n        missing: {:?}", p.form, p.filed_on, p.url, p.document_chars, secs.join(", "), p.missing);
        }
        if matches!(t, "AAPL" | "KO" | "NVDA") {
            let annual = reports.iter().find(|p| p.form == "10-K").unwrap_or_else(|| panic!("{t}: sin 10-K"));
            assert!(annual.sections.iter().any(|x| x.key == "risk_factors") && annual.sections.iter().any(|x| x.key == "mdna"), "{t}: {:?}", annual.missing);
        }
        assert!(reports.iter().all(|p| p.offer_price.is_none() && !p.preliminary && p.url.starts_with("https://www.sec.gov/Archives/edgar/data/")));
    }
}

#[tokio::test]
#[ignore = "red real"]
async fn edgar_company_reports_rejects_non_stocks_and_unknown_tickers() {
    let s = svc();
    assert!(CompanyDocs::company_reports(s.as_ref(), &AssetId::new(AssetClass::Etf, "SPY")).await.is_err());
    assert!(CompanyDocs::company_reports(s.as_ref(), &AssetId::new(AssetClass::Stock, "ZZZZZZ")).await.is_err());
}
