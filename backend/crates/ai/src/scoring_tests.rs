//! Tests de `scoring` (ver ese módulo para el diseño de las puntuaciones).

use super::*;
use crate::fixture::{self, now};
use chrono::Duration;
use domain::{MacroIndicator, MarketState, Sentiment, SentimentLabel};

fn stock() -> AssetSnapshot {
    fixture::stock_snapshot()
}
fn sc(s: &AssetSnapshot) -> Scorecard {
    score(s, Lang::Es)
}
fn ind(s: &mut AssetSnapshot) -> &mut Indicators {
    s.indicators.as_mut().unwrap()
}
fn fun(s: &mut AssetSnapshot) -> &mut Fundamentals {
    s.fundamentals.as_mut().unwrap()
}
fn tech(s: &AssetSnapshot) -> f32 {
    sc(s).scores.technical.unwrap()
}
fn news_item(hours_ago: i64, title: &str, sentiment: Option<f32>) -> NewsItem {
    NewsItem {
        id: format!("{hours_ago}-{title}"),
        title: title.into(),
        summary: None,
        url: "https://x.test".into(),
        publisher: None,
        published_at: now() - Duration::hours(hours_ago),
        image_url: None,
        tickers: vec![],
        sentiment: sentiment.map(|s| Sentiment {
            score: s,
            label: SentimentLabel::Neutral,
            method: "test".into(),
        }),
        provider: "t".into(),
        language: None,
    }
}
fn macro_ind(key: &str, value: f64, change: Option<f64>, unit: &str) -> MacroIndicator {
    MacroIndicator {
        key: key.into(),
        name: key.into(),
        value,
        change,
        unit: unit.into(),
        as_of: now(),
        source: "t".into(),
    }
}

// ---- utilidades -------------------------------------------------------------------------

#[test]
fn piecewise_interpolates_and_clamps() {
    let pts = [(0.0, 10.0), (10.0, 50.0), (20.0, 60.0)];
    assert_eq!(piecewise(-5.0, &pts), 10.0);
    assert_eq!(piecewise(0.0, &pts), 10.0);
    assert_eq!(piecewise(5.0, &pts), 30.0);
    assert_eq!(piecewise(10.0, &pts), 50.0);
    assert_eq!(piecewise(15.0, &pts), 55.0);
    assert_eq!(piecewise(99.0, &pts), 60.0);
}

#[test]
fn parts_weighted_mean_and_empty() {
    let mut p = Parts::default();
    p.add(1.0, 20.0, "a".into());
    p.add(3.0, 100.0, "b".into());
    let (s, reasons) = p.finish().unwrap();
    assert_eq!(s, 80.0);
    assert_eq!(reasons.len(), 2);
    assert!(reasons[0].starts_with("a → 20"));
    assert!(Parts::default().finish().is_none());
    // Las puntuaciones fuera de rango se recortan.
    let mut p = Parts::default();
    p.add(1.0, 250.0, "x".into());
    assert_eq!(p.finish().unwrap().0, 100.0);
}

// ---- estructura general ------------------------------------------------------------------

#[test]
fn full_stock_scores_every_component_with_reasons() {
    let c = sc(&stock());
    let s = &c.scores;
    for v in
        [s.technical, s.momentum, s.valuation, s.fundamental, s.sentiment, s.macro_score, s.risk]
    {
        let v = v.expect("componente con datos");
        assert!((0.0..=100.0).contains(&v));
    }
    assert!((0.0..=100.0).contains(&s.overall));
    let r = &c.reasons;
    for list in [
        &r.technical, &r.momentum, &r.valuation, &r.fundamental, &r.sentiment, &r.macro_score,
        &r.risk,
    ] {
        assert!(!list.is_empty());
        assert!(list.iter().all(|x| x.contains('→')), "{list:?}");
    }
    assert_eq!(c.coverage, 1.0);
    assert!(c.gaps.is_empty());
    assert!(!c.verdict_reasons.is_empty());
    assert!((0.1..=0.85).contains(&c.confidence));
}

