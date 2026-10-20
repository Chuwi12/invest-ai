import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import { App } from '@/app/App'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'

describe('ModelsPage: GPU', () => {
  beforeAll(async () => {
    await import('@/pages/ModelsPage')
  })
  beforeEach(() => setApi(mockApi))

  it('muestra el % real en GPU del modelo cargado y el ajuste de forzar GPU, apagado por defecto', async () => {
    renderApp(<App />, '/settings/models')
    const loaded = await screen.findByRole('list', { name: 'Modelos cargados' })
    expect(within(loaded).getByText(/89 % en GPU/)).toBeInTheDocument()
    expect(within(loaded).getByText(/Forzar todas las capas en GPU/)).toBeInTheDocument()
    const card = screen.getByRole('article', { name: 'Qwen 2.5 7B' })
    expect(within(card).getByRole('checkbox', { name: /Forzar todas las capas en GPU/ })).not.toBeChecked()
    // Un modelo sin instalar no ofrece el ajuste.
    expect(within(screen.getByRole('article', { name: 'Qwen 3 8B' })).queryByRole('checkbox')).toBeNull()
  })

  it('al forzar la GPU del modelo cargado se guarda y el % pasa a 100', async () => {
    renderApp(<App />, '/settings/models')
    const card = await screen.findByRole('article', { name: 'Qwen 2.5 7B' })
    await userEvent.click(within(card).getByRole('checkbox', { name: /Forzar todas las capas en GPU/ }))
    await waitFor(() => expect(within(screen.getByRole('list', { name: 'Modelos cargados' })).getByText(/100 % en GPU/)).toBeInTheDocument())
    expect(within(screen.getByRole('article', { name: 'Qwen 2.5 7B' })).getByRole('checkbox', { name: /Forzar todas las capas en GPU/ })).toBeChecked()
  })
})
