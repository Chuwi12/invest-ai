import { describe, expect, it } from 'vitest'
import type { Scenario } from '@/api/types'
import { formatDurationMs, medianDuration, scenarioSlices, splitThesis } from './analysis'

const sc = (kind: Scenario['kind'], probability: number): Scenario => ({ kind, probability, description: kind, targetPrice: null, targetBasis: null, returnPct: null })

describe('scenarioSlices', () => {
  it('ordena alcista/base/bajista y suma exactamente 100', () => {
    const r = scenarioSlices([sc('bearish', 25), sc('bullish', 30), sc('base', 45)])!
    expect(r.slices.map((s) => s.scenario.kind)).toEqual(['bullish', 'base', 'bearish'])
    expect(r.slices.map((s) => s.pct)).toEqual([30, 45, 25])
    expect(r.normalized).toBe(false)
  })
  it('reparte el redondeo para que 33,3 + 33,3 + 33,4 dé 100', () => {
    const r = scenarioSlices([sc('bullish', 33.3), sc('base', 33.3), sc('bearish', 33.4)])!
    expect(r.slices.reduce((t, s) => t + s.pct, 0)).toBe(100)
  })
  it('reescala si el modelo no sumó 100 y lo indica', () => {
    const r = scenarioSlices([sc('bullish', 20), sc('base', 30), sc('bearish', 30)])!
    expect(r.normalized).toBe(true)
    expect(r.sum).toBe(80)
    expect(r.slices.reduce((t, s) => t + s.pct, 0)).toBe(100)
  })
  it('null, vacío, negativas o suma 0 -> sin barra', () => {
    expect(scenarioSlices(null)).toBeNull()
    expect(scenarioSlices(undefined)).toBeNull()
    expect(scenarioSlices([])).toBeNull()
    expect(scenarioSlices([sc('base', -10), sc('bullish', 110)])).toBeNull()
    expect(scenarioSlices([sc('base', 0), sc('bullish', 0)])).toBeNull()
  })
})

describe('medianDuration', () => {
  const a = (depth: 'quick' | 'standard' | 'deep', durationMs: number | null, model = 'qwen2.5:7b', id?: string) => ({ id: id ?? `${depth}${durationMs}${model}`, model, depth, durationMs })
  it('usa solo el mismo modelo y la misma profundidad', () => {
    const list = [a('standard', 50_000), a('standard', 60_000), a('standard', 40_000), a('deep', 80_000), a('standard', 5_000, 'llama3:8b')]
    expect(medianDuration(list, 'qwen2.5:7b', 'standard')).toEqual({ ms: 50_000, n: 3 })
    expect(medianDuration(list, 'qwen2.5:7b', 'deep')).toEqual({ ms: 80_000, n: 1 })
    expect(medianDuration(list, 'llama3:8b', 'standard')).toEqual({ ms: 5_000, n: 1 })
  })
  it('mediana par = media de los dos centrales', () => {
    expect(medianDuration([a('quick', 40_000), a('quick', 50_000)], 'qwen2.5:7b', 'quick')).toEqual({ ms: 45_000, n: 2 })
  })
  it('sin mediciones -> null (nunca un número inventado)', () => {
    expect(medianDuration([], 'qwen2.5:7b', 'quick')).toBeNull()
    expect(medianDuration([a('quick', 40_000)], 'qwen2.5:7b', 'deep')).toBeNull()
    expect(medianDuration([a('quick', null), { id: 'x', model: 'qwen2.5:7b' }, a('quick', 0)], 'qwen2.5:7b', 'quick')).toBeNull()
    expect(medianDuration([a('quick', 40_000)], undefined, 'quick')).toBeNull()
  })
  it('no cuenta dos veces el mismo análisis (histórico + panel)', () => {
    const x = a('quick', 40_000, 'qwen2.5:7b', 'same')
    expect(medianDuration([x, { ...x }, a('quick', 60_000)], 'qwen2.5:7b', 'quick')).toEqual({ ms: 50_000, n: 2 })
  })
})

describe('formatDurationMs', () => {
  it('formatea', () => {
    expect(formatDurationMs(48_400)).toBe('48 s')
    expect(formatDurationMs(65_000)).toBe('1 min 5 s')
    expect(formatDurationMs(120_000)).toBe('2 min')
    expect(formatDurationMs(300)).toBe('<1 s')
    expect(formatDurationMs(null)).toBe('—')
  })
})

describe('splitThesis', () => {
  const full = 'Párrafo uno.\n\nPárrafo dos.\n\nIncertidumbre: faltan flujos.\n\nLa conclusión cambiaría si:\n- A\n- B'
  it('con wouldChangeIf quita el bloque duplicado y separa la incertidumbre', () => {
    const r = splitThesis(full, true)
    expect(r.paragraphs).toEqual(['Párrafo uno.', 'Párrafo dos.'])
    expect(r.uncertainty).toBe('faltan flujos.')
    expect(r.changeBlockRemoved).toBe(true)
  })
  it('sin wouldChangeIf deja «La conclusión cambiaría si» en la tesis (es la única copia)', () => {
    const r = splitThesis(full, false)
    expect(r.paragraphs).toHaveLength(3)
    expect(r.paragraphs[2]).toMatch(/^La conclusión cambiaría si/)
  })
  it('funciona si el backend ya no incluye los apartados', () => {
    const r = splitThesis('Solo tesis.', true)
    expect(r).toEqual({ paragraphs: ['Solo tesis.'], uncertainty: null, changeBlockRemoved: false })
  })
  it('reconoce la variante en inglés', () => {
    const r = splitThesis('T.\n\nUncertainty: x.\n\nThe conclusion would change if:\n- y', true)
    expect(r.paragraphs).toEqual(['T.'])
    expect(r.uncertainty).toBe('x.')
  })
})
