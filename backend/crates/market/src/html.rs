//! HTML de los documentos de la SEC (folletos S-1/F-1/424B4) a líneas de texto limpio.
//!
//! No hay un parser HTML en las dependencias y el HTML de EDGAR es de generación mecánica
//! (Donnelley, Workiva, Toppan Merrill, EdgarAgents...), así que basta un tokenizador propio:
//!
//! - quita `<script>`, `<style>`, `<head>`, el bloque oculto `ix:header` de iXBRL y todo lo que
//!   lleve `display:none`;
//! - decodifica entidades (nombradas comunes y numéricas) y colapsa espacios (también `&nbsp;`);
//! - cada párrafo / `<div>` / `<br>` / `<li>` es una línea;
//! - cada fila de tabla es una línea con las celdas unidas por « | »; los trozos típicos de las
//!   tablas financieras (`$`, `(1,234`, `)`, `%`) repartidos en celdas se vuelven a pegar;
//! - cada línea recuerda si TODO su texto va en negrita o con énfasis (cursiva/subrayado): así
//!   se reconocen los títulos de sección sin depender del texto.

use std::collections::HashMap;

/// Una línea de texto ya limpia.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Todo el texto de la línea va en negrita.
    pub bold: bool,
    /// Todo el texto va en negrita, cursiva o subrayado (candidata a título o subtítulo).
    pub emph: bool,
    /// Fila de tabla (celdas separadas por « | »).
    pub table: bool,
}

/// Convierte un documento HTML en líneas. Tolera HTML mal formado y el envoltorio SGML
/// (`<DOCUMENT><TYPE>…`) que a veces precede al HTML en la descarga de EDGAR.
pub fn html_to_lines(html: &str) -> Vec<Line> {
    let html = strip_sgml_header(html);
    let mut p = Parser::default();
    p.run(html);
    p.finish()
}

