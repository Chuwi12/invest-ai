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
  /** Página pública del proveedor donde comprobar el dato (puede faltar en backends antiguos). */
  sourceUrl?: string | null
}

/** De dónde sale una marca de tiempo: la dio el proveedor o es la hora local de la consulta. */
export type TimestampOrigin = 'provider' | 'local'

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
  sourceUrl?: string | null
  timestampOrigin?: TimestampOrigin | null
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
  /** Moneda de las cifras monetarias de los fundamentales (capitalización, objetivos, máximo histórico…). Puede diferir de la de cotización. Ausente/null en backends antiguos. */
  currency?: string | null

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
  sourceUrl?: string | null
  timestampOrigin?: TimestampOrigin | null
}

export type Trend = 'strong_up' | 'up' | 'sideways' | 'down' | 'strong_down'

/** Indicadores técnicos sobre velas diarias. Rendimientos y distancias como fracción. */
export interface Indicators {
  asOf: string | null
  /** Proveedor de las velas sobre las que la app calculó los indicadores. */
  source?: string | null
  sourceUrl?: string | null
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
  sourceUrl?: string | null
  timestampOrigin?: TimestampOrigin | null
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
  /** Por qué faltan (o cómo se calcularon) entrada, stop y objetivos. */
  levelsNote?: string | null
  /** Solo `deep`; las probabilidades suman 100. */
  scenarios?: Scenario[] | null
  /** Σ probabilidad × rentabilidad de los escenarios, en % respecto al precio del análisis. */
  expectedReturnPct?: number | null
  /** Qué tendría que pasar para cambiar el veredicto. */
  wouldChangeIf?: string[] | null
  /** Por qué ese horizonte. */
  horizonNote?: string | null
  positionSizing?: PositionSizing | null
  /** Nota de método (modelo, profundidad, llamadas, tokens, tiempo, aviso). */
  method?: string | null
  /** Avisos de calidad del propio análisis (descartes, cifras sin verificar…); no son carencias de datos. */
  qualityNotes?: string[] | null
  depth?: AnalysisDepth | null
  durationMs?: number | null
  llmCalls?: number | null
  promptTokens?: number | null
  completionTokens?: number | null
}

export type ScenarioKind = 'bullish' | 'base' | 'bearish'

export interface Scenario {
  kind: ScenarioKind
  /** 0–100. */
  probability: number
  description: string
  targetPrice: number | null
  /** Qué nivel de los datos se usó como objetivo. */
  targetBasis: string | null
  /** % desde el precio del análisis hasta `targetPrice`. */
  returnPct: number | null
}

