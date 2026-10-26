# Redes sociales (Reddit y X): opinión, no hechos

Estado: implementado, **apagado por defecto**. Sin credenciales no se crea ningún proveedor, no se
hace ninguna petición y la app (rutas, análisis, chat) es idéntica a la que no conoce las redes.

## Criterio

- Las redes son **opinión**, con alto riesgo de manipulación (bombeos, bots, cuentas pagadas) y de
  inyección de instrucciones en el texto.
- Sirven para medir **atención** (picos de menciones) y **tono** (alcista/bajista) del público
  minorista. No son evidencia de cifras ni de hechos.
- Su **valor predictivo no está demostrado**. Por eso:
  - **no entran en la puntuación ni en los pesos** (`ai::scoring`);
  - se guardan con su **fecha real** (publicación y consulta) para que `learning` pueda evaluarlas
    más adelante, sin anticipación, si algún día se quiere. Hoy `learning` no las lee.

## Acceso (comprobado el 19-oct-2026 con fuentes de terceros, no con peticiones reales)

Ninguna petición real se ha hecho durante el desarrollo: no había credenciales y X cuesta dinero.
Los tests usan servidores axum locales y fixtures inventados con la estructura de las respuestas
(`data.children` en Reddit; `data` / `includes` / `meta` en X). **El comportamiento real de ambas
APIs está sin contrastar**: al activarlo por primera vez, mira el log y `GET /api/social/status`.

| | Reddit | X |
|---|---|---|
| API | oficial, OAuth de solo aplicación (`client_credentials`) | oficial v2, búsqueda reciente |
| Coste | uso personal no comercial gratuito, ~100 consultas/min por cliente (se usa la mitad) | sin nivel gratuito desde feb-2026; pago por uso ≈ 0,005 USD por post leído |
| Requisitos | app «script» o «web» creada por ti; las apps nuevas necesitan aprobación manual de Reddit (desde finales de 2025) | `X_BEARER_TOKEN` + `X_MONTHLY_READ_BUDGET` obligatorio |
| No usar | el acceso sin autenticar (`.json`): 403 desde mayo de 2026 | scraping: incumple las condiciones |

## Cómo crear las credenciales

**Reddit**
1. Entra en Reddit con tu cuenta y abre <https://www.reddit.com/prefs/apps>.
2. «create another app…», tipo **script** (o web), nombre libre, `redirect uri` = `http://localhost:8080`
   (no se usa con `client_credentials`).
3. Si Reddit exige aprobación para apps nuevas, sigue su formulario (uso personal, no comercial) y espera.
4. `REDDIT_CLIENT_ID` = la cadena bajo el nombre de la app; `REDDIT_CLIENT_SECRET` = «secret».
5. `REDDIT_USER_AGENT` = `linux:invest-ai:0.1 (by /u/tu_usuario)` (Reddit lo exige descriptivo).
6. Pon las tres variables en `.env` y reinicia.

**X**
1. Cuenta de desarrollador en <https://developer.x.com>, crea un proyecto/app y activa el pago por uso
   con el límite de gasto más bajo que ofrezca el panel (es una segunda red de seguridad).
2. Genera el **Bearer Token** de la app → `X_BEARER_TOKEN`.
3. Decide el tope: `X_MONTHLY_READ_BUDGET=500` ≈ 2,50 USD/mes como máximo. **Sin esta variable X no se activa.**
4. `X_ACCOUNTS=cuenta1,cuenta2`: las eliges tú (la app no propone nombres). Solo se lee el cashtag
   (`$AAPL`) de esas cuentas (`from:`). `X_SEARCH_ALL=1` añade una búsqueda general por cashtag
   (sin retuits, con `X_LANGS`): es lo que más gasta y lo que más ruido y manipulación trae.
5. Reinicia y revisa `GET /api/social/status`.

El operador de cashtag en la búsqueda reciente puede estar restringido según el plan de X; no se ha
podido comprobar. Si la consulta falla, el error sale en la ruta de estado.

## Qué hace cada pieza

- `market::social` (config, emparejado de menciones, presupuesto) y `market::providers::{reddit,x}`.
- `domain`: `SocialPost`, `SocialMetrics`, `SocialDigest` y el trait `SocialData`.
- `ai::social`: tono, métricas, bloque `<social_sentiment>` y regla del system prompt.
- `server::social`: guardado, refresco con caché, presupuesto en SQLite y rutas.
- Migración `0007_social.sql`: `social_posts`, `social_runs`, `social_metrics`, `social_x_budget`.

