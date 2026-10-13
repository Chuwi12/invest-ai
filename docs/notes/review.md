# Revisión de corrección (bugs, no estilo) — invest-ai

Revisor: agente de solo lectura, 2026-10-13. Método: lectura del código + reproducción con un
crate temporal fuera del repo (`/tmp/claude-1000/-home-chuwi/review/irev`, depende del workspace
por path) y con curl contra Binance. Ningún fichero del repo se tocó salvo este.
Cada hallazgo lleva VERIFICADO (reproducido, con su salida) o SOSPECHA (leído, no reproducido).
La tabla resumen está al principio; el detalle, abajo en el orden en que se confirmó.

## Resumen

| Impacto | Id | Fichero:línea | Qué | Estado |
|---|---|---|---|---|
| Alto | R-01 | market/providers/binance.rs:150 | Histórico cripto con rango/intervalo > 1000 velas devuelve el tramo viejo (5y/1d acaba en 2024) | ARREGLADO |
| Medio | R-02 | market/indicators.rs:191 | Vol. anualizada ×1,20 en ASX/NZX/Tadawul (se detecta «fin de semana») | ARREGLADO |
| Medio | R-05 | ai/scoring.rs:520 | FCF yield mezcla moneda financiera y de cotización (ADRs) | ARREGLADO |
| Bajo | R-03 | market/indicators.rs:327 | Base de rendimientos desplazada una sesión al cruzar el cambio EST/EDT | ARREGLADO |
| Bajo | R-04 | market/indicators.rs:424 | Volumen relativo con vela parcial dentro de su media | ARREGLADO |
| Bajo | R-06 | market/realtime.rs:437 | Referencia de cambio WS no se renueva | ARREGLADO |
| Bajo | R-07 | server/routes/analysis.rs:97 | Profundidad ignorada al unirse; entrada HUB tras panic | DESCARTADO/diseño |
| Bajo | R-08 | server/routes/chat.rs:232 | Respuesta parcial guardada como completa | ARREGLADO |
| Bajo | R-09 | ai/scoring.rs (debt_equity_ratio) | Doble división de D/E > 25x | ARREGLADO |


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

- **Estado: ARREGLADO** — `binance.rs::history` ya no manda `startTime`: pide las últimas velas con `endTime` hacia atrás, paginando de 1000 en 1000 (`plan_history`, tope `MAX_KLINES` = 5 páginas); si el intervalo pedido necesitaría más, sube de intervalo (1m→5m, etc.) y lo comunica en la cabecera `X-Data-Interval` (`provenance::infer_interval`, la UI avisa si difiere del pedido). Coste: un rango largo puede costar hasta 5 peticiones a Binance (con caché de histórico). Tests: `binance::tests::history_ends_at_current_candle_and_covers_range` (Binance simulado con su semántica real; en rojo acababa en 2024-07-08), `plan_raises_interval_when_too_many_candles`, `provenance::tests::infers_the_served_interval_from_candle_spacing`, `server/tests/provenance.rs`, `frontend api/analysisStream.test.ts`; prueba real `live.rs::binance_history_ends_now` (5y/1d: 1829 velas hasta hoy; 3mo/1h: 2210 velas; 1d/1m: 1442 velas).

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

- **Estado: ARREGLADO** — `periods_per_year` mide la densidad real de velas (velas/día natural × 365,25; > 300 → 365, si no 252) en vez de contar días UTC de fin de semana. Test `indicators::tests::periods_per_year_ignores_utc_weekend_dating_of_exchange_sessions` (ASX-like daba 365).

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

- **Estado: ARREGLADO** — `at_or_before` compara por día UTC (`c.t < fin_del_día(objetivo)`), no por instante. Test `indicators::tests::calendar_returns_use_the_session_of_the_target_date_across_dst` (3m daba base del 21-ene en vez del 22-ene). Limitación: sigue siendo día UTC, no del huso de la bolsa.

### R-04 — BAJO — `relative_volume` usa el volumen parcial de la vela en curso y esa vela entra en su propio promedio (VERIFICADO)

- `indicators.rs:424-435`. `last.v / avg` con `avg` de las últimas 20 velas incluida la actual. Binance/Yahoo devuelven la vela del día en curso con volumen parcial.
- Reproducción (19 velas de 1 M y la última con 0,3 M): `relative_volume=Some(0.3108) volume_avg20=Some(965000.0)`.
- Impacto: durante la sesión (o casi todo el día en cripto) el usuario ve «0,3×» como si fuese volumen bajo; no es comparable con el promedio. Arreglo: promediar las 20 velas *anteriores* y marcar o ajustar por la fracción de sesión transcurrida (o usar la última vela completa).