#[test]
fn scoring_is_deterministic() {
    let s = stock();
    let a = serde_json::to_string(&sc(&s)).unwrap();
    let b = serde_json::to_string(&sc(&s)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn english_reasons_differ_from_spanish() {
    let s = stock();
    let es = score(&s, Lang::Es);
    let en = score(&s, Lang::En);
    assert_eq!(es.scores.overall, en.scores.overall);
    assert!(es.reasons.technical[0].contains("Tendencia"));
    assert!(en.reasons.technical[0].contains("Trend"));
}

#[test]
fn overall_ignores_missing_components_and_reports_coverage() {
    let mut s = stock();
    s.fundamentals = None;
    s.news.clear();
    s.macro_context = None;
    let c = sc(&s);
    assert!(c.scores.valuation.is_none() && c.scores.fundamental.is_none());
    assert!(c.scores.sentiment.is_none() && c.scores.macro_score.is_none());
    // Solo quedan técnico, momentum y riesgo (con el ATR y la capitalización de la cotización).
    let (t, m, r) = (
        c.scores.technical.unwrap() as f64,
        c.scores.momentum.unwrap() as f64,
        c.scores.risk.unwrap() as f64,
    );
    let w = weights(AssetClass::Stock);
    let expect = (w[0] * t + w[1] * m + w[6] * (100.0 - r)) / (w[0] + w[1] + w[6]);
    assert!((c.scores.overall as f64 - expect).abs() < 0.11, "{} vs {}", c.scores.overall, expect);
    assert!(c.coverage < 0.7 && c.coverage > 0.4);
    assert!(c.gaps.iter().any(|g| g.contains("Valoración")));
    assert!(c.gaps.iter().any(|g| g.contains("Fundamentales")));
}

#[test]
fn only_quote_gives_no_basis() {
    let c = sc(&fixture::sparse_snapshot());
    assert_eq!(c.coverage, 0.0);
    assert_eq!(c.scores.overall, 50.0);
    assert_eq!(c.verdict, Verdict::Wait);
    assert_eq!(c.confidence, 0.1);
    assert_eq!(c.gaps.len(), 7);
}

#[test]
fn non_finite_inputs_are_ignored() {
    let mut s = stock();
    ind(&mut s).rsi14 = Some(f64::NAN);
    ind(&mut s).sma200 = Some(f64::INFINITY);
    ind(&mut s).volatility30d = Some(f64::NAN);
    fun(&mut s).pe_forward = Some(f64::NAN);
    fun(&mut s).pe_trailing = Some(f64::INFINITY);
    s.quote.as_mut().unwrap().price = f64::NAN;
    let c = sc(&s);
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains("NaN") && !json.contains("null,\"x\""));
    assert!(c.scores.overall.is_finite());
    assert!(c.confidence.is_finite());
}

#[test]
fn scores_stay_in_range_for_random_inputs() {
    // Fuzz determinista: valores extremos en todos los campos numéricos.
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = move |lo: f64, hi: f64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        lo + (hi - lo) * ((seed >> 33) as f64 / (1u64 << 31) as f64)
    };
    for class in [
        AssetClass::Stock,
        AssetClass::Crypto,
        AssetClass::Etf,
        AssetClass::Forex,
        AssetClass::Bond,
    ] {
        for _ in 0..200 {
            let mut s = stock();
            s.summary.class = class;
            let px = rnd(0.0001, 5e5);
            s.quote.as_mut().unwrap().price = px;
            let i = ind(&mut s);
            i.rsi14 = Some(rnd(-20.0, 140.0));
            i.sma50 = Some(rnd(0.001, 1e6));
            i.sma200 = Some(rnd(0.001, 1e6));
            i.macd_hist = Some(rnd(-1e4, 1e4));
            i.volatility30d = Some(rnd(-1.0, 20.0));
            i.max_drawdown1y = Some(rnd(-3.0, 3.0));
            i.return1m = Some(rnd(-5.0, 50.0));
            i.return3m = Some(rnd(-5.0, 50.0));
            i.return1y = Some(rnd(-5.0, 500.0));
            i.supports = vec![rnd(0.0, 1e6), rnd(-5.0, 10.0)];
            i.resistances = vec![rnd(0.0, 1e6)];
            let f = fun(&mut s);
            f.pe_forward = Some(rnd(-100.0, 1e4));
            f.peg = Some(rnd(-10.0, 100.0));
            f.debt_to_equity = Some(rnd(-5.0, 1e4));
            f.beta = Some(rnd(-20.0, 50.0));
            f.market_cap = Some(rnd(0.0, 1e14));
            f.analyst_rating_mean = Some(rnd(-3.0, 9.0));
            let c = sc(&s);
            let all = [
                c.scores.overall,
                c.scores.technical.unwrap_or(50.0),
                c.scores.momentum.unwrap_or(50.0),
                c.scores.valuation.unwrap_or(50.0),
                c.scores.fundamental.unwrap_or(50.0),
                c.scores.sentiment.unwrap_or(50.0),
                c.scores.macro_score.unwrap_or(50.0),
                c.scores.risk.unwrap_or(50.0),
            ];
            assert!(
                all.iter().all(|v| v.is_finite() && (0.0..=100.0).contains(v)),
                "{class:?} {all:?}"
            );
            assert!((0.0..=1.0).contains(&c.confidence) && (0.0..=1.0).contains(&c.coverage));
        }
    }
}

// ---- técnico -------------------------------------------------------------------------------

#[test]
fn technical_trend_ordering() {
    let mut values = vec![];
    for t in [Trend::StrongDown, Trend::Down, Trend::Sideways, Trend::Up, Trend::StrongUp] {
        let mut s = stock();
        ind(&mut s).trend = Some(t);
        values.push(tech(&s));
    }
    assert!(values.windows(2).all(|w| w[0] < w[1]), "{values:?}");
}

