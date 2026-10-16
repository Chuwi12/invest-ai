import { describe, expect, it } from 'vitest'
import type { IpoEntry } from '@/api/types'
import { canHavePerformance, isSimulated, priceText, profileCounts, sizeText, splitCalendar, STATUS_LABEL } from './ipo'

const base: IpoEntry = {
  date: '2026-10-09',
  symbol: 'TRXB',
  name: 'TRex Bio, Inc.',
  exchange: 'NASDAQ Global Select',
  status: 'priced',
  priceLow: 14,
  priceHigh: 14,
  shares: 8_333_334,
  totalValue: 116_666_676,
  spacLikely: false,
  source: 'finnhub',
  sourceUrl: null,
  asOf: '2026-10-15T00:00:00Z',
}
const mk = (o: Partial<IpoEntry>): IpoEntry => ({ ...base, ...o })

describe('splitCalendar', () => {
  it('separa próximas (previstas primero, por fecha) de recientes (la más nueva primero)', () => {
    const g = splitCalendar([
      mk({ symbol: 'A', status: 'filed', date: '2026-10-01' }),
      mk({ symbol: 'B', status: 'expected', date: '2026-11-20' }),
      mk({ symbol: 'C', status: 'expected', date: '2026-11-05' }),
      mk({ symbol: 'D', status: 'priced', date: '2026-09-18' }),
      mk({ symbol: 'E', status: 'priced', date: '2026-10-09' }),
      mk({ symbol: 'F', status: 'withdrawn', date: '2026-09-30' }),
    ])
    expect(g.upcoming.map((e) => e.symbol)).toEqual(['C', 'B', 'A'])
    expect(g.recent.map((e) => e.symbol)).toEqual(['E', 'F', 'D'])
  })

  it('no pierde ninguna fila y no muta la entrada', () => {
    const rows = [mk({ status: 'unknown' }), mk({ status: 'priced' })]
    const g = splitCalendar(rows)
    expect(g.upcoming.length + g.recent.length).toBe(2)
    expect(rows[0]?.status).toBe('unknown')
  })
})

describe('textos de la ficha', () => {
  it('precio: fijo, rango o guion', () => {
    expect(priceText(mk({}))).toBe('14,00 USD')
    expect(priceText(mk({ priceLow: 18, priceHigh: 20 }))).toBe('18,00–20,00 USD')
    expect(priceText(mk({ priceLow: null, priceHigh: null }))).toBe('—')
  })

  it('tamaño: «hasta» si aún no hay precio definitivo', () => {
    expect(sizeText(mk({ status: 'expected', totalValue: 460_000_000 }))).toMatch(/^hasta /)
    expect(sizeText(mk({ totalValue: null }))).toBe('—')
    expect(sizeText(mk({}))).not.toMatch(/hasta/)
  })

  it('cada estado tiene etiqueta en español', () => {
    expect(Object.values(STATUS_LABEL).every((l) => l.length > 3)).toBe(true)
  })

  it('«simulado» solo si la fuente es fake', () => {
    expect(isSimulated('fake')).toBe(true)
    expect(isSimulated('finnhub')).toBe(false)
    expect(isSimulated(null)).toBe(false)
  })
})

describe('rendimiento posterior', () => {
  it('solo con precio fijado y primer día ya pasado', () => {
    expect(canHavePerformance(mk({}), '2026-10-16')).toBe(true)
    expect(canHavePerformance(mk({ date: '2026-10-20' }), '2026-10-16')).toBe(false)
    expect(canHavePerformance(mk({ status: 'expected' }), '2026-10-16')).toBe(false)
    expect(canHavePerformance(null, '2026-10-16')).toBe(false)
  })

  it('cuenta los indicadores leídos', () => {
    const i = (status: 'extracted' | 'not_extracted') => ({ status }) as never
    expect(profileCounts({ indicators: [i('extracted'), i('not_extracted'), i('extracted')] })).toEqual({ extracted: 2, total: 3 })
  })
})
