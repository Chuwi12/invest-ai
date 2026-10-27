import { ApiError } from '@/api/client'
import type {
  Analysis,
  AssetId,
  CalibratedConfidence,
  HorizonReport,
  LearningHorizon,
  PerformanceReport,
  ReliabilityBin,
  VerdictStats,
  WatchlistResult,
  WatchlistRun,
  WatchlistStatus,
  WeightsResponse,
  WeightVersion,
} from '@/api/types'
import { VERDICTS } from '@/api/types'
import { genAnalysis } from './content'

/**
 * Rendimiento simulado. Por defecto, un historial «maduro» (14 meses de pasadas diarias) para ver
 * todos los elementos de la vista; con `?perf=empty` en la URL, el estado real de hoy: cero análisis.
 * Los números son inventados: solo sirven para ver la interfaz.
 */

const DAY = 86_400_000
const FACTORS = ['technical', 'momentum', 'valuation', 'fundamental', 'sentiment', 'macro', 'risk']
const WATCHLIST: AssetId[] = [
  'stock:AAPL', 'stock:MSFT', 'stock:NVDA', 'stock:AMZN', 'stock:TSLA', 'stock:JPM', 'stock:XOM', 'stock:JNJ', 'stock:KO',
  'stock:WMT', 'stock:CAT', 'stock:NEE', 'stock:ASML', 'stock:SAP', 'etf:QQQ', 'etf:IWM', 'etf:GLD', 'etf:TLT', 'crypto:ETH', 'crypto:SOL',
]

function isEmpty(): boolean {
  try {
    return new URLSearchParams(window.location.search).get('perf') === 'empty'
  } catch {
    return false
  }
}

/** Intervalo de Wilson al 95 %. */
function wilson(hits: number, n: number): [number, number] | null {
  if (n === 0) return null
  const z = 1.96
  const p = hits / n
  const d = 1 + (z * z) / n
  const c = p + (z * z) / (2 * n)
  const m = z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n))
  return [(c - m) / d, (c + m) / d]
}

function stats(verdict: string, n: number, rate: number, indShare: number, excess: number, ci: boolean): VerdictStats {
  const hits = Math.round(n * rate)
  const ind = Math.max(0, Math.round(n * indShare))
  const indHits = Math.round(ind * rate)
  return {
    verdict,
    n,
    hits,
    hitRate: n > 0 ? hits / n : null,
    hitRateCi: wilson(indHits, ind),
    meanExcess: n > 0 ? excess : null,
    meanExcessCi: ci && n > 0 ? [excess - 0.012 * Math.sqrt(200 / Math.max(n, 1)) - 0.004, excess + 0.012 * Math.sqrt(200 / Math.max(n, 1)) + 0.004] : null,
    clusters: ci ? 40 : 3,
  }
}

function reliability(n: number): ReliabilityBin[] {
  // Modelo sobreconfiado: declara 0,5–0,9 y acierta ~0,45–0,60.
  const rows: [number, number, number][] = [[0.5, 0.46, 0.2], [0.6, 0.5, 0.3], [0.7, 0.55, 0.3], [0.8, 0.58, 0.15], [0.9, 0.6, 0.05]]
  return rows.map(([lo, rate, share]) => {
    const k = Math.round(n * share)
    const ci = wilson(Math.round(k * rate), k) ?? [0, 1]
    return { lo, hi: lo + 0.1, n: k, meanConfidence: lo + 0.05, hitRate: rate, ciLow: ci[0], ciHigh: ci[1] }
  })
}