### Reddit
Una consulta por grupo de ≤ 10 subreddits (`/r/a+b+c/search`, `restrict_sr`, `sort=new`, 100 resultados),
con la ventana `SOCIAL_DAYS` (7). Token en caché hasta su caducidad (se renueva ante 401). Limitador
propio, reintentos ante 429 (respeta `Retry-After`, máx. 60 s, 2 reintentos) y caché de 10 min.
Subreddits por defecto (los españoles están escritos de memoria y **sin verificar**; ajusta con `REDDIT_SUBREDDITS`):
generales `stocks, investing, StockMarket, wallstreetbets, SecurityAnalysis, ValueInvesting, Dividends`
(comunidades grandes con perfiles distintos: análisis, valor, especulación); ETF `ETFs, Bogleheads`;
cripto `CryptoCurrency, Bitcoin, ethereum, CryptoMarkets` (solo para activos cripto); español
`SpainFIRE, inversiones, finanzas`.

### X
Consulta `$TICKER (from:a OR from:b) -is:retweet`, agrupando cuentas para no pasar de ~420 caracteres.
Solo se pide `start_time` dentro de los últimos 7 días (límite de la API).

### Tickers ambiguos
Un ticker de 1-2 letras («F», «T», «AI») o que sea palabra corriente («ALL», «NOW», «LINK», «ONE»; lista
en `market::social`) **solo cuenta con cashtag (`$F`) o por el nombre completo** («Ford Motor»). Sin nombre
utilizable no se consulta nada. Tickers no ambiguos: cashtag, nombre, o el ticker suelto (3 letras solo en
mayúsculas; ≥ 4 en cualquier caso). Límites: nombres de una sola palabra corriente («Target», «Block»,
«Visa») pueden dar falsos positivos; se filtra en local cada resultado, pero no hay desambiguación semántica.

## Métricas (deterministas, en Rust)

Por activo y momento de consulta (`social_metrics`, solo se **añaden** filas, nunca se actualizan; cuándo se anota una, en «Instantáneas de métricas»):
menciones de las últimas 24 h y de la ventana, autores distintos (hash), alcistas/bajistas/neutras,
y comparación con la media de hasta 30 bloques previos de 24 h: cociente y z (con un suelo de Poisson
en la desviación). Tono = léxico de `ai/src/sentiment.rs` + jerga de redes (`calls`, `🚀`, `puts`…).
No hay etiqueta de usuario en Reddit/X; no se usa ninguna.

- Un bloque cuenta en la línea base **solo si hubo una consulta que lo cubriera** (`social_runs`):
  los días en que la app estaba apagada no son «cero menciones». Con < 5 bloques observados,
  `ratio` y `z` son `null` y el prompt lo dice. Es normal que el primer mes casi no haya línea base.
- Con el tope de 100 resultados por consulta, las menciones pueden ser una **cota inferior**
  (`truncated`). Las puntuaciones guardadas son las últimas vistas, no las del momento de publicar.
- Autores: hash SHA-256 truncado con sal fija. Es seudonimización, no anonimato fuerte.

## Análisis

El juez recibe un bloque `<social_sentiment>` **aparte** de `<knowledge_base>`: métricas y como mucho
5 extractos (≤ ~360 caracteres), etiquetados `S1…` (no `[D#]` ni `[N#]`). Reglas:
- el system prompt dice que es opinión anónima, no verificable, posiblemente manipulada, y que solo
  puede decirse «el sentimiento en redes es X»; nunca afirmar como hecho lo que diga un post;
- todo texto pasa por `sanitize_untrusted` (sin `<>`, `[]`, vallas, caracteres invisibles ni tokens de rol);
- el texto social **no entra en el `NumberIndex`**: una cifra que solo esté en un post no cuenta como
  respaldada y se retira (test `a_figure_only_present_in_a_post_is_not_backed`);
- las etapas previas no lo reciben; solo el juez;
- sin datos (apagado, sin publicaciones) el prompt es idéntico byte a byte (test `social_off_*` con la huella `STANDARD_GOLDEN`);
- si las redes fallan o tardan > 20 s, el análisis sigue y añade un aviso en `dataWarnings`.

## Presupuesto de X: cómo se garantiza el tope

1. `X_MONTHLY_READ_BUDGET` obligatorio; sin él X no se activa.
2. Contador por mes (UTC) en SQLite (`social_x_budget`), persistente entre reinicios.
3. Antes de cada petición se **reserva** `max_results` lecturas (atómico: mutex + transacción). X nunca
   devuelve más de `max_results`, y lo reservado nunca pasa del tope. Tras la respuesta se ajusta al
   número real de posts; si la petición falla, se devuelve la reserva.
