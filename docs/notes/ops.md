# Operación: `./invest-ai`, servicio systemd y lanzador de KDE

Estado: **funcional** (verificado con `--fake` y, el 13-oct-2026, en modo real: ver «`install` en modo real»).

## Hecho
- `./invest-ai` (bash, `set -Eeuo pipefail`, sin `eval`, `.env` leído con `grep`, nunca impreso): prerequisitos con instrucciones pacman (sin sudo), `.env` desde `.env.example` (600), `data/` (700), Ollama (reutiliza el que corre; si no, `ollama serve` con `setsid` y log en `~/.local/state/invest-ai/ollama.log`), aviso si falta el modelo (la descarga es solo con consentimiento, ver «Modelos: nada se descarga solo»), `npm ci` solo si cambia `package.json`/`package-lock.json`, build del frontend y `cargo build --release` solo si cambia la huella (ruta+tamaño+mtime, sha256), servidor en `127.0.0.1:8787` sirviendo `frontend/dist`, espera a `/api/health`, `xdg-open`.
- Ctrl+C / SIGTERM: para el servidor (grupo de procesos propio, SIGTERM -> espera 14 s -> SIGKILL) y solo el Ollama que arrancó el script. Lock con `flock` (dos lanzamientos simultáneos: el segundo espera y abre el navegador). Si ya hay instancia: abre el navegador y sale.
- Subcomandos: `run`, `dev` (debug + Vite :5180, un Ctrl+C), `stop`, `status`, `logs [-f] [server|ollama|vite]`, `open`, `pull [MODELO]`, `install [--autostart] [--no-build]`, `uninstall`, `--fake`, `--no-browser`. `dev.sh` es un atajo a `invest-ai dev`.
- `packaging/`: `invest-ai.service` (usuario, endurecido), `invest-ai-ollama.service` (Ollama de RESPALDO de usuario: no arranca si el puerto 11434 está ocupado ni si hay un `ollama.service` del sistema habilitado; dos `ExecCondition`), `invest-ai.desktop` (con acción "Detener"), `invest-ai.svg`. `install` renderiza `@ROOT@/@DATA@/@OLLAMA@` en `~/.config/systemd/user`, `~/.local/share/applications` y el icono.
- `ExecStartPre=invest-ai _prestart`: valida loopback, espera hasta 30 s a Ollama y avisa si falta el modelo. **No descarga nada y no bloquea el arranque** (ver abajo).
- Seguridad del servicio: `systemd-analyze --user security invest-ai.service` -> **1.8 OK**. Según la corrección del coordinador se quitaron `ProcSubset=pid` y `PrivateDevices=yes`; con el resto del endurecimiento (incluido `SystemCallFilter=~@privileged @resources`) `systemd-run --user ... nvidia-smi --query-gpu=name,memory.total --format=csv` lista `NVIDIA GeForce RTX 4060 Laptop GPU, 8188 MiB` y `/proc/meminfo` se lee.
- Mediciones (Linux, `--fake`): arranque en caliente **0,1-4 s** hasta health OK (el máximo cuando otros agentes acababan de tocar fuentes y se recompiló el frontend); primera compilación release del backend 1 min 3 s; `npm ci` 3 s; `dev` 6,8 s con backend debug ya compilado.
- Verificado: doble lanzamiento (el segundo no arranca otro proceso), `stop`, Ctrl+C real (SIGINT con `timeout -s INT`): "parado limpiamente", `pgrep -x invest-ai` vacío, puertos 8787/5180 libres; servicio instalado, arrancado, `status`, `logs`, `open`, `stop`, `uninstall`; `invest-ai-ollama.service` queda `inactive (Result: exec-condition)` cuando ya hay Ollama.

## Pendiente
- (Resuelto el 13-oct-2026) `/api/models` y `hardware.gpus` bajo el servicio y el modo real de punta a punta: verificados, ver «`install` en modo real».
- `shellcheck` NO está instalado (no lo he instalado; sigue sin estarlo el 23-oct-2026). Solo se comprobó `bash -n` y revisión manual de comillas.
- Probar el icono de KDE con un clic (no hay sesión gráfica interactiva aquí). `desktop-file-validate` sí está y da OK.
- Autoarranque (`install --autostart`): `enable`/`disable` probados; sin reiniciar sesión.

