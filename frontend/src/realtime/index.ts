import { RealtimeClient, defaultWsUrl } from './client'

let instance: RealtimeClient | null = null

/** Cliente WS único de la app (se crea al primer uso). */
export function getRealtime(): RealtimeClient {
  if (!instance) {
    instance = new RealtimeClient({ url: defaultWsUrl })
    if (typeof window !== 'undefined') {
      const retry = () => instance?.reconnectNow()
      window.addEventListener('online', retry)
      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') retry()
      })
    }
  }
  return instance
}

/** Sustituye el cliente (modo mock y tests). */
export function setRealtime(client: RealtimeClient): void {
  instance?.dispose()
  instance = client
}

export { RealtimeClient } from './client'
export type { ConnectionState, LiveTick, SocketLike, SocketFactory } from './client'
