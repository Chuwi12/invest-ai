import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from './index'
import { isAbortError } from './client'
import type {
  Analysis,
  AnalysisDepth,
  AssetClass,
  AssetDetail,
  AssetId,
  AssetSummary,
  Conversation,
  ConversationWithMessages,
  Dashboard,
  Favorite,
  Interval,
  ModelsResponse,
  Range,
  SearchEntry,
  UpdateConversationBody,
  ViewStat,
} from './types'

const SEC = 1000
const MIN = 60 * SEC

/** Claves de caché. */
export const qk = {
  health: ['health'] as const,
  status: ['status'] as const,
  models: ['models'] as const,
  search: (q: string, cls?: AssetClass) => ['search', q, cls ?? 'all'] as const,
  asset: (id: AssetId) => ['asset', id] as const,
  quote: (id: AssetId) => ['quote', id] as const,
  quotes: (ids: readonly AssetId[]) => ['quotes', ...ids] as const,
  history: (id: AssetId, range: Range, interval?: Interval) => ['history', id, range, interval ?? 'default'] as const,
  indicators: (id: AssetId) => ['indicators', id] as const,
  fundamentals: (id: AssetId) => ['fundamentals', id] as const,
  news: (id: AssetId) => ['news', id] as const,
  marketNews: ['news', 'market'] as const,
  macro: ['macro'] as const,
  analysis: (id: AssetId) => ['analysis', id] as const,
  analyses: (id: AssetId) => ['analyses', id] as const,
  snapshot: (id: AssetId) => ['snapshot', id] as const,
  conversations: ['conversations'] as const,
  conversation: (cid: string) => ['conversation', cid] as const,
  favorites: ['favorites'] as const,
  searchHistory: ['history', 'searches'] as const,
  views: ['history', 'views'] as const,
  dashboard: ['dashboard'] as const,
}

/** Vigencia por tipo de dato. */
export const STALE = {
  quote: 5 * SEC,
  /** Intradía: 1 min; diario: 15 min. */
  history: (range: Range) => (range === '1d' || range === '5d' ? MIN : range === '1mo' ? 5 * MIN : 15 * MIN),
  indicators: 5 * MIN,
  fundamentals: 60 * MIN,
  news: 2 * MIN,
  analysis: MIN,
  dashboard: 30 * SEC,
  status: 30 * SEC,
  search: 5 * MIN,
} as const

// --- Sistema ---------------------------------------------------------------------------------

export function useStatus() {
  return useQuery({
    queryKey: qk.status,
    queryFn: ({ signal }) => api.status(signal),
    staleTime: STALE.status,
    refetchInterval: MIN,
    refetchOnWindowFocus: true,
  })
}

// --- Mercado ---------------------------------------------------------------------------------

export function useSearch(q: string, opts?: { class?: AssetClass; limit?: number; enabled?: boolean }) {
  const term = q.trim()
  return useQuery({
    queryKey: qk.search(term, opts?.class),
    queryFn: ({ signal }) => {
      const o: { limit?: number; class?: AssetClass } = { limit: opts?.limit ?? 20 }
      if (opts?.class) o.class = opts.class
      return api.search(term, o, signal)
    },
    enabled: (opts?.enabled ?? true) && term.length >= 2,
    staleTime: STALE.search,
    placeholderData: keepPreviousData,
  })
}

/** `GET /assets/{id}` registra una visita: se evita refetch al volver a la pestaña o a la ficha. */
export function useAsset(id: AssetId) {
  return useQuery({
    queryKey: qk.asset(id),
    queryFn: ({ signal }) => api.asset(id, signal),
    staleTime: MIN,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  })
}

export function useQuote(id: AssetId, opts?: { poll?: boolean; enabled?: boolean }) {
  return useQuery({
    queryKey: qk.quote(id),
    queryFn: ({ signal }) => api.quote(id, signal),
    staleTime: STALE.quote,
    enabled: opts?.enabled ?? true,
    refetchInterval: opts?.poll ? 15 * SEC : false,
  })
}

