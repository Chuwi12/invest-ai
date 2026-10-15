//! Pruebas de humo contra las APIs reales **sin clave**. Ignoradas por defecto:
//! `cargo test -p market --test live -- --ignored --nocapture --test-threads=1`

use std::time::Duration;

use domain::{AssetClass, AssetId, Interval, MarketData, Range};
use futures::StreamExt;
use market::{MarketConfig, MarketService};

fn svc() -> std::sync::Arc<MarketService> {
    let cfg = MarketConfig::from_lookup(|k| {
        (k == "SEC_USER_AGENT").then(|| "invest-ai-test contacto@invest-ai.local".to_string())
    });
    MarketService::new(cfg).expect("servicio")
}

fn age(t: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{} (hace {} s)",
        t.to_rfc3339(),
        (chrono::Utc::now() - t).num_seconds()
    )
}

#[tokio::test]
#[ignore = "red real"]
async fn yahoo_aapl() {
    let s = svc();
    let id = AssetId::new(AssetClass::Stock, "AAPL");
    let q = s.quote(&id).await.expect("quote AAPL");
    println!(
        "YAHOO AAPL: {} {:?} fuente={} ts={}",
        q.price,
        q.currency,
        q.source,
        age(q.timestamp)
    );
    println!("  verificar: {:?}", MarketService::source_url("yahoo", &id));
    assert!(q.price > 1.0 && q.source == "yahoo");
    let h = s
        .history(&id, Range::Y1, Interval::D1)
        .await
        .expect("history");
    println!(
        "  histórico 1a: {} velas, última t={}",
        h.len(),
        h.last().unwrap().t
    );
    assert!(h.len() > 200);
    let ind = s.indicators(&id).await.expect("indicators");
    println!(
        "  RSI14={:?} SMA200={:?} tendencia={:?}",
        ind.rsi14, ind.sma200, ind.trend
    );
    assert!(ind.rsi14.is_some());
    let f = s.fundamentals(&id).await.expect("fundamentals");
    println!(
        "  PER={:?} sector={:?}",
        f.as_ref().and_then(|f| f.pe_trailing),
        f.as_ref().and_then(|f| f.sector.clone())
    );
    assert!(f.is_some());
}

#[tokio::test]
#[ignore = "red real"]
async fn binance_btc() {
    let s = svc();
    let id = AssetId::new(AssetClass::Crypto, "BTC");
    let q = s.quote(&id).await.expect("quote BTC");
    println!(
        "BINANCE BTC: {} {:?} fuente={} ts={}",
        q.price,
        q.currency,
        q.source,
        age(q.timestamp)
    );
    assert_eq!(q.source, "binance");
    let h = s
        .history(&id, Range::M1, Interval::D1)
        .await
        .expect("klines");
    println!("  velas 1d: {}", h.len());
    assert!(h.len() >= 25);
    // Tiempo real: un tick por WebSocket (o, como mínimo, el primer valor sondeado).
    let mut st = s.subscribe(std::slice::from_ref(&id));
    let t = tokio::time::timeout(Duration::from_secs(20), st.next())
        .await
        .expect("tick a tiempo")
        .expect("tick");
    println!("  tick: {} fuente={} ts={}", t.price, t.source, age(t.ts));
    let ws = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let t = st.next().await.unwrap();
            if t.source == "binance" {
                return t;
            }
        }
    })
    .await
    .expect("tick WS de Binance");
    println!(
        "  tick WS: {} cambio24h={:?}% ts={}",
        ws.price,
        ws.change_pct,
        age(ws.ts)
    );
    println!("  feeds: {:?}", s.realtime_feeds());
}

#[tokio::test]
#[ignore = "red real"]
async fn coingecko_btc() {
    let s = svc();
    let id = AssetId::new(AssetClass::Crypto, "BTC");
    let f = s
        .fundamentals(&id)
        .await
        .expect("fundamentals")
        .expect("some");
    println!(
        "COINGECKO BTC: oferta máx={:?} circulante={:?} ATH={:?} ({:?}) as_of={:?}",
        f.max_supply, f.circulating_supply, f.all_time_high, f.all_time_high_date, f.as_of
    );
    assert_eq!(f.max_supply, Some(21_000_000.0));
    let hits = s.search("bitcoin", 5).await.expect("search");
    println!(
        "  búsqueda: {:?}",
        hits.iter()
            .map(|h| format!("{}({})", h.id, h.source))
            .collect::<Vec<_>>()
    );
    assert!(hits.iter().any(|h| h.id.symbol == "BTC"));
}

