import { act, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setApi } from '@/api'
import type { Api } from '@/api'
import { ApiError } from '@/api/client'
import type { Analysis, AssetId, HorizonReport, Quote, WatchlistStatus } from '@/api/types'
import { setThemePref } from '@/app/theme'
import { mockApi } from '@/mock'
import { RealtimeClient, setRealtime, type SocketLike } from '@/realtime'
import { renderApp } from '@/test/utils'
import LandingPage from './LandingPage'

const DAY = 86_400_000
const ago = (ms: number) => new Date(Date.now() - ms).toISOString()
const NOTE = 'Se descartó una afirmación porque citaba cifras que no aparecen en los datos (70).'

const ANALYSIS: Analysis = {
  id: 'an-1',
  asset: 'stock:AAPL',
  verdict: 'accumulate',
  confidence: 0.65,
  horizon: 'medium',
  scores: { overall: 62.2, technical: 71, momentum: 68, fundamental: 77, valuation: 34, sentiment: 52, macroScore: 55, risk: 27 },
  summary: 'Acumular por tramos: fortaleza técnica y valoración exigente.',
  thesis: 'Tesis.',
  bullCase: [],
  bearCase: [],
  risks: [],
  catalysts: [],
  keyLevels: { supports: [], resistances: [], entryZone: null, stopLoss: null, targets: [] },
  strategy: null,
  dataWarnings: [],
  sources: [
    // Un titular que EMPIEZA como la cotización pero es noticia (tiene URL): el precio no puede apuntarle.
    { title: 'Cotización del bitcoin hoy: qué mirar', url: 'https://example.com/btc', provider: 'Yahoo', publishedAt: ago(5 * 3_600_000) },
    { title: 'Cotización stock:AAPL', url: null, provider: 'yahoo', publishedAt: ago(3 * DAY) },
    { title: 'Puntuación cuantitativa determinista stock:AAPL', url: null, provider: 'invest-ai', publishedAt: ago(3_600_000) },
    { title: 'Titular de prueba', url: 'https://example.com/n', provider: 'Yahoo', publishedAt: ago(2 * 3_600_000) },
  ],
  priceAtAnalysis: 347.5,
  model: 'qwen3.5:9b',
  generatedAt: ago(DAY),
  qualityNotes: [NOTE],
}

const IDS: AssetId[] = ['stock:AAPL', 'stock:MSFT', 'stock:NVDA']

function watchlist(lastOk: (string | null)[] = [ago(DAY), ago(3 * DAY), null]): WatchlistStatus {
  const base = {
    enabled: true, defaultList: true, time: '09:15', depth: 'standard' as const, nextRun: null, running: false,
    lastRunResults: [], recentRuns: [], analysesTotal: 27,
  }
  return {
    ...base,
    assets: IDS.map((id, i) => ({ id, last: null, lastOkAt: lastOk[i] ?? null })),
    lastRun: {
      id: 4, localDate: '2026-10-23', scheduledFor: ago(3 * DAY), startedAt: ago(DAY), finishedAt: ago(DAY), trigger: 'schedule',
      status: 'missed', depth: 'standard', model: null, planned: 3, power: 'unknown', powerWaitSecs: 0, ok: 0, failed: 3,
    },
  }
}

function quote(id: AssetId, price: number, prevClose: number): Quote {
  return {
    id, price, change: price - prevClose, changePct: ((price / prevClose) - 1) * 100, open: null, high: null, low: null, prevClose,
    volume: null, marketCap: null, currency: 'USD', marketState: 'regular', timestamp: ago(60_000), source: 'yahoo', delaySecs: 0,
  }
}

const QUOTES = [quote('stock:AAPL', 101, 100), quote('stock:MSFT', 49.5, 50), quote('stock:NVDA', 10, 10)]

class FakeSocket implements SocketLike {
  readyState = 0
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: unknown) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null
  send() {}
  close() {}
}

let socket: FakeSocket

