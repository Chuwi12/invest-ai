import { useQueryClient } from '@tanstack/react-query'
import { memo } from 'react'
import { Link } from 'react-router'
import { prefetchAsset } from '@/api/hooks'
import type { AssetId } from '@/api/types'
import { assetHref, symbolOf } from '@/lib/assets'

export const AssetCell = memo(function AssetCell({ id, name }: { id: AssetId; name?: string | null | undefined }) {
  const qc = useQueryClient()
  return (
    <Link
      to={assetHref(id)}
      className="asset-link"
      onMouseEnter={() => prefetchAsset(qc, id)}
      onFocus={() => prefetchAsset(qc, id)}
    >
      <span className="sym">{symbolOf(id)}</span>
      {name ? <span className="nm trunc">{name}</span> : null}
    </Link>
  )
})
