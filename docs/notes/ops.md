# Operación: `./invest-ai`, servicio systemd y lanzador de KDE

Estado: **funcional** (verificado con `--fake`; el modo real espera al cableado de `market`/`ai` en `main.rs`).

## Hecho
- `./invest-ai` (bash, `set -Eeuo pipefail`, sin `eval`, `.env` leído con `grep`, nunca impreso): prerequisitos con instrucciones pacman (sin sudo), `.env` desde `.env.example` (600), `data/` (700), Ollama (reutiliza el que corre; si no, `ollama serve` con `setsid` y log en `~/.local/state/invest-ai/ollama.log`), `ollama pull` solo si falta el modelo, `npm ci` solo si cambia `package.json`/`package-lock.json`, build del frontend y `cargo build --release` solo si cambia la huella (ruta+tamaño+mtime, sha256), servidor en `127.0.0.1:8787` sirviendo `frontend/dist`, espera a `/api/health`, `xdg-open`.
- Ctrl+C / SIGTERM: para el servidor (grupo de procesos propio, SIGTERM -> espera 14 s -> SIGKILL) y solo el Ollama que arrancó el script. Lock con `flock` (dos lanzamientos simultáneos: el segundo espera y abre el navegador). Si ya hay instancia: abre el navegador y sale.
- Subcomandos: `run`, `dev` (debug + Vite :5180, un Ctrl+C), `stop`, `status`, `logs [-f] [server|ollama|vite]`, `open`, `install [--autostart]`, `uninstall`, `--fake`, `--no-browser`. `dev.sh` es un atajo a `invest-ai dev`.
- `packaging/`: `invest-ai.service` (usuario, endurecido), `invest-ai-ollama.service` (Ollama de usuario, solo si el puerto 11434 está libre, `ExecCondition`), `invest-ai.desktop` (con acción "Detener"), `invest-ai.svg`. `install` renderiza `@ROOT@/@DATA@/@OLLAMA@` en `~/.config/systemd/user`, `~/.local/share/applications` y el icono.
- `ExecStartPre=invest-ai _prestart`: valida loopback, espera a Ollama y descarga el modelo.
- Seguridad del servicio: `systemd-analyze --user security invest-ai.service` -> **1.8 OK**. Según la corrección del coordinador se quitaron `ProcSubset=pid` y `PrivateDevices=yes`; con el resto del endurecimiento (incluido `SystemCallFilter=~@privileged @resources`) `systemd-run --user ... nvidia-smi --query-gpu=name,memory.total --format=csv` lista `NVIDIA GeForce RTX 4070 Laptop GPU, 8188 MiB` y `/proc/meminfo` se lee.
- Mediciones (CachyOS, `--fake`): arranque en caliente **0,1-4 s** hasta health OK (el máximo cuando otros agentes acababan de tocar fuentes y se recompiló el frontend); primera compilación release del backend 1 min 3 s; `npm ci` 3 s; `dev` 6,8 s con backend debug ya compilado.
- Verificado: doble lanzamiento (el segundo no arranca otro proceso), `stop`, Ctrl+C real (SIGINT con `timeout -s INT`): "parado limpiamente", `pgrep -x invest-ai` vacío, puertos 8787/5180 libres; servicio instalado, arrancado, `status`, `logs`, `open`, `stop`, `uninstall`; `invest-ai-ollama.service` queda `inactive (Result: exec-condition)` cuando ya hay Ollama.

## Pendiente
- `GET /api/models` -> `hardware.gpus` bajo el servicio: la ruta aún no existe en el servidor (devuelve 404); verificar cuando llegue. `nvidia-smi` sí funciona bajo las mismas restricciones.
- Modo real (sin `--fake`): el servidor aborta hoy en `build_services` (proveedores sin cablear); el script lo detecta y lo explica, pero no se pudo probar de punta a punta.
- `shellcheck` NO está instalado (no lo he instalado). Solo se comprobó `bash -n` y revisión manual de comillas.
- Probar el icono de KDE con un clic (no hay sesión gráfica interactiva aquí); `desktop-file-validate` no está disponible.
- Autoarranque (`install --autostart`) no probado (solo `enable`, sin reiniciar sesión).

## Decisiones
- Estado (pids, logs, huellas) en `~/.local/state/invest-ai/`, fuera del repo. Datos: `data/` según `DATABASE_URL` (el cwd del servidor es la raíz del repo).
- Servicio de usuario sin dependencia real de `ollama.service` (los servicios de usuario no pueden depender de unidades del sistema): se espera por HTTP en `_prestart`. Si hay que arrancar un Ollama de usuario, sus modelos van a `~/.ollama` (el del sistema usa `/var/lib/ollama`), es decir, se volvería a descargar el modelo.
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
