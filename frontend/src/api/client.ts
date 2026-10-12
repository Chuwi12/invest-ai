import type { ApiErrorBody } from './types'

export const API_BASE = '/api'

/** Error del contrato (`{ error: { code, message } }`) o de red (`code: "network"`). */
export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

export function isAbortError(e: unknown): boolean {
  return e instanceof DOMException ? e.name === 'AbortError' : e instanceof Error && e.name === 'AbortError'
}

type QueryValue = string | number | boolean | null | undefined

/** Construye `?a=1&b=2` omitiendo los valores nulos o indefinidos. */
export function buildQuery(params?: Record<string, QueryValue>): string {
  if (!params) return ''
  const sp = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') continue
    sp.set(k, String(v))
  }
  const s = sp.toString()
  return s ? `?${s}` : ''
}

/** Ruta de un activo: el id va percent-encoded (`stock%3AAAPL`). */
export function assetPath(id: string): string {
  return `/assets/${encodeURIComponent(id)}`
}

/** Convierte una respuesta no-2xx en `ApiError` leyendo el JSON del contrato si existe. */
export async function toApiError(res: Response): Promise<ApiError> {
  let code = `http_${res.status}`
  let message = res.statusText || `Error HTTP ${res.status}`
  try {
    const text = await res.text()
    if (text) {
      try {
        const body = JSON.parse(text) as Partial<ApiErrorBody>
        if (body.error && typeof body.error.message === 'string') {
          code = typeof body.error.code === 'string' ? body.error.code : code
          message = body.error.message
        } else {
          message = text.slice(0, 300)
        }
      } catch {
        message = text.slice(0, 300)
      }
    }
  } catch {
    /* cuerpo ilegible: nos quedamos con el estado HTTP */
  }
  return new ApiError(res.status, code, message)
}

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  query?: Record<string, QueryValue>
  body?: unknown
  signal?: AbortSignal | undefined
}

/** `fetch` tipado: JSON dentro, JSON fuera, 204 => `undefined`. */
export async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const { method = 'GET', query, body, signal } = opts
  const init: RequestInit = { method, headers: { Accept: 'application/json' } }
  if (signal) init.signal = signal
  if (body !== undefined) {
    init.body = JSON.stringify(body)
    ;(init.headers as Record<string, string>)['Content-Type'] = 'application/json'
  }

  let res: Response
  try {
    res = await fetch(`${API_BASE}${path}${buildQuery(query)}`, init)
  } catch (e) {
    if (isAbortError(e)) throw e
    throw new ApiError(0, 'network', 'No se pudo conectar con el servidor')
  }
  if (!res.ok) throw await toApiError(res)
  if (res.status === 204) return undefined as T
  const text = await res.text()
  if (!text) return undefined as T
  return JSON.parse(text) as T
}
