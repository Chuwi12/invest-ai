# Rendimiento: mediciones y optimizaciones

Estado: **en curso** (parte 2: primeras optimizaciones aplicadas; faltan memoria, arranque, análisis y selftest). Todo se mide en release, con datos reales, en el puerto 8791 y con `RUST_LOG` del `.env` (`info,server=debug,market=debug,ai=debug`).

## Metodología
- No hay `oha`/`wrk`/`ab`: cliente propio `backend/crates/server/examples/loadgen.rs` (HTTP/1.1 keep-alive, un socket por cliente; modo `ws` con latencia marca del proveedor -> recepción). Uso: `loadgen http http://127.0.0.1:8791 -n 2000 -c 50 [--gzip] /api/dashboard`, `loadgen ws http://127.0.0.1:8791 --clients 20 --secs 30 crypto:BTC ...`.
- Compilar: `CARGO_TARGET_DIR=.../backend/target/agent-perf cargo build --release -p server --bin invest-ai --example loadgen`.
- Frío = servidor recién arrancado, BD vacía, 20 activos distintos (AAPL, MSFT, ...) con 1 cliente; en caliente = mismo recurso repetido.
- Tiempo del proveedor frente al propio: contadores de peticiones salientes por host (`market::http::outbound_stats()`, expuestos como `outbound` en `GET /api/status`: `{host, requests, avgMs}`, `avgMs` = hasta recibir cabeceras). Se añadieron como instrumentación; no cambian el comportamiento.
- Binario de referencia (antes de optimizar) guardado fuera del repo para repetir.

## Medidas en frío (antes de optimizar; mismo binario + contadores)
| Endpoint | n | p50 | p95 | p99 |
|---|---|---|---|---|
| quote | 20 | 151 ms | 356 ms | 390 ms |
| history 1y/1d | 20 | 270 ms | 351 ms | 436 ms |
| history 1d/5m | 20 | 152 ms | 421 ms | 1406 ms |
| indicators | 20 | 286 ms | 400 ms | 444 ms |
| fundamentals | 20 | 249 ms | 384 ms | 450 ms |
| **news (activo)** | 20 | **6263 ms** | 6460 ms | 6489 ms |
| search | 20 | 170 ms | 512 ms | 3025 ms |
| quotes (11 del panel) | 1 | 290 ms | | |
| macro | 1 | 63 ms | | |
| **dashboard** | 1 | **6027 ms** | | |
| models | 1 | 38 ms | | |
| status | 1 | 1,3 ms | | |

Hallazgo 1: `news` y `dashboard` en frío tardan ~6 s porque esperan a GDELT, que devuelve timeout (latencia media saliente a `api.gdeltproject.org` = 6001 ms, 21 peticiones, todas fallidas) mientras Google News responde en ~380 ms. El resto del tiempo es del proveedor (Yahoo ~220 ms por petición), no del servidor.

## Medidas en caliente (antes de optimizar, 3000 peticiones, 8 clientes, RPS / p50 / p99)
quote 135k / 0,06 / 0,10 ms; quotes(11) 103k / 0,08 / 0,13; history 1y 65k / 0,13 / 0,23 (29,5 kB); history 1d 102k / 0,08 / 0,13; indicators 134k / 0,06; fundamentals 124k / 0,06; news 53k / 0,13 / 0,24; macro 132k / 0,06; dashboard 24k / 0,32 / 0,52; search 2,3k / 0,46 / 62 ms (escribe en SQLite cada vez); models 5,6k / 1,3 / 3,0 ms; status 5,7k / 1,3 / 2,9 ms (consulta a Ollama).
Conclusión: el tiempo propio del servidor en caliente es despreciable (<0,5 ms salvo rutas que tocan BD/Ollama). Todo lo medible está en el proveedor y en cómo se esperan.

