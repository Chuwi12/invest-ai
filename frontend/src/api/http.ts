import type { Api } from './api'
import { ApiError, assetPath, request, requestWithHeaders } from './client'
import { streamAnalysisEvents, streamChatEvents, streamPullEvents } from './sse'
import type { Analysis, AnalysisDepth, AnalysisStreamEvent, Candle, HistoryResult, Interval } from './types'

const INTERVALS: readonly string[] = ['1m', '5m', '15m', '30m', '1h', '1d', '1wk', '1mo']

/**
 * Progreso real del análisis por SSE. Si el backend aún no tiene el endpoint (404/405) se cae al
 * `POST /analysis` normal y se emite solo el `done`, sin etapas.
 */
async function* streamAnalysisWithFallback(
  id: string,
  depth: AnalysisDepth,
  signal?: AbortSignal,
): AsyncGenerator<AnalysisStreamEvent> {
  try {
    yield* streamAnalysisEvents(`${assetPath(id)}/analysis/stream?depth=${encodeURIComponent(depth)}`, signal)
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 405)) {
      const analysis = await request<Analysis>(`${assetPath(id)}/analysis`, { method: 'POST', query: { depth }, signal })
      yield { type: 'done', analysis }
      return
    }
    throw e
  }
}

/** Cliente real contra el backend (`/api/...`, ver docs/API.md). */
export const httpApi: Api = {
  health: (signal) => request('/health', { signal }),
  status: (signal) => request('/status', { signal }),

  search: (q, opts, signal) =>
    request('/search', { query: { q, limit: opts?.limit, class: opts?.class }, signal }),
  asset: (id, signal) => request(assetPath(id), { signal }),
  quote: (id, signal) => request(`${assetPath(id)}/quote`, { signal }),
  quotes: (ids, signal) =>
    ids.length === 0 ? Promise.resolve([]) : request('/quotes', { query: { ids: ids.join(',') }, signal }),
  history: async (id, range, interval, signal): Promise<HistoryResult> => {
    const { data, headers } = await requestWithHeaders<Candle[]>(`${assetPath(id)}/history`, { query: { range, interval }, signal })
    return {
      candles: data,
      source: headers.get('x-data-source'),
      sourceUrl: headers.get('x-data-source-url'),
      asOf: headers.get('x-data-as-of'),
      interval: ((v) => (v !== null && INTERVALS.includes(v) ? (v as Interval) : null))(headers.get('x-data-interval')),
    }
  },
  indicators: (id, signal) => request(`${assetPath(id)}/indicators`, { signal }),
  fundamentals: (id, signal) => request(`${assetPath(id)}/fundamentals`, { signal }),
  news: (id, limit, signal) => request(`${assetPath(id)}/news`, { query: { limit }, signal }),
  marketNews: (limit, signal) => request('/news', { query: { limit }, signal }),
  macro: (signal) => request('/macro', { signal }),

  latestAnalysis: (id, signal) => request(`${assetPath(id)}/analysis`, { signal }),
  generateAnalysis: (id, depth, signal) =>
    request(`${assetPath(id)}/analysis`, { method: 'POST', query: { depth }, signal }),
  streamAnalysis: (id, depth, signal) => streamAnalysisWithFallback(id, depth, signal),
  analyses: (id, signal) => request(`${assetPath(id)}/analyses`, { signal }),
  snapshot: (id, signal) => request(`${assetPath(id)}/snapshot`, { signal }),

  listConversations: (signal) => request('/conversations', { signal }),
  createConversation: (body) => request('/conversations', { method: 'POST', body }),
  getConversation: (cid, signal) => request(`/conversations/${encodeURIComponent(cid)}`, { signal }),
  updateConversation: (cid, body) =>
    request(`/conversations/${encodeURIComponent(cid)}`, { method: 'PATCH', body }),
  deleteConversation: (cid) => request(`/conversations/${encodeURIComponent(cid)}`, { method: 'DELETE' }),
  sendMessage: (cid, body, signal) =>
    streamChatEvents(`/conversations/${encodeURIComponent(cid)}/messages`, body, signal),

  listFavorites: (signal) => request('/favorites', { signal }),
  putFavorite: (id, note) =>
    request(`/favorites/${encodeURIComponent(id)}`, {
      method: 'PUT',
      ...(note !== undefined && note !== null ? { body: { note } } : {}),
    }),
  deleteFavorite: (id) => request(`/favorites/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  searchHistory: (signal) => request('/history/searches', { signal }),
  clearSearchHistory: () => request('/history/searches', { method: 'DELETE' }),
  recordSearch: (query, selected) => request('/history/searches', { method: 'POST', body: { query, selected } }),
  viewStats: (signal) => request('/history/views', { signal }),
  clearViews: () => request('/history/views', { method: 'DELETE' }),

  dashboard: (signal) => request('/dashboard', { signal }),

  models: (signal) => request('/models', { signal }),
  setActiveModel: (id) => request('/models/active', { method: 'PUT', body: { id } }),
  pullModel: (id, signal) => streamPullEvents(`/models/${encodeURIComponent(id)}/pull`, signal),
  deleteModel: (id) => request(`/models/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  setModelForceGpu: (id, force) => request(`/models/${encodeURIComponent(id)}/gpu`, { method: 'PUT', body: { force } }),

  performance: (signal) => request('/learning/performance', { signal }),
  learningWeights: (signal) => request('/learning/weights', { signal }),
  rollbackWeights: (version) => request('/learning/weights/rollback', { method: 'POST', body: { version } }),
  calibratedConfidence: (confidence, horizon = 4, model, signal) =>
    request(
      `/learning/calibrated-confidence?confidence=${encodeURIComponent(String(confidence))}&horizon=${horizon}${model ? `&model=${encodeURIComponent(model)}` : ''}`,
      { signal },
    ),
  watchlist: (signal) => request('/watchlist', { signal }),
  socialStatus: (signal) => request('/social/status', { signal }),
  assetSocial: (id, signal) => request(`/assets/${encodeURIComponent(id)}/social`, { signal }),

  ipoCalendar: (range, signal) => request('/ipo/calendar', { query: { from: range?.from, to: range?.to }, signal }),
  ipoRiskProfile: (symbol, date, signal) => request(`/ipo/${encodeURIComponent(symbol)}/risk-profile`, { query: { date }, signal }),
  ipoProspectus: (symbol, maxChars, signal) => request(`/ipo/${encodeURIComponent(symbol)}/prospectus`, { query: { maxChars }, signal }),
  ipoPerformance: (symbol, date, signal) => request(`/ipo/${encodeURIComponent(symbol)}/performance`, { query: { date }, signal }),
  ipoBaseRate: (months, signal) => request('/ipo/recent-performance', { query: { months }, signal }),
}
