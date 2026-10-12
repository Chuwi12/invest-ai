# Auditoría de seguridad — invest-ai

Fecha: 2026-10-12. Alcance: backend Rust (`domain`, `market`, `ai`, `server`), frontend React compilado, lanzador `./invest-ai`, unidades systemd de `packaging/`. Modelo de amenaza: app local monousuario sin autenticación en `127.0.0.1:8787`; el atacante realista es **una web maliciosa abierta en el navegador del usuario**, un proceso local sin privilegios, o datos hostiles de proveedores (titulares, RSS).

Metodología: lectura del código y pruebas contra el binario real (`INVEST_AI_FAKE=1`, puerto 8796, BD temporal) con `curl`, sockets en crudo y Chrome 131 headless. **Verificado** = reproducido contra el binario o medido; **Sospechado** = deducido del código, no reproducido. Código auditado en el estado de las 18:20 (otros agentes editaban `ai`, `market` y frontend en paralelo).

## Resumen

| Severidad | Nº | Ids |
|---|---|---|
| Crítica | 0 | |
| Alta | 1 | S-01 |
| Media | 2 | S-02, S-03 |
| Baja | 7 | S-04 a S-10 |
| Info | 4 | I-01 a I-04 |

Lo esencial: **el servidor no valida `Host` ni `Origin`**. Toda la defensa actual es CORS, que solo impide *leer* respuestas desde otro origen, no que la petición se ejecute, y no sirve contra DNS rebinding. Un único middleware de ~50 líneas (código verificado más abajo) cierra S-01, S-02 y S-04.

---

## Hallazgos

### S-01 — ALTA — Sin validación de `Host`: DNS rebinding da lectura y escritura completa de la API  (VERIFICADO)

- Componente: servidor HTTP. `backend/crates/server/src/routes/mod.rs:40-52` (`app()`, solo capa CORS) y `routes/ws.rs:88-95` (`origin_allowed` acepta cualquier `Origin` cuyo authority coincida con la cabecera `Host`).
- Descripción: ninguna ruta comprueba `Host`. Si una web `evil.example` re-resuelve su DNS a `127.0.0.1`, el navegador la trata como mismo origen: CORS deja de aplicar y la web lee y escribe toda la API (el puerto lo elige el atacante). En el WebSocket, `Origin: http://evil.example:8796` + `Host: evil.example:8796` pasa el chequeo por la rama "mismo origen".
- PoC (simula la petición que haría el navegador tras el rebinding):
  ```
  curl -si http://127.0.0.1:8796/api/conversations -H 'Host: evil.example:8796' -H 'Origin: http://evil.example:8796'
  -> 200 con el JSON de todas las conversaciones
  curl -si -N --max-time 2 http://127.0.0.1:8796/api/ws -H 'Host: evil.example:8796' -H 'Origin: http://evil.example:8796' \
     -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ=='
  -> 101 Switching Protocols
  ```
- Impacto: leer historial de chats, notas de favoritos, análisis, hardware (`/api/models`, `/api/status`); borrar conversaciones y favoritos; cambiar el modelo activo; **borrar cualquier modelo de Ollama del usuario** (`DELETE /api/models/{id}` no se limita al catálogo curado ni a modelos instalados por la app, `routes/models.rs:delete` -> `ai/src/models.rs:delete`); lanzar descargas de hasta decenas de GB (S-02). Matiz honesto: requiere que el usuario visite la web, y los navegadores recientes (Chrome con Local Network Access) pueden pedir permiso o bloquear parte del tráfico hacia loopback; no lo he verificado en navegador real, y Firefox/Safari protegen menos.
- Arreglo: middleware `guard` (más abajo), que rechaza con 403 cualquier `Host` que no sea `localhost`, `127.0.0.1`, `[::1]` o el authority de `FRONTEND_ORIGIN`. Defensa añadida: `DELETE /models/{id}` solo para ids que estén en `/api/tags` y, idealmente, solo los que la app descargó.

### S-02 — MEDIA — CSRF con peticiones "simples" (sin preflight): descargas de modelos, GPU y spam en BD  (VERIFICADO)

