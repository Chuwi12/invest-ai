import { QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router'
import '@fontsource-variable/geist/wght.css'
import '@fontsource-variable/newsreader/standard.css'
import './styles/app.css'
import { createQueryClient } from '@/api/queryClient'
import { App } from '@/app/App'
import { initTheme } from '@/app/theme'
import { IS_MOCK } from '@/lib/env'

async function boot() {
  initTheme()
  // El mock vive en un import dinámico bajo una constante que el build resuelve a `false`:
  // en producción ni siquiera entra en el bundle.
  if (IS_MOCK) {
    const { installMock } = await import('@/mock')
    installMock()
  }
  const root = document.getElementById('root')
  if (!root) throw new Error('Falta #root')
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={createQueryClient()}>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </QueryClientProvider>
    </StrictMode>,
  )
}

void boot()
