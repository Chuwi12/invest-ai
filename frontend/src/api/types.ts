/**
 * Reflejo 1:1 de `backend/crates/domain/src/lib.rs` y de los tipos extra de `docs/API.md`.
 * Convenciones del contrato: structs en camelCase, enums en snake_case, `Option` => `null`,
 * fechas en ISO 8601 (string) y `NaiveDate` como `YYYY-MM-DD`.
 */

// --- Activos ---------------------------------------------------------------------------------

export type AssetClass =
  | 'stock'
  | 'etf'
  | 'fund'
  | 'index'
  | 'crypto'
  | 'forex'
  | 'commodity'
  | 'bond'
  | 'future'
  | 'option'
  | 'other'

export const ASSET_CLASSES: readonly AssetClass[] = [
  'stock',
  'etf',
  'fund',
  'index',
  'crypto',
  'forex',
  'commodity',
  'bond',
  'future',
  'option',
  'other',
]

/** `"<clase>:<símbolo>"`, p. ej. `"stock:AAPL"`, `"crypto:BTC"`, `"index:^GSPC"`. */
export type AssetId = `${AssetClass}:${string}`

export interface AssetSummary {
  id: AssetId
  symbol: string
  name: string
  class: AssetClass
  exchange: string | null
  currency: string | null
  country: string | null
  /** Proveedor que devolvió el resultado (yahoo, coingecko, ...). */
  source: string
}

// --- Precios ---------------------------------------------------------------------------------

export type MarketState = 'pre' | 'regular' | 'post' | 'closed' | 'continuous' | 'unknown'

export interface Quote {
  id: AssetId
  price: number
  change: number | null
  changePct: number | null
  open: number | null
  high: number | null
  low: number | null
  prevClose: number | null
  volume: number | null
  marketCap: number | null
  currency: string | null
  marketState: MarketState
  timestamp: string
  source: string
  /** Retraso del dato en segundos (0 = tiempo real). null = desconocido. */
  delaySecs: number | null
}

/** Actualización de precio en tiempo real (WebSocket). */
export interface PriceTick {
  id: AssetId
  price: number
  changePct: number | null
  volume: number | null
  ts: string
  source: string
}

/** Vela OHLCV con claves cortas. `t` = unix en segundos (UTC). */
export interface Candle {
  t: number
  o: number
  h: number
  l: number
  c: number
  v: number | null
}

export type Range = '1d' | '5d' | '1mo' | '3mo' | '6mo' | 'ytd' | '1y' | '2y' | '5y' | '10y' | 'max'
export type Interval = '1m' | '5m' | '15m' | '30m' | '1h' | '1d' | '1wk' | '1mo'

export const RANGES: readonly Range[] = ['1d', '5d', '1mo', '3mo', '6mo', 'ytd', '1y', '2y', '5y', '10y', 'max']

/** Intervalo por defecto del backend para cada rango (`Range::default_interval`). */
export function defaultInterval(range: Range): Interval {
  switch (range) {
    case '1d':
      return '5m'
    case '5d':
      return '15m'
    case '1mo':
      return '1h'
    case '3mo':
    case '6mo':
    case 'ytd':
    case '1y':
    case '2y':
      return '1d'
    case '5y':
    case '10y':
      return '1wk'
    case 'max':
      return '1mo'
  }
}

/** Duración de una vela en segundos (`Interval::seconds`). */
export function intervalSeconds(interval: Interval): number {
  switch (interval) {
    case '1m':
      return 60
    case '5m':
      return 300
    case '15m':
      return 900
    case '30m':
      return 1800
    case '1h':
      return 3600
    case '1d':
      return 86_400
    case '1wk':
      return 604_800
    case '1mo':
      return 2_592_000
  }
}

// --- Fundamentales e indicadores -------------------------------------------------------------

export interface Holding {
  symbol: string | null
  name: string
  weight: number
}

/** Ratios y márgenes como fracción (0.25 = 25 %). Todo opcional (null). */
export interface Fundamentals {
  description: string | null
  sector: string | null
  industry: string | null
  website: string | null
  employees: number | null
  country: string | null