- Componente: `routes/mod.rs` (sin chequeo de `Origin`), `extract.rs:34` (`OptionalJson` no exige `Content-Type: application/json`), `routes/models.rs:pull`, `routes/analysis.rs:generate`, `routes/conversations.rs:create`.
- Descripción: `POST` sin cuerpo o con `Content-Type: text/plain` es una petición CORS "simple": el navegador la envía sin preflight y el servidor la ejecuta; CORS solo oculta la respuesta. Los `PUT/PATCH/DELETE` sí exigen preflight y quedan bloqueados (verificado: el preflight responde 200 pero sin `Access-Control-Allow-Origin`).
- PoC (cabeceras idénticas a un `fetch(url,{method:'POST',mode:'no-cors'})` o `<form method=post>` desde `https://evil.example`):
  ```
  curl -X POST http://127.0.0.1:8796/api/models/qwen3:14b/pull -H 'Origin: https://evil.example'   -> 200, descarga iniciada (en real: Ollama baja el modelo)
  curl -X POST http://127.0.0.1:8796/api/assets/stock:AAPL/analysis -H 'Origin: https://evil.example' -> 200, análisis completo (en real: carga la GPU, hasta 15 min)
  curl -X POST http://127.0.0.1:8796/api/conversations -H 'Origin: https://evil.example' -H 'Content-Type: text/plain' --data '{"title":"pwned"}' -> 200, conversación creada
  ```
  `POST /models/{id}/pull` solo acepta ids del catálogo curado (verificado: `evil:latest` -> 400), pero el catálogo incluye modelos de decenas de GB (`llama3.3:70b`).
- Impacto: consumo de disco/ancho de banda/GPU/batería, relleno de la BD con conversaciones y análisis, sin límite. No hay lectura de datos por esta vía ni borrado.
- Arreglo: el mismo `guard` (rechaza `Origin` no permitido y `Sec-Fetch-Site: cross-site` en métodos no seguros) + exigir `application/json` en `OptionalJson` cuando el cuerpo no está vacío.

### S-03 — MEDIA — El backend no envía ninguna cabecera de seguridad (CSP, clickjacking, nosniff…)  (VERIFICADO)

- Componente: `routes/mod.rs`, `routes/static_files.rs` (solo pone `Cache-Control`).
- Descripción: ninguna respuesta lleva `Content-Security-Policy`, `X-Content-Type-Options`, `Referrer-Policy`, `frame-ancestors`/`X-Frame-Options`, `Cross-Origin-Opener-Policy` ni `Cache-Control` en `/api/*` (`curl -si http://127.0.0.1:8796/` y `/api/status`). La app es enmarcable: una web maliciosa puede superponer un `<iframe>` de `http://127.0.0.1:8787` y hacer clickjacking sobre "borrar modelo", "borrar historial", etc. (los `DELETE` desde dentro del iframe son mismo origen y funcionan).
- Impacto: sin CSP, cualquier XSS futuro (hoy no hay, ver "Lo que está bien") sería total; el clickjacking es explotable hoy. Clasificado Media por el clickjacking sobre acciones destructivas.
- Arreglo: capa de cabeceras. CSP **probada en Chrome 131 contra el build real** (cero violaciones en `/`, `/asset/...`, `/favorites`, `/models`, `/chat`; el WebSocket `/api/ws` conecta con `connect-src 'self'`):
  ```
  Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self' 'sha256-3pRED1tOXas1FXFoPb9TGCjmYe9XQsmO9OV23khV2nY='; font-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; object-src 'none'; frame-ancestors 'none'
  X-Content-Type-Options: nosniff
  Referrer-Policy: no-referrer
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Resource-Policy: same-origin
  Permissions-Policy: camera=(), microphone=(), geolocation=()
  Cache-Control: no-store            (solo en /api/*)
  ```
  Notas: (a) lightweight-charts inyecta un `<style>` en línea; el hash de arriba es el de la versión instalada (`^5.2.1`) y **hay que recalcularlo si cambia** (el error de Chrome lo da en consola; alternativa menos estricta: `style-src 'self' 'unsafe-inline'`); (b) `frame-ancestors` solo funciona como cabecera HTTP, no como `<meta>`, por eso va en el backend; (c) en `vite dev` no aplicar esta CSP (HMR usa scripts en línea y WS a otro origen). Implementación con `tower_http::set_header::SetResponseHeaderLayer::if_not_present` sobre el router de estáticos y otra para `/api`.

### S-04 — BAJA — El WebSocket acepta cualquier `Origin` de loopback con cualquier puerto  (VERIFICADO)

