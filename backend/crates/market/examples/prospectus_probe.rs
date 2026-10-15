//! Herramienta de desarrollo: `cargo run -p market --example prospectus_probe -- fichero.htm [clave]`
//! imprime las secciones que `prospectus::extract` localiza; con `clave` vuelca ese texto.
fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("ruta del HTML");
    let key = a.next();
    let html = std::fs::read_to_string(&path).expect("leer");
    let t = std::time::Instant::now();
    let e = market::prospectus::extract(&html, 0);
    if key.as_deref() == Some("--fixture") {
        let n = a.next().and_then(|x| x.parse().ok()).unwrap_or(25);
        print!("{}", fixture(&html, n));
        return;
    }
    if key.as_deref() == Some("--headings") {
        for (i, l) in market::html::html_to_lines(&html).iter().enumerate() {
            if !l.table && (l.bold || l.emph) && l.text.len() < 100 {
                println!("{i}: {}", l.text);
            }
        }
        return;
    }
    if let Some(k) = key {
        for s in e.sections.iter().filter(|s| s.key == k) {
            println!("{}\n{}", s.title, s.text);
        }
        return;
    }
    println!("{} ms, doc {} chars, precio {:?}", t.elapsed().as_millis(), e.document_chars, e.offer_price);
    for s in &e.sections {
        let head: String = s.text.chars().take(110).collect::<String>().replace('\n', " / ");
        println!("  {:24} {:>8} chars  parent={:?}  «{}» {}", s.key, s.original_chars, s.parent, s.title, head);
    }
    println!("  FALTAN: {:?}", e.missing);
}

/// Genera un fixture recortado (HTML sencillo) a partir de las líneas reales: portada, índice,
/// títulos, y las primeras líneas de cada sección. `prospectus_probe doc.htm --fixture 30 > out.html`
pub fn fixture(html: &str, body: usize) -> String {
    use market::prospectus::{catalog_key, is_boundary, normalize};
    let ls = market::html::html_to_lines(html);
    let mut keep = vec![false; ls.len()];
    let mut left = 0usize;
    for (i, l) in ls.iter().enumerate() {
        let n = normalize(&l.text);
        let heading = !l.table && (l.bold || l.emph) && l.text.len() < 100;
        let toc = l.table && l.text.rsplit(" | ").next().is_some_and(|p| p.len() <= 4 && p.chars().any(|c| c.is_ascii_digit()) && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        let anchor = !l.table && (catalog_key(&n).is_some() || is_boundary(&n) || n.starts_with("lock up"));
        if anchor && l.text.len() < 130 {
            left = if n.starts_with("lock up") { body * 2 } else { body };
        }
        if i < 120 || heading || toc || left > 0 {
            keep[i] = true;
            if left > 0 && !heading {
                left -= 1;
            }
        }
    }
    let mut out = String::from("<html><body>\n");
    for (i, l) in ls.iter().enumerate() {
        let nav = l.text.eq_ignore_ascii_case("table of contents");
        if !(keep[i] || (nav && keep.get(i + 1).copied().unwrap_or(false))) {
            continue;
        }
        let esc = l.text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        if l.table {
            out.push_str("<table><tr>");
            for c in esc.split(" | ") {
                out.push_str(&format!("<td>{c}</td>"));
            }
            out.push_str("</tr></table>\n");
        } else if l.bold {
            out.push_str(&format!("<p><b>{esc}</b></p>\n"));
        } else if l.emph {
            out.push_str(&format!("<p><i>{esc}</i></p>\n"));
        } else {
            out.push_str(&format!("<p>{esc}</p>\n"));
        }
    }
    out.push_str("</body></html>\n");
    out
}
