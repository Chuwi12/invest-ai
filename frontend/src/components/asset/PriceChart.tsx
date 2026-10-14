import {
  CandlestickSeries,
  ColorType,
  CrosshairMode,
  HistogramSeries,
  LineSeries,
  createChart,
  type IChartApi,
  type ISeriesApi,
  type LineData,
  type UTCTimestamp,
} from 'lightweight-charts'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useHistory } from '@/api/hooks'
import type { AssetId, AssetSummary, Candle, Interval, Quote, Range } from '@/api/types'
import { RANGES, defaultInterval, intervalSeconds } from '@/api/types'
import { useResolvedScheme } from '@/app/theme'
import { ErrorNotice, Skeleton } from '@/components/states'
import { Provenance } from '@/components/Provenance'
import { LOCALE, formatCompact, formatDateTime, formatPrice } from '@/lib/format'
import { useClock } from '@/hooks/useClock'
import { intervalsFor } from '@/lib/chart'
import { freshness } from '@/lib/freshness'
import { applyTickToCandle, bollingerSeries, smaSeries } from '@/lib/indicators'
import { usePriceTicks } from '@/realtime/hooks'

const RANGE_LABEL: Record<Range, string> = {
  '1d': '1D', '5d': '5D', '1mo': '1M', '3mo': '3M', '6mo': '6M', ytd: 'YTD', '1y': '1A', '2y': '2A', '5y': '5A', '10y': '10A', max: 'Máx',
}
const INTERVAL_LABEL: Record<Interval, string> = {
  '1m': '1 min', '5m': '5 min', '15m': '15 min', '30m': '30 min', '1h': '1 h', '1d': 'Diario', '1wk': 'Semanal', '1mo': 'Mensual',
}

type Overlay = 'sma20' | 'sma50' | 'sma200' | 'bb'
const OVERLAYS: { key: Overlay; label: string; color: string }[] = [
  { key: 'sma20', label: 'MM 20', color: '#e0a526' },
  { key: 'sma50', label: 'MM 50', color: '#3b82f6' },
  { key: 'sma200', label: 'MM 200', color: '#a855f7' },
  { key: 'bb', label: 'Bollinger', color: '#8b98a0' },
]

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

const toCandle = (c: Candle) => ({ time: c.t as UTCTimestamp, open: c.o, high: c.h, low: c.l, close: c.c })
const toLine = (p: { time: number; value: number }): LineData => ({ time: p.time as UTCTimestamp, value: p.value })

interface Props {
  id: AssetId
  summary: AssetSummary
  quote: Quote | null | undefined
}

