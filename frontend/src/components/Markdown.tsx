import { memo } from 'react'
import ReactMarkdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { safeHttpUrl } from '@/lib/sources'

/** Sin HTML crudo (no hay rehype-raw), sin imágenes y solo enlaces http(s) con noopener. */
const components: Components = {
  a: ({ href, children }) => {
    const safe = safeHttpUrl(href)
    return safe ? (
      <a href={safe} target="_blank" rel="noopener noreferrer nofollow">
        {children}
      </a>
    ) : (
      <span>{children}</span>
    )
  },
  img: ({ alt }) => <span className="muted">[imagen omitida{alt ? `: ${alt}` : ''}]</span>,
}

const REMARK = [remarkGfm]

export const Markdown = memo(function Markdown({ text, streaming }: { text: string; streaming?: boolean }) {
  return (
    <div className={`md ${streaming ? 'caret' : ''}`}>
      <ReactMarkdown remarkPlugins={REMARK} components={components} urlTransform={(u) => safeHttpUrl(u) ?? ''} skipHtml>
        {text}
      </ReactMarkdown>
    </div>
  )
})
