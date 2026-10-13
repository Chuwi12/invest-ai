# Revisión de corrección (bugs, no estilo) — invest-ai

Revisor: agente de solo lectura, 2026-10-13. Método: lectura del código + reproducción con un
crate temporal fuera del repo (`/tmp/claude-1000/-home-chuwi/review/irev`, depende del workspace
por path) y con curl contra Binance. Ningún fichero del repo se tocó salvo este.
Cada hallazgo lleva VERIFICADO (reproducido, con su salida) o SOSPECHA (leído, no reproducido).
La tabla resumen está al principio; el detalle, abajo en el orden en que se confirmó.

## Resumen

| Impacto | Id | Fichero:línea | Qué | Estado |
|---|---|---|---|---|
| Alto | R-01 | market/providers/binance.rs:150 | Histórico cripto con rango/intervalo > 1000 velas devuelve el tramo viejo (5y/1d acaba en 2024) | VERIFICADO |
| Medio | R-02 | market/indicators.rs:191 | Vol. anualizada ×1,20 en ASX/NZX/Tadawul (se detecta «fin de semana») | VERIFICADO |
| Medio | R-05 | ai/scoring.rs:520 | FCF yield mezcla moneda financiera y de cotización (ADRs) | VERIFICADO (lectura) |
| Bajo | R-03 | market/indicators.rs:327 | Base de rendimientos desplazada una sesión al cruzar el cambio EST/EDT | VERIFICADO |
| Bajo | R-04 | market/indicators.rs:424 | Volumen relativo con vela parcial dentro de su media | VERIFICADO |
| Bajo | R-06 | market/realtime.rs:437 | Referencia de cambio WS no se renueva | SOSPECHA |
| Bajo | R-07 | server/routes/analysis.rs:97 | Profundidad ignorada al unirse; entrada HUB tras panic | SOSPECHA |
| Bajo | R-08 | server/routes/chat.rs:232 | Respuesta parcial guardada como completa | SOSPECHA |
| Bajo | R-09 | ai/scoring.rs (debt_equity_ratio) | Doble división de D/E > 25x | SOSPECHA |


## Hallazgos (detalle)

### R-01 — ALTO — Binance: el histórico con `startTime` + `limit=1000` devuelve el tramo VIEJO y termina años antes de hoy (VERIFICADO)

- `backend/crates/market/src/providers/binance.rs:150-165` (`history`) y `:79-98` (`range_start_ms`).
- Binance, con `startTime` y `limit`, devuelve las **primeras** `limit` velas desde `startTime`, no las últimas. El código
  pide siempre `limit=1000` y no aplica `coerce_interval` (Yahoo sí). Cuando `rango / intervalo > 1000` la serie se corta
  y **acaba en el pasado**, sin error ni aviso. El primer proveedor para cripto es Binance, así que no hay fallback.
- Combinaciones que ofrece la propia UI (`frontend/src/lib/chart.ts::intervalsFor`) y que fallan para cripto:
  `5y/1d`, `10y/1d`, `1mo/15m`, `1mo/30m`, `3mo/1h`, `6mo/1h`, `ytd/1h`, `1d/1m`, `5d/5m`.
  Las por defecto (`1d/5m`, `5d/15m`, `1mo/1h`, `1y/1d`, `5y/1wk`) caben en 1000 y funcionan.
- Reproducción (misma consulta que hace el código para `crypto:BTC`, `range=5y&interval=1d`):

```
$ S=$(( ($(date +%s) - 1827*86400)*1000 )); curl -s "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=1d&limit=1000&startTime=$S" | python3 -c "..."
1000 2021-10-13 00:00:00+00:00 2024-07-08 00:00:00+00:00      # hoy es 2026-10-13: faltan 27 meses
```
  Misma lógica para `3mo/1h`: 92 d × 24 = 2208 velas pedidas, se sirven las 1000 más antiguas (≈41 días, acabando hace ≈51).
- Impacto: el gráfico de BTC/ETH a 5 años en diario muestra una serie que acaba en 2024 como si fuera actual (la fuente y
  `as_of` mostrados son los de la última vela servida, así que al menos delatan la fecha si el usuario mira). `Range::Y2/D1`
  usado por `indicators` cabe (731 velas), por lo que **los indicadores y la puntuación no se ven afectados**.
- Arreglo: no mandar `startTime`; pedir las últimas N con `limit = min(1000, ceil(rango_s / intervalo_s) + 1)`, y si N > 1000
  paginar con `endTime` hacia atrás (o aplicar un `coerce_interval` propio de Binance como el de Yahoo):

```rust
let want = (range_secs(range) / interval.seconds() + 2).min(1000);
q.push(("limit", want.to_string()));      // sin startTime: Binance devuelve las ÚLTIMAS `limit`
// y para rangos que necesiten > 1000 velas, subir al siguiente intervalo con coerce_interval()
```

