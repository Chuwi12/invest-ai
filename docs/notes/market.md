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
- **GDELT**: ver «GDELT con tráfico real (13-oct-2026)» al final: el timeout era nuestro `connect_timeout`; el servicio queda con 8-10 s de TLS, 429 casi siempre y un índice atrasado.
- **(Finnhub, Twelve Data y FRED ya verificados con clave: ver «Finnhub y Twelve Data con clave real (14-oct-2026)».)** **Alpha Vantage, FMP, NewsAPI, Marketaux, CryptoPanic**: no hay claves en este entorno. Sus parsers se prueban con muestras escritas según la documentación oficial, no con respuestas reales. En concreto FMP (`/stable/…` y nombres de campos TTM) y CryptoPanic (`/api/developer/v2/posts/`) los he escrito de memoria de la documentación y pueden necesitar ajustes al probarlos con clave. Finnhub: `debt_to_equity` se deja sin rellenar porque no confirmo su unidad.
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
- Histórico: la caché guarda velas + proveedor; `market::provenance` (tabla estática acotada, TTL 30 min) anota proveedor/URL/última vela por (activo, rango, intervalo). Es global porque `MarketData::history` devuelve `Vec<Candle>` y no se toca el trait. `/history` la lee y pone `X-Data-Source`, `X-Data-Source-Url`, `X-Data-As-Of` y `X-Data-Interval` (intervalo realmente servido, deducido con `provenance::infer_interval`; sirve igual para Yahoo `coerce_interval` y Binance `plan_history`). Con `FakeMarket` no hay cabeceras. CORS: no hace falta exponerlas (mismo origen / proxy de Vite).
- Tests: +3 de procedencia en `service.rs`, +1 en `provenance.rs`, `server/tests/provenance.rs` (cabeceras presentes/ausentes) y claves nuevas en `server/tests/api.rs`.
- Verificado contra el backend real (12-oct-2026): `/history` devuelve `x-data-source: yahoo`, `x-data-source-url: https://finance.yahoo.com/quote/AAPL`; quotes/fichas/macro con URL; Fundamentals `timestampOrigin: local`; `/status` con `latencyMs` (yahoo 61 ms; gdelt timeout 6004 ms).
- No verificado: URLs de FMP/TwelveData/Finnhub (formato propio sin probar), `blockedSecs` con un 429 real.

