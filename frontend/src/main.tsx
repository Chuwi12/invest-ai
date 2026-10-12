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

async function boot() {
  initTheme()
  // La condición usa `import.meta.env` directamente para que el build la resuelva a `false`
  // y el mock no entre en el bundle de producción.
  if (import.meta.env.VITE_MOCK === '1' || import.meta.env.VITE_MOCK === 'true') {
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
