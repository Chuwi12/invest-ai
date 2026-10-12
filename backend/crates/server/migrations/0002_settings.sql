-- Ajustes persistentes clave-valor (p. ej. `llm.active_model`, el modelo de IA activo).
CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);
