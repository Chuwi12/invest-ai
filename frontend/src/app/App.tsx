import { lazy } from 'react'
import { Link, Navigate, Route, Routes } from 'react-router'
import LandingPage from '@/pages/LandingPage'
import { Shell } from './Shell'

const Dashboard = lazy(() => import('@/pages/Dashboard'))
const AssetPage = lazy(() => import('@/pages/AssetPage'))
const ChatPage = lazy(() => import('@/pages/ChatPage'))
const FavoritesPage = lazy(() => import('@/pages/FavoritesPage'))
const HistoryPage = lazy(() => import('@/pages/HistoryPage'))
const ModelsPage = lazy(() => import('@/pages/ModelsPage'))
const PerformancePage = lazy(() => import('@/pages/PerformancePage'))
const IpoPage = lazy(() => import('@/pages/IpoPage'))

function NotFound() {
  return (
    <div className="page">
      <div className="empty">
        <p>Esta página no existe.</p>
        <Link to="/dashboard" className="btn primary" style={{ marginTop: '1rem' }}>
          Ir al panel
        </Link>
      </div>
    </div>
  )
}

export function App() {
  return (
    <Routes>
        {/* La landing va a página completa, fuera del Shell y con su propia cabecera. */}
        <Route path="/" element={<LandingPage />} />
        <Route element={<Shell />}>
          <Route path="dashboard" element={<Dashboard />} />
          <Route path="asset/:id" element={<AssetPage />} />
          
          <Route path="chat/:cid?" element={<ChatPage />} />
          <Route path="favorites" element={<FavoritesPage />} />
          <Route path="history" element={<HistoryPage />} />
          <Route path="performance" element={<PerformancePage />} />
          <Route path="ipo/:symbol?" element={<IpoPage />} />
          <Route path="settings" element={<Navigate to="/settings/models" replace />} />
          <Route path="settings/models" element={<ModelsPage />} />
          <Route path="*" element={<NotFound />} />
        </Route>
    </Routes>
  )
}
