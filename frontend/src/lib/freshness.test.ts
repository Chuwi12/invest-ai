import { describe, expect, it } from 'vitest'
import type { Quote } from '@/api/types'
import { QUOTE_MARGIN_MS, analysisFreshness, declaredDelayNote, freshness, quoteFreshness } from './freshness'

const NOW = Date.parse('2026-01-10T12:00:00Z')
const q = (over: Partial<Quote>): Quote => ({
  id: 'stock:AAPL', price: 1, change: 0, changePct: 0, open: 1, high: 1, low: 1, prevClose: 1, volume: 1, marketCap: null,
  currency: 'USD', marketState: 'regular', timestamp: '2026-01-10T11:59:50Z', source: 'yahoo', delaySecs: 0, timestampOrigin: 'provider', ...over,
})
/** Cotización con la hora del proveedor `secs` segundos antes de NOW. */
const aged = (secs: number, over: Partial<Quote> = {}): Quote => q({ timestamp: new Date(NOW - secs * 1000).toISOString(), ...over })

describe('freshness', () => {
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

describe('quoteFreshness: antigüedad medida frente a retraso declarado', () => {
  it('medida < declarada (el caso de AAPL en Yahoo: declara 15, llega con 2 s): no es retrasado y la medida manda', () => {
    const f = quoteFreshness(aged(2, { delaySecs: 15 }), NOW)
    expect(f).toMatchObject({ stale: false, reason: null, ageMs: 2000, measured: true, declaredDelaySecs: 15 })
  })
  it('un retraso declarado grande no marca «retrasado» por sí solo si lo medido es reciente', () => {
    expect(quoteFreshness(aged(2, { delaySecs: 900 }), NOW).stale).toBe(false)
  })
  it('medida > declarada + margen: retrasado, con el motivo contando la medida y la declaración', () => {
    const f = quoteFreshness(aged(300, { delaySecs: 15 }), NOW)
    expect(f.stale).toBe(true)
    expect(f.ageMs).toBe(300_000)
    expect(f.reason).toContain('5 min')
    expect(f.reason).toContain('hasta 15 s')
  })
  it('el umbral es declarado + margen (2 min): en el límite no, un segundo después sí', () => {
    const limit = 15 + QUOTE_MARGIN_MS / 1000
    expect(quoteFreshness(aged(limit, { delaySecs: 15 }), NOW).stale).toBe(false)
    expect(quoteFreshness(aged(limit + 1, { delaySecs: 15 }), NOW).stale).toBe(true)
  })
  it('sin retraso declarado (0 o null) se tolera solo el margen', () => {
    expect(quoteFreshness(aged(100, { delaySecs: 0 }), NOW).stale).toBe(false)
    expect(quoteFreshness(aged(130, { delaySecs: 0 }), NOW).stale).toBe(true)
    expect(quoteFreshness(aged(130, { delaySecs: null }), NOW).stale).toBe(true)
    expect(quoteFreshness(aged(130, { delaySecs: null }), NOW).reason).toContain('no declara retraso')
  })
  it('un feed realmente diferido (declara 15 min y llega con ~15 min) no es «retrasado» respecto a lo declarado', () => {
    expect(quoteFreshness(aged(14 * 60, { delaySecs: 900 }), NOW).stale).toBe(false)
    expect(quoteFreshness(aged(18 * 60, { delaySecs: 900 }), NOW).stale).toBe(true)
  })
  it('cripto (mercado continuo) se evalúa como sesión abierta', () => {
    expect(quoteFreshness(aged(600, { marketState: 'continuous' }), NOW).stale).toBe(true)
  })

  it('sin hora del proveedor (origen local) no hay medida: ageMs null y la antigüedad no cuenta', () => {
    const f = quoteFreshness(aged(3600, { timestampOrigin: 'local', delaySecs: 15 }), NOW)
    expect(f).toMatchObject({ stale: false, ageMs: null, measured: false, declaredDelaySecs: 15 })
  })
  it('sin hora del proveedor solo queda la declaración: > 60 s se marca (con el motivo diciendo que no se pudo medir)', () => {
    const f = quoteFreshness(aged(0, { timestampOrigin: 'local', delaySecs: 900 }), NOW)
    expect(f.stale).toBe(true)
    expect(f.reason).toContain('hasta 15 min')
    expect(f.reason).toContain('no se puede medir')
  })
  it('origen desconocido (null/ausente) o fecha ilegible tampoco se presentan como medida', () => {
    expect(quoteFreshness(aged(2, { timestampOrigin: null }), NOW).measured).toBe(false)
    expect(quoteFreshness(q({ timestampOrigin: undefined }), NOW).measured).toBe(false)
    expect(quoteFreshness(q({ timestamp: 'no-es-fecha' }), NOW)).toMatchObject({ measured: false, ageMs: null, stale: false })
  })

  it('mercado cerrado/pre/post: un dato antiguo es lo normal, aunque haya retraso declarado o no se pueda medir', () => {
    for (const marketState of ['closed', 'pre', 'post'] as const) {
      expect(quoteFreshness(aged(3 * 3600, { marketState, delaySecs: 900 }), NOW)).toMatchObject({ stale: false, reason: null, measured: true })
      expect(quoteFreshness(aged(3 * 3600, { marketState, delaySecs: 900, timestampOrigin: 'local' }), NOW).stale).toBe(false)
    }
  })
  it('mercado cerrado: la medida sigue disponible (es la que se muestra), pero no marca nada', () => {
    const f = quoteFreshness(aged(2 * 86_400, { marketState: 'closed' }), NOW)
    expect(f.ageMs).toBe(2 * 86_400_000)
    expect(f.stale).toBe(false)
  })
  it('sin cotización', () => {
    expect(quoteFreshness(null, NOW)).toEqual({ stale: false, ageMs: null, reason: null, measured: false, declaredDelaySecs: null })
  })
})

describe('declaredDelayNote', () => {
  it('es una nota sobre lo que declara el proveedor, no una medida', () => {
    expect(declaredDelayNote({ declaredDelaySecs: 15, measured: true })).toBe('El proveedor declara un retraso de hasta 15 s para esta bolsa')
    expect(declaredDelayNote({ declaredDelaySecs: 900, measured: true })).toBe('El proveedor declara un retraso de hasta 15 min para esta bolsa')
  })
  it('sin hora del proveedor avisa de que no se ha podido medir', () => {
    expect(declaredDelayNote({ declaredDelaySecs: 15, measured: false })).toContain('no se ha podido medir')
  })
  it('no hay nota si el proveedor no declara retraso', () => {
    expect(declaredDelayNote({ declaredDelaySecs: 0, measured: true })).toBeNull()
    expect(declaredDelayNote({ declaredDelaySecs: null, measured: true })).toBeNull()
  })
})
