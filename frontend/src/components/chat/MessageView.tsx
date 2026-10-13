import { memo } from 'react'
import type { ChatMessage } from '@/api/types'
import { AnalysisReport } from '@/components/AnalysisReport'
import { Markdown } from '@/components/Markdown'
import { formatTime } from '@/lib/format'
import { ToolCallList } from './ToolCalls'

export const MessageView = memo(function MessageView({ m }: { m: ChatMessage }) {
  if (m.role === 'user') {
    return (
      <div className="msg user">
        <div style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{m.content}</div>
      </div>
    )
  }
  if (m.role !== 'assistant') return null
  return (
    <div className="msg assistant">
      <ToolCallList calls={m.toolCalls} />
      {m.content ? <Markdown text={m.content} /> : null}
      {m.analysis ? (
        <details open className="panel">
          <summary style={{ cursor: 'pointer', fontWeight: 600 }}>Análisis estructurado</summary>
          <div style={{ marginTop: '0.8rem' }}><AnalysisReport analysis={m.analysis} compact /></div>
        </details>
      ) : null}
      {m.truncated ? <div className="notice" role="status">Respuesta interrumpida por un error: el texto está incompleto.</div> : null}
      <span className="tiny muted">{formatTime(m.createdAt)}</span>
    </div>
  )
})
