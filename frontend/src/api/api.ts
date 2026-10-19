import type {
  Analysis,
  AnalysisDepth,
  AnalysisStreamEvent,
  AssetClass,
  AssetDetail,
  AssetId,
  AssetSnapshot,
  AssetSummary,
  CalibratedConfidence,
  ChatEvent,
  Conversation,
  ConversationWithMessages,
  CreateConversationBody,
  Dashboard,
  Favorite,
  Fundamentals,
  Health,
  HistoryResult,
  Indicators,
  Interval,
  IpoBaseRate,
  IpoEntry,
  IpoPerformance,
  IpoRiskProfile,
  Prospectus,
  MacroContext,
  ModelsResponse,
  PerformanceReport,
  PullEvent,
  NewsItem,
  Quote,
  Range,
  SearchEntry,
  SendMessageBody,
  ServiceStatus,
  AssetSocial,
  SocialStatus,
  UpdateConversationBody,
  ViewStat,
  WatchlistStatus,
  WeightsResponse,
  WeightVersion,
} from './types'

/**
 * Interfaz única de datos. La implementan el cliente HTTP real (`http.ts`) y el adaptador
 * simulado (`src/mock`), de modo que la UI no sabe cuál usa.
 */
export interface Api {
  health(signal?: AbortSignal): Promise<Health>
  status(signal?: AbortSignal): Promise<ServiceStatus>

  search(q: string, opts?: { limit?: number; class?: AssetClass }, signal?: AbortSignal): Promise<AssetSummary[]>
  /** Registra una visita al activo en el backend. */
  asset(id: AssetId, signal?: AbortSignal): Promise<AssetDetail>
  quote(id: AssetId, signal?: AbortSignal): Promise<Quote>
  quotes(ids: AssetId[], signal?: AbortSignal): Promise<Quote[]>
  history(id: AssetId, range: Range, interval?: Interval, signal?: AbortSignal): Promise<HistoryResult>
  indicators(id: AssetId, signal?: AbortSignal): Promise<Indicators>
  fundamentals(id: AssetId, signal?: AbortSignal): Promise<Fundamentals | null>
  news(id: AssetId, limit?: number, signal?: AbortSignal): Promise<NewsItem[]>
  marketNews(limit?: number, signal?: AbortSignal): Promise<NewsItem[]>
  macro(signal?: AbortSignal): Promise<MacroContext>

  latestAnalysis(id: AssetId, signal?: AbortSignal): Promise<Analysis | null>
  /** Puede tardar 10–60 s (más con `deep`). */
  generateAnalysis(id: AssetId, depth: AnalysisDepth, signal?: AbortSignal): Promise<Analysis>
  /**
   * SSE `POST /assets/{id}/analysis/stream`: itera `stage`* y termina con `done` o `error`.
   * Si el backend responde 404 cae al POST normal (solo emite `done`). Abortable.
   */
  streamAnalysis(id: AssetId, depth: AnalysisDepth, signal?: AbortSignal): AsyncGenerator<AnalysisStreamEvent>
  analyses(id: AssetId, signal?: AbortSignal): Promise<Analysis[]>
  snapshot(id: AssetId, signal?: AbortSignal): Promise<AssetSnapshot>

  listConversations(signal?: AbortSignal): Promise<Conversation[]>
  createConversation(body: CreateConversationBody): Promise<Conversation>
  getConversation(cid: string, signal?: AbortSignal): Promise<ConversationWithMessages>
  updateConversation(cid: string, body: UpdateConversationBody): Promise<Conversation>
  deleteConversation(cid: string): Promise<void>
  /** SSE: itera los `ChatEvent` hasta `done`/`error`. Abortable. */
  sendMessage(cid: string, body: SendMessageBody, signal?: AbortSignal): AsyncGenerator<ChatEvent>

  listFavorites(signal?: AbortSignal): Promise<Favorite[]>
  putFavorite(id: AssetId, note?: string | null): Promise<Favorite>
  deleteFavorite(id: AssetId): Promise<void>

  searchHistory(signal?: AbortSignal): Promise<SearchEntry[]>
  clearSearchHistory(): Promise<void>
  recordSearch(query: string, selected: AssetId): Promise<void>
  viewStats(signal?: AbortSignal): Promise<ViewStat[]>
  /** Propuesto, NO está en docs/API.md: `DELETE /history/views`. */
  clearViews(): Promise<void>

  dashboard(signal?: AbortSignal): Promise<Dashboard>

  // Modelos de IA: el usuario elige; la app solo informa.
  models(signal?: AbortSignal): Promise<ModelsResponse>
  /** 400 `invalid_input` si el modelo no está instalado. */
  setActiveModel(id: string): Promise<ModelsResponse>
  /** SSE: itera los `PullEvent` hasta `done`/`error`. Abortable. */
  pullModel(id: string, signal?: AbortSignal): AsyncGenerator<PullEvent>
  deleteModel(id: string): Promise<void>

  // Rendimiento medido (crate `learning`) y lista de seguimiento.
  performance(signal?: AbortSignal): Promise<PerformanceReport>
  learningWeights(signal?: AbortSignal): Promise<WeightsResponse>
  /** `version: null` = volver a los pesos por defecto. Crea una versión nueva (no borra historia). */
  rollbackWeights(version: number | null): Promise<WeightVersion>
  /** Confianza declarada junto a la calibrada (si hay calibración aplicable). */
  calibratedConfidence(confidence: number, horizon?: number, model?: string, signal?: AbortSignal): Promise<CalibratedConfidence>
  watchlist(signal?: AbortSignal): Promise<WatchlistStatus>

  // Redes sociales (Reddit/X): OPINIÓN, no hechos. Apagado por defecto (`enabled: false`).
  socialStatus(signal?: AbortSignal): Promise<SocialStatus>
  /** Consulta las redes solo si lo guardado está desfasado; un fallo de redes devuelve `digest: null`. */
  assetSocial(id: AssetId, signal?: AbortSignal): Promise<AssetSocial>

  // OPV de EE. UU.: perfil de riesgo determinista (no una recomendación).
  /** Sin fechas: de hoy−30 a hoy+60 días. Rango máx. 180 días. */
  ipoCalendar(range?: { from?: string; to?: string }, signal?: AbortSignal): Promise<IpoEntry[]>
  ipoRiskProfile(symbol: string, date?: string, signal?: AbortSignal): Promise<IpoRiskProfile>
  ipoProspectus(symbol: string, maxChars?: number, signal?: AbortSignal): Promise<Prospectus>
  /** Sin `date`, la del calendario. 404 si la OPV aún no cotiza. */
  ipoPerformance(symbol: string, date?: string, signal?: AbortSignal): Promise<IpoPerformance>
  /** Tasa base: medianas de las OPV recientes con su tamaño de muestra. */
  ipoBaseRate(months?: number, signal?: AbortSignal): Promise<IpoBaseRate>
}
