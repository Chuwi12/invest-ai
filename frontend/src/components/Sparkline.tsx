import { memo, useEffect, useRef, useState } from 'react'
import type { AssetId } from '@/api/types'
import { useSparkline } from '@/api/hooks'

/** Mini gráfico de línea en SVG (1 mes de cierres). Solo pide datos cuando entra en pantalla. */
export const Sparkline = memo(function Sparkline({ id, width = 84, height = 26 }: { id: AssetId; width?: number; height?: number }) {
  const ref = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(typeof IntersectionObserver === 'undefined')
  useEffect(() => {
    const el = ref.current
    if (visible || !el) return
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setVisible(true), { rootMargin: '120px' })
    io.observe(el)
    return () => io.disconnect()
  }, [visible])
  const { data } = useSparkline(id, visible)
  let path = ''
  let tone = 'flat'
  if (data && data.length > 1) {
    const min = Math.min(...data)
    const max = Math.max(...data)
    const span = max - min || 1
    path = data.map((v, i) => `${i === 0 ? 'M' : 'L'}${((i / (data.length - 1)) * width).toFixed(1)},${(height - 2 - ((v - min) / span) * (height - 4)).toFixed(1)}`).join('')
    tone = data[data.length - 1]! >= data[0]! ? 'up' : 'down'
  }
  return (
    <span ref={ref} style={{ display: 'inline-block', width, height }}>
      {path ? (
        <svg width={width} height={height} role="img" aria-label={`Tendencia de un mes: ${tone === 'up' ? 'al alza' : 'a la baja'}`}>
          <path d={path} fill="none" className={tone} stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
        </svg>
      ) : null}
    </span>
  )
})
