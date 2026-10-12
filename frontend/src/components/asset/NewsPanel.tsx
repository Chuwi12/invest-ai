import { memo, useMemo } from 'react'
import { useNews } from '@/api/hooks'
import type { AssetSummary, NewsItem } from '@/api/types'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { ExternalLink } from '@/components/ExternalLink'
import { VirtualList } from '@/components/VirtualList'
import { SENTIMENT_LABEL } from '@/lib/assets'
import { formatDateTime, formatRelative } from '@/lib/format'
import { sourceName } from '@/lib/sources'

export const NewsRow = memo(function NewsRow({ n }: { n: NewsItem }) {
  const s = n.sentiment
  const cls = s ? (s.score > 0.15 ? 'pos' : s.score < -0.15 ? 'neg' : '') : ''
  return (
    <article className="news-item">
      <span className={`sent ${cls}`} aria-hidden="true" />
      <div className="stack grow" style={{ gap: '0.15rem' }}>
        <ExternalLink href={n.url} className="title">{n.title}</ExternalLink>
        {n.summary ? <p className="small muted" style={{ display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>{n.summary}</p> : null}
        <div className="row wrap tiny muted">
          <span>{n.publisher ?? sourceName(n.provider)}</span>
          <time dateTime={n.publishedAt} title={formatDateTime(n.publishedAt)}>· {formatRelative(n.publishedAt)}</time>
          <span>· vía {sourceName(n.provider)}</span>
          {s ? (
            <span className={`chip ${s.score > 0.15 ? 'up' : s.score < -0.15 ? 'down' : ''}`} title={`Puntuación ${s.score.toFixed(2)} · método: ${s.method}`}>
              {SENTIMENT_LABEL[s.label]}
            </span>
          ) : null}
        </div>
      </div>
    </article>
  )
})

export function NewsList({ items, height = 560 }: { items: NewsItem[]; height?: number }) {
  return <VirtualList items={items} estimate={92} height={height} label="Noticias" getKey={(n) => n.id} render={(n) => <NewsRow n={n} />} />
}

export function NewsPanel({ summary }: { summary: AssetSummary }) {
  const q = useNews(summary.id)
  const stats = useMemo(() => {
    const withS = (q.data ?? []).filter((n) => n.sentiment)
    if (withS.length === 0) return null
    const avg = withS.reduce((s, n) => s + (n.sentiment?.score ?? 0), 0) / withS.length
    return { avg, n: withS.length, pos: withS.filter((n) => (n.sentiment?.score ?? 0) > 0.15).length, neg: withS.filter((n) => (n.sentiment?.score ?? 0) < -0.15).length }
  }, [q.data])
  if (q.isPending) return <SkeletonRows rows={5} h="4rem" />
  if (q.isError) return <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudieron cargar las noticias" />
  if (q.data.length === 0) return <div className="empty">Sin noticias recientes para este activo.</div>
  return (
    <div className="stack">
      {stats ? (
        <p className="small muted">
          Sentimiento medio de {stats.n} titulares con valoración: <strong className={stats.avg > 0.15 ? 'up' : stats.avg < -0.15 ? 'down' : ''}>{stats.avg.toFixed(2)}</strong>{' '}
          ({stats.pos} positivos, {stats.neg} negativos). El sentimiento lo calcula el proveedor, un léxico o el modelo local; pasa el ratón sobre la etiqueta para ver el método.
        </p>
      ) : null}
      <NewsList items={q.data} />
    </div>
  )
}
