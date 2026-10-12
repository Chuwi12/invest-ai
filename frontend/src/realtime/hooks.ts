import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react'
import type { AssetId } from '@/api/types'
import type { ConnectionState, LiveTick } from './client'
import { getRealtime } from './index'

const noopUnsub = () => undefined

/**
 * Suscribe `id` al WebSocket (refcount) y devuelve el último tick. Solo se re-renderiza el
 * componente que lo usa cuando llega un tick de ESE activo.
 */
export function useLivePrice(id: AssetId | null | undefined): LiveTick | undefined {
  const client = getRealtime()
  useEffect(() => (id ? client.subscribe(id) : undefined), [client, id])
  const subscribe = useCallback(
    (listener: () => void) => (id ? client.subscribeStore(id, listener) : noopUnsub),
    [client, id],
  )
  const getSnapshot = useCallback(() => (id ? client.getTick(id) : undefined), [client, id])
  return useSyncExternalStore(subscribe, getSnapshot)
}

/**
 * Suscribe `id` y llama a `onTick` por cada tick SIN re-renderizar (gráficos, animaciones).
 * `onTick` puede cambiar entre renders sin resuscribir.
 */
export function usePriceTicks(id: AssetId | null | undefined, onTick: (t: LiveTick) => void): void {
  const client = getRealtime()
  const cb = useRef(onTick)
  useEffect(() => {
    cb.current = onTick
  })
  useEffect(() => {
    if (!id) return
    const release = client.subscribe(id)
    const off = client.onTick(id, (t) => cb.current(t))
    return () => {
      off()
      release()
    }
  }, [client, id])
}

export function useConnectionState(): ConnectionState {
  const client = getRealtime()
  return useSyncExternalStore(
    useCallback((l: () => void) => client.subscribeStatus(l), [client]),
    () => client.getStatus(),
  )
}
