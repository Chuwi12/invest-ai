import { ArrowDownRight, ArrowUpRight, Minus } from 'lucide-react'
import { formatPoints, toneOf } from '@/lib/format'

/** Variación con signo, flecha y color: el color nunca es la única señal. */
export function Delta({ points, className = '' }: { points: number | null | undefined; className?: string }) {
  const tone = toneOf(points)
  const Icon = tone === 'up' ? ArrowUpRight : tone === 'down' ? ArrowDownRight : Minus
  return (
    <span className={`num ${tone} ${className}`} style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
      <Icon size={14} aria-hidden="true" />
      <span>{formatPoints(points)}</span>
      <span className="sr-only">{tone === 'up' ? ' (sube)' : tone === 'down' ? ' (baja)' : ''}</span>
    </span>
  )
}