## Decisiones
- Estado (pids, logs, huellas) en `~/.local/state/invest-ai/`, fuera del repo. Datos: `data/` según `DATABASE_URL` (el cwd del servidor es la raíz del repo).
- Servicio de usuario sin dependencia real de `ollama.service` (los servicios de usuario no pueden depender de unidades del sistema): se espera por HTTP en `_prestart`. Si hay que arrancar un Ollama de usuario, sus modelos van a `~/.ollama` (el del sistema usa `/var/lib/ollama`), es decir, el modelo no estaría: por eso el de usuario es solo un respaldo que nunca arranca si hay uno del sistema habilitado (ver «Carrera de los dos Ollama»).
- Si el build del frontend falla pero hay un `dist/` previo, se avisa y se sirve el viejo; sin `dist/`, aborta.
- `dev` usa los binarios directamente (no `cargo run`) y `setsid` por grupo para que un Ctrl+C no deje procesos huérfanos. No recarga el backend al editar Rust (relanzar `dev`).
- No se imprime `.env`; `BIND_ADDR` no loopback se rechaza (script y `_prestart`).
- Un `kill -INT` a un proceso lanzado en segundo plano desde un shell no interactivo se ignora (comportamiento de bash); Ctrl+C en terminal y SIGTERM funcionan.

## Para el coordinador
1. **Ollama corre en CPU**: con el modelo cargado, `ollama ps` dice `100% CPU` y el log de `ollama.service` muestra `library=cpu`. Falta el paquete `ollama-cuda` (solo está `ollama` 0.40.1; `/usr/lib/ollama` no tiene backend CUDA/Vulkan). `sudo pacman -S ollama-cuda` (descarga ~762 MiB) y `sudo systemctl restart ollama`; después `ollama ps` debe decir 100% GPU. `./invest-ai status` avisa si detecta CPU. Lo ejecuta el usuario (sudo).
2. Endpoint `/api/models` y `hardware.gpus`: pendiente de verificar (404 ahora).
3. Modo real: cablear `market`/`ai` en `main.rs`; hasta entonces solo funciona `./invest-ai --fake` (o `INVEST_AI_FAKE=1` en `.env`, que el servicio también respeta).
4. "Modelo activo guardado": no hay ningún ajuste persistido que el script pueda leer sin tocar SQLite; solo se usa `LLM_MODEL`. Si el servidor guarda el modelo activo, exponer `GET /api/models` (activo) y `_prestart` podría consultarlo.
5. Sugerencia: `GET /api/health` podría incluir `fake: bool` para que `status` indique el modo.
6. `.gitignore` ya cubre `.env`, `data/`, `frontend/dist`, `backend/target`; nada nuevo que añadir. Ficheros nuevos para commitear: `invest-ai`, `dev.sh`, `packaging/*`, `docs/notes/ops.md`, README, `.env.example`.

