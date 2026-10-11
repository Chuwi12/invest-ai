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
cp .env.example .env          # añade las claves de API que tengas (todas opcionales)
ollama pull qwen2.5:7b        # o el modelo que pongas en LLM_MODEL
./dev.sh                      # backend en :8787 y frontend en :5180
```

> Aviso: la información que genera la app es análisis automatizado, no asesoramiento financiero.