- **Estado: ARREGLADO** — `compute_at(candles, now)`: si la última vela es del día en curso (`now < t + 1 día`) la referencia es la última vela cerrada; la media de 20 excluye la vela de referencia. Test `indicators::tests::relative_volume_ignores_the_partial_current_candle` (daba 965 000 de media y 0,31×). Límite: tras el cierre de una bolsa y antes de la siguiente apertura se usa aún el día anterior (la última vela no se distingue de una parcial sin conocer el horario).

### R-05 — MEDIO — Rentabilidad por FCF mezcla monedas en ADRs/cotizaciones extranjeras (VERIFICADO por lectura; no reproducido con datos vivos)

- `backend/crates/ai/src/scoring.rs:520-535` (`valuation`): `fcf / market_cap`. `free_cash_flow` viene de `financialData` en la `financialCurrency` (p. ej. TWD en TSM, test propio `fundamentals_currency_distinguishes_trading_and_financial` en `yahoo.rs:1341`) y `market_cap` en la moneda de cotización (USD). `financialCurrency` solo se guarda en `extra` y `grep` confirma que nada en `ai/src` ni en el scoring lo lee.
- Impacto: un FCF de ~700 000 M TWD sobre una capitalización de ~1 B USD da «FCF yield» del 70 % y sube la valoración (tabla `FCF_YIELD` satura en 86); el LLM recibe esa razón como un hecho. Mismo riesgo en cualquier ratio que divida magnitudes de moneda distinta (el margen de FCF no: ambos en financialCurrency).
- Arreglo: omitir la razón si `extra.financialCurrency` existe y difiere de `fun.currency`:

```rust
let same = f.extra.get("financialCurrency").and_then(|v| v.as_str()).is_none_or(|fc| Some(fc) == f.currency.as_deref());
if same && !prof.financial { /* añadir FCF yield */ }
```

- **Estado: ARREGLADO** — `scoring::valuation` omite el FCF yield si `extra.financialCurrency` difiere de `currency` y añade una razón sin peso («no calculada: el flujo de caja está en TWD y la capitalización en USD», `Parts::note`). Test `scoring::tests::fcf_yield_skipped_when_financial_currency_differs` (daba «Rentabilidad por FCF = 70.0% → 86»).

### R-06 — BAJO — Referencia de cambio de los ticks WS de acciones no se renueva al cambiar de sesión (SOSPECHA)

- `realtime.rs:437-452` (`on_raw`) y `:612-620` (`poll_task`): `Entry.reference` se fija una vez (`prev_close` de la primera quote) y no se actualiza mientras el WS esté conectado. Con feed Finnhub activo (requiere clave) y una pestaña abierta varios días, `changePct` de los ticks se mide contra un cierre antiguo. Cripto no se ve afectada (Binance da `open24h`). No reproducido (necesita feed vivo). Arreglo: refrescar `reference` cuando cambie `quote.prev_close` en el sondeo o al cruzar el día de mercado.

- **Estado: ARREGLADO** — reproducida con un feed simulado «conectado» (`realtime::tests::reference_is_refreshed_while_the_feed_is_connected`: +110 % contra un cierre de hace 24 h en vez de +5 %). Con el WS conectado el sondeo se paraba y `reference` se fijaba una vez. Ahora `poll_task` consulta cada `REFERENCE_REFRESH` (10 min) aunque el WS esté conectado (sin emitir tick) y renueva `reference` siempre que llegue `prev_close`.

### R-07 — BAJO — Gestión de profundidad en el análisis compartido (SOSPECHA)

- `server/src/routes/analysis.rs:97-120` (`join_or_start`): quien se une a un análisis en curso recibe el resultado aunque pidiera otra `depth`; no se avisa. Además una tarea que hiciera panic dejaría la entrada en `HUB` para siempre (los que se unan esperan sin fin). Ninguno reproducido.

- **Estado: DESCARTADO (panic) / DECISIÓN DE DISEÑO (profundidad)** — Panic: no se reproduce; el líder corre en su propia tarea (`SingleFlight::run`), un panic llega como `JoinError` → `Error` publicado y la entrada se retira del `HUB`; el permiso se libera en el unwind. Test de regresión `analysis_stream::a_panicking_analysis_ends_with_an_error_and_does_not_block_the_next_one`. Profundidad: sí ocurre (test de caracterización `joining_a_running_analysis_shares_it_regardless_of_requested_depth`), pero es el comportamiento buscado del single-flight (evita lanzar dos análisis de minutos en una sola GPU) y no es silencioso: `Analysis.depth` lleva la profundidad real y la UI la muestra (`AnalysisSections`). Cambiarlo (cola o 409) es decisión de producto.

### R-08 — BAJO — Chat: un error a mitad de respuesta guarda el texto parcial como mensaje normal (SOSPECHA)

