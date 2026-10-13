# Rendimiento: mediciones y optimizaciones

Estado: **completo** salvo lo listado en «No verificado». Todo se mide en release, con datos reales, en el puerto 8791 y con `RUST_LOG` del `.env` (`info,server=debug,market=debug,ai=debug`).

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
132 ticks/s entregados en total (6,6 por cliente y segundo; Binance `miniTicker` ~1/s por símbolo), 0 fallos, CPU del servidor ~0,4 % de un núcleo en 30 s con 20 clientes y 10 activos; latencia marca del proveedor -> recepción en el cliente: p50 125 ms, p95 1,7 s, p99 2,7 s. La cola larga se diagnosticó después (ver «Cola de latencia del tick WS»): son los primeros ticks de cada suscripción, no la tubería. Acciones (sin feed WS, sondeo): 5 clientes con `stock:AAPL/MSFT` recibieron 10 ticks en 15 s con el mismo `ts`: NO son repeticiones, son el tick inicial que recibe cada cliente al suscribirse (5 clientes x 2 activos); el sondeo ya descartaba lo no cambiado (`changed`). Con 3 clientes x 2 activos durante 40 s llegaron exactamente 6 ticks (uno por cliente y activo). El `ts` de 60 277 s de antigüedad es el cierre del lunes (16:00 ET) visto el martes por la mañana: hora del proveedor, no retraso del servidor.


### Cola de latencia del tick WS (diagnosticada el 13-oct-2026)
`loadgen ws` (ahora con `--dump fichero.tsv`: una fila por tick con hora del proveedor y de recepción, y percentiles separados `initial_ts_to_recv_ms` / `live_ts_to_recv_ms`), 20 clientes x 10 pares (BTC ETH SOL BNB XRP ADA DOGE AVAX DOT LINK), 40 s, servidor en release:
| Subconjunto | n | p50 | p95 | p99 | máx |
|---|---|---|---|---|---|
| todos los ticks | 4860 | 138 ms | 227 ms | 1236 ms | 1363 ms |
| **primer tick de cada (cliente, activo)** | 200 | 1109 ms | 1363 | 1363 | 1363 |
| **ticks en vivo** | 4660 | **138 ms** | **199 ms** | **234 ms** | **252 ms** |
- Causa: el primer tick que recibe un cliente al suscribirse es el **último tick conocido** del hub (`last_tick`, con su `ts` original de Binance), no uno nuevo. En pares poco líquidos el último evento puede tener hasta 10 s (el `miniTicker` solo emite si hubo cambios: huecos entre ticks de 1-13 s en ADA/DOT/LINK/DOGE). Esos 200 ticks (3,5-4 % del total) eran todo el p95/p99 de la medida anterior (1,7 s / 2,7 s); sin ellos, 0 de 4660 ticks pasaron de 252 ms (la antigüedad 138 ms es red a Binance + su emisión). Primera medida (sin separar): 180 de los 200 primeros ticks > 400 ms y 0 de los 5580 restantes.
- Descartado con datos: el throttle de 4 ticks/s (separación mínima entre ticks de un activo 0,7-1,0 s, siempre > 250 ms: no actúa con `miniTicker`), el buffer del broadcast (64 por activo; 0 repeticiones ni huecos de más) y la hora del evento (`E` = momento de emisión de Binance, constante).
- No es un fallo, es el diseño documentado (el suscriptor tardío recibe el último tick con su hora real, que la UI usa para mostrar la antigüedad). Documentado en `docs/API.md`.
- **Bug real encontrado por el camino (nuestro código)**: en esa misma prueba el log mostraba `WS conectado symbols=1` y 0,6 s después `WS desconectado: cerrada por el servidor`, con reconexión ~2 s más tarde. Binance admite **5 mensajes de control por segundo y conexión**; el worker mandaba un `SUBSCRIBE` por activo (1 al conectar + 9 de los demás clientes en ráfaga). Reproducido con un cliente Node contra `data-stream.binance.vision`: 10 `SUBSCRIBE` seguidos -> 5 acks, ningún tick, la conexión deja de servir (10 con 250 ms de separación: 42 mensajes en 8 s, bien). Arreglo en `realtime.rs`: las altas/bajas que llegan juntas se agrupan 40 ms en una sola tanda (diferencia entre el conjunto deseado antes y después) y los mensajes de control se espacian con `WsFeed::min_msg_gap()` (Binance 250 ms). Tests `burst_of_subscriptions_is_batched_and_paced` y `burst_of_unsubscriptions_is_batched` (servidor WS local). Verificado de nuevo con Binance real: una sola conexión (`WS conectado`, sin desconexión prematura) y los 10 pares con ticks.