export function useHistory(id: AssetId, range: Range, interval?: Interval) {
  return useQuery({
    queryKey: qk.history(id, range, interval),
    queryFn: ({ signal }) => api.history(id, range, interval, signal),
    staleTime: STALE.history(range),
    gcTime: 30 * MIN,
    placeholderData: keepPreviousData,
  })
}

/** Mini-serie de cierres para sparklines (1 mes en velas diarias). Solo carga si `enabled`. */
export function useSparkline(id: AssetId, enabled = true) {
  return useQuery({
    queryKey: qk.history(id, '1mo', '1d'),
    queryFn: ({ signal }) => api.history(id, '1mo', '1d', signal),
    staleTime: 5 * MIN,
    gcTime: 30 * MIN,
    enabled,
    select: (candles) => candles.map((c) => c.c),
  })
}

export function useIndicators(id: AssetId) {
  return useQuery({
    queryKey: qk.indicators(id),
    queryFn: ({ signal }) => api.indicators(id, signal),
    staleTime: STALE.indicators,
  })
}

export function useFundamentals(id: AssetId) {
  return useQuery({
    queryKey: qk.fundamentals(id),
    queryFn: ({ signal }) => api.fundamentals(id, signal),
    staleTime: STALE.fundamentals,
  })
}

export function useNews(id: AssetId, limit = 40) {
  return useQuery({
    queryKey: [...qk.news(id), limit] as const,
    queryFn: ({ signal }) => api.news(id, limit, signal),
    staleTime: STALE.news,
  })
}

// --- Análisis --------------------------------------------------------------------------------

export function useLatestAnalysis(id: AssetId) {
  return useQuery({
    queryKey: qk.analysis(id),
    queryFn: ({ signal }) => api.latestAnalysis(id, signal),
    staleTime: STALE.analysis,
  })
}

export function useAnalyses(id: AssetId, enabled = true) {
  return useQuery({
    queryKey: qk.analyses(id),
    queryFn: ({ signal }) => api.analyses(id, signal),
    staleTime: 5 * MIN,
    enabled,
  })
}

export function useSnapshot(id: AssetId, enabled = true) {
  return useQuery({
    queryKey: qk.snapshot(id),
    queryFn: ({ signal }) => api.snapshot(id, signal),
    staleTime: 30 * SEC,
    enabled,
  })
}

/**
 * Genera un análisis nuevo (10–60 s). Devuelve `run` (con su propio AbortController) y `cancel`.
 * Cancelar corta la espera en el navegador; el backend puede terminar y guardar el resultado.
 */
export function useGenerateAnalysis(id: AssetId) {
  const qc = useQueryClient()
  const controller = useRef<AbortController | null>(null)
  const mutation = useMutation({
    mutationFn: (depth: AnalysisDepth) => {
      const ac = new AbortController()
      controller.current = ac
      return api.generateAnalysis(id, depth, ac.signal)
    },
    onSuccess: (analysis) => {
      qc.setQueryData<Analysis | null>(qk.analysis(id), analysis)
      qc.setQueryData<Analysis[]>(qk.analyses(id), (old) => [analysis, ...(old ?? []).filter((a) => a.id !== analysis.id)].slice(0, 20))
      void qc.invalidateQueries({ queryKey: qk.dashboard })
    },
    onSettled: () => {
      controller.current = null
    },
  })
  const cancel = useCallback(() => controller.current?.abort(), [])
  return { ...mutation, cancel }
}

// --- Conversaciones --------------------------------------------------------------------------

export function useConversations() {
  return useQuery({
    queryKey: qk.conversations,
    queryFn: ({ signal }) => api.listConversations(signal),
    staleTime: 30 * SEC,
    refetchOnWindowFocus: true,
  })
}

export function useConversation(cid: string | undefined) {
  return useQuery({
    queryKey: qk.conversation(cid ?? ''),
    queryFn: ({ signal }) => api.getConversation(cid ?? '', signal),
    enabled: !!cid,
    staleTime: 60 * SEC,
    refetchOnWindowFocus: false,
  })
}

/** Ordena fijadas primero y luego por actividad reciente. */
export function sortConversations(list: readonly Conversation[]): Conversation[] {
  return [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned) || Date.parse(b.updatedAt) - Date.parse(a.updatedAt))
}