- `routes/ws.rs:97-103`. `Origin: http://localhost:9999` -> `101`. Cualquier otra aplicación web local (otro servidor de desarrollo, una página servida desde `127.0.0.1:*`) puede abrir el socket. Impacto: solo recibe ticks de precios públicos, y puede forzar suscripciones (límite 100 por socket). Arreglo: quitar la rama "cualquier puerto de loopback" y quedarse con la lista `FRONTEND_ORIGIN` + mismo origen (con `Host` ya validado por S-01).

### S-05 — BAJA — El servidor no impone loopback; el lanzador y el servidor pueden discrepar sobre `BIND_ADDR`  (VERIFICADO)

- `server/src/config.rs:30-32` (cualquier `SocketAddr` vale) y `invest-ai:env_get` (`grep -E "^[[:space:]]*KEY="`).
- PoC: con `.env` = `export BIND_ADDR=0.0.0.0:8799` el lanzador no ve la clave (usa `127.0.0.1:8787`, pasa la validación) pero `dotenvy` sí la interpreta; el binario quedó escuchando en `0.0.0.0:8799` (`ss -ltn`). Lo mismo ocurre arrancando el binario a mano o con `cargo run`, que no pasan por `_prestart`.
- Impacto: API sin autenticación expuesta a la LAN (lectura de datos, borrado de modelos, GPU). Requiere un error de configuración del propio usuario.
- Arreglo: en `Config::from_env`, `anyhow::ensure!(bind_addr.ip().is_loopback(), "BIND_ADDR debe ser loopback")` (con opt-out explícito `INVEST_AI_ALLOW_NON_LOOPBACK=1`).

### S-06 — BAJA — Sin límites de concurrencia ni de conexiones (DoS local)  (VERIFICADO en WS; SOSPECHADO en LLM)

- `ws.rs` (100 suscripciones por socket pero sin tope de sockets), `routes/analysis.rs` (single-flight **por activo**, no global), `routes/chat.rs` (sin límite de chats simultáneos), sin `Semaphore` en el workspace (`grep -rn Semaphore` vacío).
- PoC: 3000 WebSockets simultáneos aceptados (`101`) sin rechazo; RSS del servidor en debug subió a ~415 MB; el `/api/health` siguió respondiendo. En producción con Ollama, N análisis de activos distintos o N chats se encolan en Ollama y bloquean la GPU, y cada uno retiene tareas y memoria hasta 900 s (`ANALYZE_TIMEOUT`).
- Impacto: degradación de la máquina; amplificado por S-01/S-02. Las cachés de `market` sí están acotadas (moka con `max_capacity`); no hay crecimiento ilimitado en memoria, pero sí en BD (conversaciones, análisis, `asset_cache`) sin poda.
- Arreglo: `Semaphore(2)` para análisis+chat (devolver 429 si no hay permiso), tope de ~32 WebSockets en `AppState`, y poda periódica de análisis/conversaciones.

### S-07 — BAJA — Límite de 16 MB de respuesta ineficaz con compresión/chunked  (SOSPECHADO, no probado)

- `market/src/http.rs:69-83`: el tope se comprueba con `content_length()` (que con gzip/brotli activados es `None` tras decodificar) y después de `resp.bytes().await`, es decir, con todo el cuerpo ya en memoria. Una respuesta comprimida "bomba" o con `Transfer-Encoding: chunked` agota memoria antes del chequeo. Los hosts son fijos y HTTPS (Yahoo, Binance…), así que exige un proveedor comprometido o un MITM con CA propia.
- Arreglo: leer con `resp.chunk()`/`bytes_stream()` acumulando y abortando al pasar el tope.

### S-08 — BAJA — Redirecciones de `reqwest` por defecto y claves de API en la query  (SOSPECHADO)

- `market/src/http.rs:18` no fija `redirect(Policy::...)`: sigue hasta 10 redirecciones, incluidas a IPs privadas/loopback. No hay SSRF con datos del usuario (los hosts son constantes; el usuario solo aporta símbolos y texto de búsqueda, que se codifican con `Url::path_segments_mut`/`append_pair`), pero si un proveedor devolviera un `302`, las claves que van en la query (FMP `apikey`, Alpha Vantage, FRED, Marketaux, CryptoPanic, Finnhub WS `?token=`) viajarían al destino (reqwest solo retira `Authorization`/cookies entre hosts, no la query).
- Arreglo: `redirect(Policy::custom(|a| if a.previous().len() < 3 && mismo_host { follow } else { stop }))` o `Policy::none()` para proveedores con clave.

