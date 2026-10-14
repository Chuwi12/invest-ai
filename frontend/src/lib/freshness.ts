import type { Fundamentals, Indicators, MacroIndicator, Quote } from '@/api/types'
import { formatDeclaredDelay, formatSpan } from './format'

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

/** Margen sobre el retraso declarado antes de dar por «retrasado» un dato con la sesión abierta. */
export const QUOTE_MARGIN_MS = 120 * SEC

/** Por encima de este retraso declarado, un dato que no se puede medir se marca «retrasado». */
const DECLARED_STALE_SECS = 60

export interface QuoteFreshness extends Freshness {
  /**
   * true = `ageMs` es una MEDIDA: hora de la cotización que dio el proveedor (`timestampOrigin = provider`)
   * frente al reloj de este equipo. false = el proveedor no dio hora (`local`): `ageMs` es null porque la hora
   * guardada es la de la consulta y no dice nada de la antigüedad del dato.
   */
  measured: boolean
  /** Retraso que DECLARA el proveedor, en segundos. Solo informativo; null = no declara ninguno. */
  declaredDelaySecs: number | null
}

/**
 * Frescura de una cotización. Lo que manda es la antigüedad MEDIDA (`ahora − timestamp`, solo cuando la hora es
 * del proveedor); el retraso que declara el proveedor (`delaySecs`) es información secundaria y nunca se presenta
 * como si fuese la medida (Yahoo declara 15 en AAPL/MSFT aunque sirva datos de 1-2 s: ver `docs/notes/market.md`).
 *
 * - Sesión abierta (`regular`/`continuous`) y antigüedad medida mayor que el retraso declarado + 2 min de margen:
 *   «retrasado».
 * - Sin hora del proveedor no hay medida; solo queda la declaración: se marca «retrasado» si declara más de 60 s.
 * - Con el mercado cerrado/pre/post un dato antiguo es lo normal: no se marca.
 */
export function quoteFreshness(q: Quote | null | undefined, now = Date.now()): QuoteFreshness {
  if (!q) return { stale: false, ageMs: null, reason: null, measured: false, declaredDelaySecs: null }
  const declaredDelaySecs = q.delaySecs !== null && Number.isFinite(q.delaySecs) ? q.delaySecs : null
  const ageMs = q.timestampOrigin === 'provider' ? age(q.timestamp, now) : null
  const measured = ageMs !== null
  const base = { ageMs, measured, declaredDelaySecs }
  const open = q.marketState === 'regular' || q.marketState === 'continuous'
  if (!open) return { ...base, stale: false, reason: null }

  if (measured) {
    const limitMs = (declaredDelaySecs ?? 0) * SEC + QUOTE_MARGIN_MS
    if (ageMs > limitMs) {
      const declared = formatDeclaredDelay(declaredDelaySecs)
      return {
        ...base,
        stale: true,
        reason:
          `el mercado está abierto y el último dato tiene ${formatSpan(ageMs / SEC)}, más de lo que cabe esperar ` +
          `(${declared ? `el proveedor declara ${declared}` : 'el proveedor no declara retraso'} + ${formatSpan(QUOTE_MARGIN_MS / SEC)} de margen)`,
      }
    }
    return { ...base, stale: false, reason: null }
  }
  if (declaredDelaySecs !== null && declaredDelaySecs > DECLARED_STALE_SECS) {
    return {
      ...base,
      stale: true,
      reason: `el proveedor declara un retraso de ${formatDeclaredDelay(declaredDelaySecs)} y no dio la hora del dato, así que no se puede medir`,
    }
  }
  return { ...base, stale: false, reason: null }
}

/**
 * Texto de la nota secundaria con el retraso declarado (tooltip). null si el proveedor no declara retraso.
 * Sin hora del proveedor se avisa de que no se ha podido contrastar con la antigüedad.
 */
export function declaredDelayNote(f: Pick<QuoteFreshness, 'declaredDelaySecs' | 'measured'>): string | null {
  const declared = formatDeclaredDelay(f.declaredDelaySecs)
  if (!declared) return null
  const note = `El proveedor declara un retraso de ${declared} para esta bolsa`
  return f.measured ? note : `${note}; no dio la hora del dato, así que no se ha podido medir`
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
