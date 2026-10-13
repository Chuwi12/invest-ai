# invest-ai

Aplicación con IA nativa que analiza en tiempo real si un activo (acciones, ETFs, cripto, bonos,
índices, divisas, materias primas, futuros y opciones) es buena inversión, con datos en vivo,
histórico, fundamentales, noticias y un chat con un LLM local que justifica cada conclusión con
fuentes.

- **Frontend**: Vite + React + TypeScript (`frontend/`)
- **Backend**: Rust + Axum (`backend/`), SQLite para la persistencia
- **IA**: LLM local vía Ollama, con tool calling sobre datos en vivo

Diseño en [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) y contrato de la API en [`docs/API.md`](docs/API.md).

## Puesta en marcha

```bash
./invest-ai
```

Un solo comando: comprueba prerequisitos (cargo, node/npm, ollama), crea `.env` (permisos 600) y
`data/`, arranca Ollama si no está corriendo, descarga el modelo `LLM_MODEL` si falta, instala
dependencias y compila frontend y backend **solo si cambiaron**, arranca el servidor en
`127.0.0.1:8787` (frontend y API en un único puerto y proceso) y abre el navegador. Ctrl+C lo para
todo, incluido el Ollama que arrancó el propio script (nunca uno que ya estuviera corriendo).
Si ya hay una instancia, solo abre el navegador. Primer arranque: minutos (compilación y modelo);
en caliente, unos segundos.

| Comando | Qué hace |
|---|---|
| `./invest-ai` (`run`) | Lo anterior |
| `./invest-ai dev` (`./dev.sh`) | Backend en debug + Vite con HMR en `:5180`, un solo Ctrl+C |
| `./invest-ai stop` | Para la app (servicio o script) y el Ollama que arrancó el script |
| `./invest-ai status` | Estado de app, Ollama, modelo cargado (`ollama ps`), servicio y compilación |
| `./invest-ai logs [-f] [server\|ollama\|vite]` | Logs (en `~/.local/state/invest-ai/`, o journal si corre como servicio) |
| `./invest-ai open` | Abre el navegador; arranca la app si no corre (lo usa el icono) |
| `./invest-ai install [--autostart]` | Servicio systemd **de usuario** `invest-ai.service` endurecido + entrada en el menú de KDE; `--autostart` lo arranca al iniciar sesión |
| `./invest-ai uninstall` | Quita servicio y entrada del menú (no toca `data/` ni `.env`) |
| `./invest-ai selftest [--llm]` | Prueba de humo en < 30 s sin LLM (en ~1 s en la práctica): health, cotización real y reciente de `stock:AAPL` y `crypto:BTC` (con fuente y marca de tiempo, comprobando su antigüedad según el estado del mercado), un tick real por WebSocket, `/models` (hardware y modelo activo instalado) y cabeceras de seguridad. Usa la instancia en marcha o arranca una temporal en un puerto libre. `OK`/`FALLO` por punto; código de salida 1 si algo falla. `--llm` añade un análisis `quick` de AAPL (~40 s) |
| `--fake` | Datos simulados, sin red ni Ollama (demo) |
| `--no-browser` | No abrir el navegador |

Requisitos (sin sudo automático): `sudo pacman -S rust nodejs npm ollama` (con GPU NVIDIA,
`ollama-cuda`). Seguridad: el servidor solo escucha en loopback (el script se niega a arrancar si
`BIND_ADDR` no lo es) y nunca imprime `.env`. Notas de operación en [`docs/notes/ops.md`](docs/notes/ops.md).

> Aviso: la información que genera la app es análisis automatizado, no asesoramiento financiero.
