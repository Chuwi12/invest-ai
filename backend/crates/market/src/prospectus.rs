//! Secciones clave de un folleto de OPV (S-1/F-1/424B4/424B1) en texto limpio.
//!
//! Sin dependencias de regex ni de parser HTML: el documento se pasa a líneas (`html`), se
//! descartan los elementos de paginación («Table of Contents», números de página) y se
//! localizan los títulos de sección por tres señales: coincidencia con el catálogo de títulos
//! estándar de un folleto (Regulation S-K), formato de título (todo en negrita/mayúsculas o
//! primera línea de página) y presencia en el índice del propio documento. Una sección acaba en
//! el siguiente título de sección. Lo que no se localiza se declara en `missing`; nunca se
//! inventa texto.

use domain::ProspectusSection;

use crate::html::{Line, html_to_lines};

/// Resultado de la extracción.
#[derive(Debug, Clone)]
pub struct Extracted {
    pub offer_price: Option<f64>,
    pub sections: Vec<ProspectusSection>,
    pub missing: Vec<String>,
    /// Caracteres del texto limpio del documento completo.
    pub document_chars: usize,
}

/// Claves que se intentan extraer, en el orden del documento.
pub const KEYS: &[&str] = &[
    "summary",
    "offering",
    "summary_financial_data",
    "risk_factors",
    "use_of_proceeds",
    "dividend_policy",
    "capitalization",
    "dilution",
    "principal_stockholders",
    "shares_eligible",
    "lock_up",
    "underwriting",
];

#[derive(Debug, Clone)]
struct L {
    text: String,
    norm: String,
    bold: bool,
    emph: bool,
    table: bool,
    page_start: bool,
}

/// Extrae las secciones. `max_chars` acota cada sección (0 = sin tope).
pub fn extract(html: &str, max_chars: usize) -> Extracted {
    let raw = html_to_lines(html);
    let lines = clean(raw);
    let document_chars = lines.iter().map(|l| l.text.chars().count() + 1).sum();
    let offer_price = offer_price(&lines);
    let (toc_titles, toc_end) = toc(&lines);
    let heads = Headings::new(&lines, &toc_titles, toc_end);

    let mut ranges: Vec<(&'static str, usize, usize, Option<&'static str>)> = Vec::new();
    for key in [
        "summary",
        "offering",
        "summary_financial_data",
        "risk_factors",
        "use_of_proceeds",
        "dividend_policy",
        "capitalization",
        "dilution",
        "principal_stockholders",
        "shares_eligible",
        "underwriting",
    ] {
        if let Some((s, e)) = heads.top_level(key) {
            ranges.push((key, s, e, None));
        }
    }
    let has = |ranges: &Vec<(&'static str, usize, usize, Option<&'static str>)>, k: &str| {
        ranges.iter().find(|r| r.0 == k).map(|r| (r.1, r.2))
    };
    // Subsecciones que en muchos folletos no son título de primer nivel.
    for (key, parent) in [("offering", "summary"), ("summary_financial_data", "summary")] {
        if has(&ranges, key).is_none()
            && let Some((ps, pe)) = has(&ranges, parent)
            && let Some((s, e)) = heads.sub_section(key, ps, pe)
        {
            ranges.push((key, s, e, Some(parent)));
        }
    }
    // Lock-up: primero un subtítulo propio en cualquier sección candidata; si no, los párrafos
    // que lo mencionan en la primera que los tenga.
    for with_heading in [true, false] {
        let mut found = false;
        for parent in ["underwriting", "shares_eligible", "offering", "summary"] {
            if let Some((ps, pe)) = has(&ranges, parent)
                && let Some((s, e)) = heads.lock_up(ps, pe, with_heading)
            {
                ranges.push(("lock_up", s, e, Some(parent)));
                found = true;
                break;
            }
        }
        if found {
            break;
        }
    }

    let mut sections = Vec::new();
    for key in KEYS {
        let Some(&(k, s, e, parent)) = ranges.iter().find(|r| r.0 == *key) else {
            continue;
        };
        let (title, body) = if k == "lock_up" && !is_lock_heading(&lines[s]) {
            ("Lock-up (párrafos que lo mencionan)".to_string(), &lines[s..e])
        } else {
            (lines[s].text.clone(), &lines[s + 1..e.max(s + 1)])
        };
        let text = body.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        let original_chars = text.chars().count();
        let (text, truncated) = if max_chars > 0 && original_chars > max_chars {
            (text.chars().take(max_chars).collect::<String>(), true)
        } else {
            (text, false)
        };
        if text.trim().is_empty() {
            continue;
        }
        sections.push(ProspectusSection {
            key: k.to_string(),
            title,
            text,
            parent: parent.map(str::to_string),
            truncated,
            original_chars,
        });
    }
    let missing = KEYS
        .iter()
        .filter(|k| !sections.iter().any(|s| s.key == **k))
        .map(|k| (*k).to_string())
        .collect();
    Extracted { offer_price, sections, missing, document_chars }
}

