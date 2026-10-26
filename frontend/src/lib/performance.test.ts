import { describe, expect, it } from 'vitest'
import type { VerdictStats, WeightVersion } from '@/api/types'
import { ciExcludes, enoughData, excessText, gateNumbers, hitVerdict, isDefaultWeights, rateText } from './performance'

const stats = (over: Partial<VerdictStats> = {}): VerdictStats => ({
  verdict: 'buy', n: 100, hits: 55, hitRate: 0.55, hitRateCi: [0.45, 0.65], meanExcess: 0.01, meanExcessCi: [-0.01, 0.03], clusters: 20, ...over,
})
const version = (over: Partial<WeightVersion> = {}): WeightVersion => ({
  version: 1, ruleVersion: 2, kind: 'learned', status: 'active', multipliers: [1, 1, 1, 1, 1, 1, 1], factorNames: [], createdAtMs: 0, horizonWeeks: 4,
  trainedToMs: null, validatedFromMs: null, validatedToMs: null, nTrain: null, nVal: null, metrics: {}, parentVersion: null, note: null, ...over,
})

describe('performance', () => {
  it('no enseña porcentajes con menos de 20 análisis', () => {
    expect(enoughData(stats({ n: 19 }))).toBe(false)
    expect(enoughData(stats({ n: 20 }))).toBe(true)
  })

  it('formatea acierto y exceso con su intervalo, o solo el valor si no hay', () => {
    expect(rateText(0.52, [0.44, 0.6])).toMatch(/52\s?% \(44\s?% – 60\s?%\)/)
    expect(rateText(0.52, null)).toMatch(/^52\s?%$/)
    expect(rateText(null, null)).toBe('—')
    expect(excessText(0.012, [-0.004, 0.028])).toMatch(/\+1,20\s?% \(−?0,40\s?% – \+2,80\s?%\)|\+1,20/)
    expect(excessText(null, null)).toBe('—')
  })

  it('compara el acierto con la tasa base solo si el intervalo la deja fuera', () => {
    expect(hitVerdict(stats({ hitRateCi: [0.52, 0.7] }), 0.5)).toBe('above')
    expect(hitVerdict(stats({ hitRateCi: [0.3, 0.48] }), 0.5)).toBe('below')
    expect(hitVerdict(stats({ hitRateCi: [0.4, 0.6] }), 0.5)).toBe('unclear')
    expect(hitVerdict(stats({ hitRateCi: null }), 0.5)).toBe('unclear')
    expect(hitVerdict(stats({ n: 5 }), 0.5)).toBe('none')
    expect(ciExcludes([-0.01, 0.02], 0)).toBe(false)
    expect(ciExcludes([0.01, 0.02], 0)).toBe(true)
    expect(ciExcludes(null, 0)).toBe(false)
  })

  it('lee las cifras de la puerta y detecta los pesos por defecto', () => {
    expect(gateNumbers(version({ metrics: { deltaIc: 0.04, pValue: 0.01, icCandidate: 'x' } }))).toEqual({ deltaIc: 0.04, pValue: 0.01, icCandidate: null })
    expect(gateNumbers(version())).toEqual({ deltaIc: null, pValue: null, icCandidate: null })
    expect(isDefaultWeights(null)).toBe(true)
    expect(isDefaultWeights(version())).toBe(true)
    expect(isDefaultWeights(version({ multipliers: [1, 1.5, 1, 1, 1, 1, 1] }))).toBe(false)
  })
})
