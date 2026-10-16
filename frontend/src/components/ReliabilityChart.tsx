import type { ReliabilityBin } from '@/api/types'
import { formatPercent } from '@/lib/format'

const W = 360
const H = 360
const PAD = { l: 44, r: 14, t: 14, b: 40 }

/**
 * Gráfico de fiabilidad: confianza declarada (eje X) frente a acierto real (eje Y). La diagonal es
 * la calibración perfecta; cada punto lleva su intervalo de confianza al 95 % y su tamaño crece con n.
 */
export function ReliabilityChart({ bins }: { bins: ReliabilityBin[] }) {
  const x = (v: number) => PAD.l + v * (W - PAD.l - PAD.r)
  const y = (v: number) => H - PAD.b - v * (H - PAD.t - PAD.b)
  const ticks = [0, 0.25, 0.5, 0.75, 1]
  const maxN = Math.max(1, ...bins.map((b) => b.n))
  const label = bins.length
    ? `Gráfico de fiabilidad con ${bins.length} tramos: ${bins.map((b) => `confianza ${formatPercent(b.meanConfidence, 0)}, acierto ${formatPercent(b.hitRate, 0)} (n=${b.n})`).join('; ')}.`
    : 'Gráfico de fiabilidad sin datos.'
  return (
    <figure className="rel-chart">
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label} width="100%" style={{ maxWidth: '26rem' }}>
        {ticks.map((t) => (
          <g key={t}>
            <line x1={x(0)} x2={x(1)} y1={y(t)} y2={y(t)} className="rel-grid" />
            <line x1={x(t)} x2={x(t)} y1={y(0)} y2={y(1)} className="rel-grid" />
            <text x={PAD.l - 6} y={y(t) + 4} textAnchor="end" className="rel-tick">{Math.round(t * 100)}</text>
            <text x={x(t)} y={H - PAD.b + 16} textAnchor="middle" className="rel-tick">{Math.round(t * 100)}</text>
          </g>
        ))}
        <line x1={x(0)} y1={y(0)} x2={x(1)} y2={y(1)} className="rel-diag" />
        <text x={x(1) - 4} y={y(1) + 14} textAnchor="end" className="rel-tick">calibración perfecta</text>
        {bins.map((b) => (
          <g key={b.lo}>
            <line x1={x(b.meanConfidence)} x2={x(b.meanConfidence)} y1={y(b.ciLow)} y2={y(b.ciHigh)} className="rel-err" />
            <line x1={x(b.meanConfidence) - 4} x2={x(b.meanConfidence) + 4} y1={y(b.ciLow)} y2={y(b.ciLow)} className="rel-err" />
            <line x1={x(b.meanConfidence) - 4} x2={x(b.meanConfidence) + 4} y1={y(b.ciHigh)} y2={y(b.ciHigh)} className="rel-err" />
            <circle cx={x(b.meanConfidence)} cy={y(b.hitRate)} r={3.5 + 5 * Math.sqrt(b.n / maxN)} className="rel-dot">
              <title>{`Confianza ${formatPercent(b.lo, 0)}–${formatPercent(b.hi, 0)}: acierto ${formatPercent(b.hitRate, 0)} (IC ${formatPercent(b.ciLow, 0)}–${formatPercent(b.ciHigh, 0)}), n=${b.n}`}</title>
            </circle>
            <text x={x(b.meanConfidence)} y={y(b.ciHigh) - 6} textAnchor="middle" className="rel-n">{`n=${b.n}`}</text>
          </g>
        ))}
        <text x={(PAD.l + W - PAD.r) / 2} y={H - 6} textAnchor="middle" className="rel-axis">Confianza declarada (%)</text>
        <text transform={`translate(12 ${(PAD.t + H - PAD.b) / 2}) rotate(-90)`} textAnchor="middle" className="rel-axis">Acierto real (%)</text>
      </svg>
    </figure>
  )
}