## `./invest-ai install` en modo real (13-oct-2026)
Con el binario real (sin `--fake`), mercado abierto, `CARGO_TARGET_DIR` apuntando a otro destino de compilación y el puerto del servicio movido a :8784 (`systemctl --user set-environment BIND_ADDR=127.0.0.1:8784` y, al terminar, `unset-environment`; el valor por defecto sigue siendo 8787).
| Paso | Resultado |
|---|---|
| `./invest-ai install` | compila frontend (`npm run build`) y backend release (41 s incremental); instala `invest-ai.service`, `invest-ai-ollama.service`, `invest-ai.desktop` e icono. `systemd-analyze --user verify` sin salida; `desktop-file-validate` (ya instalado): OK |
| `systemctl --user start invest-ai.service` | `ExecStartPre=_prestart` OK (`modelo qwen2.5:7b disponible`), activo en 0,06 s, 3,2 MB, escuchando en 127.0.0.1:8784 |
| `./invest-ai selftest` contra el servicio | `todo OK (1.9 s)`: AAPL `mercado=regular hace 2 s`, BTC, tick WS, `/models` con la GPU (`NVIDIA GeForce RTX 4060 Laptop GPU`, el endurecimiento no la oculta), cabeceras y `Host` ajeno -> 403 |
| `systemd-analyze --user security invest-ai.service` | **1.8 OK** (sin cambios) |
| `./invest-ai status` / `stop` | `[servicio systemd]`, pid; `stop` -> `inactive`, puerto libre, `hasta luego` en el journal, `-wal`/`-shm` de SQLite retirados |
| `install --autostart` + `uninstall` | `Created symlink .../default.target.wants/invest-ai.service`, `is-enabled` = enabled; `uninstall` lo deshabilita y borra el directorio `default.target.wants` |
| `./invest-ai uninstall` | árbol de `~/.config/systemd`, `~/.local/share/applications` y `~/.local/share/icons` **idéntico** al anterior (`find -printf '%p %y %s'` + `diff`), `mimeinfo.cache` con el mismo sha256, sin unidades (`list-units`/`list-unit-files`), sin `BIND_ADDR` en el entorno del gestor, sin procesos ni puertos, sin `invest-ai.desktop` ni icono; `systemctl --user is-system-running` = running, sin unidades fallidas |
Lo que sí cambia (no es del servicio): `data/invest-ai.db` (migraciones al arrancar, mtime), el journal de usuario y las huellas de compilación de `~/.local/state/invest-ai/{backend,frontend}.stamp` (caché de compilación, cambian con cualquier edición del código).
Bugs encontrados y arreglados en `packaging/`:
1. **`ExecStart` fijo a `backend/target/release/invest-ai`** aunque `./invest-ai install` compile en otro destino (`CARGO_TARGET_DIR`): el servicio habría arrancado un binario viejo o inexistente. Ahora la plantilla usa `@BIN@` (el script lo rellena con `$BIN`, que ya respeta `CARGO_TARGET_DIR`, y se valida contra caracteres que las unidades no admiten).
2. **`invest-ai-ollama.service` no podía arrancar nunca**: `ReadWritePaths=%h/.ollama` con `~/.ollama` inexistente (aquí Ollama es el servicio del sistema, modelos en `/var/lib/ollama`) hace fallar la unidad al montar el espacio de nombres: `Failed to set up mount namespacing: ~/.ollama: No such file or directory`, `status=226/NAMESPACE`. Como `ExecCondition` trata cualquier salida 1-254 como «condición no cumplida», systemd lo registraba como `Skipped due to 'exec-condition'` y el fallo quedaba camuflado (también cuando el puerto estaba libre y debía arrancar). Arreglo: `ExecStartPre=+/usr/bin/mkdir -p -m 700 %h/.ollama` (el prefijo `+` lo ejecuta fuera de la jaula) y `ReadWritePaths=-%h/.ollama` (el `-` tolera que falte en el `ExecCondition`). Probado con una copia temporal en el puerto 11999: `ExecCondition` 0, `ExecStartPre` 0, `ollama serve` activo, `/api/version` = 0.40.3, CUDA detectada; tras la prueba se paró, se borró la unidad de prueba y `~/.ollama` (no existía antes). No se pudo probar con el puerto 11434 real porque el Ollama del sistema lo ocupa (y es compartido).
- Sigue sin probarse: el icono con un clic en KDE (no hay sesión interactiva) y el autoarranque con reinicio de sesión real.

## Cambios del 21-23-oct-2026 (incidente de la primera pasada real)

### Carrera de los dos Ollama (y su arreglo)
El 20-oct-2026, tras arrancar el equipo (el usuario tiene `linger`), el servicio de usuario `invest-ai-ollama.service` (respaldo, modelos en
`~/.ollama`, vacío) arrancó **antes** que el `ollama.service` del sistema (modelos en `/var/lib/ollama`, con un drop-in de caché KV `q8_0`) y
se quedó el puerto 11434. El del sistema murió con «bind: address already in use» y **no se reinicia** (`RestartPreventExitStatus=1`). Su
`ExecCondition` solo miraba que el puerto estuviera libre *en ese instante*. Resultado: ningún modelo disponible; `_prestart` intentó
descargar `qwen2.5:7b` (4,7 GB) en el almacén vacío y la lista de seguimiento marcó 20 de 20 activos como fallo permanente.
- **Arreglo en `packaging/invest-ai-ollama.service`**: segundo `ExecCondition=` que omite la unidad (sin error, `Result: exec-condition`) si
  `systemctl --system is-enabled ollama.service` dice que está habilitado. Se eligió esto y no «`install` no instala el respaldo cuando hay uno
  del sistema» porque se evalúa **en cada arranque**: sigue valiendo si el usuario habilita o deshabilita `ollama.service` después de instalar,
  y no depende de que alguien repita `./invest-ai install`. `systemctl --system is-enabled` es de solo lectura (sin sudo). Si el D-Bus del
  sistema no responde, `is-enabled` falla y el respaldo puede arrancar (fallo hacia abierto: sin gestor del sistema no hay Ollama del sistema).
