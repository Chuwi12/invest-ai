import { describe, expect, it } from 'vitest'
import {
  NBSP,
  formatChange,
  formatCompact,
  fundamentalsCurrency,
  formatCompactCurrency,
  currencyNote,
  currencySymbol,
  formatFull,
  formatCurrency,
  formatDate,
  formatDeclaredDelay,
  formatSpan,
  formatInt,
  formatPercent,
  formatPoints,
  formatPrice,
  formatRelative,
  formatSigned,
  priceDecimals,
  toneOf,
} from './format'

describe('priceDecimals', () => {
  it('usa 2 decimales a partir de 1', () => {
    expect(priceDecimals(187.3)).toBe(2)
    expect(priceDecimals(1)).toBe(2)
  })
  it('aumenta los decimales en precios pequeños, hasta 8', () => {
    expect(priceDecimals(0.5)).toBe(4)
    expect(priceDecimals(0.05)).toBe(5)
    expect(priceDecimals(0.001)).toBe(6)
    expect(priceDecimals(0.00001234)).toBe(8)
  })
  it('las divisas usan 4 decimales salvo pares con precio alto', () => {
    expect(priceDecimals(1.0842, 'forex')).toBe(4)
    expect(priceDecimals(151.2, 'forex')).toBe(2)
  })
})

describe('formatPrice', () => {
  it('agrupa miles y usa coma decimal', () => {
    expect(formatPrice(67234.5)).toBe('67.234,50')
    expect(formatPrice(1234.5)).toBe('1.234,50')
  })
  it('no trunca cripto de precio pequeño', () => {
    expect(formatPrice(0.00001234)).toBe('0,00001234')
    expect(formatPrice(0.0842)).toBe('0,0842')
  })
  it('divisas con 4 decimales', () => {
    expect(formatPrice(1.0842, { class: 'forex' })).toBe('1,0842')
  })
  it('con moneda usa símbolo estrecho', () => {
    expect(formatPrice(187.3, { currency: 'USD' })).toBe(`187,30${NBSP}$`)
    expect(formatPrice(42.1, { currency: 'EUR' })).toBe(`42,10${NBSP}€`)
  })
  it('devuelve un guion si no hay dato', () => {
    expect(formatPrice(null)).toBe('—')
    expect(formatPrice(Number.NaN)).toBe('—')
  })
})

describe('formatCurrency', () => {
  it('tolera códigos inválidos', () => {
    expect(formatCurrency(10, 'XX')).toContain('XX')
  })
})

describe('formatCompact', () => {
  it('escala larga española', () => {
    expect(formatCompact(1.2e12)).toBe(`1,2${NBSP}B`)
    expect(formatCompact(350e6)).toBe(`350${NBSP}M`)
    expect(formatCompact(1.5e9)).toBe(`1,5${NBSP}mil M`)
    expect(formatCompact(125e9)).toBe(`125${NBSP}mil M`)
    expect(formatCompact(12_300)).toBe(`12,3${NBSP}mil`)
  })
  it('números pequeños sin sufijo', () => {
    expect(formatCompact(950)).toBe('950')
  })
  it('negativos', () => {
    expect(formatCompact(-2.5e9)).toBe(`-2,5${NBSP}mil M`)
  })
  it('con moneda', () => {
    expect(formatCompactCurrency(3.4e12, 'USD')).toBe(`3,4${NBSP}B${NBSP}$`)
  })
  it('conserva 3 cifras significativas', () => {
    expect(formatCompact(4_967_129_153_536)).toBe(`4,97${NBSP}B`)
    expect(formatCompact(4_967_129_153_536 / 1e3)).toBe(`4,97${NBSP}mil M`)
    expect(formatCompact(1_234_567)).toBe(`1,23${NBSP}M`)
    expect(formatCompact(12_345_678)).toBe(`12,3${NBSP}M`)
    expect(formatCompact(123_456_789)).toBe(`123${NBSP}M`)
    expect(formatCompact(5e12)).toBe(`5${NBSP}B`)
    expect(formatCompact(1.2e12)).toBe(`1,2${NBSP}B`)
  })
  it('el redondeo que llega a 1000 sube de escala', () => {
    expect(formatCompact(999_600)).toBe(`1${NBSP}M`)
    expect(formatCompact(999.6e9)).toBe(`1${NBSP}B`)
  })
  it('con moneda USDT sale literal', () => {
    expect(formatCompactCurrency(4_967_129_153_536, 'USD')).toBe(`4,97${NBSP}B${NBSP}$`)
    expect(formatCompactCurrency(2.5e9, 'USDT')).toBe(`2,5${NBSP}mil M${NBSP}USDT`)
  })
  it('formatFull da el valor completo para el tooltip', () => {
    expect(formatFull(4_967_129_153_536, 'USD')).toBe(`4.967.129.153.536${NBSP}$`)
    expect(formatFull(200_000_000)).toBe('200.000.000')
  })
})