#[test]
fn technical_penalises_overbought_rsi_and_rewards_oversold() {
    let rsi = |v: f64| {
        let mut s = stock();
        ind(&mut s).rsi14 = Some(v);
        tech(&s)
    };
    assert!(rsi(85.0) < rsi(55.0));
    assert!(rsi(25.0) > rsi(55.0));
    assert!(rsi(90.0) < rsi(75.0));
    let mut s = stock();
    ind(&mut s).rsi14 = Some(82.0);
    let c = sc(&s);
    assert!(
        c.reasons.technical.iter().any(|r| r.contains("RSI14 = 82.0") && r.contains("sobrecompra"))
    );
}

#[test]
fn technical_macd_and_sma_signals() {
    let mut bull = stock();
    ind(&mut bull).macd_hist = Some(2.0);
    let mut bear = stock();
    ind(&mut bear).macd_hist = Some(-2.0);
    assert!(tech(&bull) > tech(&bear));

    // Solo macd y señal: el histograma se deriva.
    let mut derived = stock();
    ind(&mut derived).macd_hist = None;
    ind(&mut derived).macd = Some(-1.0);
    ind(&mut derived).macd_signal = Some(1.0);
    assert!(
        sc(&derived)
            .reasons
            .technical
            .iter()
            .any(|r| r.contains("MACD histograma -2") && r.contains("bajista"))
    );

    // Precio bajo la SMA200 y cruz de la muerte.
    let mut below = stock();
    below.quote.as_mut().unwrap().price = 180.0;
    ind(&mut below).sma50 = Some(200.0);
    ind(&mut below).sma200 = Some(215.0);
    ind(&mut below).trend = Some(Trend::Down);
    let c = sc(&below);
    assert!(tech(&below) < tech(&stock()) - 10.0);
    assert!(c.reasons.technical.iter().any(|r| r.contains("cruz de la muerte")));
}

#[test]
fn technical_overextension_is_not_rewarded() {
    let far = |mult: f64| {
        let mut s = stock();
        s.quote.as_mut().unwrap().price = 221.5 * mult;
        ind(&mut s).trend = Some(Trend::Up);
        tech(&s)
    };
    // +10 % sobre la SMA50 es mejor que +40 %.
    assert!(far(1.10) > far(1.40));
}

#[test]
fn technical_reward_risk_uses_nearest_levels() {
    let mut near_res = stock();
    near_res.quote.as_mut().unwrap().price = 230.0;
    ind(&mut near_res).supports = vec![200.0];
    ind(&mut near_res).resistances = vec![231.0];
    let mut room = near_res.clone();
    ind(&mut room).resistances = vec![270.0];
    assert!(tech(&room) > tech(&near_res));
    assert!(sc(&room).reasons.technical.iter().any(|r| r.contains("recompensa/riesgo")));
    // Sin soporte por debajo del precio no hay ratio.
    let mut none = near_res.clone();
    ind(&mut none).supports = vec![300.0];
    assert!(!sc(&none).reasons.technical.iter().any(|r| r.contains("recompensa/riesgo")));
}

#[test]
fn trend_is_derived_when_provider_omits_it() {
    let mut s = stock();
    ind(&mut s).trend = None;
    let c = sc(&s);
    assert!(c.reasons.technical[0].starts_with("Tendencia alcista"));
    let mut d = stock();
    ind(&mut d).trend = None;
    d.quote.as_mut().unwrap().price = 150.0;
    assert!(sc(&d).reasons.technical[0].starts_with("Tendencia bajista"));
}

#[test]
fn missing_indicators_leave_technical_momentum_empty() {
    let mut s = stock();
    s.indicators = None;
    let c = sc(&s);
    assert!(c.scores.technical.is_none() && c.scores.momentum.is_none());
    assert!(c.gaps.iter().any(|g| g.contains("Técnico")));
    // El riesgo se sostiene con beta y capitalización.
    assert!(c.scores.risk.is_some());
}

// ---- momentum ------------------------------------------------------------------------------

#[test]
fn momentum_rewards_returns_and_scales_by_class() {
    let with = |class: AssetClass, r1m: f64, r3m: f64, r1y: f64| {
        let mut s = stock();
        s.summary.class = class;
        let i = ind(&mut s);
        i.return1m = Some(r1m);
        i.return3m = Some(r3m);
        i.return1y = Some(r1y);
        i.dist_from_high52w = None;
        sc(&s).scores.momentum.unwrap()
    };
    assert!(with(AssetClass::Stock, 0.10, 0.25, 0.40) > with(AssetClass::Stock, 0.0, 0.0, 0.0));
    assert!(with(AssetClass::Stock, -0.15, -0.25, -0.40) < 25.0);
    // El mismo +30 % trimestral es una gran noticia en un ETF y algo normal en cripto.
    assert!(with(AssetClass::Etf, 0.0, 0.30, 0.0) > with(AssetClass::Crypto, 0.0, 0.30, 0.0));
}

#[test]
fn momentum_penalises_parabolic_moves_slightly() {
    let m3 = |r: f64| {
        let mut s = stock();
        let i = ind(&mut s);
        i.return1m = None;
        i.return1y = None;
        i.dist_from_high52w = None;
        i.return3m = Some(r);
        sc(&s).scores.momentum.unwrap()
    };
    assert!(m3(0.30) > m3(0.10));
    assert!(m3(0.30) > m3(1.50));
}