## Optimizaciones aplicadas
1. **Presupuesto en `news`** (`collect_with_grace`): los proveedores corren a la vez; en cuanto uno responde bien, los demás tienen 1,5 s para llegar y el resto se descarta. Los que llegan dentro del margen sí se fusionan (tests `slow_provider_is_dropped_after_the_grace_period`, `providers_arriving_within_grace_are_kept`, `waits_for_everyone_when_nobody_has_answered_yet`). `snapshot` (que llama a `news`) y `dashboard` se benefician.
2. **Circuit breaker por timeouts**: 2 timeouts seguidos (o 2 operaciones canceladas tras >=1 s por el margen anterior) apartan al proveedor 60 s; se reinicia con un éxito. Tests `repeated_timeouts_bench_the_provider`, `a_success_resets_the_timeout_streak`, `cancelled_slow_operations_count_as_timeouts`.
3. **Single-flight en `quotes()` (lote)**: antes solo había en `quote()` individual; 50 `/dashboard` simultáneos en frío lanzaban 31 peticiones a Yahoo y 50 a Binance. Ahora lotes iguales simultáneos (mismo conjunto de ids, orden indiferente) comparten ejecución (caché de 200 ms). Test `simultaneous_identical_batches_hit_the_provider_once`.

### Antes / después
| Medida | Antes | Después |
|---|---|---|
| `/dashboard` en frío, primera petición | 6027 ms (6017 en repetición) | 1541-1636 ms (= margen de 1,5 s; el proveedor lento aún no había acumulado fallos) |
| `/assets/{id}/news` en frío, 20 activos, p50 / p95 / p99 | 6263 / 6460 / 6489 ms | 971 / 3657 / 3801 ms (la cola la marca el limitador de Google News, 1 req/s con 2 peticiones por llamada; ver abajo) |
| 50 `/dashboard` simultáneos en frío | p50 6027 ms; peticiones salientes: yahoo 31, binance 50, google_news 2, gdelt 1 | p50 1668 ms; yahoo 3, binance 1, google_news 2 |
| 5000 `/dashboard` en caliente, 50 clientes | 1549 rps, p99 1,02 ms, +50 peticiones a Binance durante la prueba | 23 348 rps, p50 2,1 / p99 2,65 ms, 0 peticiones salientes extra |
| Peticiones salientes por un dashboard en frío | 7 | 6 (GDELT ya no se reintenta mientras está apartado) |

### WebSocket (20 clientes, 10 activos cripto, 20 s)
132 ticks/s entregados en total (6,6 por cliente y segundo; Binance `miniTicker` ~1/s por símbolo), 0 fallos, CPU del servidor ~0,4 % de un núcleo en 30 s con 20 clientes y 10 activos; latencia marca del proveedor -> recepción en el cliente: p50 125 ms, p95 1,7 s, p99 2,7 s. La cola larga NO la he diagnosticado (el sospechoso es la hora del evento de los pares poco líquidos, no el reenvío; no verificado). Acciones: sin feed WS, 5 clientes con `stock:AAPL/MSFT` con el mercado cerrado recibieron 10 ticks en 15 s con `ts` del viernes (60 277 s de antigüedad: es la hora del proveedor, no un retraso del servidor).
CPU en reposo (sin clientes) 10 s: 0 ticks de reloj. RSS tras arrancar y servir ~100 peticiones: 84 MB; tras la prueba WS 88 MB.

## Qué NO se optimizó (y por qué)
- Latencia en frío de quote/history/indicators/fundamentals (150-280 ms p50): es 1 ida y vuelta a Yahoo (193 ms de media saliente); el servidor aporta <1 ms.
- Latencia de `news` por encima de 1 s en frío: el limitador de Google News (1 req/s, 2 peticiones por llamada: en + idioma) es deliberado para no ser bloqueados; subirlo es un riesgo de ban sin medir.
- `search` (p99 62 ms en caliente, cola de 3 s en frío): escribe `record_search` en SQLite en cada llamada y, en frío, espera al limitador de CoinGecko (0,15 req/s, tope de espera 3 s). Es una decisión de protección de la API gratuita.
- Compresión/HTTP2 del lado servidor: ya están (`CompressionLayer`, reqwest con HTTP/2); `history 1y` pesa 29,5 kB sin comprimir.