describe('porcentajes', () => {
  it('formatPercent parte de fracciones', () => {
    expect(formatPercent(0.2534)).toBe(`25,34${NBSP}%`)
    expect(formatPercent(0.05, 1, true)).toBe(`+5,0${NBSP}%`)
  })
  it('formatPoints parte de puntos porcentuales y siempre lleva signo', () => {
    expect(formatPoints(1.234)).toBe(`+1,23${NBSP}%`)
    expect(formatPoints(-0.5)).toBe(`-0,50${NBSP}%`)
    expect(formatPoints(0)).toBe(`0,00${NBSP}%`)
  })
})

describe('formatChange / toneOf', () => {
  it('signo explícito', () => {
    expect(formatChange(2.5)).toBe('+2,50')
    expect(formatChange(-0.0042)).toBe('-0,0042')
  })
  it('tono', () => {
    expect(toneOf(1)).toBe('up')
    expect(toneOf(-1)).toBe('down')
    expect(toneOf(0)).toBe('flat')
    expect(toneOf(null)).toBe('flat')
  })
})

describe('formatInt', () => {
  it('agrupa siempre, también 4 cifras', () => {
    expect(formatInt(1234)).toBe('1.234')
  })
})

describe('fechas', () => {
  const now = Date.parse('2026-10-11T12:00:00Z')
  it('relativa', () => {
    expect(formatRelative('2026-10-11T11:55:00Z', now)).toBe('hace 5 minutos')
    expect(formatRelative('2026-10-11T09:00:00Z', now)).toBe('hace 3 horas')
    expect(formatRelative('2026-10-10T12:00:00Z', now)).toBe('ayer')
    expect(formatRelative('2026-10-13T12:00:00Z', now)).toBe('pasado mañana')
    expect(formatRelative('2026-10-11T11:59:59Z', now)).toBe('ahora mismo')
  })
  it('fecha sin desfase de zona', () => {
    expect(formatDate('2026-12-18')).toMatch(/18/)
    expect(formatDate('2026-12-18')).toMatch(/dic/)
  })
  it('entradas inválidas', () => {
    expect(formatRelative('nope', now)).toBe('—')
    expect(formatDate(null)).toBe('—')
  })
})

describe('formatSpan / formatDeclaredDelay', () => {
  it('abrevia duraciones', () => {
    expect(formatSpan(2)).toBe('2 s')
    expect(formatSpan(15)).toBe('15 s')
    expect(formatSpan(900)).toBe('15 min')
    expect(formatSpan(7200)).toBe('2 h')
    expect(formatSpan(3 * 86_400)).toBe('3 d')
    expect(formatSpan(null)).toBe('—')
  })
  it('el retraso declarado solo existe si es > 0 y nunca dice «Retraso de» (no es una medida)', () => {
    expect(formatDeclaredDelay(15)).toBe('hasta 15 s')
    expect(formatDeclaredDelay(900)).toBe('hasta 15 min')
    expect(formatDeclaredDelay(0)).toBeNull()
    expect(formatDeclaredDelay(null)).toBeNull()
  })
})

describe('formatSigned', () => {
  it('usa coma decimal y signo explícito', () => {
    expect(formatSigned(0.4)).toBe('+0,4')
    expect(formatSigned(-0.35, 2)).toMatch(/^[-−]0,35$/)
    expect(formatSigned(0)).toBe('0,0')
    expect(formatSigned(null)).toBe('—')
  })
})

describe('moneda de la cotización', () => {
  it('USDT se rotula literal, nunca como $', () => {
    expect(formatPrice(67123.4, { currency: 'USDT' })).toBe(`67.123,40${NBSP}USDT`)
    expect(formatPrice(67123.4, { currency: 'USD' })).toBe(`67.123,40${NBSP}$`)
    expect(currencySymbol('USDT')).toBe('USDT')
  })
  it('nota de Tether solo para USDT', () => {
    expect(currencyNote('USDT')).toBe('Tether (stablecoin ≈ USD), cotización de Binance')
    expect(currencyNote('USD')).toBeUndefined()
    expect(currencyNote(null)).toBeUndefined()
  })
})

describe('fundamentalsCurrency', () => {
  it('usa la moneda propia de los fundamentales, aunque difiera de la del activo', () => {
    expect(fundamentalsCurrency({ currency: 'USD' }, 'EUR')).toMatchObject({ currency: 'USD', fallback: false, note: undefined, statements: null })
  })
  it('sin moneda propia cae a la del summary y lo indica en el tooltip', () => {
    const r = fundamentalsCurrency({ currency: null }, 'EUR')
    expect(r.currency).toBe('EUR')
    expect(r.fallback).toBe(true)
    expect(r.note).toMatch(/resumen del activo/)
    expect(fundamentalsCurrency({}, 'EUR').fallback).toBe(true)
  })
  it('estados financieros en otra moneda (ADR: cotiza en USD, publica en TWD)', () => {
    const r = fundamentalsCurrency({ currency: 'USD', extra: { financialCurrency: 'TWD' } }, 'USD')
    expect(r.currency).toBe('USD')
    expect(r.statements).toBe('TWD')
    expect(r.statementsNote).toMatch(/TWD/)
    expect(fundamentalsCurrency({ currency: 'USD', extra: { financialCurrency: 'USD' } }, 'USD').statements).toBeNull()
  })
  it('sin ninguna moneda no inventa una', () => {
    expect(fundamentalsCurrency({}, null)).toMatchObject({ currency: null, fallback: true, note: undefined })
  })
})