#[test]
fn distance_to_high_accepts_either_sign() {
    let mut a = stock();
    ind(&mut a).dist_from_high52w = Some(-0.05);
    let mut b = stock();
    ind(&mut b).dist_from_high52w = Some(0.05);
    assert_eq!(sc(&a).scores.momentum, sc(&b).scores.momentum);
}

// ---- valoración ----------------------------------------------------------------------------

fn val_only(f: impl FnOnce(&mut Fundamentals)) -> f32 {
    let mut s = stock();
    let fu = fun(&mut s);
    fu.pe_forward = None;
    fu.pe_trailing = None;
    fu.peg = None;
    fu.price_to_sales = None;
    fu.ev_to_ebitda = None;
    fu.free_cash_flow = None;
    f(fu);
    sc(&s).scores.valuation.unwrap()
}

#[test]
fn valuation_uses_sector_thresholds() {
    let pe = |sector: &str| {
        val_only(|f| {
            f.sector = Some(sector.into());
            f.pe_forward = Some(28.0);
        })
    };
    // Un PER de 28 es razonable en tecnología y caro en utilities o energía.
    assert!(pe("Technology") > pe("Utilities") + 15.0);
    assert!(pe("Technology") > pe("Energy") + 15.0);
    // Aliases GICS.
    assert_eq!(pe("Information Technology"), pe("Technology"));
}

#[test]
fn valuation_cheap_beats_expensive_for_each_multiple() {
    assert!(val_only(|f| f.pe_forward = Some(9.0)) > val_only(|f| f.pe_forward = Some(45.0)));
    assert!(val_only(|f| f.peg = Some(0.7)) > val_only(|f| f.peg = Some(2.8)));
    assert!(
        val_only(|f| f.price_to_sales = Some(1.5)) > val_only(|f| f.price_to_sales = Some(20.0))
    );
    assert!(val_only(|f| f.ev_to_ebitda = Some(8.0)) > val_only(|f| f.ev_to_ebitda = Some(40.0)));
    assert!(
        val_only(|f| {
            f.free_cash_flow = Some(120e9);
            f.market_cap = Some(1.0e12)
        }) > val_only(|f| {
            f.free_cash_flow = Some(5e9);
            f.market_cap = Some(1.0e12)
        })
    );
}

#[test]
fn valuation_prefers_forward_pe_and_handles_negative_earnings() {
    let mut s = stock();
    let f = fun(&mut s);
    f.pe_forward = Some(15.0);
    f.pe_trailing = Some(60.0);
    let c = sc(&s);
    assert!(c.reasons.valuation[0].contains("PER forward = 15"));

    let neg = val_only(|f| {
        f.pe_trailing = Some(-12.0);
    });
    assert!(neg < 20.0, "{neg}");
    // PEG <= 0 no es significativo: se ignora en lugar de premiarlo.
    let with = val_only(|f| {
        f.pe_forward = Some(20.0);
        f.peg = Some(-1.0);
    });
    let without = val_only(|f| f.pe_forward = Some(20.0));
    assert_eq!(with, without);
}

#[test]
fn valuation_financials_use_book_value_not_ebitda() {
    let mut s = stock();
    let f = fun(&mut s);
    f.sector = Some("Financial Services".into());
    f.pe_forward = Some(10.0);
    f.price_to_book = Some(1.0);
    f.ev_to_ebitda = Some(50.0);
    f.price_to_sales = Some(30.0);
    let c = sc(&s);
    let joined = c.reasons.valuation.join(" | ");
    assert!(joined.contains("P/B"), "{joined}");
    assert!(!joined.contains("EV/EBITDA") && !joined.contains("P/S"), "{joined}");
}

#[test]
fn valuation_etf_includes_cost() {
    let mut s = stock();
    s.summary.class = AssetClass::Etf;
    s.fundamentals = Some(Fundamentals {
        expense_ratio: Some(0.0003),
        pe_trailing: Some(21.0),
        ..Default::default()
    });
    let cheap = sc(&s);
    assert!(cheap.reasons.valuation.iter().any(|r| r.contains("TER")));
    let mut dear = s.clone();
    fun(&mut dear).expense_ratio = Some(0.012);
    assert!(cheap.scores.valuation.unwrap() > sc(&dear).scores.valuation.unwrap());
    // Un 0.03 «en porcentaje» (0,03 %) no se confunde con 3 %.
    let mut pct = s.clone();
    fun(&mut pct).expense_ratio = Some(0.2);
    assert!(sc(&pct).reasons.valuation.iter().any(|r| r.contains("0.2%")));
}

