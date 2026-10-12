import type {
  AssetClass,
  AssetId,
  Candle,
  Fundamentals,
  Holding,
  Indicators,
  Interval,
  MarketState,
  Quote,
  Range,
  Trend,
} from '@/api/types'
import { defaultInterval, intervalSeconds } from '@/api/types'
import { CATALOG, classOf, findEntry, symbolOfId, type CatalogEntry } from './catalog'
import { between, gauss, hashString, rngFor } from './rng'

const DAY = 86_400

/** Entrada del catálogo o una sintética para ids desconocidos (la ficha funciona con cualquier id). */
export function entryFor(id: AssetId): CatalogEntry {
  const known = findEntry(id)
  if (known) return known
  const r = rngFor(id, 'synthetic')
  return {
    id,
    name: symbolOfId(id),
    price: Math.round(between(r, 8, 480) * 100) / 100,
    vol: between(r, 0.15, 0.6),
    currency: 'USD',
    exchange: null,
    country: null,
  }
}

// --- Precio vivo ---------------------------------------------------------------------------

/** Último precio simulado por activo: lo mueven los ticks del socket falso. */
const livePrices = new Map<AssetId, number>()

export function currentPrice(id: AssetId): number {
  return livePrices.get(id) ?? entryFor(id).price
}

export function setLivePrice(id: AssetId, price: number): void {
  livePrices.set(id, price)
}

// --- Velas ---------------------------------------------------------------------------------

function candleCount(range: Range, interval: Interval): number {
  const table: Partial<Record<`${Range}/${Interval}`, number>> = {
    '1d/5m': 96,
    '1d/1m': 390,
    '5d/15m': 130,
    '1mo/1h': 154,
    '3mo/1d': 63,
    '6mo/1d': 126,
    'ytd/1d': Math.max(20, Math.round(((Date.now() - Date.UTC(new Date().getUTCFullYear(), 0, 1)) / DAY / 7) * 5)),
    '1y/1d': 252,
    '2y/1d': 504,
    '5y/1wk': 260,
    '10y/1wk': 520,
    'max/1mo': 240,
  }
  const n = table[`${range}/${interval}`]
  if (n) return n
  // Combinación no estándar: estimamos por duración.
  const days = { '1d': 1, '5d': 5, '1mo': 30, '3mo': 90, '6mo': 180, ytd: 250, '1y': 365, '2y': 730, '5y': 1825, '10y': 3650, max: 7300 }[range]
  return Math.max(10, Math.min(2000, Math.round((days * DAY) / intervalSeconds(interval))))
}

const candleCache = new Map<string, Candle[]>()

function isWeekend(tsSecs: number): boolean {
  const d = new Date(tsSecs * 1000).getUTCDay()
  return d === 0 || d === 6
}

