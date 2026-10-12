import type { AssetClass, AssetId, AssetSummary } from '@/api/types'

export interface CatalogEntry {
  id: AssetId
  name: string
  price: number
  /** Volatilidad anualizada aproximada (0.25 = 25 %). */
  vol: number
  currency: string | null
  exchange: string | null
  country: string | null
  sector?: string
  /** Segundos de retraso del dato (por defecto 0). */
  delay?: number
}

type Row = [AssetId, string, number, number, string | null, string | null, string | null, string?, number?]

const rows: Row[] = [
  // Acciones EE. UU.
  ['stock:AAPL', 'Apple Inc.', 227.4, 0.27, 'USD', 'NASDAQ', 'US', 'Tecnología'],
  ['stock:MSFT', 'Microsoft Corporation', 438.1, 0.24, 'USD', 'NASDAQ', 'US', 'Tecnología'],
  ['stock:NVDA', 'NVIDIA Corporation', 131.8, 0.55, 'USD', 'NASDAQ', 'US', 'Semiconductores'],
  ['stock:TSLA', 'Tesla, Inc.', 248.7, 0.58, 'USD', 'NASDAQ', 'US', 'Automoción'],
  ['stock:AMZN', 'Amazon.com, Inc.', 187.9, 0.31, 'USD', 'NASDAQ', 'US', 'Consumo'],
  ['stock:GOOGL', 'Alphabet Inc.', 171.3, 0.29, 'USD', 'NASDAQ', 'US', 'Tecnología'],
  ['stock:META', 'Meta Platforms, Inc.', 568.2, 0.38, 'USD', 'NASDAQ', 'US', 'Tecnología'],
  ['stock:BRK-B', 'Berkshire Hathaway Inc.', 456.3, 0.17, 'USD', 'NYSE', 'US', 'Finanzas'],
  ['stock:JPM', 'JPMorgan Chase & Co.', 211.6, 0.22, 'USD', 'NYSE', 'US', 'Finanzas'],
  ['stock:KO', 'The Coca-Cola Company', 71.9, 0.14, 'USD', 'NYSE', 'US', 'Consumo defensivo'],
  ['stock:NFLX', 'Netflix, Inc.', 712.4, 0.36, 'USD', 'NASDAQ', 'US', 'Comunicación'],
  ['stock:AMD', 'Advanced Micro Devices, Inc.', 158.2, 0.5, 'USD', 'NASDAQ', 'US', 'Semiconductores'],
  ['stock:PLTR', 'Palantir Technologies Inc.', 38.6, 0.7, 'USD', 'NASDAQ', 'US', 'Software'],
  // Acciones España y Europa
  ['stock:SAN.MC', 'Banco Santander, S.A.', 4.62, 0.24, 'EUR', 'BME', 'ES', 'Finanzas', 900],
  ['stock:ITX.MC', 'Industria de Diseño Textil, S.A.', 51.8, 0.21, 'EUR', 'BME', 'ES', 'Consumo', 900],
  ['stock:IBE.MC', 'Iberdrola, S.A.', 14.12, 0.16, 'EUR', 'BME', 'ES', 'Utilities', 900],
  ['stock:BBVA.MC', 'Banco Bilbao Vizcaya Argentaria, S.A.', 10.34, 0.26, 'EUR', 'BME', 'ES', 'Finanzas', 900],
  ['stock:TEF.MC', 'Telefónica, S.A.', 4.05, 0.2, 'EUR', 'BME', 'ES', 'Telecomunicaciones', 900],
  ['stock:ASML.AS', 'ASML Holding N.V.', 690.5, 0.38, 'EUR', 'Euronext Ámsterdam', 'NL', 'Semiconductores', 900],
  ['stock:SAP.DE', 'SAP SE', 214.3, 0.25, 'EUR', 'XETRA', 'DE', 'Software', 900],
  // ETFs y fondos
  ['etf:SPY', 'SPDR S&P 500 ETF Trust', 573.4, 0.15, 'USD', 'NYSE Arca', 'US'],
  ['etf:QQQ', 'Invesco QQQ Trust', 485.6, 0.2, 'USD', 'NASDAQ', 'US'],
  ['etf:VOO', 'Vanguard S&P 500 ETF', 527.8, 0.15, 'USD', 'NYSE Arca', 'US'],
  ['etf:VWCE.DE', 'Vanguard FTSE All-World UCITS ETF', 128.4, 0.14, 'EUR', 'XETRA', 'IE', undefined, 900],
  ['etf:IWDA.AS', 'iShares Core MSCI World UCITS ETF', 105.2, 0.14, 'EUR', 'Euronext Ámsterdam', 'IE', undefined, 900],
  ['etf:GLD', 'SPDR Gold Shares', 238.9, 0.15, 'USD', 'NYSE Arca', 'US'],
  ['etf:TLT', 'iShares 20+ Year Treasury Bond ETF', 91.7, 0.15, 'USD', 'NASDAQ', 'US'],
  ['fund:VFIAX', 'Vanguard 500 Index Fund Admiral Shares', 512.3, 0.15, 'USD', 'Fondo mutuo', 'US'],
  // Índices
  ['index:^GSPC', 'S&P 500', 5732.3, 0.14, 'USD', 'SNP', 'US', undefined, 0],
  ['index:^IXIC', 'NASDAQ Composite', 18137.2, 0.19, 'USD', 'NASDAQ', 'US'],
  ['index:^DJI', 'Dow Jones Industrial Average', 42050.3, 0.13, 'USD', 'DJI', 'US'],
  ['index:^STOXX50E', 'EURO STOXX 50', 4930.7, 0.15, 'EUR', 'STOXX', 'EU', undefined, 900],
  ['index:^IBEX', 'IBEX 35', 11620.4, 0.15, 'EUR', 'BME', 'ES', undefined, 900],
  ['index:^GDAXI', 'DAX Performance Index', 19120.8, 0.15, 'EUR', 'XETRA', 'DE', undefined, 900],
  ['index:^N225', 'Nikkei 225', 38900.2, 0.18, 'JPY', 'Osaka', 'JP', undefined, 1200],
  ['index:^VIX', 'CBOE Volatility Index', 20.4, 0.85, 'USD', 'CBOE', 'US'],
  // Cripto
  ['crypto:BTC', 'Bitcoin', 67234.5, 0.45, 'USD', null, null],
  ['crypto:ETH', 'Ethereum', 2610.4, 0.58, 'USD', null, null],
  ['crypto:SOL', 'Solana', 148.2, 0.8, 'USD', null, null],
  ['crypto:XRP', 'XRP', 0.5314, 0.7, 'USD', null, null],
  ['crypto:DOGE', 'Dogecoin', 0.1082, 0.9, 'USD', null, null],
  ['crypto:ADA', 'Cardano', 0.3421, 0.75, 'USD', null, null],
  ['crypto:SHIB', 'Shiba Inu', 0.00001634, 0.95, 'USD', null, null],
  ['crypto:PEPE', 'Pepe', 0.00000812, 1.2, 'USD', null, null],
  // Divisas
  ['forex:EURUSD=X', 'EUR/USD', 1.0912, 0.07, 'USD', 'CCY', null],
  ['forex:GBPUSD=X', 'GBP/USD', 1.3081, 0.08, 'USD', 'CCY', null],
  ['forex:USDJPY=X', 'USD/JPY', 148.62, 0.1, 'JPY', 'CCY', null],
  ['forex:EURGBP=X', 'EUR/GBP', 0.8342, 0.06, 'GBP', 'CCY', null],
  // Materias primas y futuros
  ['commodity:GC=F', 'Oro (futuro continuo)', 2680.4, 0.15, 'USD', 'COMEX', 'US'],
  ['commodity:CL=F', 'Petróleo crudo WTI', 71.8, 0.35, 'USD', 'NYMEX', 'US'],
  ['commodity:SI=F', 'Plata (futuro continuo)', 31.5, 0.28, 'USD', 'COMEX', 'US'],
  ['commodity:NG=F', 'Gas natural', 2.31, 0.55, 'USD', 'NYMEX', 'US'],
  ['future:ES=F', 'E-mini S&P 500 (continuo)', 5750.25, 0.14, 'USD', 'CME', 'US'],
  ['future:NQ=F', 'E-mini NASDAQ-100 (continuo)', 20110.5, 0.19, 'USD', 'CME', 'US'],
  // Bonos
  ['bond:^TNX', 'Rendimiento del Tesoro EE. UU. a 10 años', 4.18, 0.2, 'USD', 'CBOE', 'US'],
  ['bond:DGS10', 'Treasury Constant Maturity 10 años', 4.05, 0.15, 'USD', 'FRED', 'US'],
  ['bond:DGS2', 'Treasury Constant Maturity 2 años', 3.62, 0.18, 'USD', 'FRED', 'US'],
  // Opciones
  ['option:AAPL261218C00200000', 'AAPL 18-dic-2026 Call 200,00', 31.2, 0.6, 'USD', 'OPR', 'US'],
]

