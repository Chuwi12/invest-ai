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
