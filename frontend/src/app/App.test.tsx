import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { setApi } from '@/api'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'
import { App } from './App'

beforeAll(async () => {
  setApi(mockApi)
  // Los chunks perezosos de las rutas se transforman y evalúan la primera vez que se importan, lo
  // que con la CPU saturada tarda segundos y caía dentro de la espera de un test: se precargan aquí.
  await Promise.all([import('@/pages/Dashboard'), import('@/pages/ModelsPage'), import('@/app/SearchPalette')])
})

// Simula una máquina cargada: el chunk perezoso de Favoritos tarda 1,5 s en llegar (más que el
// timeout por defecto de `findBy*`, 1 s). Solo afecta a esa ruta.
vi.mock('@/pages/FavoritesPage', async (importOriginal) => {
  await new Promise((resolve) => setTimeout(resolve, 1500))
  return await importOriginal()
})

describe('Landing', () => {
  it('«/» muestra la landing a página completa, fuera del Shell, y «Entrar al panel» lleva al panel', async () => {
    renderApp(<App />, '/')
    expect(await screen.findByRole('heading', { level: 1, name: 'invest-ai' })).toBeInTheDocument()
    expect(screen.queryByRole('navigation', { name: 'Principal' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Panel' })).toBeNull()
    await userEvent.click(screen.getAllByRole('link', { name: 'Entrar al panel' })[0]!)
    await screen.findByRole('heading', { name: 'Panel' })
    expect(screen.queryByRole('heading', { level: 1, name: 'invest-ai' })).toBeNull()
  })
})

describe('Shell', () => {
  it('espera por condición, no por reloj: una página perezosa lenta (1,5 s) acaba pintándose', async () => {
    renderApp(<App />, '/favorites')
    await screen.findByRole('heading', { name: 'Favoritos' })
  })

  it('muestra el aviso permanente y no el banner de datos simulados fuera del modo mock', async () => {
    renderApp(<App />, '/dashboard')
    expect(screen.getAllByText(/No constituye asesoramiento financiero/).length).toBeGreaterThan(0)
    expect(screen.queryByText(/Modo demostración/)).toBeNull()
    await screen.findByRole('heading', { name: 'Panel' })
  })

  it('«/» abre el buscador, agrupa por clase y navega al elegir', async () => {
    renderApp(<App />, '/dashboard')
    await screen.findByRole('heading', { name: 'Panel' })
    await userEvent.keyboard('/')
    const input = await screen.findByRole('combobox')
    await userEvent.type(input, 'apple')
    const group = await screen.findByRole('group', { name: 'Acciones' })
    expect(within(group).getByText('AAPL')).toBeInTheDocument()
    await userEvent.keyboard('{Enter}')
    await waitFor(() => expect(screen.queryByRole('combobox')).toBeNull())
  })

  it('Ajustes → Modelos lista el catálogo por nivel y activa un modelo instalado', async () => {
    renderApp(<App />, '/settings/models')
    await screen.findByRole('heading', { name: 'Ligeros' })
    expect(screen.getByText('Hardware detectado')).toBeInTheDocument()
    const card = screen.getByRole('article', { name: 'Llama 3.1 8B' })
    await userEvent.click(within(card).getByRole('button', { name: 'Activar' }))
    await waitFor(() => expect(within(screen.getByRole('article', { name: 'Llama 3.1 8B' })).getByText('Activo')).toBeInTheDocument())
  })
})