  marketCap: number | null
  enterpriseValue: number | null
  peTrailing: number | null
  peForward: number | null
  peg: number | null
  priceToBook: number | null
  priceToSales: number | null
  evToEbitda: number | null

  epsTrailing: number | null
  epsForward: number | null
  revenueTtm: number | null
  revenueGrowthYoy: number | null
  earningsGrowthYoy: number | null
  grossMargin: number | null
  operatingMargin: number | null
  profitMargin: number | null
  roe: number | null
  roa: number | null
  freeCashFlow: number | null
  totalCash: number | null
  totalDebt: number | null
  debtToEquity: number | null
  currentRatio: number | null

  dividendYield: number | null
  payoutRatio: number | null
  beta: number | null
  sharesOutstanding: number | null
  shortPctFloat: number | null
  insiderPct: number | null
  institutionalPct: number | null

  analystTargetMean: number | null
  analystTargetHigh: number | null
  analystTargetLow: number | null
  /** 1 = compra fuerte … 5 = venta (escala de Yahoo). */
  analystRatingMean: number | null
  analystCount: number | null
  /** `YYYY-MM-DD` */
  nextEarningsDate: string | null

  circulatingSupply: number | null
  totalSupply: number | null
  maxSupply: number | null
  fullyDilutedValuation: number | null
  allTimeHigh: number | null
  allTimeHighDate: string | null

  expenseRatio: number | null
  totalAssets: number | null
  topHoldings: Holding[]

  yieldToMaturity: number | null
  coupon: number | null
  maturityDate: string | null

  /** Campos adicionales que no encajan arriba (se muestran tal cual). */
  extra: Record<string, unknown>
  sources: string[]
  asOf: string | null
}

export type Trend = 'strong_up' | 'up' | 'sideways' | 'down' | 'strong_down'

/** Indicadores técnicos sobre velas diarias. Rendimientos y distancias como fracción. */
export interface Indicators {
  asOf: string | null
  lastClose: number | null
  sma20: number | null
  sma50: number | null
  sma200: number | null
  ema12: number | null
  ema26: number | null
  rsi14: number | null
  macd: number | null
  macdSignal: number | null
  macdHist: number | null
  bbUpper: number | null
  bbMiddle: number | null
  bbLower: number | null
  atr14: number | null
  volatility30d: number | null
  maxDrawdown1y: number | null
  return1w: number | null
  return1m: number | null
  return3m: number | null
  returnYtd: number | null
  return1y: number | null
  high52w: number | null
  low52w: number | null
  distFromHigh52w: number | null
  volumeAvg20: number | null
  relativeVolume: number | null
  trend: Trend | null
  supports: number[]
  resistances: number[]
}

// --- Noticias y macro ------------------------------------------------------------------------

export type SentimentLabel = 'very_negative' | 'negative' | 'neutral' | 'positive' | 'very_positive'

export interface Sentiment {
  /** -1 (muy negativo) … 1 (muy positivo). */
  score: number
  label: SentimentLabel
  /** "llm", "lexicon", "provider:<nombre>". */
  method: string
}

export interface NewsItem {
  id: string
  title: string
  summary: string | null
  url: string
  publisher: string | null
  publishedAt: string
  imageUrl: string | null
  tickers: string[]
  sentiment: Sentiment | null
  provider: string
  language: string | null
}

export interface MacroIndicator {
  key: string
  name: string
  value: number
  change: number | null
  unit: string
  asOf: string
  source: string
}

export interface MacroContext {
  indicators: MacroIndicator[]
  asOf: string | null
}

/** Todo lo que se sabe de un activo en un instante: el contexto que recibe el LLM. */
export interface AssetSnapshot {
  summary: AssetSummary
  quote: Quote | null
  fundamentals: Fundamentals | null
  indicators: Indicators | null
  news: NewsItem[]
  macroContext: MacroContext | null
  /** Datos que no se pudieron obtener. */
  missing: string[]
  generatedAt: string
}