export function genCandles(id: AssetId, range: Range, interval?: Interval): Candle[] {
  const iv = interval ?? defaultInterval(range)
  const key = `${id}|${range}|${iv}`
  const cached = candleCache.get(key)
  if (cached) return cached

  const entry = entryFor(id)
  const cls = classOf(id)
  const step = intervalSeconds(iv)
  const n = candleCount(range, iv)
  const rand = rngFor(id, range, iv)
  const intraday = step < DAY
  const sigma = intraday
    ? entry.vol * Math.sqrt(step / (252 * 23_400))
    : entry.vol * Math.sqrt(Math.max(1, step / DAY) * (step >= 7 * DAY ? 5 / 7 : 1) / 252) * (step >= 28 * DAY ? Math.sqrt(21 / 30) : 1)
  const drift = between(rand, -0.15, 0.35) * (sigma / Math.max(entry.vol, 0.01)) * 0.4

  // Camino aleatorio en logaritmos, de más antiguo a más reciente.
  const closes: number[] = []
  let x = 0
  for (let i = 0; i < n; i++) {
    x += drift + sigma * gauss(rand)
    closes.push(x)
  }
  const last = closes[n - 1] ?? 0
  const scale = entry.price / Math.exp(last)

  // Marcas de tiempo (hacia atrás desde la última vela), saltando fines de semana en no-cripto diario.
  const skipWeekends = !intraday && cls !== 'crypto' && step === DAY
  const times: number[] = []
  let t = Math.floor(Date.now() / 1000 / step) * step
  if (skipWeekends) while (isWeekend(t)) t -= DAY
  for (let i = 0; i < n; i++) {
    times.push(t)
    t -= step
    if (skipWeekends) while (isWeekend(t)) t -= DAY
  }
  times.reverse()

  const baseVol = cls === 'crypto' ? 3e9 / Math.max(entry.price, 1) : cls === 'index' || cls === 'forex' || cls === 'bond' ? 0 : 4e7 / Math.max(entry.price / 50, 1)
  const out: Candle[] = []
  let prevClose = Math.exp((closes[0] ?? 0) - sigma * gauss(rand)) * scale
  for (let i = 0; i < n; i++) {
    const c = Math.exp(closes[i] ?? 0) * scale
    const o = prevClose
    const wick = Math.abs(gauss(rand)) * sigma * 0.55
    const h = Math.max(o, c) * (1 + wick)
    const l = Math.min(o, c) * (1 - Math.abs(gauss(rand)) * sigma * 0.55)
    const move = Math.abs(Math.log(c / o)) / (sigma || 1)
    const v = baseVol > 0 ? Math.round(baseVol * (0.5 + rand()) * (1 + move * 0.3) * (intraday ? step / 3600 : step / DAY)) : null
    out.push({ t: times[i] as number, o, h, l, c, v })
    prevClose = c
  }
  candleCache.set(key, out)
  return out
}

// --- Cotización ------------------------------------------------------------------------------

function marketStateFor(entry: CatalogEntry, now = new Date()): MarketState {
  const cls = classOf(entry.id)
  if (cls === 'crypto') return 'continuous'
  if (cls === 'forex') return now.getUTCDay() === 6 ? 'closed' : 'regular'
  const dow = now.getUTCDay()
  if (dow === 0 || dow === 6) return 'closed'
  const h = now.getUTCHours() + now.getUTCMinutes() / 60
  const eu = entry.currency === 'EUR'
  if (eu) return h >= 7 && h < 15.5 ? 'regular' : h >= 15.5 && h < 17 ? 'post' : 'closed'
  if (h >= 13.5 && h < 20) return 'regular'
  if (h >= 8 && h < 13.5) return 'pre'
  if (h >= 20 && h < 24) return 'post'
  return 'closed'
}

export function sharesFor(id: AssetId): number {
  const cls = classOf(id)
  const r = rngFor(id, 'shares')
  if (cls === 'crypto') {
    const known: Record<string, number> = { BTC: 19.7e6, ETH: 120.4e6, SOL: 470e6, XRP: 56e9, DOGE: 146e9, ADA: 36e9, SHIB: 589e12, PEPE: 420e12 }
    return known[symbolOfId(id)] ?? between(r, 1e8, 5e10)
  }
  return Math.round(between(r, 0.4e9, 14e9))
}