/// Si el documento empieza por el envoltorio SGML de EDGAR, salta hasta el primer `<html`.
fn strip_sgml_header(s: &str) -> &str {
    let head: String = s.chars().take(64).collect::<String>().to_ascii_lowercase();
    if !head.trim_start().starts_with("<document>") {
        return s;
    }
    let limit = s.len().min(8_192);
    let lower = s.as_bytes()[..limit].to_ascii_lowercase();
    match find_bytes(&lower, b"<html") {
        Some(i) => &s[i..],
        None => s,
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Posición de la primera `</name` de `hay` (sin distinguir mayúsculas), sin copiar nada. `name` es
/// ASCII. Recorre `hay` una sola vez: cada `<` cuesta una comparación de longitud fija.
fn find_close_tag(hay: &[u8], name: &[u8]) -> Option<usize> {
    let need = 2 + name.len();
    let mut from = 0;
    while let Some(p) = hay[from..].iter().position(|&c| c == b'<') {
        let at = from + p;
        // Si no cabe la etiqueta entera en lo que queda, tampoco cabrá en un `<` posterior.
        let w = hay.get(at..at + need)?;
        if w[1] == b'/' && w[2..].eq_ignore_ascii_case(name) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Acumulador de texto con indicadores de énfasis
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct TextBuf {
    text: String,
    chars: usize,
    bold: usize,
    emph: usize,
    pending_space: bool,
}

impl TextBuf {
    fn push(&mut self, s: &str, bold: bool, emph: bool) {
        for ch in s.chars() {
            if is_space(ch) {
                if !self.text.is_empty() {
                    self.pending_space = true;
                }
                continue;
            }
            if matches!(ch, '\u{200b}' | '\u{feff}' | '\u{200c}' | '\u{200d}' | '\u{ad}') {
                continue;
            }
            if self.pending_space {
                self.text.push(' ');
                self.pending_space = false;
            }
            self.text.push(ch);
            self.chars += 1;
            if bold {
                self.bold += 1;
            }
            if bold || emph {
                self.emph += 1;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.chars == 0
    }

    fn take(&mut self) -> TextBuf {
        std::mem::take(self)
    }

    fn all_bold(&self) -> bool {
        self.chars > 0 && self.bold == self.chars
    }

    fn all_emph(&self) -> bool {
        self.chars > 0 && self.emph == self.chars
    }

    fn into_line(self, table: bool) -> Line {
        Line {
            bold: self.all_bold(),
            emph: self.all_emph(),
            text: self.text,
            table,
        }
    }
}

fn is_space(ch: char) -> bool {
    ch.is_whitespace() || ch == '\u{a0}' || ch == '\u{2007}' || ch == '\u{202f}'
}

// ---------------------------------------------------------------------------------------------
// Tablas
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Cell {
    parts: Vec<TextBuf>,
    cur: TextBuf,
}

impl Cell {
    fn break_part(&mut self) {
        if !self.cur.is_empty() {
            let b = self.cur.take();
            self.parts.push(b);
        } else {
            self.cur = TextBuf::default();
        }
    }

    fn finish(mut self) -> Vec<TextBuf> {
        self.break_part();
        self.parts
    }
}

#[derive(Debug, Default)]
struct Row {
    cells: Vec<Vec<TextBuf>>,
    open: Option<Cell>,
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Frame {
    name: String,
    bold: bool,
    emph: bool,
    hidden: bool,
}

#[derive(Default)]
struct Parser {
    lines: Vec<Line>,
    cur: TextBuf,
    stack: Vec<Frame>,
    /// Marcos abiertos por nombre (siempre coincide con el contenido de `stack`). Permite saber en
    /// O(1) si un `</x>` tiene apertura; sin esto cada cierre huérfano recorría la pila entera
    /// (hasta `MAX_DEPTH`), y un documento de cierres sueltos costaba O(n · MAX_DEPTH).
    open: HashMap<String, usize>,
    hidden: usize,
    table_depth: usize,
    row: Option<Row>,
}

const VOID: &[&str] = &[
    "br", "hr", "img", "meta", "link", "input", "col", "area", "base", "wbr", "param", "source",
    "track", "embed",
];
const BLOCK: &[&str] = &[
    "p", "div", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article",
    "header", "footer", "center", "blockquote", "pre", "hr", "dd", "dt", "dl", "caption", "body",
    "html", "br", "form", "fieldset", "figure", "figcaption", "address", "nav", "aside", "main",
];
const SKIP_SUBTREE: &[&str] = &[
    "head", "title", "script", "style", "noscript", "template", "ix:header", "xbrli:context",
];
/// Elementos que se cierran solos al abrirse otro igual (HTML antiguo sin cierre).
const AUTO_CLOSE: &[&str] = &["p", "li", "td", "th", "tr", "dd", "dt", "option"];
const MAX_DEPTH: usize = 2_000;

impl Parser {
    fn bold_now(&self) -> bool {
        self.stack.last().is_some_and(|f| f.bold)
    }

    fn emph_now(&self) -> bool {
        self.stack.last().is_some_and(|f| f.emph)
    }

    fn text(&mut self, s: &str) {
        if self.hidden > 0 || s.is_empty() {
            return;
        }
        let (b, e) = (self.bold_now(), self.emph_now());
        match self.row.as_mut().and_then(|r| r.open.as_mut()) {
            Some(cell) if self.table_depth >= 1 => cell.cur.push(s, b, e),
            _ => {
                if self.table_depth >= 1 {
                    // Texto suelto dentro de <table> pero fuera de una celda: es una línea más.
                    self.cur.push(s, b, e);
                } else {
                    self.cur.push(s, b, e);
                }
            }
        }
    }

    /// Frontera de bloque: fuera de tablas cierra la línea; dentro de una celda parte el texto.
    fn block_break(&mut self) {
        if let Some(cell) = self.row.as_mut().and_then(|r| r.open.as_mut()) {
            cell.break_part();
            return;
        }
        self.flush_line();
    }

    fn flush_line(&mut self) {
        if !self.cur.is_empty() {
            let b = self.cur.take();
            self.lines.push(b.into_line(false));
        } else {
            self.cur = TextBuf::default();
        }
    }

    fn open_row(&mut self) {
        self.close_row();
        self.row = Some(Row::default());
    }

    fn close_cell(&mut self) {
        if let Some(row) = self.row.as_mut()
            && let Some(cell) = row.open.take()
        {
            row.cells.push(cell.finish());
        }
    }

    fn open_cell(&mut self) {
        if self.row.is_none() {
            self.row = Some(Row::default());
        }
        self.close_cell();
        if let Some(row) = self.row.as_mut() {
            row.open = Some(Cell::default());
        }
    }

    fn close_row(&mut self) {
        self.close_cell();
        if let Some(row) = self.row.take() {
            self.emit_row(row);
        }
    }

    fn emit_row(&mut self, row: Row) {
        // Celdas con contenido, cada una como lista de trozos.
        let cells: Vec<Vec<TextBuf>> = row
            .cells
            .into_iter()
            .filter(|parts| parts.iter().any(|p| !p.is_empty()))
            .collect();
        if cells.is_empty() {
            return;
        }
        // Una sola celda: es maquetación, no una tabla. Un párrafo por trozo.
        if cells.len() == 1 {
            for part in cells.into_iter().flatten() {
                if !part.is_empty() {
                    self.lines.push(part.into_line(false));
                }
            }
            return;
        }
        // Cada celda se aplana en una sola cadena.
        let mut flat: Vec<(String, bool, bool)> = Vec::with_capacity(cells.len());
        for parts in cells {
            let mut text = String::new();
            let (mut chars, mut bold, mut emph) = (0usize, 0usize, 0usize);
            for p in parts {
                if p.is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&p.text);
                chars += p.chars;
                bold += p.bold;
                emph += p.emph;
            }
            flat.push((text, chars > 0 && bold == chars, chars > 0 && emph == chars));
        }
        let merged = merge_financial_cells(flat);
        let all_bold = merged.iter().all(|c| c.1);
        let all_emph = merged.iter().all(|c| c.2);
        // Lista con viñeta o numeración en tabla: «• | texto» => «• texto».
        let bullet_list = merged.len() == 2 && is_marker(&merged[0].0);
        let sep = if bullet_list { " " } else { " | " };
        let text = merged
            .into_iter()
            .map(|c| c.0)
            .collect::<Vec<_>>()
            .join(sep);
        self.lines.push(Line {
            text,
            bold: all_bold,
            emph: all_emph,
            table: !bullet_list,
        });
    }

    fn start(&mut self, name: &str, style: Option<&str>, self_closing: bool) {
        if SKIP_SUBTREE.contains(&name) && !self_closing {
            self.push_frame(name, style, true);
            return;
        }
        match name {
            "table" => {
                if self.table_depth == 0 {
                    self.flush_line();
                }
                self.table_depth += 1;
            }
            "tr" if self.table_depth == 1 => self.open_row(),
            "td" | "th" if self.table_depth == 1 => self.open_cell(),
            _ => {}
        }
        if BLOCK.contains(&name) || (self.table_depth == 0 && matches!(name, "td" | "th" | "tr")) {
            self.block_break();
        }
        if VOID.contains(&name) || self_closing {
            return;
        }
        if AUTO_CLOSE.contains(&name)
            && self.stack.last().is_some_and(|f| f.name == name)
        {
            self.pop_frame();
        }
        self.push_frame(name, style, false);
    }

    fn push_frame(&mut self, name: &str, style: Option<&str>, skip: bool) {
        if self.stack.len() >= MAX_DEPTH {
            return;
        }
        let parent_bold = self.bold_now();
        let parent_emph = self.emph_now();
        let st = style.map(parse_style).unwrap_or_default();
        let mut bold = match st.bold {
            Some(b) => b,
            None => parent_bold,
        };
        if matches!(name, "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
            bold = true;
        }
        let mut emph = parent_emph;
        if matches!(name, "i" | "em" | "u" | "cite") || st.emph {
            emph = true;
        }
        let hidden = skip || (st.hidden && !matches!(name, "body" | "html"));
        if hidden {
            self.hidden += 1;
        }
        self.stack.push(Frame {
            name: name.to_string(),
            bold,
            emph,
            hidden,
        });
        match self.open.get_mut(name) {
            Some(n) => *n += 1,
            None => {
                self.open.insert(name.to_string(), 1);
            }
        }
    }

    /// Saca el último marco de la pila manteniendo `open` al día.
    fn pop_frame(&mut self) -> Option<Frame> {
        let f = self.stack.pop()?;
        if let Some(n) = self.open.get_mut(f.name.as_str()) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.open.remove(f.name.as_str());
            }
        }
        Some(f)
    }

    fn end(&mut self, name: &str) {
        // Cierre implícito de lo que quedó abierto dentro (HTML sin cerrar). Un cierre sin
        // apertura (`open` no lo conoce) no recorre la pila.
        if self.open.contains_key(name)
            && let Some(pos) = self.stack.iter().rposition(|f| f.name == name)
        {
            while self.stack.len() > pos {
                if let Some(f) = self.pop_frame()
                    && f.hidden
                {
                    self.hidden = self.hidden.saturating_sub(1);
                }
            }
        }
        match name {
            "td" | "th" if self.table_depth == 1 => self.close_cell(),
            "tr" if self.table_depth == 1 => self.close_row(),
            "table" => {
                if self.table_depth == 1 {
                    self.close_row();
                }
                self.table_depth = self.table_depth.saturating_sub(1);
            }
            _ => {}
        }
        if BLOCK.contains(&name) {
            self.block_break();
        }
    }

    fn run(&mut self, html: &str) {
        let b = html.as_bytes();
        let mut i = 0;
        let mut text_start = 0;
        while i < b.len() {
            if b[i] != b'<' {
                i += 1;
                continue;
            }
            let next = b.get(i + 1).copied().unwrap_or(0);
            let is_tag = next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?');
            if !is_tag {
                i += 1;
                continue;
            }
            self.emit_text(&html[text_start..i]);
            // Comentarios, DOCTYPE, instrucciones de proceso.
            if html[i..].starts_with("<!--") {
                i = html[i + 4..].find("-->").map_or(b.len(), |e| i + 4 + e + 3);
                text_start = i;
                continue;
            }
            if next == b'!' || next == b'?' {
                i = html[i..].find('>').map_or(b.len(), |e| i + e + 1);
                text_start = i;
                continue;
            }
            let Some(end) = find_tag_end(b, i) else {
                text_start = i;
                break;
            };
            let inner = &html[i + 1..end];
            i = end + 1;
            text_start = i;
            let closing = inner.starts_with('/');
            let body = inner.trim_start_matches('/');
            let self_closing = inner.ends_with('/');
            let name_end = body
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .unwrap_or(body.len());
            let name = body[..name_end].to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            if closing {
                self.end(&name);
                continue;
            }
            let style = attr_value(&body[name_end..], "style");
            if matches!(name.as_str(), "script" | "style") && !self_closing {
                // Texto crudo hasta el cierre. Se busca sin copiar el resto del documento: pasarlo a
                // minúsculas en cada <style> era O(n²) con miles de <style></style>.
                let skip = find_close_tag(&b[i..], name.as_bytes()).unwrap_or(html.len() - i);
                i += skip;
                text_start = i;
                continue;
            }
            self.start(&name, style, self_closing);
        }
        if text_start < html.len() {
            self.emit_text(&html[text_start..]);
        }
    }

    fn emit_text(&mut self, raw: &str) {
        if raw.is_empty() || self.hidden > 0 {
            return;
        }
        if raw.contains('&') {
            let decoded = decode_entities(raw);
            self.text(&decoded);
        } else {
            self.text(raw);
        }
    }

    fn finish(mut self) -> Vec<Line> {
        self.close_row();
        self.flush_line();
        self.lines
    }
}

/// Posición del `>` que cierra la etiqueta que empieza en `start`, respetando comillas.
fn find_tag_end(b: &[u8], start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'>' => return Some(i),
                _ => {}
            },
        }
        i += 1;
        // Una etiqueta de más de 16 KB no es una etiqueta: se descarta la comilla colgante.
        if i - start > 16_384 {
            return b[start..].iter().position(|&c| c == b'>').map(|p| start + p);
        }
    }
    None
}

/// Valor del atributo `name` en la cadena de atributos de una etiqueta.
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let at = from + pos;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric() && lower.as_bytes()[at - 1] != b'-';
        let after = lower[at + name.len()..].trim_start();
        if before_ok && after.starts_with('=') {
            let rest_start = attrs.len() - after.len() + 1;
            let rest = attrs[rest_start..].trim_start();
            let rest_off = attrs.len() - rest.len();
            let v = &attrs[rest_off..];
            return match v.as_bytes().first() {
                Some(&q @ (b'"' | b'\'')) => {
                    let inner = &v[1..];
                    inner.find(q as char).map(|e| &inner[..e])
                }
                Some(_) => {
                    let e = v.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(v.len());
                    Some(&v[..e])
                }
                None => None,
            };
        }
        from = at + name.len();
    }
    None
}

