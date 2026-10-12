# Notas del crate `market`

Estado: **funcional de extremo a extremo** (última actualización: tras la parte 6). `cargo check/test/clippy -p market --all-targets` en verde (clippy: solo el aviso previo de `domain::ChatEvent`).

## Hecho
- **Compila** (arreglado el literal duplicado de `yahoo.rs` y el test de relevancia).
- Base: `ratelimit.rs`, `http.rs` (cliente único HTTP/2 + gzip/brotli, timeouts, tope de 16 MB, errores sin URL), `secret.rs` (`Secret`: Debug/Display ocultos + `redact`), `symbol.rs` (lista blanca de símbolos, bloquea `..`, `//`, `?`, `#`, `%`, espacios; `verify_url`), `indicators.rs`, `news.rs`.
- **Proveedores sin clave**: Yahoo (search, quote, lote, histórico, fundamentales con crumb, opciones, macro de respaldo ^TNX/^IRX/^FVX/^TYX/DX-Y.NYB/^VIX…), Binance (REST 24hr/lote/klines + WS `miniTicker`), CoinGecko (búsqueda, quote, OHLC, metadatos, oferta, ATH; clave demo opcional), Google News RSS (en/es), GDELT DOC 2.0, SEC EDGAR (UA obligatorio, 8 req/s, filings 10-K/10-Q/8-K… como noticias del proveedor `sec`).
- **Con clave opcional** (se desactivan limpiamente y salen así en `providers_status`): FRED (macro + series `bond`), Finnhub (quote, fundamentales, recomendaciones, noticias, WS de trades), Alpha Vantage, FMP, Twelve Data, NewsAPI, Marketaux, CryptoPanic.
- **`MarketService`** (`service.rs`) implementa `domain::MarketData`: enrutado por clase con fallback (`order_for`), caché moka (quote 3 s, histórico intradía 60 s / diario 15 min, indicadores 15 min, fundamentales 6 h, noticias 3 min, macro 1 h, búsqueda 10 min, ficha 24 h, opciones 30 s), single-flight (`try_get_with`), rate limiter por proveedor + pausa tras 429, timeout por operación, `snapshot()` en paralelo con timeout por parte y `missing`, `providers_status()` real.
- **Hub de tiempo real** (`realtime.rs`): 1 WS por proveedor (Binance siempre; Finnhub con clave), perezoso (no conecta sin suscriptores y cierra sin ellos), reconexión con backoff exponencial + jitter (1 s → 60 s), idle-timeout 90 s, broadcast con refcount, sondeo de respaldo solo cuando el WS no está conectado (o el activo no tiene feed), throttling 4 ticks/s con último valor garantizado, el suscriptor tardío recibe el último tick.
- Tests: 98 unitarios (fixtures reales de Yahoo/Binance/CoinGecko/Google News/SEC) + 5 de humo `#[ignore]` en `tests/live.rs`.

