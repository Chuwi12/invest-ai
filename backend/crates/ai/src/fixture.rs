//! Datos de prueba fijos y un `MarketData` falso. Lo usan los tests del crate y el benchmark
//! (`examples/bench.rs`): mismas entradas siempre, para que las comparaciones entre modelos
//! sean reproducibles.
//!
//! Las cifras son **sintéticas** (plausibles, pero inventadas) y llevan `source = "fixture"`.
//! No es una cotización real de ningún activo.

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use domain::*;
use futures::stream::{self, BoxStream};

/// «Ahora» de los fixtures: viernes 2026-10-09 15:30 UTC (mercado de EE. UU. abierto).
pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 9, 15, 30, 0).unwrap()
}

/// Generador congruencial lineal: paseo aleatorio determinista sin dependencias.
struct Lcg(u64);
impl Lcg {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
    }
}

/// `n` velas diarias (la última cierra en `end_price` y es la de `now()`), con deriva y ruido.
pub fn candles(seed: u64, n: usize, end_price: f64, daily_vol: f64) -> Vec<Candle> {
    let mut rng = Lcg(seed);
    let mut closes = vec![end_price; n];
    // Se construye hacia atrás desde el precio final con una deriva alcista suave.
    for i in (0..n.saturating_sub(1)).rev() {
        let shock = (rng.next_f64() - 0.5) * 2.0 * daily_vol;
        closes[i] = closes[i + 1] / (1.0 + 0.0006 + shock);
    }
    let start = now() - Duration::days(n as i64 - 1);
    closes
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let o = if i == 0 { c } else { closes[i - 1] };
            let spread = c * daily_vol * (0.3 + rng.next_f64());
            Candle {
                t: (start + Duration::days(i as i64)).timestamp(),
                o,
                h: o.max(c) + spread,
                l: (o.min(c) - spread).max(0.0001),
                c,
                v: Some(30e6 * (0.6 + rng.next_f64())),
            }
        })
        .collect()
}

fn summary(
    class: AssetClass,
    symbol: &str,
    name: &str,
    exchange: &str,
    currency: &str,
) -> AssetSummary {
    AssetSummary {
        id: AssetId::new(class, symbol),
        symbol: symbol.into(),
        name: name.into(),
        class,
        exchange: Some(exchange.into()),
        currency: Some(currency.into()),
        country: Some("US".into()),
        source: "fixture".into(),
        source_url: None,
    }
}

fn quote(
    id: &AssetId,
    price: f64,
    change_pct: f64,
    currency: &str,
    market_cap: Option<f64>,
    state: MarketState,
) -> Quote {
    // Como los proveedores reales: `change_pct` en PUNTOS porcentuales (0.8 = +0,8 %).
    let frac = change_pct / 100.0;
    Quote {
        id: id.clone(),
        price,
        change: Some(price * frac / (1.0 + frac)),
        change_pct: Some(change_pct),
        open: Some(price * 0.997),
        high: Some(price * 1.006),
        low: Some(price * 0.99),
        prev_close: Some(price / (1.0 + frac)),
        volume: Some(48_200_000.0),
        market_cap,
        currency: Some(currency.into()),
        market_state: state,
        timestamp: now() - Duration::seconds(3),
        source: "fixture".into(),
        delay_secs: Some(0),
        source_url: None,
        timestamp_origin: None,
    }
}

fn news(
    id: &str,
    hours_ago: i64,
    title: &str,
    summary: &str,
    publisher: &str,
    tickers: &[&str],
) -> NewsItem {
    NewsItem {
        id: id.into(),
        title: title.into(),
        summary: Some(summary.into()),
        url: format!("https://news.example.com/{id}"),
        publisher: Some(publisher.into()),
        published_at: now() - Duration::hours(hours_ago),
        image_url: None,
        tickers: tickers.iter().map(|t| t.to_string()).collect(),
        sentiment: None,
        provider: "fixture".into(),
        language: Some("en".into()),
    }
}