export function genQuote(id: AssetId): Quote {
  const entry = entryFor(id)
  const cls = classOf(id)
  const price = currentPrice(id)
  const day = Math.floor(Date.now() / 1000 / DAY)
  const r = rngFor(id, 'quote', day)
  const dayMove = gauss(r) * (entry.vol / 16)
  const prevClose = entry.price / (1 + dayMove)
  const change = price - prevClose
  const noVol = cls === 'index' || cls === 'forex' || cls === 'bond'
  const delay = entry.delay ?? 0
  return {
    id,
    price,
    change,
    changePct: (change / prevClose) * 100,
    open: prevClose * (1 + gauss(r) * 0.002),
    high: Math.max(price, prevClose) * (1 + Math.abs(gauss(r)) * 0.004),
    low: Math.min(price, prevClose) * (1 - Math.abs(gauss(r)) * 0.004),
    prevClose,
    volume: noVol ? null : Math.round(between(r, 0.6, 1.6) * (cls === 'crypto' ? 2.5e10 : 4.2e7)),
    marketCap: cls === 'stock' || cls === 'crypto' ? price * sharesFor(id) : null,
    currency: entry.currency,
    marketState: marketStateFor(entry),
    timestamp: new Date(Date.now() - delay * 1000).toISOString(),
    source: cls === 'crypto' ? 'binance' : 'yahoo',
    delaySecs: delay,
  }
}

// --- Indicadores técnicos --------------------------------------------------------------------

function ema(values: number[], period: number): number[] {
  const k = 2 / (period + 1)
  const out: number[] = []
  let prev = values[0] ?? 0
  for (let i = 0; i < values.length; i++) {
    prev = i === 0 ? (values[0] ?? 0) : (values[i] as number) * k + prev * (1 - k)
    out.push(prev)
  }
  return out
}

function sma(values: number[], period: number): number | null {
  if (values.length < period) return null
  let s = 0
  for (let i = values.length - period; i < values.length; i++) s += values[i] as number
  return s / period
}

function rsiWilder(closes: number[], period = 14): number | null {
  if (closes.length <= period) return null
  let gain = 0
  let loss = 0
  for (let i = 1; i <= period; i++) {
    const d = (closes[i] as number) - (closes[i - 1] as number)
    if (d >= 0) gain += d
    else loss -= d
  }
  gain /= period
  loss /= period
  for (let i = period + 1; i < closes.length; i++) {
    const d = (closes[i] as number) - (closes[i - 1] as number)
    gain = (gain * (period - 1) + Math.max(d, 0)) / period
    loss = (loss * (period - 1) + Math.max(-d, 0)) / period
  }
  return loss === 0 ? 100 : 100 - 100 / (1 + gain / loss)
}

function pivots(candles: Candle[], kind: 'low' | 'high', win = 5): number[] {
  const out: number[] = []
  const start = Math.max(win, candles.length - 160)
  for (let i = start; i < candles.length - win; i++) {
    const cur = kind === 'low' ? (candles[i] as Candle).l : (candles[i] as Candle).h
    let ok = true
    for (let j = i - win; j <= i + win && ok; j++) {
      const v = kind === 'low' ? (candles[j] as Candle).l : (candles[j] as Candle).h
      if (kind === 'low' ? v < cur : v > cur) ok = false
    }
    if (ok) out.push(cur)
  }
  return out
}

