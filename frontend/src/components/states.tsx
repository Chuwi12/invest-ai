import { AlertTriangle, RefreshCw } from 'lucide-react'
import type { CSSProperties, ReactNode } from 'react'
import { errorText } from '@/lib/errors'

export function Skeleton({ w, h = '1em', style }: { w?: string | number; h?: string | number; style?: CSSProperties }) {
  return <span className="skeleton" aria-hidden="true" style={{ display: 'block', width: w ?? '100%', height: h, ...style }} />
}

export function SkeletonRows({ rows = 5, h = '2.2rem' }: { rows?: number; h?: string }) {
  return (
    <div className="stack" role="status" aria-label="Cargando" style={{ gap: '0.5rem' }}>
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} h={h} />
      ))}
    </div>
  )
}

export function ErrorNotice({ error, onRetry, title }: { error: unknown; onRetry?: () => void; title?: string }) {
  return (
    <div className="notice error" role="alert">
      <AlertTriangle aria-hidden="true" />
      <div className="grow">
        {title ? <strong>{title}. </strong> : null}
        {errorText(error)}
      </div>
      {onRetry ? (
        <button type="button" className="btn sm" onClick={onRetry}>
          <RefreshCw aria-hidden="true" /> Reintentar
        </button>
      ) : null}
    </div>
  )
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="empty">{children}</div>
}

export function Section({ title, hint, children, id }: { title: string; hint?: ReactNode; children: ReactNode; id?: string }) {
  const hid = id ? `${id}-h` : undefined
  return (
    <section className="section" {...(hid ? { 'aria-labelledby': hid } : {})}>
      <header>
        <h2 id={hid}>{title}</h2>
        {hint ? <span className="hint">{hint}</span> : null}
      </header>
      {children}
    </section>
  )
}
