import { AlertTriangle, CalendarClock, Info } from 'lucide-react'
import { useState } from 'react'
import { usePerformance, useRollbackWeights, useWatchlist } from '@/api/hooks'
import type { HorizonReport, LearningHorizon, PerformanceReport, VerdictStats, WatchlistStatus, WeightVersion } from '@/api/types'
import { ReliabilityChart } from '@/components/ReliabilityChart'
import { Empty, ErrorNotice, Section, SkeletonRows } from '@/components/states'
import { useTitle } from '@/hooks/useTitle'
import { symbolOf, VERDICT_LABEL } from '@/lib/assets'
import { errorText } from '@/lib/errors'
import { formatDateTime, formatInt, formatPercent, formatRelative, formatSpan } from '@/lib/format'
import {
  ciExcludes,
  enoughData,
  excessText,
  FACTOR_LABEL,
  gateNumbers,
  hitVerdict,
  isDefaultWeights,
  KIND_LABEL,
  MIN_SHOWN,
  rateText,
} from '@/lib/performance'

const HORIZONS: readonly LearningHorizon[] = [1, 4, 12]
const weeks = (h: number) => `${h} ${h === 1 ? 'semana' : 'semanas'}`
const verdictName = (v: string) => (v === 'all' ? 'Todos' : (VERDICT_LABEL as Record<string, string>)[v] ?? v)

function HorizonCards({ report }: { report: PerformanceReport }) {
  return (
    <div className="grid cols-3">
      {report.horizons.map((h) => (
        <article key={h.horizonWeeks} className="panel stat-card" aria-label={`Horizonte de ${weeks(h.horizonWeeks)}`}>
          <h3>{weeks(h.horizonWeeks)}</h3>
          <dl className="stat-grid">
            <div><dt>Resueltos</dt><dd className="num">{formatInt(h.resolved)}</dd></div>
            <div><dt>Pendientes</dt><dd className="num">{formatInt(h.pending)}</dd></div>
            <div><dt>Sin madurar</dt><dd className="num">{formatInt(h.notMatured)}</dd></div>
          </dl>
          <p className="small muted">
            {h.resolved === 0
              ? h.notMatured > 0
                ? `Ningún análisis ha cumplido aún ${weeks(h.horizonWeeks)}.`
                : 'Sin análisis todavía.'
              : `${formatInt(h.resolvedIndependent)} independientes (uno por activo y ventana de ${weeks(h.horizonWeeks)}): es la muestra que cuenta para los intervalos.`}
            {h.skipped > 0 ? ` ${formatInt(h.skipped)} descartados sin benchmark.` : ''}
          </p>
        </article>
      ))}
    </div>
  )
}

function HorizonPicker({ value, onChange, label }: { value: LearningHorizon; onChange: (h: LearningHorizon) => void; label: string }) {
  return (
    <div className="seg" role="group" aria-label={label}>
      {HORIZONS.map((h) => (
        <button key={h} type="button" aria-pressed={value === h} onClick={() => onChange(h)}>{weeks(h)}</button>
      ))}
    </div>
  )
}

function VerdictRow({ s, baseline, all }: { s: VerdictStats; baseline: number | null; all?: boolean }) {
  const ok = enoughData(s)
  const read = baseline === null ? 'none' : hitVerdict(s, baseline)
  const excessSig = ok && s.meanExcessCi !== null && ciExcludes(s.meanExcessCi, 0)
  return (
    <tr className={all ? 'row-all' : undefined}>
      <th scope="row" className="tl">{verdictName(s.verdict)}</th>
      <td className="r num">{formatInt(s.n)}</td>
      {ok ? (
        <>
          <td className="r num">
            {rateText(s.hitRate, s.hitRateCi)}
            {read === 'above' ? <span className="chip up" style={{ marginLeft: '0.4rem' }}>sobre la base</span> : null}
            {read === 'below' ? <span className="chip down" style={{ marginLeft: '0.4rem' }}>bajo la base</span> : null}
          </td>
          <td className={`r num ${excessSig ? (s.meanExcess! > 0 ? 'up' : 'down') : ''}`}>
            {excessText(s.meanExcess, s.meanExcessCi)}
            {s.meanExcessCi === null ? <span className="small muted"> · sin intervalo (pocos bloques)</span> : null}
          </td>
        </>
      ) : (
        <td className="r muted" colSpan={2}>Sin datos suficientes (mínimo {MIN_SHOWN} análisis)</td>
      )}
    </tr>
  )
}

