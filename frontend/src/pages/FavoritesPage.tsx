import { Trash2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useFavorites, useSetFavoriteNote, useToggleFavorite, useQuotes } from '@/api/hooks'
import type { Favorite } from '@/api/types'
import { WatchTable } from '@/components/WatchTable'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { formatDate } from '@/lib/format'
import { useTitle } from '@/hooks/useTitle'

function NoteEditor({ f }: { f: Favorite }) {
  const [v, setV] = useState(f.note ?? '')
  const save = useSetFavoriteNote()
  return (
    <form
      className="row"
      onSubmit={(e) => { e.preventDefault(); if (v !== (f.note ?? '')) save.mutate({ id: f.id, note: v }) }}
    >
      <label className="sr-only" htmlFor={`note-${f.id}`}>Nota de {f.summary.symbol}</label>
      <input id={`note-${f.id}`} type="text" className="grow" value={v} maxLength={300} placeholder="Añade una nota" onChange={(e) => setV(e.target.value)} onBlur={() => { if (v !== (f.note ?? '')) save.mutate({ id: f.id, note: v }) }} />
      <span className="tiny muted" style={{ whiteSpace: 'nowrap' }}>desde {formatDate(f.addedAt)}</span>
    </form>
  )
}

export default function FavoritesPage() {
  useTitle('Favoritos')
  const favs = useFavorites()
  const ids = useMemo(() => (favs.data ?? []).map((f) => f.id), [favs.data])
  const quotes = useQuotes(ids)
  const toggle = useToggleFavorite()
  const rows = useMemo(
    () => (favs.data ?? []).map((f) => ({
      summary: f.summary,
      quote: quotes.data?.find((q) => q.id === f.id) ?? null,
      extra: (
        <button type="button" className="btn sm danger" aria-label={`Quitar ${f.summary.symbol} de favoritos`} onClick={() => toggle.mutate({ summary: f.summary, on: false })}>
          <Trash2 aria-hidden="true" /> Quitar
        </button>
      ),
    })),
    [favs.data, quotes.data, toggle],
  )
  return (
    <div className="page">
      <div className="page-head"><h1>Favoritos</h1></div>
      {favs.isPending ? <SkeletonRows rows={5} /> : null}
      {favs.isError ? <ErrorNotice error={favs.error} onRetry={() => void favs.refetch()} /> : null}
      {favs.data && favs.data.length === 0 ? <div className="empty">No tienes favoritos. Busca un activo (pulsa «/») y márcalo con la estrella.</div> : null}
      {favs.data && favs.data.length > 0 ? (
        <>
          <WatchTable rows={rows} hasExtra caption="Favoritos" />
          <section className="section" aria-label="Notas">
            <header><h2>Notas</h2><span className="hint">Se guardan al salir del campo</span></header>
            <ul>
              {favs.data.map((f) => (
                <li key={f.id} className="row" style={{ padding: '0.35rem 0', borderBottom: '1px solid var(--line)' }}>
                  <strong style={{ width: '6rem' }}>{f.summary.symbol}</strong>
                  <div className="grow"><NoteEditor f={f} /></div>
                </li>
              ))}
            </ul>
          </section>
        </>
      ) : null}
    </div>
  )
}
