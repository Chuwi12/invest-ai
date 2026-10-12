import type { Fundamentals, Indicators, MacroIndicator, Quote } from '@/api/types'

export interface Freshness {
  /** true = el dato es más antiguo de lo esperado para su tipo. */
  stale: boolean
  /** Edad en ms (null si no hay marca de tiempo válida). */
  ageMs: number | null
  /** Motivo legible cuando `stale` es true. */
  reason: string | null
}

const SEC = 1000
const HOUR = 3_600_000
const DAY = 24 * HOUR

function age(iso: string | null | undefined, now: number): number | null {
  if (!iso) return null
  const t = /^\d{4}-\d{2}-\d{2}$/.test(iso) ? Date.parse(`${iso}T00:00:00Z`) : Date.parse(iso)
  return Number.isFinite(t) ? Math.max(0, now - t) : null
}

/** Umbral genérico: «retrasado» si supera `maxAgeMs` (o si no hay fecha). */
export function freshness(iso: string | null | undefined, maxAgeMs: number, now = Date.now()): Freshness {
  const ageMs = age(iso, now)
  if (ageMs === null) return { stale: true, ageMs, reason: 'sin marca de tiempo' }
  return ageMs > maxAgeMs ? { stale: true, ageMs, reason: 'más antiguo de lo esperado' } : { stale: false, ageMs, reason: null }
}

/**
 * Una cotización está retrasada si el proveedor declara un retraso > 60 s, o si el mercado
 * está abierto y la marca de tiempo tiene más de 2 minutos. Con el mercado cerrado un dato
 * antiguo es lo normal, así que no se marca.
 */
export function quoteFreshness(q: Quote | null | undefined, now = Date.now()): Freshness {
  if (!q) return { stale: false, ageMs: null, reason: null }
  const ageMs = age(q.timestamp, now)
  if (q.delaySecs !== null && q.delaySecs > 60) {
    return { stale: true, ageMs, reason: `el proveedor declara un retraso de ${Math.round(q.delaySecs / 60)} min` }
  }
  const open = q.marketState === 'regular' || q.marketState === 'continuous'
  if (open && ageMs !== null && ageMs > 120 * SEC) {
    return { stale: true, ageMs, reason: 'el mercado está abierto pero el último dato tiene más de 2 min' }
  }
  return { stale: false, ageMs, reason: null }
}

export const fundamentalsFreshness = (f: Fundamentals | null | undefined, now = Date.now()): Freshness =>
  freshness(f?.asOf, 7 * DAY, now)

/** Los indicadores se calculan sobre velas diarias: hasta 4 días cubre fines de semana y festivos. */
export const indicatorsFreshness = (i: Indicators | null | undefined, now = Date.now()): Freshness =>
  freshness(i?.asOf, 4 * DAY, now)

export const macroFreshness = (m: MacroIndicator, now = Date.now()): Freshness => freshness(m.asOf, 45 * DAY, now)

/** Un análisis de más de 24 h puede no reflejar el mercado actual. */
export const analysisFreshness = (generatedAt: string, now = Date.now()): Freshness =>
  freshness(generatedAt, DAY, now)