function VerdictTable({ h }: { h: HorizonReport }) {
  const base = h.baseRateOutperform
  if (h.resolved === 0) {
    return <Empty>Sin datos suficientes: ningún análisis ha madurado para este horizonte.</Empty>
  }
  return (
    <>
      <p className="small">
        Tasa base: el <strong className="num">{base === null ? '—' : formatPercent(base, 0)}</strong> de los análisis eligió un activo
        que batió a su índice (SPY; BTC en cripto). Un veredicto de compra solo aporta algo si acierta claramente por encima de esa cifra.
      </p>
      <div className="table-wrap panel flush">
        <table className="tbl" aria-label={`Acierto por veredicto a ${weeks(h.horizonWeeks)}`}>
          <thead>
            <tr>
              <th className="tl">Veredicto</th>
              <th className="r">Análisis</th>
              <th className="r">Acierto (IC 95 %)</th>
              <th className="r">Exceso medio (IC 95 %)</th>
            </tr>
          </thead>
          <tbody>
            <VerdictRow s={h.overall} baseline={base} all />
            {h.byVerdict.map((s) => <VerdictRow key={s.verdict} s={s} baseline={base} />)}
          </tbody>
        </table>
      </div>
      <p className="small muted">
        El acierto de «Mantener» y «Esperar» se mide con una banda neutra (±2 %·√semanas) y no es comparable con el de comprar o evitar. El
        exceso es la rentabilidad del activo menos la de su índice.
      </p>
    </>
  )
}

function Reliability({ h }: { h: HorizonReport }) {
  const c = h.calibration
  return (
    <div className="grid cols-2" style={{ alignItems: 'start' }}>
      <ReliabilityChart bins={c.reliability} />
      <div className="stack">
        <div className={`notice ${c.applied ? '' : 'warn'}`} role="status">
          <Info aria-hidden="true" />
          <div>
            <strong>{c.applied ? 'Calibración aplicada.' : 'Sin calibrar.'}</strong> Motivo: {c.reason}
            <div className="small" style={{ marginTop: '0.25rem' }}>
              Muestra independiente: <span className="num">{formatInt(c.n)}</span> de <span className="num">{formatInt(c.minRequired)}</span> necesarios.
            </div>
          </div>
        </div>
        {c.brierRaw !== null ? (
          <dl className="stat-grid">
            <div><dt>Brier (todo)</dt><dd className="num">{c.brierRaw.toFixed(4)}</dd></div>
            {c.brierHoldoutRaw !== null ? <div><dt>Brier reciente, sin corregir</dt><dd className="num">{c.brierHoldoutRaw.toFixed(4)}</dd></div> : null}
            {c.brierHoldoutCalibrated !== null ? <div><dt>Brier reciente, calibrado</dt><dd className="num">{c.brierHoldoutCalibrated.toFixed(4)}</dd></div> : null}
          </dl>
        ) : null}
        <p className="small muted">
          Cada punto es un tramo de confianza declarada; la barra es el intervalo de confianza del acierto real y el tamaño del punto crece con
          el número de análisis. Por debajo de la diagonal el modelo presume de más seguridad de la que tiene. El Brier mide el error
          cuadrático de la confianza (menos es mejor; 0,25 equivale a decir siempre 50 %).
        </p>
      </div>
    </div>
  )
}

