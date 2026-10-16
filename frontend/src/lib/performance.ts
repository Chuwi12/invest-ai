import type { VerdictStats, WeightVersion } from '@/api/types'
import { formatPercent } from './format'

/** Por debajo de esto no se enseña ningún porcentaje: con n tan pequeño solo es ruido. */
export const MIN_SHOWN = 20

export function enoughData(s: Pick<VerdictStats, 'n'>): boolean {
  return s.n >= MIN_SHOWN
}

/** «52 % (44–60 %)»; sin intervalo, solo el valor; sin datos, «—». */
export function rateText(rate: number | null, ci: [number, number] | null): string {
  if (rate === null) return '—'
  const base = formatPercent(rate, 0)
  return ci ? `${base} (${formatPercent(ci[0], 0)} – ${formatPercent(ci[1], 0)})` : base
}

export function excessText(mean: number | null, ci: [number, number] | null): string {
  if (mean === null) return '—'
  const base = formatPercent(mean, 2, true)
  return ci ? `${base} (${formatPercent(ci[0], 2, true)} – ${formatPercent(ci[1], 2, true)})` : base
}

/** ¿El intervalo deja fuera el valor de referencia? (si no, no se distingue del azar). */
export function ciExcludes(ci: [number, number] | null, ref: number): boolean {
  return ci !== null && (ref < ci[0] || ref > ci[1])
}

/** Lectura honesta del acierto frente al azar de la tasa base. */
export function hitVerdict(s: VerdictStats, baseline: number): 'above' | 'below' | 'unclear' | 'none' {
  if (!enoughData(s) || s.hitRate === null) return 'none'
  if (!s.hitRateCi) return 'unclear'
  if (s.hitRateCi[0] > baseline) return 'above'
  if (s.hitRateCi[1] < baseline) return 'below'
  return 'unclear'
}

export interface GateNumbers {
  deltaIc: number | null
  pValue: number | null
  icCandidate: number | null
}

/** Cifras de la puerta de validación guardadas en `metrics`; lo que falte sale `null`. */
export function gateNumbers(v: WeightVersion): GateNumbers {
  const num = (k: string) => {
    const x = v.metrics[k]
    return typeof x === 'number' && Number.isFinite(x) ? x : null
  }
  return { deltaIc: num('deltaIc'), pValue: num('pValue'), icCandidate: num('icCandidate') }
}

export function isDefaultWeights(v: WeightVersion | null): boolean {
  return v === null || v.multipliers.every((m) => Math.abs(m - 1) < 1e-9)
}

export const FACTOR_LABEL: Record<string, string> = {
  technical: 'Técnico',
  momentum: 'Momentum',
  valuation: 'Valoración',
  fundamental: 'Fundamentales',
  fundamentals: 'Fundamentales',
  sentiment: 'Sentimiento',
  macro: 'Macro',
  risk: 'Riesgo',
}

export const KIND_LABEL: Record<WeightVersion['kind'], string> = {
  learned: 'Aprendida',
  rollback: 'Vuelta atrás',
  decay: 'Decaimiento',
  manual: 'Manual',
}
