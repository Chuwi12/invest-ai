import { Star } from 'lucide-react'
import { useToggleFavorite } from '@/api/hooks'
import type { AssetSummary } from '@/api/types'

export function FavoriteButton({ summary, on, small }: { summary: AssetSummary; on: boolean; small?: boolean }) {
  const toggle = useToggleFavorite()
  return (
    <button
      type="button"
      className={`btn icon ${small ? 'sm' : ''} ${on ? '' : 'ghost'}`}
      aria-pressed={on}
      aria-label={on ? `Quitar ${summary.symbol} de favoritos` : `Añadir ${summary.symbol} a favoritos`}
      title={on ? 'Quitar de favoritos' : 'Añadir a favoritos'}
      disabled={toggle.isPending}
      onClick={() => toggle.mutate({ summary, on: !on })}
    >
      <Star fill={on ? 'currentColor' : 'none'} aria-hidden="true" />
    </button>
  )
}
