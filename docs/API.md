# Contrato de la API (backend ⇄ frontend)

Base: `http://127.0.0.1:8787/api` (en desarrollo, Vite hace de proxy de `/api` → `127.0.0.1:8787`).
Los tipos JSON son exactamente los de `backend/crates/domain/src/lib.rs` (structs en camelCase,
enums en snake_case). `frontend/src/api/types.ts` los refleja 1:1.

`{id}` es un `AssetId` (`"stock:AAPL"`, `"crypto:BTC"`, `"index:^GSPC"`); en la URL va
percent-encoded (`encodeURIComponent`).

## Unidades

- `changePct` (en `Quote` y `PriceTick`) va en **puntos porcentuales**: `1.2` = +1,2 %. No es una fracción.
  Confundirlo hizo que el contexto del LLM mostrara la variación del día multiplicada por 100 (R-10 en
  `docs/notes/review.md`).

## Errores

Cualquier error responde con JSON `{ "error": { "code": string, "message": string } }`.

| DomainError    | HTTP | code           |
|----------------|------|----------------|
| NotFound       | 404  | not_found      |
| InvalidInput   | 400  | invalid_input  |
| RateLimited    | 429  | rate_limited   |
| Upstream       | 502  | upstream       |
| Unavailable    | 503  | unavailable    |
| Internal       | 500  | internal       |

Errores de HTTP que no vienen del dominio:

| HTTP | code | Cuándo |
|------|------|--------|
| 403  | forbidden | `Host` no es loopback ni `FRONTEND_ORIGIN` (anti DNS rebinding); `Origin` no permitido o `Sec-Fetch-Site: cross-site` en POST/PUT/PATCH/DELETE y en el WebSocket |
| 413  | payload_too_large | cuerpo > 256 KiB |
| 415  | unsupported_media_type | cuerpo no vacío sin `Content-Type: application/json` |
| 429  | rate_limited | tope de WebSockets (64), de chats simultáneos (4) o de análisis simultáneos (2) |

## Sistema

| Método | Ruta              | Respuesta |
|--------|-------------------|-----------|
| GET    | `/health`         | `{ "status": "ok", "version": string }` |
| GET    | `/status`         | `{ "llm": LlmStatus, "providers": ProviderStatus[], "outbound": { "host": string, "requests": number, "avgMs": number }[] }` (`outbound`: peticiones HTTP salientes a proveedores desde el arranque y latencia media hasta las cabeceras; diagnóstico de caché/single-flight) |

## Mercado

| Método | Ruta | Query | Respuesta |
|--------|------|-------|-----------|
| GET | `/search` | `q` (obligatorio), `limit` (def. 20, máx. 50), `class` (opcional, AssetClass) | `AssetSummary[]` — registra la búsqueda en el historial |
| GET | `/assets/{id}` | — | `AssetDetail` — registra una visita al activo |
| GET | `/assets/{id}/quote` | — | `Quote` |
| GET | `/quotes` | `ids` (lista separada por comas, máx. 50) | `Quote[]` |
| GET | `/assets/{id}/history` | `range` (def. `1y`), `interval` (def. `range.default_interval()`) | `Candle[]` |
| GET | `/assets/{id}/indicators` | — | `Indicators` |
| GET | `/assets/{id}/fundamentals` | — | `Fundamentals \| null` |
| GET | `/assets/{id}/news` | `limit` (def. 30) | `NewsItem[]` |
| GET | `/news` | `limit` (def. 30) | `NewsItem[]` (mercado general) |
| GET | `/macro` | — | `MacroContext` |

### Procedencia verificable

Toda cifra de mercado debe poder comprobarse en su fuente. Campos añadidos (todos opcionales, `null`
si no se conocen; nunca se inventa una URL):

| Tipo | Campos nuevos |
|------|---------------|
| `AssetSummary` | `sourceUrl` |
| `Quote`, `Fundamentals`, `MacroIndicator` | `sourceUrl`, `timestampOrigin: "provider" \| "local"` |
| `Indicators` | `source` (proveedor de las velas usadas en el cálculo), `sourceUrl` |
| `ProviderStatus` | `latencyMs` (última petición), `blockedSecs` (pausa restante tras un 429) |

- **Convención de nulos**: los `Option` sin valor se serializan SIEMPRE como `null` (nunca se omiten, no hay
  `skip_serializing_if`); los campos nuevos siguen la misma regla y el frontend los declara `?: T | null`
  para tolerar backends antiguos que no los envíen (`#[serde(default)]` en el dominio).
