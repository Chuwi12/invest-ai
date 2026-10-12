# Contrato de la API (backend ⇄ frontend)

Base: `http://127.0.0.1:8787/api` (en desarrollo, Vite hace de proxy de `/api` → `127.0.0.1:8787`).
Los tipos JSON son exactamente los de `backend/crates/domain/src/lib.rs` (structs en camelCase,
enums en snake_case). `frontend/src/api/types.ts` los refleja 1:1.

`{id}` es un `AssetId` (`"stock:AAPL"`, `"crypto:BTC"`, `"index:^GSPC"`); en la URL va
percent-encoded (`encodeURIComponent`).

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

## Sistema

| Método | Ruta              | Respuesta |
|--------|-------------------|-----------|
| GET    | `/health`         | `{ "status": "ok", "version": string }` |
| GET    | `/status`         | `{ "llm": LlmStatus, "providers": ProviderStatus[] }` |

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

- `timestampOrigin = "provider"`: `timestamp`/`asOf` es la hora que dio el proveedor. `"local"`: el
  proveedor no la da y es la hora de la consulta (Yahoo/Finnhub en `Fundamentals.asOf`; Binance/CoinGecko
  si faltara el campo).
- `NewsItem.url` ya es el enlace al artículo/filing original (no necesita `sourceUrl`).
- `GET /assets/{id}/history` sigue devolviendo `Candle[]`; la procedencia va en cabeceras de respuesta:
  `X-Data-Source` (proveedor que sirvió las velas), `X-Data-Source-Url` y `X-Data-As-Of` (inicio de la
  última vela, ISO 8601). Faltan si el backend no la conoce (p. ej. el servidor simulado). La API se
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

## Análisis de IA

| Método | Ruta | Respuesta |
|--------|------|-----------|
| GET  | `/assets/{id}/analysis` | `Analysis \| null`: el último guardado (se cachea en BD) |
| POST | `/assets/{id}/analysis` | `Analysis`: genera uno nuevo con snapshot en vivo (puede tardar 10–60 s) |
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