#[derive(Default)]
struct Style {
    bold: Option<bool>,
    emph: bool,
    hidden: bool,
}

fn parse_style(style: &str) -> Style {
    let mut st = Style::default();
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().to_ascii_lowercase();
        match k.as_str() {
            "font-weight" => {
                st.bold = match v.as_str() {
                    "bold" | "bolder" | "600" | "700" | "800" | "900" => Some(true),
                    "normal" | "lighter" | "400" | "300" | "100" | "200" | "500" => Some(false),
                    _ => None,
                }
            }
            "font-style" if v.starts_with("italic") || v.starts_with("oblique") => st.emph = true,
            "text-decoration" | "text-decoration-line" if v.contains("underline") => st.emph = true,
            "display" if v.starts_with("none") => st.hidden = true,
            _ => {}
        }
    }
    st
}

// ---------------------------------------------------------------------------------------------
// Celdas de tablas financieras
// ---------------------------------------------------------------------------------------------

/// `$`, `€`... suelto se pega a la celda siguiente; `)`, `%`, `)%` sueltos, a la anterior.
fn merge_financial_cells(cells: Vec<(String, bool, bool)>) -> Vec<(String, bool, bool)> {
    let mut out: Vec<(String, bool, bool)> = Vec::with_capacity(cells.len());
    let mut prefix: Option<String> = None;
    for (text, bold, emph) in cells {
        let t = text.trim();
        if t.is_empty() {
            continue;
        }
        if matches!(t, "$" | "€" | "£" | "US$" | "(" | "$(") {
            // Se acumula en sitio: reconstruir el `String` en cada celda era O(n²) con miles de «$».
            prefix.get_or_insert_with(String::new).push_str(t);
            continue;
        }
        if matches!(t, ")" | "%" | ")%" | "%)" | "*" | "†" | "‡") && let Some(last) = out.last_mut() {
            last.0.push_str(t);
            continue;
        }
        let text = match prefix.take() {
            Some(p) => format!("{p}{t}"),
            None => t.to_string(),
        };
        out.push((text, bold, emph));
    }
    if let Some(p) = prefix
        && !p.is_empty()
    {
        out.push((p, false, false));
    }
    out
}

