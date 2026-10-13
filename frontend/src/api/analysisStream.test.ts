import { afterEach, describe, expect, it, vi } from 'vitest'
import { httpApi } from './http'
import { streamAnalysisEvents } from './sse'

function sse(chunks: string[], status = 200): typeof fetch {
  return (async () =>
    new Response(new ReadableStream({ start(c) { chunks.forEach((x) => c.enqueue(new TextEncoder().encode(x))); c.close() } }), { status })) as typeof fetch
}

const stage = (i: number) => `data: {"type":"stage","stage":"s${i}","label":"Etapa ${i}","index":${i},"total":3}\n\n`

describe('streamAnalysisEvents', () => {
  it('itera etapas y termina en done', async () => {
    const out: string[] = []
    const done = 'data: {"type":"done","analysis":{"id":"a1"}}\n\n'
    for await (const e of streamAnalysisEvents('/x', undefined, sse([stage(1), stage(2), done]))) out.push(e.type)
    expect(out).toEqual(['stage', 'stage', 'done'])
  })
  it('emite error si el servidor corta antes de done', async () => {
    const out = []
    for await (const e of streamAnalysisEvents('/x', undefined, sse([stage(1)]))) out.push(e)
    expect(out[1]).toMatchObject({ type: 'error', code: 'stream_closed' })
  })
  it('un evento error del servidor cierra el stream', async () => {
    const out = []
    for await (const e of streamAnalysisEvents('/x', undefined, sse(['data: {"type":"error","code":"unavailable","message":"LLM caído"}\n\n', stage(2)]))) out.push(e.type)
    expect(out).toEqual(['error'])
  })
  it('lanza ApiError con el estado si no es 2xx', async () => {
    const f = (async () => new Response('{"error":{"code":"not_found","message":"ruta"}}', { status: 404 })) as typeof fetch
    await expect((async () => { for await (const e of streamAnalysisEvents('/x', undefined, f)) void e })()).rejects.toMatchObject({ status: 404, code: 'not_found' })
  })
})

describe('httpApi.streamAnalysis (fallback)', () => {
  afterEach(() => vi.unstubAllGlobals())
  it('cae al POST normal si /analysis/stream da 404', async () => {
    const calls: string[] = []
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      calls.push(`${init.method} ${url}`)
      if (url.includes('/stream')) return new Response('{"error":{"code":"not_found","message":"ruta de la API desconocida"}}', { status: 404 })
      return new Response('{"id":"a9"}', { status: 200 })
    })
    const out = []
    for await (const e of httpApi.streamAnalysis('stock:AAPL', 'quick')) out.push(e)
    expect(out).toEqual([{ type: 'done', analysis: { id: 'a9' } }])
    expect(calls).toEqual([
      'POST /api/assets/stock%3AAAPL/analysis/stream?depth=quick',
      'POST /api/assets/stock%3AAAPL/analysis?depth=quick',
    ])
  })
})

describe('httpApi.history (procedencia en cabeceras)', () => {
  afterEach(() => vi.unstubAllGlobals())
  it('lee X-Data-Source*', async () => {
    vi.stubGlobal('fetch', async () =>
      new Response('[{"t":1,"o":1,"h":2,"l":0.5,"c":1.5,"v":null}]', {
        status: 200,
        headers: { 'x-data-source': 'yahoo', 'x-data-source-url': 'https://finance.yahoo.com/quote/AAPL', 'x-data-as-of': '2026-10-09T20:00:00Z' },
      }),
    )
    const h = await httpApi.history('stock:AAPL', '1y')
    expect(h.candles).toHaveLength(1)
    expect(h).toMatchObject({ source: 'yahoo', sourceUrl: 'https://finance.yahoo.com/quote/AAPL', asOf: '2026-10-09T20:00:00Z' })
  })
  it('sin cabeceras la procedencia es null (no se deduce)', async () => {
    vi.stubGlobal('fetch', async () => new Response('[]', { status: 200 }))
    expect(await httpApi.history('stock:AAPL', '1y')).toMatchObject({ source: null, sourceUrl: null, asOf: null, interval: null })
  })
  it('lee X-Data-Interval (intervalo realmente servido) e ignora valores desconocidos', async () => {
    vi.stubGlobal('fetch', async () => new Response('[]', { status: 200, headers: { 'x-data-interval': '5m' } }))
    expect((await httpApi.history('crypto:BTC', '1d', '1m')).interval).toBe('5m')
    vi.stubGlobal('fetch', async () => new Response('[]', { status: 200, headers: { 'x-data-interval': 'raro' } }))
    expect((await httpApi.history('crypto:BTC', '1d', '1m')).interval).toBeNull()
  })
})