#[test]
fn crypto_valuation_ath_and_fdv() {
    let at = |frac: f64, fdv_ratio: f64| {
        let mut s = fixture::crypto_snapshot();
        let ath = 100_000.0;
        s.quote.as_mut().unwrap().price = ath * frac;
        let f = fun(&mut s);
        f.all_time_high = Some(ath);
        f.market_cap = Some(1e11);
        f.fully_diluted_valuation = Some(1e11 * fdv_ratio);
        sc(&s)
    };
    // Cerca del ATH es menos atractivo que un 30 % del ATH (descuento) con igual dilución.
    assert!(at(0.30, 1.0).scores.valuation.unwrap() > at(0.99, 1.0).scores.valuation.unwrap());
    // Mucha dilución pendiente penaliza.
    assert!(at(0.5, 1.0).scores.valuation.unwrap() > at(0.5, 5.0).scores.valuation.unwrap());
    let c = at(0.5, 1.0);
    assert!(c.reasons.valuation.iter().any(|r| r.contains("máximo histórico")));
    assert!(c.reasons.valuation.iter().any(|r| r.contains("FDV / market cap")));
    // Sin FDV se usa oferta máxima / circulante.
    let mut s = fixture::crypto_snapshot();
    let f = fun(&mut s);
    f.fully_diluted_valuation = None;
    f.max_supply = Some(21e6);
    f.circulating_supply = Some(10.5e6);
    assert!(sc(&s).reasons.valuation.iter().any(|r| r.contains("oferta máx./circulante = 2.00x")));
}

#[test]
fn crypto_has_no_fundamentals_and_that_is_not_a_gap() {
    let c = sc(&fixture::crypto_snapshot());
    assert!(c.scores.fundamental.is_none());
    assert!(!c.gaps.iter().any(|g| g.contains("Fundamentales")));
    assert_eq!(c.coverage, 1.0);
    assert!(c.scores.valuation.is_some());
}

// ---- fundamentales -------------------------------------------------------------------------

fn fund_with(f: impl FnOnce(&mut Fundamentals)) -> f32 {
    let mut s = stock();
    let fu = fun(&mut s);
    *fu = Fundamentals { sector: fu.sector.clone(), ..Default::default() };
    f(fu);
    sc(&s).scores.fundamental.unwrap()
}

#[test]
fn fundamentals_reward_quality() {
    let strong = fund_with(|f| {
        f.operating_margin = Some(0.35);
        f.profit_margin = Some(0.25);
        f.roe = Some(0.30);
        f.revenue_growth_yoy = Some(0.30);
        f.earnings_growth_yoy = Some(0.40);
        f.debt_to_equity = Some(0.2);
        f.free_cash_flow = Some(30e9);
        f.revenue_ttm = Some(100e9);
    });
    let weak = fund_with(|f| {
        f.operating_margin = Some(-0.05);
        f.profit_margin = Some(-0.10);
        f.roe = Some(-0.10);
        f.revenue_growth_yoy = Some(-0.10);
        f.earnings_growth_yoy = Some(-0.50);
        f.debt_to_equity = Some(5.0);
        f.free_cash_flow = Some(-8e9);
        f.revenue_ttm = Some(100e9);
    });
    assert!(strong > 80.0, "{strong}");
    assert!(weak < 15.0, "{weak}");
}

#[test]
fn debt_to_equity_percent_heuristic() {
    // 150 (%) == 1,5x
    assert_eq!(
        fund_with(|f| f.debt_to_equity = Some(150.0)),
        fund_with(|f| f.debt_to_equity = Some(1.5))
    );
    assert!(
        fund_with(|f| f.debt_to_equity = Some(0.2)) > fund_with(|f| f.debt_to_equity = Some(3.0))
    );
}

#[test]
fn fundamentals_financials_skip_leverage_metrics() {
    let mut s = stock();
    let f = fun(&mut s);
    f.sector = Some("Financial Services".into());
    f.debt_to_equity = Some(9.0);
    f.free_cash_flow = Some(-1e9);
    f.current_ratio = Some(0.2);
    let c = sc(&s);
    let joined = c.reasons.fundamental.join(" | ");
    assert!(
        !joined.contains("Deuda") && !joined.contains("liquidez") && !joined.contains("FCF"),
        "{joined}"
    );
    assert!(joined.contains("ROE"));
}

#[test]
fn net_cash_and_fcf_without_revenue() {
    let mut s = stock();
    let f = fun(&mut s);
    *f = Fundamentals {
        sector: f.sector.clone(),
        total_cash: Some(100e9),
        total_debt: Some(40e9),
        free_cash_flow: Some(-3e9),
        ..Default::default()
    };
    let c = sc(&s);
    assert!(c.reasons.fundamental.iter().any(|r| r.contains("Caja neta")));
    assert!(c.reasons.fundamental.iter().any(|r| r.contains("Flujo de caja libre negativo")));
}

#[test]
fn no_fundamentals_for_indices_forex_commodities() {
    for class in [AssetClass::Index, AssetClass::Forex, AssetClass::Commodity, AssetClass::Crypto] {
        let mut s = stock();
        s.summary.class = class;
        assert!(sc(&s).scores.fundamental.is_none(), "{class:?}");
    }
}

// ---- sentimiento ---------------------------------------------------------------------------