- `server/src/routes/chat.rs:232-251`: con `terminal = Error` y `acc.has_output()` se persiste lo acumulado sin marca de truncado; al recargar, parece una respuesta completa. Además, si el cliente se va, el LLM sigue generando hasta el final (diseño declarado) y retiene el permiso de chat (límite 4).

- **Estado: ARREGLADO** — reproducida (`server/tests/chat_errors.rs`: el mensaje parcial se guardaba sin marca). `ChatMessage.truncated` (bool, `#[serde(default)]`, migración `0003_message_truncated.sql`) se pone a `true` cuando el stream termina en error con salida parcial; la UI avisa («Respuesta interrumpida…», `MessageView.test.tsx`). Documentado en docs/API.md. La parte «el LLM sigue generando si el cliente se va» es diseño declarado, no se toca.

### R-09 — BAJO — Heurística de deuda/capital (SOSPECHA)

- `scoring.rs:`(`debt_equity_ratio`): Yahoo ya se divide entre 100 en `yahoo.rs`; un D/E real > 25× (poco frecuente) se vuelve a dividir y puntúa como apalancamiento bajo. Edge case, sin reproducir.

- **Estado: ARREGLADO** — reproducida: Yahoo ya divide D/E entre 100 al parsear (`yahoo.rs`) y FMP entrega la razón, pero `scoring` volvía a dividir todo D/E > 25 (40x puntuaba como 0,4x). Se elimina `debt_equity_ratio`; el dominio documenta ratio. Test `scoring::tests::high_debt_to_equity_is_not_divided_again` (en rojo) y `debt_to_equity_is_a_ratio` (sustituye al test de la heurística de %).

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


---

# Segunda revisión (contexto del LLM, verificación de cifras, lint y hooks de React)

Revisor: agente de corrección, 2026-10-13. Alcance: `ai/src/context.rs`, `analysis/verify.rs`, `analysis/lint.rs`,
`fidelity.rs` (donde interactúan), y en el frontend `api/hooks.ts`, `realtime/`, `hooks/useChatStream.ts`, el chat y la
ficha de activo. Método: lectura + test que reproduce el fallo en ROJO → arreglo mínimo → VERDE. Donde no se pudo ver el
rojo se dice. Los ids siguen la numeración de la primera revisión.

## Resumen

| Impacto | Id | Fichero | Qué | Estado |
|---|---|---|---|---|
| Alto | R-10 | ai/context.rs `render_quote` | La variación del día se multiplicaba por 100 en el contexto (+0,2 % salía «+20.1%») | ARREGLADO |
| Medio | R-11 | ai/analysis/mod.rs | Las etapas veían más noticias que el verificador conocía: citas correctas descartadas | ARREGLADO |
| Medio | R-12 | ai/context.rs `render_fundamentals` | Ingresos, FCF, caja y deuda sin moneda junto a una capitalización en otra (ADRs) | ARREGLADO |
| Bajo | R-13 | ai/context.rs | Gastos de un ETF y rentabilidad por dividendo con un decimal: 0,03 % salía «0.0%» | ARREGLADO |
| Medio | R-14 | ai/fidelity.rs | Cifras en formato español mal leídas (`81.059,79`, `2057,45`, `-3,2 %`): falsos positivos Y negativos | ARREGLADO |
| Medio | R-15 | ai/fidelity.rs | «Caída máxima del 13.8%» se descartaba porque el dato es `-13.8%` | ARREGLADO |
| Medio | R-16 | ai/fidelity.rs | Falsos negativos: `5 %`, rangos `81000-82000`, pares `a/b`, `31.2x`, `3,2 billones`, `ema` en «sistema» | ARREGLADO |
| Bajo | R-17 | ai/fidelity.rs | `S&P 500`/`Nasdaq 100` contaban como cifras; un precio de 1990–2100 se tomaba por un año | ARREGLADO |
| Bajo | R-18 | ai/fmt.rs, levels.rs, verify.rs, gaps.rs | Proveedor en minúsculas en las bases y citas («binance», «yahoo») — TODO 4 | ARREGLADO |
| Bajo | R-19 | ai/analysis/verify.rs `cite` | La cita `[N1]` no se añadía si el texto contenía `[N10]` | ARREGLADO |
| Bajo | R-20 | ai/analysis/lint.rs | El lint marcaba texto válido («flujo de caja libre del año», nombres propios en inglés, palabra al final/inicio de frase) y `dedupe` fundía frases con cifras cortas distintas | ARREGLADO |
| Medio | R-21 | hooks/useChatStream.ts | Eventos pendientes de una respuesta abortada se mezclaban en la nueva | ARREGLADO |
| Medio | R-22 | pages/ChatPage.tsx | Tras un error (o «Detener») el texto parcial salía dos veces: la burbuja en vivo y el mensaje guardado | ARREGLADO |
| Bajo | R-23 | realtime/client.ts `reconnectNow` | Dejaba un WebSocket huérfano abierto si el reintento seguía conectando | ARREGLADO |
| Bajo | R-24 | realtime/client.ts | El backoff se reiniciaba al abrir: un servidor que acepta y cierra provoca reconexiones a ritmo fijo | ARREGLADO |
| Bajo | R-25 | api/hooks.ts `useSearch` | La clave de caché no incluía `limit` | ARREGLADO |
| Bajo | R-26 | api/hooks.ts `useHistory` | `keepPreviousData` enseñaría las velas de OTRO activo al cambiar de id (latente: `AssetView` lleva `key={id}`) | ARREGLADO |
| Bajo | R-27 | lib/indicators.ts `applyTickToCandle` | En velas diarias/semanales, los ticks del día siguiente reescribían la vela anterior | ARREGLADO |
| Medio | R-28 | frontend tests, `analysis/tests.rs` | Test intermitente bajo carga: `findBy*` de 1 s con chunk perezoso; y un test Rust con umbral de reloj | ARREGLADO |
| — | R-29 | api/hooks.ts `useGenerateAnalysis` | Análisis en curso al cambiar de activo: se pintaría el de otro activo | DESCARTADO |
| Medio | R-30 | app/SearchPalette.tsx | Enter justo tras teclear elegía un resultado de la búsqueda ANTERIOR (y lo anotaba en el historial con la nueva) | ARREGLADO |