### S-09 — BAJA — `AssetId` acepta símbolos arbitrarios (control, `/`, `..`, Unicode)  (VERIFICADO)

- `domain/src/lib.rs:124-136`: solo exige no vacío y ≤64 bytes. `GET /api/assets/stock%3A%00/quote` y `stock:a%0d%0ab` llegan al proveedor y vuelven reflejados en el mensaje de error (`"activo desconocido: stock:a\r\nb"`); ese mensaje se escribe en el log con `%message` (`error.rs`), permitiendo falsificar líneas de log (el nivel `debug` de 4xx lo reduce a poco impacto).
- Comprobado con la crate `url` 2.5.8: `extend(["v8","finance","chart", ".."])` produce `/v8/finance/chart` (el `..` sube un segmento); `a/b` -> `a%2Fb`; `../../x` -> `..%2F..%2Fx`. Es decir, **no hay path traversal hacia otros hosts**, solo puede cambiar de endpoint dentro del mismo proveedor.
- Arreglo: validar el símbolo con `^[A-Za-z0-9.^=\-_]{1,32}$` en `AssetId::from_str`.

### S-10 — BAJA — Inyección de prompt: bien mitigada, riesgo residual acotado  (VERIFICADO por revisión de código; no probado con un LLM real)

- Defensas existentes: `ai/src/safety.rs` neutraliza `<>`, `[]`, backticks, controles y caracteres invisibles en titulares, resúmenes, editores y nombres; el contexto va en `<market_data>`; el system prompt lo declara dato; las cifras de la salida se validan en Rust (`fidelity.rs`, `analysis/verify.rs`). Las 9 herramientas (`ai/src/tools.rs`) son **todas de solo lectura** sobre `MarketData` más `analyze_asset`; argumentos validados (`str_arg`, `int_arg`, `enum_arg`), 6 rondas máximo (`max_tool_rounds`) y 4 llamadas por ronda (`MAX_CALLS_PER_ROUND`). No hay herramienta de red arbitraria, ficheros ni shell, de modo que una noticia no puede exfiltrar datos por tool calling.
- Riesgo residual: (1) una noticia puede empujar al modelo a llamar `analyze_asset` repetidamente (coste de GPU, acotado por rondas); (2) el modelo puede emitir un enlace markdown hacia un dominio del atacante; el frontend solo lo hace clicable si es http(s), con `rel="noopener noreferrer nofollow"` y **omite imágenes** (`Markdown.tsx`), así que no hay exfiltración sin clic; (3) las `sources[].url` del análisis salen de los feeds y se muestran como enlace (phishing con clic).
- Mensaje de usuario: 8000 caracteres en la ruta (`chat.rs`) y 4000 en el analista (`safety.rs`); historial 40 mensajes. Pull de modelos solo del catálogo curado (verificado).

### I-01 — INFO — Dependencias

- `npm audit --omit=dev`: 0 vulnerabilidades.
- `cargo audit` **no está instalado** (no se ha instalado nada), por lo que **no hay análisis de RustSec**. Recomendado: `cargo install cargo-audit && cargo audit` en `backend/`, o `cargo deny`. Revisión manual del `Cargo.lock`: `axum 0.8.9`, `hyper 1.12.0`, `reqwest 0.12.28` (rustls), `sqlx 0.8.6`, `feed-rs 3.0.0`/`quick-xml 0.42.0` (no resuelve entidades externas), sin OpenSSL. Hay **tres versiones** de `tokio-tungstenite`/`tungstenite` (0.26, 0.29, 0.30) en el árbol: higiene de cadena de suministro, conviene unificarlas.

### I-02 — INFO — Lanzador `./invest-ai`

- Bien: `.env` se lee con `grep`, nunca con `source`/`eval` (sin inyección de comandos desde `.env`); `set -Eeuo pipefail`; estado en `~/.local/state/invest-ai` (700), logs 600, `.env` 600 (comprobado `stat`: 600), `data/` 700; `flock` evita carreras; el servidor se localiza por `/proc/<pid>/exe`, no por nombre.
- Menores: `cmd_stop` mata el PID de `run.pid` si su cmdline *contiene* "invest-ai" (`invest-ai:~680`), lo que también casa con p. ej. `tail -f …/invest-ai/server.log`; con PID reciclado podría matar un proceso propio inocente (mismo usuario, sin escalada). `ollama pull "$LLM_MODEL"` con un `LLM_MODEL` que empiece por `-` se interpreta como opción (entrada del propio `.env`). La plantilla `render` sustituye con `sed` y ya rechaza rutas con caracteres peligrosos. `BIND_ADDR` se valida en el script pero no en el binario (S-05).