/** API con datos de una instancia real simulados; `over` cambia lo que cada test necesita. */
function install(over: Partial<Api> = {}) {
  setApi({
    ...mockApi,
    watchlist: async () => watchlist(),
    quotes: async () => QUOTES,
    latestAnalysis: async (id) => (id === 'stock:AAPL' ? ANALYSIS : null),
    models: async () => ({ ...(await mockApi.models()), active: 'qwen3.5:9b', ollamaReachable: true, loaded: [] }),
    performance: async () => {
      const r = await mockApi.performance()
      return {
        ...r,
        analysesTotal: 27,
        horizons: r.horizons.map((h) => ({ ...h, analysesTotal: 27, resolved: 0, resolvedIndependent: 0, pending: 0, notMatured: 27, byVerdict: [] })),
      }
    },
    ...over,
  })
}

beforeEach(() => {
  setThemePref('system')
  socket = new FakeSocket()
  setRealtime(new RealtimeClient({ url: 'ws://test/api/ws', createSocket: () => socket }))
  install()
})
afterEach(() => setApi(mockApi))

const renderLanding = () => renderApp(<LandingPage />, '/')
const figure = (label: string) => within(screen.getByLabelText('Cifras del análisis')).getByText(label).closest('div') as HTMLElement

describe('LandingPage', () => {
  it('con un análisis real: veredicto, cuatro cifras con su referencia e índice de fuentes', async () => {
    renderLanding()
    expect(await screen.findByRole('heading', { level: 2, name: /AAPL\s*Acumular/ })).toBeInTheDocument()
    expect(screen.getByText(/fortaleza técnica y valoración exigente/)).toBeInTheDocument()

    // Cuatro cifras, solo de campos estructurados, cada una con su referencia o su origen.
    // El precio apunta a la cotización (fuente 2), no al titular que empieza igual (fuente 1).
    const price = figure('Precio en el análisis')
    expect(price).toHaveTextContent(/347,50/)
    expect(within(price).getByRole('link', { name: /Fuente 2: Cotización stock:AAPL/ })).toHaveAttribute('href', '#fuente-2')
    expect(price).toHaveTextContent('Yahoo Finance')
    const score = figure('Puntuación global')
    expect(score).toHaveTextContent('62')
    expect(within(score).getByRole('link', { name: /Fuente 3/ })).toBeInTheDocument()
    expect(score).toHaveTextContent('calculado por invest-ai')
    // La confianza mezcla la del modelo y la de la puntuación: no cuelga de ninguna fuente.
    const conf = figure('Confianza')
    expect(conf).toHaveTextContent(/65\s%/)
    expect(conf).toHaveTextContent('la del modelo, calibrada por invest-ai')
    expect(within(conf).queryByRole('link')).toBeNull()
    const quality = figure('Notas de calidad')
    expect(quality).toHaveTextContent('1')
    expect(quality).toHaveTextContent('calculado por invest-ai')

    // Índice numerado de fuentes, con antigüedad; la vieja (3 días) se atenúa y la reciente no.
    const index = screen.getByRole('region', { name: 'Fuentes de este análisis' })
    const rows = within(index).getAllByRole('listitem')
    expect(rows).toHaveLength(4)
    expect(rows[1]).toHaveTextContent(/^2\s*Cotización\s*Yahoo Finance\s*hace 3 días$/)
    expect(rows[1]).toHaveAttribute('data-old', 'true')
    expect(rows[2]).toHaveAttribute('data-old', 'false')
    expect(within(rows[3]!).getByRole('link', { name: 'Titular de prueba' })).toHaveAttribute('rel', 'noopener noreferrer')
  })

  it('el pie de la cifra de precio no apunta a ninguna fuente si no hay precio', async () => {
    install({ latestAnalysis: async () => ({ ...ANALYSIS, priceAtAnalysis: null }) })
    renderLanding()
    await screen.findByRole('heading', { level: 2, name: /AAPL/ })
    const price = figure('Precio en el análisis')
    expect(price).toHaveTextContent('—')
    expect(within(price).queryByRole('link')).toBeNull()
    expect(price).toHaveTextContent('sin precio en este análisis')
  })

  it('el titular dice qué es y que es local, y el pie de la cifra dice «TSLA, acción», no una orden', async () => {
    renderLanding()
    expect(await screen.findByText(/el modelo y tus datos no salen de este equipo/)).toBeInTheDocument()
    expect(await screen.findByText(/^AAPL, acción\. Guardado/)).toBeInTheDocument()
  })

  it('al señalar o enfocar una cifra se resalta su fuente, y al revés', async () => {
    renderLanding()
    await screen.findByRole('heading', { level: 2, name: /AAPL/ })
    const rows = within(screen.getByRole('region', { name: 'Fuentes de este análisis' })).getAllByRole('listitem')
    const price = figure('Precio en el análisis')
    const score = figure('Puntuación global')

    await userEvent.hover(price)
    expect(rows[1]).toHaveAttribute('data-active', 'true')
    expect(rows[0]).toHaveAttribute('data-active', 'false')
    await userEvent.unhover(price)
    expect(rows[1]).toHaveAttribute('data-active', 'false')

    // Con el teclado: enfocar la referencia de la puntuación.
    act(() => within(score).getByRole('link', { name: /Fuente 3/ }).focus())
    expect(rows[2]).toHaveAttribute('data-active', 'true')

    // Y al revés: señalar la fuente resalta la cifra que cuelga de ella.
    await userEvent.hover(rows[2]!)
    expect(score).toHaveAttribute('data-active', 'true')
    expect(price).toHaveAttribute('data-active', 'false')
  })

  it('sin análisis guardado: estado vacío honesto que invita a entrar y generar uno', async () => {
    install({ watchlist: async () => watchlist([null, null, null]) })
    renderLanding()
    // Habla de la LISTA: el historial sí tiene 27 análisis guardados, y la landing no se contradice.
    expect(await screen.findByRole('heading', { level: 2, name: 'La lista de seguimiento aún no ha guardado ningún análisis' })).toBeInTheDocument()
    expect(screen.queryByText(/Aún no hay ningún análisis guardado/)).toBeNull()
    expect(await screen.findByText(/27 análisis guardados/)).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Último análisis de la lista' })).toBeInTheDocument()
    expect(screen.queryByLabelText('Cifras del análisis')).toBeNull()
    expect(screen.getByText('Sin análisis no hay fuentes que citar.')).toBeInTheDocument()
    expect(screen.getAllByRole('link', { name: 'Entrar al panel' })[0]).toHaveAttribute('href', '/dashboard')
  })

  it('con la lista al día pero sin análisis para ese activo, también es el estado vacío', async () => {
    install({ latestAnalysis: async () => null })
    renderLanding()
    expect(await screen.findByRole('heading', { level: 2, name: 'La lista de seguimiento aún no ha guardado ningún análisis' })).toBeInTheDocument()
  })

  it('la cinta lista los activos con su variación, avanza con ticks en vivo y enlaza a su ficha', async () => {
    renderLanding()
    const list = await screen.findByRole('list', { name: 'Activos de la lista de seguimiento' })
    const links = within(list).getAllByRole('link')
    expect(links.map((l) => l.getAttribute('href'))).toEqual(IDS.map((id) => `/asset/${encodeURIComponent(id)}`))
    expect(await within(list).findByRole('link', { name: /AAPL/ })).toHaveTextContent(/101,00.*\+1,00\s%/)
    expect(within(list).getByRole('link', { name: /MSFT/ })).toHaveTextContent(/-1,00\s%/)

    // Un tick del WebSocket cambia el precio sin recargar nada.
    await act(async () => {
      socket.readyState = 1
      socket.onopen?.({})
      socket.onmessage?.({ data: JSON.stringify({ type: 'tick', tick: { id: 'stock:AAPL', price: 321.5, changePct: 2, volume: null, ts: new Date().toISOString(), source: 'test' } }) })
    })
    expect(within(list).getByRole('link', { name: /AAPL/ })).toHaveTextContent(/321,50/)
  })

  it('el botón de pausa alterna aria-pressed; la mitad duplicada del bucle queda fuera del foco', async () => {
    renderLanding()
    await screen.findByRole('list', { name: 'Activos de la lista de seguimiento' })
    const pause = screen.getByRole('button', { name: 'Pausar cinta' })
    expect(pause).toHaveAttribute('aria-pressed', 'false')
    await userEvent.click(pause)
    expect(pause).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(pause)
    expect(pause).toHaveAttribute('aria-pressed', 'false')
    // 14 celdas por mitad como mínimo: con 3 activos se repiten, y la segunda mitad es inert y aria-hidden.
    expect(document.querySelectorAll('.lp-dup[aria-hidden="true"]')).toHaveLength(1)
    // Las copias se pueden pulsar con el ratón (sin inert), pero no reciben el foco del teclado.
    expect(document.querySelectorAll('.lp-dup a:not([tabindex="-1"]), .lp-rep a:not([tabindex="-1"])')).toHaveLength(0)
  })

  it('el conmutador de la cabecera cicla sistema, claro y oscuro, aplica data-theme y recuerda la preferencia', async () => {
    renderLanding()
    await screen.findByRole('heading', { level: 2, name: /AAPL/ })
    const toggle = within(screen.getByRole('banner')).getByRole('button', { name: 'Tema: automático' })
    const root = document.documentElement
    expect(root).not.toHaveAttribute('data-theme')

    await userEvent.click(toggle)
    expect(toggle).toHaveAccessibleName('Tema: claro')
    expect(root).toHaveAttribute('data-theme', 'light')
    expect(localStorage.getItem('invest-ai.theme')).toBe('light')

    await userEvent.click(toggle)
    expect(toggle).toHaveAccessibleName('Tema: oscuro')
    expect(root).toHaveAttribute('data-theme', 'dark')
    expect(localStorage.getItem('invest-ai.theme')).toBe('dark')

    // De vuelta al sistema: sin atributo y sin preferencia guardada.
    await userEvent.click(toggle)
    expect(toggle).toHaveAccessibleName('Tema: automático')
    expect(root).not.toHaveAttribute('data-theme')
    expect(localStorage.getItem('invest-ai.theme')).toBeNull()
  })

  it('«Entrar al panel» enlaza a /dashboard arriba y al cierre', async () => {
    renderLanding()
    await screen.findByRole('heading', { level: 2, name: /AAPL/ })
    const ctas = screen.getAllByRole('link', { name: 'Entrar al panel' })
    expect(ctas).toHaveLength(2)
    for (const c of ctas) expect(c).toHaveAttribute('href', '/dashboard')
    expect(screen.getByText('Análisis automatizado con un modelo de IA local: no es asesoramiento financiero.')).toBeInTheDocument()
  })

  it('si fallan las cotizaciones, la cinta muestra los activos sin precio y avisa: ningún número inventado', async () => {
    install({ quotes: async () => Promise.reject(new ApiError(0, 'network', 'sin red')) })
    renderLanding()
    expect(await screen.findByText('No se pudieron cargar las cotizaciones.')).toBeInTheDocument()
    const list = screen.getByRole('list', { name: 'Activos de la lista de seguimiento' })
    const links = within(list).getAllByRole('link')
    expect(links).toHaveLength(3)
    for (const l of links) expect(l).toHaveTextContent('sin precio')
    expect(list.textContent).not.toMatch(/\d/)
    expect(screen.getByRole('button', { name: 'Reintentar' })).toBeInTheDocument()
  })

  it('la cabecera dice qué modelo hay y si está cargado, y cómo fue la última pasada', async () => {
    renderLanding()
    expect(await screen.findByText('qwen3.5:9b, sin cargar ahora')).toBeInTheDocument()
    expect(screen.getByText(/perdida, no se analizó a tiempo/)).toBeInTheDocument()
  })

  it('bajo el pliegue: las notas del verificador tal cual y un historial vacío con lo que hay', async () => {
    renderLanding()
    expect(await screen.findByText(NOTE)).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'El verificador descarta o marca lo que no cuadra' })).toBeInTheDocument()
    expect(screen.getByText(/descarta esa afirmación o la marca con ⚠/)).toBeInTheDocument()
    expect(screen.getByText(/Notas de calidad del último análisis de la lista, de AAPL, tal cual/)).toBeInTheDocument()
    expect(await screen.findByText('Todavía no hay resultados que medir: 27 análisis guardados y ninguno ha cumplido su plazo.')).toBeInTheDocument()
    const rows = within(screen.getByRole('table', { name: 'Análisis medidos por plazo' })).getAllByRole('row')
    expect(rows).toHaveLength(4)
    expect(rows[1]).toHaveTextContent(/1 semana.*27 esperando su plazo/)
    expect(screen.getByText(/no mostró ventaja frente al mercado/)).toBeInTheDocument()
  })

  it('si ya han vencido, el historial dice «pendientes de medir» y no «ninguno ha cumplido su plazo»', async () => {
    install({ performance: async () => withHorizons((h) => ({ ...h, pending: 5, notMatured: 22 })) })
    renderLanding()
    expect(await screen.findByText(/27 análisis guardados, con 5 ya vencidos y pendientes de medir/)).toBeInTheDocument()
    expect(screen.queryByText(/ninguno ha cumplido su plazo/)).toBeNull()
    const rows = within(screen.getByRole('table', { name: 'Análisis medidos por plazo' })).getAllByRole('row')
    expect(rows[1]).toHaveTextContent(/5 pendientes de medir, 22 esperando su plazo/)
  })

  it('el porcentaje de acierto va siempre con la tasa base; sin ella no se da', async () => {
    const matured = (base: number | null) => (h: HorizonReport): HorizonReport =>
      h.horizonWeeks === 4
        ? { ...h, resolved: 60, resolvedIndependent: 40, pending: 0, notMatured: 0, baseRateOutperform: base, overall: { verdict: 'all', n: 60, hits: 30, hitRate: 0.5, hitRateCi: [0.4, 0.6], meanExcess: 0, meanExcessCi: null, clusters: 8 } }
        : h
    install({ performance: async () => withHorizons(matured(0.49)) })
    const { unmount } = renderLanding()
    const row = async () => (await within(await screen.findByRole('table', { name: 'Análisis medidos por plazo' })).findAllByRole('row'))[2]!
    expect(await row()).toHaveTextContent(/acierto 50\s%.*no se distingue de la tasa base \(49\s%\)/)
    unmount()
    install({ performance: async () => withHorizons(matured(null)) })
    renderLanding()
    const noBase = await row()
    expect(noBase).toHaveTextContent(/60 medidos: sin tasa base, no se da el porcentaje/)
    expect(noBase.textContent).not.toMatch(/\d\s%/)
  })

  it('tras un refresco fallido las cotizaciones viejas se siguen enseñando, con su hora y un aviso', async () => {
    let calls = 0
    install({ quotes: async () => (++calls > 1 ? Promise.reject(new ApiError(0, 'network', 'sin red')) : QUOTES) })
    renderLanding()
    const list = await screen.findByRole('list', { name: 'Activos de la lista de seguimiento' })
    expect(await within(list).findByRole('link', { name: /AAPL/ })).toHaveTextContent(/101,00/)
    // Pasa el tiempo (los precios quedan viejos) y vuelve la pestaña: el refresco falla.
    const now = Date.now()
    vi.spyOn(Date, 'now').mockReturnValue(now + 120_000)
    await act(async () => {
      window.dispatchEvent(new Event('visibilitychange'))
    })
    expect(await screen.findByText('No se pudieron actualizar las cotizaciones.')).toBeInTheDocument()
    expect(screen.getByText(/Última actualización: /)).toBeInTheDocument()
    expect(within(list).getByRole('link', { name: /AAPL/ })).toHaveTextContent(/101,00/)
    expect(screen.queryByText(/sin precio/)).toBeNull()
    vi.restoreAllMocks()
  })
})

/** El informe de rendimiento del mock con cada plazo retocado: 27 análisis guardados y ninguno medido. */
async function withHorizons(change: (h: HorizonReport) => HorizonReport) {
  const r = await mockApi.performance()
  const base = r.horizons.map((h) => ({ ...h, analysesTotal: 27, resolved: 0, resolvedIndependent: 0, pending: 0, notMatured: 27, byVerdict: [] }))
  return { ...r, analysesTotal: 27, horizons: base.map(change) }
}
