# Notas del crate `market`

Estado: **en curso**. Se actualiza al terminar cada parte.

## Hecho
- Dependencias del crate, `ratelimit.rs` (token bucket), `http.rs` (cliente único, mapeo de errores sin filtrar claves en URLs).
- `indicators.rs`: SMA, EMA, RSI de Wilder, MACD, Bollinger, ATR, volatilidad 30d, drawdown 1a, rendimientos por calendario, 52 semanas, volumen relativo, tendencia, soportes/resistencias. Tests con valores contrastados con una implementación independiente en Python.
- Fixtures reales en `crates/market/tests/fixtures/` (Yahoo, Binance, CoinGecko, Google News, SEC).

## Pendiente
- Proveedores (Yahoo, Binance, CoinGecko, GDELT, Google News, FRED primero; luego Finnhub y resto).
- Enrutado con fallback, caché + single-flight, `snapshot`, `search`, hub de tiempo real.

## Decisiones
- `max_drawdown1y` es **negativo o cero** (-0.34 = -34 %), igual que `dist_from_high52w`.
- `supports`/`resistances` van ordenados del más cercano al precio al más lejano.
- Dependencias propias de `market`: moka, tokio-tungstenite, tokio-util, feed-rs, sha2, url; `reqwest` con la feature extra `http2`.

## Cambios propuestos al dominio
(ninguno por ahora)