#[tokio::test]
#[ignore = "red real"]
async fn news_google_and_sec() {
    let s = svc();
    let id = AssetId::new(AssetClass::Stock, "AAPL");
    let n = s.news(Some(&id), 10).await.expect("news");
    let mut by: std::collections::BTreeMap<&str, usize> = Default::default();
    for i in &n {
        *by.entry(i.provider.as_str()).or_default() += 1;
    }
    println!("NOTICIAS AAPL: {} ítems por proveedor {by:?}", n.len());
    for i in n.iter().take(3) {
        println!(
            "  [{}] {} | {} | {}",
            i.provider,
            i.title,
            i.published_at.to_rfc3339(),
            i.url
        );
    }
    assert!(!n.is_empty());
    let f = s.filings(&id, 5).await.expect("SEC filings");
    println!("SEC AAPL: {} filings", f.len());
    for i in f.iter().take(3) {
        println!(
            "  {} | {} | {}",
            i.title,
            i.published_at.to_rfc3339(),
            i.url
        );
    }
    assert!(!f.is_empty());
}

#[tokio::test]
#[ignore = "red real"]
async fn macro_and_snapshot() {
    let s = svc();
    let m = s.macro_context().await.expect("macro");
    println!(
        "MACRO ({} indicadores, as_of {:?}):",
        m.indicators.len(),
        m.as_of
    );
    for i in &m.indicators {
        println!(
            "  {} = {} {} ({}) as_of={}",
            i.key,
            i.value,
            i.unit,
            i.source,
            i.as_of.to_rfc3339()
        );
    }
    assert!(m.indicators.iter().any(|i| i.key == "us10y"));
    let snap = s
        .snapshot(&AssetId::new(AssetClass::Stock, "AAPL"))
        .await
        .expect("snapshot");
    println!(
        "SNAPSHOT AAPL: nombre={:?} quote={} fundamentales={} indicadores={} noticias={} macro={} faltan={:?}",
        snap.summary.name,
        snap.quote.is_some(),
        snap.fundamentals.is_some(),
        snap.indicators.is_some(),
        snap.news.len(),
        snap.macro_context.is_some(),
        snap.missing
    );
    assert!(snap.quote.is_some() && snap.indicators.is_some());
    let bad = s
        .snapshot(&AssetId::new(AssetClass::Stock, "ZZZZNOEXISTE"))
        .await;
    println!("SNAPSHOT inexistente: {:?}", bad.as_ref().err());
    assert!(matches!(bad, Err(domain::DomainError::NotFound(_))));
    for p in s.providers_status() {
        println!(
            "  proveedor {:<13} activo={} clave={} ok={:?} err={:?}",
            p.name,
            p.enabled,
            p.has_key,
            p.last_ok.map(|t| t.to_rfc3339()),
            p.last_error
        );
    }
}

/// R-01: el histórico de Binance debe acabar en la vela actual y cubrir el rango pedido.
#[tokio::test]
#[ignore = "red real"]
async fn binance_history_ends_now() {
    let s = svc();
    let id = AssetId::new(AssetClass::Crypto, "BTC");
    for (range, interval, min_days) in [
        (Range::Y5, Interval::D1, 1820),
        (Range::M3, Interval::H1, 91),
        (Range::D1, Interval::M1, 0),
    ] {
        let h = s.history(&id, range, interval).await.expect("klines");
        let (a, b) = (h.first().unwrap().t, h.last().unwrap().t);
        let now = chrono::Utc::now().timestamp();
        println!(
            "  {range:?}/{interval:?}: {} velas, {} -> {} (última hace {} s, cubre {} d)",
            h.len(),
            chrono::DateTime::from_timestamp(a, 0).unwrap(),
            chrono::DateTime::from_timestamp(b, 0).unwrap(),
            now - b,
            (b - a) / 86_400
        );
        assert!(now - b < 2 * interval.seconds(), "acaba en el pasado");
        assert!((b - a) / 86_400 >= min_days, "no cubre el rango");
    }
}

