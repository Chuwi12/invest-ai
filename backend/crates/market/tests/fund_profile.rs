//! Perfil de ETF con respuestas REALES de Yahoo `quoteSummary` (19-oct-2026): SPY, QQQ y TLT.
//! Sin red ni claves.

use chrono::{DateTime, NaiveDate, Utc};
use market::providers::yahoo::parse_quote_summary;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-19T14:15:00Z").unwrap().with_timezone(&Utc)
}

fn profile(text: &str) -> domain::FundProfile {
    parse_quote_summary(text, now()).unwrap().fund_profile.expect("perfil del fondo")
}

#[test]
fn spy_profile_has_costs_assets_holdings_sectors_and_portfolio_multiples() {
    let p = profile(include_str!("fixtures/yahoo_quotesummary_spy.json"));
    assert_eq!(p.family.as_deref(), Some("State Street Investment Management"));
    assert_eq!(p.category.as_deref(), Some("Large Blend"));
    assert_eq!(p.inception_date, NaiveDate::from_ymd_opt(1993, 1, 22));
    assert_eq!(p.expense_ratio, Some(0.000945));
    assert_eq!(p.turnover, Some(0.03));
    assert_eq!(p.total_assets, Some(809_216_114_688.0));
    assert_eq!(p.dividend_yield, Some(0.0099));
    assert_eq!(p.beta_3y, Some(1.0));
    assert_eq!(p.top_holdings.len(), 10);
    assert_eq!(p.top_holdings[0].symbol.as_deref(), Some("NVDA"));
    let w: f64 = p.top_holdings.iter().map(|h| h.weight).sum();
    assert!((0.25..0.45).contains(&w), "{w}");
    assert_eq!(p.sector_weights[0].name, "technology");
    assert!((p.sector_weights[0].weight - 0.4045).abs() < 1e-9);
    assert!(p.sector_weights.windows(2).all(|w| w[0].weight >= w[1].weight));
    // Yahoo da los INVERSOS (beneficio/precio): 0,04132 -> PER 24,2.
    let e = p.equity.expect("estadísticas de renta variable");
    assert!((e.price_to_earnings.unwrap() - 24.2).abs() < 0.05, "{e:?}");
    assert!((e.price_to_book.unwrap() - 5.25).abs() < 0.05);
    assert!((e.price_to_sales.unwrap() - 3.64).abs() < 0.05);
    // Lo que no viene, no se inventa.
    assert_eq!(e.three_year_earnings_growth, None);
    assert_eq!(e.median_market_cap, None);
    assert!(p.bond.is_none() && p.gross_expense_ratio.is_none() && p.front_load.is_none());
    assert_eq!(p.source, "yahoo");
    assert_eq!(p.as_of, Some(now()));
}

#[test]
fn qqq_profile_treats_a_zero_turnover_as_unknown() {
    let p = profile(include_str!("fixtures/yahoo_quotesummary_qqq.json"));
    assert_eq!(p.family.as_deref(), Some("Invesco"));
    assert_eq!(p.category.as_deref(), Some("Large Growth"));
    assert_eq!(p.top_holdings.len(), 10);
    assert!(p.sector_weights[0].name == "technology" && p.sector_weights[0].weight > 0.5);
    assert_eq!(p.turnover, None, "Yahoo da 0 cuando no sabe");
    assert!((p.expense_ratio.unwrap() - 0.0018).abs() < 1e-6);
    assert!((p.equity.unwrap().price_to_earnings.unwrap() - 28.3).abs() < 0.1);
}

#[test]
fn tlt_bond_etf_has_no_holdings_no_equity_multiples_and_drops_an_inconsistent_duration() {
    let f = parse_quote_summary(include_str!("fixtures/yahoo_quotesummary_tlt.json"), now()).unwrap();
    let p = f.fund_profile.expect("perfil");
    assert_eq!(p.category.as_deref(), Some("Long Government"));
    assert!(p.top_holdings.is_empty(), "Yahoo no da posiciones de este fondo");
    assert!(p.equity.is_none(), "los ceros de renta variable no son datos");
    // Yahoo da duración 3,62 y vencimiento 7,29 para un fondo «Long»: contradice su categoría.
    assert!(p.bond.is_none());
    assert_eq!(p.asset_mix[0].name, "bond");
    assert_eq!(p.bond_ratings[0].name, "aa");
    assert_eq!(p.turnover, Some(0.18));
    assert_eq!(p.dividend_yield, Some(0.05));
    // El P/E «forward» de Yahoo para TLT es -4002,5: en un fondo no se muestra.
    assert_eq!(f.pe_forward, None);
}

#[test]
fn a_stock_has_no_fund_profile() {
    let f = parse_quote_summary(include_str!("fixtures/yahoo_quotesummary_aapl.json"), now()).unwrap();
    assert!(f.fund_profile.is_none());
}

#[test]
fn an_empty_fund_response_gives_no_profile() {
    let t = r#"{"quoteSummary":{"error":null,"result":[{"price":{"currency":"USD"},"fundProfile":{},"topHoldings":{"holdings":[]}}]}}"#;
    assert!(parse_quote_summary(t, now()).unwrap().fund_profile.is_none());
}