- Probado dentro de la misma jaula de la unidad (`ProtectSystem=strict`, `NoNewPrivileges`…) con `systemd-run --user`: `is-enabled` devuelve
  `enabled` (código 0, la condición falla y la unidad se omite); con una unidad inexistente devuelve `not-found` (código 4, arranca).
- El drop-in temporal del usuario (`~/.config/systemd/user/invest-ai-ollama.service.d/no-si-hay-ollama-del-sistema.conf`) queda **redundante**
  tras reinstalar; es aditivo (otro `ExecCondition`), así que dejarlo no estorba y puede borrarse.
- `./invest-ai install` avisa si hay un `ollama.service` del sistema habilitado (el respaldo no arrancará nunca).
- **Orden de arranque**: una unidad de usuario no puede ordenarse respecto a una del sistema, así que `invest-ai.service` no «espera» a
  `ollama.service` por dependencia sino por HTTP en `_prestart` (hasta 30 s). Sigue siendo así, pero sin bloquear (ver abajo).

### `_prestart` no bloquea el arranque
Antes: sin Ollama tras 90 s, `_prestart` hacía `die` y, al ser `ExecStartPre`, la app no arrancaba y entraba en un bucle de reinicios
(`Restart=on-failure`) aunque la interfaz y los datos funcionan sin LLM. Ahora espera **hasta 30 s** y, si Ollama no responde, **avisa en el
journal y deja arrancar** la app; los análisis fallan con un error claro mientras no haya LLM y la lista de seguimiento comprueba Ollama y el
modelo antes de empezar y reintenta (ver `docs/notes/learning.md`). Sigue fallando (y debe) si `BIND_ADDR` no es loopback.
Verificado: servidor real arrancado con un `ExecStartPre` real, `LLM_BASE_URL` apuntando a un puerto muerto y el endurecimiento del
servicio: `_prestart` esperó 30 s, avisó, y `GET /api/health` -> `{"status":"ok"}`.

### Modelos: nada se descarga solo
- `_prestart` **avisa en el log** («el modelo X no está instalado en el Ollama de …») y sigue. Solo descarga con `AUTO_PULL_MODEL=1` en `.env`
  (desactivado por defecto; cualquier otro valor lo deja desactivado).
- `./invest-ai pull [MODELO]` descarga (por defecto `LLM_MODEL`) en el Ollama de `LLM_BASE_URL`; exige que Ollama responda. También se descarga
  desde Ajustes -> Modelos de la app.
- `./invest-ai run` / `dev` (interactivos) conservan la descarga de antes, pero **preguntan** (`¿Descargar X ahora (varios GB)? [s/N]`, por
  defecto no) y solo si hay terminal; sin terminal (icono de KDE, servicio) solo avisan.
- El script solo conoce `LLM_MODEL`; el modelo *activo* (el elegido en la UI) vive en la BD. La comprobación del modelo activo la hace la
  lista de seguimiento antes de cada ronda.
- Probado con un Ollama simulado: falta + sin `AUTO_PULL_MODEL` -> 0 llamadas a `ollama pull`; con `AUTO_PULL_MODEL=1` -> `ollama pull`;
  terminal real (pty) con «n» no descarga y con «s» sí; sin terminal no descarga.

