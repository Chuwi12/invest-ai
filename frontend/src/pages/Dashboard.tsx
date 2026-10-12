import { memo, useMemo } from 'react'
import { Link } from 'react-router'
import { useDashboard } from '@/api/hooks'
import type { Quote } from '@/api/types'
import { LivePrice } from '@/components/LivePrice'
import { NewsList } from '@/components/asset/NewsPanel'
import { Provenance } from '@/components/Provenance'
import { WatchTable } from '@/components/WatchTable'
import { ErrorNotice, Section, SkeletonRows } from '@/components/states'
import { MARKET_LABEL, VERDICT_LABEL, assetHref, symbolOf } from '@/lib/assets'
import { formatPercent, formatRelative } from '@/lib/format'
import { quoteFreshness } from '@/lib/freshness'
import { useTitle } from '@/hooks/useTitle'

const Tile = memo(function Tile({ q }: { q: Quote }) {
  const sym = symbolOf(q.id)
  const f = quoteFreshness(q)
  return (
    <Link to={assetHref(q.id)} className="market-tile" title={`${q.source} · ${q.timestamp}${f.stale ? ' · retrasado' : ''}`}>
      <span className="lbl">{MARKET_LABEL[sym] ?? sym}{f.stale ? <span className="stale-badge" style={{ marginLeft: 6 }}>retrasado</span> : null}</span>
      <LivePrice quote={q} className="px" dot={false} />
    </Link>
  )
})

export default function Dashboard() {
  useTitle('Panel')
  const q = useDashboard()
  const d = q.data

  const favRows = useMemo(() => (d?.favorites ?? []).map((f) => ({ summary: f.favorite.summary, quote: f.quote })), [d])
  const viewRows = useMemo(() => (d?.mostViewed ?? []).map((v) => ({ summary: v.stat.summary, quote: v.quote })), [d])
  const marketSources = useMemo(() => [...new Set((d?.market ?? []).map((m) => m.source))], [d])
  const oldest = useMemo(() => (d?.market ?? []).reduce<string | null>((o, m) => (o === null || m.timestamp < o ? m.timestamp : o), null), [d])
  const anyStale = (d?.market ?? []).some((m) => quoteFreshness(m).stale)

  return (
    <div className="page">
      <div className="page-head"><h1>Panel</h1></div>
      {q.isPending ? <SkeletonRows rows={5} h="3.2rem" /> : null}
      {q.isError ? <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudo cargar el panel" /> : null}
      {d ? (
        <>
          <Section title="Mercado" hint="Panel fijo, en vivo cuando hay ticks" id="market">
            <div className="market-strip">{d.market.map((m) => <Tile key={m.id} q={m} />)}</div>
            <Provenance provider={marketSources} asOf={oldest} label="Fuentes" stale={anyStale} extra="se muestra la cotización más antigua del panel" />
          </Section>

          <div className="grid main-side">
            <div className="stack" style={{ gap: '1.5rem' }}>
              <Section title="Favoritos" id="favs" hint={<Link to="/favorites" className="small" style={{ textDecoration: 'underline' }}>Gestionar</Link>}>
                {favRows.length ? <WatchTable rows={favRows} caption="Favoritos" /> : <div className="empty">Aún no tienes favoritos. Abre un activo y pulsa la estrella.</div>}
              </Section>
              <Section title="Más vistos" id="viewed" hint="Ponderado por recencia">
                {viewRows.length ? <WatchTable rows={viewRows} caption="Activos más vistos" /> : <div className="empty">Aún no has visitado ningún activo.</div>}
              </Section>
              <Section title="Titulares" id="news" hint="Mercado general">
                <NewsList items={d.news} height={420} />
              </Section>
            </div>

            <aside className="stack" style={{ gap: '1.5rem' }} aria-label="Actividad reciente">
              <Section title="Últimos análisis de IA" id="analyses">
                {d.latestAnalyses.length ? (
                  <ul>
                    {d.latestAnalyses.map((a) => (
                      <li key={a.id} className="lvl" style={{ alignItems: 'center' }}>
                        <Link to={`${assetHref(a.asset)}?tab=analysis`} className="asset-link">
                          <span className="sym">{symbolOf(a.asset)}</span>
                          <span className="nm">{formatRelative(a.generatedAt)} · {a.model}</span>
                        </Link>
                        <span className="row">
                          <span className={`chip ${a.verdict === 'buy' || a.verdict === 'accumulate' ? 'up' : a.verdict === 'reduce' || a.verdict === 'avoid' ? 'down' : ''}`}>{VERDICT_LABEL[a.verdict]}</span>
                          <span className="small muted num">{formatPercent(a.confidence, 0)}</span>
                        </span>
                      </li>
                    ))}
                  </ul>
                ) : <div className="empty small">Genera tu primer análisis desde la ficha de un activo.</div>}
              </Section>
              <Section title="Búsquedas recientes" id="searches">
                {d.recentSearches.length ? (
                  <div className="row wrap">
                    {d.recentSearches.map((s) => (
                      s.selected
                        ? <Link key={`${s.query}-${s.at}`} className="chip" to={assetHref(s.selected)} title={`Abrir ${s.selected}`}>{s.query}</Link>
                        : <span key={`${s.query}-${s.at}`} className="chip muted">{s.query}</span>
                    ))}
                  </div>
                ) : <div className="empty small">Tus búsquedas aparecerán aquí.</div>}
              </Section>
              <Section title="Conversaciones" id="convs" hint={<Link to="/chat" className="small" style={{ textDecoration: 'underline' }}>Abrir chat</Link>}>
                {d.recentConversations.length ? (
                  <ul>
                    {d.recentConversations.map((c) => (
                      <li key={c.id}>
                        <Link to={`/chat/${c.id}`} className="conv">
                          <span className="trunc" style={{ fontWeight: 500 }}>{c.title || 'Sin título'}</span>
                          <span className="tiny muted trunc">{c.lastMessagePreview ?? ''}</span>
                        </Link>
                      </li>
                    ))}
                  </ul>
                ) : <div className="empty small">Pregunta a la IA sobre cualquier activo.</div>}
              </Section>
            </aside>
          </div>
        </>
      ) : null}
    </div>
  )
}
