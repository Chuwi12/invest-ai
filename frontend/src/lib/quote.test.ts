import { describe, expect, it } from 'vitest'
import type { Quote } from '@/api/types'
import type { LiveTick } from '@/realtime/client'
import { mergeLive } from './quote'

const quote: Quote = {
  id: 'stock:AAPL', price: 100, change: 2, changePct: 2, open: null, high: null, low: null, prevClose: 98,
  volume: null, marketCap: null, currency: 'USD', marketState: 'regular',
  timestamp: '2026-10-11T10:00:00Z', source: 't', delaySecs: 0,
}
const tick = (price: number, ts: string): LiveTick => ({
  id: 'stock:AAPL', price, changePct: null, volume: null, ts, source: 't', seq: 1, dir: 'up',
})

describe('mergeLive', () => {
  it('sin tick usa la cotización REST', () => {
    expect(mergeLive(quote, undefined)).toMatchObject({ price: 100, live: false })
  })
  it('con tick más reciente usa su precio y recalcula la variación contra el cierre previo', () => {
    const m = mergeLive(quote, tick(103.9, '2026-10-11T10:00:05Z'))
    expect(m.price).toBe(103.9)
    expect(m.change).toBeCloseTo(5.9)
    expect(m.changePct).toBeCloseTo(6.0204, 3)
    expect(m.live).toBe(true)
  })
  it('un tick anterior a la cotización se descarta', () => {
    expect(mergeLive(quote, tick(90, '2026-10-11T09:59:00Z'))).toMatchObject({ price: 100, live: false })
  })
  it('sin cotización usa solo el tick', () => {
    expect(mergeLive(undefined, tick(5, '2026-10-11T10:00:00Z'))).toMatchObject({ price: 5, live: true })
  })
  it('sin nada devuelve nulls', () => {
    expect(mergeLive(null, undefined)).toEqual({ price: null, change: null, changePct: null, live: false })
  })
})