### El servicio ejecuta una versión instalada inmutable, no el árbol de trabajo
Antes, `invest-ai.service` ejecutaba directamente `invest-ai _prestart`, `backend/target/release/invest-ai` y `frontend/dist` del repo: editar
el script o un `cargo build --release` / `npm run build` de cualquier agente cambiaba el servicio en producción en su siguiente reinicio.
Ahora `./invest-ai install` hace una **copia inmutable** en `~/.local/share/invest-ai/releases/<id>/` y las unidades apuntan allí:
- Contenido: `bin/invest-ai` (el binario), `frontend/dist/`, `invest-ai` (el script, que ejecuta el `ExecStartPre`) y `VERSION` (id, commit,
  fecha, proyecto). Todo de solo lectura (`chmod -R a-w`). Las migraciones y el catálogo de modelos van **embebidos** en el binario
  (`sqlx::migrate!`, `include_str!`); el servidor no lee nada más del árbol en ejecución salvo `.env` y `data/`.
- `<id>` es una huella de **contenido** (12 hex de sha256 del binario + `frontend/dist` + script): instalar dos veces lo mismo no crea nada;
  cambiar cualquier cosa crea otra carpeta.
- Siguen en el proyecto la configuración y los datos: `WorkingDirectory=<proyecto>` (para `.env` y `data/` relativos) y `ReadWritePaths=<data>`.
  La unidad añade `Environment=INVEST_AI_ROOT=<proyecto>` (la copia del script lo usa para encontrar `.env` y `data/`; solo lo honra una copia
  instalada, la que tiene `VERSION` al lado) y `Environment=STATIC_DIR=<release>/frontend/dist`.
- El icono de KDE (`invest-ai.desktop`) sigue llamando al script del repo: solo hace `systemctl --user start`.
- **Actualizar**: `git pull` (o los cambios que sean), luego `./invest-ai install` (compila lo que haga falta, copia la versión nueva y reescribe
  las unidades; `--no-build` instala lo ya compilado y falla si falta) y `systemctl --user restart invest-ai.service`. `install` no reinicia
  nada: el servicio en marcha sigue con la versión anterior hasta que se reinicie (avisa). Conserva las 3 versiones más recientes más la que
  usaba la unidad hasta entonces y la que ejecuta el proceso en marcha (`systemctl --user show -p MainPID` + `/proc/<pid>/exe`; sin esto, 4 `install`
  seguidos sin reiniciar borraban el `STATIC_DIR` del servidor y la UI daba 404). Volver atrás: reinstalar el código anterior o apuntar las unidades a una carpeta que siga en `releases/`; entre migraciones hay que restaurar también la copia de `data/` hecha antes de instalar (sqlx no arranca un binario sobre una BD con una migración que no conoce: `VersionMissing`).
- `./invest-ai status` muestra la versión instalada y avisa si el árbol de trabajo tiene otra build. `uninstall` borra también las versiones.
- Una instalación hecha antes de este cambio apunta todavía al árbol de trabajo (`status` lo avisa): basta con repetir `./invest-ai install`. Si la
  unidad apunta a `releases/<id>` y esa carpeta no existe (borrada a mano), `status` dice «la versión instalada <id> no existe: repite ./invest-ai
  install» (antes lo confundía con una instalación antigua y `systemctl start` fallaba en `ExecStartPre` sin pista).

#### Endurecimiento de `install`, la poda y `pull` (revisiones B-1, B-2, B-3, #6, #7, #8)
- **La poda no se fía de los nombres (B-1)**: las listas de `find` van separadas por NUL (`-printf '%T@ %p\0'`, `sort -z`, `read -d ''`); un directorio
  `a\nb` ya no produce la ruta `releases/a`. Solo se tocan hijos directos de `releases/`, que no sean enlaces y cuyo nombre sea el de una versión
  (`^[0-9a-f]{12}$`, el formato de `release_id`) o de un temporal (`.tmp-XXXXXX`); lo demás se ignora (no se cuenta ni se borra). El único `chmod -R`
  de la poda está en `drop_release`, detrás de esa validación (`chmod` sigue los enlaces de la línea de órdenes: así se cambiaban permisos de un árbol
  ajeno, y con `..\nx` los de todo `invest-ai/`). Consecuencia: una carpeta con otro nombre dentro de `releases/` se queda donde está.
- **`DATA_DIR` no puede abrir `ReadWritePaths` de más (B-2)**: `prepare_files` (y con él `install`, `run`, `dev` y `selftest`) aborta antes de su `mkdir`/`chmod 700` si
  `realpath -m $DATA_DIR` es `/` o `$HOME`, es igual o ancestro del proyecto, de `releases/` o del directorio de unidades, o está dentro de `releases/`.
  Un `DATABASE_URL` hacia `~/.local/share/invest-ai/` daba escritura sobre las versiones instaladas y permitía reescribir el binario del servicio.