## Resultado literal de las pruebas
`cargo test -p market`:
```
test result: ok. 98 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out   (tests/live.rs)
```
`cargo test -p market --test live -- --ignored --nocapture --test-threads=1` (12-oct-2026 ~09:31 UTC, sin ninguna clave; extracto literal):
```
BINANCE BTC: 83466.87 Some("USDT") fuente=binance ts=2026-10-12T09:31:02.008+00:00 (hace 1 s)
  velas 1d: 31
  tick WS: 83474.4 cambio24h=Some(0.5207003684882361)% ts=2026-10-12T09:31:06.015+00:00 (hace 0 s)
  feeds: [("binance", true)]
COINGECKO BTC: oferta máx=Some(21000000.0) circulante=Some(20097053.0) ATH=Some(126080.0) (Some(2025-10-06)) as_of=Some(2026-10-12T09:28:50Z)
  búsqueda: ["crypto:BTC(coingecko)", "crypto:BCH(yahoo)", "future:BTC=F(yahoo)", ...]
NOTICIAS AAPL: 10 ítems por proveedor {"google_news": 1, "yahoo": 9}
SEC AAPL: 5 filings
  Apple Inc.: formulario 8-K/A ante la SEC | 2026-09-02T00:30:35+00:00 | https://www.sec.gov/Archives/edgar/data/320193/000114036126035325/ef20081427_8ka.htm
SNAPSHOT AAPL: nombre="Apple Inc." quote=true fundamentales=true indicadores=true noticias=15 macro=true faltan=[]
SNAPSHOT inexistente: Some(NotFound("el activo stock:ZZZZNOEXISTE no existe (...)"))
YAHOO AAPL: 336.64 Some("USD") fuente=yahoo ts=2026-10-09T20:00:00+00:00 (hace 221486 s)   <- mercado cerrado (fin de semana)
  histórico 1a: 251 velas; RSI14=Some(56.12) SMA200=Some(290.74) tendencia=Some(Up); PER=Some(38.6) sector=Some("Technology")
MACRO (9 indicadores, todos source=yahoo porque no hay FRED_API_KEY): us10y=5.244 %, vix=15.44, dxy=102.309, eurusd=1.119, gold=4200.4, wti=92.42 ...
test result: ok. 5 passed; 0 failed; 0 ignored
```
Estado de proveedores tras la prueba: yahoo, coingecko, google_news, sec con `last_ok`; **gdelt: `last_error = "timeout"`**; binance activo; el resto sin clave → `enabled=false`.

## Lo que NO está verificado (honestidad)
- **GDELT**: en la prueba en vivo dio *timeout* y en la captura del fixture respondió el aviso de límite (1 petición/5 s). El parser está probado contra el aviso real (→ `RateLimited`) y contra el formato documentado, **no contra una respuesta real de artículos**.
- **Finnhub (REST y WS), Alpha Vantage, FMP, Twelve Data, NewsAPI, Marketaux, CryptoPanic, FRED**: no hay claves en este entorno. Sus parsers se prueban con muestras escritas según la documentación oficial, no con respuestas reales. En concreto FMP (`/stable/…` y nombres de campos TTM) y CryptoPanic (`/api/developer/v2/posts/`) los he escrito de memoria de la documentación y pueden necesitar ajustes al probarlos con clave. Finnhub: `debt_to_equity` se deja sin rellenar porque no confirmo su unidad.
- La reconexión del WS tras una caída real no se ha provocado; está cubierta por diseño (backoff, idle-timeout) y `backoff()` tiene test, pero no hay test de integración de caída.
- Yahoo `^TNX` etc.: valores en % tal como los da Yahoo (5.244 = 5,244 %).

## Pendiente / límites conocidos
- Binance `history` pide como máximo 1000 velas (sin paginar): `Max` en diario ≈ 2,7 años. CoinGecko OHLC solo llega a 365 días y su granularidad no la elige el intervalo; por eso va el último en el fallback de histórico cripto.
- GDELT solo consulta artículos en inglés; Google News usa `en` y el idioma de `LLM_LANGUAGE`.
- Sin test de integración con servidor WS local (el hub se prueba con un feed simulado y con Binance real en el test `#[ignore]`).
- El último tick del hub no alimenta la caché de `quote()` (la quote usa su TTL de 3 s).

