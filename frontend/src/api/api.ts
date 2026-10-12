import type {
  Analysis,
  AnalysisDepth,
  AssetClass,
  AssetDetail,
  AssetId,
  AssetSnapshot,
  AssetSummary,
  Candle,
  ChatEvent,
  Conversation,
  ConversationWithMessages,
  CreateConversationBody,
  Dashboard,
  Favorite,
  Fundamentals,
  Health,
  Indicators,
  Interval,
  MacroContext,
  ModelsResponse,
  PullEvent,
  NewsItem,
  Quote,
  Range,
  SearchEntry,
  SendMessageBody,
  ServiceStatus,
  UpdateConversationBody,
  ViewStat,
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
  history(id: AssetId, range: Range, interval?: Interval, signal?: AbortSignal): Promise<Candle[]>
  indicators(id: AssetId, signal?: AbortSignal): Promise<Indicators>
  fundamentals(id: AssetId, signal?: AbortSignal): Promise<Fundamentals | null>
  news(id: AssetId, limit?: number, signal?: AbortSignal): Promise<NewsItem[]>
  marketNews(limit?: number, signal?: AbortSignal): Promise<NewsItem[]>
  macro(signal?: AbortSignal): Promise<MacroContext>

  latestAnalysis(id: AssetId, signal?: AbortSignal): Promise<Analysis | null>
  /** Puede tardar 10–60 s (más con `deep`). */
  generateAnalysis(id: AssetId, depth: AnalysisDepth, signal?: AbortSignal): Promise<Analysis>
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
}
