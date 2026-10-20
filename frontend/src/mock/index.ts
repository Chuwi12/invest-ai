import type { Api } from '@/api/api'
import { ApiError } from '@/api/client'
import { setApi } from '@/api'
import type {
  Analysis,
  AssetId,
  Conversation,
  Dashboard,
  Favorite,
  ProviderStatus,
  ServiceStatus,
  ViewStat,
} from '@/api/types'
import { RealtimeClient, setRealtime } from '@/realtime'
import { MARKET_PANEL, searchCatalog, toSummary } from './catalog'
import { mockChatStream } from './chat'
import { genAnalysis, genMacro, genNews, genSnapshot } from './content'
import { getDb, nextId, saveDb } from './db'
import { entryFor, genCandles, genFundamentals, genIndicators, genQuote } from './market'
import { mockIpoBaseRate, mockIpoCalendar, mockIpoPerformance, mockIpoProspectus, mockIpoRiskProfile } from './ipo'
import { mockCalibrated, mockPerformance, mockRollback, mockWatchlist, mockWeights } from './learning'
import { activeModelId, installedModelIds, mockDelete, mockModels, mockPull, mockSetActive, mockSetForceGpu } from './models'
import { sleep } from './rng'
import { mockAssetSocial, mockSocialStatus } from './social'
import { MockSocket } from './socket'

/** Latencia artificial: deja ver los skeletons sin hacerse pesada. */
const LATENCY = 90

async function lag(signal?: AbortSignal, ms = LATENCY): Promise<void> {
  await sleep(ms, signal)
}

function assertExists(id: AssetId): void {
  if (id.endsWith(':NOTFOUND')) throw new ApiError(404, 'not_found', `activo no encontrado: ${id}`)
}

function viewStats(): ViewStat[] {
  const db = getDb()
  const now = Date.now()
  return Object.entries(db.views)
    .map(([id, v]) => ({ id: id as AssetId, summary: toSummary(entryFor(id as AssetId)), views: v.views, lastViewedAt: v.lastViewedAt }))
    .sort((a, b) => {
      // Ponderado por recencia: cada día de antigüedad resta peso.
      const w = (s: ViewStat) => s.views / (1 + (now - Date.parse(s.lastViewedAt)) / 86_400_000 / 4)
      return w(b) - w(a)
    })
}

const PROVIDERS: ProviderStatus[] = [
  {
    name: 'yahoo', enabled: true, requiresKey: false, hasKey: false,
    capabilities: ['search', 'quote', 'history', 'fundamentals', 'news', 'options'],
    assetClasses: ['stock', 'etf', 'fund', 'index', 'future', 'commodity', 'forex', 'option', 'bond'],
    lastError: null, lastOk: new Date(Date.now() - 4_000).toISOString(), latencyMs: 184,
  },
  {
    name: 'binance', enabled: true, requiresKey: false, hasKey: false,
    capabilities: ['quote', 'realtime', 'history'], assetClasses: ['crypto'],
    lastError: null, lastOk: new Date(Date.now() - 1_000).toISOString(), latencyMs: 61,
  },
  {
    name: 'coingecko', enabled: true, requiresKey: false, hasKey: false,
    capabilities: ['search', 'fundamentals'], assetClasses: ['crypto'],
    lastError: 'límite de peticiones alcanzado', lastOk: new Date(Date.now() - 600_000).toISOString(), blockedSecs: 42, latencyMs: 912,
  },
  {
    name: 'finnhub', enabled: false, requiresKey: true, hasKey: false,
    capabilities: ['realtime', 'news', 'fundamentals'], assetClasses: ['stock', 'etf', 'forex', 'crypto'],
    lastError: null, lastOk: null,
  },
  {
    name: 'fred', enabled: true, requiresKey: true, hasKey: true,
    capabilities: ['macro', 'history'], assetClasses: ['bond'],
    lastError: null, lastOk: new Date(Date.now() - 3_600_000).toISOString(),
  },
  {
    name: 'gdelt', enabled: true, requiresKey: false, hasKey: false,
    capabilities: ['news'], assetClasses: ['stock', 'etf', 'crypto', 'index', 'forex', 'commodity', 'bond'],
    lastError: null, lastOk: new Date(Date.now() - 90_000).toISOString(),
  },
]