## GDELT con tráfico real (13-oct-2026)
Medido con curl, openssl, Python y nuestro cliente, desde esta máquina (IPv4; GDELT no publica IPv6: `curl -6` falla):
- **Causa del «timeout» de 6,0 s: nuestro `connect_timeout(6 s)` frente al apretón de manos TLS de GDELT, que tarda 8-10 s** (curl: `connect=0,12 s tls=8,8-10,4 s starttransfer=11-15 s`; `openssl s_client` 8,2 s; Python `handshake 7,7 s`; igual con TLS 1.2, 1.3, HTTP/1.1 y HTTP/2). No es la URL, ni el User-Agent, ni IPv6: es el servidor. Después de conectar la respuesta tarda 1,5-5 s más y no admite keep-alive (cierra la conexión). Por eso `outbound` mostraba `api.gdeltproject.org avgMs=6001`.
- Arreglo nuestro: cliente propio para GDELT (`http::build_slow_client`, `connect_timeout` 20 s, total 24 s < `OP_TIMEOUT`). Con él las peticiones completan (`avgMs` hasta cabeceras 8,9 s; `latencyMs` 17,6 s en una).
- Aun así 11-18 s por petición no caben en ninguna ventana de espera, y el circuit breaker lo apartaba siempre por cancelación. Ahora es un proveedor **en segundo plano** (`BACKGROUND_NEWS = ["gdelt"]`): `news()` lo lanza en su propia tarea (`spawn_late_news`), no lo espera ni lo cancela, y al terminar con éxito deja sus noticias, ya filtradas por relevancia, en `late_news`; cada lectura de `news()` fusiona `late_news` con la lista cacheada (3 min). Fallos y timeouts los anota el `Slot` como siempre (circuit breaker). Tests `slow_background_provider_does_not_delay_news_and_is_merged_once_it_answers` (la respuesta no espera, el lento llega a los 12 s y se fusiona sin repetir la petición; falla si `BACKGROUND_NEWS` está vacío) y `failing_background_provider_leaves_news_intact_and_is_accounted`.
- **Respuesta real de GDELT** (límite, con espaciados de 17-40 s entre peticiones, casi todas): HTTP 429, cuerpo `Please limit requests to one every 5 seconds or contact kalev.leetaru5@gmail.com for larger queries. All high-traffic users should switch to our ngrams dataset...`. Incluso espaciando las peticiones 17-40 s dio 429 en la gran mayoría (>75 % de unas 45 peticiones; el límite parece por IP y con una ventana mucho más larga que 5 s, o compartido con otros usuarios de esta IP). El parser ya lo trata como `RateLimited` y el `Slot` aparta al proveedor (`lastError: «límite de peticiones alcanzado»`, `blockedSecs`).
- **Respuesta real con artículos** (primera vez): `timespan=1w` + `apple sourcelang:english` -> 200, 4552 bytes, 10 artículos `language: English`. **Pero el índice está atrasado**: todos llevan `seendate = 20261010T064500Z` cuando eran las 20261013T162852Z (3,4 días), y `timespan=3d` (lo que pedíamos) devuelve `{}` también para `apple`, `apple sourcelang:eng` y nuestra consulta de AAPL. Es decir, con la ventana de 3 días GDELT no puede devolver nada hoy aunque conteste (los intentos con 2d y 5d recibieron 429 y no se pudieron confirmar; no se cambia la ventana por una sola medida, ni se sabe si el atraso es pasajero).
- Circuit breaker con tráfico real (servidor con el código anterior): tras 2 cancelaciones de GDELT, `blockedSecs` 59 -> 58 -> 57 -> 54 en `/api/status` (apartado 60 s); pasado el minuto se vuelve a probar con una sola petición (el contador de timeouts no se reinicia hasta un éxito: un nuevo fallo lo aparta otra vez al instante).
- Conclusión: GDELT queda como enriquecimiento opcional que casi nunca aporta nada hoy (429 casi siempre, índice atrasado y 11-18 s por petición). No bloquea ni retrasa nada. Fuente alternativa si se necesitara: Google News y Yahoo ya dan 40-50 titulares por activo.

## Cambios del 13-oct-2026 (verificación con el mercado abierto)
- `yahoo::fold_live_point`: pliega el punto «en vivo» desalineado que Yahoo añade a las series intradía con la sesión abierta (ver `perf.md`).
- `realtime.rs`: altas/bajas del feed WS agrupadas (40 ms) y espaciadas con `WsFeed::min_msg_gap()` (Binance 250 ms: su límite es 5 mensajes/s por conexión).
- `service.rs`: `slots: Vec<Arc<Slot>>`, proveedores de noticias en segundo plano (arriba) y `Box::pin` de las partes de `snapshot` y de los bloques de `cached(...)` grandes: en debug el futuro de `snapshot` pesaba **206 448 B** (`news` 54 144, `summary` 17 352, `indicators` 14 200, `fundamentals` 8 984, `quote` 4 664, `macro` 3 432) y viajaba por valor dentro de `join!`, `timeout` y `async_trait`, copiándose en la pila; con los 2 MiB de un worker de Tokio `GET /assets/{id}/snapshot` abortaba el servidor debug con `has overflowed its stack` (en release no, por optimizar los marcos). Ahora `snapshot` ocupa 2 368 B y `news` 1 584 B, y el mismo recorrido con proveedores simulados necesita < 768 KiB de pila en vez de 2-3 MiB. Test `snapshot_runs_in_a_2_mib_stack` (hilo de 2 MiB + runtime de un hilo; abortaba antes del arreglo y comprueba que los futuros sean < 16 KiB).


## `exchangeDataDelayedBy` de Yahoo son MINUTOS, y no es el retraso del feed (13-oct-2026)
Problema: la UI mostraba «Retraso de 15 s» junto a «ahora mismo» en AAPL/MSFT con la bolsa abierta. `yahoo.rs` leía `exchangeDataDelayedBy` como segundos (`delay_secs = 15`).