fn macro_context() -> MacroContext {
    let ind = |key: &str, name: &str, value: f64, change: Option<f64>, unit: &str| MacroIndicator {
        key: key.into(),
        name: name.into(),
        value,
        change,
        unit: unit.into(),
        as_of: now() - Duration::hours(2),
        source: "fixture".into(),
        source_url: None,
        timestamp_origin: None,
    };
    MacroContext {
        indicators: vec![
            ind("fed_funds", "Tipo de la Fed", 4.25, Some(0.0), "%"),
            ind("us10y", "Bono EE. UU. 10 años", 4.18, Some(0.06), "%"),
            ind("us2y", "Bono EE. UU. 2 años", 3.89, Some(0.03), "%"),
            ind("curve_10y2y", "Curva 10a-2a", 0.29, Some(0.03), "pp"),
            ind("vix", "VIX", 17.4, Some(-0.6), "pts"),
            ind("cpi_yoy", "IPC interanual (EE. UU.)", 2.9, None, "%"),
            ind("unemployment", "Paro (EE. UU.)", 4.3, None, "%"),
            ind("dxy", "Índice dólar", 101.2, Some(0.2), "pts"),
        ],
        as_of: Some(now() - Duration::hours(2)),
    }
}

/// Acción de gran capitalización con tendencia alcista y valoración exigente.
pub fn stock_snapshot() -> AssetSnapshot {
    let s = summary(AssetClass::Stock, "AAPL", "Apple Inc.", "NASDAQ", "USD");
    let id = s.id.clone();
    AssetSnapshot {
        quote: Some(quote(&id, 228.40, 0.8, "USD", Some(3.45e12), MarketState::Regular)),
        fundamentals: Some(Fundamentals {
            description: Some(
                "Diseña y vende smartphones, ordenadores, wearables y servicios digitales.".into(),
            ),
            sector: Some("Technology".into()),
            industry: Some("Consumer Electronics".into()),
            country: Some("US".into()),
            employees: Some(164_000),
            market_cap: Some(3.45e12),
            enterprise_value: Some(3.50e12),
            pe_trailing: Some(34.8),
            pe_forward: Some(29.6),
            peg: Some(2.4),
            price_to_book: Some(52.0),
            price_to_sales: Some(8.9),
            ev_to_ebitda: Some(25.1),
            eps_trailing: Some(6.56),
            eps_forward: Some(7.72),
            revenue_ttm: Some(402e9),
            revenue_growth_yoy: Some(0.06),
            earnings_growth_yoy: Some(0.11),
            gross_margin: Some(0.46),
            operating_margin: Some(0.31),
            profit_margin: Some(0.24),
            roe: Some(1.51),
            roa: Some(0.23),
            free_cash_flow: Some(99e9),
            total_cash: Some(65e9),
            total_debt: Some(105e9),
            debt_to_equity: Some(1.4),
            current_ratio: Some(0.87),
            dividend_yield: Some(0.0045),
            payout_ratio: Some(0.15),
            beta: Some(1.2),
            shares_outstanding: Some(15.1e9),
            short_pct_float: Some(0.008),
            analyst_target_mean: Some(245.0),
            analyst_target_high: Some(290.0),
            analyst_target_low: Some(190.0),
            analyst_rating_mean: Some(2.0),
            analyst_count: Some(38),
            next_earnings_date: NaiveDate::from_ymd_opt(2026, 10, 30),
            sources: vec!["fixture".into()],
            as_of: Some(now() - Duration::hours(1)),
            ..Default::default()
        }),
        indicators: Some(Indicators {
            as_of: Some(now() - Duration::hours(1)),
            last_close: Some(226.6),
            sma20: Some(226.1),
            sma50: Some(221.5),
            sma200: Some(205.7),
            ema12: Some(226.9),
            ema26: Some(225.5),
            rsi14: Some(61.3),
            macd: Some(1.42),
            macd_signal: Some(1.10),
            macd_hist: Some(0.32),
            bb_upper: Some(234.0),
            bb_middle: Some(226.1),
            bb_lower: Some(218.2),
            atr14: Some(3.9),
            volatility30d: Some(0.24),
            max_drawdown1y: Some(-0.17),
            return1w: Some(0.012),
            return1m: Some(0.031),
            return3m: Some(0.082),
            return_ytd: Some(0.175),
            return1y: Some(0.243),
            high52w: Some(237.2),
            low52w: Some(164.1),
            dist_from_high52w: Some(-0.037),
            volume_avg20: Some(52_000_000.0),
            relative_volume: Some(0.93),
            trend: Some(Trend::Up),
            supports: vec![221.5, 214.0, 205.7],
            resistances: vec![232.0, 237.2],
            source: None,
            source_url: None,
        }),
        news: vec![
            news(
                "n1",
                3,
                "Apple shares rise as analysts upgrade iPhone demand outlook",
                "Two brokers raised their targets citing strong early orders.",
                "Reuters",
                &["AAPL"],
            ),
            news(
                "n2",
                20,
                "Apple faces antitrust probe over App Store fees in the EU",
                "Regulators opened a formal investigation that could lead to fines.",
                "Financial Times",
                &["AAPL"],
            ),
            news(
                "n3",
                30,
                "Apple announces record services revenue ahead of earnings",
                "Services segment grew double digits for the fourth straight quarter.",
                "Bloomberg",
                &["AAPL"],
            ),
            news(
                "n4",
                55,
                "Supply chain concerns weigh on hardware makers",
                "Component shortages could delay some product launches.",
                "WSJ",
                &["AAPL", "MSFT"],
            ),
            news(
                "n5",
                90,
                "Tech sector rallies as Fed signals patience on rate cuts",
                "Equities advanced after the minutes pointed to a gradual path.",
                "CNBC",
                &[],
            ),
            news(
                "n6",
                140,
                "Apple plans new AI features for the next iOS release",
                "The company will expand on-device models, according to people familiar.",
                "The Verge",
                &["AAPL"],
            ),
        ],
        macro_context: Some(macro_context()),
        missing: vec![],
        generated_at: now(),
        summary: s,
    }
}