/// GDELT (11-15 s por petición, 8-10 s de TLS): corre en segundo plano. La primera respuesta no lo
/// espera; sus noticias aparecen en una lectura posterior si responde (a veces da 429 o `{}`).
#[tokio::test]
#[ignore = "red real"]
async fn gdelt_background_news() {
    let s = svc();
    let id = AssetId::new(AssetClass::Stock, "AAPL");
    let t0 = std::time::Instant::now();
    let providers = |items: &[domain::NewsItem]| {
        let mut m = std::collections::BTreeMap::<String, usize>::new();
        for n in items {
            *m.entry(n.provider.clone()).or_default() += 1;
        }
        m
    };
    let first = s.news(Some(&id), 50).await.expect("noticias");
    println!("GDELT: primera respuesta en {:?}: {:?}", t0.elapsed(), providers(&first));
    for _ in 0..45 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = s.news(Some(&id), 50).await.expect("noticias");
        if providers(&now).contains_key("gdelt") {
            println!("GDELT: sus noticias llegaron a los {:?}: {:?}", t0.elapsed(), providers(&now));
            for n in now.iter().filter(|n| n.provider == "gdelt").take(3) {
                println!("  {} | {} | {}", n.published_at, n.title, n.url);
            }
            break;
        }
    }
    let h = s.provider_health();
    let g = h.iter().find(|p| p.name == "gdelt").unwrap();
    println!("GDELT: ok={:?} error={:?} latencia={:?} ms bloqueado={:?}", g.last_ok, g.last_error, g.last_latency_ms, g.blocked_secs);
}

// ---------------------------------------------------------------------------------------------
// Proveedores con clave. Se saltan (sin fallar) si la clave no está en el entorno ni en `.env`.
// ---------------------------------------------------------------------------------------------

/// Clave del entorno o del `.env` de la raíz del repositorio. Nunca se imprime.
fn env_key(name: &str) -> Option<market::secret::Secret> {
    let from_env = std::env::var(name).ok();
    let from_file = || {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../.env");
        std::fs::read_to_string(p).ok()?.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    };
    from_env.or_else(from_file).and_then(market::secret::Secret::new)
}

#[tokio::test]
#[ignore = "red real y FINNHUB_API_KEY"]
async fn finnhub_rest_and_ws() {
    use market::providers::{
        NewsQuery, Provider,
        finnhub::{Finnhub, FinnhubFeed},
    };
    use market::realtime::WsFeed;
    let Some(key) = env_key("FINNHUB_API_KEY") else {
        println!("FINNHUB: sin FINNHUB_API_KEY, se omite");
        return;
    };
    let http = market::http::build_client(Duration::from_secs(10)).unwrap();
    let fh = Finnhub::new(http, Some(key.clone()));
    for sym in ["AAPL", "MSFT"] {
        let id = AssetId::new(AssetClass::Stock, sym);
        let q = fh.quote(&id).await.expect("quote");
        println!(
            "FINNHUB {sym}: {} {:?} cambio={:?}% prev={:?} ts={} origen={:?}",
            q.price, q.currency, q.change_pct, q.prev_close, age(q.timestamp), q.timestamp_origin
        );
        assert!(q.price > 1.0 && q.source == "finnhub");
        let calc = q.prev_close.map(|p| (q.price / p - 1.0) * 100.0).unwrap();
        assert!((q.change_pct.unwrap() - calc).abs() < 0.02, "dp debe estar en puntos porcentuales");
        assert!((chrono::Utc::now() - q.timestamp).num_days() < 5);
    }
    let aapl = AssetId::new(AssetClass::Stock, "AAPL");
    assert!(matches!(
        fh.quote(&AssetId::new(AssetClass::Stock, "ZZZZNOPE")).await,
        Err(market::providers::ProviderError::NotFound(_))
    ));
    let f = fh.fundamentals(&aapl).await.expect("fundamentals").expect("perfil");
    println!(
        "  fundamentales: cap={:?} PER={:?} margen={:?} analistas={:?} media={:?} próx.resultados={:?}",
        f.market_cap, f.pe_trailing, f.profit_margin, f.analyst_count, f.analyst_rating_mean, f.next_earnings_date
    );
    assert!(f.market_cap.unwrap() > 1.0e12 && f.profit_margin.unwrap() < 1.0);
    assert!(f.analyst_count.unwrap_or(0) > 5);
    assert!(f.next_earnings_date.is_some());
    let news = fh
        .news(&NewsQuery { id: Some(aapl.clone()), name: None, limit: 5 })
        .await
        .expect("news");
    println!("  noticias: {} (primera: {:?})", news.len(), news.first().map(|n| n.title.clone()));
    assert!(!news.is_empty() && news.iter().all(|n| n.url.starts_with("http")));

    // WebSocket con un símbolo 24/7 (la bolsa de EE. UU. puede estar cerrada).
    let feed = FinnhubFeed::new(key.clone());
    let (mut ws, _) = tokio_tungstenite::connect_async(feed.url())
        .await
        .unwrap_or_else(|e| {
            // El error puede incluir la URL con la clave: solo se muestra la variante.
            let dbg = format!("{e:?}");
            let kind: String = dbg.chars().take_while(|c| c.is_alphanumeric()).collect();
            let http = match &e {
                tokio_tungstenite::tungstenite::Error::Http(r) => Some(r.status().as_u16()),
                _ => None,
            };
            panic!("no se pudo conectar al WS de Finnhub: {kind} {http:?}")
        });
    use futures::SinkExt;
    for m in feed.subscribe_msgs(&["BINANCE:BTCUSDT".to_string()], true) {
        ws.send(tokio_tungstenite::tungstenite::Message::text(m)).await.unwrap();
    }
    let mut ticks = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Ok(m)) = ws.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(t) = m {
                ticks.extend(feed.parse(t.as_str()));
                if !ticks.is_empty() {
                    break;
                }
            }
        }
    })
    .await;
    println!("  WS BINANCE:BTCUSDT: {} ticks, primero {:?}", ticks.len(), ticks.first().map(|t| (t.price, age(t.ts))));
    assert!(!ticks.is_empty(), "sin trades en 15 s");
    assert!((chrono::Utc::now() - ticks[0].ts).num_seconds().abs() < 30);
}