// ---------------------------------------------------------------------------------------------
// Limpieza
// ---------------------------------------------------------------------------------------------

/// Minúsculas, solo letras y dígitos separados por un espacio (guiones y comillas = espacio).
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut sep = true;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            sep = false;
        } else if ch == '\'' || ch == '’' {
            // «management's» -> «management s»: se trata como separador
            if !sep {
                out.push(' ');
                sep = true;
            }
        } else if !sep {
            out.push(' ');
            sep = true;
        }
    }
    out.trim_end().to_string()
}

fn is_page_number(t: &str) -> bool {
    let t = t.trim().trim_matches('-').trim();
    if t.is_empty() || t.len() > 6 {
        return false;
    }
    let all_digits = t.chars().all(|c| c.is_ascii_digit());
    let roman = t.chars().all(|c| matches!(c.to_ascii_lowercase(), 'i' | 'v' | 'x'));
    let f_page = t.strip_prefix(['F', 'f', 'A', 'a']).and_then(|r| r.strip_prefix('-')).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
    all_digits || roman || f_page
}

fn clean(raw: Vec<Line>) -> Vec<L> {
    let mut out = Vec::with_capacity(raw.len());
    let mut page_start = false;
    for l in raw {
        let t = l.text.trim();
        if t.is_empty() {
            continue;
        }
        let low = t.to_ascii_lowercase();
        if !l.table && (low == "table of contents" || low == "index to financial statements" || low == "back to contents" || low == "index") {
            page_start = true;
            continue;
        }
        if !l.table && is_page_number(t) {
            continue;
        }
        let norm = normalize(t);
        out.push(L {
            text: t.to_string(),
            norm,
            bold: l.bold,
            emph: l.emph,
            table: l.table,
            page_start,
        });
        page_start = false;
    }
    out
}

fn is_upper(s: &str) -> bool {
    s.chars().any(|c| c.is_alphabetic()) && !s.chars().any(|c| c.is_lowercase())
}

// ---------------------------------------------------------------------------------------------
// Índice (TOC)
// ---------------------------------------------------------------------------------------------

/// Títulos del índice del documento (texto exacto) y línea donde acaba.
fn toc(lines: &[L]) -> (Vec<String>, usize) {
    let mut rows: Vec<(usize, String)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.table {
            continue;
        }
        let cells: Vec<&str> = l.text.split(" | ").collect();
        if cells.len() < 2 {
            continue;
        }
        let last = cells[cells.len() - 1];
        let title = cells[0].trim();
        if is_page_number(last) && title.chars().filter(|c| c.is_alphabetic()).count() >= 4 && title.len() <= 160 {
            rows.push((i, title.to_string()));
        }
    }
    // Primer grupo de filas casi contiguas con >= 5 entradas.
    let mut start = 0;
    while start < rows.len() {
        let mut end = start;
        while end + 1 < rows.len() && rows[end + 1].0 - rows[end].0 <= 4 {
            end += 1;
        }
        if end - start + 1 >= 5 {
            let titles = rows[start..=end].iter().map(|r| r.1.clone()).collect();
            return (titles, rows[end].0);
        }
        start = end + 1;
    }
    (Vec::new(), 0)
}

