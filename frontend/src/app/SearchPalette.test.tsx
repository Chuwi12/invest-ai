import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import type { AssetSummary } from '@/api/types'
import { App } from '@/app/App'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'

const sum = (symbol: string, name: string): AssetSummary =>
  ({ id: `stock:${symbol}`, symbol, name, class: 'stock', exchange: 'NASDAQ', country: 'US', source: 'x', currency: 'USD' }) as AssetSummary

describe('SearchPalette: carreras al teclear', () => {
  beforeAll(async () => {
    await Promise.all([import('@/pages/Dashboard'), import('@/pages/AssetPage'), import('@/app/SearchPalette')])
  })
  beforeEach(() => setApi(mockApi))

  it('Enter no elige un resultado de la búsqueda anterior mientras llegan los de lo que se acaba de teclear', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const recorded: Array<[string, string]> = []
    setApi({
      ...mockApi,
      search: async (q) => {
        if (q === 'te') return [sum('TSCO', 'Tesco PLC')]
        await gate
        return [sum('TSLA', 'Tesla Inc.')]
      },
      recordSearch: async (query, selected) => void recorded.push([query, selected]),
    })
    renderApp(<App />, '/dashboard')
    await screen.findByRole('heading', { name: 'Panel' })
    await userEvent.keyboard('/')
    const input = await screen.findByRole('combobox')
    await userEvent.type(input, 'te')
    await screen.findByText('Tesco PLC')
    // El usuario sigue escribiendo y pulsa Enter sin esperar: «Tesco» es de la búsqueda «te», no de «tesla».
    await userEvent.type(input, 'sla')
    await userEvent.keyboard('{Enter}')
    expect(recorded).toEqual([])
    expect(screen.getByRole('combobox')).toBeInTheDocument()
    // Llegan los resultados de «tesla»: ahora sí.
    release()
    await screen.findByText('Tesla Inc.')
    await userEvent.keyboard('{Enter}')
    await waitFor(() => expect(recorded).toEqual([['tesla', 'stock:TSLA']]))
  })
})