## Hallazgos (detalle)

### R-10 — ALTO — El contexto multiplicaba por 100 la variación del día (VERIFICADO)

- `ai/src/context.rs::render_quote`: `val(q.change_pct, fmt::pct_signed)`. `pct_signed` espera una FRACCIÓN, pero
  `Quote.change_pct` viene en PUNTOS porcentuales en todos los proveedores (Yahoo `(p/prev-1)*100`, Binance
  `priceChangePercent` = `0.201`, CoinGecko, Finnhub `dp`…; el frontend ya lo trata así, `lib/format.ts`). `tools.rs` sí
  dividía entre 100; el contexto del análisis y la precarga del chat no.
- Por qué no se vio: `ai/src/fixture.rs` daba `change_pct` como fracción (`0.008`), así que todos los tests cuadraban.
- Impacto: el LLM leía «Variación del día | +20.1%» para un +0,2 % real y razonaba (y citaba) con ello; la verificación
  de cifras daba por buena la cifra porque estaba en el contexto. Afecta a todos los análisis y al chat.
- Reproducción: `context::tests::day_change_is_shown_in_percent_points_not_multiplied_by_100` (rojo: `+20.1%, (0.46)`).
- **Estado: ARREGLADO** — `pct_signed(c / 100.0)`; el fixture ahora usa puntos porcentuales como los proveedores reales.

### R-11 — MEDIO — La etapa de noticias veía noticias que el verificador no conocía (VERIFICADO)

- `analysis/mod.rs`: el contexto completo (`built`) fija la numeración y el índice, pero cada etapa se construía con su
  presupuesto y una COPIA del registro. Si el completo se degradaba (menos noticias), la etapa de sentimiento (solo
  noticias, cabe holgada) seguía mostrando hasta 10 (`[N9]`, `[N10]`…) y el `Verifier` solo conocía las del completo:
  un hallazgo correcto con `[N10]` se descartaba («la noticia [N10] no existe») y sus cifras no estaban en el índice.
- Con `num_ctx = 8192` el completo (3276 tokens) se degrada con 10-15 noticias reales: es el caso normal.
- Reproducción: `analysis::tests::news_shown_to_a_stage_can_be_cited_even_if_the_full_context_was_trimmed` (rojo: el
  digest del juez salía sin el hallazgo).
- **Estado: ARREGLADO** — todos los contextos de etapa y el del juez se construyen con el MISMO registro, antes de crear
  el `Verifier`, y cada texto se añade al índice (`NumberIndex::add_context`).

### R-12 — MEDIO — Cuentas anuales sin moneda junto a cifras en otra moneda (VERIFICADO)

- R-05 arregló el FCF yield de la puntuación, pero el contexto seguía imprimiendo `revenue TTM 2.9T`, `FCF 700B`,
  `cash`, `debt` sin moneda, al lado de una capitalización en USD (TSM: cuentas en TWD). El modelo los comparaba.
- Reproducción: `context::tests::statement_amounts_carry_their_currency_when_it_differs_from_the_price_currency`.
- **Estado: ARREGLADO** — si `extra.financialCurrency` difiere de la moneda de cotización, esas cuatro cifras llevan
  su moneda (`FCF 700B TWD`) y hay una fila «Monedas» que dice cuál es cuál. Misma moneda o desconocida: sin ruido.
  Limitación: `enterpriseValue` no se rotula (no se sabe en qué moneda lo da cada proveedor).