function horizon(h: LearningHorizon, total: number): HorizonReport {
  // Pasadas diarias de 20 activos; un análisis madura a los 7·h días (+1).
  const matured = Math.max(0, total - 20 * (7 * h + 1))
  const resolved = Math.round(matured * 0.97)
  const independent = Math.round(((matured / 20) / (7 * h)) * 20 * 0.95)
  const share = resolved > 0 ? independent / resolved : 0
  const rate = h === 1 ? 0.52 : h === 4 ? 0.54 : 0.5
  const by: VerdictStats[] =
    resolved === 0
      ? []
      : VERDICTS.map((v, i) => {
          const n = Math.round(resolved * [0.1, 0.34, 0.3, 0.12, 0.09, 0.05][i]!)
          const r = { buy: rate + 0.03, accumulate: rate + 0.02, hold: rate - 0.1, wait: rate - 0.12, reduce: rate - 0.02, avoid: rate - 0.04 }[v]
          const ex = { buy: 0.006, accumulate: 0.003, hold: 0.001, wait: -0.001, reduce: -0.002, avoid: -0.004 }[v] * h
          return stats(v, n, r, share, ex, independent / Math.max(1, h) >= 30)
        })
  const calibrationN = independent
  const applied = calibrationN >= 200
  return {
    horizonWeeks: h,
    analysesTotal: total,
    resolved,
    fallbackRows: 0,
    resolvedIndependent: independent,
    pending: Math.max(0, matured - resolved),
    notMatured: Math.min(total, 20 * (7 * h + 1)),
    skipped: 0,
    baseRateOutperform: resolved > 0 ? 0.49 : null,
    overall: resolved > 0 ? stats('all', resolved, rate, share, 0.0012 * h, independent / Math.max(1, h) >= 30) : stats('all', 0, 0, 0, 0, false),
    byVerdict: by,
    calibration: {
      horizonWeeks: h,
      n: calibrationN,
      minRequired: 200,
      applied,
      reason: applied
        ? 'aplicado: Brier fuera de muestra 0.0053 mejor'
        : calibrationN === 0
          ? 'sin análisis resueltos todavía: no hay nada que calibrar'
          : `muestra independiente insuficiente (${calibrationN} de 200): se mide la fiabilidad, pero la confianza no se corrige`,
      brierRaw: calibrationN > 0 ? 0.2602 : null,
      brierHoldoutRaw: applied ? 0.2531 : null,
      brierHoldoutCalibrated: applied ? 0.2478 : null,
      reliability: calibrationN > 0 ? reliability(calibrationN) : [],
    },
  }
}

const CAVEATS = [
  'Todos los activos comparten el mismo régimen de mercado: la muestra efectiva es mucho menor que el nº de análisis.',
  'Mantener/esperar acierta con una banda de ±2 %·√semanas: su tasa de acierto no es comparable con la de comprar/evitar sin mirar la tasa base.',
  'Los activos sin precio posterior (excluidos de bolsa) no entran: sesgo de supervivencia en el lado optimista.',
]

function mature(): WeightVersion[] {
  const now = Date.now()
  return [
    {
      version: 2, ruleVersion: 2, kind: 'learned', status: 'active', multipliers: [0.9, 1.6, 0.7, 1.1, 1, 0.8, 1.2], factorNames: FACTORS,
      createdAtMs: now - 38 * DAY, horizonWeeks: 4, trainedToMs: now - 120 * DAY, validatedFromMs: now - 118 * DAY, validatedToMs: now - 64 * DAY,
      nTrain: 212, nVal: 96, metrics: { decision: 'adopt', deltaIc: 0.041, pValue: 0.012, icCandidate: 0.067, icIncumbent: 0.026 },
      parentVersion: 1, note: 'adoptada tras validación posterior',
    },
    {
      version: 1, ruleVersion: 2, kind: 'learned', status: 'retired', multipliers: [1, 1.3, 0.9, 1, 1, 1, 1], factorNames: FACTORS,
      createdAtMs: now - 190 * DAY, horizonWeeks: 4, trainedToMs: now - 260 * DAY, validatedFromMs: now - 258 * DAY, validatedToMs: now - 200 * DAY,
      nTrain: 158, nVal: 84, metrics: { decision: 'adopt', deltaIc: 0.033, pValue: 0.021, icCandidate: 0.049, icIncumbent: 0.016 },
      parentVersion: null, note: null,
    },
  ]
}

let versions: WeightVersion[] | null = null
function getVersions(): WeightVersion[] {
  if (isEmpty()) return versions ?? []
  versions ??= mature()
  return versions
}

export function mockPerformance(): PerformanceReport {
  const empty = isEmpty()
  const total = empty ? 0 : 20 * 425
  const vs = getVersions()
  return {
    generatedAt: new Date().toISOString(),
    ruleVersion: 2,
    analysesTotal: total,
    horizons: ([1, 4, 12] as const).map((h) => horizon(h, total)),
    activeWeights: vs.find((v) => v.status === 'active') ?? null,
    weightHistory: vs,
    caveats: CAVEATS,
  }
}

export function mockWeights(): WeightsResponse {
  const vs = getVersions()
  return { active: vs.find((v) => v.status === 'active') ?? null, versions: vs }
}