// ---------------------------------------------------------------------------------------------
// Catálogo de títulos
// ---------------------------------------------------------------------------------------------

/// Clave de sección que corresponde a un título normalizado.
pub fn catalog_key(n: &str) -> Option<&'static str> {
    let words = n.split(' ').count();
    Some(match n {
        "prospectus summary" | "summary" | "our company" | "prospectus summary overview" => "summary",
        "the offering" | "summary of the offering" | "the offering summary" | "offering summary" | "the offering in brief" => "offering",
        "risk factors" => "risk_factors",
        "use of proceeds" => "use_of_proceeds",
        "dividend policy" | "dividends and dividend policy" | "dividends" => "dividend_policy",
        "capitalization" => "capitalization",
        "dilution" => "dilution",
        _ if (n.starts_with("summary") || n.starts_with("selected")) && n.contains("financial") && words <= 10 => "summary_financial_data",
        _ if (n.starts_with("principal") || n.starts_with("selling")) && (n.contains("stockholders") || n.contains("shareholders") || n.contains("securityholders") || n.contains("equityholders")) => "principal_stockholders",
        _ if n.starts_with("security ownership") && n.contains("beneficial") => "principal_stockholders",
        _ if n.starts_with("shares eligible") || n.starts_with("securities eligible") => "shares_eligible",
        _ if n == "underwriting" || n.starts_with("underwriting ") || n == "plan of distribution" => "underwriting",
        _ => return None,
    })
}

/// Títulos de primer nivel que no se extraen pero delimitan secciones.
pub fn is_boundary(n: &str) -> bool {
    const STARTS: &[&str] = &[
        "management s discussion", "business", "our business", "management", "executive compensation",
        "executive and director compensation", "director compensation", "certain relationships", "related party transactions",
        "description of capital stock", "description of securities", "description of share capital", "description of our capital stock",
        "legal matters", "experts", "where you can find", "index to financial", "cautionary note", "special note", "forward looking",
        "market and industry", "industry and market", "material u s federal", "material united states", "certain u s federal",
        "certain material", "taxation", "enforceability of civil", "expenses of the offering", "selected consolidated", "unaudited pro forma",
        "unaudited", "corporate history", "corporate structure", "our history", "glossary", "proposed business", "proposed acquisition",
        "comparison of", "conflicts of interest", "information about", "executive officers", "directors, executive",
        "limitation of liability", "transfer agent", "financial statements", "report of independent",
    ];
    // `n == s` o `n` empieza por `s` seguido de un espacio (sin construir una cadena por prefijo).
    STARTS.iter().any(|s| n.strip_prefix(s).is_some_and(|rest| rest.is_empty() || rest.starts_with(' ')))
}

struct Headings<'a> {
    lines: &'a [L],
    /// Posiciones de los títulos de primer nivel (terminadores), ordenadas.
    terms: Vec<usize>,
    /// Candidatos por clave: (posición, puntuación).
    cands: Vec<(usize, &'static str, u32)>,
}

impl<'a> Headings<'a> {
    fn new(lines: &'a [L], toc_titles: &[String], toc_end: usize) -> Self {
        let toc_exact: std::collections::HashSet<&str> = toc_titles.iter().map(String::as_str).collect();
        let mut terms = Vec::new();
        let mut cands = Vec::new();
        for (i, l) in lines.iter().enumerate().skip(toc_end) {
            if l.table || l.text.len() > 160 || l.text.ends_with('.') || l.text.ends_with(',') || l.text.ends_with(';') {
                continue;
            }
            let key = catalog_key(&l.norm);
            let boundary = is_boundary(&l.norm);
            // Un título en mayúsculas al comienzo de página es un cambio de sección aunque no
            // esté en el catálogo («UNAUDITED CONSOLIDATED PRO FORMA…», «NOTICE TO INVESTORS»).
            if key.is_none() && !boundary && l.page_start && is_upper(&l.text) && l.text.len() >= 6 {
                terms.push(i);
                continue;
            }
            if key.is_none() && !boundary {
                continue;
            }
            let exact_toc = toc_exact.contains(l.text.as_str());
            let upper = is_upper(&l.text);
            let style = l.bold || l.emph;
            // Un título de primer nivel: mayúsculas, primera línea de página o igual al índice.
            let top = upper || l.page_start || exact_toc;
            if !top && !(style && key.is_some()) {
                continue;
            }
            if top {
                terms.push(i);
            }
            if let Some(k) = key {
                let score = u32::from(exact_toc) * 3 + u32::from(l.page_start) * 2 + u32::from(upper) * 2 + u32::from(l.bold) + u32::from(top) * 2;
                cands.push((i, k, score));
            }
        }
        Self { lines, terms, cands }
    }

