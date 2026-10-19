import { useAssetSocial, useSocialStatus } from '@/api/hooks'
import type { AssetSummary, SocialExcerpt, SocialMetrics, SocialStance } from '@/api/types'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { ExternalLink } from '@/components/ExternalLink'
import { formatDateTime, formatNumber, formatPercent, formatRelative } from '@/lib/format'

const STANCE: Record<SocialStance, { label: string; cls: string }> = {
  bullish: { label: 'alcista', cls: 'up' },
  bearish: { label: 'bajista', cls: 'down' },
  neutral: { label: 'neutro', cls: '' },
}

const SOURCE: Record<string, string> = { reddit: 'Reddit', x: 'X' }

function Metrics({ m }: { m: SocialMetrics }) {
  const attention =
    m.ratio != null && m.z != null
      ? `×${formatNumber(m.ratio, 2)} su media diaria (z ${formatNumber(m.z, 2)}, ${m.baselineDays} bloques de 24 h observados)`
      : `sin histórico suficiente para compararlo con su media (${m.baselineDays} bloques de 24 h observados)`
  const rows: [string, string][] = [
    ['Menciones (24 h)', `${m.mentions}${m.truncated ? '+' : ''}${m.bySource.length ? ` (${m.bySource.map((s) => `${SOURCE[s.source] ?? s.source} ${s.mentions}`).join(', ')})` : ''}`],
    ['Atención', attention],
    ['Autores distintos (24 h)', String(m.authors)],
    ['Tono (24 h)', `${m.bullish} alcistas · ${m.bearish} bajistas · ${m.neutral} neutras${m.bullishShare != null ? ` (${formatPercent(m.bullishShare, 0)} alcista entre las que tienen tono)` : ''}`],
    [`En ${m.windowDays} días`, `${m.mentionsWindow} menciones · ${m.bullishWindow} alcistas · ${m.bearishWindow} bajistas`],
  ]
  return (
    <dl className="kv" aria-label="Métricas de redes sociales">
      {rows.map(([k, v]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd className="num">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

function Excerpt({ e }: { e: SocialExcerpt }) {
  const st = STANCE[e.stance]
  const where = e.source === 'reddit' ? `Reddit · r/${e.community}` : 'X'
  return (
    <article className="news-item">
      <div className="stack grow" style={{ gap: '0.15rem' }}>
        <ExternalLink href={e.url} className="title">{e.title ?? e.text}</ExternalLink>
        {e.title && e.text ? <p className="small muted" style={{ display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>{e.text}</p> : null}
        <div className="row wrap tiny muted">
          <span>{where}</span>
          <time dateTime={e.createdAt} title={formatDateTime(e.createdAt)}>· {formatRelative(e.createdAt)}</time>
          <span>· {e.score} puntos · {e.comments} comentarios</span>
          <span className={`chip ${st.cls}`} title="Tono según un léxico, no una valoración del modelo">{st.label}</span>
        </div>
      </div>
    </article>
  )
}

export function SocialPanel({ summary }: { summary: AssetSummary }) {
  const q = useAssetSocial(summary.id)
  const status = useSocialStatus(q.data?.enabled === true)
  return (
    <section className="stack" aria-labelledby="social-title">
      <h3 id="social-title">Redes sociales (opinión, no verificado)</h3>
      <div className="notice warn" role="note">
        <span>
          Son opiniones de usuarios anónimos de Reddit y X: no se pueden verificar y pueden estar manipuladas (bombeos, bots, cuentas pagadas). Miden la
          atención y el tono del público minorista, no hechos ni cifras. No entran en la puntuación ni en los pesos; su valor predictivo no está demostrado.
        </span>
      </div>
      {q.isPending ? <SkeletonRows rows={4} h="3.5rem" /> : null}
      {q.isError ? <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudieron cargar los datos de redes" /> : null}
      {q.data && !q.data.enabled ? (
        <div className="empty">
          Las redes sociales están desactivadas (sin credenciales de Reddit ni de X). Ver <code>docs/notes/social.md</code> para activarlas.
        </div>
      ) : null}
      {q.data?.enabled && !q.data.digest ? <div className="empty">Sin publicaciones recientes que mencionen este activo, o las redes no respondieron.</div> : null}
      {q.data?.digest ? (
        <>
          <Metrics m={q.data.digest.metrics} />
          {q.data.digest.metrics.truncated ? <p className="tiny muted">La consulta alcanzó el tope de resultados: las menciones son una cota inferior.</p> : null}
          <div role="list" aria-label="Extractos de publicaciones">
            {q.data.digest.excerpts.map((e) => (
              <div role="listitem" key={e.id}>
                <Excerpt e={e} />
              </div>
            ))}
          </div>
          <p className="tiny muted">Datos a {formatDateTime(q.data.digest.metrics.asOf)}. Autores seudonimizados; los enlaces llevan a la publicación original.</p>
        </>
      ) : null}
      {status.data?.x.enabled ? (
        <p className="tiny muted" data-testid="x-budget">
          X (pago por uso): {status.data.x.used} de {status.data.x.monthlyReadBudget} lecturas este mes, coste estimado ≈ {formatNumber(status.data.x.estimatedCostUsd ?? 0, 2)} USD
          {status.data.x.exhausted ? ' · presupuesto agotado: no se leen más posts este mes' : ''}.
        </p>
      ) : null}
      {status.data?.lastError ? <p className="tiny muted">Último error de redes ({status.data.lastError.source}): {status.data.lastError.message}</p> : null}
    </section>
  )
}
