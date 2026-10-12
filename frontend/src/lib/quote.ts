import type { Quote } from '@/api/types'
import type { LiveTick } from '@/realtime/client'

export interface LiveQuote {
  price: number | null
  change: number | null
  /** Puntos porcentuales (1,5 = +1,5 %). */
  changePct: number | null
  live: boolean
}

/**
 * Combina la cotización REST con el último tick del WebSocket. Usa el tick solo si no es más
 * antiguo que la cotización, y recalcula la variación contra el cierre previo para que sea
 * coherente con el precio mostrado (independientemente de la unidad de `changePct`).
 */
export function mergeLive(quote: Quote | null | undefined, tick: LiveTick | undefined): LiveQuote {
  const tickTs = tick ? Date.parse(tick.ts) : Number.NaN
  const quoteTs = quote ? Date.parse(quote.timestamp) : Number.NaN
  const tickIsFresh = !!tick && (!quote || Number.isNaN(tickTs) || Number.isNaN(quoteTs) || tickTs >= quoteTs)
  const price = tickIsFresh && tick ? tick.price : (quote?.price ?? null)
  if (price === null) return { price: null, change: null, changePct: null, live: false }

  let prev = quote?.prevClose ?? null
  if (prev === null && quote && quote.change !== null) prev = quote.price - quote.change
  if (prev !== null && prev !== 0) {
    return { price, change: price - prev, changePct: (price / prev - 1) * 100, live: tickIsFresh }
  }
  return {
    price,
    change: tickIsFresh ? null : (quote?.change ?? null),
    changePct: tickIsFresh && tick?.changePct != null ? tick.changePct : (quote?.changePct ?? null),
    live: tickIsFresh,
  }
}