    /// Primer título de primer nivel posterior a `after` (o el final). `terms` está ordenado:
    /// la búsqueda binaria evita O(títulos) por candidato, que con miles de «DILUTION» en negrita
    /// era O(n²).
    fn next_term(&self, after: usize) -> usize {
        let k = self.terms.partition_point(|&t| t <= after);
        self.terms.get(k).copied().unwrap_or(self.lines.len())
    }

    fn chars_between(&self, a: usize, b: usize) -> usize {
        self.lines[a + 1..b.max(a + 1)].iter().map(|l| l.text.len()).sum()
    }

    /// Mejor título de primer nivel para `key`: máxima puntuación entre los que tienen cuerpo.
    fn top_level(&self, key: &str) -> Option<(usize, usize)> {
        let mut best: Option<(u32, usize, usize)> = None;
        for &(i, k, score) in &self.cands {
            if k != key {
                continue;
            }
            let end = self.next_term(i);
            // Los títulos de primer nivel exigen forma de título (no solo negrita).
            let top = self.terms.binary_search(&i).is_ok();
            if !top || self.chars_between(i, end) < 150 {
                continue;
            }
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, i, end));
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// Subtítulo (negrita/cursiva) dentro del rango del padre.
    fn sub_section(&self, key: &str, ps: usize, pe: usize) -> Option<(usize, usize)> {
        for &(i, k, _) in &self.cands {
            if k == key && i > ps && i < pe {
                let end = (i + 1..pe)
                    .find(|&j| self.is_heading_like(j) && self.lines[j].norm != self.lines[i].norm && self.is_sub_end(j))
                    .unwrap_or(pe);
                return Some((i, end));
            }
        }
        // Subtítulos que no son de primer nivel no entran en `cands` si no son negrita: búsqueda directa.
        for i in ps + 1..pe {
            let l = &self.lines[i];
            if !l.table && (l.bold || l.emph) && l.text.len() <= 120 && catalog_key(&l.norm) == Some(key) {
                let end = (i + 1..pe).find(|&j| self.is_heading_like(j) && self.is_sub_end(j)).unwrap_or(pe);
                return Some((i, end));
            }
        }
        None
    }

    fn is_heading_like(&self, j: usize) -> bool {
        let l = &self.lines[j];
        !l.table && (l.bold || l.emph) && l.text.len() <= 120 && !l.text.ends_with('.') && !l.text.ends_with(',')
    }

    /// Un subtítulo que cierra un bloque: uno de primer nivel o el catálogo (no cualquier negrita).
    fn is_sub_end(&self, j: usize) -> bool {
        let n = &self.lines[j].norm;
        catalog_key(n).is_some() || is_boundary(n) || self.terms.binary_search(&j).is_ok()
    }