- `timestampOrigin = "provider"`: `timestamp`/`asOf` es la hora que dio el proveedor. `"local"`: el
  proveedor no la da y es la hora de la consulta (Yahoo/Finnhub en `Fundamentals.asOf`; Binance/CoinGecko
  si faltara el campo).
- `Quote.delaySecs`: **segundos de retraso que declara el proveedor para el feed que sirve** (0 = tiempo real, `null` =
  desconocido). Es una declaración, no una medida: la antigüedad real del dato es `ahora − timestamp` (con
  `timestampOrigin = "provider"`) y es lo que la UI muestra como cifra principal. En Yahoo `exchangeDataDelayedBy` viene
  en **minutos** y es el retraso estándar de la bolsa, no el del feed (AAPL declara 15 min y llega con ~3 s); el backend
  lo convierte a segundos y devuelve 0 cuando `quoteSourceName` dice «Real Time». Ver `docs/notes/market.md`.
- `NewsItem.url` ya es el enlace al artículo/filing original (no necesita `sourceUrl`).
- `GET /assets/{id}/history` sigue devolviendo `Candle[]`; la procedencia va en cabeceras de respuesta:
  `X-Data-Source` (proveedor que sirvió las velas), `X-Data-Source-Url`, `X-Data-As-Of` (inicio de la
  última vela, ISO 8601) y `X-Data-Interval` (`1m|5m|15m|30m|1h|1d|1wk|1mo`: intervalo **realmente servido**,
  deducido de la separación entre velas; difiere del pedido cuando el proveedor sube de intervalo porque
  la combinación rango/intervalo no cabe, p. ej. `5d/1m` en Binance se sirve a `5m`; la UI lo avisa).
  Faltan si el backend no la conoce (p. ej. el servidor simulado). Con la sesión abierta Yahoo añade a
  las series intradía un punto «en vivo» desalineado (`o=h=l=c`, volumen 0, hora del último trade); el
  servidor lo pliega en la vela de su tramo (actualiza su cierre/máximo/mínimo) o, si ese tramo aún no
  tiene vela, la abre alineada a su inicio: todas las velas caen siempre en fronteras del intervalo. La API se
  sirve en el mismo origen que la UI (o a través del proxy de Vite), por lo que no hace falta
  `Access-Control-Expose-Headers`; solo haría falta si el frontend se alojara en otro origen.

```ts
type AssetDetail = {
  summary: AssetSummary;
  quote: Quote | null;
  fundamentals: Fundamentals | null;
  isFavorite: boolean;
};
```

### Tiempo real — WebSocket `GET /ws`

Un único socket por pestaña, multiplexado.

Cliente → servidor:
```json
{ "type": "subscribe",   "ids": ["crypto:BTC", "stock:AAPL"] }
{ "type": "unsubscribe", "ids": ["stock:AAPL"] }
{ "type": "ping" }
```
Servidor → cliente:
```json
{ "type": "tick",  "tick": PriceTick }
{ "type": "pong" }
{ "type": "error", "message": string }
```
El servidor agrupa ticks del mismo activo (como mucho ~4 por segundo y activo) para no saturar el
navegador.

`tick.ts` es siempre la hora **del proveedor**, no la de envío. Al suscribirse (o al unirse a un activo
que ya tiene suscriptores) el servidor reenvía de inmediato el último tick conocido con su `ts` original:
en pares cripto poco líquidos puede tener varios segundos (Binance `miniTicker` solo emite al haber
cambios), y no es retraso del servidor. Medido (13-oct-2026, 20 clientes x 10 pares): ticks en vivo
p50 138 ms / p99 234 ms; el primer tick de cada activo, p50 ~1 s. Las acciones sin feed WS se sondean
cada 5 s con el mercado abierto y solo emiten si cambió el precio o la hora (a 60 s si el mercado está
cerrado/pre/post y no hay cambios). Con `FINNHUB_API_KEY` las acciones/ETF de EE. UU. tienen feed WS
(`tick.source = "finnhub"`, trades reales); sin clave, o mientras ese WS no esté conectado, se sondean.
`Quote.sourceUrl` es `null` para Finnhub y Twelve Data (no tienen página pública por activo).

