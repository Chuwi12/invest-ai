import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import { App } from '@/app/App'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'

beforeAll(async () => {
  setApi(mockApi)
  await import('@/pages/IpoPage')
})

describe('OPV', () => {
  it('muestra el aviso permanente y el calendario con estado', async () => {
    renderApp(<App />, '/ipo')
    await screen.findByRole('heading', { name: 'OPV de EE. UU.' })
    const note = screen.getByRole('note', { name: 'Aviso sobre el perfil de riesgo' })
    expect(note).toHaveTextContent('Perfil de riesgo, no una recomendación')
    expect(note).toHaveTextContent('no predice cómo irá una en concreto')
    const upcoming = await screen.findByRole('list', { name: 'OPV próximas' })
    expect(within(upcoming).getByText('Cobalt Meridian Energy')).toBeInTheDocument()
    expect(within(upcoming).getAllByText('Prevista').length).toBeGreaterThan(0)
    expect(within(upcoming).getByText('Presentada')).toBeInTheDocument()
    // Fase previa a cotizar: las de precio fijado están en «Recientes».
    await userEvent.click(screen.getByRole('button', { name: /Recientes/ }))
    const recent = await screen.findByRole('list', { name: 'OPV recientes' })
    expect(within(recent).getByText('Orbital Freight Systems')).toBeInTheDocument()
    expect(within(recent).getAllByText('Precio fijado').length).toBeGreaterThan(0)
  })

  it('la barra de navegación incluye OPV', async () => {
    renderApp(<App />, '/ipo')
    await screen.findByRole('heading', { name: 'OPV de EE. UU.' })
    expect(screen.getAllByRole('link', { name: /OPV/ }).length).toBeGreaterThanOrEqual(2) // lateral + móvil
  })

  it('la ficha muestra cada indicador con fuente, evidencia y «no extraído»', async () => {
    renderApp(<App />, '/ipo/LHTX')
    await screen.findByRole('heading', { name: 'Lumen Harbor Therapeutics' })
    const lock = await screen.findByRole('article', { name: 'Lock-up' })
    expect(within(lock).getByText('180 días')).toBeInTheDocument()
    expect(within(lock).getByRole('link', { name: /Folleto 424B4 \(simulado\), sección «Lock-Up Agreements»/ })).toHaveAttribute('rel', 'noopener noreferrer')
    // Destino de los fondos (biotecnológica): categorías sin banderas de atención.
    const uses = screen.getByRole('article', { name: 'Destino de los fondos' })
    expect(within(uses).queryByText(/Atención:/)).toBeNull()
    // Indicador que no se pudo leer: dice «No extraído» y por qué, sin inventar valor.
    const uw = screen.getByRole('article', { name: 'Colocadores principales' })
    expect(within(uw).getByText('No extraído')).toBeInTheDocument()
    expect(within(uw).getByText(/no termina en una fila «Total»/)).toBeInTheDocument()
    const rev = screen.getByRole('article', { name: 'Ingresos' })
    expect(within(rev).getByText('No extraído')).toBeInTheDocument()
    expect(screen.getByText(/Indicadores leídos:/)).toBeInTheDocument()
  })

  it('banderas de atención y texto del folleto en ORFS', async () => {
    renderApp(<App />, '/ipo/ORFS')
    const sec = await screen.findByRole('article', { name: 'Venta de accionistas existentes' })
    expect(within(sec).getByText(/54,5 % de lo ofrecido lo venden accionistas existentes/)).toBeInTheDocument()
    expect(within(sec).getByText('Atención: Venta secundaria: los de dentro venden')).toBeInTheDocument()
    await userEvent.click(within(sec).getByText('Ver texto del folleto'))
    expect(within(sec).getAllByText(/selling stockholders/).length).toBeGreaterThan(0)
    const uses = screen.getByRole('article', { name: 'Destino de los fondos' })
    expect(within(uses).getByText(/Atención: Devolver deuda/)).toBeInTheDocument()
  })

  it('tasa base con tamaños de muestra y advertencia de que no predice', async () => {
    renderApp(<App />, '/ipo/ORFS')
    await screen.findByRole('heading', { name: 'Momento de mercado: tasa base' })
    expect(await screen.findByText(/Mediana día 1 \(n=27\)/)).toBeInTheDocument()
    expect(screen.getByText(/Mediana a 3 meses \(n=22\)/)).toBeInTheDocument()
    expect(screen.getByText(/No predice esta OPV/)).toBeInTheDocument()
  })

  it('rendimiento posterior solo si ya cotiza; pendiente cuando no se alcanzó el horizonte', async () => {
    renderApp(<App />, '/ipo/LHTX')
    const table = await screen.findByRole('table', { name: 'Rentabilidad de LHTX tras la OPV' })
    // LHTX salió hace 12 días: ningún horizonte se ha cumplido.
    expect(within(table).getAllByText('Pendiente').length).toBeGreaterThanOrEqual(4)
  })

  it('una OPV que aún no cotiza no tiene sección de rendimiento y avisa del folleto preliminar', async () => {
    renderApp(<App />, '/ipo/CBME')
    await screen.findByRole('heading', { name: 'Cobalt Meridian Energy' })
    expect(await screen.findByText(/folleto disponible es preliminar/)).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Rendimiento posterior' })).toBeNull()
  })

  it('secciones del folleto plegables', async () => {
    renderApp(<App />, '/ipo/LHTX')
    const summary = await screen.findByText('Destino de los fondos', { selector: 'summary span' })
    const details = summary.closest('details')!
    expect(details.open).toBe(false)
    await userEvent.click(summary)
    await waitFor(() => expect(details.open).toBe(true))
    expect(within(details).getByText(/net proceeds to fund clinical/)).toBeInTheDocument()
  })

  it('sin folleto: todo «no extraído» y aviso, sin cifras inventadas', async () => {
    renderApp(<App />, '/ipo/VLPM')
    await screen.findByRole('heading', { name: 'Verdant Loop Mobility' })
    expect(await screen.findByText(/No se pudo obtener el folleto/)).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Folleto' })).toBeNull()
    expect(screen.getAllByText('No extraído').length).toBeGreaterThanOrEqual(10)
  })

  it('símbolo desconocido: error claro con reintento', async () => {
    renderApp(<App />, '/ipo/ZZZZ')
    expect(await screen.findByText(/No se pudo cargar el perfil de ZZZZ/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Reintentar/ })).toBeInTheDocument()
  })
})
