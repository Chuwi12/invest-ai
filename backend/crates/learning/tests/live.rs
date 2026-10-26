//! Humo con red real (3 peticiones de histórico): `cargo test -p learning --test live -- --ignored --nocapture`.
//! Comprueba las convenciones de velas (hora, última vela parcial) sobre las que se apoya `rules::compute`.

use domain::{AssetId, Interval, MarketData, Range};
use learning::rules::{Computation, Trading, compute};

#[tokio::test]
#[ignore = "usa red real"]
async fn real_daily_candles_resolve_a_four_week_outcome() {
    let market = market::MarketService::new(market::MarketConfig::from_env()).unwrap();
    let now = chrono::Utc::now();
    let t0_ms = (now - chrono::Duration::days(45)).timestamp_millis();
    for (asset, bench) in [("stock:AAPL", Some("etf:SPY")), ("crypto:BTC", None)] {
        let id: AssetId = asset.parse().unwrap();
        let trading = Trading::of(id.class);
        let candles = market.history(&id, Range::M3, Interval::D1).await.unwrap();
        let b = match bench {
            Some(b) => Some(
                market
                    .history(&b.parse().unwrap(), Range::M3, Interval::D1)
                    .await
                    .unwrap(),
            ),
            None => None,
        };
        let last = candles.last().unwrap();
        println!(
            "{asset}: {} velas, última t={} (hace {:.1} h), cierre {}",
            candles.len(),
            last.t,
            (now.timestamp() - last.t) as f64 / 3600.0,
            last.c
        );
        for h in [1u32, 4] {
            match compute(&candles, b.as_deref(), t0_ms, trading, h, now.timestamp()) {
                Computation::Resolved(r) => println!(
                    "  {h} sem: entrada {} -> salida {} | activo {:+.2} % | índice {:+.2} % | exceso {:+.2} %",
                    r.entry_ts,
                    r.exit_ts,
                    r.asset_return * 100.0,
                    r.bench_return * 100.0,
                    r.excess * 100.0
                ),
                other => panic!("{asset} {h}s: {other:?}"),
            }
        }
    }
}