## Análisis de IA

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET  | `/assets/{id}/analysis` | `Analysis \| null`: el último guardado (se cachea en BD) |
| POST | `/assets/{id}/analysis` | `Analysis`: genera uno nuevo con snapshot en vivo (puede tardar 10–60 s) |
| POST | `/assets/{id}/analysis/stream` | SSE (`data: <json>\n\n`) de `AnalysisEvent` mientras se genera (ver «Análisis en streaming»). Mismo trabajo único por activo que el POST; guarda en BD; sigue si el cliente se va |
| GET  | `/assets/{id}/analyses` | `Analysis[]`: histórico de análisis del activo (máx. 20, más reciente primero) |
| GET  | `/assets/{id}/snapshot` | `AssetSnapshot`: el contexto exacto que ve el LLM (transparencia) |

## Rendimiento medido y lista de seguimiento

Resultados reales de los análisis (crate `learning`, ver `docs/notes/learning.md`) y los análisis diarios automáticos que los alimentan.

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET  | `/learning/performance` | `PerformanceReport` (camelCase; tal cual lo devuelve `learning::performance_report` con el modelo activo) |
| GET  | `/learning/weights` | `{ active: WeightVersion \| null, versions: WeightVersion[] }` (más reciente primero) |
| POST | `/learning/weights/rollback` | Cuerpo `{ "version": 3 }` o `{ "version": null }` (pesos por defecto). Crea una versión nueva `rollback` y la activa; devuelve `WeightVersion`. 404 `not_found` si la versión no existe. Método no seguro: pasa por el guard de `Origin` |
| GET  | `/learning/calibrated-confidence?confidence=0.7&horizon=4&model=qwen3:8b` | `{ raw, calibrated \| null, horizonWeeks, n, minRequired, reason }`. `calibrated` solo existe con calibración aplicable (>= 200 análisis independientes y mejora fuera de muestra); `horizon` en {1, 4, 12} (por defecto 4). `model` (opcional) es el modelo con cuyo historial se calibra: el que hizo el análisis, porque la confianza de otro modelo significa otra cosa; sin él, el modelo activo; vacío = 400 |
| GET  | `/watchlist` | `WatchlistStatus` |

```ts
type PerformanceReport = {
  generatedAt: string; ruleVersion: number; analysesTotal: number
  horizons: {                       // 3 elementos: 1, 4 y 12 semanas
    horizonWeeks: 1 | 4 | 12
    analysesTotal: number; resolved: number; resolvedIndependent: number
    pending: number; notMatured: number; skipped: number
    baseRateOutperform: number | null           // tasa base de superar al índice
    overall: VerdictStats; byVerdict: VerdictStats[]   // verdict "all" en overall
    calibration: { horizonWeeks: number; n: number; minRequired: number; applied: boolean; reason: string
      brierRaw: number | null; brierHoldoutRaw: number | null; brierHoldoutCalibrated: number | null
      reliability: { lo: number; hi: number; n: number; meanConfidence: number; hitRate: number; ciLow: number; ciHigh: number }[] }
  }[]
  activeWeights: WeightVersion | null; weightHistory: WeightVersion[]
  caveats: string[]
}
type VerdictStats = { verdict: string; n: number; hits: number; hitRate: number | null; hitRateCi: [number, number] | null
  meanExcess: number | null; meanExcessCi: [number, number] | null; clusters: number }
type WeightVersion = { version: number; kind: "learned" | "rollback" | "decay" | "manual"; status: "active" | "retired"
  multipliers: number[]; factorNames: string[]; createdAtMs: number; horizonWeeks: number | null
  trainedToMs: number | null; validatedFromMs: number | null; validatedToMs: number | null
  nTrain: number | null; nVal: number | null; metrics: object; parentVersion: number | null; note: string | null }

type WatchlistStatus = {
  enabled: boolean; defaultList: boolean      // defaultList: la lista a priori (WATCHLIST vacío)
  time: string; depth: AnalysisDepth          // hora local "HH:MM"
  nextRun: string | null                      // próxima pasada programada
  running: boolean                            // hay una pasada en marcha
  assets: { id: AssetId; last: WatchlistResult | null; lastOkAt: string | null }[]   // último intento de cada activo
  lastRun: WatchlistRun | null; lastRunResults: WatchlistResult[]
  recentRuns: WatchlistRun[]                  // las 7 últimas, más reciente primero
  analysesTotal: number                       // análisis guardados con origin = "watchlist"
}
type WatchlistRun = { id: number; localDate: string; scheduledFor: string; startedAt: string; finishedAt: string | null
  trigger: "schedule" | "recovery" | "retry"  // recovery: el servidor arrancó tarde; startedAt dice cuándo fue de verdad
  status: "running" | "completed" | "aborted" | "interrupted"; depth: string; model: string | null
  planned: number; power: "ac" | "battery" | "unknown"; powerWaitSecs: number; ok: number; failed: number }
type WatchlistResult = { id: number; runId: number; localDate: string; assetId: AssetId
  status: "ok" | "failed" | "skipped"         // skipped: no se intentó porque la pasada se abortó
  transient: boolean; attempts: number; analysisId: string | null; verdict: string | null
  errorCode: string | null; error: string | null; startedAt: string; finishedAt: string }
```

