import { Check, Loader2, Sparkles, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useAnalyses, useGenerateAnalysis, useLatestAnalysis, useStatus } from '@/api/hooks'
import type { AnalysisDepth, AnalysisProgress, AssetSummary, Quote } from '@/api/types'
import { ANALYSIS_DEPTHS } from '@/api/types'
import { AnalysisReport } from '@/components/AnalysisReport'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { VERDICT_LABEL } from '@/lib/assets'
import { formatDateTime, formatInt, formatPercent, formatPrice, formatRelative } from '@/lib/format'

const DEPTH_INFO: Record<AnalysisDepth, { label: string; hint: string; typical: string }> = {
  quick: { label: 'Rápido', hint: 'Solo los datos ya reunidos; sin consultas extra.', typical: '≈ 10 s' },
  standard: { label: 'Estándar', hint: 'Datos en vivo, indicadores, fundamentales y noticias.', typical: '≈ 30 s' },
  deep: { label: 'Profundo', hint: 'Más herramientas y verificaciones cruzadas del modelo.', typical: '60 s o más' },
}

/** Etapas reales emitidas por el backend (`stage`), acumuladas por índice. */
function Progress({ depth, stages, onCancel }: { depth: AnalysisDepth; stages: AnalysisProgress[]; onCancel: () => void }) {
  const [secs, setSecs] = useState(0)
  useEffect(() => {
    const t = setInterval(() => setSecs((s) => s + 1), 1000)
    return () => clearInterval(t)
  }, [])
  const seen = stages
  const progress = stages[stages.length - 1] ?? null
  const now = progress?.stage
  return (
    <div className="panel stack" role="status" aria-live="polite">
      <div className="row">
        <Loader2 className="spin" aria-hidden="true" style={{ animation: 'spin 1s linear infinite' }} />
        <strong className="grow">
          Generando análisis ({DEPTH_INFO[depth].label.toLowerCase()})
          {progress ? <span className="muted small"> · paso {formatInt(progress.index)} de {formatInt(progress.total)}</span> : null}
        </strong>
        <span className="num muted">{secs} s</span>
        <button type="button" className="btn sm" onClick={onCancel}><X aria-hidden="true" /> Cancelar</button>
      </div>
      <div className="indet" />
      {seen.length > 0 ? (
        <ol className="progress-steps">
          {seen.map((p) => (
            <li key={p.stage} className={p.stage === now ? 'now' : 'done'}>
              {p.stage === now ? <span style={{ width: 14 }} /> : <Check size={14} aria-hidden="true" />} {p.label}
            </li>
          ))}
        </ol>
      ) : (
        <p className="small muted">Conectando con el análisis…</p>
      )}
      <p className="tiny muted">
        {seen.length === 0 && secs >= 6
          ? 'Todavía no ha llegado ninguna etapa: este backend puede no informar del avance. '
          : ''}
        Suele tardar {DEPTH_INFO[depth].typical}; en equipos modestos o con modelos grandes puede tardar bastante más.
        Al cancelar se corta la conexión y el servidor deja de trabajar en el análisis.
      </p>
    </div>
  )
}