export interface PositionSizing {
  /** % máximo orientativo de la cartera. */
  maxPortfolioPct: number
  riskPerTradePct: number | null
  stopDistancePct: number | null
  atrPct: number | null
  volatility30d: number | null
  rationale: string
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
  /** true si un error cortó la respuesta: el texto es parcial. Ausente en backends antiguos. */
  truncated?: boolean
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
  /** Latencia de la última petición (ms). */
  latencyMs?: number | null
  /** Segundos que quedan de pausa tras un 429. */
  blockedSecs?: number | null
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

// --- Procedencia del histórico y progreso del análisis ----------------------------------------

/** Velas + procedencia (cabeceras `X-Data-Source`, `X-Data-Source-Url`, `X-Data-As-Of`, `X-Data-Interval`). */
export interface HistoryResult {
  candles: Candle[]
  /** null si el backend no informa del proveedor. */
  source: string | null
  sourceUrl: string | null
  asOf: string | null
  /** Intervalo realmente servido (`X-Data-Interval`); distinto del pedido si el proveedor subió de intervalo. */
  interval?: Interval | null
}

/** Eventos de `POST /assets/{id}/analysis/stream` (SSE). */
export type AnalysisStreamEvent =
  | { type: 'stage'; stage: string; label: string; index: number; total: number }
  | { type: 'done'; analysis: Analysis }
  | { type: 'error'; code: string; message: string }

export interface AnalysisProgress {
  stage: string
  label: string
  index: number
  total: number
}

// --- Rendimiento medido y lista de seguimiento (docs/API.md, «Rendimiento medido…») ------------

export type LearningHorizon = 1 | 4 | 12

export interface VerdictStats {
  /** `all` en `overall`; en `byVerdict`, un `Verdict`. */
  verdict: string
  n: number
  hits: number
  hitRate: number | null
  /** Wilson 95 % sobre la muestra independiente. */
  hitRateCi: [number, number] | null
  meanExcess: number | null
  /** Bootstrap por bloques temporales 95 %; null con menos de 5 bloques. */
  meanExcessCi: [number, number] | null
  clusters: number
}

export interface ReliabilityBin {
  lo: number
  hi: number
  n: number
  meanConfidence: number
  hitRate: number
  ciLow: number
  ciHigh: number
}

export interface CalibrationStatus {
  horizonWeeks: number
  n: number
  minRequired: number
  applied: boolean
  reason: string
  brierRaw: number | null
  brierHoldoutRaw: number | null
  brierHoldoutCalibrated: number | null
  reliability: ReliabilityBin[]
}

export interface HorizonReport {
  horizonWeeks: LearningHorizon
  analysesTotal: number
  resolved: number
  resolvedIndependent: number
  pending: number
  notMatured: number
  skipped: number
  baseRateOutperform: number | null
  overall: VerdictStats
  byVerdict: VerdictStats[]
  calibration: CalibrationStatus
}

export interface WeightVersion {
  version: number
  kind: 'learned' | 'rollback' | 'decay' | 'manual'
  status: 'active' | 'retired'
  multipliers: number[]
  factorNames: string[]
  createdAtMs: number
  horizonWeeks: number | null
  trainedToMs: number | null
  validatedFromMs: number | null
  validatedToMs: number | null
  nTrain: number | null
  nVal: number | null
  /** Informe de la puerta de validación (`deltaIc`, `pValue`, `icCandidate`…); puede venir vacío. */
  metrics: Record<string, unknown>
  parentVersion: number | null
  note: string | null
}

export interface PerformanceReport {
  generatedAt: string
  ruleVersion: number
  analysesTotal: number
  horizons: HorizonReport[]
  activeWeights: WeightVersion | null
  weightHistory: WeightVersion[]
  caveats: string[]
}

export interface WeightsResponse {
  active: WeightVersion | null
  versions: WeightVersion[]
}

export interface CalibratedConfidence {
  raw: number
  /** null = sin calibración aplicable. */
  calibrated: number | null
  horizonWeeks: number
  n: number
  minRequired: number
  reason: string
}

export interface WatchlistRun {
  id: number
  localDate: string
  scheduledFor: string
  startedAt: string
  finishedAt: string | null
  trigger: 'schedule' | 'recovery' | 'retry'
  status: 'running' | 'completed' | 'aborted' | 'interrupted'
  depth: string
  model: string | null
  planned: number
  power: 'ac' | 'battery' | 'unknown'
  powerWaitSecs: number
  ok: number
  failed: number
}

export interface WatchlistResult {
  id: number
  runId: number
  localDate: string
  assetId: AssetId
  status: 'ok' | 'failed' | 'skipped'
  transient: boolean
  attempts: number
  analysisId: string | null
  verdict: string | null
  errorCode: string | null
  error: string | null
  startedAt: string
  finishedAt: string
}

export interface WatchlistAsset {
  id: AssetId
  last: WatchlistResult | null
  lastOkAt: string | null
}

export interface WatchlistStatus {
  enabled: boolean
  defaultList: boolean
  /** Hora local de la pasada diaria, `HH:MM`. */
  time: string
  depth: AnalysisDepth
  nextRun: string | null
  running: boolean
  assets: WatchlistAsset[]
  lastRun: WatchlistRun | null
  lastRunResults: WatchlistResult[]
  recentRuns: WatchlistRun[]
  analysesTotal: number
}

// --- OPV de EE. UU.: calendario, folleto, perfil de riesgo y tasa base (docs/API.md, «OPV») -----

export type IpoStatus = 'expected' | 'priced' | 'filed' | 'withdrawn' | 'unknown'

/** Una OPV del calendario. Lo que el proveedor no da va a `null`. */
export interface IpoEntry {
  /** `priced`: primer día de cotización; `expected`: fecha prevista; `filed`: presentación. */
  date: string
  symbol: string | null
  name: string
  exchange: string | null
  status: IpoStatus
  priceLow: number | null
  priceHigh: number | null
  shares: number | null
  /** USD. En `expected` es el máximo posible (límite alto × acciones × 1,15). */
  totalValue: number | null
  /** Indicio (no certeza) de sociedad vehículo (SPAC). */
  spacLikely: boolean
  source: string
  sourceUrl: string | null
  asOf: string
}

export interface ProspectusSection {
  key: string
  title: string
  text: string
  parent: string | null
  truncated: boolean
  originalChars: number
}

export interface Prospectus {
  issuer: string
  cik: number
  ticker: string | null
  form: string
  preliminary: boolean
  filedOn: string
  accession: string
  /** Documento original en sec.gov. */
  url: string
  indexUrl: string
  offerPrice: number | null
  sections: ProspectusSection[]
  /** Claves de secciones que no se localizaron. */
  missing: string[]
  documentChars: number
  source: string
  fetchedAt: string
}

export interface IpoReturnPoint {
  horizon: '1m' | '3m' | '6m' | '12m'
  targetDate: string
  priceDate: string | null
  price: number | null
  /** Puntos porcentuales desde el precio de la OPV; `null` si aún no se alcanza. */
  returnPct: number | null
  sinceFirstClosePct: number | null
  benchmarkReturnPct: number | null
  excessPct: number | null
}

export interface IpoPerformance {
  symbol: string
  ipoDate: string
  offerPrice: number
  offerPriceSource: string
  firstDayClose: number
  firstDayReturnPct: number
  firstDayHighPct: number
  points: IpoReturnPoint[]
  latestPrice: number
  latestDate: string
  latestReturnPct: number
  maxDrawdownPct: number
  benchmark: string
  source: string
  sourceUrl: string | null
  warnings: string[]
  asOf: string
}

export type IpoIndicatorStatus = 'extracted' | 'not_extracted'

export interface IpoIndicatorSource {
  /** `prospectus` (sección de un folleto en EDGAR) o `calendar`. */
  kind: 'prospectus' | 'calendar'
  section: string | null
  label: string
  url: string | null
}

/** Un hecho leído del texto (no un veredicto); `attention` marca lo que conviene mirar con cuidado. */
export interface IpoRiskFlag {
  code: string
  label: string
  attention: boolean
  /** Frase literal del folleto. */
  evidence: string
}

export interface IpoRiskIndicator {
  key: string
  label: string
  status: IpoIndicatorStatus
  value: string | null
  numeric: number | null
  unit: string | null
  flags: IpoRiskFlag[]
  evidence: string | null
  /** Limitación del indicador o motivo de «no extraído». */
  note: string | null
  source: IpoIndicatorSource
}

export interface IpoBaseRate {
  months: number
  benchmark: string
  sample: number
  nDay1: number
  n3m: number
  /** Puntos porcentuales. */
  medianDay1Pct: number | null
  positiveDay1Pct: number | null
  median3mPct: number | null
  median3mExcessPct: number | null
  beatBenchmark3mPct: number | null
  symbols: string[]
  source: string
  asOf: string
  caveats: string[]
}

export interface IpoProspectusRef {
  form: string
  preliminary: boolean
  filedOn: string
  url: string
  indexUrl: string
  documentChars: number
  missing: string[]
}

/** Perfil de riesgo determinista de una OPV: no es una recomendación ni una predicción. */
export interface IpoRiskProfile {
  symbol: string
  name: string | null
  entry: IpoEntry | null
  prospectus: IpoProspectusRef | null
  indicators: IpoRiskIndicator[]
  baseRate: IpoBaseRate | null
  warnings: string[]
  disclaimer: string
  generatedAt: string
}
