import { memo, type ReactNode } from 'react'
import type { AssetSummary, Quote } from '@/api/types'
import { assetHrefLabel } from '@/lib/assets'
import { quoteFreshness } from '@/lib/freshness'
import { formatRelative } from '@/lib/format'
import { sourceName } from '@/lib/sources'
import { AssetCell } from './AssetCell'
import { LivePrice } from './LivePrice'
import { Sparkline } from './Sparkline'

export interface WatchRow {
  summary: AssetSummary
  quote: Quote | null
  extra?: ReactNode
}

const Row = memo(function Row({ row }: { row: WatchRow }) {
  const f = quoteFreshness(row.quote)
  return (
    <tr>
      <td><AssetCell id={row.summary.id} name={row.summary.name} /></td>
      <td className="r">
        {row.quote ? <LivePrice quote={row.quote} assetClass={row.summary.class} currency={row.summary.currency} dot={false} /> : <span className="muted">sin dato</span>}
      </td>
      <td className="r" aria-label={`Tendencia de ${assetHrefLabel(row.summary.id)}`}><Sparkline id={row.summary.id} /></td>
      <td className="small muted">
        {row.quote ? (
          <span title={`Cotización de ${sourceName(row.quote.source)} · ${row.quote.timestamp}`}>
            {sourceName(row.quote.source)} · {formatRelative(row.quote.timestamp)}
            {f.stale ? <span className="stale-badge" style={{ marginLeft: 4 }} title={f.reason ?? ''}>retrasado</span> : null}
          </span>
        ) : '—'}
      </td>
      {row.extra !== undefined ? <td className="r">{row.extra}</td> : null}
    </tr>
  )
})

export function WatchTable({ rows, hasExtra, caption }: { rows: WatchRow[]; hasExtra?: boolean; caption: string }) {
  return (
    <div className="table-wrap">
      <table className="tbl">
        <caption className="sr-only">{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Activo</th><th scope="col" className="r">Precio</th><th scope="col" className="r">1 mes</th><th scope="col">Fuente</th>
            {hasExtra ? <th scope="col" className="r"><span className="sr-only">Acciones</span></th> : null}
          </tr>
        </thead>
        <tbody>{rows.map((r) => <Row key={r.summary.id} row={r} />)}</tbody>
      </table>
    </div>
  )
}