### R-13 — BAJO — Ratios pequeños con un solo decimal (VERIFICADO)

- `expense ratio` y `dividend yield` pasaban por `fmt::pct` (1 decimal): un ETF con 0,03 % de gastos salía «0.0%» (el
  test existente aceptaba «0.0%» como correcto). Nuevo `fmt::pct2` (2 decimales) para ambos.
- Test: `context::tests::crypto_and_etf_specific_rows` (rojo con «0.03%»), `fmt::tests::percentages`.
- **Estado: ARREGLADO.**

### R-14 — MEDIO — Cifras en formato español (TODO 4) (VERIFICADO)

- `fidelity::extract_numbers` solo entendía el inglés. Salidas medidas con el código anterior:
  `81.059,79 → [81.059, 79]`, `2057,45 → [2057 (exento: «año»), 45]`, `-3,2 % → [-3 (exento), 2 (exento)]`,
  `1.234.567 → [1.234, 567]`. Consecuencias: FALSOS POSITIVOS (la frase de escenario correcta con `81.059,79` se
  descartaba; en el chat salía «⚠ cifras no verificadas» sobre cifras correctas) y FALSOS NEGATIVOS (un `-3,2 %`
  inventado pasaba entero por exento).
- **Estado: ARREGLADO** — el extractor lee los dos formatos (tabla en el módulo y en `docs/notes/ai.md`). Regla de las
  ambiguas (`1.234`, `1,234`: ¿1,234 o 1234?): (1) si el MISMO texto trae una cifra inequívoca (`81.059,79`, `2057,45`
  → español; `1,234.5` → inglés) todas las ambiguas de ese texto se leen así; (2) si no, `value` es la lectura inglesa
  (la del contexto) y la española queda en `Num::alt`: se verifica si CUALQUIER lectura está en los datos. Un solo
  separador con otro número de dígitos que 3 nunca es ambiguo (`12.5` decimal, `2057,45` decimal español).
- Tests: `fidelity::tests::{spanish_number_formats_are_parsed_as_one_figure, english_formats_are_unchanged,
  ambiguous_thousands_are_decided_by_the_rest_of_the_text, spanish_figures_match_the_english_context}`,
  `every_figure_the_context_prints_parses_back_to_itself` (4000 magnitudes × 5 formateadores: lo que imprime el
  contexto se lee igual), `scenario_text_tests::spanish_formatted_figures_of_the_scenario_are_kept` (el filtro de
  escenarios), `chat::tests::spanish_formatted_figures_that_are_in_the_data_are_not_flagged`.

### R-15 — MEDIO — Cifras sin signo frente a datos negativos (VERIFICADO)

- El contexto dice `Caída máxima 1 año | -13.8%`; el modelo escribe, naturalmente, «caída máxima del 13.8%». La
  comparación era con signo: se descartaba una cifra correcta.
- **Estado: ARREGLADO** — con signo explícito (`-3.2`, `−3,2`, `+3.2`) el signo debe coincidir; sin signo, la cifra
  coincide con un dato NEGATIVO solo si la frase cercana expresa una bajada (caída, cayó, pérdida, por debajo, drawdown…).
  «Sube un 3.2%» frente a `-3.2%` sigue sin pasar. Test: `fidelity::tests::unsigned_figures_match_a_negative_datum_only_when_the_text_says_it_fell`.
  Límite: la dirección dicha en palabras solo se comprueba con esa lista de palabras (una frase «cayó… sube» pasa).

### R-16 — MEDIO — Falsos negativos del extractor (VERIFICADO)

Cifras inventadas que pasaban sin comprobarse, medidas con el código anterior:
- `5 %` (norma española, con espacio) se eximía como «entero pequeño»; `5%` pegado sí se comprobaba.
- Rangos y pares: `81000-82000` y `81059.79/84014.13` se tomaban por fechas (ambos extremos exentos).
- `31.2x`, `2.1bps` se tomaban por códigos (`52w`) por ir pegados a letras.
- `3.21 T`, `450 millones`, `2,5 mil millones`, `3,2 billones`: el sufijo separado o en palabra se ignoraba (la cifra
  quedaba sin escalar y, si era pequeña, exenta).
- Umbrales convencionales por subcadena: `ema` dentro de «sistema»/«problema», `atr` dentro de «atractivo».
- **Estado: ARREGLADO** — `%` con espacio y «por ciento»; fechas solo con patrón de fecha (`aaaa-mm-dd`, `dd/mm/aaaa`,
  `hh:mm`); sufijos `K/M/B/T` separados y palabras (`mil`, `millones`, `mil millones`, `billones` = 10^12,
  `billion` = 10^9…); las unidades pegadas solo eximen a enteros; umbrales por palabra. `mark_unverified` pone la marca
  tras el `%` aunque lleve espacio. Tests: `spaced_percent_is_still_a_percentage_not_a_small_integer`,
  `ranges_and_pairs_are_figures_but_dates_and_codes_are_not`, `scale_suffixes_in_all_their_spellings`,
  `multiples_and_units_with_decimals_are_figures`, `thresholds_need_the_indicator_as_a_word_not_as_a_substring`
  (este último escrito junto al arreglo, sin ejecutar el rojo).

