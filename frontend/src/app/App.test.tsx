import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'
import { App } from './App'

beforeAll(() => setApi(mockApi))

describe('Shell', () => {
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
    const group = await screen.findByRole('group', { name: 'Acciones' }, { timeout: 3000 })
    expect(within(group).getByText('AAPL')).toBeInTheDocument()
    await userEvent.keyboard('{Enter}')
    await waitFor(() => expect(screen.queryByRole('combobox')).toBeNull())
  })

  it('Ajustes → Modelos lista el catálogo por nivel y activa un modelo instalado', async () => {
    renderApp(<App />, '/settings/models')
    await screen.findByRole('heading', { name: 'Ligeros' }, { timeout: 3000 })
    expect(screen.getByText('Hardware detectado')).toBeInTheDocument()
    const card = screen.getByRole('article', { name: 'Llama 3.1 8B' })
    await userEvent.click(within(card).getByRole('button', { name: 'Activar' }))
    await waitFor(() => expect(within(screen.getByRole('article', { name: 'Llama 3.1 8B' })).getByText('Activo')).toBeInTheDocument())
  })
})