#[tokio::test]
#[ignore = "red real y TWELVEDATA_API_KEY"]
async fn twelvedata_quote_and_history() {
    use market::providers::{Provider, twelvedata::TwelveData};
    let Some(key) = env_key("TWELVEDATA_API_KEY") else {
        println!("TWELVEDATA: sin TWELVEDATA_API_KEY, se omite");
        return;
    };
    // Plan gratuito: 8 créditos/min y 800/día. Este test gasta 5.
    let td = TwelveData::new(market::http::build_client(Duration::from_secs(10)).unwrap(), Some(key));
    for (class, sym, quote_ccy) in [
        (AssetClass::Stock, "AAPL", "USD"),
        (AssetClass::Forex, "EURUSD=X", "USD"),
        (AssetClass::Crypto, "BTC", "USD"),
    ] {
        let id = AssetId::new(class, sym);
        let q = td.quote(&id).await.expect("quote");
        println!(
            "TWELVEDATA {sym}: {} {:?} cambio={:?}% estado={:?} ts={} origen={:?}",
            q.price, q.currency, q.change_pct, q.market_state, age(q.timestamp), q.timestamp_origin
        );
        assert!(q.price > 0.0 && q.source == "twelvedata");
        assert_eq!(q.currency.as_deref(), Some(quote_ccy));
        // La hora es la de la última cotización, no el inicio de la barra diaria.
        let hours = (chrono::Utc::now() - q.timestamp).num_hours();
        assert!((0..=96).contains(&hours), "antigüedad rara: {hours} h");
        if q.market_state == domain::MarketState::Regular {
            assert!((chrono::Utc::now() - q.timestamp).num_minutes() < 30, "mercado abierto y cotización vieja");
        }
        let calc = q.prev_close.map(|p| (q.price / p - 1.0) * 100.0).unwrap();
        assert!((q.change_pct.unwrap() - calc).abs() < 0.02, "percent_change debe estar en puntos porcentuales");
    }
    let h = td
        .history(&AssetId::new(AssetClass::Stock, "AAPL"), Range::M1, Interval::D1)
        .await
        .expect("history");
    println!("  histórico AAPL 1M diario: {} velas, última t={}", h.len(), h.last().unwrap().t);
    assert!(h.len() >= 15);
    let h = td
        .history(&AssetId::new(AssetClass::Forex, "EURUSD=X"), Range::D5, Interval::H1)
        .await
        .expect("history fx");
    println!("  histórico EUR/USD 5d 1h: {} velas, última {}", h.len(), age(chrono::DateTime::from_timestamp(h.last().unwrap().t, 0).unwrap()));
    assert!(h.len() > 20);
}