    /// Subsección «Lock-up agreements» dentro del rango; si no hay título, los párrafos que lo mencionan.
    fn lock_up(&self, ps: usize, pe: usize, with_heading: bool) -> Option<(usize, usize)> {
        for i in (ps + 1..pe).filter(|_| with_heading) {
            let l = &self.lines[i];
            if is_lock_heading(l) && (l.bold || l.emph) {
                let mut end = pe;
                // Bytes acumulados de las líneas posteriores al título (suma corrida: recalcularla
                // en cada vuelta era O(12 000²) con miles de líneas de un carácter).
                let mut acc = 0usize;
                for (body, j) in (i + 1..pe).enumerate() {
                    acc += self.lines[j].text.len();
                    if self.is_heading_like(j) && !is_lock_heading(&self.lines[j]) && body >= 1 {
                        end = j;
                        break;
                    }
                    if acc > 12_000 {
                        end = j + 1;
                        break;
                    }
                }
                return Some((i, end));
            }
        }
        if with_heading {
            return None;
        }
        // Sin título: de la primera mención a unos pocos párrafos que la contengan.
        let mention = |l: &L| {
            let t = l.text.to_ascii_lowercase();
            t.contains("lock-up") || t.contains("lock up") || t.contains("lockup")
        };
        let first = (ps + 1..pe).find(|&i| mention(&self.lines[i]))?;
        let mut last = first;
        let mut count = 1;
        for j in first + 1..pe.min(first + 12) {
            if mention(&self.lines[j]) && count < 4 {
                last = j;
                count += 1;
            }
        }
        // Se marca con norm vacío en la primera línea para que el título sea genérico.
        Some((first, last + 1))
    }
}

fn is_lock_heading(l: &L) -> bool {
    !l.table && l.text.len() <= 90 && (l.norm.starts_with("lock up") || l.norm.starts_with("lockup") || l.norm.starts_with("market stand off"))
}

// ---------------------------------------------------------------------------------------------
// Portada: precio de la OPV
// ---------------------------------------------------------------------------------------------

fn offer_price(lines: &[L]) -> Option<f64> {
    let cover = &lines[..lines.len().min(220)];
    for (i, l) in cover.iter().enumerate() {
        let low = l.text.to_ascii_lowercase();
        let hit = low.contains("public offering price") || low.contains("offering price of") || low.contains("price to public");
        if !hit {
            continue;
        }
        for cand in cover[i..cover.len().min(i + 3)].iter() {
            if let Some(p) = first_dollar(&cand.text) {
                return Some(p);
            }
        }
    }
    None
}

