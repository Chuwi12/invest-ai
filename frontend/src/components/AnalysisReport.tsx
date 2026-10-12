import { AlertTriangle } from 'lucide-react'
import { memo, useState } from 'react'
import { useSnapshot } from '@/api/hooks'
import type { Analysis, Catalyst, ScoreBreakdown } from '@/api/types'
import { VERDICTS } from '@/api/types'
import { ExternalLink } from '@/components/ExternalLink'
import { LivePrice } from '@/components/LivePrice'
import { Provenance } from '@/components/Provenance'
import { HORIZON_HINT, HORIZON_LABEL, VERDICT_HINT, VERDICT_LABEL, symbolOf } from '@/lib/assets'
import { analysisFreshness } from '@/lib/freshness'
import { formatDate, formatDateTime, formatPercent, formatPrice, formatRelative, formatSigned } from '@/lib/format'
import { sourceName } from '@/lib/sources'
import type { Quote } from '@/api/types'

const SCORE_ROWS: [keyof ScoreBreakdown, string][] = [
  ['overall', 'Global'], ['technical', 'Técnico'], ['momentum', 'Momentum'], ['fundamental', 'Fundamental'],
  ['valuation', 'Valoración'], ['sentiment', 'Sentimiento'], ['macroScore', 'Macro'], ['risk', 'Riesgo'],
]

export function VerdictScale({ verdict }: { verdict: Analysis['verdict'] }) {
  return (
    <div aria-hidden="true">
      <div className="scale">
        {VERDICTS.map((v) => <span key={v} className={v === verdict ? `on ${v}` : ''} />)}
      </div>
      <div className="scale-labels">
        {VERDICTS.map((v) => <span key={v} className={v === verdict ? 'on' : ''}>{VERDICT_LABEL[v]}</span>)}
      </div>
    </div>
  )
}

function Scores({ s }: { s: ScoreBreakdown }) {
  return (
    <div className="stack" style={{ gap: '0.45rem' }} role="list" aria-label="Puntuaciones por bloque (0 a 100)">
      {SCORE_ROWS.map(([k, label]) => {
        const v = s[k]
        if (v === null) return (
          <div key={k} className="score-row muted" role="listitem"><span>{label}</span><span className="small">sin datos suficientes</span><span /></div>
        )
        return (
          <div key={k} className="score-row" role="listitem">
            <span title={k === 'risk' ? 'Más alto = más riesgo' : undefined}>{label}</span>
            <div className={`bar ${k === 'risk' ? 'risk' : ''}`} role="img" aria-label={`${label}: ${Math.round(v)} de 100`}><i style={{ width: `${Math.max(0, Math.min(100, v))}%` }} /></div>
            <span className="num right">{Math.round(v)}</span>
          </div>
        )
      })}
    </div>
  )
}

const CatalystRow = ({ c }: { c: Catalyst }) => (
  <li className="lvl" style={{ alignItems: 'baseline' }}>
    <span>{c.description}</span>
    <span className="muted small num" style={{ whiteSpace: 'nowrap' }}>
      {c.date ? formatDate(c.date) : 'sin fecha'}
      {c.impact !== null ? <span className={c.impact > 0 ? 'up' : c.impact < 0 ? 'down' : ''}> · impacto {formatSigned(c.impact, 1)}</span> : null}
    </span>
  </li>
)

/** Precio actual frente al del análisis; es el único hijo que se mueve con los ticks. */
const Drift = memo(function Drift({ quote, atAnalysis }: { quote: Quote | null | undefined; atAnalysis: number | null }) {
  if (!quote) return null
  return (
    <span className="row small muted" style={{ gap: '0.35rem' }}>
      <span>Ahora:</span>
      <LivePrice quote={quote} className="small" dot={false} />
      {atAnalysis ? <span>(al analizar: {formatPrice(atAnalysis, { currency: quote.currency })})</span> : null}
    </span>
  )
})

