import type { Api } from './api'
import { assetPath, request } from './client'
import { streamChatEvents, streamPullEvents } from './sse'

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
  history: (id, range, interval, signal) =>
    request(`${assetPath(id)}/history`, { query: { range, interval }, signal }),
  indicators: (id, signal) => request(`${assetPath(id)}/indicators`, { signal }),
  fundamentals: (id, signal) => request(`${assetPath(id)}/fundamentals`, { signal }),
  news: (id, limit, signal) => request(`${assetPath(id)}/news`, { query: { limit }, signal }),
  marketNews: (limit, signal) => request('/news', { query: { limit }, signal }),
  macro: (signal) => request('/macro', { signal }),

  latestAnalysis: (id, signal) => request(`${assetPath(id)}/analysis`, { signal }),
  generateAnalysis: (id, depth, signal) =>
    request(`${assetPath(id)}/analysis`, { method: 'POST', query: { depth }, signal }),
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
}