/// Primer importe tras un `$` en el texto (0 < p < 100 000).
pub fn first_dollar(s: &str) -> Option<f64> {
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        let tail = rest[i + 1..].trim_start();
        let num: String = tail.chars().take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
        let num = num.trim_end_matches(['.', ',']).replace(',', "");
        if let Ok(v) = num.parse::<f64>()
            && v > 0.0
            && v < 100_000.0
        {
            return Some(v);
        }
        rest = &rest[i + 1..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_and_catalog() {
        assert_eq!(normalize("MANAGEMENT’S  DISCUSSION"), "management s discussion");
        assert_eq!(normalize("Lock-Up Agreements"), "lock up agreements");
        assert_eq!(catalog_key("summary consolidated financial and other data"), Some("summary_financial_data"));
        assert_eq!(catalog_key("principal and selling stockholders"), Some("principal_stockholders"));
        assert_eq!(catalog_key("underwriting conflicts of interest"), Some("underwriting"));
        assert_eq!(catalog_key("risk factors summary"), None);
        assert!(is_boundary("management s discussion and analysis of financial condition"));
    }

    #[test]
    fn dollar_parsing() {
        assert_eq!(first_dollar("is $14.00 per share"), Some(14.0));
        assert_eq!(first_dollar("Per Share | $ 1,234.50 | x"), Some(1234.5));
        assert_eq!(first_dollar("no price"), None);
    }

    #[test]
    fn synthetic_prospectus_sections() {
        let filler = "Lorem ipsum dolor sit amet ".repeat(20);
        let html = format!(
            "<html><body><p>The initial public offering price is $12.50 per share.</p>\
             <table><tr><td>PROSPECTUS SUMMARY</td><td>1</td></tr><tr><td>RISK FACTORS</td><td>5</td></tr>\
             <tr><td>USE OF PROCEEDS</td><td>9</td></tr><tr><td>DILUTION</td><td>10</td></tr>\
             <tr><td>UNDERWRITING</td><td>20</td></tr></table>\
             <p>Table of Contents</p><p><b>PROSPECTUS SUMMARY</b></p><p>{filler}</p><p><b>Use of proceeds</b></p><p>sub</p>\
             <p>Table of Contents</p><p><b>RISK FACTORS</b></p><p>{filler}</p>\
             <p>Table of Contents</p><p><b>USE OF PROCEEDS</b></p><p>We will receive net proceeds. {filler}</p>\
             <p>Table of Contents</p><p><b>DILUTION</b></p><p>Dilution text {filler}</p>\
             <p>Table of Contents</p><p><b>UNDERWRITING</b></p><p>x</p><p><i>Lock-Up Agreements</i></p><p>Our officers agree to a 180 day lock-up. {filler}</p>\
             <p><i>Stabilization</i></p><p>other</p><p>Table of Contents</p><p><b>LEGAL MATTERS</b></p></body></html>"
        );
        let e = extract(&html, 0);
        assert_eq!(e.offer_price, Some(12.5));
        let get = |k: &str| e.sections.iter().find(|s| s.key == k).map(|s| s.text.clone());
        assert!(get("risk_factors").unwrap().starts_with("Lorem"));
        assert!(get("use_of_proceeds").unwrap().starts_with("We will receive"), "el subtítulo del resumen no cuenta");
        assert!(get("summary").unwrap().contains("sub"));
        assert!(get("dilution").unwrap().starts_with("Dilution text"));
        let lock = get("lock_up").unwrap();
        assert!(lock.contains("180 day lock-up") && !lock.contains("other"), "{lock}");
        assert!(e.missing.contains(&"capitalization".to_string()));
    }

    fn line(text: &str) -> L {
        L { text: text.into(), norm: normalize(text), bold: false, emph: false, table: false, page_start: false }
    }

    /// `next_term` (búsqueda binaria) debe dar lo mismo que el recorrido lineal original.
    #[test]
    fn next_term_matches_a_linear_scan() {
        let lines: Vec<L> = (0..60).map(|i| line(&format!("l{i}"))).collect();
        for terms in [vec![], vec![0], vec![5, 6, 7], vec![3, 10, 59], (0..60).step_by(7).collect::<Vec<_>>()] {
            let h = Headings { lines: &lines, terms: terms.clone(), cands: Vec::new() };
            for after in 0..65 {
                let want = terms.iter().copied().find(|&t| t > after).unwrap_or(lines.len());
                assert_eq!(h.next_term(after), want, "terms={terms:?} after={after}");
            }
        }
    }

    #[test]
    fn boundary_titles_need_the_whole_word() {
        assert!(is_boundary("business"));
        assert!(is_boundary("business overview"));
        assert!(is_boundary("management s discussion and analysis"));
        assert!(!is_boundary("businesses"));
        assert!(!is_boundary("managementteam"));
        assert!(!is_boundary("our"));
        assert!(!is_boundary(""));
    }

    /// El lock-up bajo su título acaba, como mucho, cuando las líneas del cuerpo suman más de
    /// 12 000 bytes: con líneas de 100 bytes, la 121.ª es la última que entra.
    #[test]
    fn lockup_body_stops_after_twelve_thousand_bytes() {
        let filler = "Lorem ipsum dolor sit amet ".repeat(20);
        let body = format!("{}b", "a".repeat(99));
        let rows = format!("<p>{body}</p>").repeat(200);
        let html = format!(
            "<html><body><p>Table of Contents</p><p><b>UNDERWRITING</b></p><p>{filler}</p><p><i>Lock-Up Agreements</i></p>{rows}\
             <p>Table of Contents</p><p><b>LEGAL MATTERS</b></p><p>x</p></body></html>"
        );
        let e = extract(&html, 0);
        let lock = e.sections.iter().find(|s| s.key == "lock_up").expect("hay lock-up");
        assert_eq!(lock.text.lines().count(), 121, "{}", lock.text.len());
        assert!(lock.text.lines().all(|l| l == body));
    }
}
