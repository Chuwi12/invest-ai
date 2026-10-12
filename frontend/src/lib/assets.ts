import type { AssetClass, AssetId, Horizon, MarketState, SentimentLabel, Trend, Verdict } from '@/api/types'

export const CLASS_LABEL: Record<AssetClass, string> = {
  stock: 'Acciones',
  etf: 'ETFs',
  fund: 'Fondos',
  index: 'Índices',
  crypto: 'Criptomonedas',
  forex: 'Divisas',
  commodity: 'Materias primas',
  bond: 'Bonos',
  future: 'Futuros',
  option: 'Opciones',
  other: 'Otros',
}

export const CLASS_SINGULAR: Record<AssetClass, string> = {
  stock: 'Acción',
  etf: 'ETF',
  fund: 'Fondo',
  index: 'Índice',
  crypto: 'Cripto',
  forex: 'Divisa',
  commodity: 'Materia prima',
  bond: 'Bono',
  future: 'Futuro',
  option: 'Opción',
  other: 'Otro',
}

/** Orden en el que se agrupan los resultados de búsqueda. */
export const CLASS_ORDER: AssetClass[] = [
  'stock',
  'etf',
  'crypto',
  'index',
  'forex',
  'commodity',
  'bond',
  'future',
  'fund',
  'option',
  'other',
]

export function parseAssetId(id: string): { class: AssetClass; symbol: string } {
  const i = id.indexOf(':')
  if (i === -1) return { class: 'other', symbol: id }
  return { class: id.slice(0, i) as AssetClass, symbol: id.slice(i + 1) }
}

export function symbolOf(id: AssetId | string): string {
  return parseAssetId(id).symbol
}

/** Enlace a la ficha: el id va percent-encoded porque contiene `:`, `^`, `=`... */
export function assetHref(id: AssetId | string): string {
  return `/asset/${encodeURIComponent(id)}`
}

export const MARKET_STATE_LABEL: Record<MarketState, string> = {
  pre: 'Pre-mercado',
  regular: 'Mercado abierto',
  post: 'Post-mercado',
  closed: 'Mercado cerrado',
  continuous: 'Abierto 24/7',
  unknown: 'Estado desconocido',
}

export const VERDICT_LABEL: Record<Verdict, string> = {
  buy: 'Comprar',
  accumulate: 'Acumular',
  hold: 'Mantener',
  wait: 'Esperar',
  reduce: 'Reducir',
  avoid: 'Evitar',
}

export const VERDICT_HINT: Record<Verdict, string> = {
  buy: 'Buen momento para comprar',
  accumulate: 'Comprar de forma escalonada',
  hold: 'Mantener si ya se tiene; no añadir',
  wait: 'Esperar un mejor punto de entrada o un catalizador',
  reduce: 'Reducir posición',
  avoid: 'Evitar o vender',
}

export const HORIZON_LABEL: Record<Horizon, string> = {
  short: 'Corto plazo',
  medium: 'Medio plazo',
  long: 'Largo plazo',
}

export const HORIZON_HINT: Record<Horizon, string> = {
  short: 'días a semanas',
  medium: 'meses',
  long: 'años',
}

export const SENTIMENT_LABEL: Record<SentimentLabel, string> = {
  very_negative: 'Muy negativo',
  negative: 'Negativo',
  neutral: 'Neutral',
  positive: 'Positivo',
  very_positive: 'Muy positivo',
}

export const TREND_LABEL: Record<Trend, string> = {
  strong_up: 'Alcista fuerte',
  up: 'Alcista',
  sideways: 'Lateral',
  down: 'Bajista',
  strong_down: 'Bajista fuerte',
}

export const TOOL_LABEL: Record<string, string> = {
  get_quote: 'Cotización',
  get_history: 'Histórico',
  get_indicators: 'Indicadores técnicos',
  get_fundamentals: 'Fundamentales',
  get_news: 'Noticias',
  get_macro: 'Contexto macro',
  search_assets: 'Búsqueda de activos',
  compare_assets: 'Comparación',
  analyze_asset: 'Análisis completo',
}

export function assetHrefLabel(id: AssetId | string): string {
  return symbolOf(id)
}

/** Etiquetas del panel de mercado fijo (la API devuelve solo cotizaciones, sin nombre). */
export const MARKET_LABEL: Record<string, string> = {
  '^GSPC': 'S&P 500',
  '^IXIC': 'Nasdaq Composite',
  '^STOXX50E': 'Euro Stoxx 50',
  '^IBEX': 'IBEX 35',
  BTC: 'Bitcoin',
  ETH: 'Ethereum',
  'GC=F': 'Oro',
  'CL=F': 'Petróleo WTI',
  'EURUSD=X': 'EUR/USD',
  '^TNX': 'Bono EE. UU. 10 a.',
  '^VIX': 'VIX (volatilidad)',
}

/** De dónde sale el dato que pide cada herramienta del modelo (para la transparencia del chat). */
export const TOOL_SOURCE: Record<string, string> = {
  get_quote: 'Cotización del proveedor de mercado (último tick si hay)',
  get_history: 'Velas del proveedor de mercado',
  get_indicators: 'Cálculo propio sobre velas diarias',
  get_fundamentals: 'Proveedores de fundamentales (Yahoo, FMP, CoinGecko…)',
  get_news: 'Proveedores de noticias (GDELT, Google News, Yahoo…)',
  get_macro: 'Indicadores macro (FRED, Yahoo)',
  search_assets: 'Búsqueda de proveedores',
  compare_assets: 'Cotización e indicadores de cada activo',
  analyze_asset: 'Análisis completo (snapshot + puntuación + modelo)',
}
