# Notas del crate `ai`

Estado: **en curso**. Se actualiza tras cada parte.

## Hecho
- (previo) `scoring`, `context`, `sentiment`, `think`, `tokens`, `config`, `fixture`: puntuación determinista y contexto del LLM, con sus tests.
- `ollama.rs`: cliente de Ollama (`/api/chat` en streaming con tools, `format` JSON Schema, `think`, `num_ctx`, `keep_alive`; `/api/tags|show|ps|delete|pull`). Timeouts por inactividad, cancelación al soltar el stream. Tests contra un Ollama simulado (`testutil.rs`).
- `crates/ai/models.json` + `tools/gen_catalog.py`: catálogo verificado contra el registro (ver sección Catálogo).
- `domain`: tipos de modelos (`ModelTier`, `ModelRequirements`, `LlmModel`, `GpuInfo`, `HardwareInfo`, `ModelsResponse`, `PullEvent`), `AnalysisDepth`, traits `ModelService` y `SettingsStore`, y `Analyst::analyze_with` (método con implementación por defecto).

## Pendiente
- Todo lo demás (hardware, ModelManager, pipeline de análisis, chat con tools, rutas/migración del server, prueba real).

## Decisiones
- Unidades de memoria: GiB (2^30 bytes) en todos los `*_gb`.

## Para el coordinador
(se rellenará con la firma de cableado)
