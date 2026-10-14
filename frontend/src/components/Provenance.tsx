import { Info } from 'lucide-react'
import { memo } from 'react'
import { useClock } from '@/hooks/useClock'
import { formatDateTime, formatRelative } from '@/lib/format'
import { safeHttpUrl, sourceName, sourceUrl } from '@/lib/sources'
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
  /**
   * URL de verificación REAL que da el backend (`sourceUrl`). Si existe, se muestra el enlace
   * «verificar en la fuente»; si no, no se inventa ninguno.
   */
  verifyUrl?: string | null | undefined
  /** `local` = la marca de tiempo es la hora de la consulta, no la del proveedor. */
  origin?: 'provider' | 'local' | null | undefined
  /** Texto previo, p. ej. «Fuente». */
  label?: string
  /** Información adicional (método, nº de velas...). */
  extra?: string | null | undefined
  /**
   * Nota SECUNDARIA con el retraso que declara el proveedor (tooltip de un icono «i»). La cifra principal es la
   * antigüedad medida (`asOf`); esto es solo lo que el proveedor dice de su feed y nunca se muestra como medida.
   */
  declaredDelayNote?: string | null | undefined
}

/**
 * Pie de procedencia: proveedor (con enlace cuando existe), antigüedad y marca «retrasado».
 * Toda cifra relevante de la UI cuelga de uno de estos.
 */
export const Provenance = memo(function Provenance({ provider, asOf, assetId, href, verifyUrl, origin, stale, staleReason, label = 'Fuente', extra, declaredDelayNote }: Props) {
  const now = useClock(30_000) // la antigüedad avanza aunque no lleguen datos nuevos
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
          · {formatRelative(asOf, now)}
        </time>
      ) : (
        <span>· sin fecha</span>
      )}
      {origin === 'local' ? (
        <span title="El proveedor no da hora del dato: es la hora a la que lo consultó la app">· hora de consulta</span>
      ) : null}
      {declaredDelayNote ? (
        <span className="prov-note" role="img" aria-label={declaredDelayNote} title={declaredDelayNote} tabIndex={0}>
          <Info aria-hidden="true" />
        </span>
      ) : null}
      {extra ? <span>· {extra}</span> : null}
      {safeHttpUrl(verifyUrl) ? (
        <span>
          · <ExternalLink href={verifyUrl} className="verify">verificar en la fuente ↗</ExternalLink>
        </span>
      ) : null}
      {stale ? (
        <span className="stale-badge" title={staleReason ?? 'Dato más antiguo de lo esperado'}>
          retrasado
        </span>
      ) : null}
    </span>
  )
})