## Con la bolsa de EE. UU. abierta (13-oct-2026, 15:42-16:30 UTC = 11:42-12:30 ET, servidor release en :8786)
Hasta hoy todo se había medido cerrado o en pre-market.
| Comprobación | Medida |
|---|---|
| `marketState` de `stock:AAPL`, `stock:MSFT`, `index:^GSPC`, `etf:SPY` | `regular` en los cuatro (15 muestras cada uno) |
| Antigüedad real del `timestamp` (hora local - hora del proveedor), 15 muestras cada 4 s | AAPL 0,5 / 2,0 / 4,2 s (mín / mediana / máx); MSFT 1,1 / 2,1 / 2,9; ^GSPC 0,6 / 1,1 / 1,7; SPY 1,0 / 1,9 / 3,7. Los 4 cambian de (precio, hora) en las 15 muestras |
| Retraso **declarado** (`delaySecs`, de `exchangeDataDelayedBy` de Yahoo) | AAPL 15 s, MSFT 15 s, ^GSPC 0, SPY 0. **No coincide con la realidad**: AAPL tiene 2 s de antigüedad mediana. Contraste independiente: `api.nasdaq.com/api/quote/AAPL/info` (`isRealTime: true`) daba 344,98 a las 11:44 ET y nuestra cotización 344,9557 a las 15:44:01Z. Yahoo declara 15 s por defecto en NMS/NYQ aunque sirva datos casi en tiempo real; la UI muestra «Retraso de 15 s» junto a «ahora mismo». No se oculta ni se corrige en backend (es lo que declara el proveedor); queda como observación para el frontend |
| Ticks WS de acciones (sin feed WS: sondeo) | 3 clientes x 4 activos, 90 s: 216 ticks = 18 por activo y cliente = uno cada 5,0-5,2 s (huecos mín/mediana/máx 5,0/5,1/5,2 s); 0 ticks repetidos (mismo precio y hora); edad p50 1,0-2,4 s, p95 2,3-4,2 s |
| Peticiones salientes a Yahoo en esos 90 s (`/api/status` -> `outbound`) | 72 = 4 activos x 90 s / 5 s: el sondeo va a 5 s y se comparte entre clientes |
| ¿Se aplica el espaciado a 60 s? | **No** con el mercado abierto (18 sondeos por activo en 90 s; con 60 s habría 1-2). Los tests con tiempo simulado lo cubren y ahora hay medida real |
| `./invest-ai selftest` | `todo OK (1.9 s)`: cotización AAPL `mercado=regular hace 1 s`, BTC `continuous hace 0 s`, tick WS BTC `hace 0 s` |
- **Bug encontrado solo con el mercado abierto**: Yahoo añade a las series intradía un punto «en vivo» desalineado (hora del último trade, p. ej. 15:47:31 en una serie de 5 min, `o=h=l=c`, volumen 0), p. ej. `1d/5m`: velas 15:40, 15:45 y **15:47:36**; `1mo/1h`: 15:30 y 15:46:47. El gráfico recibía una vela fantasma dentro del tramo en curso y `X-Data-As-Of` salía desalineado. Arreglo: `yahoo::fold_live_point` (lo pliega en la vela de su tramo, o abre la del tramo alineada). Verificado en real tras el arreglo: 1m/5m/15m/1h con 0 velas desalineadas. Tests con la respuesta real capturada (`yahoo_chart_aapl_5m_live.json`) y casos de 1 min, 1 h con inicio a :30, huecos y diario.
- No verificado: el cambio `regular -> post` (cierre 22:00 Madrid) ni la apertura (el espaciado a 60 s al cerrar y volver a 5 s al abrir solo se prueba con tiempo simulado).

## Qué NO se optimizó (y por qué)
- Latencia en frío de quote/history/indicators/fundamentals (150-280 ms p50): es 1 ida y vuelta a Yahoo (193 ms de media saliente); el servidor aporta <1 ms.
- Latencia de `news` por encima de 1 s en frío: el limitador de Google News (1 req/s, 2 peticiones por llamada: en + idioma) es deliberado para no ser bloqueados; subirlo es un riesgo de ban sin medir.
- `search` (p99 62 ms en caliente, cola de 3 s en frío): escribe `record_search` en SQLite en cada llamada y, en frío, espera al limitador de CoinGecko (0,15 req/s, tope de espera 3 s). Es una decisión de protección de la API gratuita.
- Compresión/HTTP2 del lado servidor: ya están (`CompressionLayer`, reqwest con HTTP/2); `history 1y` pesa 29,5 kB sin comprimir.

## Sondeo con el mercado cerrado (optimización 4)
Hallazgo: aunque el tick no se repetía, el sondeo de respaldo preguntaba a Yahoo cada 5 s por activo aunque el mercado estuviera cerrado o en pre/post (la quote regular no se mueve). Ahora, si `marketState` es `closed`/`pre`/`post` y precio+`ts` no cambiaron desde el sondeo anterior, el siguiente sondeo se espacia a 60 s (`CLOSED_POLL`); con la sesión regular, cripto o estado desconocido sigue a 5 s (3 s para cripto). Coste: abrir el mercado se nota con hasta 60 s de retraso en el sondeo (con feed WS no aplica).
| Medida (pre-market de EE. UU., 3 clientes, `stock:AAPL` + `stock:MSFT`, 40 s) | Antes | Después |
|---|---|---|
| Peticiones salientes a Yahoo | 16 | 2 |
| Ticks entregados | 6 (1 por cliente y activo) | 6 |
Tests: `closed_market_without_changes_is_polled_rarely_and_emits_one_tick` (closed, pre y post), `open_market_keeps_the_fast_poll_but_does_not_repeat_identical_ticks`.
(La primera prueba, solo con `Closed`, no cambió nada porque el estado real era `pre`; por eso la condición incluye pre y post.)