**Medida** (13-oct-2026, 17:15 UTC = 13:15 ET, sesión regular abierta; `curl` a `https://finance.yahoo.com/quote/AAPL/` y `/quote/GC%3DF/`, sin claves, con cabecera `Date` de la respuesta; la página lleva incrustadas ~60 cotizaciones con los mismos campos que `/v7/finance/quote`. La API `query1/query2` devolvía 429 para esta IP desde las 16:35 UTC —solo se pudo contrastar la página HTML—). Desfase = `Date` de la respuesta − `regularMarketTime`:

| Símbolo | `exchangeDataDelayedBy` | `quoteSourceName` | Desfase medido |
|---|---|---|---|
| `GC=F` (COMEX) | 10 | Delayed Quote | 608 s (**10 min**) |
| `CL=F` (NYMEX) | 10 | Delayed Quote | 622 s (**10 min**) |
| `^RUT` | 15 | Delayed Quote | 908 s (**15 min**) |
| `^VIX`, `^TNX` (Cboe) | 15 | *(ausente)* | 907 s, 913 s (**15 min**) |
| `AAPL` (Nasdaq) | 15 | Nasdaq Real Time Price | **3 s** |
| `JPM` (NYSE) | 20 | Nasdaq Real Time Price | **20 s** |
| `CNQ.TO` (TSX) | 15 | Free Realtime Quote | **20 s** |
| `^GSPC` | 0 | Free Realtime Quote | 4 s |
| `BTC-USD` | 0 | CoinMarketCap | 21 s |

Sobre el resto de cotizaciones de la página (dos descargas, 42 y 61 con el campo): las NMS/NCM (15), NYQ (20) y TOR (15) etiquetadas «Nasdaq Real Time Price»/«Free Realtime Quote» tenían 3-87 s de antigüedad (una ilíquida, 209 s); los futuros COMEX/NYMEX de vencimiento cercano (`GC=F`, `GCZ26`, `CL=F`; campo 10) iban a 604-643 s y los índices Cboe (campo 15) a 906-913 s. Los contratos COMEX lejanos tienen desfases enormes porque apenas operan (no miden nada).

Conclusión:
1. **La unidad es el minuto** (10 y 15 minutos de desfase real en los feeds diferidos; en segundos no cuadra con nada). El backend lo leía como segundos: error nuestro.
2. **El campo es el retraso estándar de la bolsa para quien no paga su licencia** (Nasdaq 15, NYSE 20, TSX 15, CME 10, Cboe 15), **no el del feed que sirve Yahoo**: donde Yahoo tiene tiempo real el campo sigue valiendo 15/20 (AAPL, JPM, CNQ.TO). Su propia etiqueta `quoteSourceName` («… Real Time Price», «Free Realtime Quote») lo desmiente y la medida le da la razón.
3. **Arreglo en backend** (`yahoo::effective_delay_secs`): etiqueta de tiempo real -> `delay_secs = 0`; si no, `exchangeDataDelayedBy × 60`; sin campo ni etiqueta, `None`. Así `Quote.delay_secs` es de verdad «segundos de retraso del feed» (lo consumen `ai` —`gaps.rs` avisa con ≥ 60 s y `context.rs` lo pasa al modelo— y la UI). Con solo `× 60`, AAPL habría pasado a «900 s de retraso» y el análisis de IA lo habría repetido como un hueco de datos: falso.
4. Tests: fixture real `tests/fixtures/yahoo_quote_delay_units.json` (10 cotizaciones capturadas con su hora) y `delay_is_in_minutes_and_real_time_labels_win`, que comprueba con la antigüedad de la propia captura que los diferidos van `×60` ± 2 min y los de tiempo real con < 2 min aunque el campo diga 15/20; `delay_edge_cases` (decimal, negativo, texto, desbordamiento, etiqueta sin campo). `parses_batch_quotes` pasa a esperar AAPL = 0 y SAN.MC = 900 (antes `Some(15)`). Los dos tests fallan con el código anterior.
5. **No verificado**: bolsas europeas/asiáticas (cerradas durante la medida: con el mercado cerrado `regularMarketTime` es la última operación y no mide el retraso; SAN.MC figura como «Delayed Quote» con 15 en el fixture del 11-oct y se interpreta como 900 s sin contrastar), ni la respuesta de `/v7/finance/quote` en sí (bloqueada por 429 en esta IP durante la medida; se usó la página HTML con los mismos campos). El `^TNX`/`^VIX` de la macro de respaldo (`macro_indicators_from_quotes`) viene de feeds diferidos 15 min y `MacroIndicator` no tiene campo de retraso: queda anotado, sin tocar.
6. Presentación en la UI (frontend): ver `frontend.md → Antigüedad medida frente a retraso declarado`.

