import { describe, expect, it } from 'vitest'
import type { Quote } from '@/api/types'
import { analysisFreshness, freshness, quoteFreshness } from './freshness'

const NOW = Date.parse('2026-01-10T12:00:00Z')
const q = (over: Partial<Quote>): Quote => ({
  id: 'stock:AAPL', price: 1, change: 0, changePct: 0, open: 1, high: 1, low: 1, prevClose: 1, volume: 1, marketCap: null,
  currency: 'USD', marketState: 'regular', timestamp: '2026-01-10T11:59:50Z', source: 'yahoo', delaySecs: 0, ...over,
})

describe('freshness', () => {
  it('marca como retrasado un dato con retraso declarado > 60 s', () => {
    expect(quoteFreshness(q({ delaySecs: 900 }), NOW).stale).toBe(true)
  })
  it('marca retrasado un dato viejo con el mercado abierto, pero no con el mercado cerrado', () => {
    const old = '2026-01-10T11:50:00Z'
    expect(quoteFreshness(q({ timestamp: old }), NOW).stale).toBe(true)
    expect(quoteFreshness(q({ timestamp: old, marketState: 'closed' }), NOW).stale).toBe(false)
  })
  it('un dato reciente en tiempo real no es retrasado', () => {
    expect(quoteFreshness(q({}), NOW).stale).toBe(false)
  })
  it('sin fecha cuenta como retrasado; YYYY-MM-DD se interpreta', () => {
    expect(freshness(null, 1000, NOW).stale).toBe(true)
    expect(freshness('2026-01-09', 3 * 86_400_000, NOW).stale).toBe(false)
  })
  it('un análisis de más de 24 h está desfasado', () => {
    expect(analysisFreshness('2026-01-08T12:00:00Z', NOW).stale).toBe(true)
    expect(analysisFreshness('2026-01-10T06:00:00Z', NOW).stale).toBe(false)
  })
})
