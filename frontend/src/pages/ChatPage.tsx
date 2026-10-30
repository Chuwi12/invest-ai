import { Pin, PinOff, Plus, Send, Square, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router'
import { sortConversations, useConversation, useConversations, useCreateConversation, useDeleteConversation, useStatus, useUpdateConversation } from '@/api/hooks'
import type { AssetId, Conversation } from '@/api/types'
import { ASSET_CLASSES } from '@/api/types'
import { AnalysisReport } from '@/components/AnalysisReport'
import { Markdown } from '@/components/Markdown'
import { MessageView } from '@/components/chat/MessageView'
import { ToolCallList } from '@/components/chat/ToolCalls'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { VirtualList } from '@/components/VirtualList'
import { useChatStream } from '@/hooks/useChatStream'
import { useTitle } from '@/hooks/useTitle'
import { assetHref, symbolOf } from '@/lib/assets'
import { formatRelative } from '@/lib/format'

const STARTERS = [
  '¿Es buen momento para entrar en Apple?',
  'Compara Bitcoin con Ethereum según datos de hoy',
  '¿Qué riesgos macro pesan ahora sobre el S&P 500?',
  'Resume las noticias recientes de Nvidia y su sentimiento',
]

function ConversationItem({ c, active, onDelete }: { c: Conversation; active: boolean; onDelete: (id: string) => void }) {
  const upd = useUpdateConversation()
  const [confirm, setConfirm] = useState(false)
  return (
    <div className="conv" aria-current={active ? 'page' : undefined}>
      <div className="row">
        <Link to={`/chat/${c.id}`} className="grow trunc" style={{ fontWeight: 500 }} aria-current={active ? 'page' : undefined}>{c.title || 'Sin título'}</Link>
        <button type="button" className="btn icon sm ghost" aria-label={c.pinned ? 'Desfijar conversación' : 'Fijar conversación'} aria-pressed={c.pinned} onClick={() => upd.mutate({ cid: c.id, body: { pinned: !c.pinned } })}>
          {c.pinned ? <PinOff aria-hidden="true" /> : <Pin aria-hidden="true" />}
        </button>
        {confirm ? (
          <button type="button" className="btn sm danger" onClick={() => onDelete(c.id)} onBlur={() => setConfirm(false)} autoFocus>¿Borrar?</button>
        ) : (
          <button type="button" className="btn icon sm ghost" aria-label={`Borrar conversación ${c.title}`} onClick={() => setConfirm(true)}><Trash2 aria-hidden="true" /></button>
        )}
      </div>
      <span className="tiny muted trunc">{formatRelative(c.updatedAt)}{c.lastMessagePreview ? ` · ${c.lastMessagePreview}` : ''}</span>
    </div>
  )
}

export default function ChatPage() {
  const { cid } = useParams()
  const [search] = useSearchParams()
  const navigate = useNavigate()
  const convs = useConversations()
  const thread = useConversation(cid)
  const create = useCreateConversation()
  const del = useDeleteConversation()
  const status = useStatus()
  const { state, send, abort, clear } = useChatStream()
  const [draft, setDraft] = useState('')
  const [ctxIds, setCtxIds] = useState<AssetId[]>(() =>
    search.getAll('asset').filter((a): a is AssetId => ASSET_CLASSES.includes(a.slice(0, a.indexOf(':')) as never)),
  )
  const endRef = useRef<HTMLDivElement>(null)
  const scroller = useRef<HTMLDivElement>(null)
  const justCreated = useRef<string | null>(null)

  const conv = thread.data?.conversation
  useTitle(conv?.title || 'Chat')

  // Al cambiar de conversación (que no sea la recién creada) se descarta lo que se estaba escribiendo/recibiendo.
  useEffect(() => {
    if (cid && justCreated.current === cid) return
    clear()
  }, [cid, clear])

  const streaming = !!state && !state.done && !state.error && state.cid === cid
  const messages = useMemo(() => (thread.data?.messages ?? []).filter((m) => m.role === 'user' || m.role === 'assistant'), [thread.data])
  const assets = conv?.assetIds ?? ctxIds
  const live = state && state.cid === cid ? state : null
  // El hilo guardado ya trae la pregunta (base + 1) y la respuesta, entera o truncada (base + 2):
  // la burbuja en vivo correspondiente sobra (si no, se pinta dos veces tras un error o al detener).
  const userSaved = !!live && messages.length >= live.base + 1
  const answerSaved = !!live && messages.length >= live.base + 2
  const nearBottom = () => {
    const el = scroller.current
    return !el || el.scrollHeight - el.scrollTop - el.clientHeight < 140
  }
  const lastLen = (live?.text.length ?? 0) + (live?.tools.length ?? 0) + messages.length
  useEffect(() => {
    if (nearBottom()) endRef.current?.scrollIntoView({ block: 'end' })
  }, [lastLen])

  const submit = async () => {
    const content = draft.trim()
    if (!content || streaming) return
    setDraft('')
    let target = cid
    if (!target) {
      const c = await create.mutateAsync({ assetIds: ctxIds })
      target = c.id
      justCreated.current = c.id
      void navigate(`/chat/${c.id}`, { replace: true })
    }
    void send(target, content, assets.length ? assets : undefined)
  }

  const onKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault()
      void submit()
    }
  }

  const sorted = useMemo(() => sortConversations(convs.data ?? []), [convs.data])
  const llmDown = status.data ? !status.data.llm.available : false
  const empty = !cid || (messages.length === 0 && !live)

  return (
    <div className="chat-layout" data-has-thread={cid ? 'true' : 'false'}>
      <aside className="chat-list" aria-label="Conversaciones">
        <Link to="/chat" className="btn primary" onClick={() => { clear(); setCtxIds([]) }}><Plus aria-hidden="true" /> Nueva conversación</Link>
        {convs.isPending ? <SkeletonRows rows={4} h="2.6rem" /> : null}
        {convs.isError ? <ErrorNotice error={convs.error} onRetry={() => void convs.refetch()} /> : null}
        {convs.data && sorted.length === 0 ? <div className="empty small">Sin conversaciones todavía.</div> : null}
        {sorted.length > 0 ? (
          <VirtualList
            items={sorted}
            estimate={58}
            height="100%"
            label="Conversaciones guardadas"
            getKey={(c) => c.id}
            render={(c) => <ConversationItem c={c} active={c.id === cid} onDelete={(id) => { del.mutate(id); if (id === cid) void navigate('/chat') }} />}
          />
        ) : null}
      </aside>

      <section className="chat-thread" aria-label="Conversación">
        <div className="row" style={{ padding: '0.5rem 0.8rem', borderBottom: '1px solid var(--line)' }}>
          <Link to="/chat" className="btn sm ghost" style={{ display: cid ? undefined : 'none' }}>Conversaciones</Link>
          <strong className="grow trunc">{conv?.title || (cid ? '…' : 'Nueva conversación')}</strong>
          {assets.map((a) => (
            <Link key={a} to={assetHref(a)} className="chip accent" title={`Contexto: ${a}`}>{symbolOf(a)}</Link>
          ))}
          {!cid && ctxIds.length > 0 ? <button type="button" className="btn sm ghost" onClick={() => setCtxIds([])}>Quitar contexto</button> : null}
        </div>

        <div className="msgs" ref={scroller} role="log" aria-live="polite" aria-label="Mensajes">
          {cid && thread.isPending ? <SkeletonRows rows={3} h="3rem" /> : null}
          {cid && thread.isError ? <ErrorNotice error={thread.error} onRetry={() => void thread.refetch()} title="No se pudo abrir la conversación" /> : null}
          {/* Sin `cid` la query está desactivada y sigue en `isPending`: el estado vacío no puede esperar a ella. */}
          {empty && !(cid && thread.isPending) ? (
            <div className="stack" style={{ margin: 'auto', maxWidth: '34rem', textAlign: 'center', alignItems: 'center' }}>
              <h1>Pregunta sobre cualquier activo</h1>
              <p className="muted small">
                El modelo local consulta datos en vivo (cotización, indicadores, fundamentales, noticias, macro) y te muestra qué ha consultado y de dónde.
                {ctxIds.length ? ` Contexto: ${ctxIds.map(symbolOf).join(', ')}.` : ''}
              </p>
              <div className="row wrap" style={{ justifyContent: 'center' }}>
                {STARTERS.map((s) => <button key={s} type="button" className="chip" onClick={() => setDraft(s)}>{s}</button>)}
              </div>
            </div>
          ) : null}
          {messages.map((m) => <MessageView key={m.id} m={m} />)}
          {live && !answerSaved ? (
            <>
              {!live.done && !live.error && !userSaved ? <div className="msg user"><div style={{ whiteSpace: 'pre-wrap' }}>{live.user}</div></div> : null}
              <div className="msg assistant" aria-busy={streaming}>
                <ToolCallList calls={live.tools} />
                {live.thinking && !live.text ? (
                  <details className="tool"><summary>Razonando…</summary><div className="body"><pre>{live.thinking}</pre></div></details>
                ) : null}
                {live.text ? <Markdown text={live.text} streaming={streaming} /> : streaming && live.tools.length === 0 ? <span className="muted small caret">Pensando</span> : null}
                {live.analysis ? <div className="panel"><AnalysisReport analysis={live.analysis} compact /></div> : null}
                {live.error ? <ErrorNotice error={new Error(live.error)} title="La respuesta se interrumpió" /> : null}
              </div>
            </>
          ) : null}
          <div ref={endRef} />
        </div>

        {llmDown ? <div className="notice warn" role="status" style={{ margin: '0 0.75rem' }}>La IA local no está disponible; el mensaje no podrá responderse hasta que arranque Ollama.</div> : null}
        <form className="composer" onSubmit={(e) => { e.preventDefault(); void submit() }}>
          <label className="sr-only" htmlFor="chat-input">Mensaje</label>
          <textarea id="chat-input" rows={2} value={draft} onChange={(e) => setDraft(e.target.value)} onKeyDown={onKey} placeholder="Escribe tu pregunta (Intro envía, Mayús+Intro salto de línea)" />
          {streaming ? (
            <button type="button" className="btn" onClick={abort}><Square aria-hidden="true" /> Detener</button>
          ) : (
            <button type="submit" className="btn primary" disabled={!draft.trim() || create.isPending}><Send aria-hidden="true" /> Enviar</button>
          )}
        </form>
      </section>
    </div>
  )
}
