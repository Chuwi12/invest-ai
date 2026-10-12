import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { Quote } from '@/api/types'
import { LivePrice } from './LivePrice'

const base = { id: 'crypto:BTC', price: 67123.4, change: 10, changePct: 0.1, marketState: 'continuous', source: 'binance', timestamp: '2026-01-01T00:00:00Z' } as unknown as Quote

describe('LivePrice', () => {
  it('rotula con la moneda de la cotización (USDT) y explica Tether', () => {
    render(<LivePrice quote={{ ...base, currency: 'USDT' }} dot={false} />)
    const el = screen.getByTestId('live-price')
    expect(el.textContent).toContain('USDT')
    expect(el.textContent).not.toContain('$')
    expect(el.getAttribute('title')).toBe('Tether (stablecoin ≈ USD), cotización de Binance')
  })
  it('USD sigue siendo $', () => {
    render(<LivePrice quote={{ ...base, id: 'stock:AAPL', currency: 'USD' }} dot={false} />)
    const el = screen.getByTestId('live-price')
    expect(el.textContent).toContain('$')
    expect(el.getAttribute('title')).toBeNull()
  })
})