export const CATALOG: CatalogEntry[] = rows.map(([id, name, price, vol, currency, exchange, country, sector, delay]) => {
  const e: CatalogEntry = { id, name, price, vol, currency, exchange, country }
  if (sector !== undefined) e.sector = sector
  if (delay !== undefined) e.delay = delay
  return e
})

const byId = new Map(CATALOG.map((e) => [e.id, e]))

export function findEntry(id: string): CatalogEntry | undefined {
  return byId.get(id as AssetId)
}

export function classOf(id: string): AssetClass {
  return id.slice(0, id.indexOf(':')) as AssetClass
}

export function symbolOfId(id: string): string {
  return id.slice(id.indexOf(':') + 1)
}

export function toSummary(e: CatalogEntry): AssetSummary {
  return {
    id: e.id,
    symbol: symbolOfId(e.id),
    name: e.name,
    class: classOf(e.id),
    exchange: e.exchange,
    currency: e.currency,
    country: e.country,
    source: classOf(e.id) === 'crypto' ? 'coingecko' : 'yahoo',
  }
}

function norm(s: string): string {
  return s.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase()
}

export function searchCatalog(q: string, limit: number, cls?: AssetClass): CatalogEntry[] {
  const needle = norm(q.trim())
  if (!needle) return []
  const scored: [number, CatalogEntry][] = []
  for (const e of CATALOG) {
    if (cls && classOf(e.id) !== cls) continue
    const sym = norm(symbolOfId(e.id))
    const name = norm(e.name)
    let score = -1
    if (sym === needle) score = 0
    else if (sym.startsWith(needle)) score = 1
    else if (name.startsWith(needle)) score = 2
    else if (name.split(/[\s.,/-]+/).some((w) => w.startsWith(needle))) score = 3
    else if (sym.includes(needle) || name.includes(needle)) score = 4
    if (score >= 0) scored.push([score, e])
  }
  scored.sort((a, b) => a[0] - b[0])
  return scored.slice(0, limit).map(([, e]) => e)
}

/** Panel fijo del Inicio (docs/API.md). */
export const MARKET_PANEL: AssetId[] = [
  'index:^GSPC',
  'index:^IXIC',
  'index:^STOXX50E',
  'index:^IBEX',
  'crypto:BTC',
  'crypto:ETH',
  'commodity:GC=F',
  'commodity:CL=F',
  'forex:EURUSD=X',
  'bond:^TNX',
  'index:^VIX',
]
