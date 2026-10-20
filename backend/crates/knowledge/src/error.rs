//! Errores del crate.

/// Fallo al calcular embeddings.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// Ollama (u otro servidor) inalcanzable o con timeout: reintentable.
    #[error("servidor de embeddings no disponible: {0}")]
    Unavailable(String),
    /// El modelo no está instalado (`ollama pull <modelo>`; este crate nunca descarga modelos).
    #[error("modelo de embeddings no instalado: {0} (descárgalo con `ollama pull {0}`; la app nunca lo descarga)")]
    ModelMissing(String),
    /// Respuesta inválida (número de vectores, NaN, dimensiones incoherentes…).
    #[error("respuesta de embeddings inválida: {0}")]
    Bad(String),
}

impl EmbedError {
    /// ¿Merece la pena reintentar más tarde?
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("base de datos: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error("entrada no válida: {0}")]
    Invalid(String),
    #[error("falta el esquema del archivo de conocimiento: aplica la migración 0004_knowledge.sql")]
    SchemaMissing,
    #[error("error interno: {0}")]
    Internal(String),
}

pub type Result<T, E = KnowledgeError> = std::result::Result<T, E>;