export function AnalysisPanel({ summary, quote }: { summary: AssetSummary; quote: Quote | null | undefined }) {
  const id = summary.id
  const latest = useLatestAnalysis(id)
  const list = useAnalyses(id)
  const gen = useGenerateAnalysis(id)
  const status = useStatus()
  const [depth, setDepth] = useState<AnalysisDepth>('standard')
  const [picked, setPicked] = useState<string | null>(null)

  const history = list.data ?? []
  const shown = (picked ? history.find((a) => a.id === picked) : null) ?? latest.data ?? history[0] ?? null
  const llmDown = status.data ? !status.data.llm.available : false

  return (
    <div className="stack" style={{ gap: '1.25rem' }}>
      <div className="panel stack" style={{ gap: '0.7rem' }}>
        <div className="row wrap">
          <fieldset style={{ border: 0, padding: 0, margin: 0 }}>
            <legend className="small muted" style={{ padding: 0, marginBottom: 4 }}>Profundidad</legend>
            <div className="seg" role="radiogroup" aria-label="Profundidad del análisis">
              {ANALYSIS_DEPTHS.map((d) => (
                <button key={d} type="button" role="radio" aria-checked={depth === d} aria-pressed={depth === d} onClick={() => setDepth(d)} disabled={gen.isPending} title={DEPTH_INFO[d].hint}>
                  {DEPTH_INFO[d].label} <span className="muted tiny">{DEPTH_INFO[d].typical}</span>
                </button>
              ))}
            </div>
          </fieldset>
          <button type="button" className="btn primary right" disabled={gen.isPending || llmDown} onClick={() => { setPicked(null); gen.mutate(depth) }}>
            <Sparkles aria-hidden="true" /> {shown ? 'Generar análisis nuevo' : 'Generar análisis'}
          </button>
        </div>
        <p className="small muted">{DEPTH_INFO[depth].hint} Modelo: {status.data?.llm.model ?? '…'} (se cambia en Ajustes → Modelos de IA).</p>
        {llmDown ? <div className="notice warn" role="status">La IA local no está disponible{status.data?.llm.error ? `: ${status.data.llm.error}` : ''}. Arranca Ollama para generar análisis.</div> : null}
      </div>

      {gen.isPending ? <Progress depth={depth} stages={gen.stages} onCancel={gen.cancel} /> : null}
      {gen.isError && !(gen.error instanceof DOMException) && gen.error.name !== 'AbortError' ? (
        <ErrorNotice error={gen.error} onRetry={() => gen.mutate(depth)} title="No se pudo generar el análisis" />
      ) : null}

      {latest.isPending ? <SkeletonRows rows={4} h="3rem" /> : null}
      {latest.isError ? <ErrorNotice error={latest.error} onRetry={() => void latest.refetch()} title="No se pudo cargar el último análisis" /> : null}
      {shown ? (
        <>
          {picked && shown.id !== latest.data?.id ? (
            <div className="notice" role="status">
              Estás viendo un análisis anterior ({formatDateTime(shown.generatedAt)}).
              <button type="button" className="btn sm right" onClick={() => setPicked(null)}>Ver el más reciente</button>
            </div>
          ) : null}
          <AnalysisReport analysis={shown} quote={quote} />
        </>
      ) : !latest.isPending && !gen.isPending ? (
        <div className="empty">Aún no hay análisis de {summary.symbol}. Elige la profundidad y genera el primero.</div>
      ) : null}

      {history.length > 1 ? (
        <section aria-label="Histórico de análisis">
          <h3 style={{ fontSize: '0.9375rem', fontWeight: 600, margin: '0 0 0.4rem' }}>Histórico de análisis</h3>
          <div className="table-wrap">
            <table className="tbl">
              <thead><tr><th>Fecha</th><th>Veredicto</th><th className="r">Confianza</th><th className="r">Global</th><th className="r">Precio</th><th>Modelo</th><th><span className="sr-only">Ver</span></th></tr></thead>
              <tbody>
                {history.map((a) => (
                  <tr key={a.id} aria-selected={a.id === shown?.id}>
                    <td title={formatDateTime(a.generatedAt)}>{formatRelative(a.generatedAt)}</td>
                    <td><span className={`chip ${a.verdict === 'buy' || a.verdict === 'accumulate' ? 'up' : a.verdict === 'reduce' || a.verdict === 'avoid' ? 'down' : ''}`}>{VERDICT_LABEL[a.verdict]}</span></td>
                    <td className="r num">{formatPercent(a.confidence, 0)}</td>
                    <td className="r num">{Math.round(a.scores.overall)}</td>
                    <td className="r num">{formatPrice(a.priceAtAnalysis, { currency: quote?.currency })}</td>
                    <td className="small muted">{a.model}</td>
                    <td><button type="button" className="btn sm" disabled={a.id === shown?.id} onClick={() => setPicked(a.id)}>{a.id === shown?.id ? 'Mostrado' : 'Ver'}</button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ) : null}
    </div>
  )
}
