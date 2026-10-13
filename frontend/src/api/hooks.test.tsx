import { QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import { createQueryClient } from '@/api/queryClient'
import type { AssetId, AssetSummary, Interval, Range } from '@/api/types'
import { mockApi } from '@/mock'
import { genAnalysis } from '@/mock/content'
import { qk, useGenerateAnalysis, useHistory, useNews, useSearch } from './hooks'

const sum = (n: number): AssetSummary => ({ id: `stock:S${n}`, symbol: `S${n}`, name: `S${n}`, class: 'stock', exchange: null, country: null, source: 'x', currency: 'USD' }) as AssetSummary

function shared() {
  const qc = createQueryClient()
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  return { qc, wrapper }
}

describe('claves de React Query: incluyen todos los parámetros que cambian la petición', () => {
  beforeEach(() => setApi(mockApi))

  it('useSearch distingue el límite (antes dos llamadas con límites distintos compartían caché)', async () => {
    setApi({ ...mockApi, search: async (_q, opts) => Array.from({ length: opts?.limit ?? 20 }, (_, i) => sum(i)) })
    const { wrapper } = shared()
    const five = renderHook(() => useSearch('app', { limit: 5 }), { wrapper })
    await waitFor(() => expect(five.result.current.data).toHaveLength(5))
    const twenty = renderHook(() => useSearch('app', { limit: 20 }), { wrapper })
    await waitFor(() => expect(twenty.result.current.data).toHaveLength(20))
    expect(qk.search('app', undefined, 5)).not.toEqual(qk.search('app', undefined, 20))
  })

  it('useSearch distingue la clase y el texto (recortado)', async () => {
    const seen: string[] = []
    setApi({ ...mockApi, search: async (q, opts) => { seen.push(`${q}|${opts?.class ?? ''}`); return [] } })
    const { wrapper } = shared()
    renderHook(() => useSearch(' app ', { class: 'stock' }), { wrapper })
    renderHook(() => useSearch('app', { class: 'crypto' }), { wrapper })
    renderHook(() => useSearch('app'), { wrapper })
    await waitFor(() => expect(seen.sort()).toEqual(['app|', 'app|crypto', 'app|stock']))
  })

  it('useHistory y useNews: rango, intervalo, activo y límite forman parte de la clave', async () => {
    const calls: string[] = []
    setApi({
      ...mockApi,
      history: async (id, range, interval) => {
        calls.push(`${id}/${range}/${interval ?? 'auto'}`)
        return { candles: [], source: null, sourceUrl: null, asOf: null, interval: null }
      },
      news: async (id, limit) => { calls.push(`news ${id}/${limit}`); return [] },
    })
    const { wrapper } = shared()
    type P = { id: AssetId; range: Range; interval: Interval | undefined }
    const props: P = { id: 'stock:AAPL', range: '1y', interval: undefined }
    const h = renderHook((p: P) => useHistory(p.id, p.range, p.interval), { wrapper, initialProps: props })
    await waitFor(() => expect(h.result.current.isSuccess).toBe(true))
    h.rerender({ ...props, range: '5y' })
    await waitFor(() => expect(calls).toContain('stock:AAPL/5y/auto'))
    h.rerender({ id: 'stock:AAPL', range: '5y', interval: '1d' })
    await waitFor(() => expect(calls).toContain('stock:AAPL/5y/1d'))
    h.rerender({ id: 'crypto:BTC', range: '5y', interval: '1d' })
    await waitFor(() => expect(calls).toContain('crypto:BTC/5y/1d'))
    expect(new Set(calls).size).toBe(calls.length) // ninguna petición repetida por compartir clave
    const n = renderHook(() => useNews('stock:AAPL', 10), { wrapper })
    renderHook(() => useNews('stock:AAPL', 40), { wrapper })
    await waitFor(() => expect(n.result.current.isSuccess).toBe(true))
    await waitFor(() => expect(calls).toEqual(expect.arrayContaining(['news stock:AAPL/10', 'news stock:AAPL/40'])))
  })

  it('al cambiar de activo no se enseñan las velas del anterior (keepPreviousData solo entre rangos del mismo activo)', async () => {
    setApi({
      ...mockApi,
      history: async (id) => ({ candles: [{ t: id === 'stock:AAPL' ? 1 : 2, o: 1, h: 1, l: 1, c: 1, v: null }], source: null, sourceUrl: null, asOf: null, interval: null }),
    })
    const { wrapper } = shared()
    const h = renderHook((p: { id: AssetId }) => useHistory(p.id, '1y'), { wrapper, initialProps: { id: 'stock:AAPL' as AssetId } })
    await waitFor(() => expect(h.result.current.data?.candles[0]?.t).toBe(1))
    h.rerender({ id: 'crypto:BTC' })
    // Mientras llega BTC, no puede asomar la serie de AAPL como si fuera de BTC.
    expect(h.result.current.data?.candles[0]?.t).not.toBe(1)
    await waitFor(() => expect(h.result.current.data?.candles[0]?.t).toBe(2))
  })
})

describe('useGenerateAnalysis: carreras al cambiar de activo con un análisis en curso', () => {
  it('el análisis que termina tras salir de la ficha se guarda en SU activo y no aparece en el nuevo', async () => {
    let finish!: () => void
    const gate = new Promise<void>((r) => (finish = r))
    const aapl = genAnalysis('stock:AAPL', 1, new Date())
    setApi({
      ...mockApi,
      streamAnalysis: async function* (id) {
        yield { type: 'stage', stage: 'snapshot', label: 'Preparando', index: 1, total: 4 }
        await gate
        yield { type: 'done', analysis: id === 'stock:AAPL' ? aapl : genAnalysis(id, 1, new Date()) }
      },
    })
    const { qc, wrapper } = shared()
    const a = renderHook(() => useGenerateAnalysis('stock:AAPL'), { wrapper })
    act(() => a.result.current.mutate('quick'))
    await waitFor(() => expect(a.result.current.stages).toHaveLength(1))
    // El usuario navega a otra ficha (AssetView se remonta por `key={id}`): el hook de AAPL se desmonta.
    a.unmount()
    const b = renderHook(() => useGenerateAnalysis('stock:NVDA'), { wrapper })
    expect(b.result.current.isPending).toBe(false)
    expect(b.result.current.stages).toEqual([])
    // Termina el análisis de AAPL.
    finish()
    await waitFor(() => expect(qc.getQueryData(qk.analysis('stock:AAPL'))).toEqual(aapl))
    expect(qc.getQueryData(qk.analysis('stock:NVDA'))).toBeUndefined()
    expect(qc.getQueryData(qk.analyses('stock:NVDA'))).toBeUndefined()
    expect(b.result.current.data).toBeUndefined()
    expect(b.result.current.stages).toEqual([])
  })
})