### R-17 — BAJO — Nombres de índice y precios que parecen años

- `S&P 500`, `Nasdaq 100`, `IBEX 35`… contaban como cifras de datos (⚠ si no estaban en el contexto): ahora son nombres.
- Un entero entre 1990 y 2100 era siempre «año»: el oro, ETH o MELI cotizan ahí. Con moneda al lado (`2050 USD`, `$2050`,
  «2050 dólares») ya no se exime. Sin moneda («ETH a 2050») sigue pasando: es indistinguible de un año sin un calendario.
- **Estado: ARREGLADO** (con el límite dicho). Tests: `index_names_are_not_figures`, `a_price_that_looks_like_a_year_is_still_a_figure`.

### R-18 — BAJO — Nombre del proveedor en minúsculas (TODO 4)

- Las bases de niveles/escenarios («Precio = SMA50 81.059,79 (binance, 13 oct 2026)»), las citas de las afirmaciones
  («fuente: cotización yahoo 2026-…») y los avisos de datos usaban el id tal cual.
- **Estado: ARREGLADO** — `fmt::provider_name` (mismos nombres que `frontend/src/lib/sources.ts`: «Binance», «Yahoo
  Finance», «Alpha Vantage»…; un id desconocido en minúsculas se capitaliza). Se usa SOLO en texto que ve el usuario:
  `SourceRef.provider` y el contexto del modelo conservan el id (la UI lo usa para el enlace de verificación).
  Tests: `fmt::tests::provider_names_are_capitalised_for_the_user`,
  `levels::tests::the_source_in_the_notes_uses_the_provider_display_name`,
  `verify::tests::citations_show_the_provider_display_name_but_sources_keep_the_id` (rojo comprobado anulando la función).

### R-19 — BAJO — `cite` comparaba etiquetas por subcadena

- `!text.contains("N1")` es cierto si el texto lleva `[N10]`: una cifra respaldada solo por N1 salía sin `[N1]`.
  Test `verify::tests::news_tag_is_added_even_when_another_tag_contains_it_as_a_prefix` (rojo). **ARREGLADO** (por número de cita).

### R-20 — BAJO — El lint rompía texto válido

- «caja libre del» se marcaba como calco aun dentro de «flujo de caja libre del año» (disparaba una llamada de
  corrección y un aviso falso); «palabra repetida» saltaba con la misma palabra al final de una frase y al principio de
  la siguiente; «price/growth/the» saltaban en nombres propios («T. Rowe Price», «Vanguard Growth ETF», «The Home Depot»);
  `dedupe_sentences` ignoraba las palabras de < 3 caracteres, incluidas las cifras: «PER 31» y «PER 29» eran duplicadas.
- Tests `lint::tests::{valid_spanish_is_not_flagged, sentences_that_differ_only_in_a_short_number_are_not_duplicates}`
  (rojos). **ARREGLADO.**

### R-21 — MEDIO — Eventos de una respuesta abortada se mezclaban con la nueva (VERIFICADO)

- `useChatStream` guardaba el texto en un `ref` compartido. `parseSseStream` no mira `signal` entre eventos de un mismo
  chunk, así que tras `abort()` (cambio de conversación, «Nueva conversación») la ejecución anterior seguía entregando
  los eventos ya decodificados y los sumaba al búfer de la nueva: `B1` + `A2 A3`.
- Test `hooks/useChatStream.test.tsx` (rojo: `'B1A2 A3 '`). **ARREGLADO** — cada `send` comprueba que sigue siendo la
  vigente antes de tocar búfer o estado.

### R-22 — MEDIO — Texto parcial pintado dos veces (VERIFICADO)

- Desde R-08 el servidor guarda la respuesta cortada por un error (`truncated`). La UI conservaba además la burbuja en
  vivo (cuando hay `error` o tras «Detener» el estado no se limpia): tras el refetch salían el mensaje guardado (con su
  aviso) Y la burbuja en vivo con el mismo texto.
- Test `pages/ChatPage.test.tsx` (rojo: 2 párrafos con el texto). **ARREGLADO** — el estado en vivo recuerda cuántos
  mensajes tenía el hilo al enviar (`base`); cuando el hilo guardado pasa de `base + 1`/`base + 2` se oculta la pregunta/
  respuesta en vivo. Si el servidor no guardó nada, el aviso del error sigue visible (test).

### R-23 / R-24 — BAJO — WebSocket

