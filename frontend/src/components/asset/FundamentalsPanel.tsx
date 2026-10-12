import { useFundamentals } from '@/api/hooks'
import type { AssetSummary, Fundamentals } from '@/api/types'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { ExternalLink } from '@/components/ExternalLink'
import { Provenance } from '@/components/Provenance'
import { formatCompact, formatCompactCurrency, formatDate, formatInt, formatNumber, formatPercent, formatPrice } from '@/lib/format'
import { fundamentalsFreshness } from '@/lib/freshness'

type Row = [label: string, value: string | null]

function group(title: string, rows: Row[]) {
  const shown = rows.filter((r): r is [string, string] => r[1] !== null && r[1] !== '—')
  if (shown.length === 0) return null
  return (
    <section key={title} aria-label={title}>
      <h3 className="small" style={{ fontWeight: 600, margin: '0.8rem 0 0.3rem' }}>{title}</h3>
      <dl className="kv">
        {shown.map(([k, v]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd className="num">{v}</dd>
          </div>
        ))}
      </dl>
    </section>
  )
}

const n = (v: number | null, d = 2) => (v === null ? null : formatNumber(v, d))
const p = (v: number | null, d = 1) => (v === null ? null : formatPercent(v, d))
const c = (v: number | null, cur: string | null | undefined) => (v === null ? null : formatCompactCurrency(v, cur))

export function FundamentalsPanel({ summary }: { summary: AssetSummary }) {
  const q = useFundamentals(summary.id)
  if (q.isPending) return <SkeletonRows rows={6} />
  if (q.isError) return <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudieron cargar los fundamentales" />
  const f: Fundamentals | null = q.data
  if (!f) return <div className="empty">No hay fundamentales para este tipo de activo ({summary.class}).</div>
  const fr = fundamentalsFreshness(f)
  const cur = summary.currency
  const extra = Object.entries(f.extra ?? {}).filter(([, v]) => v !== null && v !== undefined && typeof v !== 'object')

  return (
    <div className="stack" style={{ gap: '0.5rem' }}>
      <div className="row wrap">
        <Provenance provider={f.sources} asOf={f.asOf} assetId={summary.id} stale={fr.stale} staleReason={fr.reason} label="Fuentes" />
      </div>
      {f.description ? <p className="small" style={{ maxWidth: '70ch' }}>{f.description}</p> : null}
      <div className="row wrap small muted">
        {f.sector ? <span>Sector: {f.sector}</span> : null}
        {f.industry ? <span>· {f.industry}</span> : null}
        {f.country ? <span>· {f.country}</span> : null}
        {f.employees ? <span>· {formatInt(f.employees)} empleados</span> : null}
        {f.website ? <ExternalLink href={f.website} className="prov"><span style={{ textDecoration: 'underline' }}>Sitio web</span></ExternalLink> : null}
      </div>
      {group('Valoración', [
        ['Capitalización', c(f.marketCap, cur)], ['Valor empresa', c(f.enterpriseValue, cur)],
        ['PER (12 m)', n(f.peTrailing, 1)], ['PER (previsto)', n(f.peForward, 1)], ['PEG', n(f.peg)],
        ['Precio / valor contable', n(f.priceToBook)], ['Precio / ventas', n(f.priceToSales)], ['EV / EBITDA', n(f.evToEbitda, 1)],
      ])}
      {group('Resultados', [
        ['BPA (12 m)', n(f.epsTrailing)], ['BPA (previsto)', n(f.epsForward)], ['Ingresos (12 m)', c(f.revenueTtm, cur)],
        ['Crec. ingresos', p(f.revenueGrowthYoy)], ['Crec. beneficios', p(f.earningsGrowthYoy)],
        ['Margen bruto', p(f.grossMargin)], ['Margen operativo', p(f.operatingMargin)], ['Margen neto', p(f.profitMargin)],
        ['ROE', p(f.roe)], ['ROA', p(f.roa)], ['Próximos resultados', f.nextEarningsDate ? formatDate(f.nextEarningsDate) : null],
      ])}
      {group('Balance', [
        ['Caja', c(f.totalCash, cur)], ['Deuda', c(f.totalDebt, cur)], ['Deuda / capital', n(f.debtToEquity, 1)],
        ['Ratio corriente', n(f.currentRatio)], ['Flujo de caja libre', c(f.freeCashFlow, cur)],
      ])}
      {group('Dividendo y riesgo', [
        ['Rentab. por dividendo', p(f.dividendYield)], ['Payout', p(f.payoutRatio, 0)], ['Beta', n(f.beta)],
        ['Acciones en circulación', f.sharesOutstanding === null ? null : formatCompact(f.sharesOutstanding)],
        ['Posición corta (% flotante)', p(f.shortPctFloat)], ['Insiders', p(f.insiderPct)], ['Instituciones', p(f.institutionalPct)],
      ])}
      {group('Analistas', [
        ['Objetivo medio', f.analystTargetMean === null ? null : formatPrice(f.analystTargetMean, { currency: cur })],
        ['Objetivo máximo', f.analystTargetHigh === null ? null : formatPrice(f.analystTargetHigh, { currency: cur })],
        ['Objetivo mínimo', f.analystTargetLow === null ? null : formatPrice(f.analystTargetLow, { currency: cur })],
        ['Recomendación (1 compra – 5 venta)', n(f.analystRatingMean)], ['Nº de analistas', f.analystCount === null ? null : String(f.analystCount)],
      ])}
      {group('Criptoactivo', [
        ['Oferta circulante', f.circulatingSupply === null ? null : formatCompact(f.circulatingSupply)],
        ['Oferta total', f.totalSupply === null ? null : formatCompact(f.totalSupply)],
        ['Oferta máxima', f.maxSupply === null ? null : formatCompact(f.maxSupply)],
        ['Valoración diluida', c(f.fullyDilutedValuation, cur)],
        ['Máximo histórico', f.allTimeHigh === null ? null : formatPrice(f.allTimeHigh, { currency: cur })],
        ['Fecha del máximo', f.allTimeHighDate ? formatDate(f.allTimeHighDate) : null],
      ])}
      {group('Fondo / ETF', [['Gastos corrientes', p(f.expenseRatio, 2)], ['Patrimonio', c(f.totalAssets, cur)]])}
      {group('Bono', [['Rentab. al vencimiento', p(f.yieldToMaturity, 2)], ['Cupón', p(f.coupon, 2)], ['Vencimiento', f.maturityDate ? formatDate(f.maturityDate) : null]])}
      {f.topHoldings.length > 0 ? (
        <section aria-label="Principales posiciones">
          <h3 className="small" style={{ fontWeight: 600, margin: '0.8rem 0 0.3rem' }}>Principales posiciones</h3>
          <div className="table-wrap">
            <table className="tbl">
              <tbody>
                {f.topHoldings.slice(0, 10).map((h) => (
                  <tr key={`${h.symbol}-${h.name}`}>
                    <td>{h.symbol ?? '—'}</td>
                    <td>{h.name}</td>
                    <td className="r num">{formatPercent(h.weight, 2)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ) : null}
      {extra.length > 0
        ? group('Otros datos del proveedor', extra.map(([k, v]) => [k, String(v)] as Row))
        : null}
    </div>
  )
}
