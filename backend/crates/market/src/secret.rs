//! Valores secretos (claves de API). Su `Debug`/`Display` nunca muestran el contenido, de modo
//! que no pueden acabar en un log por accidente.

use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// `None` si la cadena está vacía o tiene caracteres que no caben en una cabecera/URL
    /// (saltos de línea, espacios...): una clave así solo puede ser un error de configuración.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let v: String = value.into();
        let v = v.trim().trim_matches(['"', '\'']).to_string();
        if v.is_empty() || v.len() > 256 || !v.chars().all(|c| c.is_ascii_graphic()) {
            return None;
        }
        Some(Self(v))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Sustituye cualquier aparición de los secretos en `text` (mensajes de error de terceros).
pub fn redact(text: &str, secrets: &[Secret]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if s.0.len() >= 4 {
            out = out.replace(&s.0, "***");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_value() {
        let s = Secret::new("abc123SECRET").unwrap();
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(
            redact("fallo con abc123SECRET en la url", &[s]),
            "fallo con *** en la url"
        );
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Secret::new("").is_none());
        assert!(Secret::new("   ").is_none());
        assert!(Secret::new("a b").is_none());
        assert!(Secret::new("a\nb").is_none());
        assert_eq!(Secret::new(" \"k3y\" ").unwrap().expose(), "k3y");
    }
}