## Tercera revisión (14-oct)
- `fold_live_point` también pliega el punto plano de cierre de sesión que Yahoo pone en la frontera siguiente a la última vela (5m/15m/30m): ver `review.md` R-31.
- `infer_interval` usa la mediana inferior (3 velas con fin de semana ya no dan `1wk`), R-32. `Slot::run` ya no cuenta como timeout la cancelación (abort del cliente); solo `run_discardable` (noticias en primer plano con `collect_with_grace`), R-33. Rentabilidades por sesión con holgura de 2 h (ASX), R-34.


## Finnhub y Twelve Data con clave real (14-oct-2026)
Medido ~07:40-08:00 UTC, bolsa de EE. UU. cerrada (último cierre 13-oct 20:00 UTC). Fixtures reales en `tests/fixtures/finnhub_*.json` y `twelvedata_*.json` (sin clave; los tests de parseo ya no usan muestras inventadas).

**Finnhub, plan gratuito (60 peticiones/min, cabecera `x-ratelimit-*`)**
- Permitido y verificado: `quote` (AAPL, MSFT, BRK.B, hasta `BINANCE:BTCUSDT`), `stock/profile2`, `stock/metric?metric=all`, `stock/recommendation`, `company-news`, `news?category=general`, `calendar/earnings`, `search`, `stock/market-status`, WS.
- **403 `{"error":"You don't have access to this resource."}`** (no incluido en el plan): `stock/candle`, `forex/rates`, `forex/candle`, `crypto/candle`, `stock/price-target`, `news-sentiment` y `quote` de símbolos no estadounidenses (`SAN.MC`). Clave inválida = 401 `{"error":"Invalid API key."}`. El código solo usa recursos permitidos; ahora un 403 en cualquier recurso devuelve `Unsupported` y se recuerda por (ruta, símbolo) (tope 512): esa capacidad se apaga sin anotar `last_error` ni apartar al proveedor. El 401 sigue siendo error. Test `forbidden_resource_disables_only_that_capability`.
- Unidades confirmadas: `dp` en puntos porcentuales (0,088 = 0,088 %, coincide con `(c/pc-1)*100`); `t` en segundos = hora del último cierre/operación; `marketCapitalization` y `shareOutstanding` en millones; márgenes, ROE, payout y `dividendYieldIndicatedAnnual` en %; `recommendation[0]` es el periodo más reciente (mensual; el de sep-2026 tenía 53 analistas frente a 39 de Yahoo). Noticias: `datetime` en segundos, `url` es una redirección 302 de `finnhub.io/api/news?id=...` al artículo original, `source` es el editor (Yahoo, SeekingAlpha...), 245 titulares/semana para AAPL.
- **Errores corregidos**:
  1. **El WS no conectaba nunca**: la URL `wss://ws.finnhub.io?token=...` (sin `/`) da HTTP 400 en el handshake con tungstenite. Ahora `wss://ws.finnhub.io/?token=...`. Se detectó con el test `#[ignore]` real; las pruebas con muestras no podían verlo.
  2. `source_url` de Finnhub apuntaba a `finnhub.io/quote/AAPL`, que redirige (302) a la portada: ahora `None` (no hay página pública por activo). Igual con Twelve Data (`twelvedata.com/markets/AAPL` da 404).
  3. Nuevo: `Fundamentals.next_earnings_date` desde `calendar/earnings` (ventana de 120 días; AAPL 2026-11-02, `hour: amc`). Es la 4.ª petición de `fundamentals` (caché 6 h).
  4. 403 por recurso (arriba). 5. `ws_error_kind` distingue «HTTP 401/403 (clave rechazada)» en el log.