### R-02 — MEDIO — `periods_per_year` toma como «fin de semana» las velas de ASX/NZX (horario de verano) y de Tadawul: volatilidad inflada ×1,20 (VERIFICADO)

- `backend/crates/market/src/indicators.rs:191-211`. Cuenta velas con día UTC sábado/domingo en las últimas 60; si > 10 % usa 365 en vez de 252.
- Yahoo fecha la vela diaria de ASX en AEDT (oct–abr) a las 23:00 UTC del día anterior: la sesión del lunes cae en domingo UTC (20 % de las velas). Mercados con semana dom–jue (Tadawul) igual.
- Reproducción (`irev/src/bin/dst.rs`, velas sintéticas lun–vie con apertura 23:00 UTC dom–jue):

```
ASX-like (AEDT) periods_per_year = 365
NYSE-like periods_per_year = 252
```
- Impacto: `volatility30d` de acciones `.AX`/`.NZ` sale √(365/252) = 1,20× mayor; sube la puntuación de riesgo y el tamaño de posición se recorta (el tope por `vol > 0.5/0.8` salta antes).
- Arreglo: medir la densidad real de velas, no el día UTC:

```rust
let span_days = (tail.last().unwrap().t - tail[0].t) as f64 / 86_400.0;
let per_year = (tail.len() - 1) as f64 / span_days * 365.25;   // ~7 vel./sem. cripto -> 365; ~5 -> 261
if per_year > 300.0 { 365.0 } else { 252.0 }
```

### R-03 — BAJO — Rendimientos de calendario usan como base el cierre de la sesión anterior cuando el huso EST/EDT difiere (VERIFICADO)

- `indicators.rs:327-331` (`at_or_before`) y `:416-421`. Las velas de EE. UU. son 14:30 UTC en invierno y 13:30 en verano; el objetivo es `last.t - N meses` con la hora de la última vela.
- Reproducción (velas diarias NYSE sintéticas, cierre = nº de día; última 2026-04-22 13:30 UTC, EDT):

```
return3m rust = 0.60265  (base = cierre del 21-ene)
si la base fuera el cierre del 22-ene: 0.59211
```
  La vela del 22-ene es 14:30 UTC (EST) > 13:30, queda excluida y se usa la del día anterior. Afecta a 1s/1m/3m en las ventanas que cruzan el cambio de hora (marzo y noviembre).
- Impacto: desplazamiento de una sesión en la base (≈ 1 punto porcentual en el ejemplo sintético; en datos reales es el movimiento de un día).
- Arreglo: comparar por fecha, no por instante: `candles.partition_point(|c| c.t < day_start(target) + DAY)` con `target` truncado a medianoche UTC.

### R-04 — BAJO — `relative_volume` usa el volumen parcial de la vela en curso y esa vela entra en su propio promedio (VERIFICADO)

- `indicators.rs:424-435`. `last.v / avg` con `avg` de las últimas 20 velas incluida la actual. Binance/Yahoo devuelven la vela del día en curso con volumen parcial.
- Reproducción (19 velas de 1 M y la última con 0,3 M): `relative_volume=Some(0.3108) volume_avg20=Some(965000.0)`.
- Impacto: durante la sesión (o casi todo el día en cripto) el usuario ve «0,3×» como si fuese volumen bajo; no es comparable con el promedio. Arreglo: promediar las 20 velas *anteriores* y marcar o ajustar por la fracción de sesión transcurrida (o usar la última vela completa).

### R-05 — MEDIO — Rentabilidad por FCF mezcla monedas en ADRs/cotizaciones extranjeras (VERIFICADO por lectura; no reproducido con datos vivos)

- `backend/crates/ai/src/scoring.rs:520-535` (`valuation`): `fcf / market_cap`. `free_cash_flow` viene de `financialData` en la `financialCurrency` (p. ej. TWD en TSM, test propio `fundamentals_currency_distinguishes_trading_and_financial` en `yahoo.rs:1341`) y `market_cap` en la moneda de cotización (USD). `financialCurrency` solo se guarda en `extra` y `grep` confirma que nada en `ai/src` ni en el scoring lo lee.
- Impacto: un FCF de ~700 000 M TWD sobre una capitalización de ~1 B USD da «FCF yield» del 70 % y sube la valoración (tabla `FCF_YIELD` satura en 86); el LLM recibe esa razón como un hecho. Mismo riesgo en cualquier ratio que divida magnitudes de moneda distinta (el margen de FCF no: ambos en financialCurrency).
- Arreglo: omitir la razón si `extra.financialCurrency` existe y difiere de `fun.currency`:

```rust
let same = f.extra.get("financialCurrency").and_then(|v| v.as_str()).is_none_or(|fc| Some(fc) == f.currency.as_deref());
if same && !prof.financial { /* añadir FCF yield */ }
```

### R-06 — BAJO — Referencia de cambio de los ticks WS de acciones no se renueva al cambiar de sesión (SOSPECHA)