- R-23: `reconnectNow()` (eventos `online`/`visibilitychange`) con un reintento aún en el handshake creaba otro socket sin
  cerrar el anterior (huérfano). Ahora lo descarta y lo cierra.
- R-24: `attempt = 0` en `onopen`: un servidor que acepta y cierra enseguida reconectaba cada ≈ `initialMs` para siempre.
  Ahora el backoff solo se reinicia tras `stableAfterMs` (10 s) abierto. Se cambió el test «el backoff se reinicia al abrir»
  (que fijaba el comportamiento anterior) por «…tras una conexión estable».
- Resuscripción tras reconectar, refcount, ping/pong y fugas de listeners: ya estaban cubiertos y correctos (ver abajo).
- Tests `realtime/client.test.ts` (rojos). **ARREGLADO.**

### R-25 / R-26 — BAJO — Claves de React Query

- `qk.search(q, cls)` no incluía `limit` aunque cambia la petición: dos llamadas con límites distintos compartían caché.
- `useHistory` con `placeholderData: keepPreviousData` devuelve la serie del activo anterior al cambiar el `id`. Hoy no se
  ve porque `AssetView` lleva `key={id}`; con cualquier otro uso del hook enseñaría velas de otro activo. Ahora solo
  conserva las previas si son del mismo activo (rango/intervalo distintos sí).
- Tests `api/hooks.test.tsx` (rojos). Revisadas y correctas: `history`, `news`, `quote`, `indicators`, `fundamentals`,
  `analysis(es)`, `snapshot`, `quotes`, `conversation(s)`. **ARREGLADO.**

### R-27 — BAJO — Ticks sobre velas diarias/semanales

- `applyTickToCandle` trataba todo intervalo ≥ 1 día como «siempre la última vela»: con la pestaña abierta al cruzar las
  00:00 UTC en cripto, los ticks de hoy reescribían máximo/mínimo/cierre de la vela de ayer. Ahora diarias y semanales
  (duración fija) abren vela nueva; las mensuales (duración variable) siguen actualizando la última.
- Test `lib/indicators.test.ts` (rojo comprobado revirtiendo la línea). **ARREGLADO.** Limitación: sin calendario de la
  bolsa, la vela nueva de una acción nace en `t_anterior + k·86400` (puede diferir 1 h en el cambio de hora y un tick
  de pre-mercado puede caer en un día no hábil); se corrige al recargar.

### R-28 — MEDIO — Test intermitente bajo carga (TODO 2)

- **Causa**: `App.test.tsx` esperaba con `findBy*` de 1 s (el valor por defecto) a páginas cargadas con `React.lazy`:
  el chunk se transforma y evalúa la primera vez que se importa, y con la CPU saturada tarda más de 1 s
  («Unable to find role="heading" and name "Panel"»). En saturación fuerte también caían tests síncronos por el
  `testTimeout` de 5 s.
- **Reproducción**: 16 procesos `yes` + 3 `vitest` a la vez (≈ 13× más lento): 17 de 33 ejecuciones fallaban, siempre en
  `App.test.tsx` (14× «Unable to find … Panel», 7× «Test timed out in 5000ms»). Test determinista:
  `App.test.tsx › espera por condición, no por reloj…` (chunk de Favoritos que tarda 1,5 s: rojo con el timeout por defecto).
- **Estado: ARREGLADO** — `asyncUtilTimeout: 10_000` global (las esperas siguen terminando en cuanto se cumple la
  condición), `testTimeout`/`hookTimeout` de 30 s, precarga de los chunks perezosos en `beforeAll`, y stub de
  `scrollIntoView` (jsdom no lo tiene). Estrés tras el arreglo, mismas cargas que antes: 12 `yes` + 1 `vitest` × 30 →
  30/30 verdes; 16 `yes` + 3 `vitest` en paralelo × 8 (24 ejecuciones, ≈ 29 s cada una frente a 4 s sin carga) → 24/24
  verdes (antes 17 de 33 fallaban). Con 12 `yes` + 1 `vitest` × 40 el código anterior ya pasaba 40/40: la carga de 6
  núcleos que sugería el TODO no basta en una máquina de 20 hilos; hace falta saturar la CPU.
- Mismo problema en Rust: `analysis::tests::parallel_stages_overlap_when_configured` exigía `< 600 ms` de reloj y falló
  (925 ms) con la máquina cargada. Ahora mide el pico de respuestas abiertas a la vez (`MockState::max_open_streams`).

### R-29 — DESCARTADO — Análisis en curso al cambiar de activo