export function genIndicators(id: AssetId): Indicators {
  const candles = genCandles(id, '1y', '1d')
  const closes = candles.map((c) => c.c)
  const price = currentPrice(id)
  const lastClose = closes[closes.length - 1] ?? price
  const e12 = ema(closes, 12)
  const e26 = ema(closes, 26)
  const macdLine = e12.map((v, i) => v - (e26[i] as number))
  const signalLine = ema(macdLine, 9)
  const macd = macdLine[macdLine.length - 1] ?? null
  const macdSignal = signalLine[signalLine.length - 1] ?? null

  const sma20 = sma(closes, 20)
  let bbUpper: number | null = null
  let bbLower: number | null = null
  if (sma20 !== null) {
    const win = closes.slice(-20)
    const sd = Math.sqrt(win.reduce((s, v) => s + (v - sma20) ** 2, 0) / 20)
    bbUpper = sma20 + 2 * sd
    bbLower = sma20 - 2 * sd
  }

  // ATR(14) de Wilder
  let atr = 0
  for (let i = 1; i < candles.length; i++) {
    const c = candles[i] as Candle
    const pc = (candles[i - 1] as Candle).c
    const tr = Math.max(c.h - c.l, Math.abs(c.h - pc), Math.abs(c.l - pc))
    atr = i <= 14 ? atr + tr / 14 : (atr * 13 + tr) / 14
  }

  const rets = closes.slice(1).map((c, i) => Math.log(c / (closes[i] as number)))
  const last30 = rets.slice(-30)
  const mean30 = last30.reduce((s, v) => s + v, 0) / (last30.length || 1)
  const vol30 = Math.sqrt(last30.reduce((s, v) => s + (v - mean30) ** 2, 0) / (last30.length || 1)) * Math.sqrt(252)

  let peak = closes[0] ?? 0
  let mdd = 0
  for (const c of closes) {
    peak = Math.max(peak, c)
    mdd = Math.min(mdd, c / peak - 1)
  }

  const back = (n: number) => {
    const c = closes[closes.length - 1 - n]
    return c ? lastClose / c - 1 : null
  }
  const yearStart = Date.UTC(new Date().getUTCFullYear(), 0, 1) / 1000
  const ytdBase = candles.find((c) => c.t >= yearStart)
  const high52 = Math.max(...candles.map((c) => c.h))
  const low52 = Math.min(...candles.map((c) => c.l))
  const vols = candles.map((c) => c.v ?? 0)
  const volAvg20 = vols.length >= 20 ? vols.slice(-20).reduce((s, v) => s + v, 0) / 20 : null

  const sma50 = sma(closes, 50)
  const sma200 = sma(closes, 200)
  const r3m = back(63)
  let trend: Trend = 'sideways'
  if (sma50 !== null && sma200 !== null) {
    if (lastClose > sma50 && sma50 > sma200) trend = r3m !== null && r3m > 0.12 ? 'strong_up' : 'up'
    else if (lastClose < sma50 && sma50 < sma200) trend = r3m !== null && r3m < -0.12 ? 'strong_down' : 'down'
    else if (lastClose > sma200) trend = 'up'
    else if (lastClose < sma200) trend = 'down'
  }

  const lows = pivots(candles, 'low').filter((v) => v < lastClose).sort((a, b) => b - a)
  const highs = pivots(candles, 'high').filter((v) => v > lastClose).sort((a, b) => a - b)

  return {
    asOf: new Date().toISOString(),
    lastClose,
    sma20,
    sma50,
    sma200,
    ema12: e12[e12.length - 1] ?? null,
    ema26: e26[e26.length - 1] ?? null,
    rsi14: rsiWilder(closes),
    macd,
    macdSignal,
    macdHist: macd !== null && macdSignal !== null ? macd - macdSignal : null,
    bbUpper,
    bbMiddle: sma20,
    bbLower,
    atr14: atr || null,
    volatility30d: vol30,
    maxDrawdown1y: mdd,
    return1w: back(5),
    return1m: back(21),
    return3m: r3m,
    returnYtd: ytdBase ? lastClose / ytdBase.c - 1 : null,
    return1y: closes[0] ? lastClose / closes[0] - 1 : null,
    high52w: high52,
    low52w: low52,
    distFromHigh52w: lastClose / high52 - 1,
    volumeAvg20: volAvg20 && volAvg20 > 0 ? volAvg20 : null,
    relativeVolume: volAvg20 && volAvg20 > 0 ? (vols[vols.length - 1] as number) / volAvg20 : null,
    trend,
    supports: lows.slice(0, 3),
    resistances: highs.slice(0, 3),
  }
}

// --- Fundamentales ---------------------------------------------------------------------------