/// OPV: calendario (Finnhub o EDGAR), folleto y rendimiento posterior con datos reales.
/// Necesita `SEC_USER_AGENT` real; con `FINNHUB_API_KEY` usa el calendario de Finnhub.
#[tokio::test]
#[ignore = "red real"]
async fn ipo_calendar_prospectus_and_performance() {
    let key = env_key("FINNHUB_API_KEY");
    let ua = std::env::var("SEC_USER_AGENT").ok().or_else(|| dotenv_value("SEC_USER_AGENT"));
    let cfg = MarketConfig::from_lookup(|k| match k {
        "FINNHUB_API_KEY" => key.as_ref().map(|s| s.expose().to_string()),
        "SEC_USER_AGENT" => ua.clone(),
        _ => None,
    });
    let s = MarketService::new(cfg).expect("servicio");
    let today = chrono::Utc::now().date_naive();
    let from = today - chrono::Duration::days(60);
    let cal = s.ipo_calendar(from, today).await.expect("calendario");
    let priced: Vec<_> = cal
        .iter()
        .filter(|e| e.status == domain::IpoStatus::Priced && e.price_low.is_some() && e.price_low == e.price_high)
        .collect();
    println!("CALENDARIO {from}..{today}: {} filas ({} con precio definitivo), fuente {}", cal.len(), priced.len(), cal[0].source);
    assert!(!cal.is_empty());
    if key.is_some() {
        assert!(!priced.is_empty() && cal[0].source == "finnhub");
    }
    // Folleto de la primera OPV con símbolo.
    if let Some(e) = priced.iter().find(|e| e.symbol.is_some() && !e.spac_likely) {
        let sym = e.symbol.clone().unwrap();
        let p = s.ipo_prospectus(&sym, false, 3_000).await.expect("folleto");
        println!(
            "FOLLETO {sym}: {} {} {} precio portada={:?} secciones={:?} faltan={:?}\n  {}",
            p.issuer, p.form, p.filed_on, p.offer_price, p.sections.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), p.missing, p.url
        );
        assert!(p.url.starts_with("https://www.sec.gov/Archives/edgar/data/"));
        assert!(p.sections.iter().any(|s| s.key == "risk_factors"));
        assert_eq!(p.offer_price, e.price_low, "precio de portada = precio del calendario");
    }
    // Rendimiento de las OPV de los últimos 14 meses.
    let perf = s.recent_ipo_performance(14, 5, "SPY").await.expect("rendimiento");
    println!("RENDIMIENTO ({} OPV, índice SPY):", perf.len());
    for p in &perf {
        let f = |x: Option<f64>| x.map_or("   n/d".to_string(), |v| format!("{v:+6.1}"));
        println!(
            "  {:6} {} oferta {:>7.2}  d1 {:+6.1}  1m {} 3m {} 6m {} 12m {}  exc12m {}  ahora {:+6.1}  dd {:5.1}  {:?}",
            p.symbol, p.ipo_date, p.offer_price, p.first_day_return_pct,
            f(p.points[0].return_pct), f(p.points[1].return_pct), f(p.points[2].return_pct), f(p.points[3].return_pct),
            f(p.points[3].excess_pct), p.latest_return_pct, p.max_drawdown_pct, p.warnings
        );
    }
    assert!(!perf.is_empty());
}

fn dotenv_value(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../.env")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
}

/// Historial de resultados con OPV maduras (el precio de salida sale del calendario de Finnhub).
#[tokio::test]
#[ignore = "red real"]
async fn ipo_performance_of_mature_ipos() {
    let key = env_key("FINNHUB_API_KEY");
    let ua = dotenv_value("SEC_USER_AGENT");
    let cfg = MarketConfig::from_lookup(|k| match k {
        "FINNHUB_API_KEY" => key.as_ref().map(|s| s.expose().to_string()),
        "SEC_USER_AGENT" => ua.clone(),
        _ => None,
    });
    let s = MarketService::new(cfg).expect("servicio");
    let cases = [("CRWV", (2025, 3, 28)), ("CRCL", (2025, 6, 5)), ("CHYM", (2025, 6, 12)), ("FIG", (2025, 7, 31)), ("KLAR", (2025, 9, 10))];
    let mut ok = 0;
    for (sym, (y, m, d)) in cases {
        let date = chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap();
        match s.ipo_performance(sym, date, None, "SPY").await {
            Ok(p) => {
                ok += 1;
                let f = |x: Option<f64>| x.map_or("   n/d".to_string(), |v| format!("{v:+6.1}"));
                println!(
                    "  {:5} {} oferta {:>6.2} ({}) d1 {:+6.1} | 1m {} 3m {} 6m {} 12m {} | vs SPY 1m {} 3m {} 6m {} 12m {} | ahora {:+6.1} dd {:5.1} {:?}",
                    p.symbol, p.ipo_date, p.offer_price, p.offer_price_source, p.first_day_return_pct,
                    f(p.points[0].return_pct), f(p.points[1].return_pct), f(p.points[2].return_pct), f(p.points[3].return_pct),
                    f(p.points[0].excess_pct), f(p.points[1].excess_pct), f(p.points[2].excess_pct), f(p.points[3].excess_pct),
                    p.latest_return_pct, p.max_drawdown_pct, p.warnings
                );
            }
            Err(e) => println!("  {sym}: {e}"),
        }
    }
    assert!(ok >= 3, "al menos 3 de 5 OPV deberían calcularse");
}