- **WS** (`wss://ws.finnhub.io/?token=`): con `BINANCE:BTCUSDT` llegan trades `{"type":"trade","data":[{"t":ms,"p":precio,"v":volumen,"s":símbolo}]}` (decenas de trades por mensaje, ~3 mensajes/s); parser correcto. Con solo acciones y la bolsa cerrada no llega ningún trade, pero el servidor manda `{"type":"ping"}` de texto cada ~30 s (medido: 2 en 70 s): el worker los cuenta como actividad y **no hay idle-timeout ni reconexión en bucle**. Servidor de la app con AAPL+MSFT suscritos 200 s en cerrado: 1 conexión, 0 desconexiones, un tick de sondeo de Yahoo por acción. Suscribir 51 símbolos de golpe no dio ningún error visible (el límite documentado es 50 símbolos; el hub no lo acota: pendiente si se suscriben >50 acciones a la vez).
- **Enrutado con la clave puesta**: cotización/fundamentales/histórico de EE. UU. siguen sirviéndolos **Yahoo primero** (orden `order_for`), Finnhub solo es respaldo REST (y Twelve Data después). **Tiempo real de acciones: Finnhub es el único feed WS** (Yahoo no tiene): los ticks llevarán `source = "finnhub"`, `ts` = hora del trade, y el cambio se calcula contra el `prev_close` de la quote (Yahoo). Con el WS conectado el sondeo de Yahoo se pausa (salvo renovar la referencia cada 10 min). Con clave inválida (probado con una falsa): el WS reintenta con backoff 1→60 s (HTTP 401), los ticks de acciones siguen llegando por sondeo de Yahoo y Binance no se ve afectado.
- `Quote.delay_secs` de Finnhub queda en `None`: con el mercado cerrado no se puede medir su antigüedad real (Finnhub anuncia tiempo real para EE. UU. en el plan gratuito). **No verificado con la bolsa abierta**: antigüedad de los trades WS de acciones ni `delay`.
- Secreto: servidor con `RUST_LOG=debug` (incluye tungstenite): 0 apariciones de la clave en el log ni en `/api/status`, tanto con la clave real como con una falsa que falla en el handshake (el log solo dice `error=HTTP`).

**Twelve Data, plan gratuito (8 créditos/min, 800/día; 1 crédito por petición)**
- Verificado: `quote` y `time_series` (AAPL, EUR/USD, BTC/USD), intradía y diario. Cabeceras `api-credits-used/left`. Error de símbolo inexistente: HTTP 404 + `{"code":404,...}`. Límite por minuto: **HTTP 429** + `{"code":429,"message":"You have run out of API credits for the current minute..."}` (provocado una vez con 10 peticiones seguidas; no se cobran); `send()` lo convierte en `RateLimited` y el `Slot` aparta al proveedor 30 s. La clave va en `Authorization: apikey ...` (test con servidor local: no está en la URL).
- **Errores corregidos**: (1) `timestamp` de la quote es el **inicio de la barra diaria** (AAPL 13:30 UTC = apertura; cripto/forex 00:00 UTC), no la hora de la cotización: ahora se usa `last_quote_at` (AAPL 19:59 UTC). Antes la antigüedad medida salía hasta 24 h peor. (2) Forex y cripto no traen `currency`: se deduce del par (`EUR/USD` -> USD). (3) `source_url` -> `None` (404). (4) Presupuesto diario local de 780 peticiones (reinicio 00:00 UTC): agotado, devuelve `RateLimited` sin salir a la red, porque el 429 no dice si es minuto o día.
- Unidades confirmadas: `percent_change` en puntos porcentuales respecto a `previous_close` (para BTC no es el cambio de 24 h, que va en `rolling_1d_change`), números como cadenas, `volume` diario completo (AAPL 38,9 M = Yahoo), `is_market_open` booleano (cripto/forex `true`). Series con `timezone=UTC`: intradía en UTC (AAPL 1h primera barra 13:30 UTC = 9:30 ET); diario como fecha (se interpreta 00:00 UTC, mientras Yahoo usa la apertura: las velas de ambos proveedores no están alineadas); forex sin `volume`.
- Fallback: cotización de acciones `yahoo -> finnhub -> twelvedata -> fmp -> alphavantage`; forex `yahoo -> twelvedata`; cripto `binance -> coingecko -> yahoo -> twelvedata`; histórico acciones `yahoo -> twelvedata -> alphavantage`, forex `yahoo -> twelvedata`, cripto `binance -> yahoo -> twelvedata -> coingecko`. Twelve Data solo se llama si fallan los anteriores.
- No verificado: el 429 diario (mensaje y cabeceras), `delay` de las cotizaciones de acciones de EE. UU. con la bolsa abierta, un fallo real de Yahoo que fuerce a Twelve Data/Finnhub en el servidor (solo cubierto por tests de enrutado con proveedores simulados).
- Smoke tests: `cargo test -p market --test live -- finnhub twelvedata --ignored --nocapture --test-threads=1` (se saltan sin clave; Twelve Data gasta 5 créditos).

