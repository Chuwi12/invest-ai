import { useIndicators } from '@/api/hooks'
import type { AssetSummary, Indicators } from '@/api/types'
import { ErrorNotice, SkeletonRows } from '@/components/states'
import { Provenance } from '@/components/Provenance'
import { TREND_LABEL } from '@/lib/assets'
import { formatNumber, formatPercent, formatPrice, formatRelative, toneOf } from '@/lib/format'
import { indicatorsFreshness } from '@/lib/freshness'
import { interpretIndicators } from '@/lib/indicators'

function Kv({ rows }: { rows: [string, string, string?][] }) {
  return (
    <dl className="kv">
      {rows.map(([k, v, tone]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd className={`num ${tone ?? ''}`}>{v}</dd>
        </div>
      ))}
    </dl>
  )
}

export function IndicatorsPanel({ summary }: { summary: AssetSummary }) {
  const q = useIndicators(summary.id)
  if (q.isPending) return <SkeletonRows rows={6} />
  if (q.isError) return <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudieron cargar los indicadores" />
  const i: Indicators = q.data
  const f = indicatorsFreshness(i)
  const opt = { class: summary.class }
  const pct = (v: number | null) => formatPercent(v, 2, true)
  const readings = interpretIndicators(i)
  return (
    <div className="stack" style={{ gap: '1rem' }}>
      <div className="row wrap">
        {i.trend ? <span className="chip accent">Tendencia: {TREND_LABEL[i.trend]}</span> : null}
        <Provenance
          provider={i.source ?? summary.source}
          asOf={i.asOf}
          assetId={summary.id}
          href={i.sourceUrl}
          verifyUrl={i.sourceUrl}
          stale={f.stale}
          staleReason={f.reason}
          label="Datos base"
          extra={`calculado por la app sobre velas diarias (cierre ${formatPrice(i.lastClose, opt)})${i.source ? '' : ' · proveedor deducido, el backend no lo indicó'}`}
        />
      </div>

      <div role="list" aria-label="Lecturas">
        {readings.map((r) => (
          <div className="reading" role="listitem" key={r.key}>
            <strong>{r.label}</strong>
            <span className={`num ${r.tone}`}>{r.value}</span>
            <span className="txt">{r.text}</span>
          </div>
        ))}
      </div>

      <h3 className="small" style={{ fontWeight: 600 }}>Rendimiento</h3>
      <Kv
        rows={[
          ['1 semana', pct(i.return1w), toneOf(i.return1w)],
          ['1 mes', pct(i.return1m), toneOf(i.return1m)],
          ['3 meses', pct(i.return3m), toneOf(i.return3m)],
          ['En el año', pct(i.returnYtd), toneOf(i.returnYtd)],
          ['1 año', pct(i.return1y), toneOf(i.return1y)],
          ['Desde máx. 52 sem.', pct(i.distFromHigh52w), toneOf(i.distFromHigh52w)],
        ]}
      />
      <h3 className="small" style={{ fontWeight: 600 }}>Niveles y riesgo</h3>
      <Kv
        rows={[
          ['Media móvil 20', formatPrice(i.sma20, opt)],
          ['Media móvil 50', formatPrice(i.sma50, opt)],
          ['Media móvil 200', formatPrice(i.sma200, opt)],
          ['Bollinger (sup. / inf.)', `${formatPrice(i.bbUpper, opt)} / ${formatPrice(i.bbLower, opt)}`],
          ['Máx. 52 sem.', formatPrice(i.high52w, opt)],
          ['Mín. 52 sem.', formatPrice(i.low52w, opt)],
          ['ATR (14)', formatPrice(i.atr14, opt)],
          ['Volatilidad 30 d', formatPercent(i.volatility30d, 1)],
          ['Caída máx. 1 año', formatPercent(i.maxDrawdown1y, 1)],
          ['Volumen relativo', i.relativeVolume === null ? '—' : `${formatNumber(i.relativeVolume, 2)}×`],
        ]}
      />
      {i.supports.length + i.resistances.length > 0 ? (
        <div className="grid cols-2">
          <div>
            <h3 className="small" style={{ fontWeight: 600 }}>Soportes</h3>
            {i.supports.map((s) => <div key={s} className="lvl"><span className="muted">Soporte</span><span className="num">{formatPrice(s, opt)}</span></div>)}
          </div>
          <div>
            <h3 className="small" style={{ fontWeight: 600 }}>Resistencias</h3>
            {i.resistances.map((s) => <div key={s} className="lvl"><span className="muted">Resistencia</span><span className="num">{formatPrice(s, opt)}</span></div>)}
          </div>
        </div>
      ) : null}
      <p className="tiny muted">Calculado {formatRelative(i.asOf)}. Las lecturas son reglas generales, no señales de operación.</p>
    </div>
  )
}
