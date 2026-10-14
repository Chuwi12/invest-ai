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
cerrado/pre/post y no hay cambios).

## Análisis de IA

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET  | `/assets/{id}/analysis` | `Analysis \| null`: el último guardado (se cachea en BD) |
| POST | `/assets/{id}/analysis` | `Analysis`: genera uno nuevo con snapshot en vivo (puede tardar 10–60 s) |
| POST | `/assets/{id}/analysis/stream` | SSE (`data: <json>\n\n`) de `AnalysisEvent` mientras se genera (ver «Análisis en streaming»). Mismo trabajo único por activo que el POST; guarda en BD; sigue si el cliente se va |
| GET  | `/assets/{id}/analyses` | `Analysis[]`: histórico de análisis del activo (máx. 20, más reciente primero) |
| GET  | `/assets/{id}/snapshot` | `AssetSnapshot`: el contexto exacto que ve el LLM (transparencia) |

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