/// Cripto de gran capitalización: sin fundamentales clásicos, con ATH y FDV.
pub fn crypto_snapshot() -> AssetSnapshot {
    let s = summary(AssetClass::Crypto, "BTC", "Bitcoin", "CRYPTO", "USD");
    let id = s.id.clone();
    AssetSnapshot {
        quote: Some(quote(&id, 98_450.0, 2.1, "USD", Some(1.95e12), MarketState::Continuous)),
        fundamentals: Some(Fundamentals {
            market_cap: Some(1.95e12),
            circulating_supply: Some(19.8e6),
            total_supply: Some(19.8e6),
            max_supply: Some(21e6),
            fully_diluted_valuation: Some(2.07e12),
            all_time_high: Some(109_000.0),
            all_time_high_date: NaiveDate::from_ymd_opt(2026, 1, 20),
            sources: vec!["fixture".into()],
            as_of: Some(now()),
            ..Default::default()
        }),
        indicators: Some(Indicators {
            as_of: Some(now()),
            last_close: Some(98_100.0),
            sma20: Some(95_200.0),
            sma50: Some(91_000.0),
            sma200: Some(78_500.0),
            rsi14: Some(66.0),
            macd: Some(1850.0),
            macd_signal: Some(1500.0),
            macd_hist: Some(350.0),
            atr14: Some(2900.0),
            volatility30d: Some(0.46),
            max_drawdown1y: Some(-0.31),
            return1w: Some(0.04),
            return1m: Some(0.12),
            return3m: Some(0.31),
            return_ytd: Some(0.52),
            return1y: Some(0.88),
            high52w: Some(109_000.0),
            low52w: Some(54_000.0),
            dist_from_high52w: Some(-0.097),
            trend: Some(Trend::StrongUp),
            supports: vec![95_200.0, 91_000.0, 84_000.0],
            resistances: vec![102_000.0, 109_000.0],
            ..Default::default()
        }),
        news: vec![
            news(
                "c1",
                5,
                "Bitcoin climbs as spot ETF inflows hit a monthly high",
                "Net inflows reached their strongest level since January.",
                "CoinDesk",
                &["BTC"],
            ),
            news(
                "c2",
                26,
                "Exchange hack raises fears over custody risks",
                "A mid-size exchange suspended withdrawals after a security breach.",
                "The Block",
                &["BTC"],
            ),
        ],
        macro_context: Some(macro_context()),
        missing: vec![],
        generated_at: now(),
        summary: s,
    }
}

/// Snapshot casi vacío: solo cotización. Para probar degradación elegante.
pub fn sparse_snapshot() -> AssetSnapshot {
    let s = summary(AssetClass::Stock, "ZZZ", "Zeta Zeta Holdings", "NYSE", "USD");
    let id = s.id.clone();
    AssetSnapshot {
        quote: Some(quote(&id, 12.5, -0.4, "USD", None, MarketState::Regular)),
        fundamentals: None,
        indicators: None,
        news: vec![],
        macro_context: None,
        missing: vec![
            "fundamentals: proveedor sin datos".into(),
            "indicators: historial insuficiente".into(),
            "news: sin resultados".into(),
        ],
        generated_at: now(),
        summary: s,
    }
}