export const mockApi: Api = {
  async health(signal) {
    await lag(signal, 30)
    return { status: 'ok', version: '0.0.0-mock' }
  },

  async status(signal): Promise<ServiceStatus> {
    await lag(signal)
    return {
      llm: {
        available: true,
        baseUrl: 'http://127.0.0.1:11434',
        model: activeModelId(),
        installedModels: installedModelIds(),
        supportsTools: true,
        error: null,
      },
      providers: PROVIDERS,
    }
  },

  async search(q, opts, signal) {
    await lag(signal, 120)
    const db = getDb()
    const query = q.trim()
    // Igual que el backend real: cada búsqueda queda registrada.
    if (db.searches[0]?.query !== query) {
      db.searches.unshift({ query, selected: null, at: new Date().toISOString() })
      db.searches = db.searches.slice(0, 50)
      saveDb()
    }
    return searchCatalog(query, opts?.limit ?? 20, opts?.class).map(toSummary)
  },

  async asset(id, signal) {
    await lag(signal)
    assertExists(id)
    const db = getDb()
    const prev = db.views[id]
    db.views[id] = { views: (prev?.views ?? 0) + 1, lastViewedAt: new Date().toISOString() }
    saveDb()
    return {
      summary: toSummary(entryFor(id)),
      quote: genQuote(id),
      fundamentals: genFundamentals(id),
      isFavorite: db.favorites.some((f) => f.id === id),
    }
  },

  async quote(id, signal) {
    await lag(signal, 40)
    assertExists(id)
    return genQuote(id)
  },

  async quotes(ids, signal) {
    await lag(signal, 40)
    return ids.filter((id) => !id.endsWith(':NOTFOUND')).map(genQuote)
  },

  async history(id, range, interval, signal) {
    await lag(signal, 150)
    assertExists(id)
    const candles = genCandles(id, range, interval)
    const source = id.startsWith('crypto:') ? 'binance' : 'yahoo'
    const sym = encodeURIComponent(id.slice(id.indexOf(':') + 1))
    return {
      candles,
      source,
      sourceUrl: source === 'binance' ? `https://www.binance.com/en/trade/${sym}_USDT` : `https://finance.yahoo.com/quote/${sym}`,
      asOf: new Date((candles[candles.length - 1]?.t ?? 0) * 1000).toISOString(),
    }
  },

  async indicators(id, signal) {
    await lag(signal)
    assertExists(id)
    return genIndicators(id)
  },

  async fundamentals(id, signal) {
    await lag(signal)
    return genFundamentals(id)
  },

  async news(id, limit, signal) {
    await lag(signal, 140)
    return genNews(id, limit ?? 30)
  },

  async marketNews(limit, signal) {
    await lag(signal)
    return genNews(null, limit ?? 30)
  },

  async macro(signal) {
    await lag(signal)
    return genMacro()
  },

  async latestAnalysis(id, signal) {
    await lag(signal)
    return getDb().analyses[id]?.[0] ?? null
  },

  async generateAnalysis(id, depth, signal) {
    // Simula la espera real del LLM (en producción son 10–60 s).
    const t0 = Date.now()
    await sleep(depth === 'quick' ? 3500 : depth === 'deep' ? 14_000 : 7000, signal)
    const db = getDb()
    const list = (db.analyses[id] ??= [])
    const a = genAnalysis(id, list.length + 1, new Date(), { depth, durationMs: Date.now() - t0 })
    list.unshift(a)
    db.analyses[id] = list.slice(0, 20)
    saveDb()
    return a
  },

  async *streamAnalysis(id, depth, signal) {
    // Etapas como las del backend real (mismo contrato SSE), con pausas proporcionales a la profundidad.
    const scale = depth === 'quick' ? 0.4 : depth === 'deep' ? 2 : 1
    const stages: [string, string, number][] = [
      ['data', 'Reuniendo datos en vivo', 1500],
      ['scoring', 'Calculando puntuaciones', 1000],
      ['llm', 'El modelo redacta el análisis', 3500],
      ['validate', 'Validando el resultado', 800],
    ]
    const t0 = Date.now()
    for (const [i, [stage, label, ms]] of stages.entries()) {
      yield { type: 'stage', stage, label, index: i + 1, total: stages.length }
      await sleep(ms * scale, signal)
    }
    const db = getDb()
    const list = (db.analyses[id] ??= [])
    const a = genAnalysis(id, list.length + 1, new Date(), { depth, durationMs: Date.now() - t0 })
    list.unshift(a)
    db.analyses[id] = list.slice(0, 20)
    saveDb()
    yield { type: 'done', analysis: a }
  },

  async analyses(id, signal) {
    await lag(signal)
    return getDb().analyses[id] ?? []
  },

  async snapshot(id, signal) {
    await lag(signal, 200)
    return genSnapshot(id)
  },

  async listConversations(signal) {
    await lag(signal)
    return [...getDb().conversations].sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt))
  },

  async createConversation(body) {
    await lag(undefined, 60)
    const db = getDb()
    const now = new Date().toISOString()
    const conv: Conversation = {
      id: nextId('c'),
      title: body.title ?? '',
      assetIds: body.assetIds ?? [],
      pinned: false,
      messageCount: 0,
      lastMessagePreview: null,
      createdAt: now,
      updatedAt: now,
    }
    db.conversations.unshift(conv)
    db.messages[conv.id] = []
    saveDb()
    return conv
  },

  async getConversation(cid, signal) {
    await lag(signal)
    const db = getDb()
    const conversation = db.conversations.find((c) => c.id === cid)
    if (!conversation) throw new ApiError(404, 'not_found', 'conversación no encontrada')
    return { conversation, messages: db.messages[cid] ?? [] }
  },

  async updateConversation(cid, body) {
    await lag(undefined, 60)
    const db = getDb()
    const c = db.conversations.find((x) => x.id === cid)
    if (!c) throw new ApiError(404, 'not_found', 'conversación no encontrada')
    if (body.title !== undefined) c.title = body.title
    if (body.assetIds !== undefined) c.assetIds = body.assetIds
    if (body.pinned !== undefined) c.pinned = body.pinned
    saveDb()
    return c
  },

  async deleteConversation(cid) {
    await lag(undefined, 60)
    const db = getDb()
    db.conversations = db.conversations.filter((c) => c.id !== cid)
    delete db.messages[cid]
    saveDb()
  },

  sendMessage: (cid, body, signal) => mockChatStream(cid, body, signal),

  async listFavorites(signal) {
    await lag(signal)
    return getDb().favorites
  },

  async putFavorite(id, note) {
    await lag(undefined, 80)
    const db = getDb()
    const existing = db.favorites.find((f) => f.id === id)
    if (existing) {
      if (note !== undefined) existing.note = note === null || note.trim() === '' ? null : note
      saveDb()
      return existing
    }
    const fav: Favorite = { id, summary: toSummary(entryFor(id)), note: note ?? null, addedAt: new Date().toISOString() }
    db.favorites.unshift(fav)
    saveDb()
    return fav
  },

  async deleteFavorite(id) {
    await lag(undefined, 80)
    const db = getDb()
    db.favorites = db.favorites.filter((f) => f.id !== id)
    saveDb()
  },

  async searchHistory(signal) {
    await lag(signal)
    return getDb().searches.slice(0, 50)
  },

  async clearSearchHistory() {
    await lag(undefined, 60)
    getDb().searches = []
    saveDb()
  },

  async recordSearch(query, selected) {
    const db = getDb()
    const entry = db.searches.find((s) => s.query === query.trim())
    if (entry) entry.selected = selected
    else db.searches.unshift({ query: query.trim(), selected, at: new Date().toISOString() })
    saveDb()
  },

  async viewStats(signal) {
    await lag(signal)
    return viewStats()
  },

  async clearViews() {
    await lag(undefined, 60)
    getDb().views = {}
    saveDb()
  },

  async dashboard(signal): Promise<Dashboard> {
    await lag(signal, 140)
    const db = getDb()
    const latest: Analysis[] = Object.values(db.analyses)
      .flat()
      .sort((a, b) => Date.parse(b.generatedAt) - Date.parse(a.generatedAt))
      .slice(0, 5)
    return {
      favorites: db.favorites.map((favorite) => ({ favorite, quote: genQuote(favorite.id) })),
      mostViewed: viewStats().slice(0, 8).map((stat) => ({ stat, quote: genQuote(stat.id) })),
      recentSearches: db.searches.slice(0, 10),
      recentConversations: [...db.conversations].sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt)).slice(0, 5),
      latestAnalyses: latest,
      market: MARKET_PANEL.map(genQuote),
      news: genNews(null, 10),
    }
  },

  async models(signal) {
    await lag(signal)
    return mockModels()
  },

  async setActiveModel(id) {
    await lag(undefined, 120)
    return mockSetActive(id)
  },

  pullModel: (id, signal) => mockPull(id, signal),

  async deleteModel(id) {
    await lag(undefined, 150)
    mockDelete(id)
  },

  async setModelForceGpu(id, force) {
    await lag(undefined, 120)
    return mockSetForceGpu(id, force)
  },

  async performance(signal) {
    await lag(signal, 160)
    return mockPerformance()
  },

  async learningWeights(signal) {
    await lag(signal)
    return mockWeights()
  },

  async rollbackWeights(version) {
    await lag(undefined, 200)
    return mockRollback(version)
  },

  async calibratedConfidence(confidence, horizon = 4, _model, signal) {
    await lag(signal, 60)
    return mockCalibrated(confidence, horizon)
  },

  async watchlist(signal) {
    await lag(signal, 120)
    return mockWatchlist()
  },

  async socialStatus(signal) {
    await lag(signal, 60)
    return mockSocialStatus()
  },

  async assetSocial(id, signal) {
    await lag(signal, 150)
    return mockAssetSocial(id)
  },

  async ipoCalendar(range, signal) {
    await lag(signal)
    return mockIpoCalendar(range?.from, range?.to)
  },

  async ipoRiskProfile(symbol, _date, signal) {
    await lag(signal, 160)
    return mockIpoRiskProfile(symbol)
  },

  async ipoProspectus(symbol, _maxChars, signal) {
    await lag(signal, 120)
    return mockIpoProspectus(symbol)
  },

  async ipoPerformance(symbol, _date, signal) {
    await lag(signal, 120)
    return mockIpoPerformance(symbol)
  },

  async ipoBaseRate(months, signal) {
    await lag(signal, 120)
    return mockIpoBaseRate(months)
  },
}

/** Activa el modo mock: API simulada + WebSocket simulado. */
export function installMock(): void {
  setApi(mockApi)
  setRealtime(new RealtimeClient({ url: 'mock://ws', createSocket: () => new MockSocket() }))
  console.info('[invest-ai] modo MOCK activo (VITE_MOCK=1): datos simulados, sin backend')
}