## Decisiones
- `max_drawdown1y` es **negativo o cero**; `supports`/`resistances` del más cercano al más lejano.
- `change_pct` en **puntos porcentuales** (0.52 = 0,52 %) en `Quote` y `PriceTick`, igual que Yahoo.
- La marca de tiempo (`Quote.timestamp`, `PriceTick.ts`, `MacroIndicator.as_of`) es **la del proveedor**: Binance `closeTime`/`E`, CoinGecko `last_updated_at`, Finnhub `t`, FRED fecha de la observación (00:00 UTC), Alpha Vantage solo la fecha de la sesión (00:00 UTC, el proveedor no da hora). Solo si el proveedor no da hora (`Utc::now()`) se usa la local: ocurre únicamente en Binance/CoinGecko cuando falta el campo (no observado) y en `Fundamentals.as_of` de Yahoo/Finnhub (fecha de consulta; FMP la deja `None`).
- Seguridad: claves en cabecera cuando la API lo permite (Finnhub, Twelve Data, NewsAPI, CoinGecko); en query donde no hay otra vía (Alpha Vantage, FMP, Marketaux, CryptoPanic, FRED) y el WS de Finnhub (`?token=`). Nunca se registra la URL: `map_reqwest` hace `without_url()`, los errores de WS solo registran su tipo, `Secret` oculta Debug/Display y los mensajes de error pasan por `redact`. Los símbolos del usuario se validan con lista blanca (`symbol::validate_symbol`; rechaza `..`, `//`, `?`, `#`, `%`, espacios) y se codifican en ruta/query; los nombres usados en consultas de noticias se limpian con `news::brand` (sin comillas ni operadores). No se hace ninguna petición a URLs que vengan del usuario ni de los datos externos.
- Los filings de la SEC se exponen como `NewsItem` con `provider = "sec"` (el dominio no tiene tipo de filing).
- Los bonos (`bond`) usan series FRED (`DGS10`) si hay clave y Yahoo (`^TNX`) si no.

## Cambios propuestos al dominio (1-3 YA HECHOS, ver «Procedencia verificable»)
1. **URL de verificación**: añadir `source_url: Option<String>` a `Quote`, `Fundamentals`, `AssetSummary` y `MacroIndicator` (o un `SourceRef`). Mientras tanto, `MarketService::source_url(provider, &AssetId) -> Option<String>` devuelve la página pública (Yahoo, Binance, CoinGecko, SEC, FRED…) y se puede montar en el servidor a partir de `quote.source`.
2. **Latencia en `ProviderStatus`**: `latency_ms: Option<u64>` y `blocked_secs: Option<u64>`. Hoy están en `MarketService::provider_health()`.
3. **Origen del `as_of`** en `Quote`/`Fundamentals`: un `time_source: "provider" | "local"` evitaría ambigüedad (ahora siempre es el del proveedor salvo lo dicho arriba).
4. `ChatEvent` provoca `clippy::large_enum_variant` (preexistente; boxear `Analysis`).

## Para el coordinador
Cableado en `server/src/main.rs` (dentro del runtime de Tokio; `new` arranca tareas):

```rust
// Cargo.toml de server: market.workspace = true
let market: std::sync::Arc<dyn domain::MarketData> =
    market::MarketService::new(market::MarketConfig::from_env())?; // anyhow::Result<Arc<MarketService>>
```
Firmas públicas:
- `market::MarketConfig::from_env() -> MarketConfig` (lee `FINNHUB_API_KEY`, `ALPHAVANTAGE_API_KEY`, `FMP_API_KEY`, `TWELVEDATA_API_KEY`, `FRED_API_KEY`, `COINGECKO_API_KEY`, `NEWSAPI_API_KEY`, `MARKETAUX_API_KEY`, `CRYPTOPANIC_API_KEY`, `SEC_USER_AGENT`, `LLM_LANGUAGE` (idiomas de Google News), y opcionales `MARKET_HTTP_TIMEOUT_SECS` (2–60, def. 10) y `MARKET_REALTIME=0|false|off` para desactivar los WS). `MarketConfig::default()` y `from_lookup(fn(&str)->Option<String>)` para tests. Su `Debug` no muestra claves.
- `market::MarketService::new(config: MarketConfig) -> anyhow::Result<Arc<MarketService>>` (sync, **requiere runtime Tokio activo**). `Arc<MarketService>` coerciona a `Arc<dyn MarketData>`. Al soltar el último `Arc` se cancelan los WS y sondeos.
- Extras fuera del contrato (inherentes de `MarketService`): `options_chain(&AssetId, Option<i64>) -> Result<market::options::OptionsChain>`, `filings(&AssetId, limit) -> Result<Vec<NewsItem>>`, `summary(&AssetId) -> Result<AssetSummary>`, `provider_health() -> Vec<ProviderHealth>`, `realtime_feeds() -> Vec<(&'static str, bool)>`, `MarketService::source_url(provider, &AssetId) -> Option<String>`.
- Errores: `InvalidInput` para símbolos/consultas no válidos, `NotFound` si el activo no existe, `Upstream{provider,..}`/`RateLimited{provider}` cuando fallan todos los proveedores; `snapshot` solo falla con `NotFound`/`InvalidInput`.
- `.env.example` ya tiene todas las variables salvo las opcionales `MARKET_HTTP_TIMEOUT_SECS` y `MARKET_REALTIME`; `SEC_USER_AGENT` por defecto lleva `example.com` y el servicio avisa por log: conviene poner un contacto real.
- Test de humo: `cargo test -p market --test live -- --ignored --nocapture --test-threads=1` (usa red real; `--test-threads=1` para respetar los límites de Yahoo/CoinGecko/GDELT).

