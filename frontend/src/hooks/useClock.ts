import { useEffect, useState } from 'react'

/**
 * Reloj que fuerza un render cada `everyMs` y devuelve la hora actual. La antigüedad de un dato (`ahora − hora del
 * dato`) cambia con el tiempo aunque no lleguen datos nuevos; sin esto, un feed parado dejaba de renderizar (React Query
 * conserva la misma referencia) y ni el «hace 3 min» avanzaba ni aparecía «retrasado».
 */
export function useClock(everyMs = 15_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), everyMs)
    return () => clearInterval(t)
  }, [everyMs])
  return now
}
