import { describe, expect, it } from 'vitest'
import type { Candle, Indicators } from '@/api/types'
import { applyTickToCandle, bollingerSeries, interpretIndicators, smaSeries } from './indicators'

const candles = (closes: number[]): Candle[] =>
  closes.map((c, i) => ({ t: 1000 + i * 86_400, o: c, h: c, l: c, c, v: 1 }))

describe('smaSeries', () => {
  it('calcula la media móvil y omite el calentamiento', () => {
    const s = smaSeries(candles([1, 2, 3, 4, 5]), 3)
    expect(s.map((p) => p.value)).toEqual([2, 3, 4])
    expect(s[0]?.time).toBe(1000 + 2 * 86_400)
  })
  it('devuelve vacío si no hay suficientes velas', () => {
    expect(smaSeries(candles([1, 2]), 3)).toEqual([])
  })
})

describe('bollingerSeries', () => {
  it('con precios constantes las bandas colapsan en la media', () => {
    const [p] = bollingerSeries(candles([10, 10, 10, 10]), 4)
    expect(p).toMatchObject({ upper: 10, middle: 10, lower: 10 })
  })
  it('usa desviación típica poblacional', () => {
    const [p] = bollingerSeries(candles([2, 4, 4, 4, 5, 5, 7, 9]), 8)
    expect(p?.middle).toBe(5)
    expect(p?.upper).toBe(9) // sd = 2
    expect(p?.lower).toBe(1)
  })
})

describe('applyTickToCandle', () => {
  const last: Candle = { t: 1000, o: 10, h: 11, l: 9, c: 10, v: 5 }
  it('actualiza cierre, máximo y mínimo dentro de la misma vela', () => {
    expect(applyTickToCandle(last, 12, 1100, 300)).toEqual({ ...last, h: 12, c: 12 })
    expect(applyTickToCandle(last, 8, 1100, 300)).toEqual({ ...last, l: 8, c: 8 })
  })
  it('abre una vela nueva al cruzar el intervalo intradía', () => {
    const next = applyTickToCandle(last, 12, 1000 + 300 * 2 + 10, 300)
    expect(next).toMatchObject({ t: 1600, o: 12, h: 12, l: 12, c: 12 })
  })
  it('las velas diarias o mayores siempre se actualizan', () => {
    expect(applyTickToCandle(last, 13, 1000 + 86_400 * 3, 86_400).t).toBe(1000)
  })
  it('sin velas previas crea una', () => {
    expect(applyTickToCandle(undefined, 5, 1234, 300)).toMatchObject({ t: 1200, c: 5 })
  })
})

describe('interpretIndicators', () => {
  const base: Indicators = {
    asOf: null, lastClose: 100, sma20: null, sma50: null, sma200: null, ema12: null, ema26: null,
    rsi14: null, macd: null, macdSignal: null, macdHist: null, bbUpper: null, bbMiddle: null, bbLower: null,
    atr14: null, volatility30d: null, maxDrawdown1y: null, return1w: null, return1m: null, return3m: null,
    returnYtd: null, return1y: null, high52w: null, low52w: null, distFromHigh52w: null, volumeAvg20: null,
    relativeVolume: null, trend: null, supports: [], resistances: [],
  }
  it('RSI: sobrecompra y sobreventa', () => {
    expect(interpretIndicators({ ...base, rsi14: 78 })[0]).toMatchObject({ tone: 'down' })
    expect(interpretIndicators({ ...base, rsi14: 78 })[0]?.text).toMatch(/Sobrecompra/)
    expect(interpretIndicators({ ...base, rsi14: 22 })[0]?.text).toMatch(/Sobreventa/)
    expect(interpretIndicators({ ...base, rsi14: 50 })[0]?.tone).toBe('flat')
  })
  it('MACD: signo del histograma', () => {
    const r = interpretIndicators({ ...base, macd: 2, macdSignal: 1, macdHist: 1 })
    expect(r[0]).toMatchObject({ key: 'macd', tone: 'up' })
    const b = interpretIndicators({ ...base, macd: -2, macdSignal: -1, macdHist: -1 })
    expect(b[0]).toMatchObject({ tone: 'down' })
  })
  it('cruce dorado / de la muerte', () => {
    const g = interpretIndicators({ ...base, sma50: 95, sma200: 90 }).find((r) => r.key === 'cross')
    expect(g?.value).toBe('Dorado')
    const d = interpretIndicators({ ...base, sma50: 85, sma200: 90 }).find((r) => r.key === 'cross')
    expect(d?.value).toBe('De la muerte')
  })
  it('omite lo que es null', () => {
    expect(interpretIndicators(base)).toEqual([])
  })
})
