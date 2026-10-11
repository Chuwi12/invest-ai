//! Idioma de los textos generados por el propio crate (razones de puntuación, cabeceras de
//! contexto, mensajes de aviso). Los prompts del LLM sí cubren otros idiomas pidiéndole que
//! responda en el idioma configurado; aquí solo hay español e inglés.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    /// "es", "es-ES", "es_MX" → `Es`; cualquier otro código → `En`.
    pub fn from_code(code: &str) -> Self {
        if code.trim().to_ascii_lowercase().starts_with("es") { Self::Es } else { Self::En }
    }

    pub fn t(self, es: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::Es => es,
            Self::En => en,
        }
    }
}

/// `tr!(lang, "texto {}", "text {}", arg)`: formatea la variante del idioma activo.
macro_rules! tr {
    ($l:expr, $es:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $l {
            $crate::lang::Lang::Es => format!($es $(, $arg)*),
            $crate::lang::Lang::En => format!($en $(, $arg)*),
        }
    };
}
pub(crate) use tr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_language() {
        assert_eq!(Lang::from_code("es-ES"), Lang::Es);
        assert_eq!(Lang::from_code("ES"), Lang::Es);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("fr"), Lang::En);
        assert_eq!(tr!(Lang::En, "hola {}", "hello {}", 3), "hello 3");
        assert_eq!(tr!(Lang::Es, "hola {}", "hello {}", 3), "hola 3");
    }
}