fn sent_only(news: Vec<NewsItem>) -> Option<f32> {
    let mut s = stock();
    s.fundamentals = None;
    s.news = news;
    sc(&s).scores.sentiment
}

#[test]
fn sentiment_none_without_news_or_consensus() {
    assert!(sent_only(vec![]).is_none());
}

#[test]
fn sentiment_direction_and_recency() {
    let pos = sent_only(vec![
        news_item(2, "a", Some(0.8)),
        news_item(5, "b", Some(0.7)),
        news_item(9, "c", Some(0.6)),
    ])
    .unwrap();
    let neg = sent_only(vec![
        news_item(2, "a", Some(-0.8)),
        news_item(5, "b", Some(-0.7)),
        news_item(9, "c", Some(-0.6)),
    ])
    .unwrap();
    assert!(pos > 70.0 && neg < 30.0, "{pos} {neg}");
    // Una noticia negativa de hoy pesa más que tres positivas de hace 10 días.
    let mixed = sent_only(vec![
        news_item(1, "hoy malo", Some(-0.9)),
        news_item(240, "viejo 1", Some(0.8)),
        news_item(241, "viejo 2", Some(0.8)),
        news_item(242, "viejo 3", Some(0.8)),
    ])
    .unwrap();
    assert!(mixed < 50.0, "{mixed}");
}

#[test]
fn sentiment_ignores_stale_news_and_shrinks_for_few_items() {
    // Pasadas dos semanas no cuentan.
    assert!(sent_only(vec![news_item(24 * 15, "viejo", Some(0.9))]).is_none());
    // Con una sola noticia la señal pesa menos que con diez iguales (misma media).
    let mut s = stock();
    s.fundamentals = Some(Fundamentals {
        analyst_rating_mean: Some(5.0),
        analyst_count: Some(20),
        ..Default::default()
    });
    s.news = vec![news_item(1, "uno", Some(0.9))];
    let few = sc(&s).scores.sentiment.unwrap();
    s.news = (0..10).map(|i| news_item(1 + i, &format!("n{i}"), Some(0.9))).collect();
    let many = sc(&s).scores.sentiment.unwrap();
    assert!(many > few, "{many} vs {few}");
}

#[test]
fn sentiment_falls_back_to_lexicon() {
    let good = sent_only(vec![news_item(3, "Shares surge on record profit", None)]).unwrap();
    let bad = sent_only(vec![news_item(3, "Shares plunge amid fraud probe", None)]).unwrap();
    assert!(good > 55.0 && bad < 45.0, "{good} {bad}");
}

#[test]
fn analyst_consensus_feeds_sentiment() {
    let mut s = stock();
    s.news.clear();
    fun(&mut s).analyst_rating_mean = Some(1.4);
    fun(&mut s).analyst_target_mean = Some(300.0);
    let hi = sc(&s).scores.sentiment.unwrap();
    fun(&mut s).analyst_rating_mean = Some(4.2);
    fun(&mut s).analyst_target_mean = Some(190.0);
    let lo = sc(&s).scores.sentiment.unwrap();
    assert!(hi > 75.0 && lo < 30.0, "{hi} {lo}");
}

// ---- macro ---------------------------------------------------------------------------------

fn macro_with(inds: Vec<MacroIndicator>) -> Option<f32> {
    let mut s = stock();
    s.macro_context = Some(MacroContext { indicators: inds, as_of: Some(now()) });
    sc(&s).scores.macro_score
}

#[test]
fn macro_inverted_curve_and_fear_are_bearish() {
    let calm = macro_with(vec![
        macro_ind("curve_10y2y", 1.0, None, "pp"),
        macro_ind("vix", 13.0, None, "pts"),
    ])
    .unwrap();
    let stress = macro_with(vec![
        macro_ind("curve_10y2y", -0.8, None, "pp"),
        macro_ind("vix", 32.0, None, "pts"),
    ])
    .unwrap();
    assert!(calm > 60.0 && stress < 35.0, "{calm} {stress}");
}

#[test]
fn macro_rate_direction_and_units() {
    let r =
        |ch: f64, unit: &str| macro_with(vec![macro_ind("us10y", 4.2, Some(ch), unit)]).unwrap();
    assert!(r(-0.25, "%") > r(0.25, "%"));
    // 25 pb == 0,25 pp
    assert_eq!(r(25.0, "bps"), r(0.25, "%"));
}

#[test]
fn macro_curve_derived_from_yields_and_missing_keys() {
    let s = macro_with(vec![macro_ind("us10y", 3.5, None, "%"), macro_ind("us2y", 4.3, None, "%")])
        .unwrap();
    let mut st = stock();
    st.macro_context = Some(MacroContext {
        indicators: vec![macro_ind("us10y", 3.5, None, "%"), macro_ind("us2y", 4.3, None, "%")],
        as_of: None,
    });
    assert!(sc(&st).reasons.macro_score[0].contains("-0.80"));
    assert!(s < 40.0);
    assert!(macro_with(vec![macro_ind("unemployment", 4.0, None, "%")]).is_none());
    assert!(macro_with(vec![]).is_none());
    let mut none = stock();
    none.macro_context = None;
    assert!(sc(&none).scores.macro_score.is_none());
}

