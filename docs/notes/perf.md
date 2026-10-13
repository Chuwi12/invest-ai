# Rendimiento: mediciones y optimizaciones

Estado: **en curso** (parte 1: instrumentación y medidas en frío). Todo se mide en release, con datos reales, en el puerto 8791 y con `RUST_LOG` del `.env` (`info,server=debug,market=debug,ai=debug`).

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