const EMPTY_FUNDAMENTALS: Fundamentals = {
  description: null, sector: null, industry: null, website: null, employees: null, country: null,
  marketCap: null, enterpriseValue: null, peTrailing: null, peForward: null, peg: null, priceToBook: null,
  priceToSales: null, evToEbitda: null, epsTrailing: null, epsForward: null, revenueTtm: null,
  revenueGrowthYoy: null, earningsGrowthYoy: null, grossMargin: null, operatingMargin: null, profitMargin: null,
  roe: null, roa: null, freeCashFlow: null, totalCash: null, totalDebt: null, debtToEquity: null,
  currentRatio: null, dividendYield: null, payoutRatio: null, beta: null, sharesOutstanding: null,
  shortPctFloat: null, insiderPct: null, institutionalPct: null, analystTargetMean: null,
  analystTargetHigh: null, analystTargetLow: null, analystRatingMean: null, analystCount: null,
  nextEarningsDate: null, circulatingSupply: null, totalSupply: null, maxSupply: null,
  fullyDilutedValuation: null, allTimeHigh: null, allTimeHighDate: null, expenseRatio: null, totalAssets: null,
  topHoldings: [], yieldToMaturity: null, coupon: null, maturityDate: null, extra: {}, sources: [], asOf: null,
}

const SP500_TOP: Holding[] = [
  { symbol: 'AAPL', name: 'Apple Inc.', weight: 0.071 },
  { symbol: 'NVDA', name: 'NVIDIA Corporation', weight: 0.068 },
  { symbol: 'MSFT', name: 'Microsoft Corporation', weight: 0.064 },
  { symbol: 'AMZN', name: 'Amazon.com, Inc.', weight: 0.037 },
  { symbol: 'META', name: 'Meta Platforms, Inc.', weight: 0.026 },
  { symbol: 'GOOGL', name: 'Alphabet Inc. Clase A', weight: 0.021 },
  { symbol: 'BRK-B', name: 'Berkshire Hathaway Inc.', weight: 0.017 },
  { symbol: 'AVGO', name: 'Broadcom Inc.', weight: 0.016 },
  { symbol: 'TSLA', name: 'Tesla, Inc.', weight: 0.014 },
  { symbol: 'JPM', name: 'JPMorgan Chase & Co.', weight: 0.013 },
]

function dateOffset(days: number): string {
  return new Date(Date.now() + days * DAY * 1000).toISOString().slice(0, 10)
}

