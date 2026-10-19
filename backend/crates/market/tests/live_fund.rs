//! Pruebas de humo de ETF contra Yahoo y EDGAR reales, sin clave. Ignoradas por defecto:
//! `cargo test -p market --test live_fund -- --ignored --nocapture --test-threads=1`

use domain::{AssetClass, AssetId, FundDocs, MarketData};
use market::{MarketConfig, MarketService};

fn svc() -> std::sync::Arc<MarketService> {
    let cfg = MarketConfig::from_lookup(|k| (k == "SEC_USER_AGENT").then(|| "invest-ai-test contacto@invest-ai.local".to_string()));
    MarketService::new(cfg).expect("servicio")
}

#[tokio::test]
#[ignore = "red real"]
async fn yahoo_fund_profile_spy_and_tlt() {
    let s = svc();
    for t in ["SPY", "TLT"] {
        let id = AssetId::new(AssetClass::Etf, t);
        let f = s.fundamentals(&id).await.expect("fundamentales").expect("hay fundamentales");
        let p = f.fund_profile.expect("perfil del fondo");
        println!("{t}: {} / {:?} ER={:?} AUM={:?} pos={} sectores={} equity={:?} bond={:?} url={:?}", p.family.clone().unwrap_or_default(), p.category, p.expense_ratio, p.total_assets, p.top_holdings.len(), p.sector_weights.len(), p.equity, p.bond, p.source_url);
        assert!(p.expense_ratio.is_some_and(|e| e > 0.0 && e < 0.02) && p.total_assets.is_some_and(|a| a > 1e9));
    }
}

#[tokio::test]
#[ignore = "red real"]
async fn edgar_fund_prospectus_spy_qqq_tlt() {
    let s = svc();
    for t in ["SPY", "QQQ", "TLT"] {
        let id = AssetId::new(AssetClass::Etf, t);
        let p = FundDocs::fund_prospectus(s.as_ref(), &id).await.unwrap_or_else(|e| panic!("{t}: {e}"));
        let keys: Vec<_> = p.sections.iter().map(|x| (x.key.as_str(), x.text.chars().count())).collect();
        println!("{t}: {} {} {} {} secciones={keys:?} faltan={:?}", p.issuer, p.form, p.filed_on, p.url, p.missing);
        assert!(p.sections.iter().any(|x| x.key == "objective") && p.sections.len() >= 3);
    }
}
