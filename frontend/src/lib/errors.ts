import { ApiError } from '@/api/client'

export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.code === 'network') return 'No se pudo conectar con el servidor. ¿Está arrancado el backend?'
    if (e.code === 'rate_limited') return 'Demasiadas peticiones al proveedor. Reintenta en unos segundos.'
    if (e.code === 'unavailable') return `Servicio no disponible: ${e.message}`
    return e.message
  }
  return e instanceof Error ? e.message : 'Error desconocido'
}