export function mockRollback(version: number | null): WeightVersion {
  const vs = getVersions()
  const target = version === null ? null : vs.find((v) => v.version === version)
  if (version !== null && !target) throw new ApiError(404, 'not_found', `la versión de pesos ${version} no existe`)
  const current = vs.find((v) => v.status === 'active')
  for (const v of vs) v.status = 'retired'
  const next: WeightVersion = {
    version: Math.max(0, ...vs.map((v) => v.version)) + 1,
    ruleVersion: 2,
    kind: 'rollback',
    status: 'active',
    multipliers: target ? [...target.multipliers] : FACTORS.map(() => 1),
    factorNames: FACTORS,
    createdAtMs: Date.now(),
    horizonWeeks: null, trainedToMs: null, validatedFromMs: null, validatedToMs: null, nTrain: null, nVal: null,
    metrics: {},
    parentVersion: current?.version ?? null,
    note: version === null ? 'vuelta a los pesos por defecto' : `vuelta a la versión ${version}`,
  }
  versions = [next, ...vs]
  return next
}

export function mockCalibrated(confidence: number, horizon: number): CalibratedConfidence {
  const h = mockPerformance().horizons.find((x) => x.horizonWeeks === horizon) ?? mockPerformance().horizons[1]!
  const c = h.calibration
  return {
    raw: confidence,
    calibrated: c.applied ? Math.max(0.05, Math.min(0.95, 0.42 + 0.22 * confidence)) : null,
    horizonWeeks: h.horizonWeeks,
    n: c.n,
    minRequired: c.minRequired,
    reason: c.reason,
  }
}

export function mockWatchlist(): WatchlistStatus {
  const now = Date.now()
  const empty = isEmpty()
  const next = new Date()
  next.setHours(23, 0, 0, 0)
  if (next.getTime() <= now) next.setDate(next.getDate() + 1)
  const base = { enabled: true, defaultList: true, time: '23:00', depth: 'standard' as const, nextRun: next.toISOString(), running: false }
  if (empty) {
    return {
      ...base,
      assets: WATCHLIST.map((id) => ({ id, last: null, lastOkAt: null })),
      lastRun: null, lastRunResults: [], recentRuns: [], analysesTotal: 0,
    }
  }
  const yesterday = new Date(next.getTime() - DAY)
  const run = (id: number, daysAgo: number, over: Partial<WatchlistRun> = {}): WatchlistRun => {
    const at = yesterday.getTime() - daysAgo * DAY
    return {
      id, localDate: new Date(at).toISOString().slice(0, 10), scheduledFor: new Date(at).toISOString(), startedAt: new Date(at + 4_000).toISOString(),
      finishedAt: new Date(at + 52 * 60_000).toISOString(), trigger: 'schedule', status: 'completed', depth: 'standard', model: 'qwen2.5:7b',
      planned: 20, power: 'ac', powerWaitSecs: 0, ok: 20, failed: 0, ...over,
    }
  }
  const last = run(425, 0, { ok: 19, failed: 1 })
  const results: WatchlistResult[] = WATCHLIST.map((id, i) => {
    const started = new Date(yesterday.getTime() + 4_000 + i * 150_000)
    const fail = id === 'crypto:SOL'
    return {
      id: 8000 + i, runId: last.id, localDate: last.localDate, assetId: id, status: fail ? 'failed' : 'ok', transient: fail, attempts: fail ? 3 : 1,
      analysisId: fail ? null : `an-${i}`, verdict: fail ? null : (['accumulate', 'hold', 'buy', 'wait', 'reduce'] as const)[i % 5]!,
      errorCode: fail ? 'unavailable' : null, error: fail ? 'servicio no disponible: Binance no responde' : null,
      startedAt: started.toISOString(), finishedAt: new Date(started.getTime() + 140_000).toISOString(),
    }
  })
  return {
    ...base,
    assets: WATCHLIST.map((id, i) => ({ id, last: results[i]!, lastOkAt: results[i]!.status === 'ok' ? results[i]!.finishedAt : new Date(yesterday.getTime() - 2 * DAY).toISOString() })),
    lastRun: last,
    lastRunResults: results,
    recentRuns: [last, run(424, 1), run(423, 2, { trigger: 'recovery', startedAt: new Date(yesterday.getTime() - 2 * DAY + 3 * 3_600_000).toISOString(), power: 'battery', powerWaitSecs: 10_800 }), run(422, 3), run(421, 4, { status: 'aborted', ok: 6, failed: 14 }), run(420, 5), run(419, 6)],
    analysesTotal: 20 * 425 - 3,
  }
}

/** Último análisis de un activo de la lista: la pasada diaria los guarda todos, pero el mock solo siembra unos pocos. */
export function mockWatchlistAnalysis(id: AssetId): Analysis | null {
  const at = mockWatchlist().assets.find((a) => a.id === id)?.lastOkAt
  return at ? genAnalysis(id, 0, new Date(at)) : null
}