## Arranque, binario y compilación
| Medida | Resultado |
|---|---|
| `./invest-ai` en caliente hasta `/api/health` OK | ~0,05 s medidos con sondeo de 20 ms desde la invocación (Ollama y modelo ya listos, sin recompilar); antes el script anunciaba «listo en 0.3 s» porque sondeaba cada 200 ms |
| Huella de cambios (`backend_fp`) | 158 ms -> 4 ms (`find -prune` en vez de `-not -path`: antes recorría `target/`); hash idéntico |
| Binario release | 18,9 MB (strip, thin LTO) |
| Compilación incremental release (tocar `service.rs` / `system.rs`) | 59 s / 45 s en esta máquina con otros agentes compilando; primera completa 1 min 46 s |
Correcciones del script: (a) `BIND_ADDR=... ./invest-ai` y `LLM_BASE_URL`/`LLM_MODEL` del entorno se ignoraban porque las variables globales del script se llaman igual y las pisaban (arrancó en el puerto por defecto); (b) el script respeta `CARGO_TARGET_DIR` para localizar el binario.

## Coste de un análisis `quick` (stock:AAPL, qwen2.5:7b, 100 % GPU)
| Parte | Tiempo |
|---|---|
| Snapshot en frío (quote, indicadores, fundamentales, noticias, macro, ficha, en paralelo) | 1,95 s (antes de la optimización de noticias sería ~6 s por GDELT) |
| Snapshot en caliente | 0,5 ms |
| Análisis completo (2 llamadas al modelo, 6161 tokens de entrada, 1529 de salida) | 43,4 s (otra ejecución vía `selftest --llm`: 37,3 s, 1644 tokens de salida) |
El LLM supone >95 % del tiempo; el snapshot es ~4 % en frío. No hay nada que optimizar en la parte de datos; la palanca es el modelo (tokens de salida, `num_ctx`), que pertenece a `crates/ai`.

## `./invest-ai selftest`
Ver README. Implementado en `backend/crates/server/src/bin/selftest.rs` (binario `invest-ai-selftest`, cliente HTTP/WS mínimo, sin dependencias nuevas salvo mover `tokio-tungstenite` de dev a normales) y `cmd_selftest` en el script. Antigüedad máxima aceptada: cripto 3 min; sesión regular 20 min; pre/post 20 h; cerrado 5 días.

## Memoria y CPU en reposo
Servidor nuevo, recorrido de 200 activos distintos (S&P 500, 189-200 válidos) por 6 endpoints seguidos (~6 min de uso continuo), y 4 min en reposo:
| Momento | RSS |
|---|---|
| Recién arrancado | 14,3 MB |
| tras 200 x quote | 20,1 MB |
| + indicators (200) | 27,2 MB |
| + fundamentals | 28,4 MB |
| + history 1y/1d | 31,5 MB |
| + history 1d/5m | 32,5 MB |
| + news (200 x ~170 kB) | 66,0 MB |
| 4 min después, sin tráfico | 66,7 MB |
CPU en reposo: 0 ticks de reloj en 20 s (y en 10 s con el servidor recién usado); no hay sondeos sin clientes (el hub de tiempo real es perezoso). Todas las cachés `moka` tienen tope (`quotes` 5000, `history` 500+500, `indicators` 1000, `fundamentals` 1000, `news` 500, `search` 1000, `summaries` 5000, `quote_batches` 200): la memoria **no crece sin límite**, pero el tope de `news` (500 entradas de ~170 kB, 50 noticias cada una) permite llegar a ~85 MB más que se verían como ~150 MB de RSS. El RSS no baja tras el TTL porque moka solo expulsa en mantenimiento al acceder y el asignador no devuelve memoria al SO. No lo recorté: es memoria acotada y un uso real toca pocas decenas de activos. Candidato si molestara: guardar solo 20 noticias por clave o bajar el tope a 200.
(Con 20 clientes WS y 10 activos durante 30 s el RSS pasó de 84 a 88 MB en el servidor de la primera tanda.)

## No verificado
- Tras 10 min de uso continuo exactos: la serie anterior son ~6 min de carga + 4 min de reposo (10 min en total), no 10 min de carga sostenida.
- Que el espaciado a 60 s del mercado cerrado se comporte igual con Finnhub/otros proveedores (solo probado con Yahoo y tests).
- El paso regular -> post / pre -> regular con mercado abierto real (solo medido a media sesión, ver arriba).
- `./invest-ai` completo con navegador y Ctrl+C no se repitió tras los cambios del script (se probó `selftest`, la instancia temporal, y el arranque con `--no-browser`).
- Compresión de respuestas: el cliente de carga midió con `identity`; no se midió el coste de br/gzip.
- Servicio systemd: probado el 13-oct-2026 (ver `ops.md`).
