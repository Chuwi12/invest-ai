import { Clock, Search, Star } from 'lucide-react'
import { useDeferredValue, useEffect, useId, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router'
import { useFavorites, useRecordSearch, useSearch, useSearchHistory } from '@/api/hooks'
import type { AssetSummary } from '@/api/types'
import { assetHref, CLASS_LABEL, CLASS_ORDER } from '@/lib/assets'

interface Entry {
  summary: AssetSummary
  kind: 'result' | 'favorite'
}

/** Buscador global (Ctrl+K o «/»): resultados agrupados por clase de activo, navegable con teclado. */
export function SearchPalette({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('')
  const [debounced, setDebounced] = useState('')
  const [active, setActive] = useState(0)
  const navigate = useNavigate()
  const record = useRecordSearch()
  const listId = useId()
  const inputRef = useRef<HTMLInputElement>(null)
  const dq = useDeferredValue(debounced)

  useEffect(() => {
    const t = setTimeout(() => setDebounced(q), 180)
    return () => clearTimeout(t)
  }, [q])
  useEffect(() => inputRef.current?.focus(), [])
  // Devuelve el foco a lo que lo tenía al abrir.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null
    return () => prev?.focus?.()
  }, [])

  const term = q.trim()
  const search = useSearch(dq)
  const favs = useFavorites()
  const hist = useSearchHistory(term.length < 2)

  const groups = useMemo(() => {
    if (term.length < 2) return [] as { label: string; items: Entry[] }[]
    const by = new Map<string, Entry[]>()
    for (const s of search.data ?? []) {
      const arr = by.get(s.class) ?? []
      arr.push({ summary: s, kind: 'result' })
      by.set(s.class, arr)
    }
    return CLASS_ORDER.filter((c) => by.has(c)).map((c) => ({ label: CLASS_LABEL[c], items: by.get(c)! }))
  }, [search.data, term])

  const idleItems: Entry[] = useMemo(
    () => (favs.data ?? []).slice(0, 6).map((f) => ({ summary: f.summary, kind: 'favorite' as const })),
    [favs.data],
  )
  const flat: Entry[] = term.length < 2 ? idleItems : groups.flatMap((g) => g.items)
  const sel = Math.min(active, Math.max(0, flat.length - 1))
  // Término al que corresponden los resultados que se ven. Mientras se escribe (antirrebote) o llega la
  // respuesta siguen los de la búsqueda anterior (`keepPreviousData`): Enter no debe elegir uno de ellos
  // (acababa en la ficha de «Tesco» al teclear «tesla») ni anotar en el historial «tesla → Tesco».
  const shownFor = dq.trim()
  const fresh = term.length < 2 || (shownFor === term && !search.isPlaceholderData)

  const choose = (e: Entry) => {
    if (term.length >= 2) record.mutate({ query: e.kind === 'result' ? shownFor : term, selected: e.summary.id })
    onClose()
    void navigate(assetHref(e.summary.id))
  }

  const onKey = (ev: React.KeyboardEvent) => {
    if (ev.key === 'ArrowDown') {
      ev.preventDefault()
      setActive((sel + 1) % Math.max(1, flat.length))
    } else if (ev.key === 'ArrowUp') {
      ev.preventDefault()
      setActive((sel - 1 + flat.length) % Math.max(1, flat.length))
    } else if (ev.key === 'Enter') {
      const e = flat[sel]
      if (e) {
        ev.preventDefault()
        if (fresh) choose(e)
      }
    } else if (ev.key === 'Escape') {
      ev.preventDefault()
      onClose()
    } else if (ev.key === 'Tab') {
      ev.preventDefault() // trampa de foco mínima: todo se hace desde el campo
    }
  }

  const renderOpt = (e: Entry) => {
    const i = flat.indexOf(e)
    return (
      <div
        key={`${e.kind}-${e.summary.id}`}
        id={`${listId}-${i}`}
        role="option"
        aria-selected={i === sel}
        onMouseMove={() => setActive(i)}
        onClick={() => choose(e)}
      >
        {e.kind === 'favorite' ? <Star size={14} aria-hidden="true" /> : null}
        <strong style={{ minWidth: '5rem' }}>{e.summary.symbol}</strong>
        <span className="grow trunc">{e.summary.name}</span>
        <span className="muted tiny">{e.summary.exchange ?? e.summary.class}</span>
      </div>
    )
  }

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Buscar activos">
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={flat.length ? `${listId}-${sel}` : undefined}
          aria-autocomplete="list"
          placeholder="Buscar acciones, ETFs, cripto, índices, divisas…"
          value={q}
          onChange={(e) => {
            setQ(e.target.value)
            setActive(0)
          }}
          onKeyDown={onKey}
          autoComplete="off"
          spellCheck={false}
        />
        <div className="list" id={listId} role="listbox" aria-label="Resultados">
          {term.length < 2 ? (
            <>
              {idleItems.length > 0 ? <div className="grp">Favoritos</div> : null}
              {idleItems.map(renderOpt)}
              {hist.data && hist.data.length > 0 ? (
                <>
                  <div className="grp">Búsquedas recientes</div>
                  <div className="row wrap" style={{ padding: '0.2rem 1rem' }}>
                    {hist.data.slice(0, 8).map((h) => (
                      <button key={`${h.query}-${h.at}`} type="button" className="chip" onClick={() => { setQ(h.query); setDebounced(h.query); inputRef.current?.focus() }}>
                        <Clock aria-hidden="true" /> {h.query}
                      </button>
                    ))}
                  </div>
                </>
              ) : null}
              {idleItems.length === 0 && !hist.data?.length ? <div className="empty small">Escribe al menos 2 letras: nombre, símbolo o ISIN.</div> : null}
            </>
          ) : search.isError ? (
            <div className="empty small" role="alert">No se pudo buscar. Revisa la conexión con el servidor.</div>
          ) : search.isFetching && flat.length === 0 ? (
            <div className="empty small" role="status">Buscando…</div>
          ) : flat.length === 0 && dq === term ? (
            <div className="empty small">Sin resultados para «{term}».</div>
          ) : (
            groups.map((g) => (
              <div key={g.label} role="group" aria-label={g.label}>
                <div className="grp" aria-hidden="true">{g.label}</div>
                {g.items.map(renderOpt)}
              </div>
            ))
          )}
        </div>
        <div className="row small muted" style={{ padding: '0.4rem 1rem', borderTop: '1px solid var(--line)' }}>
          <Search size={13} aria-hidden="true" /> <span>↑↓ para moverte · Intro para abrir · Esc para cerrar</span>
        </div>
      </div>
    </div>
  )
}
