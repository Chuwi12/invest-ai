import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, assetPath, buildQuery, request } from './client'

afterEach(() => vi.unstubAllGlobals())

function stubFetch(res: Response | Error | DOMException) {
  const f = vi.fn(async (..._args: unknown[]) => {
    if (!(res instanceof Response)) throw res
    return res
  })
  vi.stubGlobal('fetch', f)
  return f
}

describe('buildQuery', () => {
  it('omite nulos y vacíos', () => {
    expect(buildQuery({ q: 'apple', limit: 5, class: undefined, x: null, y: '' })).toBe('?q=apple&limit=5')
    expect(buildQuery({})).toBe('')
  })
})

describe('assetPath', () => {
  it('codifica el id', () => {
    expect(assetPath('stock:AAPL')).toBe('/assets/stock%3AAAPL')
    expect(assetPath('index:^GSPC')).toBe('/assets/index%3A%5EGSPC')
    expect(assetPath('forex:EURUSD=X')).toBe('/assets/forex%3AEURUSD%3DX')
  })
})

describe('request', () => {
  it('devuelve el JSON', async () => {
    const f = stubFetch(new Response('{"status":"ok","version":"1"}', { status: 200 }))
    await expect(request('/health')).resolves.toEqual({ status: 'ok', version: '1' })
    expect(f.mock.calls[0]?.[0]).toBe('/api/health')
  })

  it('204 devuelve undefined', async () => {
    stubFetch(new Response(null, { status: 204 }))
    await expect(request('/favorites/x', { method: 'DELETE' })).resolves.toBeUndefined()
  })

  it('envía el body como JSON', async () => {
    const f = stubFetch(new Response('{}', { status: 200 }))
    await request('/conversations', { method: 'POST', body: { title: 'a' } })
    const init = f.mock.calls[0]?.[1] as RequestInit
    expect(init.body).toBe('{"title":"a"}')
    expect((init.headers as Record<string, string>)['Content-Type']).toBe('application/json')
  })

  it('traduce el error JSON del contrato', async () => {
    stubFetch(new Response('{"error":{"code":"rate_limited","message":"Demasiadas peticiones"}}', { status: 429 }))
    await expect(request('/search')).rejects.toMatchObject({
      name: 'ApiError',
      status: 429,
      code: 'rate_limited',
      message: 'Demasiadas peticiones',
    })
  })

  it('tolera cuerpos de error que no son JSON', async () => {
    stubFetch(new Response('Bad gateway', { status: 502 }))
    const err = await request('/x').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 502, code: 'http_502', message: 'Bad gateway' })
  })

  it('error de red => ApiError network', async () => {
    stubFetch(new TypeError('Failed to fetch'))
    await expect(request('/x')).rejects.toMatchObject({ code: 'network', status: 0 })
  })

  it('no envuelve los AbortError', async () => {
    stubFetch(new DOMException('abort', 'AbortError'))
    await expect(request('/x')).rejects.toMatchObject({ name: 'AbortError' })
  })
})