function Snapshot({ a }: { a: Analysis }) {
  const [open, setOpen] = useState(false)
  const snap = useSnapshot(a.asset, open)
  return (
    <details onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}>
      <summary className="small" style={{ cursor: 'pointer' }}>Ver el contexto de datos que recibe el modelo ahora</summary>
      {open ? (
        snap.isPending ? <p className="small muted">Cargando…</p> :
        snap.isError ? <p className="small down">No se pudo cargar el contexto.</p> : (
          <div className="stack small" style={{ marginTop: '0.5rem' }}>
            <p className="muted">Contexto actual (puede diferir del que vio este análisis, generado {formatRelative(a.generatedAt)}). Generado {formatDateTime(snap.data.generatedAt)}.</p>
            <ul className="bullets">
              <li>Cotización: {snap.data.quote ? `${formatPrice(snap.data.quote.price, { currency: snap.data.quote.currency })} (${sourceName(snap.data.quote.source)}, ${formatRelative(snap.data.quote.timestamp)})` : 'no disponible'}</li>
              <li>Indicadores técnicos: {snap.data.indicators ? `calculados ${formatRelative(snap.data.indicators.asOf)}` : 'no disponibles'}</li>
              <li>Fundamentales: {snap.data.fundamentals ? `${snap.data.fundamentals.sources.map(sourceName).join(', ') || 'fuente no indicada'}` : 'no disponibles'}</li>
              <li>Noticias: {snap.data.news.length} titulares</li>
              <li>Macro: {snap.data.macroContext ? `${snap.data.macroContext.indicators.length} indicadores` : 'no disponible'}</li>
            </ul>
            {snap.data.missing.length > 0 ? <p className="warn-text">Faltan: {snap.data.missing.join('; ')}</p> : null}
          </div>
        )
      ) : null}
    </details>
  )
}

interface Props {
  analysis: Analysis
  quote?: Quote | null | undefined
  /** En el chat se omiten el histórico de fuentes largo y el snapshot. */
  compact?: boolean
}

