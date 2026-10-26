import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { setApi } from '@/api'
import type { Analysis } from '@/api/types'
import { App } from '@/app/App'
import { AnalysisReport } from '@/components/AnalysisReport'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'

function setUrl(search: string) {
  window.history.pushState({}, '', `/${search}`)
}

describe('PerformancePage', () => {
  beforeAll(async () => {
    await import('@/pages/PerformancePage') // chunk perezoso: se transforma una vez, fuera de las esperas
  })
  beforeEach(() => setApi(mockApi))
  afterEach(() => setUrl(''))

  it('sin análisis (el estado real de hoy) es clara y no inventa ninguna cifra', async () => {
    setUrl('?perf=empty')
    renderApp(<App />, '/performance')
    await screen.findByText(/Todavía no hay análisis que medir/)
    expect(screen.getByRole('heading', { name: 'Rendimiento' })).toBeInTheDocument()
    // Lista de seguimiento: próxima pasada y ningún intento todavía.
    expect(await screen.findByText(/Cada día de bolsa tras el cierre de EE\. UU\. · preferida 23:00/)).toBeInTheDocument()
    expect(screen.getByText(/Todavía no se ha hecho ninguna pasada/)).toBeInTheDocument()
    // Tarjetas con ceros, tabla y gráfico con «sin datos», pesos por defecto.
    expect(await screen.findByLabelText('Horizonte de 4 semanas')).toHaveTextContent('Sin análisis todavía')
    expect(screen.getByText(/Sin datos suficientes: ningún análisis ha madurado/)).toBeInTheDocument()
    expect(screen.getByText(/aún no hay análisis resueltos a 4 semanas/)).toBeInTheDocument()
    expect(screen.getByText(/Se usan los pesos por defecto/)).toBeInTheDocument()
    expect(screen.queryByRole('table', { name: /Acierto por veredicto/ })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Volver a/ })).not.toBeInTheDocument()
    expect(screen.getAllByText(/uno por activo|Sin datos/).length).toBeGreaterThan(0)
  })

  it('con historial muestra la tabla con intervalos, la tasa base, la fiabilidad y los avisos', async () => {
    // A 12 semanas, un historial aún sin madurar.
    setApi({
      ...mockApi,
      performance: async () => {
        const r = await mockApi.performance()
        r.horizons = r.horizons.map((h) => (h.horizonWeeks === 12 ? { ...h, resolved: 0, resolvedIndependent: 0, byVerdict: [], notMatured: h.analysesTotal } : h))
        return r
      },
    })
    renderApp(<App />, '/performance')
    const table = await screen.findByRole('table', { name: 'Acierto por veredicto a 4 semanas' })
    expect(within(table).getByRole('row', { name: /Acumular/ })).toBeInTheDocument()
    expect(within(table).getByRole('row', { name: /Todos/ })).toBeInTheDocument()
    expect(screen.getByText(/Tasa base/)).toBeInTheDocument()
    expect(screen.getByRole('img', { name: /Gráfico de fiabilidad con 5 tramos/ })).toBeInTheDocument()
    expect(screen.getByText(/Cómo leer estas cifras/)).toBeInTheDocument()
    expect(screen.getByText(/Todos los activos comparten el mismo régimen de mercado/)).toBeInTheDocument()
    // Sin nada madurado: mensaje honesto en vez de porcentajes.
    await userEvent.click(screen.getByRole('button', { name: '12 semanas' }))
    expect(await screen.findByText(/Sin datos suficientes: ningún análisis ha madurado/)).toBeInTheDocument()
    expect(screen.queryByRole('table', { name: /Acierto por veredicto/ })).not.toBeInTheDocument()
  })

  it('volver a una versión de pesos pide confirmación y crea la versión nueva', async () => {
    const spy = vi.spyOn(mockApi, 'rollbackWeights')
    renderApp(<App />, '/performance')
    const btn = await screen.findByRole('button', { name: 'Volver a la versión 1' })
    await userEvent.click(btn)
    expect(spy).not.toHaveBeenCalled() // aún no: falta confirmar
    expect(screen.getByText('¿Activar de nuevo la v1?')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'No' }))
    expect(spy).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Volver a la versión 1' }))
    await userEvent.click(screen.getByRole('button', { name: 'Sí, volver' }))
    await waitFor(() => expect(spy).toHaveBeenCalledWith(1))
    await screen.findByText('Versión 3 activa')
    spy.mockRestore()
  })

  it('un error del servidor al volver atrás se ve', async () => {
    setApi({ ...mockApi, rollbackWeights: async () => { throw new Error('la versión de pesos 1 no existe') } })
    renderApp(<App />, '/performance')
    await userEvent.click(await screen.findByRole('button', { name: 'Volver a la versión 1' }))
    await userEvent.click(screen.getByRole('button', { name: 'Sí, volver' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('la versión de pesos 1 no existe')
  })

  it('la lista de seguimiento enseña la última pasada y los fallos por activo', async () => {
    renderApp(<App />, '/performance')
    await screen.findByText(/Última pasada/)
    expect(screen.getByText(/1 con fallo/)).toBeInTheDocument()
    expect(screen.getByText(/recuperada|Estado por activo/)).toBeInTheDocument()
    await userEvent.click(screen.getByText(/Estado por activo/))
    const row = await screen.findByRole('row', { name: /SOL/ })
    expect(row).toHaveTextContent('Falló')
    expect(row).toHaveTextContent('Binance no responde')
  })

  it('una pasada perdida tiene su texto y no se cuenta como fallos', async () => {
    setApi({
      ...mockApi,
      watchlist: async () => {
        const w = await mockApi.watchlist()
        return { ...w, lastRun: w.lastRun && { ...w.lastRun, status: 'missed', ok: 0, failed: 20 } }
      },
    })
    renderApp(<App />, '/performance')
    expect(await screen.findByText(/perdida: llegó la apertura sin completar la pasada \(equipo apagado o Ollama sin responder\)/)).toBeInTheDocument()
    expect(screen.queryByText(/con fallo/)).not.toBeInTheDocument()
    expect(screen.queryByText(/0 correctos/)).not.toBeInTheDocument()
  })

  it('una pasada abortada avisa de que también puede cerrarse la ventana', async () => {
    setApi({
      ...mockApi,
      watchlist: async () => {
        const w = await mockApi.watchlist()
        return { ...w, lastRun: w.lastRun && { ...w.lastRun, status: 'aborted' } }
      },
    })
    renderApp(<App />, '/performance')
    expect(await screen.findByText(/abortada \(Ollama o el mercado no respondían, o se cerró la ventana\)/)).toBeInTheDocument()
  })

  it('un activo cuya ventana se perdió dice «Ventana perdida» con el motivo, no «No intentado»', async () => {
    const error = 'llegó la apertura sin completar la pasada'
    setApi({
      ...mockApi,
      watchlist: async () => {
        const w = await mockApi.watchlist()
        return { ...w, assets: w.assets.map((a) => (a.last && a.id === 'crypto:SOL' ? { ...a, last: { ...a.last, status: 'skipped', errorCode: 'missed', error, transient: false, attempts: 0 } } : a)) }
      },
    })
    renderApp(<App />, '/performance')
    await userEvent.click(await screen.findByText(/Estado por activo/))
    const row = await screen.findByRole('row', { name: /SOL/ })
    expect(row).toHaveTextContent('Ventana perdida')
    expect(row).toHaveTextContent(error)
    expect(row).not.toHaveTextContent('No intentado')
    expect(row).not.toHaveTextContent('se abortó')
  })

  it('un activo no intentado por una pasada abortada sigue diciendo «No intentado»', async () => {
    setApi({
      ...mockApi,
      watchlist: async () => {
        const w = await mockApi.watchlist()
        return { ...w, assets: w.assets.map((a) => (a.last && a.id === 'crypto:SOL' ? { ...a, last: { ...a.last, status: 'skipped', errorCode: null, error: null, transient: false, attempts: 0 } } : a)) }
      },
    })
    renderApp(<App />, '/performance')
    await userEvent.click(await screen.findByText(/Estado por activo/))
    const row = await screen.findByRole('row', { name: /SOL/ })
    expect(row).toHaveTextContent('No intentado')
    expect(row).toHaveTextContent('se abortó o se cerró la ventana')
  })

  it('la lista desactivada se avisa', async () => {
    setApi({ ...mockApi, watchlist: async () => ({ ...(await mockApi.watchlist()), enabled: false }) })
    renderApp(<App />, '/performance')
    expect(await screen.findByText(/lista de seguimiento está desactivada/)).toBeInTheDocument()
  })

  it('aparece en la navegación', async () => {
    renderApp(<App />, '/performance')
    const nav = await screen.findByRole('navigation', { name: 'Principal' })
    expect(within(nav).getByRole('link', { name: /Rendimiento/ })).toHaveAttribute('href', '/performance')
  })
})

const analysis = { id: '1', asset: 'stock:AAPL', verdict: 'accumulate', confidence: 0.74, horizon: 'medium', scores: { overall: 69, technical: 72, momentum: 80, fundamental: null, valuation: 60, sentiment: 50, macroScore: 55, risk: 36 }, summary: 's', thesis: 't', bullCase: [], bearCase: [], risks: [], catalysts: [], keyLevels: { supports: [], resistances: [], entryZone: null, stopLoss: null, targets: [] }, strategy: null, dataWarnings: [], sources: [], priceAtAnalysis: 1, model: 'm', generatedAt: new Date().toISOString() } as Analysis

describe('confianza calibrada en el informe', () => {
  it('sale rotulada junto a la declarada cuando hay calibración aplicable', async () => {
    setApi({ ...mockApi, calibratedConfidence: async (raw) => ({ raw, calibrated: 0.58, horizonWeeks: 4, n: 412, minRequired: 200, reason: 'ok' }) })
    renderApp(<AnalysisReport analysis={analysis} />)
    expect(await screen.findByText(/Calibrada con el historial/)).toBeInTheDocument()
    expect(screen.getByText(/Confianza declarada/)).toBeInTheDocument()
    expect(screen.getByText(/n=412/)).toBeInTheDocument()
  })

  it('no sale nada sin calibración', async () => {
    const spy = vi.fn(async (raw: number) => ({ raw, calibrated: null, horizonWeeks: 4, n: 12, minRequired: 200, reason: 'poca muestra' }))
    setApi({ ...mockApi, calibratedConfidence: spy })
    renderApp(<AnalysisReport analysis={analysis} />)
    await waitFor(() => expect(spy).toHaveBeenCalled())
    await new Promise((r) => setTimeout(r, 30))
    expect(screen.queryByText(/Calibrada/)).not.toBeInTheDocument()
  })

  it('se calibra con el modelo que hizo el análisis, no con el activo', async () => {
    const spy = vi.fn(async (raw: number, _h?: number, _model?: string) => ({ raw, calibrated: 0.58, horizonWeeks: 4, n: 412, minRequired: 200, reason: 'ok' }))
    setApi({ ...mockApi, calibratedConfidence: spy })
    renderApp(<AnalysisReport analysis={{ ...analysis, model: 'qwen3:8b' }} />)
    await waitFor(() => expect(spy).toHaveBeenCalled())
    expect(spy.mock.calls[0]?.slice(0, 3)).toEqual([0.74, 4, 'qwen3:8b'])
  })

  it('un fallo al pedirla no rompe el informe', async () => {
    setApi({ ...mockApi, calibratedConfidence: async () => { throw new Error('caído') } })
    renderApp(<AnalysisReport analysis={analysis} />)
    expect(await screen.findByText(/Confianza declarada/)).toBeInTheDocument()
    expect(screen.queryByText(/Calibrada/)).not.toBeInTheDocument()
  })
})