/// Acción genérica derivada del fixture principal con otro precio y valoración.
fn derived_stock(
    symbol: &str,
    name: &str,
    exchange: &str,
    price: f64,
    pe: f64,
    trend: Trend,
    ret3m: f64,
) -> AssetSnapshot {
    let mut s = stock_snapshot();
    let ratio = price / 228.40;
    s.summary = summary(AssetClass::Stock, symbol, name, exchange, "USD");
    let id = s.summary.id.clone();
    s.quote = Some(quote(&id, price, 0.4, "USD", Some(1.2e12), MarketState::Regular));
    if let Some(f) = s.fundamentals.as_mut() {
        f.pe_forward = Some(pe);
        f.pe_trailing = Some(pe * 1.15);
        f.market_cap = Some(1.2e12);
        f.next_earnings_date = NaiveDate::from_ymd_opt(2026, 10, 28);
    }
    if let Some(i) = s.indicators.as_mut() {
        i.last_close = Some(price * 0.997);
        for v in [
            &mut i.sma20, &mut i.sma50, &mut i.sma200, &mut i.bb_upper, &mut i.bb_middle,
            &mut i.bb_lower, &mut i.atr14, &mut i.high52w, &mut i.low52w,
        ] {
            *v = v.map(|x| x * ratio);
        }
        i.supports = i.supports.iter().map(|x| x * ratio).collect();
        i.resistances = i.resistances.iter().map(|x| x * ratio).collect();
        i.trend = Some(trend);
        i.return3m = Some(ret3m);
    }
    s.news.truncate(3);
    for n in &mut s.news {
        n.tickers = vec![symbol.to_string()];
        n.title = n.title.replace("Apple", name);
    }
    s
}

/// `MarketData` en memoria con un catálogo pequeño y llamadas registradas.
pub struct FakeMarket {
    snapshots: HashMap<AssetId, AssetSnapshot>,
    /// Activos cuyas consultas fallan (para probar errores de herramientas).
    failing: Mutex<Vec<AssetId>>,
    calls: Mutex<Vec<String>>,
}

impl Default for FakeMarket {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeMarket {
    pub fn new() -> Self {
        let mut snapshots = HashMap::new();
        for s in [
            stock_snapshot(),
            crypto_snapshot(),
            derived_stock("MSFT", "Microsoft Corporation", "NASDAQ", 441.2, 31.5, Trend::Up, 0.05),
            derived_stock(
                "NVDA",
                "NVIDIA Corporation",
                "NASDAQ",
                182.7,
                36.0,
                Trend::StrongUp,
                0.22,
            ),
            derived_stock("TSLA", "Tesla, Inc.", "NASDAQ", 251.3, 78.0, Trend::Sideways, -0.04),
            sparse_snapshot(),
        ] {
            snapshots.insert(s.summary.id.clone(), s);
        }
        Self { snapshots, failing: Mutex::new(vec![]), calls: Mutex::new(vec![]) }
    }

    pub fn with_snapshot(mut self, s: AssetSnapshot) -> Self {
        self.snapshots.insert(s.summary.id.clone(), s);
        self
    }

    pub fn fail_for(&self, id: AssetId) {
        self.failing.lock().unwrap().push(id);
    }

    /// Llamadas recibidas, en orden («quote stock:AAPL», «history stock:AAPL 1y 1d», …).
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }

    fn get(&self, id: &AssetId) -> Result<&AssetSnapshot> {
        if self.failing.lock().unwrap().contains(id) {
            return Err(DomainError::Upstream {
                provider: "fixture".into(),
                message: format!("fallo simulado para {id}"),
            });
        }
        self.snapshots.get(id).ok_or_else(|| DomainError::NotFound(id.to_string()))
    }
}

#[async_trait]
impl MarketData for FakeMarket {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<AssetSummary>> {
        self.log(format!("search {query}"));
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let mut hits: Vec<(u8, AssetSummary)> = self
            .snapshots
            .values()
            .filter_map(|s| {
                let sym = s.summary.symbol.to_lowercase();
                let name = s.summary.name.to_lowercase();
                let rank = if sym == q {
                    0
                } else if name.starts_with(&q) {
                    1
                } else if name.contains(&q) || sym.contains(&q) {
                    2
                } else {
                    return None;
                };
                Some((rank, s.summary.clone()))
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.symbol.cmp(&b.1.symbol)));
        Ok(hits.into_iter().take(limit).map(|(_, s)| s).collect())
    }