- **`LLM_MODEL` se valida en `pull_model` (B-3)**, único punto por el que llega a `ollama pull` (lo usan `pull`, `_prestart` con `AUTO_PULL_MODEL=1` y el
  `run`/`dev` interactivos). Regex ASCII estricta con `LC_ALL=C` (con `es_ES.UTF-8`, `[A-Za-z]` aceptaba `á`) y la orden es `ollama pull -- "$modelo"`
  (cobra respeta `--`; probado sin red con `OLLAMA_HOST` a un puerto muerto). Sigue admitiendo `host/ns/modelo` (p. ej. `hf.co/...`): se cierra la
  inyección de opciones (`--insecure`), no la elección de registro. Efecto secundario: `pull` con un nombre no válido ya no falla antes de consultar
  a Ollama; si Ollama no responde, el error que se ve es ese.
- **`.tmp-*` (#7)**: `snapshot_release` instala un `trap ... EXIT` que borra el temporal si algo falla a mitad de la copia (`${tmp@Q}` se expande al
  instalar el trap: la `local` ya no existe cuando se ejecuta), y la poda borra los `.tmp-*` de más de 1 h (un `kill -9` no ejecuta el trap).
- Pruebas nuevas en `packaging/test-invest-ai.sh` (87 comprobaciones; contra el script anterior fallan 38): nombres con salto de línea, `..` y enlaces
  (la víctima mantiene permisos y contenido), versión en marcha simulada con un `sleep` copiado como binario de la versión y `systemctl show` simulado,
  `install` que falla a mitad (un `install` falso en el PATH), temporales viejos/recientes/hostiles, diez `DATABASE_URL` peligrosos (más uno hermano
  válido), `LLM_MODEL=--insecure`/`-x`/con tilde (con `LC_ALL=es_ES.utf8` si existe) sin llegar a `ollama`, y `status` con la versión ausente.
- Pruebas: `packaging/test-invest-ai.sh` (proyecto y `XDG_*` temporales, `systemctl`/`ollama` simulados, Ollama falso por HTTP; ~7 s, sale con 1 si algo
  falla) cubre `_prestart` (avisa/sigue/no descarga/`AUTO_PULL_MODEL`), `pull`, `install` (inmutable, idempotente, actualiza, poda, `--no-build`), la
  independencia del árbol de trabajo, las unidades (`systemd-analyze --user verify`) y `uninstall`. `INVEST_AI_PRESTART_WAIT=<s>` acorta la espera a Ollama
  de `_prestart` (30 s por defecto; solo lo usan las pruebas).
- Probado con un proyecto temporal, `XDG_*` temporales y un `systemctl` simulado (no se tocó el servicio real): crea la versión, es idempotente,
  el contenido es de solo lectura, las unidades pasan `systemd-analyze --user verify`; con el árbol de trabajo «envenenado» (script que
  falla, binario borrado, `index.html` distinto) la unidad (réplica con el endurecimiento real) arrancó desde la copia y sirvió el frontend
  instalado; al cambiar el contenido aparece otra versión y la poda deja las 3 últimas; `uninstall` lo retira todo.

### Migración 0008 (`watchlist_runs.status = 'missed'`)
`server/migrations/0008_watchlist_missed.sql` reconstruye `watchlist_runs` y `watchlist_results` (SQLite no permite cambiar un `CHECK`) para
admitir el estado `missed`. Copia todas las filas con sus ids y borra el hijo antes que el padre (con las claves ajenas activas, el `DROP` del
padre borraría en cascada los resultados). Probada con una **copia** de la BD del usuario (1 pasada, 20 resultados): mismas filas, `foreign_key_check`
e `integrity_check` limpios, `ON DELETE CASCADE` y el `CHECK` siguen vigentes; y con un test que migra 1..7, inserta filas y aplica la 0008.
Se aplica sola al arrancar el binario nuevo. **Recomendación: copiar `data/invest-ai.db*` antes de reiniciar con el binario nuevo.**