- Hipótesis: al navegar a otra ficha con un análisis en curso se pintaría el de otro activo. No ocurre: `AssetPage`
  monta `AssetView` con `key={id}`, el estado de `useGenerateAnalysis` (etapas, `isPending`, `data`) es por instancia, y
  `onSuccess` escribe con el `id` capturado al llamar a `mutate`. Test de caracterización
  `api/hooks.test.tsx › useGenerateAnalysis: carreras al cambiar de activo…` (verde sin cambios). Efecto conocido y
  asumido: al salir de la ficha el SSE sigue abierto hasta que termina el análisis (queda guardado; si se vuelve a la
  ficha antes, «Generar» se une al trabajo en curso por single-flight).

### R-30 — MEDIO — El buscador elegía un resultado de la búsqueda anterior (VERIFICADO)

- `SearchPalette` mantiene los resultados previos (`keepPreviousData`) mientras corre el antirrebote (180 ms) y llega la
  respuesta. Si el usuario teclea «te», ve «Tesco» y sigue con «sla» + Enter sin esperar, `flat[sel]` seguía siendo
  «Tesco»: navegaba a su ficha y anotaba en el historial `tesla → Tesco`.
- Test `app/SearchPalette.test.tsx` (rojo: `recorded = [['tesla', 'stock:TSCO']]`). **ARREGLADO** — Enter se ignora hasta
  que los resultados que se ven son los del término actual (`fresh`); clic sobre un resultado anterior sigue funcionando
  y anota la consulta que realmente lo produjo.

## Lo revisado en la segunda revisión que está bien (no repetir)

| Área | Comprobación | Resultado |
|---|---|---|
| `context.rs` | Unidades: rendimientos, márgenes, ROE, caída máxima, distancia al máximo 52 s, dividend yield como FRACCIÓN según `domain`; ATR% sobre el precio; soportes/resistencias ordenados | Correcto (salvo R-10, R-13) |
| `context.rs` | Cabeceras con fuente + hora UTC, noticias numeradas por fecha desc., degradación por niveles solo recorta (nunca cambia cifras), `render_part` == contexto del análisis | Correcto |
| `verify.rs` | `calibrate` (tope 0,85, +0,15 sobre la puntuación, descartes/⚠), `sources()`, orden y deduplicado de fuentes, `news_supports` | Correcto (salvo R-19) |
| `fidelity.rs` | Tolerancia (media unidad del decimal menos preciso), segmentos, `mark_unverified`, exenciones (RSI 70/30, medias 20/50/200, ATR14…) | Correcto (ver R-14…R-17) |
| `lint.rs` | Rangos de alfabetos, plantillas de tesis, `specificity` | Correcto (salvo R-20) |
| `chat.rs` | La verificación final usa el mismo índice; el aviso lista hasta 6 cifras | Correcto |
| Hooks | Claves de `history`, `news`, `quote`, `analysis`, `snapshot`, `quotes`, `conversation`; mutaciones optimistas con `cancelQueries` + reversión; `useModelPulls` aborta al desmontar | Correcto |
| `RealtimeClient` | Refcount con margen, resuscripción de lo vigente tras reconectar, backoff con tope y jitter, ping/pong, `dispose`, sin fugas de listeners (`subscribeStore`/`onTick` liberan el `Set`) | Correcto (salvo R-23, R-24) |
| `LivePrice`/`usePriceTicks` | `useSyncExternalStore` por id; el tick de otro activo no llega; `cb` por ref sin resuscribir | Correcto |
| `AnalysisReport › Snapshot` | Muestra el contexto ACTUAL del activo y lo dice («puede diferir del que vio este análisis») | Correcto |

## Límites que quedan (no son arreglos)

- Los enteros ≤ 12 sin unidad siguen exentos (no se pueden comprobar: «3 razones», «PER 8»); un entero inventado de ese
  tamaño pasa. Los decimales sí se comprueban.
- La verificación es por VALOR: una cifra correcta con la etiqueta equivocada («PER 61.3» cuando 61.3 es el RSI) pasa.
- `clean_citations` solo quita citas de una noticia (`[N4]`); en `[N1, N4]` no separa la que no respalda.
- En el recorte duro del contexto (ni el nivel mínimo cabe) el registro conserva noticias que el texto recortó.
- `yield_to_maturity`/`coupon` (bonos) se imprimen como puntos porcentuales: ningún proveedor los rellena hoy.
- En cripto la cabecera dice `moneda USD` (resumen de CoinGecko) y el precio `USDT` (Binance): ambas se nombran, no hay
  cifra ambigua, pero no son la misma moneda.

## Para el coordinador

- `docs/API.md` y el comentario de `domain::Quote.change_pct` / `PriceTick.change_pct` deberían decir «puntos
  porcentuales (1.2 = +1,2 %)»: el contrato solo estaba implícito y el crate `ai` lo leyó mal (R-10). No he tocado `domain`.
- `ai::fixture` ahora da `change_pct` en puntos porcentuales; nada fuera de `ai` lo usaba.
- No hay cambios en `market`, `server`, `./invest-ai` ni `packaging/`.