Con `INVEST_AI_FAKE=1` la lista de seguimiento queda desactivada (`enabled: false`).

## Conversaciones (persistentes)

| Método | Ruta | Body | Respuesta |
|--------|------|------|-----------|
| GET    | `/conversations` | — | `Conversation[]` (más reciente primero) |
| POST   | `/conversations` | `{ title?: string, assetIds?: AssetId[] }` | `Conversation` |
| GET    | `/conversations/{cid}` | — | `{ conversation: Conversation, messages: ChatMessage[] }` |
| PATCH  | `/conversations/{cid}` | `{ title?, assetIds?, pinned? }` | `Conversation` |
| DELETE | `/conversations/{cid}` | — | 204 |
| POST   | `/conversations/{cid}/messages` | `{ content: string, assetIds?: AssetId[] }` | **SSE** de `ChatEvent` |

`ChatMessage.truncated` (bool, opcional, `false` si falta): `true` si un error cortó la respuesta del
asistente y `content` es parcial (la UI lo avisa).

SSE: cada evento es `data: <ChatEvent JSON>\n\n`. Termina con `done` o `error`. El servidor
guarda el mensaje del usuario antes de empezar y el del asistente (con tool calls y análisis) al
terminar, también si el cliente se desconecta. Si la conversación no tiene título, se genera a
partir del primer mensaje.

## Favoritos, historial y preview

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET    | `/favorites` | `Favorite[]` |
| PUT    | `/favorites/{id}` | `Favorite` (body opcional `{ note?: string }`) |
| DELETE | `/favorites/{id}` | 204 |
| GET    | `/history/searches` | `SearchEntry[]` (máx. 50) |
| DELETE | `/history/searches` | 204 |
| GET    | `/history/views` | `ViewStat[]`: activos más vistos |
| GET    | `/dashboard` | `Dashboard` |

```ts
type Favorite    = { id: AssetId; summary: AssetSummary; note: string | null; addedAt: string };
type SearchEntry = { query: string; selected: AssetId | null; at: string };
type ViewStat    = { id: AssetId; summary: AssetSummary; views: number; lastViewedAt: string };
type Dashboard = {
  favorites: { favorite: Favorite; quote: Quote | null }[];
  mostViewed: { stat: ViewStat; quote: Quote | null }[];   // top 8, ponderado por recencia
  recentSearches: SearchEntry[];                            // 10
  recentConversations: Conversation[];                      // 5
  latestAnalyses: Analysis[];                               // 5
  market: Quote[];  // panel fijo: ^GSPC, ^IXIC, ^STOXX50E, ^IBEX, crypto:BTC, crypto:ETH, GC=F, CL=F, EURUSD=X, ^TNX, ^VIX
  news: NewsItem[]; // 10 titulares de mercado
};
```

Para registrar qué activo eligió el usuario tras una búsqueda:
`POST /history/searches { query: string, selected: AssetId }` → 204.


## Análisis en streaming y campos nuevos de `Analysis`

`POST /assets/{id}/analysis/stream?depth=quick|standard|deep` responde `text/event-stream`; cada evento es JSON:

```ts
type AnalysisEvent =
  | { type: "stage"; stage: string; label: string; index: number; total: number }  // index desde 1
  | { type: "done"; analysis: Analysis }
  | { type: "error"; code: string; message: string };   // code = el de la tabla de errores (p. ej. "rate_limited")
```

`stage` es un id estable: `snapshot`, `technical`, `fundamental`, `sentiment`, `macro_risk`, `debate` (solo deep),
`judge`, `verify`. En `quick` la etapa `technical` es el análisis combinado (su `label` lo dice). `total` = 4 (quick),
7 (standard), 8 (deep). Dos clientes del mismo activo comparten un único análisis (el tardío recibe la etapa en curso y
el final). Con demasiados análisis simultáneos: `429` en el POST y evento `error` con `code: "rate_limited"` en el SSE.