## OPV de EE. UU.: calendario, folleto y rendimiento posterior (15-oct-2026)
Código: `providers/finnhub.rs` (`ipo_calendar`), `providers/sec.rs` (`prospectus`, `recent_final_prospectuses`), `html.rs` (HTML de EDGAR -> líneas), `prospectus.rs` (secciones), `ipo.rs` (rendimiento), `yahoo::daily_between`. Tipos nuevos en `domain` (solo añadidos): `IpoEntry`, `IpoStatus`, `Prospectus`, `ProspectusSection`, `IpoPerformance`, `IpoReturnPoint`. El trait `MarketData` NO cambia: son métodos inherentes de `MarketService`.

**Fuentes (verificado con clave real, 14/15-oct-2026)**
- **Finnhub `/calendar/ipo`: SÍ está en el plan gratuito** (200, 60 req/min). Devuelve `date, exchange, name, numberOfShares, price (texto: «14.00», rango «17.00-19.00» o null), status (expected|priced|filed|withdrawn), symbol, totalSharesValue`; histórico hasta 2005 al menos. Límites medidos: **tope de 200 filas por petición** (una ventana de un año devuelve exactamente 200, las más recientes) -> `ipo_calendar` parte la ventana en dos mientras se alcance el tope. `date` = primer día de cotización en `priced` (TRXB 9-oct = primera vela de Yahoo), fecha prevista en `expected`, fecha de presentación en `filed` (sin precio ni acciones). `totalSharesValue`: en `priced` = acciones x precio; en `expected` = límite alto x acciones x 1,15 (3 casos coinciden, es una inferencia). Hay `priced` sin precio (TRBG, RZAI: salidas por registro de reventa/Reg A), con símbolo `""` o null en `withdrawn`. Una OPV puede aparecer una vez por estado.
- **Respaldo sin clave (EDGAR)**: índice de texto completo `efts.sec.gov/LATEST/search-index?forms=424B4,424B1&dateRange=custom&startdt=..&enddt=..` (200 con User-Agent; no documentado oficialmente: puede cambiar). Solo da empresa/CIK/fecha/accesión: sin precio, símbolo ni filtro «es OPV» (incluye ofertas posteriores). `ipo_calendar` lo usa si no hay clave de Finnhub o esta falla. Los índices diarios `Archives/edgar/daily-index/YYYY/QTRn/form.YYYYMMDD.idx` también responden (626 KB/día) pero no se usan.
- **Folleto**: `data.sec.gov/submissions/CIK##########.json` (documentado) + `company_tickers.json` (ticker -> CIK; ya incluía las OPV de septiembre) + descarga del documento principal (2-8 MB, 0,2-0,5 s). Para OPV de hace más de ~1 año el 424B4 ya no está en `filings.recent` sino en `filings.files[]` (CoreWeave: `CIK0001769628-submissions-001.json`): se leen esos ficheros. Nombre -> CIK con el mismo índice de texto completo. Límite propio: 8 req/s por petición HTTP (token bucket dentro de `Sec`, porque una operación encadena varias).
- **Rendimiento**: Yahoo con `period1/period2` + `events=split`. **Con `range=max` Yahoo devuelve barras de 5 min o 1 h en las salidas recientes aunque se pida `1d`**; por eso no se usa. La primera barra diaria es el día de la OPV; su apertura suele ser el precio de salida (marcador de Yahoo), así que **no se usa la apertura**, solo cierre y máximo del primer día.

