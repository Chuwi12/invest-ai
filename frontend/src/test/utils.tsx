import { QueryClientProvider } from '@tanstack/react-query'
import { render } from '@testing-library/react'
import type { ReactElement } from 'react'
import { MemoryRouter } from 'react-router'
import { createQueryClient } from '@/api/queryClient'

export function renderApp(ui: ReactElement, route = '/') {
  const qc = createQueryClient()
  qc.setDefaultOptions({ queries: { retry: false, staleTime: 30_000 } })
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[route]}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  )
}