Campos añadidos a `Analysis` (todos opcionales, `null`/ausentes en análisis antiguos):

```ts
type Analysis = { /* ...los de siempre... */
  levelsNote?: string | null;          // por qué faltan (o cómo se calcularon) entrada, stop y objetivos
  scenarios?: Scenario[] | null;       // solo deep; las probabilidades suman 100
  expectedReturnPct?: number | null;   // Σ prob × returnPct de los escenarios
  wouldChangeIf?: string[] | null;     // qué haría cambiar el veredicto
  horizonNote?: string | null;         // por qué ese horizonte
  positionSizing?: PositionSizing | null;
  method?: string | null;              // modelo, profundidad, llamadas, tokens, tiempo, aviso de no asesoramiento
  qualityNotes?: string[] | null;      // descartes, cifras sin verificar, reintentos, redacción dudosa
  depth?: "quick" | "standard" | "deep" | null;   // metadatos del método, para mostrar tiempos reales
  durationMs?: number | null;
  llmCalls?: number | null;            // incluye reintentos y correcciones
  promptTokens?: number | null;
  completionTokens?: number | null;
};
type Scenario = { kind: "bullish" | "base" | "bearish"; probability: number; description: string;
  targetPrice: number | null; targetBasis: string | null; returnPct: number | null };
type PositionSizing = { maxPortfolioPct: number; riskPerTradePct: number | null; stopDistancePct: number | null;
  atrPct: number | null; volatility30d: number | null; rationale: string };
```

Cambios de significado: `keyLevels.entryZone/stopLoss/targets` los **calcula el backend** (stop = soporte − 1×ATR14,
objetivos = resistencias y niveles de los datos), no el modelo; `null` = sin base en los datos (ver `levelsNote`).
`dataWarnings` contiene solo carencias reales de los datos; la nota de método ya no va ahí sino en `method`.

## OPV de EE. UU. (tipos en `domain`; perfil de riesgo en `docs/notes/ipo-risk.md`)

Los datos vienen de `MarketService` (métodos inherentes, expuestos al servidor por el trait `IpoData`; no forman parte de `MarketData`). Todos los `Option` sin valor viajan como `null`.

```ts
type IpoEntry = { date: string /*YYYY-MM-DD*/; symbol: string | null; name: string; exchange: string | null;
  status: "expected" | "priced" | "filed" | "withdrawn" | "unknown";
  priceLow: number | null; priceHigh: number | null;   // iguales = precio definitivo
  shares: number | null; totalValue: number | null;    // USD; en expected incluye +15 % de sobreasignación
  spacLikely: boolean; source: "finnhub" | "sec"; sourceUrl: string | null; asOf: string };
type ProspectusSection = { key: "summary" | "offering" | "summary_financial_data" | "risk_factors" | "use_of_proceeds"
  | "dividend_policy" | "capitalization" | "dilution" | "principal_stockholders" | "shares_eligible" | "lock_up" | "underwriting";
  title: string; text: string; parent: string | null; truncated: boolean; originalChars: number };
type Prospectus = { issuer: string; cik: number; ticker: string | null; form: string; preliminary: boolean;
  filedOn: string; accession: string; url: string; indexUrl: string; offerPrice: number | null;
  sections: ProspectusSection[]; missing: string[]; documentChars: number; source: "sec"; fetchedAt: string };
type IpoReturnPoint = { horizon: "1m" | "3m" | "6m" | "12m"; targetDate: string; priceDate: string | null; price: number | null;
  returnPct: number | null; sinceFirstClosePct: number | null; benchmarkReturnPct: number | null; excessPct: number | null };
type IpoPerformance = { symbol: string; ipoDate: string; offerPrice: number; offerPriceSource: "manual" | "calendar" | "prospectus";
  firstDayClose: number; firstDayReturnPct: number; firstDayHighPct: number; points: IpoReturnPoint[];
  latestPrice: number; latestDate: string; latestReturnPct: number; maxDrawdownPct: number /* <= 0 */;
  benchmark: string; source: "yahoo"; sourceUrl: string | null; warnings: string[]; asOf: string };
```
Porcentajes en puntos porcentuales (`12.5` = +12,5 %). Un horizonte aún no alcanzado lleva `null` (nunca se extrapola). `url`/`sourceUrl` apuntan al documento original para verificar.