4. La API exige ≥ 10 resultados: con menos de 10 lecturas de saldo se corta, se registra un `warn`
   («presupuesto mensual agotado») y la ruta de estado marca `exhausted`.
5. Caché de 30 min por consulta y mínimo `SOCIAL_REFRESH_MINUTES` (360) entre consultas de un activo.
6. Estimación de coste en el log de arranque y en `GET /api/social/status` (`estimatedCostUsd`, `maxMonthlyCostUsd`).
7. Con el presupuesto agotado hay una **espera de 1 h por activo** (en memoria): sin ella cada GET repetía la consulta y escribía otra fila `ok = 0` en `social_runs`
   sin poder conseguir nada. Pasada la hora, o tras reiniciar con más presupuesto, se vuelve a intentar.
8. Segunda barrera: `X_MAX_ASSETS_PER_HOUR` (más abajo) limita los activos DISTINTOS consultados en X por hora, para que símbolos inventados no agoten el mes en una tarde.
   El presupuesto mensual cuenta SIEMPRE, también para los activos que no cuentan para ese tope.

Sigue siendo una estimación: el cargo real lo decide X (p. ej. si factura también los usuarios
expandidos, o reintentos). Pon también un límite de gasto en el panel de X.

## Instantáneas de métricas: cuándo se anotan

Una fila de `social_metrics` es lo que se sabía en su `as_of`; es la serie que `learning` podrá evaluar algún día. Para que ni los duplicados ni los huecos la sesguen:

- **Solo tras un refresco real**: existe una ejecución correcta (`social_runs.ok = 1`) más reciente que la última instantánea del activo. Leer lo ya guardado, un fallo o
  el presupuesto agotado (`ok = 0`) no crean ninguna.
- **Como mucho una por activo y hora** (`METRICS_MIN_INTERVAL`).
- Las dos condiciones se deciden dentro de la propia sentencia, `INSERT … SELECT … WHERE NOT EXISTS (instantánea en la última hora) AND EXISTS (ejecución `ok = 1` posterior
  a la última instantánea)`, y no con un booleano en memoria. `digest_for` la intenta en **cada** llamada: es idempotente y barata (una sentencia cuyas subconsultas usan los índices por activo).
  Así resiste:
  - **cancelaciones**: el análisis corta `digest()` a los 20 s. Antes la decisión era `record = refreshed`, una variable de la petición; si se cancelaba después de guardar una
    ejecución `ok = 1` y antes de anotar la instantánea, no había otra hasta el siguiente refresco (360 min). Ahora la siguiente lectura la anota;
  - **reinicios** (no hay estado en memoria que recordar) y **peticiones concurrentes** (la sentencia es atómica).
- Si un refresco cae dentro de la hora de otra instantánea no se anota en el momento; la primera lectura pasada la hora la anota con su `as_of` real (los datos son los de la
  última consulta, así que no hay anticipación). Sin publicaciones guardadas no hay instantánea.

Tests (`server/tests/social.rs`): `a_refresh_saved_but_cancelled_before_the_snapshot_is_recorded_by_the_next_read`, `a_refresh_saved_before_a_restart_gets_its_snapshot_from_the_new_process`,
`metrics_snapshot_needs_a_real_refresh_and_is_at_most_hourly` y `a_failed_or_exhausted_refresh_records_no_snapshot`.

## Retención

Se aplica tras cada refresco y tras cada fallo registrado (`purge_old`), en la misma transacción que el guardado cuando la hay.

| Tabla | Se conserva | Criterio | Por qué |
|---|---|---|---|
| `social_posts` | 45 d | fecha de **publicación** | más que los 30 d de línea base + el día actual |
| `social_runs` | 45 d | `fetched_at` | dice qué periodos están observados; `stored_digest` mira 40 d atrás |
| `social_metrics` | 365 d | `as_of` | es el material que `learning` evaluará; con una por activo y hora son como mucho 8 760 filas por activo y año |
| `social_x_budget` | sin purga | — | una fila por mes |

## Tope de activos distintos en X (`X_MAX_ASSETS_PER_HOUR`)

- Entero entre 1 y 1000, **30 por defecto** (sin definir o vacío); no numérico o 0 → el valor por defecto, con un `warn` en el log. Cuenta activos DISTINTOS consultados en X en la última hora
  (ventana móvil de 60 min); un activo ya contado en la ventana sigue pudiendo consultarse. Reddit no tiene este tope.