**Folleto: elección y extracción**
- Elección: `Initial` (por defecto) = 424B4/424B1/253G2 más antiguo (la OPV); `Latest` = el más reciente (puede ser una oferta posterior). Sin definitivo, el último S-1/A, F-1/A, S-1, F-1 o 253G1 con `preliminary = true`. Newsmax (Reg A+) usa 253G1/253G2: soportado por nombre de formulario, **no probado con un 253G2 real** (solo vi 253G1).
- Extracción sin regex ni parser HTML: `html.rs` + títulos por (a) catálogo de Regulation S-K, (b) formato (negrita/mayúsculas/primera línea de página tras «Table of Contents») y (c) el índice del propio documento. Una sección acaba en el siguiente título de primer nivel; los títulos en mayúsculas al inicio de página fuera del catálogo también cortan (arregló «UNAUDITED CONSOLIDATED PRO FORMA…» que inflaba Dilution a 49 k caracteres en Accelevation). Subsecciones (`offering`, `summary_financial_data`, `lock_up`) llevan `parent` para no indexarlas dos veces. Lo no encontrado va a `missing`; nunca se rellena.
- Probado sobre 11 folletos reales (todos los de la tabla): DFIN (TRXB, ETRA, ADRX), Workiva (ACCV, OIG, CRWV), EdgarAgents (CHWM 424B1/F-1, LOVI, RZAI, HYAC), SPAC (HYAC, LOVI, PORT). 37 ms por documento de 5,6 MB en release. Claves: 12/12 en TRXB, ETRA, ADRX, ACCV, OIG, CRWV; HYAC/LOVI/PORT sin `shares_eligible` (las SPAC no la tienen) y PORT sin `summary_financial_data`; CHWM (F-1 pequeña) sin `summary_financial_data` ni `shares_eligible` (no existen en el documento); RZAI (reventa a accionistas, sin OPV) sin offering/dilution y con «Plan of Distribution» como `underwriting`.
- Ejemplos reales: TRXB `use_of_proceeds` = «We estimate that the net proceeds from this offering will be approximately $103.5 million…»; `lock_up` (título «Lock-Up Agreements», 1 543 caracteres); ETRA `risk_factors` 348 758 caracteres; ADRX `dilution` 9 198.
- Límites: **el lock-up de algunas SPAC (PORT, LOVI) y de RZAI no tiene subtítulo propio**: se devuelven los párrafos que mencionan «lock-up» (el `title` lo dice). `risk_factors` y `underwriting` pueden ser enormes (300 k+): usa `max_chars`. Precio de portada (`offer_price`) leído de la frase «initial public offering price»: coincidió con el calendario en los 7 casos con precio; RZAI (sin precio) da `null`. No hay tablas financieras estructuradas: las filas salen como «Concepto | $1,234 | (567)».

**Rendimiento posterior** (`MarketService::ipo_performance`, `recent_ipo_performance`): primer día (cierre y máximo vs precio de salida), 1/3/6/12 meses (última sesión <= fecha objetivo; `null` si aún no se alcanza, nunca se extrapola), rentabilidad desde el cierre del primer día, contra SPY/IWM/QQQ (cierre ajustado de la sesión anterior a la OPV), máxima caída desde el primer cierre y avisos (escisión posterior: el precio de salida se ajusta; primera sesión distinta de la del calendario; datos antiguos). Precio de salida: el dado, si no el del calendario (±5 días), si no el de la portada del folleto. Sesgos: es rentabilidad de precio desde el precio de colocación (solo la obtiene quien recibe acciones); los cierres de Yahoo no incluyen dividendos (el índice sí); supervivencia: OPV deslistadas pueden faltar en Yahoo.

