# Arquitectura

```
frontend (Vite + React + TS)  ──HTTP/SSE/WS──▶  server (Axum)
                                                 ├── market  (proveedores + caché + indicadores + hub tiempo real)
                                                 ├── ai      (Ollama: análisis estructurado + chat con tool calling)
                                                 ├── SQLite  (conversaciones, favoritos, historial, análisis)
                                                 └── domain  (tipos y traits compartidos — el contrato)
```

## Crates (`backend/crates`)

| Crate    | Responsabilidad | Depende de |
|----------|-----------------|------------|
| `domain` | Tipos serializables, `DomainError`, traits `MarketData` y `Analyst`. Sin I/O. | — |
| `market` | Implementa `MarketData`: un cliente por proveedor, enrutado por clase de activo con fallback, caché en memoria (moka) con TTL por tipo de dato, rate limiting por proveedor, cálculo de indicadores técnicos y un hub de tiempo real (WebSockets de Binance/Finnhub + sondeo para el resto) con broadcast y refcount de suscripciones. | `domain` |
| `ai`     | Implementa `Analyst` sobre Ollama (`/api/chat`, streaming, tools). Puntuación cuantitativa determinista previa al LLM, prompts, bucle de tool calling, salida JSON validada con reintento y sentimiento de noticias. | `domain` |
| `server` | Binario `invest-ai`: config por entorno, rutas Axum (REST + SSE + WS), persistencia SQLite (sqlx, migraciones embebidas), compresión, CORS, trazas y apagado ordenado. Es el único que conoce las implementaciones concretas. | `domain`, `market`, `ai` |

## Proveedores de datos

Todos son opcionales salvo los que no necesitan clave. Sin ninguna clave la app funciona con
Yahoo, Binance, CoinGecko, GDELT, Google News RSS y SEC EDGAR.

| Proveedor | Clave | Clases | Uso |
|-----------|-------|--------|-----|
| Yahoo Finance (no oficial) | no | stock, etf, fund, index, future, commodity, forex, option, bond (yields) | búsqueda, quote, histórico, fundamentales (quoteSummary con crumb), cadenas de opciones, noticias RSS |
| Binance (WS + REST público) | no | crypto | **tiempo real** (`@miniTicker`, `@trade`), velas |
| CoinGecko | opcional (demo) | crypto | búsqueda, metadatos, oferta circulante/máxima, ATH |
| Finnhub | `FINNHUB_API_KEY` | stock, etf, forex, crypto | **tiempo real** de acciones de EE. UU. por WS, noticias por empresa, recomendaciones de analistas, calendario de resultados |
| Alpha Vantage | `ALPHAVANTAGE_API_KEY` | stock, forex, crypto | respaldo de histórico, NEWS_SENTIMENT |
| Financial Modeling Prep | `FMP_API_KEY` | stock, etf | respaldo de fundamentales y ratios |
| Twelve Data | `TWELVEDATA_API_KEY` | stock, etf, forex, crypto | respaldo de quote e histórico |
| FRED (St. Louis Fed) | `FRED_API_KEY` | bond, macro | curva de tipos, fed funds, IPC, paro |
| SEC EDGAR | no (User-Agent) | stock (EE. UU.) | últimos filings (10-K, 10-Q, 8-K) |
| GDELT DOC 2.0 | no | todas | noticias globales en tiempo casi real |
| Google News RSS | no | todas | titulares por consulta |
| NewsAPI | `NEWSAPI_API_KEY` | todas | noticias |
| Marketaux | `MARKETAUX_API_KEY` | stock, crypto | noticias con entidades y sentimiento |
| CryptoPanic | `CRYPTOPANIC_API_KEY` | crypto | noticias cripto con votos |

TTL de caché orientativos: quote 2–5 s (o el último tick del hub), histórico intradía 60 s,
histórico diario 15 min, fundamentales 6 h, noticias 2–5 min, macro 1 h, búsqueda 10 min.

## IA: por qué no se «entrena» con los datos

Un LLM afinado (fine-tuning) con datos de mercado se queda congelado en la fecha de
entrenamiento y alucina cifras con aparente seguridad. Para que las respuestas sean **verídicas y
en tiempo real** el diseño es:

1. **Snapshot en vivo**: antes de responder, `market` reúne quote, indicadores, fundamentales,
   noticias y macro del activo, en paralelo y con fuentes y marcas de tiempo.
2. **Puntuación cuantitativa determinista** (`ai::scoring`): técnico, momentum, valoración,
   fundamentales, sentimiento, macro y riesgo, todo calculado en Rust. El LLM no se inventa estos
   números; los interpreta.
3. **Tool calling**: en el chat, el modelo pide datos (`get_quote`, `get_history`,
   `get_indicators`, `get_fundamentals`, `get_news`, `get_macro`, `search_assets`,
   `compare_assets`, `analyze_asset`), y cada resultado se cita con su fuente.
4. **Salida estructurada** (`Analysis` en JSON, validado con un reintento): veredicto
   (buy/accumulate/hold/wait/reduce/avoid), confianza, puntuaciones, tesis, casos alcista y
   bajista, riesgos, catalizadores, niveles clave y estrategia de entrada.
5. **Honestidad obligatoria en el prompt**: solo cifras presentes en el contexto, decir qué datos
   faltan y explicar la incertidumbre. No es asesoramiento financiero.

Modelo: configurable (`LLM_MODEL`). Con 8 GB de VRAM (RTX 4070 Laptop) caben bien modelos de
7–9 B en Q4 con tools; los MoE de unos 3 B activos (30 B en total) también funcionan bien si se
descargan parcialmente a RAM. Por defecto `qwen2.5:7b`, que ya está instalado. Comparativa en
`docs/LLM.md`.

## Rendimiento

- Backend: Tokio multihilo, `reqwest` con pool y HTTP/2, caché `moka` y deduplicación de
  peticiones en vuelo (single-flight), `tower-http` con compresión (br/gzip), SQLite en modo WAL
  con pool, indicadores calculados una sola vez por vela diaria y cacheados.
- Tiempo real: una única conexión WS por proveedor, compartida por todos los clientes (broadcast)
  y con throttling por activo.
- Frontend: rutas con code-splitting (`React.lazy`), TanStack Query (staleTime por tipo de dato),
  un solo WebSocket multiplexado, `lightweight-charts` (canvas), listas virtualizadas y
  memoización donde haya ticks.
