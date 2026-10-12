import { MessageSquare } from 'lucide-react'
import { Link } from 'react-router'
import type { AssetSummary, Quote } from '@/api/types'
import { FavoriteButton } from '@/components/FavoriteButton'
import { LivePrice } from '@/components/LivePrice'
import { Provenance } from '@/components/Provenance'
import { CLASS_SINGULAR, MARKET_STATE_LABEL } from '@/lib/assets'
import { formatCompactCurrency, formatDelay, formatInt, formatPrice } from '@/lib/format'
import { quoteFreshness } from '@/lib/freshness'

/** Cabecera: identidad, precio en vivo (animado por tick) y cifras del día con su procedencia. */
export function AssetHeader({ summary, quote, isFavorite }: { summary: AssetSummary; quote: Quote | null | undefined; isFavorite: boolean }) {
  const fresh = quoteFreshness(quote)
  const cur = summary.currency ?? quote?.currency ?? undefined
  const opt = { class: summary.class, currency: cur }
  const cell = (label: string, value: string) => (
    <div>
      <dt>{label}</dt>
      <dd className="num">{value}</dd>
    </div>
  )
  return (
    <header className="stack" style={{ gap: '0.9rem' }}>
      <div className="asset-head">
        <div className="stack" style={{ gap: '0.35rem' }}>
          <div className="row wrap">
            <span className="chip">{CLASS_SINGULAR[summary.class]}</span>
            {summary.exchange ? <span className="chip">{summary.exchange}</span> : null}
            {quote ? <span className="chip">{MARKET_STATE_LABEL[quote.marketState]}</span> : null}
          </div>
          <h1>
            {summary.name} <span className="muted" style={{ fontFamily: 'var(--font-ui)', fontSize: '1rem', fontWeight: 400 }}>{summary.symbol}</span>
          </h1>
        </div>
        <div className="right row wrap">
          <FavoriteButton summary={summary} on={isFavorite} />
          <Link to={`/chat?asset=${encodeURIComponent(summary.id)}`} className="btn">
            <MessageSquare aria-hidden="true" /> Preguntar a la IA
          </Link>
        </div>
      </div>

      {quote ? (
        <>
          <LivePrice quote={quote} assetClass={summary.class} currency={cur} showAbs className="big-price" />
          <Provenance
            provider={quote.source}
            asOf={quote.timestamp}
            assetId={summary.id}
            href={quote.sourceUrl}
            verifyUrl={quote.sourceUrl}
            origin={quote.timestampOrigin}
            stale={fresh.stale}
            staleReason={fresh.reason}
            label="Cotización"
            extra={formatDelay(quote.delaySecs)}
          />
          <dl className="kv">
            {cell('Apertura', formatPrice(quote.open, opt))}
            {cell('Máximo del día', formatPrice(quote.high, opt))}
            {cell('Mínimo del día', formatPrice(quote.low, opt))}
            {cell('Cierre anterior', formatPrice(quote.prevClose, opt))}
            {cell('Volumen', quote.volume === null ? '—' : formatInt(quote.volume))}
            {cell('Capitalización', quote.marketCap === null ? '—' : formatCompactCurrency(quote.marketCap, cur))}
          </dl>
        </>
      ) : (
        <div className="notice warn">Sin cotización disponible ahora mismo para este activo.</div>
      )}
    </header>
  )
}
