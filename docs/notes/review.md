# Revisión de corrección (bugs, no estilo) — invest-ai

Revisor: agente de solo lectura, 2026-10-13. Método: lectura del código + reproducción con un
crate temporal fuera del repo (`/tmp/claude-1000/-home-chuwi/review/irev`, depende del workspace
por path) y con curl contra Binance. Ningún fichero del repo se tocó salvo este.
Cada hallazgo lleva VERIFICADO (reproducido, con su salida) o SOSPECHA (leído, no reproducido).
La tabla resumen está al principio; el detalle, abajo en el orden en que se confirmó.

<!-- RESUMEN -->

## Hallazgos (detalle)

### R-01 — ALTO — Binance: el histórico con `startTime` + `limit=1000` devuelve el tramo VIEJO y termina años antes de hoy (VERIFICADO)

- `backend/crates/market/src/providers/binance.rs:150-165` (`history`) y `:79-98` (`range_start_ms`).
- Binance, con `startTime` y `limit`, devuelve las **primeras** `limit` velas desde `startTime`, no las últimas. El código
  pide siempre `limit=1000` y no aplica `coerce_interval` (Yahoo sí). Cuando `rango / intervalo > 1000` la serie se corta
  y **acaba en el pasado**, sin error ni aviso. El primer proveedor para cripto es Binance, así que no hay fallback.
- Combinaciones que ofrece la propia UI (`frontend/src/lib/chart.ts::intervalsFor`) y que fallan para cripto:
  `5y/1d`, `10y/1d`, `1mo/15m`, `1mo/30m`, `3mo/1h`, `6mo/1h`, `ytd/1h`, `1d/1m`, `5d/5m`.
  Las por defecto (`1d/5m`, `5d/15m`, `1mo/1h`, `1y/1d`, `5y/1wk`) caben en 1000 y funcionan.
- Reproducción (misma consulta que hace el código para `crypto:BTC`, `range=5y&interval=1d`):

```
$ S=$(( ($(date +%s) - 1827*86400)*1000 )); curl -s "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=1d&limit=1000&startTime=$S" | python3 -c "..."
1000 2021-10-13 00:00:00+00:00 2024-07-08 00:00:00+00:00      # hoy es 2026-10-13: faltan 27 meses
```
  Misma lógica para `3mo/1h`: 92 d × 24 = 2208 velas pedidas, se sirven las 1000 más antiguas (≈41 días, acabando hace ≈51).
- Impacto: el gráfico de BTC/ETH a 5 años en diario muestra una serie que acaba en 2024 como si fuera actual (la fuente y
  `as_of` mostrados son los de la última vela servida, así que al menos delatan la fecha si el usuario mira). `Range::Y2/D1`
  usado por `indicators` cabe (731 velas), por lo que **los indicadores y la puntuación no se ven afectados**.
- Arreglo: no mandar `startTime`; pedir las últimas N con `limit = min(1000, ceil(rango_s / intervalo_s) + 1)`, y si N > 1000
  paginar con `endTime` hacia atrás (o aplicar un `coerce_interval` propio de Binance como el de Yahoo):

```rust
let want = (range_secs(range) / interval.seconds() + 2).min(1000);
q.push(("limit", want.to_string()));      // sin startTime: Binance devuelve las ÚLTIMAS `limit`
// y para rangos que necesiten > 1000 velas, subir al siguiente intervalo con coerce_interval()
```