function Weights({ report }: { report: PerformanceReport }) {
  const rollback = useRollbackWeights()
  const [confirming, setConfirming] = useState<number | 'default' | null>(null)
  const active = report.activeWeights
  const versions = report.weightHistory
  const act = (target: number | 'default') => {
    setConfirming(null)
    rollback.mutate(target === 'default' ? null : target)
  }
  return (
    <div className="stack">
      {rollback.isError ? (
        <div className="notice error" role="alert"><AlertTriangle aria-hidden="true" /><span className="grow">{errorText(rollback.error)}</span></div>
      ) : null}
      {active ? (
        <div className="panel">
          <div className="row wrap" style={{ marginBottom: '0.6rem' }}>
            <strong>Versión {active.version} activa</strong>
            <span className="chip accent">{KIND_LABEL[active.kind]}</span>
            <span className="small muted">desde {formatDateTime(new Date(active.createdAtMs).toISOString())}</span>
          </div>
          <MultiplierBars v={active} />
          <p className="small muted" style={{ marginTop: '0.5rem' }}>
            Cada barra es un multiplicador sobre el peso por defecto de ese componente (1× = sin cambio). Los pesos se ajustan solo si superan una validación
            posterior; si la evidencia se agota, vuelven poco a poco a los de por defecto.
          </p>
        </div>
      ) : (
        <div className="notice" role="status">
          <Info aria-hidden="true" />
          <span>
            Se usan los pesos por defecto: todavía no hay datos para aprender otros. La puerta exige al menos 150 análisis independientes para
            entrenar y 80 para validar (con 8 bloques de 4 semanas); con menos, ajustar pesos sería ajustar ruido.
          </span>
        </div>
      )}

      {versions.length > 0 ? (
        <div className="table-wrap panel flush">
          <table className="tbl" aria-label="Versiones de pesos">
            <thead>
              <tr>
                <th className="tl">Versión</th>
                <th className="tl">Tipo</th>
                <th className="tl">Creada</th>
                <th className="r">Entreno / validación</th>
                <th className="r">ΔIC · p</th>
                <th className="tl">Nota</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {versions.map((v) => {
                const g = gateNumbers(v)
                const isActive = v.status === 'active'
                return (
                  <tr key={v.version}>
                    <th scope="row" className="tl num">v{v.version}{isActive ? <span className="chip up" style={{ marginLeft: '0.4rem' }}>activa</span> : null}</th>
                    <td>{KIND_LABEL[v.kind]}</td>
                    <td className="num">{formatDateTime(new Date(v.createdAtMs).toISOString())}</td>
                    <td className="r num">{v.nTrain !== null && v.nVal !== null ? `${v.nTrain} / ${v.nVal}` : '—'}</td>
                    <td className="r num">{g.deltaIc !== null ? `${g.deltaIc >= 0 ? '+' : ''}${g.deltaIc.toFixed(3)}` : '—'}{g.pValue !== null ? ` · p=${g.pValue.toFixed(3)}` : ''}</td>
                    <td className="small muted">{v.note ?? ''}</td>
                    <td className="r">
                      {isActive ? null : confirming === v.version ? (
                        <span className="row wrap" style={{ justifyContent: 'flex-end' }}>
                          <span className="small">¿Activar de nuevo la v{v.version}?</span>
                          <button type="button" className="btn sm danger" onClick={() => act(v.version)}>Sí, volver</button>
                          <button type="button" className="btn sm" onClick={() => setConfirming(null)}>No</button>
                        </span>
                      ) : (
                        <button type="button" className="btn sm" disabled={rollback.isPending} onClick={() => setConfirming(v.version)} aria-label={`Volver a la versión ${v.version}`}>
                          Volver a esta versión
                        </button>
                      )}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      ) : null}

      {!isDefaultWeights(active) ? (
        <div className="row wrap">
          {confirming === 'default' ? (
            <>
              <span className="small">¿Volver a los pesos por defecto? Se crea una versión nueva; no se borra nada.</span>
              <button type="button" className="btn sm danger" onClick={() => act('default')}>Sí, volver a los de por defecto</button>
              <button type="button" className="btn sm" onClick={() => setConfirming(null)}>No</button>
            </>
          ) : (
            <button type="button" className="btn sm" disabled={rollback.isPending} onClick={() => setConfirming('default')}>
              Volver a los pesos por defecto
            </button>
          )}
        </div>
      ) : null}
    </div>
  )
}

function MultiplierBars({ v }: { v: WeightVersion }) {
  const max = Math.max(2, ...v.multipliers)
  return (
    <ul className="mult-list" aria-label={`Multiplicadores de la versión ${v.version}`}>
      {v.multipliers.map((m, i) => {
        const name = v.factorNames[i] ?? `Factor ${i + 1}`
        return (
          <li key={name}>
            <span className="mult-name">{FACTOR_LABEL[name] ?? name}</span>
            <span className="mult-track" aria-hidden="true">
              <span className="mult-one" style={{ left: `${(1 / max) * 100}%` }} />
              <span className={`mult-bar ${m > 1.05 ? 'up' : m < 0.95 ? 'down' : ''}`} style={{ width: `${(m / max) * 100}%` }} />
            </span>
            <span className="num mult-val">{m.toFixed(2)}×</span>
          </li>
        )
      })}
    </ul>
  )
}

function WatchlistPanel({ w }: { w: WatchlistStatus }) {
  const run = w.lastRun
  if (!w.enabled) {
    return (
      <div className="notice warn" role="status">
        <AlertTriangle aria-hidden="true" />
        <span>La lista de seguimiento está desactivada (<code>WATCHLIST_ENABLED=0</code> o modo de datos simulados): no se generan análisis automáticos y el historial de aciertos no crece.</span>
      </div>
    )
  }
  const late = run && run.trigger === 'recovery'
  return (
    <div className="stack">
      <div className="row wrap">
        <span className="chip accent"><CalendarClock aria-hidden="true" /> Cada día a las {w.time}</span>
        <span className="chip">{w.assets.length} activos · profundidad {w.depth}</span>
        {w.running ? <span className="chip warn">Pasada en marcha</span> : null}
        <span className="small">
          Próxima pasada: <strong>{w.nextRun ? formatDateTime(w.nextRun) : '—'}</strong>
          {w.nextRun ? <span className="muted"> ({formatRelative(w.nextRun)})</span> : null}
        </span>
        <span className="small muted">{formatInt(w.analysesTotal)} análisis guardados por la lista</span>
      </div>
      {w.defaultList ? (
        <p className="small muted">
          Lista por defecto, fijada antes de ver ningún resultado. Cambiarla según cómo salgan los aciertos invalidaría la medición.
        </p>
      ) : null}
      {run ? (
        <p className="small">
          Última pasada: <strong>{formatDateTime(run.startedAt)}</strong> · {run.ok} correctos{run.failed > 0 ? <>, <strong className="down">{run.failed} con fallo</strong></> : null}
          {run.finishedAt ? <span className="muted"> · duró {formatSpan((Date.parse(run.finishedAt) - Date.parse(run.startedAt)) / 1000)}</span> : null}
          {late ? <span className="muted"> · recuperada: tocaba a las {formatDateTime(run.scheduledFor)}</span> : null}
          {run.powerWaitSecs > 0 ? <span className="muted"> · esperó {formatSpan(run.powerWaitSecs)} a que se enchufara el equipo</span> : null}
          {run.status === 'aborted' ? <span className="down"> · abortada (Ollama o el mercado no respondían); se reintenta</span> : null}
          {run.status === 'interrupted' ? <span className="down"> · interrumpida al apagar el servidor</span> : null}
        </p>
      ) : (
        <p className="small muted">Todavía no se ha hecho ninguna pasada.</p>
      )}
      <details className="panel flush">
        <summary className="watch-summary">Estado por activo ({w.assets.length})</summary>
        <div className="table-wrap">
          <table className="tbl" aria-label="Último intento por activo">
            <thead>
              <tr><th className="tl">Activo</th><th className="tl">Último intento</th><th className="tl">Veredicto</th><th className="tl">Último correcto</th><th className="tl">Detalle</th></tr>
            </thead>
            <tbody>
              {w.assets.map((a) => (
                <tr key={a.id}>
                  <th scope="row" className="tl">{symbolOf(a.id)}</th>
                  <td>
                    {a.last === null ? <span className="muted">Sin intentos</span>
                      : a.last.status === 'ok' ? <span className="up">Correcto</span>
                      : a.last.status === 'skipped' ? <span className="muted">No intentado</span>
                      : <span className="down">Falló</span>}
                    {a.last ? <span className="small muted"> · {formatRelative(a.last.finishedAt)}</span> : null}
                  </td>
                  <td>{a.last?.verdict ? verdictName(a.last.verdict) : '—'}</td>
                  <td className="small">{a.lastOkAt ? formatDateTime(a.lastOkAt) : '—'}</td>
                  <td className="small muted">
                    {a.last?.error ? `${a.last.error}${a.last.transient ? ' (transitorio)' : ''}${a.last.attempts > 1 ? ` · ${a.last.attempts} intentos` : ''}` : ''}
                    {a.last?.status === 'skipped' ? 'La pasada se abortó antes de llegar aquí.' : ''}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </div>
  )
}

export default function PerformancePage() {
  useTitle('Rendimiento')
  const perf = usePerformance()
  const watch = useWatchlist()
  const [horizon, setHorizon] = useState<LearningHorizon>(4)
  const report = perf.data
  const h = report?.horizons.find((x) => x.horizonWeeks === horizon)

  return (
    <div className="page">
      <div className="page-head">
        <h1>Rendimiento</h1>
        <p className="muted small grow" style={{ maxWidth: '50rem' }}>
          Cuánto han acertado de verdad los análisis, comparados con lo que hizo el mercado después. Solo cuentan los análisis que ya han
          cumplido su plazo, y los intervalos descuentan que los análisis de un mismo periodo no son independientes entre sí.
        </p>
      </div>

      {perf.isPending ? <SkeletonRows rows={4} h="4rem" /> : null}
      {perf.isError ? <ErrorNotice error={perf.error} onRetry={() => void perf.refetch()} title="No se pudo cargar el rendimiento" /> : null}

      {report && report.analysesTotal === 0 ? (
        <div className="notice" role="status">
          <Info aria-hidden="true" />
          <div>
            <strong>Todavía no hay análisis que medir.</strong> No se muestra ninguna cifra de acierto porque no existe ninguna.
            {watch.data?.enabled
              ? ' La lista de seguimiento generará el primero en su próxima pasada; los resultados empiezan a llegar una semana después y los intervalos fiables tardan meses.'
              : ' Activa la lista de seguimiento o analiza activos para empezar a acumular historial.'}
          </div>
        </div>
      ) : null}

      {watch.isError ? <ErrorNotice error={watch.error} onRetry={() => void watch.refetch()} title="No se pudo cargar la lista de seguimiento" /> : null}
      {watch.data ? (
        <Section title="Lista de seguimiento" hint="Análisis automáticos diarios" id="watchlist">
          <WatchlistPanel w={watch.data} />
        </Section>
      ) : watch.isPending ? <SkeletonRows rows={2} h="2.4rem" /> : null}

      {report ? (
        <>
          <Section title="Análisis resueltos" hint={`${formatInt(report.analysesTotal)} en total`} id="resolved">
            <HorizonCards report={report} />
          </Section>

          <Section title="Acierto por veredicto" hint="Frente a la tasa base" id="verdicts">
            <HorizonPicker value={horizon} onChange={setHorizon} label="Horizonte" />
            {h ? <VerdictTable h={h} /> : null}
          </Section>

          <Section title="Fiabilidad de la confianza" hint="Lo que dice el modelo frente a lo que ocurre" id="reliability">
            {h ? (h.calibration.reliability.length > 0 ? <Reliability h={h} /> : <Empty>Sin datos suficientes: aún no hay análisis resueltos a {weeks(horizon)} con los que dibujar la curva.</Empty>) : null}
          </Section>

          <Section title="Pesos de la puntuación" hint="Versiones y vuelta atrás" id="weights">
            <Weights report={report} />
          </Section>

          <Section title="Cómo leer estas cifras" id="caveats">
            <ul className="caveats">
              {report.caveats.map((c) => <li key={c}>{c}</li>)}
              <li>Reglas de resolución versión {report.ruleVersion}: fijadas antes de ver ningún resultado.</li>
            </ul>
          </Section>
        </>
      ) : null}
    </div>
  )
}
