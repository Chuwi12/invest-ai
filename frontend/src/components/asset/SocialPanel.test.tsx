import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import type { AssetSocial, AssetSummary } from '@/api/types'
import { mockApi } from '@/mock'
import { mockAssetSocial } from '@/mock/social'
import { renderApp } from '@/test/utils'
import { SocialPanel } from './SocialPanel'

const summary = (symbol: string) => ({ id: `stock:${symbol}`, symbol, name: symbol, class: 'stock', exchange: null, country: null, source: 'yahoo', currency: 'USD' }) as AssetSummary

describe('SocialPanel', () => {
  beforeEach(() => setApi(mockApi))

  it('muestra el aviso de opinión, las métricas y los extractos con enlace', async () => {
    renderApp(<SocialPanel summary={summary('AAPL')} />)
    expect(screen.getByRole('heading', { name: 'Redes sociales (opinión, no verificado)' })).toBeInTheDocument()
    expect(screen.getByRole('note')).toHaveTextContent(/no se pueden verificar/)
    expect(screen.getByRole('note')).toHaveTextContent(/No entran en la puntuación/)
    const metrics = await screen.findByLabelText('Métricas de redes sociales')
    expect(metrics).toHaveTextContent('Menciones (24 h)18 (Reddit 15, X 3)')
    expect(metrics).toHaveTextContent('×2,43 su media diaria (z 2,90')
    const link = screen.getByRole('link', { name: /AAPL parece fuerte/ })
    expect(link).toHaveAttribute('href', 'https://www.reddit.com/r/stocks/comments/m1/ejemplo/')
    expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    expect(screen.getAllByRole('listitem')).toHaveLength(3)
    await waitFor(() => expect(screen.getByTestId('x-budget')).toHaveTextContent('130 de 500 lecturas'))
  })

  it('sin histórico no inventa un cociente', async () => {
    const base = mockAssetSocial('stock:AAPL')
    const data: AssetSocial = { ...base, digest: { ...base.digest!, metrics: { ...base.digest!.metrics, ratio: null, z: null, baselineMean: null, baselineDays: 2 } } }
    setApi({ ...mockApi, assetSocial: async () => data })
    renderApp(<SocialPanel summary={summary('AAPL')} />)
    expect(await screen.findByLabelText('Métricas de redes sociales')).toHaveTextContent('sin histórico suficiente para compararlo con su media (2 bloques')
  })

  it('desactivado: lo dice y no pinta métricas', async () => {
    renderApp(<SocialPanel summary={summary('SOCIALOFF')} />)
    expect(await screen.findByText(/están desactivadas/)).toBeInTheDocument()
    expect(screen.queryByLabelText('Métricas de redes sociales')).not.toBeInTheDocument()
  })

  it('sin publicaciones', async () => {
    renderApp(<SocialPanel summary={summary('NOSOCIAL')} />)
    expect(await screen.findByText(/Sin publicaciones recientes/)).toBeInTheDocument()
  })

  it('el texto de un usuario se pinta como texto, nunca como HTML, y una URL peligrosa no es enlace', async () => {
    const base = mockAssetSocial('stock:AAPL')
    const evil = { ...base.digest!.excerpts[0]!, title: '<img src=x onerror=alert(1)> comprar', text: '', url: 'javascript:alert(1)' }
    setApi({ ...mockApi, assetSocial: async () => ({ ...base, digest: { ...base.digest!, excerpts: [evil] } }) })
    const { container } = renderApp(<SocialPanel summary={summary('AAPL')} />)
    expect(await screen.findByText(/comprar/)).toBeInTheDocument()
    expect(container.querySelector('img')).toBeNull()
    expect(screen.queryByRole('link')).toBeNull()
  })

  it('un error de red se muestra con reintento', async () => {
    setApi({ ...mockApi, assetSocial: async () => { throw new Error('caído') } })
    renderApp(<SocialPanel summary={summary('AAPL')} />)
    expect(await screen.findByText(/No se pudieron cargar los datos de redes/)).toBeInTheDocument()
  })
})
