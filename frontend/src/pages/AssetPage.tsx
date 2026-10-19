import { useEffect, useRef } from 'react'
import { useParams, useSearchParams } from 'react-router'
import { useAsset, useQuote } from '@/api/hooks'
import type { AssetId } from '@/api/types'
import { ASSET_CLASSES } from '@/api/types'
import { ApiError } from '@/api/client'
import { AnalysisPanel } from '@/components/asset/AnalysisPanel'
import { AssetHeader } from '@/components/asset/AssetHeader'
import { FundamentalsPanel } from '@/components/asset/FundamentalsPanel'
import { IndicatorsPanel } from '@/components/asset/IndicatorsPanel'
import { MacroPanel } from '@/components/asset/MacroPanel'
import { NewsPanel } from '@/components/asset/NewsPanel'
import { PriceChart } from '@/components/asset/PriceChart'
import { SocialPanel } from '@/components/asset/SocialPanel'
import { ErrorNotice, Skeleton } from '@/components/states'
import { useTitle } from '@/hooks/useTitle'

const TABS = [
  ['analysis', 'Análisis de IA'],
  ['indicators', 'Indicadores'],
  ['fundamentals', 'Fundamentales'],
  ['news', 'Noticias'],
  ['social', 'Redes'],
  ['macro', 'Macro'],
] as const
type Tab = (typeof TABS)[number][0]

function isAssetId(v: string | undefined): v is AssetId {
  if (!v) return false
  const i = v.indexOf(':')
  return i > 0 && i < v.length - 1 && (ASSET_CLASSES as readonly string[]).includes(v.slice(0, i))
}

function AssetView({ id }: { id: AssetId }) {
  const detail = useAsset(id)
  const quote = useQuote(id, { poll: true })
  const [params, setParams] = useSearchParams()
  const tab: Tab = (TABS.find(([k]) => k === params.get('tab'))?.[0] ?? 'analysis') as Tab
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([])
  const summary = detail.data?.summary
  useTitle(summary ? `${summary.symbol} · ${summary.name}` : 'Activo')

  useEffect(() => {
    window.scrollTo({ top: 0 })
  }, [id])

  // La pestaña activa siempre visible cuando la barra desborda (móvil).
  useEffect(() => {
    document.getElementById(`tab-${tab}`)?.scrollIntoView({ inline: 'nearest', block: 'nearest' })
  }, [tab, detail.isSuccess])

  if (detail.isPending) {
    return (
      <div className="page">
        <Skeleton w="40%" h="2.4rem" />
        <Skeleton w="30%" h="3rem" />
        <Skeleton h="26rem" />
      </div>
    )
  }
  if (detail.isError) {
    const notFound = detail.error instanceof ApiError && detail.error.status === 404
    return (
      <div className="page">
        <ErrorNotice error={notFound ? new Error(`No existe el activo «${id}».`) : detail.error} onRetry={notFound ? undefined : () => void detail.refetch()} />
      </div>
    )
  }

  const q = quote.data ?? detail.data.quote
  const setTab = (t: Tab) => setParams((p) => { const n = new URLSearchParams(p); n.set('tab', t); return n }, { replace: true })
  const onTabKey = (e: React.KeyboardEvent, i: number) => {
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (!d) return
    e.preventDefault()
    const j = (i + d + TABS.length) % TABS.length
    setTab(TABS[j]![0])
    tabRefs.current[j]?.focus()
  }

  return (
    <div className="page">
      <AssetHeader summary={detail.data.summary} quote={q} isFavorite={detail.data.isFavorite} />
      <PriceChart id={id} summary={detail.data.summary} quote={q} />
      <div>
        <div className="tabs" role="tablist" aria-label="Secciones del activo">
          {TABS.map(([k, label], i) => (
            <button
              key={k}
              ref={(el) => { tabRefs.current[i] = el }}
              role="tab"
              id={`tab-${k}`}
              aria-selected={tab === k}
              aria-controls={`panel-${k}`}
              tabIndex={tab === k ? 0 : -1}
              onClick={() => setTab(k)}
              onKeyDown={(e) => onTabKey(e, i)}
            >
              {label}
            </button>
          ))}
        </div>
        <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} style={{ paddingTop: '1.25rem' }}>
          {tab === 'analysis' ? <AnalysisPanel summary={detail.data.summary} quote={q} /> : null}
          {tab === 'indicators' ? <IndicatorsPanel summary={detail.data.summary} /> : null}
          {tab === 'fundamentals' ? <FundamentalsPanel summary={detail.data.summary} /> : null}
          {tab === 'news' ? <NewsPanel summary={detail.data.summary} /> : null}
          {tab === 'social' ? <SocialPanel summary={detail.data.summary} /> : null}
          {tab === 'macro' ? <MacroPanel /> : null}
        </div>
      </div>
    </div>
  )
}

export default function AssetPage() {
  const { id } = useParams()
  if (!isAssetId(id)) {
    return (
      <div className="page">
        <ErrorNotice error={new Error('Identificador de activo no válido.')} />
      </div>
    )
  }
  return <AssetView key={id} id={id} />
}
