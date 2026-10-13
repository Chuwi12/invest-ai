-- Marca las respuestas del asistente cortadas por un error a mitad de generación (texto parcial).
ALTER TABLE messages ADD COLUMN truncated INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1));
