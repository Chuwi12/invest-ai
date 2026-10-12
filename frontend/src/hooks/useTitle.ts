import { useEffect } from 'react'

export function useTitle(title: string | null | undefined): void {
  useEffect(() => {
    document.title = title ? `${title} · invest-ai` : 'invest-ai'
  }, [title])
}