- `realtime.rs:437-452` (`on_raw`) y `:612-620` (`poll_task`): `Entry.reference` se fija una vez (`prev_close` de la primera quote) y no se actualiza mientras el WS esté conectado. Con feed Finnhub activo (requiere clave) y una pestaña abierta varios días, `changePct` de los ticks se mide contra un cierre antiguo. Cripto no se ve afectada (Binance da `open24h`). No reproducido (necesita feed vivo). Arreglo: refrescar `reference` cuando cambie `quote.prev_close` en el sondeo o al cruzar el día de mercado.

### R-07 — BAJO — Gestión de profundidad en el análisis compartido (SOSPECHA)

- `server/src/routes/analysis.rs:97-120` (`join_or_start`): quien se une a un análisis en curso recibe el resultado aunque pidiera otra `depth`; no se avisa. Además una tarea que hiciera panic dejaría la entrada en `HUB` para siempre (los que se unan esperan sin fin). Ninguno reproducido.

### R-08 — BAJO — Chat: un error a mitad de respuesta guarda el texto parcial como mensaje normal (SOSPECHA)

- `server/src/routes/chat.rs:232-251`: con `terminal = Error` y `acc.has_output()` se persiste lo acumulado sin marca de truncado; al recargar, parece una respuesta completa. Además, si el cliente se va, el LLM sigue generando hasta el final (diseño declarado) y retiene el permiso de chat (límite 4).

### R-09 — BAJO — Heurística de deuda/capital (SOSPECHA)

- `scoring.rs:`(`debt_equity_ratio`): Yahoo ya se divide entre 100 en `yahoo.rs`; un D/E real > 25× (poco frecuente) se vuelve a dividir y puntúa como apalancamiento bajo. Edge case, sin reproducir.

## Lo revisado que está bien (no repetir)

| Área | Comprobación | Resultado |
|---|---|---|
| RSI(14) Wilder, MACD(12,26,9), Bollinger(20,2σ poblacional), ATR(14) Wilder, SMA/EMA, vol. 30d (σ muestral log × √ppy), drawdown 1a, 52 sem. | Referencia independiente en Python (`ref.py`) sobre `yahoo_chart_aapl_1y.json` y 731 velas reales de BTCUSDT | Coinciden a 1e-6 relativo (todos) |
| √252 vs √365 | Acciones EE. UU. → 252, BTC → 365 | Correcto (salvo R-02) |
| Precio ajustado | Yahoo `quote.close` (ajustado por splits, no por dividendos) | Coherente: rendimientos de precio; no hay mezcla con `adjclose` |
| `sanitize` | Orden, duplicados, h/l rotos, NaN | Correcto; sin NaN/inf al JSON en indicadores (`annualized_volatility` devuelve None si hay no finitos) |
| `change_pct` de quotes | Yahoo `(p/prev-1)*100` y Binance `priceChangePercent`: en porcentaje (5 = 5 %); indicadores en fracción | Unidades coherentes |
| Fuente/marca de tiempo | Binance `closeTime`, Yahoo `regularMarketTime` (`TimestampOrigin::Provider`); el fallback marca `Local` | Coherente; moneda Binance `USDT` explícita |
| Single-flight (`singleflight.rs`) | Líder en tarea propia, `Cleanup` incluso con panic, errores compartidos, tests propios | Correcto |
| Hub de tiempo real | refcount en `acquire`/`release`, reemplazo de stream en WS sin pasar por 0, `last_tick` sin hueco, throttle con último valor, backoff | Correcto |
| `ws.rs` | Heartbeat/idle, límites, throttle por activo, cierre ordenado | Correcto |
| SSE de análisis | Eventos tardíos vía `last_stage`/`terminal` bajo cerrojo, guardado en BD aunque se vaya el cliente, permiso en el líder | Correcto (salvo R-07) |
| Persistencia | FK + cascade, `insert_message` transaccional, orden `(created_at,id)`, `recent_messages`, decaimiento de visitas, `analyses` upsert | Correcto |
| `models.rs` | `set_active` valida instalado; `delete` rechaza el activo; mediana de tiempos del frontend (`lib/analysis.ts`) y formato `formatPrice`/`formatCurrency` (USDT cae a «n USDT») | Correcto |
| `levels.rs` | Stop = soporte − 1·ATR, tamaño = 1 %/distancia al stop con tope | Aritmética correcta |

## Alcance no cubierto (honestidad)

No se revisó a fondo: `collect_with_grace` bajo cancelación real, circuit breaker con proveedores reales, proveedores distintos de Binance/Yahoo, `context.rs`/`verify.rs`/`lint.rs`, y los hooks de React (claves de React Query, `useChatStream`, reconexión del WebSocket del frontend). No se arrancó el servidor en el puerto 8790. Que no haya hallazgos ahí no implica que esté bien.
