import { QueryClient } from '@tanstack/react-query'
import { ApiError } from './client'

/** No reintenta errores 4xx (salvo 429): no van a cambiar solos. */
function shouldRetry(count: number, err: unknown): boolean {
  if (err instanceof ApiError && err.status >= 400 && err.status < 500 && err.status !== 429) return false
  return count < 2
}

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: shouldRetry,
        retryDelay: (n) => Math.min(1000 * 2 ** n, 8000),
        staleTime: 30_000,
        gcTime: 10 * 60_000,
        refetchOnWindowFocus: false,
      },
    },
  })
}