**Resultado literal (`cargo test -p market --test live ipo_ -- --ignored --nocapture`, 15-oct-2026)** — rentabilidad desde el precio de salida (%), exceso = rentabilidad − SPY en el mismo periodo:
```
CRWV  2025-03-28 oferta 40,00 d1 +0,0 | 1m +5,1  3m +300,0 6m +200,8 12m +87,0  | exceso 12m +73,9 | ahora +52,6 dd -66,9
CRCL  2025-06-05 oferta 31,00 d1 +168,5 | 1m +508,9 3m +269,5 6m +176,2 12m +159,0 | exceso 12m +133,8 | ahora +143,8 dd -80,9
CHYM  2025-06-12 oferta 27,00 d1 +37,4 | 1m +11,6 3m -12,4 6m -3,0 12m -38,1 | exceso 12m -62,9 | ahora +6,2 dd -55,7
FIG   2025-07-31 oferta 33,00 d1 +250,0 | 1m +113,0 3m +51,0 6m -21,5 12m -26,3 | exceso 12m -45,4 | ahora -34,5 dd -86,2
KLAR  2025-09-11 oferta 40,00 d1 +6,9 | 1m -3,0 3m -19,6 6m -61,7 12m -65,5 | exceso 12m -83,3 | ahora -67,4 dd -73,0
```
Muestra de 5 elegida a mano (OPV grandes y conocidas): NO es una estadística y tiene sesgo de selección. Las 4 OPV más recientes (RTSN, TRXB, CHWM, ADRX) aún no cumplen 1 mes. `KLAR`: pedí 10-sep y la primera sesión fue 11-sep (aviso emitido).

**Tests**: `tests/ipo_fixtures.rs` (13, sin red, fixtures reales en `tests/fixtures/finnhub_ipo_calendar_*`, `sec_submissions_{etra,crwv,crwv_files001}`, `sec_fts_*`, `sec_prospectus_*` y `yahoo_daily_*`), unitarios en `html.rs` (12), `prospectus.rs` (3), `ipo.rs` (4); `tests/live.rs`: `ipo_calendar_prospectus_and_performance` y `ipo_performance_of_mature_ipos` (`#[ignore]`). Los fixtures de folleto son **recortados**: HTML sencillo regenerado desde las líneas reales (portada, índice, títulos y las primeras líneas de cada sección) con `examples/prospectus_probe.rs --fixture 9`, salvo `sec_prospectus_dfin_424b4_head.html`, que son los primeros 130 KB del HTML crudo de TRXB. Sin claves en ningún fixture (comprobado con grep).

**Pendiente / no verificado**: 253G2 real; folletos en PDF o texto plano antiguo (pre-2001); documentos > 16 MB (tope de `http::send`); `Slot` de la SEC usa el `OP_TIMEOUT` de 25 s para toda la operación; no hay endpoint en el servidor; el calendario de Finnhub no distingue SPAC (solo `spac_likely`, heurística por nombre/precio 10); la apertura del primer día de Yahoo no es el primer precio negociado.

### Para el coordinador (rutas sugeridas, no creadas)
- `GET /api/ipo/calendar?from=YYYY-MM-DD&to=YYYY-MM-DD` -> `MarketService::ipo_calendar(from,to)` (`IpoEntry[]`).
- `GET /api/ipo/prospectus?issuer=<ticker|CIK|nombre>&latest=false&maxChars=20000` -> `ipo_prospectus(issuer, latest, max_chars)` (`Prospectus`). Para el RAG de `knowledge`: indexar cada `ProspectusSection` con `{issuer, cik, form, filedOn, url, key}`; omitir las que tengan `parent` si ya se indexa el padre.
- `GET /api/ipo/performance?symbol=&date=&offerPrice=&benchmark=SPY` -> `ipo_performance(...)`; `GET /api/ipo/performance/recent?months=12&limit=10` -> `recent_ipo_performance`.
- Errores: `InvalidInput` (rango > 2 años, emisor/índice no válido, fecha errónea), `NotFound` (sin folleto/precio), `Upstream`/`RateLimited` como el resto. Tiempo: folleto en frío ~1-3 s; `recent_ipo_performance` es secuencial (~3 s por OPV en frío).
