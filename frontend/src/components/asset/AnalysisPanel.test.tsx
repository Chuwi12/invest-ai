import { screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import type { Analysis, AnalysisDepth, AssetSummary } from '@/api/types'
import { genAnalysis } from '@/mock/content'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'
import { AnalysisPanel } from './AnalysisPanel'

const summary = { id: 'stock:AAPL', symbol: 'AAPL', name: 'Apple', class: 'stock', exchange: null, country: null, source: 'yahoo', currency: 'USD' } as AssetSummary

function an(id: string, depth: AnalysisDepth, durationMs: number | null, model = 'modelo-a', asset: 'stock:AAPL' | 'stock:NVDA' = 'stock:AAPL'): Analysis {
  const base = genAnalysis(asset, 1, new Date(), { depth, durationMs: durationMs ?? 1 })
  const out: Analysis = { ...base, id, model, depth, durationMs }
  return out
}

function useApi(history: Analysis[], panel: Analysis[], model = 'modelo-a') {
  setApi({
    ...mockApi,
    analyses: async () => history,
    latestAnalysis: async () => history[0] ?? null,
    status: async () => ({ ...(await mockApi.status()), llm: { ...(await mockApi.status()).llm, model } }),
    dashboard: async () => ({ ...(await mockApi.dashboard()), latestAnalyses: panel }),
  })
}

describe('AnalysisPanel: tiempos medidos', () => {
  beforeEach(() => setApi(mockApi))

  it('estima con la mediana de análisis del mismo modelo y profundidad (histórico + panel, sin duplicar)', async () => {
    const h = [an('1', 'standard', 50_000), an('2', 'standard', 60_000), an('3', 'deep', 80_000), an('4', 'standard', 5_000, 'otro-modelo')]
    const panel = [h[0]!, an('5', 'standard', 40_000, 'modelo-a', 'stock:NVDA')]
    useApi(h, panel)
    renderApp(<AnalysisPanel summary={summary} quote={null} />)
    // estándar: {50, 60, 40} -> mediana 50 s, 3 análisis (el id 1 está en ambas fuentes)
    await waitFor(() => expect(screen.getByTestId('depth-estimate')).toHaveTextContent('≈ 50 s (mediana de 3 análisis previos)'))
    const group = screen.getByRole('radiogroup', { name: 'Profundidad del análisis' })
    expect(within(group).getByRole('radio', { name: /Profundo/ })).toHaveTextContent('≈ 1 min 20 s')
    expect(within(group).getByRole('radio', { name: /Rápido/ })).not.toHaveTextContent('≈')
    expect(document.body.textContent).not.toMatch(/≈ 10 s|≈ 30 s|60 s o más/)
  })

  it('sin mediciones con el modelo activo: lo dice y no inventa ningún número', async () => {
    useApi([an('1', 'standard', 50_000, 'modelo-a')], [], 'modelo-nuevo')
    renderApp(<AnalysisPanel summary={summary} quote={null} />)
    await screen.findByRole('button', { name: 'Generar análisis nuevo' })
    await screen.findByText(/Modelo: modelo-nuevo/)
    expect(screen.getByTestId('depth-estimate')).toHaveTextContent('sin medición todavía con este modelo')
    const group = screen.getByRole('radiogroup', { name: 'Profundidad del análisis' })
    expect(group.textContent).not.toMatch(/\d+ s/)
  })

  it('análisis antiguos sin duración ni profundidad no cuentan', async () => {
    const old = an('1', 'standard', 50_000)
    delete old.durationMs
    delete old.depth
    useApi([old], [])
    renderApp(<AnalysisPanel summary={summary} quote={null} />)
    await screen.findByRole('button', { name: 'Generar análisis nuevo' })
    await screen.findByText(/Modelo: modelo-a/)
    expect(screen.getByTestId('depth-estimate')).toHaveTextContent('sin medición todavía con este modelo')
  })
})
