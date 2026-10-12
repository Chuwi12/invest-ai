import { memo } from 'react'
import type { AssetClass, Quote } from '@/api/types'
import { NBSP, formatChange, formatPrice, toneOf } from '@/lib/format'
import { mergeLive } from '@/lib/quote'
import { useLivePrice } from '@/realtime/hooks'
import { Delta } from './Delta'

interface Props {
  quote: Quote | null | undefined
  assetClass?: AssetClass | undefined
  currency?: string | null | undefined
  /** Muestra la variación absoluta además de la relativa. */
  showAbs?: boolean
  className?: string
  /** Si es false no muestra el indicador de tiempo real. */
  dot?: boolean
}

/**
 * Precio + variación en vivo. Es el ÚNICO componente que se re-renderiza con cada tick de su
 * activo (useSyncExternalStore por id), así que va memoizado y sin hijos pesados.
 */
export const LivePrice = memo(function LivePrice({ quote, assetClass, currency, showAbs, className = '', dot = true }: Props) {
  const id = quote?.id
  const tick = useLivePrice(id)
  const m = mergeLive(quote, tick)
  const cls = assetClass ?? (id?.slice(0, id.indexOf(':')) as AssetClass | undefined)
  // Un índice se mide en puntos y un bono en rendimiento (%): no son importes en moneda.
  const noMoney = cls === 'index' || cls === 'bond'
  const cur = noMoney ? undefined : (currency ?? quote?.currency ?? undefined)
  const flash = tick && tick.dir !== 'flat' ? `flash-${tick.dir}` : ''
  return (
    <span className={`num ${className}`} style={{ display: 'inline-flex', alignItems: 'baseline', gap: '0.5em', flexWrap: 'wrap' }}>
      <span key={tick?.seq ?? 0} className={`tick ${flash}`} data-testid="live-price">
        {formatPrice(m.price, { class: cls, currency: cur })}{cls === 'bond' ? `${NBSP}%` : ''}
      </span>
      <span className={toneOf(m.changePct)} style={{ fontSize: '0.8em' }}>
        {showAbs ? <span className="num">{formatChange(m.change, assetClass)} </span> : null}
        <Delta points={m.changePct} />
      </span>
      {dot ? (
        <span
          className={`livedot ${m.live ? '' : 'off'}`}
          role="img"
          aria-label={m.live ? 'Actualizándose en tiempo real' : 'Sin actualización en tiempo real'}
          title={m.live ? 'Tiempo real (WebSocket)' : 'Sin ticks en vivo'}
        />
      ) : null}
    </span>
  )
})
