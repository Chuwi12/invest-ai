import { Monitor, Moon, Sun } from 'lucide-react'
import { useCallback, useSyncExternalStore } from 'react'

export type ThemePref = 'system' | 'light' | 'dark'
const KEY = 'invest-ai.theme'
const listeners = new Set<() => void>()

function read(): ThemePref {
  try {
    const v = localStorage.getItem(KEY)
    return v === 'light' || v === 'dark' ? v : 'system'
  } catch {
    return 'system'
  }
}

let current: ThemePref = read()

/** Aplica la preferencia sin script en línea: `system` deja mandar a `prefers-color-scheme`. */
export function applyTheme(pref: ThemePref): void {
  const el = document.documentElement
  if (pref === 'system') el.removeAttribute('data-theme')
  else el.setAttribute('data-theme', pref)
}

export function initTheme(): void {
  applyTheme(current)
}

export function setThemePref(pref: ThemePref): void {
  current = pref
  try {
    if (pref === 'system') localStorage.removeItem(KEY)
    else localStorage.setItem(KEY, pref)
  } catch {
    /* sin almacenamiento: la preferencia dura la sesión */
  }
  applyTheme(pref)
  listeners.forEach((l) => l())
}

export function useTheme(): [ThemePref, (p: ThemePref) => void] {
  const pref = useSyncExternalStore(
    useCallback((l: () => void) => {
      listeners.add(l)
      return () => listeners.delete(l)
    }, []),
    () => current,
  )
  return [pref, setThemePref]
}

/** Siguiente en el ciclo del conmutador: sistema → claro → oscuro. */
export function nextTheme(p: ThemePref): ThemePref {
  return p === 'system' ? 'light' : p === 'light' ? 'dark' : 'system'
}

/** Nombre accesible y icono del conmutador, compartidos por el panel (Shell) y la landing. */
export const THEME_LABEL = { system: 'Tema: automático', light: 'Tema: claro', dark: 'Tema: oscuro' } as const
export const THEME_ICON = { system: Monitor, light: Sun, dark: Moon } as const

const dark = typeof window !== 'undefined' && typeof window.matchMedia === 'function' ? window.matchMedia('(prefers-color-scheme: dark)') : null

/** Esquema efectivo (`light` | `dark`) teniendo en cuenta la preferencia guardada y el sistema. */
export function useResolvedScheme(): 'light' | 'dark' {
  return useSyncExternalStore(
    useCallback((l: () => void) => {
      listeners.add(l)
      dark?.addEventListener('change', l)
      return () => {
        listeners.delete(l)
        dark?.removeEventListener('change', l)
      }
    }, []),
    () => (current === 'system' ? (dark?.matches ? 'dark' : 'light') : current),
  )
}