- Al alcanzarlo, el activo nuevo no llega a X (no se escribe ninguna fila en `social_runs`), el error de esa lectura es «tope de activos distintos por hora en X alcanzado» y el log
  avisa una vez por ventana. Se sigue sirviendo lo de Reddit y lo guardado.
- **La lista de seguimiento no cuenta para el tope.** Su pasada diaria (20 activos por defecto) gastaba 20 de los 30 huecos; si el usuario abría 11 activos distintos esa hora,
  el resto de la lista se analizaba sin X y la serie que medirá `learning` quedaba sesgada por lo que se hubiera mirado ese día. Desde `SocialData::digest(&AssetSummary)` no se
  puede saber quién llama (el análisis corre en una tarea aparte, `analyze_stream`, y el trait no lleva origen), así que se exime **por activo, no por origen**: `SocialService::from_config`
  lee la misma configuración que el planificador (`WatchlistConfig::from_env`; con `WATCHLIST_ENABLED=0` no hay exentos). No abre una vía de abuso: es un conjunto fijo por
  configuración (no crece con símbolos inventados), y cada activo sigue limitado por `SOCIAL_REFRESH_MINUTES`, por la caché de 30 min y, sobre todo, por el **presupuesto mensual, que
  cuenta siempre**. Consecuencia aceptada: abrir a mano un activo de la lista tampoco ocupa hueco.
- **Por qué no se subió el valor por defecto en su lugar**: la pasada puede coincidir con el uso interactivo (con `WATCHLIST_TIME` por la mañana, p. ej. `09:15`, el usuario
  también está usando la app), y un tope más alto solo traslada el problema: la lista es configurable (`WATCHLIST` puede tener más de 20 activos) y lo que se abra a la vez sigue
  compitiendo por los mismos huecos. Con la exención, el resultado de la pasada no depende de lo que se mire ese rato ni del tamaño de la lista.
- `GET /api/social/status` expone el estado en `x.assetsPerHour`: `{ limit, used, full, windowMinutes, exemptAssets, nextSlotAt }` (`used` = huecos ocupados ahora, sin contar los
  exentos; `nextSlotAt` = cuándo se libera uno, `null` si no está lleno). Con `full: true`, los activos nuevos se consultan sin X hasta `nextSlotAt`.
- Tests: `watchlist_assets_do_not_use_the_hourly_x_asset_cap_and_the_status_reports_it`, `exempt_assets_still_spend_the_monthly_budget` y `x_is_queried_for_a_limited_number_of_distinct_assets_per_hour`.

## Esperas

| Cuándo | Espera | Dónde |
|---|---|---|
| Fallo de una consulta (red, HTTP, guardado) | 10 min por activo y red | memoria |
| Presupuesto mensual de X agotado | 1 h por activo | memoria |
| Última consulta correcta más reciente que `SOCIAL_REFRESH_MINUTES` (360) | hasta cumplirse | `social_runs` (persiste) |

Las esperas por fallo o por presupuesto viven en memoria: un reinicio las borra, y el primer GET tras él vuelve a intentarlo (con el presupuesto agotado, sin salir a la red).

## RAG (tarea opcional): no se ha hecho

Un tipo `Social` en `knowledge` obligaría a rehacer la tabla (el `kind` tiene un `CHECK`), tocar los
pesos de fuente, la retención y la herramienta del chat, y la ficha ya muestra lo guardado. Se deja
fuera; `RAG_SOCIAL_RETENTION_DAYS` no existe. Las publicaciones se conservan 45 días en `social_posts`.

## Límites y riesgos

- Reddit exige respetar sus términos de la API y el borrado de contenido que el usuario elimine; aquí
  se guarda texto recortado 45 días. Uso personal no comercial: revisa las condiciones vigentes.
- La manipulación no se detecta: un pico de menciones puede ser un bombeo. Por eso no puntúa.
- El tono por léxico es burdo (ironía, sarcasmo, jerga). Solo sirve como indicador grueso.
- Cobertura de Reddit: la búsqueda de Reddit es difusa y limitada a 100 resultados; `sort=new`.
- Sin verificar contra las APIs reales (ver arriba).

## Cómo probar sin red

`cargo test -p market --test social`, `-p ai --lib social`, `-p server --test social` y
`npx vitest run src/components/asset/SocialPanel.test.tsx`. En modo `INVEST_AI_FAKE=1` las redes
siempre están apagadas; el mock del frontend (`VITE_MOCK=1`) muestra datos inventados.