#[test]
fn macro_high_inflation_hurts() {
    let cpi = |v: f64| macro_with(vec![macro_ind("cpi_yoy", v, None, "%")]).unwrap();
    assert!(cpi(2.3) > cpi(7.5));
}

// ---- riesgo --------------------------------------------------------------------------------

#[test]
fn risk_grows_with_volatility_drawdown_beta_and_small_cap() {
    let risk = |f: &dyn Fn(&mut AssetSnapshot)| {
        let mut s = stock();
        f(&mut s);
        sc(&s).scores.risk.unwrap()
    };
    let base = risk(&|_| {});
    assert!(risk(&|s| ind(s).volatility30d = Some(0.9)) > base + 5.0);
    assert!(risk(&|s| ind(s).max_drawdown1y = Some(-0.6)) > base + 5.0);
    assert!(risk(&|s| fun(s).beta = Some(2.2)) > base + 3.0);
    assert!(
        risk(&|s| {
            fun(s).market_cap = Some(2e8);
        }) > base + 5.0
    );
    // El signo de la caída máxima da igual.
    assert_eq!(
        risk(&|s| ind(s).max_drawdown1y = Some(0.3)),
        risk(&|s| ind(s).max_drawdown1y = Some(-0.3))
    );
}

#[test]
fn crypto_is_riskier_than_large_cap_stock_and_risk_is_inverted_in_overall() {
    let stock_risk = sc(&stock()).scores.risk.unwrap();
    let crypto_risk = sc(&fixture::crypto_snapshot()).scores.risk.unwrap();
    assert!(crypto_risk > stock_risk);
    let mut calm = stock();
    ind(&mut calm).volatility30d = Some(0.10);
    ind(&mut calm).max_drawdown1y = Some(-0.04);
    let mut wild = stock();
    ind(&mut wild).volatility30d = Some(1.1);
    ind(&mut wild).max_drawdown1y = Some(-0.7);
    assert!(sc(&calm).scores.overall > sc(&wild).scores.overall);
}

// ---- veredicto -----------------------------------------------------------------------------

fn verdict_for(
    overall: f32,
    tweak: impl FnOnce(&mut ScoreBreakdown, &mut Indicators),
    coverage: f64,
) -> (Verdict, Vec<String>) {
    let mut b =
        ScoreBreakdown { overall, technical: Some(60.0), risk: Some(40.0), ..Default::default() };
    let mut i = Indicators { rsi14: Some(55.0), sma50: Some(100.0), ..Default::default() };
    tweak(&mut b, &mut i);
    let ctx = Ctx {
        l: Lang::Es,
        class: AssetClass::Stock,
        price: Some(105.0),
        ind: Some(&i),
        fun: None,
        news: &[],
        macro_ctx: None,
        now: now(),
        market_cap: None,
    };
    quantitative_verdict(&b, &ctx, coverage)
}

#[test]
fn verdict_thresholds() {
    let v = |o: f32| verdict_for(o, |_, _| {}, 1.0).0;
    assert_eq!(v(80.0), Verdict::Buy);
    assert_eq!(v(70.0), Verdict::Buy);
    assert_eq!(v(65.0), Verdict::Accumulate);
    assert_eq!(v(55.0), Verdict::Hold);
    assert_eq!(v(45.0), Verdict::Wait);
    assert_eq!(v(35.0), Verdict::Reduce);
    assert_eq!(v(20.0), Verdict::Avoid);
}

#[test]
fn verdict_adjustments() {
    // Técnica débil con puntuación media: esperar en vez de mantener.
    let (v, why) = verdict_for(55.0, |b, _| b.technical = Some(30.0), 1.0);
    assert_eq!(v, Verdict::Wait);
    assert!(why.iter().any(|w| w.contains("Señal técnica débil")));
    // Sobreextendido: baja un escalón.
    assert_eq!(verdict_for(75.0, |_, i| i.rsi14 = Some(80.0), 1.0).0, Verdict::Accumulate);
    assert_eq!(verdict_for(65.0, |_, i| i.rsi14 = Some(80.0), 1.0).0, Verdict::Wait);
    assert_eq!(verdict_for(75.0, |_, i| i.sma50 = Some(80.0), 1.0).0, Verdict::Accumulate);
    // Riesgo muy alto: compra escalonada.
    assert_eq!(verdict_for(75.0, |b, _| b.risk = Some(85.0), 1.0).0, Verdict::Accumulate);
    // Pocos datos: nunca comprar ni vender con convicción.
    assert_eq!(verdict_for(80.0, |_, _| {}, 0.3).0, Verdict::Hold);
    assert_eq!(verdict_for(20.0, |_, _| {}, 0.3).0, Verdict::Wait);
    assert_eq!(verdict_for(80.0, |_, _| {}, 0.0).0, Verdict::Wait);
}

