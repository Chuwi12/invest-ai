import type { Analysis, AnalysisDepth, Scenario, ScenarioKind } from '@/api/types'

/** Orden fijo de lectura: de más a menos favorable. */
export const SCENARIO_ORDER: readonly ScenarioKind[] = ['bullish', 'base', 'bearish']

export const SCENARIO_LABEL: Record<ScenarioKind, string> = { bullish: 'Alcista', base: 'Base', bearish: 'Bajista' }

export interface ScenarioSlice {
  scenario: Scenario
  /** Entero 0–100; la suma de todas las porciones es exactamente 100 (método del mayor resto). */
  pct: number
}

/**
 * Prepara las porciones de la barra apilada. Las probabilidades del backend deberían sumar 100;
 * si no (modelo, redondeos, escenario descartado) se reescalan para que la barra sea siempre del 100 %
 * y se avisa con `normalized`. Con probabilidades inválidas (negativas, NaN o suma 0) no se pinta barra.
 */
export function scenarioSlices(scenarios: Scenario[] | null | undefined): { slices: ScenarioSlice[]; normalized: boolean; sum: number } | null {
  if (!scenarios || scenarios.length === 0) return null
  const ordered = [...scenarios].sort((a, b) => SCENARIO_ORDER.indexOf(a.kind) - SCENARIO_ORDER.indexOf(b.kind))
  if (ordered.some((s) => !Number.isFinite(s.probability) || s.probability < 0)) return null
  const sum = ordered.reduce((t, s) => t + s.probability, 0)
  if (sum <= 0) return null
  const exact = ordered.map((s) => (s.probability / sum) * 100)
  const floor = exact.map(Math.floor)
  let rest = 100 - floor.reduce((t, v) => t + v, 0)
  const byRemainder = exact.map((v, i) => ({ i, r: v - Math.floor(v) })).sort((a, b) => b.r - a.r || a.i - b.i)
  for (const { i } of byRemainder) {
    if (rest <= 0) break
    floor[i]! += 1
    rest -= 1
  }
  return {
    slices: ordered.map((scenario, i) => ({ scenario, pct: floor[i]! })),
    normalized: Math.abs(sum - 100) > 0.5,
    sum,
  }
}

const SECTION_UNCERTAINTY = /^(incertidumbre|uncertainty)\s*:\s*/i
const SECTION_CHANGE = /^(la conclusión cambiaría si|the conclusion would change if)\s*:/i

export interface SplitThesis {
  /** Párrafos de la tesis sin los apartados duplicados. */
  paragraphs: string[]
  /** Texto de «Incertidumbre», si la tesis lo incluye. */
  uncertainty: string | null
  /** El apartado «La conclusión cambiaría si» tal como venía (solo se retira si hay `wouldChangeIf`). */
  changeBlockRemoved: boolean
}

/**
 * Separa de la tesis los apartados «Incertidumbre» y «La conclusión cambiaría si». El segundo se retira solo si
 * el análisis trae `wouldChangeIf` (si no, es la única copia y se deja en la prosa). «Incertidumbre» no tiene
 * campo propio: se devuelve aparte para pintarla como nota diferenciada, sin repetirla en el cuerpo.
 * Funciona igual si el backend ya no los incluye en la tesis.
 */
export function splitThesis(thesis: string, hasWouldChangeIf: boolean): SplitThesis {
  const paragraphs: string[] = []
  let uncertainty: string | null = null
  let changeBlockRemoved = false
  for (const raw of thesis.split(/\n{2,}/)) {
    const p = raw.trim()
    if (!p) continue
    const u = SECTION_UNCERTAINTY.exec(p)
    if (u) {
      uncertainty = p.slice(u[0].length).trim() || null
      continue
    }
    if (SECTION_CHANGE.test(p) && hasWouldChangeIf) {
      changeBlockRemoved = true
      continue
    }
    paragraphs.push(p)
  }
  return { paragraphs, uncertainty, changeBlockRemoved }
}

export interface DurationSample {
  model: string
  depth?: AnalysisDepth | null | undefined
  durationMs?: number | null | undefined
}

export interface MedianDuration {
  ms: number
  /** Mediciones usadas. */
  n: number
}

function median(values: number[]): number {
  const v = [...values].sort((a, b) => a - b)
  const mid = v.length >> 1
  return v.length % 2 ? v[mid]! : (v[mid - 1]! + v[mid]!) / 2
}

/**
 * Mediana de `durationMs` de análisis previos con el MISMO modelo y la misma profundidad. Ignora análisis sin
 * medición (antiguos) y duraciones no positivas. Sin mediciones devuelve `null`: nunca se inventa un número.
 * Un mismo análisis puede llegar por dos vías (histórico y panel): se deduplica por `id` si se pasa.
 */
export function medianDuration(
  analyses: readonly (DurationSample & { id?: string })[],
  model: string | null | undefined,
  depth: AnalysisDepth,
): MedianDuration | null {
  if (!model) return null
  const seen = new Set<string>()
  const values: number[] = []
  for (const a of analyses) {
    if (a.id) {
      if (seen.has(a.id)) continue
      seen.add(a.id)
    }
    if (a.model !== model || a.depth !== depth) continue
    if (typeof a.durationMs !== 'number' || !Number.isFinite(a.durationMs) || a.durationMs <= 0) continue
    values.push(a.durationMs)
  }
  return values.length ? { ms: median(values), n: values.length } : null
}

/** «48 s», «1 min 5 s», «2 min». Redondea al segundo; por debajo de 1 s, «<1 s». */
export function formatDurationMs(ms: number | null | undefined): string {
  if (typeof ms !== 'number' || !Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1000) return '<1 s'
  const total = Math.round(ms / 1000)
  if (total < 60) return `${total} s`
  const m = Math.floor(total / 60)
  const s = total % 60
  return s ? `${m} min ${s} s` : `${m} min`
}

export type AnalysisLike = Pick<Analysis, 'id' | 'model' | 'depth' | 'durationMs'>
