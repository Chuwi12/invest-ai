export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toLocaleString('es-ES', { maximumFractionDigits: 2 })} GB`
  if (n >= 1e6) return `${(n / 1e6).toLocaleString('es-ES', { maximumFractionDigits: 0 })} MB`
  return `${Math.round(n / 1e3)} kB`
}
