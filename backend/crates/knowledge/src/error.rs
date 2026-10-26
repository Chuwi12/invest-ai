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

impl KnowledgeError {
    /// ¿Se arregla solo reintentando más tarde? BUSY y LOCKED de SQLite (también sus códigos extendidos) y el pool agotado.
    /// Corrupción, módulo FTS5 ausente o tabla borrada no.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Db(sqlx::Error::PoolTimedOut) => true,
            Self::Db(sqlx::Error::Database(d)) => d.code().is_some_and(|c| transient_code(&c)),
            Self::Embed(e) => e.is_transient(),
            _ => false,
        }
    }
}

/// `SQLITE_BUSY` (5) y `SQLITE_LOCKED` (6); el código extendido lleva el primario en el byte bajo
/// (262 = LOCKED_SHAREDCACHE, 517 = BUSY_SNAPSHOT). 267 = CORRUPT_VTAB no es transitorio.
fn transient_code(code: &str) -> bool {
    code.parse::<i32>().is_ok_and(|c| matches!(c & 0xff, 5 | 6))
}

pub type Result<T, E = KnowledgeError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_and_locked_are_transient_and_corruption_is_not() {
        for c in ["5", "6", "261", "262", "517", "773"] {
            assert!(transient_code(c), "{c}");
        }
        for c in ["1", "11", "267", "779", "", "x"] {
            assert!(!transient_code(c), "{c}");
        }
        assert!(KnowledgeError::Db(sqlx::Error::PoolTimedOut).is_transient());
        assert!(!KnowledgeError::Db(sqlx::Error::RowNotFound).is_transient());
        assert!(KnowledgeError::Embed(EmbedError::Unavailable("x".into())).is_transient());
        assert!(!KnowledgeError::Invalid("x".into()).is_transient());
    }
}
