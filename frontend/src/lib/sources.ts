import { parseAssetId } from './assets'

const NAMES: Record<string, string> = {
  yahoo: 'Yahoo Finance',
  yahoo_rss: 'Yahoo Finance',
  binance: 'Binance',
  coingecko: 'CoinGecko',
  finnhub: 'Finnhub',
  alphavantage: 'Alpha Vantage',
  fmp: 'Financial Modeling Prep',
  twelvedata: 'Twelve Data',
  fred: 'FRED',
  sec: 'SEC EDGAR',
  edgar: 'SEC EDGAR',
  gdelt: 'GDELT',
  google_news: 'Google News',
  newsapi: 'NewsAPI',
  marketaux: 'Marketaux',
  cryptopanic: 'CryptoPanic',
  ecb: 'BCE',
}

export function sourceName(provider: string): string {
  const key = provider.toLowerCase()
  if (key.startsWith('provider:')) return sourceName(key.slice(9))
  return NAMES[key] ?? provider
}

/** Página pública del proveedor para el activo, solo si se conoce con certeza. */
export function sourceUrl(provider: string | null | undefined, assetId?: string): string | null {
  if (!provider) return null
  const p = provider.toLowerCase()
  const sym = assetId ? parseAssetId(assetId).symbol : null
  const enc = sym ? encodeURIComponent(sym) : null
  switch (p) {
    case 'yahoo':
    case 'yahoo_rss':
      return enc ? `https://finance.yahoo.com/quote/${enc}` : 'https://finance.yahoo.com'
    case 'binance':
      return enc ? `https://www.binance.com/en/trade/${enc}_USDT` : 'https://www.binance.com'
    case 'coingecko':
      return 'https://www.coingecko.com'
    case 'finnhub':
      return 'https://finnhub.io'
    case 'fred':
      return 'https://fred.stlouisfed.org'
    case 'sec':
    case 'edgar':
      return enc ? `https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK=${enc}` : 'https://www.sec.gov/edgar'
    case 'gdelt':
      return 'https://www.gdeltproject.org'
    default:
      return null
  }
}

/** Solo se enlazan URLs http(s): descarta `javascript:`, `data:` y similares. */
export function safeHttpUrl(url: string | null | undefined): string | null {
  if (!url) return null
  try {
    const u = new URL(url)
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.toString() : null
  } catch {
    return null
  }
}