export function useCreateConversation() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: { title?: string; assetIds?: AssetId[] }) => api.createConversation(body),
    onSuccess: (conv) => {
      qc.setQueryData<Conversation[]>(qk.conversations, (old) => [conv, ...(old ?? [])])
      qc.setQueryData<ConversationWithMessages>(qk.conversation(conv.id), { conversation: conv, messages: [] })
    },
  })
}

export function useUpdateConversation() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ cid, body }: { cid: string; body: UpdateConversationBody }) => api.updateConversation(cid, body),
    onMutate: async ({ cid, body }) => {
      await qc.cancelQueries({ queryKey: qk.conversations })
      const prev = qc.getQueryData<Conversation[]>(qk.conversations)
      const patch = (c: Conversation): Conversation => ({
        ...c,
        ...(body.title !== undefined ? { title: body.title } : {}),
        ...(body.pinned !== undefined ? { pinned: body.pinned } : {}),
        ...(body.assetIds !== undefined ? { assetIds: body.assetIds } : {}),
      })
      qc.setQueryData<Conversation[]>(qk.conversations, (old) => old?.map((c) => (c.id === cid ? patch(c) : c)))
      qc.setQueryData<ConversationWithMessages>(qk.conversation(cid), (old) =>
        old ? { ...old, conversation: patch(old.conversation) } : old,
      )
      return { prev }
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.conversations, ctx.prev)
    },
    onSettled: (_d, _e, { cid }) => {
      void qc.invalidateQueries({ queryKey: qk.conversations })
      void qc.invalidateQueries({ queryKey: qk.conversation(cid) })
    },
  })
}

export function useDeleteConversation() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (cid: string) => api.deleteConversation(cid),
    onMutate: async (cid) => {
      await qc.cancelQueries({ queryKey: qk.conversations })
      const prev = qc.getQueryData<Conversation[]>(qk.conversations)
      qc.setQueryData<Conversation[]>(qk.conversations, (old) => old?.filter((c) => c.id !== cid))
      return { prev }
    },
    onError: (_e, _cid, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.conversations, ctx.prev)
    },
    onSettled: (_d, _e, cid) => {
      qc.removeQueries({ queryKey: qk.conversation(cid) })
      void qc.invalidateQueries({ queryKey: qk.conversations })
      void qc.invalidateQueries({ queryKey: qk.dashboard })
    },
  })
}

// --- Favoritos -------------------------------------------------------------------------------

export function useFavorites() {
  return useQuery({
    queryKey: qk.favorites,
    queryFn: ({ signal }) => api.listFavorites(signal),
    staleTime: MIN,
  })
}

const EMPTY_IDS: ReadonlySet<AssetId> = new Set()

/** Conjunto de ids favoritos (el `select` memoizado evita renders si la lista no cambia). */
export function useFavoriteIds(): ReadonlySet<AssetId> {
  const { data } = useQuery({
    queryKey: qk.favorites,
    queryFn: ({ signal }) => api.listFavorites(signal),
    staleTime: MIN,
    select: selectIds,
  })
  return data ?? EMPTY_IDS
}

const idsCache = new WeakMap<Favorite[], Set<AssetId>>()
function selectIds(list: Favorite[]): Set<AssetId> {
  let s = idsCache.get(list)
  if (!s) idsCache.set(list, (s = new Set(list.map((f) => f.id))))
  return s
}