/// Marcador de lista: viñetas, «(a)», «(1)», «1.», «a)».
fn is_marker(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if matches!(t, "•" | "·" | "●" | "○" | "▪" | "■" | "◦" | "-" | "–" | "—" | "*" | "§") {
        return true;
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() > 5 {
        return false;
    }
    let core = t.trim_start_matches('(').trim_end_matches([')', '.']);
    !core.is_empty()
        && (t.starts_with('(') || t.ends_with(')') || t.ends_with('.'))
        && core.chars().all(|c| c.is_ascii_alphanumeric())
        && core.len() <= 3
}

// ---------------------------------------------------------------------------------------------
// Entidades
// ---------------------------------------------------------------------------------------------

pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        // Una entidad mide como mucho ~10 caracteres hasta el `;`.
        let semi = tail.char_indices().take(12).find(|(_, c)| *c == ';').map(|(i, _)| i);
        match semi.and_then(|e| decode_one(&tail[1..e]).map(|c| (c, e))) {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_one(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).filter(|c| *c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ndash" => '–',
        "mdash" => '—',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "hellip" => '…',
        "trade" => '™',
        "reg" => '®',
        "copy" => '©',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "sect" => '§',
        "para" => '¶',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "eacute" => 'é',
        "egrave" => 'è',
        "aacute" => 'á',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "ccedil" => 'ç',
        "shy" => '\u{ad}',
        "ensp" | "emsp" | "thinsp" => ' ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(html: &str) -> Vec<String> {
        html_to_lines(html).into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn paragraphs_entities_and_whitespace() {
        let h = "<html><head><title>x</title><style>p{}</style></head><body>\
                 <p>Hello&nbsp;&nbsp;<b>World</b> &amp; co.</p>\n<div>Dos\n  l&#237;neas&#x2019;</div>a<br>b</body></html>";
        assert_eq!(texts(h), vec!["Hello World & co.", "Dos líneas’", "a", "b"]);
    }

    #[test]
    fn bold_and_emphasis_are_tracked_per_line() {
        let h = "<div><p><font style=\"font-weight:bold\">RISK FACTORS</font></p>\
                 <p><b>Risks</b> in prose</p><p><i>Lock-up</i></p>\
                 <p style=\"font-weight:700\"><span style=\"font-weight:normal\">plain</span></p></div>";
        let l = html_to_lines(h);
        assert_eq!(l.len(), 4);
        assert!(l[0].bold && l[0].emph);
        assert!(!l[1].bold && !l[1].emph, "negrita parcial no es título");
        assert!(!l[2].bold && l[2].emph);
        assert!(!l[3].bold, "font-weight:normal anula la negrita heredada");
    }

    #[test]
    fn hidden_content_and_ixbrl_header_are_dropped() {
        let h = "<body><div style=\"display:none\"><ix:header>secret 123</ix:header></div>\
                 <ix:header>otro</ix:header><p>visible</p><span style=\"display: none;\">oculto</span>\
                 <script>var a = '<p>no</p>';</script><!-- <p>comentario</p> --></body>";
        assert_eq!(texts(h), vec!["visible"]);
    }

    #[test]
    fn sgml_wrapper_is_removed() {
        let h = "<DOCUMENT>\n<TYPE>424B4\n<SEQUENCE>1\n<TEXT>\n<html><body><p>Prospectus</p></body></html>";
        assert_eq!(texts(h), vec!["Prospectus"]);
    }

    #[test]
    fn table_rows_become_lines_and_financial_cells_are_rejoined() {
        let h = "<table><tr><td>Net loss</td><td>$</td><td>(1,234</td><td>)</td><td></td><td>$</td><td>(987</td><td>)</td></tr>\
                 <tr><td>Growth</td><td>12.5</td><td>%</td></tr>\
                 <tr><td>&#8226;</td><td><p>Primera viñeta</p><p>continúa</p></td></tr></table><p>fin</p>";
        let l = html_to_lines(h);
        assert_eq!(l[0].text, "Net loss | $(1,234) | $(987)");
        assert!(l[0].table);
        assert_eq!(l[1].text, "Growth | 12.5%");
        assert_eq!(l[2].text, "• Primera viñeta continúa");
        assert!(!l[2].table);
        assert_eq!(l[3].text, "fin");
    }

    #[test]
    fn single_cell_tables_keep_their_paragraphs() {
        let h = "<table><tr><td><p>Uno</p><p>Dos</p></td></tr></table>";
        assert_eq!(texts(h), vec!["Uno", "Dos"]);
    }

    #[test]
    fn nested_tables_do_not_break_the_outer_row() {
        let h = "<table><tr><td>A</td><td><table><tr><td>x</td><td>y</td></tr></table></td></tr></table>";
        let t = texts(h);
        assert_eq!(t.len(), 1, "{t:?}");
        assert!(t[0].starts_with("A | "), "{t:?}");
    }

    #[test]
    fn unclosed_html_and_stray_angle_brackets_survive() {
        let h = "<p>a < b and c<p>segunda<li>uno<li>dos<td>x<td>y";
        let t = texts(h);
        assert!(t.iter().any(|l| l == "a < b and c"), "{t:?}");
        assert!(t.contains(&"segunda".to_string()));
        assert!(t.contains(&"uno".to_string()) && t.contains(&"dos".to_string()));
    }

    #[test]
    fn entity_decoder_is_lenient() {
        assert_eq!(decode_entities("a &amp b &unknown; &#65;&#x42; &lt;"), "a &amp b &unknown; AB <");
        assert_eq!(decode_entities("sin entidades"), "sin entidades");
    }

    #[test]
    fn attribute_parsing_handles_quotes() {
        assert_eq!(attr_value(" class=\"a\" style='x: y; z' ", "style"), Some("x: y; z"));
        assert_eq!(attr_value(" data-style=\"no\" style=bold ", "style"), Some("bold"));
        assert_eq!(attr_value(" class=\"a\"", "style"), None);
    }

    // ---- Equivalencia con el código anterior a B1 ------------------------------------------------
    //
    // Los arreglos de coste cuadrático no deben cambiar ningún resultado: se comparan con una copia
    // de la versión original sobre entradas aleatorias deterministas y acotadas.

    /// Generador pseudoaleatorio determinista (xorshift64).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn soup(&mut self, items: &[&str], pieces: usize) -> String {
            (0..pieces).map(|_| items[(self.next() % items.len() as u64) as usize]).collect()
        }
    }

    #[test]
    fn style_and_script_end_at_the_first_closer_in_any_case() {
        assert_eq!(texts("<p>a</p><STYLE>x{}</Style><p>b</p>"), vec!["a", "b"]);
        assert_eq!(texts("<p>a</p><script>if (a<b) {}</SCRIPT ><p>b</p>"), vec!["a", "b"]);
        assert_eq!(texts("<p>a</p><style>p{} <p>nunca</p>"), vec!["a"], "sin cierre se descarta el resto");
        assert_eq!(texts("<p>a</p><style type=\"text/css\">p{}</style\n><p>b</p>"), vec!["a", "b"]);
    }

    #[test]
    fn close_tag_search_matches_the_lowercase_copy_search() {
        let pieces = [
            "<", "/", ">", "style", "STYLE", "StYlE", "script", "SCRIPT", "</style", "</STYLE>", "</Style >", "</sty", "<style>", "é", "日", "a", " ", "</",
            "<//style", "</script", "</SCRIPT",
        ];
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for name in ["style", "script"] {
            for _ in 0..4_000 {
                let n = 1 + (rng.next() % 20) as usize;
                let h = rng.soup(&pieces, n);
                let want = h.to_ascii_lowercase().find(&format!("</{name}"));
                assert_eq!(find_close_tag(h.as_bytes(), name.as_bytes()), want, "{name}: {h:?}");
            }
        }
    }

    /// `open` debe coincidir siempre con el contenido de la pila: `end()` se fía de él para no
    /// recorrerla, así que un descuadre cambiaría qué cierres se aplican.
    #[test]
    fn open_counts_follow_the_stack_on_tag_soup() {
        let tags = [
            "<p>", "</p>", "<div>", "</div>", "<b>", "</b>", "<i>", "</i>", "<u>", "</u>", "<span>", "</span>", "<br>", "<table>", "</table>", "<tr>", "</tr>",
            "<td>", "</td>", "<th>", "</th>", "<li>", "</li>", "<h2>", "</h2>", "<style>", "</style>", "<script>", "<span style=\"display:none\">", "<ix:header>",
            "</ix:header>", "<head>", "</head>", "<title>", "</title>", "</x>", "<x>", "text", " ", "<!-- c -->", "<STYLE>", "</STYLE>", "<option>", "<dd>",
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..4_000 {
            let n = 1 + (rng.next() % 60) as usize;
            let h = rng.soup(&tags, n);
            let mut p = Parser::default();
            p.run(&h);
            let mut want: HashMap<String, usize> = HashMap::new();
            for f in &p.stack {
                *want.entry(f.name.clone()).or_default() += 1;
            }
            assert_eq!(p.open, want, "{h:?}");
        }
        // Pila llena: los marcos que no caben no se cuentan y un cierre huérfano no la recorre.
        let mut p = Parser::default();
        p.run(&format!("{}{}", "<i>".repeat(MAX_DEPTH + 50), "</u>".repeat(100)));
        assert_eq!(p.stack.len(), MAX_DEPTH);
        assert_eq!(p.open.get("i"), Some(&MAX_DEPTH));
        assert!(!p.open.contains_key("u"));
    }

    /// `merge_financial_cells` tal como era antes de B1 (reconstruía el prefijo en cada celda).
    fn merge_reference(cells: Vec<(String, bool, bool)>) -> Vec<(String, bool, bool)> {
        let mut out: Vec<(String, bool, bool)> = Vec::with_capacity(cells.len());
        let mut prefix: Option<String> = None;
        for (text, bold, emph) in cells {
            let t = text.trim();
            if t.is_empty() {
                continue;
            }
            if matches!(t, "$" | "€" | "£" | "US$" | "(" | "$(") {
                prefix = Some(format!("{}{}", prefix.unwrap_or_default(), t));
                continue;
            }
            if matches!(t, ")" | "%" | ")%" | "%)" | "*" | "†" | "‡") && let Some(last) = out.last_mut() {
                last.0.push_str(t);
                continue;
            }
            let text = match prefix.take() {
                Some(p) => format!("{p}{t}"),
                None => t.to_string(),
            };
            out.push((text, bold, emph));
        }
        if let Some(p) = prefix
            && !p.is_empty()
        {
            out.push((p, false, false));
        }
        out
    }

    #[test]
    fn financial_cells_merge_like_the_original() {
        let cells = ["$", "(", "$(", "US$", "€", "£", ")", "%", ")%", "%)", "*", "†", "‡", "1,234", "12.5", "x", "", "  ", "(5", "Revenue", " $ "];
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for _ in 0..4_000 {
            let n = 1 + (rng.next() % 14) as usize;
            let row: Vec<(String, bool, bool)> = (0..n)
                .map(|_| {
                    let r = rng.next();
                    (cells[(r % cells.len() as u64) as usize].to_string(), r & 0x100 != 0, r & 0x200 != 0)
                })
                .collect();
            assert_eq!(merge_financial_cells(row.clone()), merge_reference(row.clone()), "{row:?}");
        }
        let row = vec![("Label".to_string(), false, false), ("$".into(), false, false), ("$".into(), false, false), ("5".into(), true, false)];
        assert_eq!(merge_financial_cells(row)[1].0, "$$5");
    }
}
