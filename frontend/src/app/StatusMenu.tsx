import { Activity } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useStatus } from '@/api/hooks'
import type { ProviderStatus } from '@/api/types'
import { formatInt, formatRelative } from '@/lib/format'
import { sourceName } from '@/lib/sources'
import { useConnectionState } from '@/realtime/hooks'

const WS_LABEL = { idle: 'En reposo (sin suscripciones)', connecting: 'Conectando…', open: 'Conectado', reconnecting: 'Reconectando…' } as const

/** Indicador de estado: WebSocket, LLM y proveedores (`/status`). Se abre como popover accesible. */
export function StatusMenu() {
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  const ws = useConnectionState()
  const { data, isError } = useStatus()

  useEffect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false)
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const providers = data?.providers.filter((p) => p.enabled) ?? []
  const failing = providers.filter((p) => p.lastError || p.blockedSecs)
  const llmOk = data?.llm.available ?? false
  const wsOk = ws === 'open' || ws === 'idle'
  const level = isError || (data && !llmOk) || ws === 'reconnecting' ? 'bad' : failing.length > 0 || ws === 'connecting' ? 'wait' : data ? 'ok' : 'wait'

  return (
    <div ref={box} style={{ position: 'relative' }}>
      <button type="button" className="btn sm" aria-expanded={open} aria-haspopup="true" onClick={() => setOpen((o) => !o)}>
        <span className={`dot ${level}`} aria-hidden="true" />
        <Activity aria-hidden="true" />
        <span className="small">Estado</span>
        <span className="sr-only">: {level === 'ok' ? 'todo correcto' : level === 'wait' ? 'con avisos' : 'con problemas'}</span>
      </button>
      {open ? (
        <div className="popover" role="region" aria-label="Estado de los servicios">
          <Row ok={wsOk} label="Tiempo real (WebSocket)" value={WS_LABEL[ws]} />
          {isError ? (
            <Row ok={false} label="Backend" value="No responde" />
          ) : data ? (
            <>
              <Row
                ok={llmOk}
                label="IA local (Ollama)"
                value={llmOk ? `${data.llm.model}${data.llm.supportsTools ? '' : ' · sin herramientas'}` : (data.llm.error ?? 'No disponible')}
              />
              <div className="small muted">Proveedores de datos</div>
              <ul className="stack" style={{ gap: '0.3rem' }}>
                {data.providers.map((p) => (
                  <li key={p.name} className="row small" title={[p.lastError, p.blockedSecs ? `En pausa ${p.blockedSecs} s tras un límite de peticiones` : null, p.latencyMs != null ? `Última petición: ${p.latencyMs} ms` : null].filter(Boolean).join(' · ') || undefined}>
                    <span className={`dot ${!p.enabled ? '' : p.lastError || p.blockedSecs ? 'wait' : 'ok'}`} aria-hidden="true" />
                    <span className="grow">{sourceName(p.name)}</span>
                    <span className="muted tiny trunc" style={{ maxWidth: '14rem' }}>
                      {providerDetail(p)}
                    </span>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <div className="muted small">Consultando…</div>
          )}
        </div>
      ) : null}
    </div>
  )
}

function providerDetail(p: ProviderStatus): string {
  if (!p.enabled) return p.requiresKey && !p.hasKey ? 'sin clave' : 'desactivado'
  const parts: string[] = []
  if (p.blockedSecs) parts.push(`en pausa ${formatInt(p.blockedSecs)} s`)
  if (p.lastError) parts.push(`error: ${p.lastError}`)
  else if (p.lastOk) parts.push(formatRelative(p.lastOk))
  else parts.push('sin uso')
  if (p.latencyMs !== null && p.latencyMs !== undefined) parts.push(`${formatInt(p.latencyMs)} ms`)
  return parts.join(' · ')
}

function Row({ ok, label, value }: { ok: boolean; label: string; value: string }) {
  return (
    <div className="row small">
      <span className={`dot ${ok ? 'ok' : 'bad'}`} aria-hidden="true" />
      <span className="grow">{label}</span>
      <span className="muted trunc" style={{ maxWidth: '14rem' }}>
        {value}
      </span>
    </div>
  )
}
