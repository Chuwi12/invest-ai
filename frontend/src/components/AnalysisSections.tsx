import { Info } from 'lucide-react'
import type { Analysis, PositionSizing } from '@/api/types'
import { HORIZON_LABEL } from '@/lib/assets'
import { SCENARIO_LABEL, formatDurationMs, scenarioSlices } from '@/lib/analysis'
import { formatInt, formatNumber, formatPrice, formatSigned } from '@/lib/format'

const pctText = (v: number, digits = 1) => `${formatSigned(v, digits)} %`

/** Escenarios con barra apilada (suma siempre 100 %), sus precios y la rentabilidad esperada. */
export function Scenarios({ a, currency }: { a: Analysis; currency: string | null | undefined }) {
  const built = scenarioSlices(a.scenarios)
  if (!built) return null
  const { slices, normalized, sum } = built
  const label = slices.map((s) => `${SCENARIO_LABEL[s.scenario.kind]} ${s.pct} %`).join(', ')
  const er = a.expectedReturnPct
  return (
    <section aria-label="Escenarios">
      <h3>Escenarios</h3>
      <div className="stack" style={{ gap: '0.7rem' }}>
        <div className="scen-bar" role="img" aria-label={`Probabilidades: ${label}`}>
          {slices.filter((s) => s.pct > 0).map((s) => (
            <span key={s.scenario.kind} className={`scen ${s.scenario.kind}`} style={{ flexGrow: s.pct }}>
              {s.pct >= 10 ? `${s.pct} %` : ''}
            </span>
          ))}
        </div>
        {normalized ? (
          <p className="tiny muted">Las probabilidades del modelo sumaban {formatNumber(sum, 1)} %; se han reescalado al 100 % para dibujar la barra.</p>
        ) : null}
        <ul className="scen-list">
          {slices.map(({ scenario: s, pct }) => (
            <li key={s.kind}>
              <div className="scen-head">
                <span className={`scen-dot ${s.kind}`} aria-hidden="true" />
                <strong>{SCENARIO_LABEL[s.kind]}</strong>
                <span className="num muted">{pct} %</span>
                <span className="grow" />
                {s.targetPrice !== null ? (
                  <span className="num small">
                    objetivo {formatPrice(s.targetPrice, { currency })}
                    {s.returnPct !== null ? <span className={s.returnPct > 0 ? 'up' : s.returnPct < 0 ? 'down' : ''}> ({s.returnPct > 0 ? '▲' : s.returnPct < 0 ? '▼' : '■'} {pctText(s.returnPct)})</span> : null}
                  </span>
                ) : <span className="small muted">sin precio objetivo en los datos</span>}
              </div>
              <p className="small">{s.description}</p>
              {s.targetPrice !== null && s.targetBasis ? <p className="tiny muted">Base del precio: {s.targetBasis}.</p> : null}
            </li>
          ))}
        </ul>
        {er !== null && er !== undefined ? (
          <p className="small" title="Suma de probabilidad × rentabilidad de cada escenario, sobre el precio del análisis. Es una cuenta, no una predicción.">
            Rentabilidad esperada ponderada por probabilidad:{' '}
            <strong className={`num ${er > 0 ? 'up' : er < 0 ? 'down' : ''}`}>{er > 0 ? '▲ ' : er < 0 ? '▼ ' : ''}{pctText(er)}</strong>
            <span className="muted"> a {HORIZON_LABEL[a.horizon].toLowerCase()}, sobre el precio del análisis. No es una predicción.</span>
          </p>
        ) : null}
      </div>
    </section>
  )
}

export function WouldChange({ items }: { items: string[] | null | undefined }) {
  const list = (items ?? []).filter((t) => t.trim())
  if (list.length === 0) return null
  return (
    <section aria-label="Qué cambiaría el veredicto">
      <h3>Qué cambiaría el veredicto</h3>
      <ul className="bullets">{list.map((t, i) => <li key={i}>{t}</li>)}</ul>
    </section>
  )
}

export function PositionSizingBlock({ ps }: { ps: PositionSizing | null | undefined }) {
  if (!ps) return null
  const rows: [string, string][] = [['Máximo orientativo de la cartera', `${formatNumber(ps.maxPortfolioPct, 1)} %`]]
  if (ps.riskPerTradePct !== null) rows.push(['Riesgo por operación', `${formatNumber(ps.riskPerTradePct, 1)} % de la cartera`])
  if (ps.stopDistancePct !== null) rows.push(['Distancia al stop', `${formatNumber(ps.stopDistancePct, 1)} %`])
  if (ps.atrPct !== null) rows.push(['ATR diario', `${formatNumber(ps.atrPct, 1)} %`])
  if (ps.volatility30d !== null) rows.push(['Volatilidad a 30 días', `${formatNumber(ps.volatility30d * 100, 1)} %`])
  return (
    <section aria-label="Tamaño de posición">
      <h3>Tamaño de posición</h3>
      <dl className="kv">
        {rows.map(([k, v]) => <div key={k}><dt>{k}</dt><dd className="num">{v}</dd></div>)}
      </dl>
      <p className="small" style={{ marginTop: '0.5rem', maxWidth: '68ch' }}>{ps.rationale}</p>
      <p className="tiny muted">Calculado en código a partir del ATR y la volatilidad; orientativo, no es asesoramiento.</p>
    </section>
  )
}

/** Avisos de calidad del propio modelo: discretos, sin color de alarma (no son carencias de datos). */
export function QualityNotes({ notes }: { notes: string[] | null | undefined }) {
  const list = (notes ?? []).filter((t) => t.trim())
  if (list.length === 0) return null
  return (
    <aside className="quality" aria-label="Calidad del análisis">
      <p className="tiny"><Info aria-hidden="true" /> <strong>Calidad del modelo</strong></p>
      <ul>{list.map((t, i) => <li key={i} className="tiny">{t}</li>)}</ul>
    </aside>
  )
}

export function MethodFooter({ a }: { a: Analysis }) {
  const depthName = a.depth ? { quick: 'rápido', standard: 'estándar', deep: 'profundo' }[a.depth] : null
  const tokens = (a.promptTokens ?? 0) + (a.completionTokens ?? 0)
  const hasTokens = a.promptTokens != null || a.completionTokens != null
  const parts: string[] = []
  if (depthName) parts.push(`profundidad ${depthName}`)
  if (a.durationMs != null) parts.push(`duración ${formatDurationMs(a.durationMs)}`)
  if (a.llmCalls != null) parts.push(`${formatInt(a.llmCalls)} ${a.llmCalls === 1 ? 'llamada' : 'llamadas'} al modelo`)
  if (hasTokens) {
    parts.push(`${formatInt(tokens)} tokens`)
  }
  return (
    <>
      {parts.map((p, i) => (
        <span key={i} title={hasTokens && p.endsWith('tokens') ? `${formatInt(a.promptTokens)} de entrada + ${formatInt(a.completionTokens)} de salida` : undefined}>· {p}</span>
      ))}
    </>
  )
}