export function PriceChart({ id, summary, quote }: Props) {
  const now = useClock()
  const [range, setRange] = useState<Range>('1y')
  const [interval, setIntervalSel] = useState<Interval | 'auto'>('auto')
  const [overlays, setOverlays] = useState<Set<Overlay>>(new Set(['sma50']))
  const effective: Interval = interval === 'auto' || !intervalsFor(range).includes(interval) ? defaultInterval(range) : interval
  const q = useHistory(id, range, interval === 'auto' || !intervalsFor(range).includes(interval) ? undefined : interval)
  const candles = q.data?.candles
  const origin = q.data
  // Intervalo realmente servido: el proveedor puede haber subido de intervalo (p. ej. 1m -> 5m).
  const served: Interval = origin?.interval ?? effective
  const coerced = !q.isPlaceholderData && origin?.interval != null && origin.interval !== effective

  const host = useRef<HTMLDivElement>(null)
  const tip = useRef<HTMLDivElement>(null)
  const chart = useRef<IChartApi | null>(null)
  const candleSeries = useRef<ISeriesApi<'Candlestick'> | null>(null)
  const volSeries = useRef<ISeriesApi<'Histogram'> | null>(null)
  const overlaySeries = useRef(new Map<string, ISeriesApi<'Line'>>())
  const last = useRef<Candle | undefined>(undefined)
  const scheme = useResolvedScheme()
  const cur = quote?.currency ?? undefined
  const classRef = useRef(summary.class)
  // Eje, etiquetas y crosshair en formato español (coma decimal, punto de miles), sin símbolo de moneda.
  const priceFormat = () => ({ type: 'custom' as const, minMove: 0.01, formatter: (p: number) => formatPrice(p, { class: classRef.current }) })
  useEffect(() => {
    classRef.current = summary.class
  }, [summary.class])

  // Crear / destruir el gráfico.
  useEffect(() => {
    const el = host.current
    if (!el) return
    const c = createChart(el, {
      autoSize: true,
      crosshair: { mode: CrosshairMode.Normal },
      // Fechas en español. El precio se formatea por serie (priceFormat custom): un
      // `localization.priceFormatter` global pisaría también el formato compacto del volumen.
      localization: { locale: LOCALE, dateFormat: 'dd MMM yyyy' },
      rightPriceScale: { borderVisible: false },
      timeScale: { borderVisible: false },
    })
    chart.current = c
    candleSeries.current = c.addSeries(CandlestickSeries, { priceLineVisible: true, priceFormat: priceFormat() }, 0)
    volSeries.current = c.addSeries(HistogramSeries, { priceFormat: { type: 'custom', formatter: (v: number) => formatCompact(v), minMove: 1 }, lastValueVisible: false, priceLineVisible: false }, 1)
    c.panes()[1]?.setHeight(90)

    const series = overlaySeries.current
    const onMove: Parameters<IChartApi['subscribeCrosshairMove']>[0] = (p) => {
      const t = tip.current
      if (!t || !candleSeries.current) return
      const d = p.seriesData.get(candleSeries.current) as { open: number; high: number; low: number; close: number } | undefined
      if (!p.time || !d) {
        t.textContent = ''
        return
      }
      const pc = { class: classRef.current }
      const v = volSeries.current ? (p.seriesData.get(volSeries.current) as { value?: number } | undefined) : undefined
      t.textContent = `A ${formatPrice(d.open, pc)}  Máx ${formatPrice(d.high, pc)}  Mín ${formatPrice(d.low, pc)}  C ${formatPrice(d.close, pc)}${v?.value ? `  Vol ${formatCompact(v.value)}` : ''}`
    }
    c.subscribeCrosshairMove(onMove)
    return () => {
      c.unsubscribeCrosshairMove(onMove)
      c.remove()
      chart.current = null
      candleSeries.current = null
      volSeries.current = null
      series.clear()
    }
  }, [])

  // Colores según el tema (variables CSS).
  useEffect(() => {
    const c = chart.current
    if (!c) return
    const ink = cssVar('--ink-2')
    const line = cssVar('--line')
    const up = cssVar('--up')
    const down = cssVar('--down')
    c.applyOptions({
      layout: { background: { type: ColorType.Solid, color: cssVar('--surface') }, textColor: ink, fontFamily: cssVar('--font-ui'), attributionLogo: true },
      grid: { vertLines: { color: line }, horzLines: { color: line } },
      rightPriceScale: { borderColor: line },
      timeScale: { borderColor: line },
    })
    candleSeries.current?.applyOptions({ upColor: up, downColor: down, borderUpColor: up, borderDownColor: down, wickUpColor: up, wickDownColor: down })
  }, [scheme])

  // Datos.
  useEffect(() => {
    const c = chart.current
    if (!c || !candles) return
    candleSeries.current?.setData(candles.map(toCandle))
    const up = cssVar('--up')
    const down = cssVar('--down')
    volSeries.current?.setData(
      candles.filter((x) => x.v !== null).map((x) => ({ time: x.t as UTCTimestamp, value: x.v ?? 0, color: (x.c >= x.o ? up : down) + '66' })),
    )
    last.current = candles[candles.length - 1]
    c.applyOptions({ timeScale: { timeVisible: intervalSeconds(served) < 86_400, secondsVisible: false } })
    c.timeScale().fitContent()
  }, [candles, served, scheme])

  // Superposiciones.
  const overlayData = useMemo(() => {
    const out = new Map<string, { color: string; data: LineData[] }>()
    if (!candles) return out
    for (const o of OVERLAYS) {
      if (!overlays.has(o.key)) continue
      if (o.key === 'bb') {
        const bb = bollingerSeries(candles)
        out.set('bb-u', { color: o.color, data: bb.map((p) => toLine({ time: p.time, value: p.upper })) })
        out.set('bb-l', { color: o.color, data: bb.map((p) => toLine({ time: p.time, value: p.lower })) })
      } else {
        out.set(o.key, { color: o.color, data: smaSeries(candles, Number(o.key.slice(3))).map(toLine) })
      }
    }
    return out
  }, [candles, overlays])

  useEffect(() => {
    const c = chart.current
    if (!c) return
    const map = overlaySeries.current
    for (const [k, s] of map) {
      if (!overlayData.has(k)) {
        c.removeSeries(s)
        map.delete(k)
      }
    }
    for (const [k, { color, data }] of overlayData) {
      let s = map.get(k)
      if (!s) {
        s = c.addSeries(LineSeries, { priceFormat: priceFormat(), color, lineWidth: 1, priceLineVisible: false, lastValueVisible: false, crosshairMarkerVisible: false }, 0)
        map.set(k, s)
      }
      s.setData(data)
    }
  }, [overlayData])

  // Ticks en vivo: actualizan la última vela sin re-renderizar React.
  usePriceTicks(id, (t) => {
    const s = candleSeries.current
    if (!s || !last.current) return
    const next = applyTickToCandle(last.current, t.price, Date.parse(t.ts) / 1000, intervalSeconds(served), t.volume)
    last.current = next
    s.update(toCandle(next))
  })

  const lastIso = candles?.length ? new Date((candles[candles.length - 1] as Candle).t * 1000).toISOString() : null
  const open = quote?.marketState === 'regular' || quote?.marketState === 'continuous'
  const fresh = lastIso ? freshness(lastIso, open && intervalSeconds(served) < 86_400 ? intervalSeconds(served) * 3000 : 5 * 86_400_000, now) : null

  const toggle = (k: Overlay) =>
    setOverlays((prev) => {
      const n = new Set(prev)
      if (n.has(k)) n.delete(k)
      else n.add(k)
      return n
    })

  return (
    <div className="panel stack" style={{ gap: '0.6rem' }}>
      <div className="row wrap">
        <div className="seg" role="group" aria-label="Rango">
          {RANGES.map((r) => (
            <button key={r} type="button" aria-pressed={range === r} onClick={() => setRange(r)}>
              {RANGE_LABEL[r]}
            </button>
          ))}
        </div>
        <label className="row small">
          <span className="muted">Vela</span>
          <select value={interval === 'auto' ? 'auto' : effective} onChange={(e) => setIntervalSel(e.target.value as Interval | 'auto')} aria-label="Intervalo de las velas">
            <option value="auto">Automático ({INTERVAL_LABEL[defaultInterval(range)]})</option>
            {intervalsFor(range).map((i) => (
              <option key={i} value={i}>{INTERVAL_LABEL[i]}</option>
            ))}
          </select>
        </label>
        <div className="row wrap right" role="group" aria-label="Superposiciones">
          {OVERLAYS.map((o) => (
            <button key={o.key} type="button" className="chip" aria-pressed={overlays.has(o.key)} onClick={() => toggle(o.key)} style={overlays.has(o.key) ? { borderColor: o.color, color: 'var(--ink)' } : undefined}>
              <span aria-hidden="true" style={{ width: 8, height: 8, borderRadius: 2, background: o.color, opacity: overlays.has(o.key) ? 1 : 0.35 }} /> {o.label}
            </button>
          ))}
        </div>
      </div>

      <div className="chart-box" role="img" aria-label={`Gráfico de velas de ${summary.name}, rango ${RANGE_LABEL[range]}`}>
        <div ref={tip} className="chart-tip num" aria-hidden="true" />
        <div ref={host} style={{ position: 'absolute', inset: 0 }} />
        {q.isPending ? <Skeleton h="100%" style={{ position: 'absolute', inset: 0, zIndex: 2 }} /> : null}
      </div>

      {coerced ? (
        <div className="notice" role="status">
          El proveedor no admite velas de {INTERVAL_LABEL[effective]} en este rango: se muestran de {INTERVAL_LABEL[served]}.
        </div>
      ) : null}
      {q.isError ? <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudo cargar el histórico" /> : null}
      {candles && candles.length === 0 ? <div className="notice">El proveedor no devolvió velas para este rango.</div> : null}
      {lastIso ? (
        <Provenance
          provider={origin?.source ?? quote?.source ?? summary.source}
          asOf={lastIso}
          assetId={id}
          href={origin?.sourceUrl}
          verifyUrl={origin?.sourceUrl}
          stale={fresh?.stale ?? false}
          staleReason={fresh?.reason}
          label="Velas"
          extra={`${candles?.length ?? 0} velas · última ${formatDateTime(lastIso)}${cur ? ` · ${cur}` : ''}${origin?.source ? '' : ' · proveedor deducido, el backend no lo indicó'}`}
        />
      ) : null}
    </div>
  )
}
