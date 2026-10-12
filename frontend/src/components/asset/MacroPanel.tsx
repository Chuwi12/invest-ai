import { useMacro } from '@/api/hooks'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { Provenance } from '@/components/Provenance'
import { formatNumber, toneOf } from '@/lib/format'
import { macroFreshness } from '@/lib/freshness'

export function MacroPanel() {
  const q = useMacro()
  if (q.isPending) return <SkeletonRows rows={5} />
  if (q.isError) return <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudo cargar el contexto macro" />
  if (q.data.indicators.length === 0) return <div className="empty">Sin indicadores macro disponibles (FRED necesita clave de API).</div>
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr><th>Indicador</th><th className="r">Valor</th><th className="r">Cambio</th><th>Fuente</th></tr>
        </thead>
        <tbody>
          {q.data.indicators.map((m) => {
            const f = macroFreshness(m)
            return (
              <tr key={m.key}>
                <td>{m.name}</td>
                <td className="r num">{formatNumber(m.value, 2)} {m.unit}</td>
                <td className={`r num ${toneOf(m.change)}`}>{m.change === null ? '—' : formatNumber(m.change, 2)}</td>
                <td><Provenance provider={m.source} asOf={m.asOf} stale={f.stale} staleReason={f.reason} label="" /></td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
