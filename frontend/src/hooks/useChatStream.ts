import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '@/api'
import { isAbortError } from '@/api/client'
import { qk } from '@/api/hooks'
import type { Analysis, AssetId, ConversationWithMessages, ToolCallRecord } from '@/api/types'
import { errorText } from '@/lib/errors'

export interface StreamState {
  cid: string
  user: string
  text: string
  thinking: string
  tools: ToolCallRecord[]
  analysis: Analysis | null
  error: string | null
  done: boolean
  /**
   * Mensajes (usuario + asistente) que ya tenía guardados el hilo al enviar. Cuando el hilo guardado
   * pasa de `base + 2`, el servidor ya tiene esta respuesta (entera o truncada por un error) y la
   * burbuja en vivo sobra; si no se pintaría dos veces. `Infinity` = se desconocía (no se oculta).
   */
  base: number
}

/** Mensajes visibles (usuario y asistente) de un hilo cargado. */
export function visibleCount(thread: ConversationWithMessages | undefined): number {
  return thread ? thread.messages.filter((m) => m.role === 'user' || m.role === 'assistant').length : Number.POSITIVE_INFINITY
}

/** Estado de UNA respuesta en curso (SSE). Los tokens se agrupan por fotograma para no re-renderizar por cada trozo. */
export function useChatStream() {
  const qc = useQueryClient()
  const [state, setState] = useState<StreamState | null>(null)
  const ctrl = useRef<AbortController | null>(null)
  const buf = useRef({ text: '', thinking: '' })
  const raf = useRef(0)

  const flush = useCallback(() => {
    raf.current = 0
    const { text, thinking } = buf.current
    setState((s) => (s ? { ...s, text, thinking } : s))
  }, [])
  const schedule = useCallback(() => {
    if (!raf.current) raf.current = requestAnimationFrame(flush)
  }, [flush])

  const abort = useCallback(() => ctrl.current?.abort(), [])
  useEffect(() => () => { ctrl.current?.abort(); cancelAnimationFrame(raf.current) }, [])

  const send = useCallback(
    async (cid: string, content: string, assetIds?: AssetId[]) => {
      ctrl.current?.abort()
      const ac = new AbortController()
      ctrl.current = ac
      // ¿Sigue siendo ésta la respuesta vigente? Otra `send` (o `clear`) la sustituye; sus eventos
      // pendientes (un chunk de red trae varios) ya no deben tocar ni el búfer ni el estado.
      const current = () => ctrl.current === ac
      buf.current = { text: '', thinking: '' }
      const base = visibleCount(qc.getQueryData<ConversationWithMessages>(qk.conversation(cid)))
      setState({ cid, user: content, text: '', thinking: '', tools: [], analysis: null, error: null, done: false, base })
      const patch = (f: (s: StreamState) => StreamState) => setState((s) => (s && s.cid === cid ? f(s) : s))
      try {
        for await (const ev of api.sendMessage(cid, { content, ...(assetIds?.length ? { assetIds } : {}) }, ac.signal)) {
          if (!current()) break
          switch (ev.type) {
            case 'token': buf.current.text += ev.text; schedule(); break
            case 'thinking': buf.current.thinking += ev.text; schedule(); break
            case 'tool_call':
              patch((s) => ({ ...s, tools: [...s.tools, { id: ev.id, name: ev.name, arguments: ev.arguments, resultSummary: null, ok: null }] }))
              break
            case 'tool_result':
              patch((s) => ({ ...s, tools: s.tools.map((t) => (t.id === ev.id ? { ...t, ok: ev.ok, resultSummary: ev.summary } : t)) }))
              break
            case 'analysis': patch((s) => ({ ...s, analysis: ev.analysis })); break
            case 'error':
              flush()
              patch((s) => ({ ...s, error: ev.message }))
              void qc.invalidateQueries({ queryKey: qk.conversation(cid) })
              break
            case 'done': flush(); break
            default: break
          }
        }
        if (!current()) return
        // Refresca el hilo guardado y suelta el estado local cuando el servidor ya lo devuelve.
        await Promise.all([qc.refetchQueries({ queryKey: qk.conversation(cid) }), qc.invalidateQueries({ queryKey: qk.conversations })])
        setState((s) => (s && s.cid === cid && !s.error ? null : s))
      } catch (e) {
        if (isAbortError(e) || ac.signal.aborted) {
          void qc.invalidateQueries({ queryKey: qk.conversation(cid) })
          void qc.invalidateQueries({ queryKey: qk.conversations })
          if (!current()) return // sustituida por otra respuesta: el búfer y el estado ya son de ésta
          flush()
          setState((s) => (s && s.cid === cid ? { ...s, done: true } : s))
          return
        }
        if (!current()) return
        patch((s) => ({ ...s, error: errorText(e) }))
        void qc.invalidateQueries({ queryKey: qk.conversation(cid) })
      }
    },
    [qc, flush, schedule],
  )

  const clear = useCallback(() => { ctrl.current?.abort(); setState(null) }, [])
  return { state, send, abort, clear }
}