### Rutas `/api/ipo/*`

El perfil es un **perfil de riesgo determinista** (reglas sobre el folleto; sin LLM), no una recomendación ni una predicción.
`{symbol}` se normaliza a mayúsculas y solo admite letras, cifras, `.` y `-` (máx. 12; además de la validación de `market::symbol`).
Los errores de proveedor se devuelven genéricos (`502 upstream` «el proveedor de datos no respondió»; el detalle va al log del servidor).

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET | `/ipo/calendar?from=&to=` | `IpoEntry[]` ordenado por fecha. `from`/`to` = `YYYY-MM-DD`. Sin parámetros: hoy−30 → hoy+60 días. Con solo `from`: `from` → `from`+90. **400** si `from > to`, rango > 180 días, fecha inexistente o fuera de [hoy−5 años, hoy+1 año]. Caché 30 min en `market`. |
| GET | `/ipo/{symbol}/prospectus?maxChars=` | `Prospectus` del folleto de la OPV (424B4/424B1; si no hay definitivo, el último S-1/A o F-1/A con `preliminary: true`). Cada sección se recorta a `maxChars` (def. 12 000; 500–60 000; `truncated`/`originalChars` lo indican). **404** si no hay folleto. Caché 6 h. |
| GET | `/ipo/{symbol}/performance?date=&offerPrice=&benchmark=SPY` | `IpoPerformance`: día 1 y 1/3/6/12 meses frente a `SPY`, `IWM` o `QQQ`. Sin `date`, la del calendario (debe estar con precio fijado); `offerPrice` opcional (0–100 000). **400** si `date` es futura o `benchmark` no admitido; **404** si la OPV no está en el calendario. Caché 1 h. |
| GET | `/ipo/recent-performance?months=12` | `IpoBaseRate`: **tasa base** = medianas del primer día y de los 3 meses de las OPV con precio definitivo (sin SPAC) de los últimos `months` meses (1–24, def. 12), con el tamaño de muestra de cada una (`nDay1`, `n3m`). Muestra de hasta 30 OPV repartidas por la ventana. Se calcula una vez y se cachea 6 h; en frío tarda ~15–25 s (si supera el tiempo de espera, **503** y el cálculo sigue en segundo plano: reintenta). |
| GET | `/ipo/{symbol}/risk-profile?date=` | `IpoRiskProfile`: indicadores + tasa base + avisos. `date` (opcional) acota la búsqueda en el calendario (±7 días). Si el folleto no está, devuelve el perfil con lo que haya (calendario) y todo lo demás `not_extracted`, más un aviso. **404** solo si no hay ni calendario ni folleto. Si la tasa base tarda más de 12 s se sirve sin ella (aviso) y se completa en la siguiente petición. |

```ts
type IpoRiskProfile = { symbol: string; name: string | null; entry: IpoEntry | null;
  prospectus: { form: string; preliminary: boolean; filedOn: string; url: string; indexUrl: string; documentChars: number; missing: string[] } | null;
  indicators: IpoRiskIndicator[]; baseRate: IpoBaseRate | null; warnings: string[]; disclaimer: string; generatedAt: string };
type IpoRiskIndicator = { key: "offer_size" | "use_of_proceeds" | "secondary_sale" | "float_offered" | "lock_up" | "underwriters"
    | "dilution" | "voting" | "controlled" | "net_result" | "revenue";
  label: string; status: "extracted" | "not_extracted";
  value: string | null;            // texto para mostrar; null si no se extrajo
  numeric: number | null; unit: string | null;
  flags: { code: string; label: string; attention: boolean; evidence: string }[];   // hechos leídos, no veredictos
  evidence: string | null;         // cita literal del folleto
  note: string | null;             // limitación del indicador o motivo de «no extraído»
  source: { kind: "prospectus" | "calendar"; section: string | null; label: string; url: string | null } };
type IpoBaseRate = { months: number; benchmark: string; sample: number; nDay1: number; n3m: number;
  medianDay1Pct: number | null; positiveDay1Pct: number | null; median3mPct: number | null;
  median3mExcessPct: number | null; beatBenchmark3mPct: number | null;   // puntos porcentuales / % de OPV
  symbols: string[]; source: string; asOf: string; caveats: string[] };
```
Con `INVEST_AI_FAKE=1` todo es inventado (`source: "fake"`, empresas ficticias, URLs genéricas de EDGAR) y el perfil se calcula con el extractor real sobre texto de folleto inventado.