/** Añade o quita un favorito con actualización optimista y reversión si falla. */
export function useToggleFavorite() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ summary, on }: { summary: AssetSummary; on: boolean }) =>
      on ? api.putFavorite(summary.id).then(() => undefined) : api.deleteFavorite(summary.id),
    onMutate: async ({ summary, on }) => {
      const id = summary.id
      await Promise.all([qc.cancelQueries({ queryKey: qk.favorites }), qc.cancelQueries({ queryKey: qk.asset(id) })])
      const prevFavs = qc.getQueryData<Favorite[]>(qk.favorites)
      const prevAsset = qc.getQueryData<AssetDetail>(qk.asset(id))
      qc.setQueryData<Favorite[]>(qk.favorites, (old = []) => {
        const rest = old.filter((f) => f.id !== id)
        return on ? [{ id, summary, note: null, addedAt: new Date().toISOString() }, ...rest] : rest
      })
      qc.setQueryData<AssetDetail>(qk.asset(id), (old) => (old ? { ...old, isFavorite: on } : old))
      return { prevFavs, prevAsset, id }
    },
    onError: (_e, _v, ctx) => {
      if (!ctx) return
      qc.setQueryData(qk.favorites, ctx.prevFavs)
      qc.setQueryData(qk.asset(ctx.id), ctx.prevAsset)
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: qk.favorites })
      void qc.invalidateQueries({ queryKey: qk.dashboard })
    },
  })
}

/** Guarda la nota de un favorito (`PUT /favorites/{id}` con `{ note }`). */
export function useSetFavoriteNote() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, note }: { id: AssetId; note: string }) => api.putFavorite(id, note),
    onMutate: async ({ id, note }) => {
      await qc.cancelQueries({ queryKey: qk.favorites })
      const prev = qc.getQueryData<Favorite[]>(qk.favorites)
      qc.setQueryData<Favorite[]>(qk.favorites, (old) =>
        old?.map((f) => (f.id === id ? { ...f, note: note.trim() === '' ? null : note } : f)),
      )
      return { prev }
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.favorites, ctx.prev)
    },
    onSettled: () => void qc.invalidateQueries({ queryKey: qk.favorites }),
  })
}

// --- Historial y panel -----------------------------------------------------------------------

export function useSearchHistory(enabled = true) {
  return useQuery({
    queryKey: qk.searchHistory,
    queryFn: ({ signal }) => api.searchHistory(signal),
    staleTime: 30 * SEC,
    enabled,
  })
}

export function useViewStats() {
  return useQuery({
    queryKey: qk.views,
    queryFn: ({ signal }) => api.viewStats(signal),
    staleTime: 30 * SEC,
  })
}

export function useClearSearchHistory() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => api.clearSearchHistory(),
    onMutate: async () => {
      await qc.cancelQueries({ queryKey: qk.searchHistory })
      const prev = qc.getQueryData<SearchEntry[]>(qk.searchHistory)
      qc.setQueryData<SearchEntry[]>(qk.searchHistory, [])
      return { prev }
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.searchHistory, ctx.prev)
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: qk.searchHistory })
      void qc.invalidateQueries({ queryKey: qk.dashboard })
    },
  })
}

export function useClearViews() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => api.clearViews(),
    onMutate: async () => {
      await qc.cancelQueries({ queryKey: qk.views })
      const prev = qc.getQueryData<ViewStat[]>(qk.views)
      qc.setQueryData<ViewStat[]>(qk.views, [])
      return { prev }
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.views, ctx.prev)
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: qk.views })
      void qc.invalidateQueries({ queryKey: qk.dashboard })
    },
  })
}

export function useRecordSearch() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ query, selected }: { query: string; selected: AssetId }) => api.recordSearch(query, selected),
    onSettled: () => void qc.invalidateQueries({ queryKey: qk.searchHistory }),
  })
}

export function useDashboard() {
  return useQuery<Dashboard>({
    queryKey: qk.dashboard,
    queryFn: ({ signal }) => api.dashboard(signal),
    staleTime: STALE.dashboard,
    refetchInterval: 60 * SEC,
    refetchOnWindowFocus: true,
  })
}

// --- Prefetch --------------------------------------------------------------------------------

/**
 * Precarga lo que pinta la ficha de un activo. NO precarga `GET /assets/{id}` porque el backend
 * registra una visita con esa llamada y un simple hover no es una visita.
 */
