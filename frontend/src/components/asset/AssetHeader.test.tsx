import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AssetSummary, Quote } from '@/api/types'
import { renderApp } from '@/test/utils'
import { AssetHeader } from './AssetHeader'

// 13-oct-2026 12:35:00 ET, con la bolsa abierta.
const NOW = Date.parse('2026-10-13T16:35:00Z')
const NOTE = 'El proveedor declara un retraso de hasta 15 s para esta bolsa'

const summary: AssetSummary = {
  id: 'stock:AAPL', class: 'stock', symbol: 'AAPL', name: 'Apple Inc.', exchange: 'NasdaqGS', currency: 'USD', country: 'US', source: 'yahoo',
}

const quote = (over: Partial<Quote> = {}): Quote => ({
  id: 'stock:AAPL', price: 344.96, change: 1, changePct: 0.3, open: 343, high: 345, low: 342, prevClose: 343.9, volume: 1e6, marketCap: 5e12,
  currency: 'USD', marketState: 'regular', timestamp: new Date(NOW - 2000).toISOString(), source: 'yahoo', delaySecs: 15,
  sourceUrl: 'https://finance.yahoo.com/quote/AAPL', timestampOrigin: 'provider', ...over,
})

const header = (q: Quote | null) => renderApp(<AssetHeader summary={summary} quote={q} isFavorite={false} />)

describe('AssetHeader: antigüedad medida frente a retraso declarado', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
  })
  afterEach(() => vi.useRealTimers())

  it('Yahoo declara 15 s pero el dato tiene 2 s: se ve la antigüedad medida, el 15 solo va en un tooltip y no hay «retrasado»', () => {
    const { container } = header(quote())
    expect(container.querySelector('.prov time')).toHaveTextContent('ahora mismo')
    expect(container.textContent).not.toMatch(/Retraso de 15 s/i)
    expect(container.textContent).not.toMatch(/retrasado/i)
    expect(screen.getByRole('img', { name: NOTE })).toHaveAttribute('title', NOTE)
  })

  it('con la sesión abierta y el dato más viejo que lo declarado + margen se marca «retrasado» con la medida en el motivo', () => {
    header(quote({ timestamp: new Date(NOW - 10 * 60_000).toISOString() }))
    const badge = screen.getByText('retrasado')
    expect(badge.getAttribute('title')).toContain('10 min')
    expect(badge.getAttribute('title')).toContain('hasta 15 s')
    expect(screen.getByRole('img', { name: NOTE })).toBeInTheDocument()
  })

  it('sin hora del proveedor no se presenta una medida: se avisa «hora de consulta» y la nota dice que no se pudo medir', () => {
    const { container } = header(quote({ timestampOrigin: 'local' }))
    expect(within(container).getByText(/hora de consulta/)).toBeInTheDocument()
    const icon = screen.getByRole('img', { name: /no se ha podido medir/ })
    expect(icon.getAttribute('title')).toContain(NOTE)
    expect(screen.queryByText('retrasado')).toBeNull()
  })

  it('mercado cerrado: dato de hace horas sin «retrasado» (el retraso declarado sigue siendo solo una nota)', () => {
    header(quote({ marketState: 'closed', timestamp: new Date(NOW - 20 * 3_600_000).toISOString() }))
    expect(screen.queryByText('retrasado')).toBeNull()
    expect(screen.getByRole('img', { name: NOTE })).toBeInTheDocument()
  })

  it.each([0, null])('sin retraso declarado (%s) no hay nota', (delaySecs) => {
    header(quote({ delaySecs }))
    expect(screen.queryByRole('img', { name: /retraso/i })).toBeNull()
  })
})
