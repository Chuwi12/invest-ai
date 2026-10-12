import { useVirtualizer } from '@tanstack/react-virtual'
import { useRef, type ReactNode } from 'react'

interface Props<T> {
  items: readonly T[]
  estimate: number
  height: number | string
  render: (item: T, index: number) => ReactNode
  getKey: (item: T, index: number) => string
  label: string
}

/** Lista virtualizada con altura medida por elemento: solo se pintan las filas visibles. */
export function VirtualList<T>({ items, estimate, height, render, getKey, label }: Props<T>) {
  const parent = useRef<HTMLDivElement>(null)
  // eslint-disable-next-line react-hooks/incompatible-library
  const v = useVirtualizer({
    count: items.length,
    getScrollElement: () => parent.current,
    estimateSize: () => estimate,
    overscan: 6,
    getItemKey: (i) => getKey(items[i] as T, i),
    initialRect: { width: 800, height: typeof height === 'number' ? height : 480 },
  })
  return (
    <div ref={parent} role="list" aria-label={label} style={{ height, overflowY: 'auto', contain: 'strict' }}>
      <div style={{ height: v.getTotalSize(), position: 'relative' }}>
        {v.getVirtualItems().map((row) => (
          <div
            key={row.key}
            role="listitem"
            ref={v.measureElement}
            data-index={row.index}
            style={{ position: 'absolute', top: 0, left: 0, right: 0, transform: `translateY(${row.start}px)` }}
          >
            {render(items[row.index] as T, row.index)}
          </div>
        ))}
      </div>
    </div>
  )
}
