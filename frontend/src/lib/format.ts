import type { AssetClass } from '@/api/types'

export const LOCALE = 'es-ES'

const nfCache = new Map<string, Intl.NumberFormat>()

function nf(key: string, opts: Intl.NumberFormatOptions): Intl.NumberFormat {
  let f = nfCache.get(key)
  if (!f) {
    f = new Intl.NumberFormat(LOCALE, { useGrouping: 'always', ...opts })
    nfCache.set(key, f)
  }
  return f
}

export const NBSP = ' '
const DASH = '—'

function isNum(v: number | null | undefined): v is number {
  return typeof v === 'number' && Number.isFinite(v)
}

/**
 * Decimales máximos según la magnitud del precio (y la clase): 2 para el grueso de activos,
 * más para divisas y para cripto o tokens de precio muy pequeño (hasta 8).
 */
export function priceDecimals(value: number, assetClass?: AssetClass): number {
  const abs = Math.abs(value)
  if (assetClass === 'forex') return abs >= 20 ? 2 : 4
  if (abs >= 1) return 2
  if (abs >= 0.1) return 4
  if (abs >= 0.01) return 5
  if (abs >= 0.0001) return 6
  return 8
}

export interface PriceOptions {
  class?: AssetClass | undefined
  /** Código ISO 4217; si se indica, se formatea como moneda. */
  currency?: string | null | undefined
}

/** Precio con decimales adaptativos. */
export function formatPrice(value: number | null | undefined, opts: PriceOptions = {}): string {
  if (!isNum(value)) return DASH
  const max = priceDecimals(value, opts.class)
  const min = Math.min(max, opts.class === 'forex' ? max : 2)
  if (opts.currency) return formatCurrency(value, opts.currency, { min, max })
  return nf(`p${min}-${max}`, { minimumFractionDigits: min, maximumFractionDigits: max }).format(value)
}

/** Importe en una moneda. Si el código no es válido para `Intl`, cae a «123,45 XXX». */
export function formatCurrency(
  value: number | null | undefined,
  currency: string | null | undefined,
  digits?: { min?: number; max?: number },
): string {
  if (!isNum(value)) return DASH
  const max = digits?.max ?? priceDecimals(value)
  const min = digits?.min ?? Math.min(2, max)
  if (!currency) return nf(`n${min}-${max}`, { minimumFractionDigits: min, maximumFractionDigits: max }).format(value)
  try {
    return nf(`c${currency}${min}-${max}`, {
      style: 'currency',
      currency,
      currencyDisplay: 'narrowSymbol',
      minimumFractionDigits: min,
      maximumFractionDigits: max,
    }).format(value)
  } catch {
    const n = nf(`n${min}-${max}`, { minimumFractionDigits: min, maximumFractionDigits: max }).format(value)
    return `${n}${NBSP}${currency}`
  }
}

/** Número genérico con `digits` decimales fijos. */
export function formatNumber(value: number | null | undefined, digits = 2): string {
  if (!isNum(value)) return DASH
  return nf(`n${digits}`, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value)
}

/** Número con signo explícito y coma decimal: «+0,4», «−0,3», «0,0». */
export function formatSigned(value: number | null | undefined, digits = 1): string {
  if (!isNum(value)) return DASH
  return nf(`sg${digits}`, { minimumFractionDigits: digits, maximumFractionDigits: digits, signDisplay: 'exceptZero' }).format(value)
}

export function formatInt(value: number | null | undefined): string {
  if (!isNum(value)) return DASH
  return nf('i', { maximumFractionDigits: 0 }).format(value)
}

/**
 * Cantidades grandes en escala larga española: `B` = billón (10^12), `mil M` = 10^9, `M` = 10^6.
 * 1,2 B · 125 mil M · 350 M · 12,3 mil.
 */
export function formatCompact(value: number | null | undefined, digits = 1): string {
  if (!isNum(value)) return DASH
  const abs = Math.abs(value)
  const scale = (div: number, suffix: string) => {
    const n = value / div
    // Con 3 o más cifras enteras los decimales sobran.
    const d = Math.abs(n) >= 100 ? 0 : digits
    return `${nf(`k${d}`, { minimumFractionDigits: 0, maximumFractionDigits: d }).format(n)}${NBSP}${suffix}`
  }
  if (abs >= 1e12) return scale(1e12, 'B')
  if (abs >= 1e9) return scale(1e9, 'mil M')
  if (abs >= 1e6) return scale(1e6, 'M')
  if (abs >= 1e4) return scale(1e3, 'mil')
  return nf('k0', { maximumFractionDigits: 0 }).format(value)
}

