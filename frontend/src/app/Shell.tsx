import { History, LayoutDashboard, MessageSquare, Monitor, Moon, Search, Settings, Star, Sun } from 'lucide-react'
import { lazy, Suspense, useEffect, useState } from 'react'
import { NavLink, Outlet } from 'react-router'
import { SkeletonRows } from '@/components/states'
import { IS_MOCK } from '@/lib/env'
import { StatusMenu } from './StatusMenu'
import { nextTheme, useTheme } from './theme'

const SearchPalette = lazy(() => import('./SearchPalette').then((m) => ({ default: m.SearchPalette })))

const NAV = [
  { to: '/dashboard', label: 'Panel', Icon: LayoutDashboard },
  { to: '/chat', label: 'Chat', Icon: MessageSquare },
  { to: '/favorites', label: 'Favoritos', Icon: Star },
  { to: '/history', label: 'Historial', Icon: History },
  { to: '/settings/models', label: 'Modelos de IA', Icon: Settings },
] as const

const THEME_LABEL = { system: 'Tema: automático', light: 'Tema: claro', dark: 'Tema: oscuro' } as const
const THEME_ICON = { system: Monitor, light: Sun, dark: Moon } as const

export function Shell() {
  const [searchOpen, setSearchOpen] = useState(false)
  const [theme, setTheme] = useTheme()
  const ThemeIcon = THEME_ICON[theme]

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)
      if ((e.key === 'k' || e.key === 'K') && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        setSearchOpen(true)
      } else if (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault()
        setSearchOpen(true)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="app">
      <aside className="rail">
        <div className="brand">
          invest<b>-ai</b>
        </div>
        <nav className="nav" aria-label="Principal">
          {NAV.map(({ to, label, Icon }) => (
            <NavLink key={to} to={to}>
              <Icon aria-hidden="true" /> {label}
            </NavLink>
          ))}
        </nav>
        <div className="rail-foot small muted">
          {IS_MOCK ? <span className="chip warn">Datos simulados</span> : null}
        </div>
      </aside>

      <header className="top">
        <button type="button" className="search-trigger" onClick={() => setSearchOpen(true)} aria-label="Buscar activos (atajo: barra o Ctrl+K)">
          <Search aria-hidden="true" />
          <span>Buscar activos</span>
          <kbd aria-hidden="true">Ctrl K</kbd>
        </button>
        <div className="right row">
          <StatusMenu />
          <button type="button" className="btn icon sm ghost" onClick={() => setTheme(nextTheme(theme))} aria-label={THEME_LABEL[theme]} title={THEME_LABEL[theme]}>
            <ThemeIcon aria-hidden="true" />
          </button>
        </div>
      </header>

      <main className="main" id="main">
        {IS_MOCK ? (
          <div className="mock-banner" role="status" style={{ margin: '-1.25rem -1.25rem 1rem' }}>
            Modo demostración: todos los datos, precios y análisis son simulados.
          </div>
        ) : null}
        <Suspense fallback={<div className="page"><SkeletonRows rows={4} h="3rem" /></div>}>
          <Outlet />
        </Suspense>
      </main>

      <footer className="foot">
        <span className="trunc"><span className="long">Información generada automáticamente. </span>No constituye asesoramiento financiero.</span>
      </footer>

      <nav className="mobile-nav" aria-label="Principal (móvil)">
        {NAV.map(({ to, label, Icon }) => (
          <NavLink key={to} to={to}>
            <Icon aria-hidden="true" />
            <span>{label.replace('Modelos de IA', 'Modelos')}</span>
          </NavLink>
        ))}
      </nav>

      {searchOpen ? (
        <Suspense fallback={null}>
          <SearchPalette onClose={() => setSearchOpen(false)} />
        </Suspense>
      ) : null}
    </div>
  )
}
