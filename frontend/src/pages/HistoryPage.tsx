import { Trash2 } from 'lucide-react'
import { Link } from 'react-router'
import { useClearSearchHistory, useClearViews, useSearchHistory, useViewStats } from '@/api/hooks'
import { VirtualList } from '@/components/VirtualList'
import { Empty, ErrorNotice, Section, SkeletonRows } from '@/components/states'
import { assetHref, symbolOf } from '@/lib/assets'
import { formatDateTime, formatInt, formatRelative } from '@/lib/format'
import { useTitle } from '@/hooks/useTitle'

export default function HistoryPage() {
  useTitle('Historial')
  const searches = useSearchHistory()
  const views = useViewStats()
  const clearS = useClearSearchHistory()
  const clearV = useClearViews()
  return (
    <div className="page">
      <div className="page-head"><h1>Historial</h1></div>
      <div className="grid cols-2" style={{ alignItems: 'start' }}>
        <Section
          title="Búsquedas"
          id="searches"
          hint={searches.data?.length ? <button type="button" className="btn sm" onClick={() => clearS.mutate()}><Trash2 aria-hidden="true" /> Borrar</button> : undefined}
        >
          {searches.isPending ? <SkeletonRows rows={5} h="2rem" /> : null}
          {searches.isError ? <ErrorNotice error={searches.error} onRetry={() => void searches.refetch()} /> : null}
          {searches.data && searches.data.length === 0 ? <Empty>Todavía no has buscado nada.</Empty> : null}
          {searches.data && searches.data.length > 0 ? (
            <VirtualList
              items={searches.data}
              estimate={44}
              height={480}
              label="Búsquedas recientes"
              getKey={(s, i) => `${s.query}-${s.at}-${i}`}
              render={(s) => (
                <div className="lvl" style={{ alignItems: 'center' }}>
                  <span className="grow trunc">{s.query}</span>
                  {s.selected ? <Link className="chip" to={assetHref(s.selected)}>{symbolOf(s.selected)}</Link> : <span className="tiny muted">sin elegir</span>}
                  <time className="tiny muted" dateTime={s.at} title={formatDateTime(s.at)}>{formatRelative(s.at)}</time>
                </div>
              )}
            />
          ) : null}
        </Section>
        <Section
          title="Más vistos"
          id="views"
          hint={views.data?.length ? <button type="button" className="btn sm" onClick={() => clearV.mutate()}><Trash2 aria-hidden="true" /> Borrar</button> : undefined}
        >
          {views.isPending ? <SkeletonRows rows={5} h="2rem" /> : null}
          {views.isError ? <ErrorNotice error={views.error} onRetry={() => void views.refetch()} /> : null}
          {views.data && views.data.length === 0 ? <Empty>Todavía no has visto ningún activo.</Empty> : null}
          {views.data && views.data.length > 0 ? (
            <VirtualList
              items={views.data}
              estimate={52}
              height={480}
              label="Activos más vistos"
              getKey={(v) => v.id}
              render={(v) => (
                <div className="lvl" style={{ alignItems: 'center' }}>
                  <Link to={assetHref(v.id)} className="asset-link grow"><span className="sym">{symbolOf(v.id)}</span><span className="nm trunc">{v.summary.name}</span></Link>
                  <span className="small num">{formatInt(v.views)} {v.views === 1 ? 'visita' : 'visitas'}</span>
                  <time className="tiny muted" dateTime={v.lastViewedAt} title={formatDateTime(v.lastViewedAt)}>{formatRelative(v.lastViewedAt)}</time>
                </div>
              )}
            />
          ) : null}
        </Section>
      </div>
    </div>
  )
}
