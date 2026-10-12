import { AlertCircle, CheckCircle2, Loader2 } from 'lucide-react'
import { memo } from 'react'
import type { ToolCallRecord } from '@/api/types'
import { TOOL_LABEL, TOOL_SOURCE } from '@/lib/assets'

function argsLine(args: unknown): string {
  if (args && typeof args === 'object') {
    return Object.entries(args as Record<string, unknown>)
      .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
      .join(' · ')
  }
  return args === undefined || args === null ? '' : String(args)
}

export const ToolCall = memo(function ToolCall({ t }: { t: ToolCallRecord }) {
  const pending = t.ok === null
  const Icon = pending ? Loader2 : t.ok ? CheckCircle2 : AlertCircle
  return (
    <details className="tool">
      <summary>
        <Icon size={14} aria-hidden="true" className={pending ? 'spin' : t.ok ? 'up' : 'down'} style={pending ? { animation: 'spin 1s linear infinite' } : undefined} />
        <strong>{TOOL_LABEL[t.name] ?? t.name}</strong>
        <span className="muted trunc grow">{argsLine(t.arguments)}</span>
        <span className="sr-only">{pending ? 'en curso' : t.ok ? 'correcto' : 'con error'}</span>
      </summary>
      <div className="body">
        {t.resultSummary ? <div>{t.resultSummary}</div> : <div className="muted">{pending ? 'Esperando el resultado…' : 'Sin resumen.'}</div>}
        <div className="tiny muted">Origen: {TOOL_SOURCE[t.name] ?? 'proveedor de datos del backend'}</div>
        <pre>{JSON.stringify(t.arguments, null, 2)}</pre>
      </div>
    </details>
  )
})

export function ToolCallList({ calls }: { calls: ToolCallRecord[] }) {
  if (calls.length === 0) return null
  return (
    <div className="stack" style={{ gap: '0.3rem' }} role="list" aria-label="Datos que consultó el modelo">
      {calls.map((t) => <div role="listitem" key={t.id}><ToolCall t={t} /></div>)}
    </div>
  )
}
