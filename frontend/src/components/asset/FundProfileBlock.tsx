import type { FundProfile, WeightedItem } from '@/api/types'
import { Provenance } from '@/components/Provenance'
import { formatCompactCurrency, formatDate, formatFull, formatNumber, formatPercent } from '@/lib/format'

const SECTOR_NAMES: Record<string, string> = {
  technology: 'Tecnología', financial_services: 'Servicios financieros', communication_services: 'Comunicaciones',
  healthcare: 'Salud', consumer_cyclical: 'Consumo cíclico', consumer_defensive: 'Consumo defensivo',
  industrials: 'Industriales', energy: 'Energía', utilities: 'Servicios públicos', realestate: 'Inmobiliario',
  basic_materials: 'Materiales básicos',
}
const MIX_NAMES: Record<string, string> = { stock: 'Acciones', bond: 'Bonos', cash: 'Efectivo', preferred: 'Preferentes', convertible: 'Convertibles', other: 'Otros' }
const RATING_NAMES: Record<string, string> = { us_government: 'Deuda del Tesoro EE. UU.', below_b: 'Por debajo de B', other: 'Otros' }

const label = (names: Record<string, string>, key: string) => names[key] ?? key.replace(/_/g, ' ').toUpperCase().replace(/^(.)(.*)$/, (_, a: string, b: string) => a + b.toLowerCase())

type Row = [label: string, value: string | null, title?: string]

function Dl({ title, rows }: { title: string; rows: Row[] }) {
  const shown = rows.filter((r): r is [string, string, string?] => r[1] !== null)
  if (shown.length === 0) return null
  return (
    <section aria-label={title}>
      <h4 className="small" style={{ fontWeight: 600, margin: '0.7rem 0 0.3rem' }}>{title}</h4>
      <dl className="kv">
        {shown.map(([k, v, t]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd className="num" title={t}>{v}</dd>
          </div>
        ))}
      </dl>
    </section>
  )
}

function Weights({ title, items, names }: { title: string; items: WeightedItem[]; names: Record<string, string> }) {
  if (items.length === 0) return null
  return (
    <section aria-label={title}>
      <h4 className="small" style={{ fontWeight: 600, margin: '0.7rem 0 0.3rem' }}>{title}</h4>
      <div className="table-wrap">
        <table className="tbl">
          <tbody>
            {items.map((s) => (
              <tr key={s.name}>
                <td>{label(names, s.name)}</td>
                <td className="r num">{formatPercent(s.weight, 1)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

const pct = (v: number | null | undefined, d = 2) => (v === null || v === undefined ? null : formatPercent(v, d))
const num = (v: number | null | undefined, d = 2) => (v === null || v === undefined ? null : formatNumber(v, d))

/**
 * Bloque «Perfil del fondo» de la ficha de un ETF: costes, patrimonio, diez primeras posiciones,
 * sectores y valoración de la cartera, con la fuente y la fecha. Solo muestra lo que el proveedor dio.
 */
export function FundProfileBlock({ profile: p, assetId, currency }: { profile: FundProfile; assetId: string; currency?: string | null }) {
  const top = p.topHoldings.slice(0, 10)
  const topSum = top.reduce((a, h) => a + h.weight, 0)
  return (
    <section aria-label="Perfil del fondo" data-testid="fund-profile">
      <h3 className="small" style={{ fontWeight: 600, margin: '0.8rem 0 0.3rem' }}>Perfil del fondo</h3>
      <div className="row wrap" style={{ marginBottom: '0.3rem' }}>
        <Provenance provider={p.source} asOf={p.asOf} assetId={assetId} href={p.sourceUrl} verifyUrl={p.sourceUrl} origin="local" label="Fuente" />
      </div>
      {[p.family, p.category, p.legalType].some(Boolean) ? (
        <p className="small muted">{[p.family, p.category, p.legalType].filter(Boolean).join(' · ')}{p.inceptionDate ? ` · inicio ${formatDate(p.inceptionDate)}` : ''}</p>
      ) : null}
      <Dl
        title="Costes y patrimonio"
        rows={[
          ['Gastos corrientes (TER)', pct(p.expenseRatio)], ['Gastos brutos', pct(p.grossExpenseRatio)], ['Comisión 12b-1', pct(p.twelveB1)],
          ['Comisión de entrada', pct(p.frontLoad)], ['Comisión de salida', pct(p.deferredLoad)], ['Rotación de la cartera', pct(p.turnover, 1)],
          ['Patrimonio', p.totalAssets === null ? null : formatCompactCurrency(p.totalAssets, currency), p.totalAssets === null ? undefined : formatFull(p.totalAssets, currency)],
          ['Rentab. por distribución', pct(p.dividendYield)], ['Beta (3 años)', num(p.beta3y)],
          ['Rentab. en el año', pct(p.ytdReturn, 1)], ['Rentab. anual 3 años', pct(p.threeYearReturn, 1)], ['Rentab. anual 5 años', pct(p.fiveYearReturn, 1)],
        ]}
      />
      {top.length > 0 ? (
        <section aria-label="Diez primeras posiciones">
          <h4 className="small" style={{ fontWeight: 600, margin: '0.7rem 0 0.3rem' }}>
            Primeras {top.length} posiciones <span className="muted">· suman {formatPercent(topSum, 1)}</span>
          </h4>
          <div className="table-wrap">
            <table className="tbl">
              <tbody>
                {top.map((h) => (
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
      <Weights title="Sectores" items={p.sectorWeights} names={SECTOR_NAMES} />
      <Weights title="Tipo de activo" items={p.assetMix} names={MIX_NAMES} />
      <Weights title="Calificaciones crediticias" items={p.bondRatings} names={RATING_NAMES} />
      {p.equity ? (
        <Dl
          title="Valoración de la cartera de acciones"
          rows={[
            ['PER', num(p.equity.priceToEarnings, 1)], ['Precio / valor contable', num(p.equity.priceToBook)], ['Precio / ventas', num(p.equity.priceToSales)],
            ['Precio / flujo de caja', num(p.equity.priceToCashflow, 1)], ['Crec. beneficios (3 años)', pct(p.equity.threeYearEarningsGrowth, 1)],
            ['Capitalización mediana', p.equity.medianMarketCap === null ? null : formatCompactCurrency(p.equity.medianMarketCap, currency)],
          ]}
        />
      ) : null}
      {p.bond ? (
        <Dl
          title="Cartera de renta fija"
          rows={[
            ['Duración (años)', num(p.bond.duration, 1)], ['Vencimiento medio (años)', num(p.bond.maturity, 1)], ['Calidad crediticia media', num(p.bond.creditQuality, 1)],
          ]}
        />
      ) : null}
    </section>
  )
}