#[test]
fn verdict_ranks_are_ordered() {
    let order = [
        Verdict::Avoid,
        Verdict::Reduce,
        Verdict::Wait,
        Verdict::Hold,
        Verdict::Accumulate,
        Verdict::Buy,
    ];
    assert!(order.windows(2).all(|w| verdict_rank(w[0]) < verdict_rank(w[1])));
    assert_eq!(verdict_name(Lang::En, Verdict::Accumulate), "accumulate");
}

#[test]
fn strong_and_weak_assets_get_opposite_verdicts() {
    let mut strong = stock();
    {
        let i = ind(&mut strong);
        i.trend = Some(Trend::StrongUp);
        i.rsi14 = Some(58.0);
        i.macd_hist = Some(1.5);
        i.return1m = Some(0.08);
        i.return3m = Some(0.2);
        i.return1y = Some(0.4);
        i.volatility30d = Some(0.18);
        i.max_drawdown1y = Some(-0.08);
        i.sma50 = Some(222.0);
        i.sma200 = Some(190.0);
        i.supports = vec![224.0];
        i.resistances = vec![260.0];
    }
    {
        let f = fun(&mut strong);
        f.pe_forward = Some(17.0);
        f.peg = Some(0.9);
        f.price_to_sales = Some(3.0);
        f.ev_to_ebitda = Some(13.0);
        f.revenue_growth_yoy = Some(0.22);
        f.earnings_growth_yoy = Some(0.3);
        f.debt_to_equity = Some(0.3);
        f.beta = Some(0.9);
        f.analyst_rating_mean = Some(1.6);
        f.analyst_target_mean = Some(290.0);
    }
    strong.news =
        vec![news_item(2, "Shares surge as company beats estimates and raises guidance", None)];
    let s = sc(&strong);
    assert!(
        matches!(s.verdict, Verdict::Buy | Verdict::Accumulate),
        "{:?} {}",
        s.verdict,
        s.scores.overall
    );

    let mut weak = stock();
    {
        let i = ind(&mut weak);
        i.trend = Some(Trend::StrongDown);
        i.rsi14 = Some(38.0);
        i.macd_hist = Some(-2.0);
        i.return1m = Some(-0.15);
        i.return3m = Some(-0.3);
        i.return1y = Some(-0.5);
        i.volatility30d = Some(0.7);
        i.max_drawdown1y = Some(-0.6);
        i.sma50 = Some(210.0);
        i.sma200 = Some(260.0);
        i.supports = vec![150.0];
        i.resistances = vec![229.0];
    }
    {
        let f = fun(&mut weak);
        f.pe_forward = Some(70.0);
        f.peg = Some(3.5);
        f.price_to_sales = Some(15.0);
        f.ev_to_ebitda = Some(45.0);
        f.operating_margin = Some(-0.04);
        f.profit_margin = Some(-0.1);
        f.revenue_growth_yoy = Some(-0.12);
        f.debt_to_equity = Some(4.5);
        f.free_cash_flow = Some(-2e9);
        f.beta = Some(2.0);
        f.analyst_rating_mean = Some(4.0);
        f.analyst_target_mean = Some(190.0);
    }
    weak.news = vec![news_item(2, "Company under fraud investigation, shares plunge", None)];
    weak.macro_context = Some(MacroContext {
        indicators: vec![
            macro_ind("vix", 33.0, None, "pts"),
            macro_ind("curve_10y2y", -0.7, None, "pp"),
        ],
        as_of: None,
    });
    let w = sc(&weak);
    assert!(
        matches!(w.verdict, Verdict::Reduce | Verdict::Avoid),
        "{:?} {}",
        w.verdict,
        w.scores.overall
    );
    assert!(s.scores.overall > w.scores.overall + 30.0);
}

// ---- confianza -----------------------------------------------------------------------------

#[test]
fn confidence_depends_on_coverage_and_quote_quality() {
    let full = sc(&stock()).confidence;
    let mut partial = stock();
    partial.fundamentals = None;
    partial.news.clear();
    partial.macro_context = None;
    assert!(sc(&partial).confidence < full);
    let mut no_quote = stock();
    no_quote.quote = None;
    assert!(sc(&no_quote).confidence < full);
    let mut stale = stock();
    stale.quote.as_mut().unwrap().timestamp = now() - Duration::days(3);
    stale.quote.as_mut().unwrap().market_state = MarketState::Regular;
    assert!(sc(&stale).confidence < full);
}

#[test]
fn contradictory_signals_lower_confidence() {
    let mut mixed = stock();
    // Técnico/momentum muy alcistas y valoración/fundamentales muy flojos.
    ind(&mut mixed).trend = Some(Trend::StrongUp);
    ind(&mut mixed).return3m = Some(0.4);
    let f = fun(&mut mixed);
    f.pe_forward = Some(120.0);
    f.peg = Some(5.0);
    f.operating_margin = Some(-0.2);
    f.revenue_growth_yoy = Some(-0.2);
    let mut agree = stock();
    ind(&mut agree).trend = Some(Trend::StrongUp);
    assert!(sc(&mixed).confidence <= sc(&agree).confidence);
}