/** Importe grande con moneda: «3,4 B US$». */
export function formatCompactCurrency(
  value: number | null | undefined,
  currency: string | null | undefined,
): string {
  if (!isNum(value)) return DASH
  const base = formatCompact(value)
  if (!currency) return base
  let symbol = currency
  try {
    const parts = new Intl.NumberFormat(LOCALE, { style: 'currency', currency, currencyDisplay: 'narrowSymbol' })
    symbol = parts.formatToParts(0).find((p) => p.type === 'currency')?.value ?? currency
  } catch {
    /* código no válido: usamos el literal */
  }
  return `${base}${NBSP}${symbol}`
}

/** Porcentaje a partir de una FRACCIÓN (0,25 => «25,00 %»). */
export function formatPercent(fraction: number | null | undefined, digits = 2, signed = false): string {
  if (!isNum(fraction)) return DASH
  return nf(`pc${digits}${signed ? 's' : ''}`, {
    style: 'percent',
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
    signDisplay: signed ? 'exceptZero' : 'auto',
  }).format(fraction)
}

/** Porcentaje a partir de PUNTOS porcentuales (1,5 => «+1,50 %»). Es la unidad de `Quote.changePct`. */
export function formatPoints(points: number | null | undefined, digits = 2, signed = true): string {
  if (!isNum(points)) return DASH
  return formatPercent(points / 100, digits, signed)
}

/** Variación absoluta con signo. */
export function formatChange(value: number | null | undefined, assetClass?: AssetClass): string {
  if (!isNum(value)) return DASH
  const d = priceDecimals(value, assetClass)
  return nf(`ch${d}`, { minimumFractionDigits: Math.min(2, d), maximumFractionDigits: d, signDisplay: 'exceptZero' }).format(value)
}

export type Tone = 'up' | 'down' | 'flat'

export function toneOf(value: number | null | undefined): Tone {
  if (!isNum(value) || value === 0) return 'flat'
  return value > 0 ? 'up' : 'down'
}

// --- Fechas ----------------------------------------------------------------------------------

const rtf = new Intl.RelativeTimeFormat(LOCALE, { numeric: 'auto' })

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 86_400_000],
  ['month', 30 * 86_400_000],
  ['week', 7 * 86_400_000],
  ['day', 86_400_000],
  ['hour', 3_600_000],
  ['minute', 60_000],
  ['second', 1000],
]

/** «hace 5 minutos», «ayer», «dentro de 2 días». */
export function formatRelative(iso: string | number | Date | null | undefined, now: number = Date.now()): string {
  if (iso === null || iso === undefined) return DASH
  const ts = iso instanceof Date ? iso.getTime() : typeof iso === 'number' ? iso : Date.parse(iso)
  if (!Number.isFinite(ts)) return DASH
  const diff = ts - now
  if (Math.abs(diff) < 5000) return 'ahora mismo'
  for (const [unit, ms] of UNITS) {
    if (Math.abs(diff) >= ms || unit === 'second') {
      return rtf.format(Math.round(diff / ms), unit)
    }
  }
  return DASH
}

const dateFmt = new Intl.DateTimeFormat(LOCALE, { day: 'numeric', month: 'short', year: 'numeric' })
const dateTimeFmt = new Intl.DateTimeFormat(LOCALE, {
  day: 'numeric',
  month: 'short',
  year: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
})
const timeFmt = new Intl.DateTimeFormat(LOCALE, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

/** Acepta ISO 8601 o `YYYY-MM-DD` (interpretado como fecha local, sin desfase de zona). */
export function parseDate(value: string): Date {
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    const [y, m, d] = value.split('-').map(Number) as [number, number, number]
    return new Date(y, m - 1, d)
  }
  return new Date(value)
}

export function formatDate(value: string | null | undefined): string {
  if (!value) return DASH
  const d = parseDate(value)
  return Number.isNaN(d.getTime()) ? DASH : dateFmt.format(d)
}

export function formatDateTime(value: string | null | undefined): string {
  if (!value) return DASH
  const d = parseDate(value)
  return Number.isNaN(d.getTime()) ? DASH : dateTimeFmt.format(d)
}

export function formatTime(value: string | number | Date | null | undefined): string {
  if (value === null || value === undefined) return DASH
  const d = value instanceof Date ? value : new Date(value)
  return Number.isNaN(d.getTime()) ? DASH : timeFmt.format(d)
}

/** Retraso del dato: 0 = «Tiempo real», 900 = «Retraso de 15 min». */
export function formatDelay(secs: number | null | undefined): string {
  if (!isNum(secs)) return 'Retraso desconocido'
  if (secs <= 1) return 'Tiempo real'
  if (secs < 60) return `Retraso de ${Math.round(secs)} s`
  if (secs < 3600) return `Retraso de ${Math.round(secs / 60)} min`
  return `Retraso de ${Math.round(secs / 3600)} h`
}