// --- Análisis de IA --------------------------------------------------------------------------

export type Verdict = 'buy' | 'accumulate' | 'hold' | 'wait' | 'reduce' | 'avoid'
export type Horizon = 'short' | 'medium' | 'long'

export const VERDICTS: readonly Verdict[] = ['buy', 'accumulate', 'hold', 'wait', 'reduce', 'avoid']

/** Puntuaciones 0–100 (más alto = más favorable; en `risk`, más alto = más riesgo). */
export interface ScoreBreakdown {
  overall: number
  technical: number | null
  momentum: number | null
  fundamental: number | null
  valuation: number | null
  sentiment: number | null
  macroScore: number | null
  risk: number | null
}

export interface Catalyst {
  /** `YYYY-MM-DD` */
  date: string | null
  description: string
  /** Impacto esperado: -1 … 1. */
  impact: number | null
}

export interface KeyLevels {
  supports: number[]
  resistances: number[]
  /** Tupla `[desde, hasta]` (serde serializa `(f64, f64)` como array de 2). */
  entryZone: [number, number] | null
  stopLoss: number | null
  targets: number[]
}

export interface SourceRef {
  title: string
  url: string | null
  provider: string
  publishedAt: string | null
}

export interface Analysis {
  id: string
  asset: AssetId
  verdict: Verdict
  /** 0–1. */
  confidence: number
  horizon: Horizon
  scores: ScoreBreakdown
  summary: string
  thesis: string
  bullCase: string[]
  bearCase: string[]
  risks: string[]
  catalysts: Catalyst[]
  keyLevels: KeyLevels
  strategy: string | null
  dataWarnings: string[]
  sources: SourceRef[]
  priceAtAnalysis: number | null
  model: string
  generatedAt: string
}

// --- Chat ------------------------------------------------------------------------------------

export type Role = 'system' | 'user' | 'assistant' | 'tool'

export interface ToolCallRecord {
  id: string
  name: string
  arguments: unknown
  /** Resumen corto del resultado para la UI. */
  resultSummary: string | null
  ok: boolean | null
}

export interface ChatMessage {
  id: string
  conversationId: string
  role: Role
  content: string
  toolCalls: ToolCallRecord[]
  /** Veredicto estructurado emitido en este mensaje. */
  analysis: Analysis | null
  createdAt: string
}

export interface Conversation {
  id: string
  title: string
  assetIds: AssetId[]
  pinned: boolean
  messageCount: number
  lastMessagePreview: string | null
  createdAt: string
  updatedAt: string
}

/** Evento del stream SSE del chat, discriminado por `type`. */
export type ChatEvent =
  | { type: 'start'; messageId: string }
  | { type: 'thinking'; text: string }
  | { type: 'token'; text: string }
  | { type: 'tool_call'; id: string; name: string; arguments: unknown }
  | { type: 'tool_result'; id: string; name: string; ok: boolean; summary: string }
  | { type: 'analysis'; analysis: Analysis }
  | {
      type: 'done'
      messageId: string
      model: string
      promptTokens: number | null
      completionTokens: number | null
    }
  | { type: 'error'; message: string }

// --- Estado de servicios ---------------------------------------------------------------------

export interface ProviderStatus {
  name: string
  enabled: boolean
  requiresKey: boolean
  hasKey: boolean
  /** search, quote, realtime, history, fundamentals, news, macro, options. */
  capabilities: string[]
  assetClasses: AssetClass[]
  lastError: string | null
  lastOk: string | null
}

export interface LlmStatus {
  available: boolean
  baseUrl: string
  model: string
  installedModels: string[]
  supportsTools: boolean
  error: string | null
}

// --- Tipos extra de la API (docs/API.md) -----------------------------------------------------

export interface Health {
  status: 'ok'
  version: string
}

export interface ServiceStatus {
  llm: LlmStatus
  providers: ProviderStatus[]
}

export interface AssetDetail {
  summary: AssetSummary
  quote: Quote | null
  fundamentals: Fundamentals | null
  isFavorite: boolean
}