    async fn quote(&self, id: &AssetId) -> Result<Quote> {
        self.log(format!("quote {id}"));
        self.get(id)?
            .quote
            .clone()
            .ok_or_else(|| DomainError::NotFound(format!("sin cotización para {id}")))
    }

    async fn quotes(&self, ids: &[AssetId]) -> Result<Vec<Quote>> {
        self.log(format!(
            "quotes {}",
            ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ));
        Ok(ids.iter().filter_map(|id| self.get(id).ok().and_then(|s| s.quote.clone())).collect())
    }

    async fn history(&self, id: &AssetId, range: Range, interval: Interval) -> Result<Vec<Candle>> {
        self.log(format!("history {id} {} {}", range.as_str(), interval.as_str()));
        let s = self.get(id)?;
        let price = s.quote.as_ref().map(|q| q.price).unwrap_or(100.0);
        let days = match range {
            Range::D1 | Range::D5 => 5,
            Range::M1 => 22,
            Range::M3 => 66,
            Range::M6 => 130,
            Range::Ytd => 190,
            Range::Y1 => 252,
            Range::Y2 => 504,
            Range::Y5 | Range::Y10 | Range::Max => 756,
        };
        let seed = id.symbol.bytes().fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
        let vol = if id.class == AssetClass::Crypto { 0.025 } else { 0.014 };
        Ok(candles(seed, days, price, vol))
    }

    async fn fundamentals(&self, id: &AssetId) -> Result<Option<Fundamentals>> {
        self.log(format!("fundamentals {id}"));
        Ok(self.get(id)?.fundamentals.clone())
    }

    async fn indicators(&self, id: &AssetId) -> Result<Indicators> {
        self.log(format!("indicators {id}"));
        self.get(id)?
            .indicators
            .clone()
            .ok_or_else(|| DomainError::NotFound(format!("sin indicadores para {id}")))
    }

    async fn news(&self, id: Option<&AssetId>, limit: usize) -> Result<Vec<NewsItem>> {
        self.log(format!("news {}", id.map(|i| i.to_string()).unwrap_or_else(|| "-".into())));
        match id {
            Some(id) => Ok(self.get(id)?.news.iter().take(limit).cloned().collect()),
            None => Ok(stock_snapshot().news.into_iter().take(limit).collect()),
        }
    }

    async fn macro_context(&self) -> Result<MacroContext> {
        self.log("macro".into());
        Ok(macro_context())
    }

    async fn snapshot(&self, id: &AssetId) -> Result<AssetSnapshot> {
        self.log(format!("snapshot {id}"));
        self.get(id).cloned()
    }

    fn subscribe(&self, _ids: &[AssetId]) -> BoxStream<'static, PriceTick> {
        Box::pin(stream::empty())
    }

    fn providers_status(&self) -> Vec<ProviderStatus> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_market_searches_and_serves() {
        let m = FakeMarket::new();
        let hits = m.search("apple", 5).await.unwrap();
        assert_eq!(hits[0].id.to_string(), "stock:AAPL");
        assert_eq!(m.search("btc", 5).await.unwrap()[0].id.to_string(), "crypto:BTC");
        assert!(m.search("zzzz-no-existe", 5).await.unwrap().is_empty());
        let id: AssetId = "stock:MSFT".parse().unwrap();
        assert!(m.quote(&id).await.unwrap().price > 0.0);
        let h = m.history(&id, Range::Y1, Interval::D1).await.unwrap();
        assert_eq!(h.len(), 252);
        assert!((h.last().unwrap().c - 441.2).abs() < 1e-9);
        assert!(m.snapshot(&"stock:NOPE".parse().unwrap()).await.is_err());
    }

    #[test]
    fn candles_are_deterministic_and_valid() {
        let a = candles(1, 100, 50.0, 0.02);
        let b = candles(1, 100, 50.0, 0.02);
        assert_eq!(a, b);
        assert!(a.iter().all(|c| c.h >= c.o.max(c.c) && c.l <= c.o.min(c.c) && c.l > 0.0));
        assert!(a.windows(2).all(|w| w[1].t > w[0].t));
    }
}
