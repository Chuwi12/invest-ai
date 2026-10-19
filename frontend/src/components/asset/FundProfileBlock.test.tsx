import { screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import type { AssetSummary, FundProfile } from '@/api/types'
import { genFundamentals } from '@/mock/market'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'
import { FundProfileBlock } from './FundProfileBlock'
import { FundamentalsPanel } from './FundamentalsPanel'

// Recorte de la respuesta real de Yahoo para SPY (19-oct-2026), ya tratada por el backend.
const SPY: FundProfile = {
  family: 'State Street Investment Management', category: 'Large Blend', legalType: 'Exchange Traded Fund', inceptionDate: '1993-01-22',
  expenseRatio: 0.000945, grossExpenseRatio: null, frontLoad: null, deferredLoad: null, twelveB1: null, turnover: 0.03,
  totalAssets: 809_216_114_688, dividendYield: 0.0099, beta3y: 1, ytdReturn: 0.14380279, threeYearReturn: 0.23161751, fiveYearReturn: 0.1357875,
  topHoldings: [
    { symbol: 'NVDA', name: 'NVIDIA Corp', weight: 0.0836162 }, { symbol: 'AAPL', name: 'Apple Inc', weight: 0.0735303 },
    { symbol: 'MSFT', name: 'Microsoft Corp', weight: 0.0576201 },
  ],
  sectorWeights: [{ name: 'technology', weight: 0.4045 }, { name: 'financial_services', weight: 0.112 }, { name: 'realestate', weight: 0.017 }],
  assetMix: [{ name: 'stock', weight: 0.9975 }, { name: 'cash', weight: 0.0025 }], bondRatings: [],
  equity: { priceToEarnings: 24.2, priceToBook: 5.25, priceToSales: 3.64, priceToCashflow: 18.78, threeYearEarningsGrowth: null, medianMarketCap: null },
  bond: null, source: 'yahoo', asOf: '2026-10-19T14:15:00Z', sourceUrl: 'https://finance.yahoo.com/quote/SPY/holdings',
}

const summary = (symbol: string, cls = 'etf') => ({ id: `${cls}:${symbol}`, symbol, name: symbol, class: cls, exchange: null, country: null, source: 'yahoo', currency: 'USD' }) as AssetSummary

describe('FundProfileBlock', () => {
  it('muestra costes, patrimonio, posiciones con peso, sectores y la fuente con enlace', () => {
    renderApp(<FundProfileBlock profile={SPY} assetId="etf:SPY" currency="USD" />)
    const block = screen.getByTestId('fund-profile')
    expect(within(block).getByRole('heading', { name: 'Perfil del fondo' })).toBeInTheDocument()
    expect(block).toHaveTextContent('State Street Investment Management · Large Blend · Exchange Traded Fund')
    expect(block).toHaveTextContent('Gastos corrientes (TER)0,09 %')
    expect(block).toHaveTextContent('Rotación de la cartera3,0 %')
    expect(block).toHaveTextContent('Patrimonio')
    expect(within(block).getByLabelText('Diez primeras posiciones')).toHaveTextContent('NVDANVIDIA Corp8,36 %')
    expect(within(block).getByLabelText('Diez primeras posiciones')).toHaveTextContent('suman 21,5 %')
    expect(within(block).getByLabelText('Sectores')).toHaveTextContent('Tecnología40,5 %')
    expect(within(block).getByLabelText('Sectores')).toHaveTextContent('Inmobiliario1,7 %')
    expect(block).toHaveTextContent('PER24,2')
    expect(within(block).getByRole('link', { name: /Yahoo/i })).toHaveAttribute('href', 'https://finance.yahoo.com/quote/SPY/holdings')
    expect(block.querySelector('.prov time')).toHaveAttribute('datetime', '2026-10-19T14:15:00Z')
  })

  it('lo que el proveedor no da no se pinta: sin comisiones brutas ni renta fija ni calificaciones', () => {
    renderApp(<FundProfileBlock profile={SPY} assetId="etf:SPY" currency="USD" />)
    const block = screen.getByTestId('fund-profile')
    expect(block).not.toHaveTextContent('Gastos brutos')
    expect(block).not.toHaveTextContent('Comisión de entrada')
    expect(block).not.toHaveTextContent('Cartera de renta fija')
    expect(block).not.toHaveTextContent('Calificaciones crediticias')
    expect(block).not.toHaveTextContent('Crec. beneficios')
  })

  it('un ETF de bonos sin posiciones muestra el reparto y las calificaciones, no una tabla vacía', () => {
    const tlt: FundProfile = { ...SPY, topHoldings: [], sectorWeights: [], equity: null, assetMix: [{ name: 'bond', weight: 0.996 }], bondRatings: [{ name: 'us_government', weight: 0.996 }] }
    renderApp(<FundProfileBlock profile={tlt} assetId="etf:TLT" currency="USD" />)
    const block = screen.getByTestId('fund-profile')
    expect(within(block).queryByLabelText('Diez primeras posiciones')).toBeNull()
    expect(within(block).queryByLabelText('Sectores')).toBeNull()
    expect(within(block).getByLabelText('Tipo de activo')).toHaveTextContent('Bonos99,6 %')
    expect(within(block).getByLabelText('Calificaciones crediticias')).toHaveTextContent('Deuda del Tesoro EE. UU.99,6 %')
  })
})

describe('FundamentalsPanel con perfil de fondo', () => {
  beforeEach(() => setApi(mockApi))

  it('la ficha de un ETF muestra «Perfil del fondo» y no duplica las posiciones', async () => {
    renderApp(<FundamentalsPanel summary={summary('SPY')} />)
    const block = await screen.findByTestId('fund-profile')
    expect(block).toHaveTextContent('Fuente')
    expect(within(block).getAllByRole('row').length).toBeGreaterThanOrEqual(10)
    expect(screen.queryByLabelText('Principales posiciones')).toBeNull()
    expect(screen.queryByLabelText('Fondo / ETF')).toBeNull()
  })

  it('una acción no tiene el bloque', async () => {
    renderApp(<FundamentalsPanel summary={summary('AAPL', 'stock')} />)
    expect(await screen.findByLabelText('Valoración')).toBeInTheDocument()
    expect(screen.queryByTestId('fund-profile')).toBeNull()
  })

  it('un ETF sin perfil conserva el bloque antiguo «Fondo / ETF» y las posiciones', async () => {
    const f = genFundamentals('etf:SPY')!
    setApi({ ...mockApi, fundamentals: async () => ({ ...f, fundProfile: null }) })
    renderApp(<FundamentalsPanel summary={summary('SPY')} />)
    expect(await screen.findByLabelText('Fondo / ETF')).toBeInTheDocument()
    expect(screen.getByLabelText('Principales posiciones')).toBeInTheDocument()
    expect(screen.queryByTestId('fund-profile')).toBeNull()
  })
})
