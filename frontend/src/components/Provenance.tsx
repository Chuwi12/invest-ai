import { memo } from 'react'
import { formatDateTime, formatRelative } from '@/lib/format'
import { sourceName, sourceUrl } from '@/lib/sources'
import { ExternalLink } from './ExternalLink'

interface Props {
  provider: string | string[] | null | undefined
  /** Marca de tiempo del dato (ISO). */
  asOf?: string | null | undefined
  assetId?: string | undefined
  /** URL concreta de la fuente (si no, se deduce del proveedor). */
  href?: string | null | undefined
  stale?: boolean
  staleReason?: string | null | undefined
  /** Texto previo, p. ej. «Fuente». */
  label?: string
  /** Información adicional (retraso, método...). */
  extra?: string | null | undefined
}

/**
 * Pie de procedencia: proveedor (con enlace cuando existe), antigüedad y marca «retrasado».
 * Toda cifra relevante de la UI cuelga de uno de estos.
 */
export const Provenance = memo(function Provenance({ provider, asOf, assetId, href, stale, staleReason, label = 'Fuente', extra }: Props) {
  const list = Array.isArray(provider) ? provider : provider ? [provider] : []
  return (
    <span className="prov">
      {label ? <span>{label}:</span> : null}
      {list.length === 0 ? <span>no indicada</span> : null}
      {list.map((p, i) => (
        <span key={p}>
          <ExternalLink href={list.length === 1 && href ? href : sourceUrl(p, assetId)}>{sourceName(p)}</ExternalLink>
          {i < list.length - 1 ? ',' : ''}
        </span>
      ))}
      {asOf ? (
        <time dateTime={asOf} title={formatDateTime(asOf)}>
          · {formatRelative(asOf)}
        </time>
      ) : (
        <span>· sin fecha</span>
      )}
      {extra ? <span>· {extra}</span> : null}
      {stale ? (
        <span className="stale-badge" title={staleReason ?? 'Dato más antiguo de lo esperado'}>
          retrasado
        </span>
      ) : null}
    </span>
  )
})