### I-03 — INFO — systemd

- `systemd-analyze --user security --offline=true` sobre la unidad renderizada: **1.8 OK**. Incluye `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, `ReadWritePaths` solo para `data/`, `PrivateTmp`, `MemoryDenyWriteExecute`, `CapabilityBoundingSet=` vacío, `SystemCallFilter`, `RestrictAddressFamilies`, `UMask=0077`.
- Mejoras opcionales: `PrivateUsers=yes`, `ProcSubset=pid` y `PrivateDevices=yes` (retirados a propósito por `nvidia-smi`, según `ops.md`), `IPAddressDeny=any` + `IPAddressAllow` no es viable (proveedores públicos). La unidad de Ollama no tiene `ProtectHome` ni `ProtectKernelTunables`: Ollama puede leer todo `$HOME` (incluido `~/.ssh`); si no se necesita, añadir `ProtectHome=tmpfs` + `BindPaths=%h/.ollama`.

### I-04 — INFO — Secretos

- Bien: las claves son `Secret` (`market/src/secret.rs`): `Debug`/`Display` muestran `***`; `redact()` las limpia de los errores de terceros; `map_reqwest` usa `without_url()`; los errores del WebSocket de Finnhub (`?token=`) se registran solo con `ws_error_kind(&e)` (una etiqueta fija), nunca el error ni la URL (`market/src/realtime.rs:167,227`). `/api/status` solo expone `hasKey` booleano. El frontend no contiene claves ni las recibe. `.env` está en `.gitignore`.
- Menor: `TraceLayer` registra la URI completa a nivel INFO (incluye el texto de búsqueda `q` del usuario) en `server.log` (600).

---

## Lo que está bien (verificado salvo indicación)

- **SQL**: todas las consultas de `repo/*` usan `sqlx::query` con `?N` y binds; no hay `format!` en SQL. `' OR 1=1--` y `';DROP TABLE…` en `q` y en ids -> 200 `[]` / 404. Migraciones embebidas.
- **Path traversal en estáticos**: `/../../../etc/passwd`, `/%2e%2e/%2e%2e/etc/passwd`, doble codificación `%252e%252e`, `/assets/..%2f..%2f…`, `..%5c` -> siempre `index.html` (808 B) o 404; nunca contenido fuera de `dist`. `ServeDir` + fallback de SPA que da 404 a rutas con extensión o bajo `/assets/`.
- **Límites de entrada**: cuerpo 256 KiB (300 KB -> 413), URI larga -> 414, `ids` de `/quotes` ≤50, `limit` acotado con `clamp` y rechazo de negativos/desbordados (400), `content` del chat ≤8000, título ≤200, nota ≤2000, `AssetId` ≤64 bytes, WS: mensaje ≤16 KiB, 100 suscripciones, `IDLE_LIMIT` 90 s, `SEND_TIMEOUT` 5 s.
- **CORS**: lista blanca explícita (`FRONTEND_ORIGIN`); un origen ajeno recibe preflight sin `Access-Control-Allow-Origin` (verificado), por lo que no puede leer respuestas ni lanzar `PUT/PATCH/DELETE`. El WebSocket sí comprueba `Origin` (un origen ajeno -> 403, verificado).
- **XSS**: `react-markdown` sin `rehype-raw` y con `skipHtml`, `urlTransform` que solo deja http(s), imágenes sustituidas por texto, enlaces `target=_blank` con `noopener noreferrer`; `ExternalLink` aplica `safeHttpUrl` (descarta `javascript:`/`data:`). No hay `dangerouslySetInnerHTML`, `innerHTML`, `eval`. `localStorage` solo guarda la preferencia de tema (y el mock de desarrollo). `<meta name="referrer" content="no-referrer">` ya está en `index.html`.
- **Egress**: ninguna URL saliente contiene datos del usuario en el host; símbolos y consultas van codificados como segmentos o pares de query. Timeouts de conexión (5-6 s) y de petición en todos los clientes. RSS con `feed-rs` (sin entidades externas). Single-flight acotado y cachés con capacidad máxima.
- **Pull/borrado de modelos**: el pull exige estar en el catálogo curado (`catalog::find`), una descarga por modelo, y no se puede borrar el modelo activo.
- **Superficie de proceso**: `bind` por defecto 127.0.0.1; el lanzador rechaza hosts no loopback; apagado ordenado; no hay `unsafe`/`Command` con entrada del usuario (el único `Command::new` es `hardware.rs:90` con argumentos fijos).

---

## Arreglo recomendado de S-01, S-02 y S-04 (probado)

Probado en una copia del crate `server` (no se ha tocado el repo): compilado y atacado con `curl` -> `Host: evil.example` 403 (HTTP y WS), `POST` con `Origin: https://evil.example` 403 (incluido `text/plain` y `pull`), `Sec-Fetch-Site: cross-site` 403, `curl` sin `Origin` 200, mismo origen 200, `Origin: http://localhost:5180` (Vite) 200, `Host: [::1]:…` 200, WebSocket legítimo 101, preflight de origen ajeno sigue sin `Access-Control-Allow-Origin`.

Nuevo fichero `backend/crates/server/src/guard.rs` (y `pub mod guard;` en `lib.rs`):

```rust
//! Defensa del navegador -> servidor local: valida `Host` (anti DNS rebinding) y `Origin` /
//! `Sec-Fetch-Site` en peticiones que cambian estado (anti CSRF).
use std::sync::Arc;
use axum::{Json, extract::{Request, State}, http::{HeaderMap, Method, StatusCode, header},
           middleware::Next, response::{IntoResponse, Response}};
use serde_json::json;
use crate::state::Settings;

fn authority_of(origin: &str) -> Option<&str> {
    origin.split_once("://").map(|(_, a)| a.trim_end_matches('/'))
}

fn host_allowed(host: &str, settings: &Settings) -> bool {
    let host = host.to_ascii_lowercase();
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
        || settings.allowed_origins.iter().filter_map(|o| authority_of(o)).any(|a| a.eq_ignore_ascii_case(&host))
}

fn origin_allowed(headers: &HeaderMap, settings: &Settings) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let origin = origin.trim_end_matches('/');
    if settings.allowed_origins.iter().any(|a| a.eq_ignore_ascii_case(origin)) { return true; }
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).zip(authority_of(origin))
        .is_some_and(|(h, a)| h.eq_ignore_ascii_case(a)) // mismo origen; el Host ya está validado
}

fn deny(code: &'static str, msg: &'static str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": { "code": code, "message": msg } }))).into_response()
}

pub async fn guard(State(settings): State<Arc<Settings>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if !headers.get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(|h| host_allowed(h, &settings)) {
        return deny("forbidden_host", "Host no permitido");
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        if !origin_allowed(headers, &settings) { return deny("forbidden_origin", "origen no permitido"); }
        if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()).is_some_and(|v| v == "cross-site") {
            return deny("forbidden_origin", "petición cross-site");
        }
    }
    next.run(req).await
}
```

Y en `routes/mod.rs::app()`:

```rust
let settings = state.settings.clone();
// ...
app.layer(DefaultBodyLimit::max(BODY_LIMIT))
    .layer(cors(&origins))
    .layer(middleware::from_fn_with_state(settings, crate::guard::guard))   // la más externa
    .layer(TraceLayer::new_for_http() /* ... */)
```

Para `ws.rs`: eliminar el bloque "Cualquier puerto de loopback" (S-04) o delegar en `guard`. Para S-02, además, en `OptionalJson::from_request` rechazar con 415 si el cuerpo no está vacío y `Content-Type` no es `application/json`.

Nota: `guard` bloquea cualquier cliente que envíe `Host` distinto de loopback (p. ej. un proxy inverso con otro dominio): en ese caso añadir el dominio a `FRONTEND_ORIGIN`.

## Prioridad de arreglos

1. `guard` (Host + Origin + Sec-Fetch-Site): cierra S-01, S-02, S-04.
2. Cabeceras de seguridad con la CSP probada (S-03), `frame-ancestors 'none'` y `Cache-Control: no-store` en `/api`.
3. Forzar loopback en `Config::from_env` (S-05) y `application/json` obligatorio en `OptionalJson`.
4. `Semaphore` global para análisis/chat y tope de WebSockets (S-06); `DELETE /models/{id}` solo sobre modelos instalados y, si es posible, solo los descargados por la app.
5. Validar el símbolo de `AssetId` (S-09), cuerpo con tope en streaming y política de redirecciones (S-07/S-08); instalar `cargo-audit` (I-01).
