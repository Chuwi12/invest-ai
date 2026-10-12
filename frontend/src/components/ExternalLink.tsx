import type { ReactNode } from 'react'
import { safeHttpUrl } from '@/lib/sources'

/** Enlace externo seguro: solo http(s), `noopener noreferrer` y nueva pestaña. Si la URL no es válida, texto plano. */
export function ExternalLink({ href, children, className }: { href: string | null | undefined; children: ReactNode; className?: string }) {
  const safe = safeHttpUrl(href)
  if (!safe) return <span className={className}>{children}</span>
  return (
    <a href={safe} target="_blank" rel="noopener noreferrer" {...(className ? { className } : {})}>
      {children}
    </a>
  )
}