export function genFundamentals(id: AssetId): Fundamentals | null {
  const entry = entryFor(id)
  const cls = classOf(id) as AssetClass
  const price = currentPrice(id)
  const r = rngFor(id, 'fund')
  const asOf = new Date().toISOString()
  const base = { ...EMPTY_FUNDAMENTALS, asOf, sources: [cls === 'crypto' ? 'coingecko' : 'yahoo'] }

  if (cls === 'stock') {
    const shares = sharesFor(id)
    const fin = entry.sector === 'Finanzas'
    const growth = between(r, -0.05, 0.35)
    const pe = between(r, 11, 48)
    const eps = price / pe
    const revenue = shares * price * between(r, 0.12, 0.45)
    const cash = revenue * between(r, 0.05, 0.4)
    return {
      ...base,
      description: `${entry.name} es una compañía del sector ${entry.sector ?? 'general'} con presencia internacional. Esta descripción es de ejemplo (datos simulados).`,
      sector: entry.sector ?? null,
      industry: entry.sector ? `${entry.sector} — industria principal` : null,
      website: `https://www.${symbolOfId(id).toLowerCase().replace(/[^a-z]/g, '')}.example.com`,
      employees: Math.round(between(r, 8_000, 220_000)),
      country: entry.country,
      marketCap: price * shares,
      enterpriseValue: price * shares + cash * between(r, -0.5, 0.8),
      peTrailing: pe,
      peForward: pe * between(r, 0.8, 0.95),
      peg: r() > 0.25 ? between(r, 0.7, 3.2) : null,
      priceToBook: between(r, 1.1, 14),
      priceToSales: between(r, 1.2, 12),
      evToEbitda: fin ? null : between(r, 7, 32),
      epsTrailing: eps,
      epsForward: eps * between(r, 1.05, 1.25),
      revenueTtm: revenue,
      revenueGrowthYoy: growth,
      earningsGrowthYoy: growth * between(r, 0.6, 1.8),
      grossMargin: fin ? null : between(r, 0.28, 0.76),
      operatingMargin: between(r, 0.08, 0.42),
      profitMargin: between(r, 0.05, 0.3),
      roe: between(r, 0.06, 0.45),
      roa: between(r, 0.02, 0.2),
      freeCashFlow: fin ? null : revenue * between(r, 0.05, 0.28),
      totalCash: cash,
      totalDebt: cash * between(r, 0.4, 2.4),
      debtToEquity: between(r, 0.15, 2.2),
      currentRatio: fin ? null : between(r, 0.8, 2.8),
      dividendYield: r() > 0.3 ? between(r, 0.004, 0.05) : null,
      payoutRatio: r() > 0.3 ? between(r, 0.1, 0.7) : null,
      beta: between(r, 0.6, 1.9),
      sharesOutstanding: shares,
      shortPctFloat: between(r, 0.004, 0.06),
      insiderPct: between(r, 0.001, 0.12),
      institutionalPct: between(r, 0.45, 0.93),
      analystTargetMean: price * between(r, 0.95, 1.25),
      analystTargetHigh: price * between(r, 1.3, 1.6),
      analystTargetLow: price * between(r, 0.65, 0.92),
      analystRatingMean: between(r, 1.5, 3.3),
      analystCount: Math.round(between(r, 12, 48)),
      nextEarningsDate: dateOffset(Math.round(between(r, 6, 60))),
      extra: {
        'Recompra de acciones (TTM)': Math.round(between(r, 1, 80) * 1e8),
        'Empleados a tiempo completo': Math.round(between(r, 8_000, 220_000)),
        'Fecha de cierre fiscal': 'Diciembre',
      },
      sources: ['yahoo', 'sec_edgar'],
    }
  }

  if (cls === 'etf' || cls === 'fund') {
    return {
      ...base,
      description: `${entry.name}: vehículo de inversión diversificado (datos simulados).`,
      country: entry.country,
      marketCap: null,
      expenseRatio: between(r, 0.0003, 0.0075),
      totalAssets: between(r, 2e9, 6e11),
      dividendYield: between(r, 0.002, 0.03),
      beta: between(r, 0.6, 1.3),
      peTrailing: between(r, 12, 28),
      topHoldings: SP500_TOP.map((h) => ({ ...h, weight: h.weight * between(r, 0.85, 1.15) })),
      extra: { 'Réplica': 'Física', 'Domicilio': entry.country ?? '—' },
    }
  }

  if (cls === 'crypto') {
    const supply = sharesFor(id)
    const known = symbolOfId(id) === 'BTC' ? 21e6 : null
    return {
      ...base,
      description: `${entry.name} es una criptomoneda descentralizada (datos simulados).`,
      marketCap: price * supply,
      circulatingSupply: supply,
      totalSupply: known ?? supply * between(r, 1, 1.15),
      maxSupply: known,
      fullyDilutedValuation: price * (known ?? supply * between(r, 1.1, 1.6)),
      allTimeHigh: price * between(r, 1.1, 3.2),
      allTimeHighDate: dateOffset(-Math.round(between(r, 40, 900))),
      sources: ['coingecko'],
    }
  }

  if (cls === 'bond') {
    return {
      ...base,
      description: `${entry.name} (datos simulados).`,
      yieldToMaturity: price / 100,
      coupon: symbolOfId(id) === 'DGS2' ? null : 0.0425,
      maturityDate: symbolOfId(id) === 'DGS2' ? null : dateOffset(3650),
      sources: ['fred'],
    }
  }

  return null
}

export function allCatalogIds(): AssetId[] {
  return CATALOG.map((e) => e.id)
}

export function dayHash(id: string): number {
  return hashString(id)
}