export interface Favorite {
  id: AssetId
  summary: AssetSummary
  note: string | null
  addedAt: string
}

export interface SearchEntry {
  query: string
  selected: AssetId | null
  at: string
}

export interface ViewStat {
  id: AssetId
  summary: AssetSummary
  views: number
  lastViewedAt: string
}

export interface Dashboard {
  favorites: { favorite: Favorite; quote: Quote | null }[]
  /** Top 8, ponderado por recencia. */
  mostViewed: { stat: ViewStat; quote: Quote | null }[]
  /** 10 */
  recentSearches: SearchEntry[]
  /** 5 */
  recentConversations: Conversation[]
  /** 5 */
  latestAnalyses: Analysis[]
  /** Panel fijo: ^GSPC, ^IXIC, ^STOXX50E, ^IBEX, BTC, ETH, GC=F, CL=F, EURUSD=X, ^TNX, ^VIX. */
  market: Quote[]
  /** 10 titulares de mercado. */
  news: NewsItem[]
}

export interface ConversationWithMessages {
  conversation: Conversation
  messages: ChatMessage[]
}

export interface CreateConversationBody {
  title?: string
  assetIds?: AssetId[]
}

export interface UpdateConversationBody {
  title?: string
  assetIds?: AssetId[]
  pinned?: boolean
}

export interface SendMessageBody {
  content: string
  assetIds?: AssetId[]
}

/** Cuerpo de error del contrato: `{ "error": { "code", "message" } }`. */
export type ApiErrorCode =
  | 'not_found'
  | 'invalid_input'
  | 'rate_limited'
  | 'upstream'
  | 'unavailable'
  | 'internal'

export interface ApiErrorBody {
  error: { code: string; message: string }
}

// --- Mensajes del WebSocket ------------------------------------------------------------------

export type WsClientMessage =
  | { type: 'subscribe'; ids: AssetId[] }
  | { type: 'unsubscribe'; ids: AssetId[] }
  | { type: 'ping' }

export type WsServerMessage =
  | { type: 'tick'; tick: PriceTick }
  | { type: 'pong' }
  | { type: 'error'; message: string }

// --- Análisis: profundidad -------------------------------------------------------------------

/** `POST /assets/{id}/analysis?depth=...`: más profundidad = más herramientas y más tiempo. */
export type AnalysisDepth = 'quick' | 'standard' | 'deep'

export const ANALYSIS_DEPTHS: readonly AnalysisDepth[] = ['quick', 'standard', 'deep']

// --- Modelos de IA (Ajustes) -----------------------------------------------------------------

export type ModelTier = 'light' | 'balanced' | 'heavy' | 'max'

export const MODEL_TIERS: readonly ModelTier[] = ['light', 'balanced', 'heavy', 'max']

export interface ModelRequirements {
  minVramGb: number
  recommendedVramGb: number
  minRamGb: number
  diskGb: number
  cpuOnlyOk: boolean
}

export interface LlmModel {
  id: string
  name: string
  family: string
  parameters: string
  activeParameters: string | null
  quantization: string
  contextWindow: number
  tier: ModelTier | null
  supportsTools: boolean
  reasoning: boolean
  description: string
  requirements: ModelRequirements | null
  curated: boolean
  installed: boolean
  active: boolean
}

export interface GpuInfo {
  name: string
  vendor: string
  vramTotalGb: number
  vramFreeGb: number | null
}

export interface HardwareInfo {
  cpuModel: string
  cpuThreads: number
  ramTotalGb: number
  ramAvailableGb: number
  gpus: GpuInfo[]
  diskFreeGb: number | null
}

export interface ModelsResponse {
  active: string
  ollamaReachable: boolean
  models: LlmModel[]
  hardware: HardwareInfo
}

/** Evento del SSE de `POST /models/{id}/pull`. */
export type PullEvent =
  | { type: 'progress'; status: string; completed: number | null; total: number | null }
  | { type: 'done' }
  | { type: 'error'; message: string }