export const AnalysisReport = memo(function AnalysisReport({ analysis: a, quote, compact }: Props) {
  const fresh = analysisFreshness(a.generatedAt)
  const k = a.keyLevels
  const opt = { currency: quote?.currency ?? null }
  return (
    <article className="report" aria-label={`Análisis de IA de ${symbolOf(a.asset)}`}>
      <div className="verdict-block">
        <div>
          <div className={`verdict-word ${a.verdict}`}>{VERDICT_LABEL[a.verdict]}</div>
          <div className="small muted">{VERDICT_HINT[a.verdict]}</div>
        </div>
        <div className="stack" style={{ gap: '0.6rem' }}>
          <VerdictScale verdict={a.verdict} />
          <div className="row wrap small">
            <span title="Confianza del modelo en su veredicto, de 0 a 100 %. No es una probabilidad de acierto.">
              Confianza <strong className="num">{formatPercent(a.confidence, 0)}</strong>
            </span>
            <span>· Horizonte <strong>{HORIZON_LABEL[a.horizon]}</strong> <span className="muted">({HORIZON_HINT[a.horizon]})</span></span>
            <span>· Puntuación global <strong className="num">{Math.round(a.scores.overall)}</strong>/100</span>
          </div>
        </div>
      </div>

      {fresh.stale ? (
        <div className="notice warn" role="status">
          <AlertTriangle aria-hidden="true" />
          <span>Este análisis es de {formatRelative(a.generatedAt)}: puede no reflejar el mercado actual. Genera uno nuevo para datos frescos.</span>
        </div>
      ) : null}

      <p className="report-prose" style={{ fontSize: '1.2rem' }}>{a.summary}</p>

      <div className="grid cols-2" style={{ alignItems: 'start' }}>
        <section><h3>Puntuaciones</h3><Scores s={a.scores} /></section>
        <section>
          <h3>Niveles clave</h3>
          <div className="stack" style={{ gap: 0 }}>
            {k.entryZone ? <div className="lvl"><span>Zona de entrada</span><span className="num">{formatPrice(k.entryZone[0], opt)} – {formatPrice(k.entryZone[1], opt)}</span></div> : null}
            {k.stopLoss !== null ? <div className="lvl"><span>Stop sugerido</span><span className="num down">{formatPrice(k.stopLoss, opt)}</span></div> : null}
            {k.targets.length > 0 ? <div className="lvl"><span>Objetivos</span><span className="num up">{k.targets.map((t) => formatPrice(t, opt)).join(' · ')}</span></div> : null}
            {k.supports.length > 0 ? <div className="lvl"><span>Soportes</span><span className="num">{k.supports.map((t) => formatPrice(t, opt)).join(' · ')}</span></div> : null}
            {k.resistances.length > 0 ? <div className="lvl"><span>Resistencias</span><span className="num">{k.resistances.map((t) => formatPrice(t, opt)).join(' · ')}</span></div> : null}
            {!k.entryZone && k.stopLoss === null && k.targets.length + k.supports.length + k.resistances.length === 0 ? <p className="muted small">El modelo no propuso niveles.</p> : null}
          </div>
          <div style={{ marginTop: '0.5rem' }}><Drift quote={quote} atAnalysis={a.priceAtAnalysis} /></div>
        </section>
      </div>

      <section>
        <h3>Tesis</h3>
        <p className="report-prose">{a.thesis}</p>
      </section>

      <div className="cases">
        <section className="bull"><h3 className="up">Caso alcista</h3><ul>{a.bullCase.map((t, i) => <li key={i}>{t}</li>)}</ul></section>
        <section className="bear"><h3 className="down">Caso bajista</h3><ul>{a.bearCase.map((t, i) => <li key={i}>{t}</li>)}</ul></section>
      </div>

      <div className="grid cols-2" style={{ alignItems: 'start' }}>
        <section><h3>Riesgos</h3><ul className="bullets">{a.risks.map((t, i) => <li key={i}>{t}</li>)}</ul></section>
        <section>
          <h3>Catalizadores</h3>
          {a.catalysts.length ? <ul>{a.catalysts.map((c, i) => <CatalystRow key={i} c={c} />)}</ul> : <p className="muted small">Ninguno identificado.</p>}
        </section>
      </div>

      {a.strategy ? <section><h3>Estrategia</h3><p className="report-prose">{a.strategy}</p></section> : null}

      <section>
        <h3>Qué datos faltan o son dudosos</h3>
        {a.dataWarnings.length ? (
          <ul className="bullets">{a.dataWarnings.map((t, i) => <li key={i}>{t}</li>)}</ul>
        ) : (
          <p className="muted small">El modelo no declaró carencias de datos.</p>
        )}
      </section>

      {!compact ? (
        <section>
          <h3>Fuentes citadas</h3>
          {a.sources.length ? (
            <ul className="stack" style={{ gap: '0.3rem' }}>
              {a.sources.map((s, i) => (
                <li key={i} className="small">
                  {s.url ? <ExternalLink href={s.url}><span style={{ textDecoration: 'underline' }}>{s.title}</span></ExternalLink> : <span>{s.title}</span>}{' '}
                  <Provenance provider={s.provider} asOf={s.publishedAt ?? a.generatedAt} label="vía" />
                </li>
              ))}
            </ul>
          ) : <p className="muted small">Sin fuentes citadas.</p>}
        </section>
      ) : null}

      <footer className="row wrap tiny muted" style={{ borderTop: '1px solid var(--line)', paddingTop: '0.6rem' }}>
        <span>Modelo: <strong>{a.model}</strong></span>
        <time dateTime={a.generatedAt} title={formatDateTime(a.generatedAt)}>· generado {formatRelative(a.generatedAt)} ({formatDateTime(a.generatedAt)})</time>
        {a.priceAtAnalysis !== null ? <span>· precio en ese momento {formatPrice(a.priceAtAnalysis, opt)}</span> : null}
        <span>· Análisis automatizado: no es asesoramiento financiero.</span>
      </footer>
      {!compact ? <Snapshot a={a} /> : null}
    </article>
  )
})