export function prefetchAsset(qc: QueryClient, id: AssetId): void {
  void qc.prefetchQuery({ queryKey: qk.quote(id), queryFn: ({ signal }) => api.quote(id, signal), staleTime: STALE.quote })
  void qc.prefetchQuery({
    queryKey: qk.history(id, '1y'),
    queryFn: ({ signal }) => api.history(id, '1y', undefined, signal),
    staleTime: STALE.history('1y'),
  })
  void qc.prefetchQuery({
    queryKey: qk.analysis(id),
    queryFn: ({ signal }) => api.latestAnalysis(id, signal),
    staleTime: STALE.analysis,
  })
  void qc.prefetchQuery({
    queryKey: qk.indicators(id),
    queryFn: ({ signal }) => api.indicators(id, signal),
    staleTime: STALE.indicators,
  })
  void import('@/pages/AssetPage')
}

// --- Modelos de IA ---------------------------------------------------------------------------

export function useModels() {
  return useQuery({
    queryKey: qk.models,
    queryFn: ({ signal }) => api.models(signal),
    staleTime: 15 * SEC,
    refetchOnWindowFocus: true,
  })
}

/** Tras tocar los modelos hay que refrescar también el estado del LLM de la cabecera. */
function refreshModelState(qc: QueryClient, data?: ModelsResponse) {
  if (data) qc.setQueryData(qk.models, data)
  else void qc.invalidateQueries({ queryKey: qk.models })
  void qc.invalidateQueries({ queryKey: qk.status })
}

export function useActivateModel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => api.setActiveModel(id),
    onSuccess: (data) => refreshModelState(qc, data),
  })
}

export function useDeleteModel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => api.deleteModel(id),
    onSettled: () => refreshModelState(qc),
  })
}

export interface PullState {
  status: string
  completed: number | null
  total: number | null
  error?: string
}

/** Descargas en curso por id de modelo (varias a la vez), con progreso por SSE y cancelación. */
export function useModelPulls() {
  const qc = useQueryClient()
  const [pulls, setPulls] = useState<Record<string, PullState>>({})
  const controllers = useRef(new Map<string, AbortController>())

  useEffect(() => {
    const map = controllers.current
    return () => map.forEach((c) => c.abort())
  }, [])

  const drop = useCallback((id: string) => {
    controllers.current.delete(id)
    setPulls((p) => {
      const { [id]: _removed, ...rest } = p
      return rest
    })
  }, [])

  const start = useCallback(
    async (id: string) => {
      if (controllers.current.has(id)) return
      const ac = new AbortController()
      controllers.current.set(id, ac)
      setPulls((p) => ({ ...p, [id]: { status: 'Iniciando descarga', completed: null, total: null } }))
      try {
        for await (const ev of api.pullModel(id, ac.signal)) {
          if (ev.type === 'progress') {
            setPulls((p) => ({ ...p, [id]: { status: ev.status, completed: ev.completed, total: ev.total } }))
          } else if (ev.type === 'error') {
            controllers.current.delete(id)
            setPulls((p) => ({ ...p, [id]: { status: 'Error', completed: null, total: null, error: ev.message } }))
            return
          } else {
            break
          }
        }
        drop(id)
        refreshModelState(qc)
      } catch (e) {
        if (isAbortError(e) || ac.signal.aborted) {
          drop(id)
          return
        }
        controllers.current.delete(id)
        const message = e instanceof Error ? e.message : 'Error desconocido'
        setPulls((p) => ({ ...p, [id]: { status: 'Error', completed: null, total: null, error: message } }))
      }
    },
    [qc, drop],
  )

  const cancel = useCallback((id: string) => controllers.current.get(id)?.abort(), [])
  const dismiss = useCallback((id: string) => drop(id), [drop])

  return { pulls, start, cancel, dismiss }
}

// --- Macro y titulares -----------------------------------------------------------------------

export function useMacro() {
  return useQuery({
    queryKey: qk.macro,
    queryFn: ({ signal }) => api.macro(signal),
    staleTime: 30 * MIN,
  })
}

/** Cotizaciones REST de varios activos a la vez (sondeo suave; los ticks los pinta `LivePrice`). */
export function useQuotes(ids: readonly AssetId[]) {
  return useQuery({
    queryKey: qk.quotes(ids),
    queryFn: ({ signal }) => api.quotes([...ids], signal),
    enabled: ids.length > 0,
    staleTime: STALE.quote,
    refetchInterval: 30 * SEC,
  })
}