## Procedencia verificable (hecho)
- Dominio (solo campos `Option` con `#[serde(default)]`, serializados como `null`): `source_url` en `AssetSummary`, `Quote`, `Fundamentals`, `MacroIndicator`, `Indicators`; `timestamp_origin: provider|local` (enum `TimestampOrigin`) en `Quote`, `Fundamentals`, `MacroIndicator`; `Indicators.source`; `ProviderStatus.latency_ms/blocked_secs`. `NewsItem.url` ya es el enlace original. Los literales de otros crates (ai, server/fake.rs, resolve.rs) se completaron con `None` con permiso del coordinador.
- `MarketService`: `route_named` sabe qué proveedor respondió; quotes (también en lote) y fichas llevan `source_url` (`symbol::verify_url`; `None` si el proveedor no tiene página por activo, p. ej. Alpha Vantage); macro: URL de la serie FRED o de la cotización Yahoo; fundamentales: URL del primer proveedor y `timestamp_origin = local` para Yahoo/Finnhub (fecha de consulta), `provider` para el resto.
- `timestamp_origin` en quotes: `provider` si el proveedor dio la hora (Yahoo `regularMarketTime`, Binance `closeTime`, CoinGecko `last_updated_at`, Finnhub/Twelve Data/FMP `timestamp`, FRED/Alpha Vantage fecha de sesión); `local` si se usó `Utc::now()`.
- Histórico: la caché guarda velas + proveedor; `market::provenance` (tabla estática acotada, TTL 30 min) anota proveedor/URL/última vela por (activo, rango, intervalo). Es global porque `MarketData::history` devuelve `Vec<Candle>` y no se toca el trait. `/history` la lee y pone `X-Data-Source`, `X-Data-Source-Url`, `X-Data-As-Of`. Con `FakeMarket` no hay cabeceras. CORS: no hace falta exponerlas (mismo origen / proxy de Vite).
- Tests: +3 de procedencia en `service.rs`, +1 en `provenance.rs`, `server/tests/provenance.rs` (cabeceras presentes/ausentes) y claves nuevas en `server/tests/api.rs`.
- Verificado contra el backend real (12-oct-2026): `/history` devuelve `x-data-source: yahoo`, `x-data-source-url: https://finance.yahoo.com/quote/AAPL`; quotes/fichas/macro con URL; Fundamentals `timestampOrigin: local`; `/status` con `latencyMs` (yahoo 61 ms; gdelt timeout 6004 ms).
- No verificado: URLs de FMP/TwelveData/Finnhub (formato propio sin probar), `blockedSecs` con un 429 real.
