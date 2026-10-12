import type { Api } from './api'
import { httpApi } from './http'

let impl: Api = httpApi

/** Sustituye la implementación (lo usa el modo mock y los tests). */
export function setApi(next: Api): void {
  impl = next
}

/** Fachada estable: delega siempre en la implementación activa. */
export const api: Api = new Proxy({} as Api, {
  get: (_t, key: string | symbol) => Reflect.get(impl, key) as unknown,
})

export type { Api }
