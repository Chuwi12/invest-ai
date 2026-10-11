//! Separa el razonamiento `<think>…</think>` del texto de respuesta en un stream de fragmentos.
//!
//! Algunos modelos (qwen3, deepseek-r1…) lo mezclan en `content`; otros usan el campo
//! `thinking` de Ollama y no pasan por aquí. Las etiquetas pueden llegar partidas entre
//! fragmentos (`<thi` + `nk>`), por eso se retiene el sufijo que podría ser el inicio de una.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Thinking(String),
    Text(String),
}

#[derive(Debug, Default)]
pub struct ThinkSplitter {
    in_think: bool,
    buf: String,
    /// Tras `</think>` los modelos sueltan saltos de línea que no queremos mostrar.
    trim_leading: bool,
}

impl ThinkSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<Piece> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        loop {
            let tag = if self.in_think { CLOSE } else { OPEN };
            if let Some(i) = self.buf.find(tag) {
                let before: String = self.buf[..i].to_string();
                self.buf.drain(..i + tag.len());
                self.emit(before, &mut out);
                self.in_think = !self.in_think;
                self.trim_leading = !self.in_think;
                continue;
            }
            // Retiene el mayor sufijo que sea prefijo propio de la etiqueta esperada.
            let hold = (1..tag.len()).rev().find(|&k| self.buf.ends_with(&tag[..k])).unwrap_or(0);
            let emit_len = self.buf.len() - hold;
            let ready: String = self.buf[..emit_len].to_string();
            self.buf.drain(..emit_len);
            self.emit(ready, &mut out);
            break;
        }
        out
    }

    /// Vacía lo retenido al terminar el stream (una etiqueta incompleta es texto normal).
    pub fn finish(&mut self) -> Vec<Piece> {
        let rest = std::mem::take(&mut self.buf);
        let mut out = Vec::new();
        self.emit(rest, &mut out);
        out
    }

    fn emit(&mut self, mut text: String, out: &mut Vec<Piece>) {
        if self.trim_leading && !self.in_think {
            let trimmed = text.trim_start();
            if trimmed.is_empty() && !text.is_empty() {
                // Solo espacio: se sigue recortando con el siguiente fragmento.
                return;
            }
            text = trimmed.to_string();
        }
        if text.is_empty() {
            return;
        }
        if !self.in_think {
            self.trim_leading = false;
        }
        let piece = if self.in_think { Piece::Thinking(text) } else { Piece::Text(text) };
        // Une con el fragmento anterior si es del mismo tipo.
        match (out.last_mut(), &piece) {
            (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(b),
            (Some(Piece::Thinking(a)), Piece::Thinking(b)) => a.push_str(b),
            _ => out.push(piece),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String) {
        let mut s = ThinkSplitter::new();
        let (mut think, mut text) = (String::new(), String::new());
        let mut all: Vec<Piece> = chunks.iter().flat_map(|c| s.push(c)).collect();
        all.extend(s.finish());
        for p in all {
            match p {
                Piece::Thinking(t) => think.push_str(&t),
                Piece::Text(t) => text.push_str(&t),
            }
        }
        (think, text)
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(run(&["Hola ", "mundo"]), (String::new(), "Hola mundo".into()));
    }

    #[test]
    fn splits_think_block() {
        let (think, text) = run(&["<think>razono</think>\n\nRespuesta"]);
        assert_eq!(think, "razono");
        assert_eq!(text, "Respuesta");
    }

    #[test]
    fn tags_split_across_chunks() {
        let (think, text) = run(&["<thi", "nk>pien", "so</th", "ink>", "\n", "Listo <b>"]);
        assert_eq!(think, "pienso");
        assert_eq!(text, "Listo <b>");
    }

    #[test]
    fn char_by_char() {
        let src = "<think>uno dos</think>\nfin";
        let chunks: Vec<String> = src.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let (think, text) = run(&refs);
        assert_eq!(think, "uno dos");
        assert_eq!(text, "fin");
    }

    #[test]
    fn dangling_partial_tag_is_text_at_end() {
        let (think, text) = run(&["a < b y <thi"]);
        assert_eq!(think, "");
        assert_eq!(text, "a < b y <thi");
    }

    #[test]
    fn unterminated_think_stays_thinking() {
        let (think, text) = run(&["<think>pensando sin cerrar"]);
        assert_eq!(think, "pensando sin cerrar");
        assert_eq!(text, "");
    }

    #[test]
    fn utf8_in_think() {
        let (think, text) = run(&["<think>análisis € ", "ñ</think>Sí"]);
        assert_eq!(think, "análisis € ñ");
        assert_eq!(text, "Sí");
    }
}
