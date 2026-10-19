//! Perfil propio de un ETF o fondo a partir del `quoteSummary` de Yahoo (módulos `fundProfile`,
//! `topHoldings` y `defaultKeyStatistics`). Sin peticiones extra: son los mismos módulos que ya
//! pide `fundamentals` para ETF/fondos.
//!
//! Regla: lo que Yahoo no da queda en `None` o vacío; nunca se inventa ni se rellena con otra fuente.
//!
//! Rareza de Yahoo: `topHoldings.equityHoldings.priceToEarnings/priceToBook/priceToSales/
//! priceToCashflow` NO son múltiplos sino sus INVERSOS (SPY: 0,0413 de «PER» = beneficio/precio; el
//! PER real es 1/0,0413 = 24,2). Aquí se invierten y se descarta lo que no sea positivo.

use chrono::{DateTime, Utc};
use domain::{FundBondStats, FundEquityStats, FundProfile, Holding, WeightedItem};
use serde_json::Value;

use crate::providers::yahoo::{raw, raw_date};

/// Posiciones que se conservan.
pub const MAX_HOLDINGS: usize = 10;

fn first(vals: &[Option<f64>]) -> Option<f64> {
    vals.iter().copied().flatten().next()
}

/// Inverso de un «múltiplo» que Yahoo da como rentabilidad (beneficio/precio). Acota lo absurdo.
fn invert(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x > 0.0).map(|x| 1.0 / x).filter(|m| m.is_finite() && *m < 1000.0)
}

/// `[{"technology": {"raw": 0.4}}, …]` -> pares ordenados de mayor a menor peso (sin nulos ni ceros).
fn weights(v: &Value) -> Vec<WeightedItem> {
    let mut out: Vec<WeightedItem> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|o| o.iter())
        .filter_map(|(k, x)| Some(WeightedItem { name: k.clone(), weight: raw(x)? }))
        .filter(|w| w.weight > 0.0)
        .collect();
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

/// Perfil del fondo. `None` si Yahoo no trae nada propio de un fondo.
pub fn fund_profile(r: &Value, now: DateTime<Utc>) -> Option<FundProfile> {
    let (fund, th, ks, sd) = (&r["fundProfile"], &r["topHoldings"], &r["defaultKeyStatistics"], &r["summaryDetail"]);
    let fees = &fund["feesExpensesInvestment"];
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

    let mut p = FundProfile {
        family: text(&fund["family"]).or_else(|| text(&ks["fundFamily"])),
        category: text(&fund["categoryName"]).or_else(|| text(&ks["category"])),
        legal_type: text(&fund["legalType"]).or_else(|| text(&ks["legalType"])),
        inception_date: raw_date(&ks["fundInceptionDate"]),
        expense_ratio: first(&[raw(&fees["annualReportExpenseRatio"]), raw(&fees["netExpRatio"]), raw(&ks["annualReportExpenseRatio"])]),
        gross_expense_ratio: raw(&fees["grossExpRatio"]),
        front_load: raw(&fees["frontEndSalesLoad"]),
        deferred_load: raw(&fees["deferredSalesLoad"]),
        twelve_b1: raw(&fees["twelveBOne"]),
        // Yahoo da 0 cuando no sabe (QQQ): un 0 % de rotación no es un dato.
        turnover: first(&[raw(&fees["annualHoldingsTurnover"]), raw(&ks["annualHoldingsTurnover"])]).filter(|v| *v > 0.0),
        total_assets: first(&[raw(&ks["totalAssets"]), raw(&sd["totalAssets"])]).filter(|v| *v > 0.0),
        dividend_yield: first(&[raw(&ks["yield"]), raw(&sd["yield"]), raw(&sd["dividendYield"])]),
        beta_3y: raw(&ks["beta3Year"]),
        ytd_return: raw(&ks["ytdReturn"]),
        three_year_return: raw(&ks["threeYearAverageReturn"]),
        five_year_return: raw(&ks["fiveYearAverageReturn"]),
        top_holdings: th["holdings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some(Holding {
                    symbol: h["symbol"].as_str().map(str::to_string).filter(|s| !s.is_empty()),
                    name: h["holdingName"].as_str()?.to_string(),
                    weight: raw(&h["holdingPercent"])?,
                })
            })
            .take(MAX_HOLDINGS)
            .collect(),
        sector_weights: weights(&th["sectorWeightings"]),
        bond_ratings: weights(&th["bondRatings"]),
        source: "yahoo".into(),
        as_of: Some(now),
        ..FundProfile::default()
    };

    p.asset_mix = [
        ("stock", "stockPosition"),
        ("bond", "bondPosition"),
        ("cash", "cashPosition"),
        ("preferred", "preferredPosition"),
        ("convertible", "convertiblePosition"),
        ("other", "otherPosition"),
    ]
    .iter()
    .filter_map(|(name, key)| Some(WeightedItem { name: (*name).into(), weight: raw(&th[*key])? }))
    .filter(|w| w.weight > 0.0)
    .collect();
    p.asset_mix.sort_by(|a, b| b.weight.total_cmp(&a.weight));

    let eq = &th["equityHoldings"];
    let equity = FundEquityStats {
        price_to_earnings: invert(raw(&eq["priceToEarnings"])),
        price_to_book: invert(raw(&eq["priceToBook"])),
        price_to_sales: invert(raw(&eq["priceToSales"])),
        price_to_cashflow: invert(raw(&eq["priceToCashflow"])),
        three_year_earnings_growth: raw(&eq["threeYearEarningsGrowth"]),
        median_market_cap: raw(&eq["medianMarketCap"]).filter(|v| *v > 0.0),
    };
    if equity != FundEquityStats::default() {
        p.equity = Some(equity);
    }
    let bh = &th["bondHoldings"];
    let mut bond = FundBondStats {
        duration: raw(&bh["duration"]).filter(|v| *v > 0.0),
        maturity: raw(&bh["maturity"]).filter(|v| *v > 0.0),
        credit_quality: raw(&bh["creditQuality"]).filter(|v| *v > 0.0),
    };
    // Control de coherencia: en la taxonomía de Morningstar «Long» (largo plazo) significa duración
    // efectiva > 6 años. Yahoo da para TLT (20+ años) una duración de 3,6 y un vencimiento de 7,3:
    // contradicen la propia categoría y el folleto, así que no se presentan como dato.
    if p.category.as_deref().is_some_and(|c| c.to_ascii_lowercase().starts_with("long")) && bond.duration.is_some_and(|d| d <= 6.0) {
        bond.duration = None;
        bond.maturity = None;
    }
    if bond != FundBondStats::default() {
        p.bond = Some(bond);
    }

    // Sin nada propio de un fondo (p. ej. una acción): no hay perfil.
    let empty = p.family.is_none()
        && p.category.is_none()
        && p.expense_ratio.is_none()
        && p.total_assets.is_none()
        && p.top_holdings.is_empty()
        && p.sector_weights.is_empty()
        && p.inception_date.is_none();
    (!empty).then_some(p)
}
