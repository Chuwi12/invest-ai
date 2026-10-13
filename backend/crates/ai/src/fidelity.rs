//! Anti-alucinación numérica: toda cifra que cite el modelo debe existir en el contexto que se le
//! dio. Se indexan los números del contexto (con el tramo del que salen: cotización, indicadores,
//! fundamentales, puntuaciones, macro o una noticia concreta) y se comprueban las cifras de la
//! salida contra ese índice.
//!
//! Tolerancia: dos cifras coinciden si difieren menos que media unidad del decimal menos preciso
//! (el contexto muestra `12.3%`; el modelo puede escribir `12.3%` o `12%`, no `13%`). Cifras
//! con sufijo (`3.21T`, `450M`) se comparan ya escaladas. Los porcentajes se comparan como el
//! número escrito (12.3), porque así los presenta el contexto.
//!
//! # Formatos de número que se entienden
//! El contexto usa siempre el formato inglés (`81059.79`), pero el modelo (y el usuario en el chat)
//! escriben a menudo en castellano, así que una cifra se lee en cualquiera de los dos:
//!
//! | Texto | Lectura |
//! |---|---|
//! | `81059.79`, `1,234.5`, `1,234,567` | inglés (la coma agrupa de 3 en 3, el punto es decimal) |
//! | `81.059,79`, `1.234.567`, `2057,45`, `-3,2 %` | español (el punto agrupa, la coma es decimal) |
//! | `81 059,79` (con espacio fino o no separable) | español |
//! | `0.123`, `1234.567`, `12.5` | decimal inglés (un solo separador que no agrupa de 3 en 3) |
//!
//! **Ambiguas**: un único separador seguido de exactamente 3 dígitos y precedido de 1-3 dígitos
//! (`1.234`, `1,234`): pueden ser `1,234` (decimal) o `1234` (miles). Regla: (1) si el MISMO texto
//! trae una cifra inequívoca (`81.059,79` o `2057,45` → español; `1,234.5` → inglés) todas las
//! ambiguas de ese texto se leen en ese formato; (2) si no, `value` es la lectura inglesa (la del
//! contexto) y la española queda en `alt`: la cifra se da por verificada si CUALQUIERA de las dos
//! lecturas está en los datos.
//!
//! Sufijos de escala: `K/M/B/T` pegados o separados por un espacio, y palabras (`mil`, `millones`,
//! `mil millones`, `billones` = 10^12 en español, `billion` = 10^9 en inglés…). El porcentaje puede
//! ir pegado o con espacio (`5%`, `5 %`, `5 por ciento`).
//!
//! # Signo
//! Una cifra con signo explícito (`-3.2`, `−3,2`, `+3.2`) debe coincidir también en el signo. Una
//! cifra SIN signo («cayó un 3.2%», «caída máxima del 13.8%») coincide con un dato negativo solo si
//! el texto cercano expresa una bajada (caída, pérdida, por debajo…): así no se descarta una cifra
//! correcta por estar escrita en palabras, y «sube un 3.2%» no se da por buena si el dato es -3.2%.
//!
//! Exentos (no son datos): enteros pequeños sin unidad (≤ 12), años, números pegados a letras
//! (`SMA200`, `RSI14`, `52w`, `N3`), fechas/horas (`2026-10-12`, `12/10/2026`, `14:03`) y los
//! nombres de índices (`S&P 500`, `Nasdaq 100`…). Un rango de cifras (`81000-82000`) o un par
//! (`81059.79/84014.13`) NO es una fecha: cada extremo se comprueba.

use std::ops::Range;

/// De qué parte del contexto sale una cifra.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    Header,
    Quote,
    Indicators,
    Fundamentals,
    Scores,
    /// Entrada, stop y objetivos calculados en Rust a partir de los datos.
    Levels,
    Macro,
    /// Noticia con su etiqueta (`N3`).
    News(String),
    Gaps,
    /// Tramo sin clasificar (p. ej. resultados de herramientas en el chat).
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Num {
    pub raw: String,
    /// Valor ya escalado por el sufijo K/M/B/T (o `mil`, `millones`…).
    pub value: f64,
    /// Decimales escritos en la mantisa.
    pub decimals: u32,
    pub scale: f64,
    /// Posición en el texto (bytes) del token: signo, dígitos y sufijo de escala.
    pub span: Range<usize>,
    /// Posición (bytes) tras la cifra y su `%` (con o sin espacio): ahí se pone la marca ⚠.
    pub mark_end: usize,
    /// `true` si es un entero pequeño, un año o un fragmento de fecha/código: no es una cifra
    /// «de datos» y no se exige que esté en el contexto.
    pub exempt: bool,
    /// Otra lectura posible si el formato es ambiguo (`1.234`: 1,234 o 1234), ya escalada y con
    /// el mismo signo: `(valor, decimales)`.
    pub alt: Option<(f64, u32)>,
    /// El texto la escribió con `+` o `-`/`−`.
    pub signed: bool,
    /// Sin signo, pero el texto cercano habla de una bajada: puede ser un dato negativo en palabras.
    pub neg_ok: bool,
}

impl Num {
    fn tol(decimals: u32, scale: f64) -> f64 {
        0.5 * 10f64.powi(-(decimals as i32)) * scale
    }

    fn tolerance(&self) -> f64 {
        Self::tol(self.decimals, self.scale)
    }

    /// Todas las lecturas posibles: `(valor, tolerancia)`.
    fn readings(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        std::iter::once((self.value, self.tolerance()))
            .chain(self.alt.map(|(v, d)| (v, Self::tol(d, self.scale))))
    }

    /// Valores posibles de la cifra (la principal y, si el formato es ambiguo, la alternativa).
    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.readings().map(|(v, _)| v)
    }
}

// ---------------------------------------------------------------------------------------------
// Lectura de la parte numérica (separadores de millares y decimales)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Locale {
    Es,
    En,
}

/// Espacios que se usan como separador de millares (no separable, fino, fino no separable).
fn is_group_space(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{2009}' | '\u{202f}')
}

fn is_space(c: char) -> bool {
    c == ' ' || is_group_space(c)
}

struct Group {
    sep: char,
    digits: String,
    /// Índice (en chars) tras los dígitos del grupo.
    end: usize,
}

/// Lectura de la parte numérica.
struct Body {
    /// Índice (en chars) tras la parte numérica.
    end: usize,
    value: f64,
    decimals: u32,
    /// Lectura española de una cifra ambigua (`1.234` → 1234).
    other: Option<(f64, u32)>,
    /// Formato que delata este número, si es inequívoco.
    hint: Option<Locale>,
    /// Solo dígitos, sin separadores.
    plain: bool,
}

fn num_of(int: &str, frac: &str) -> f64 {
    let s = if frac.is_empty() { int.to_string() } else { format!("{int}.{frac}") };
    s.parse().unwrap_or(f64::NAN)
}

/// Interpreta `int` + los primeros grupos; `None` si no forman un número válido.
fn interpret(int: &str, groups: &[Group], end: usize) -> Option<Body> {
    let n = int.len();
    // Los grupos de millares exigen 1-3 dígitos delante y sin cero inicial.
    let thousands_ok = (1..=3).contains(&n) && !int.starts_with('0');
    let body = |value: f64, decimals: u32, other: Option<(f64, u32)>, hint: Option<Locale>| Body {
        end,
        value,
        decimals,
        other,
        hint,
        plain: groups.is_empty(),
    };
    let dotlike = |c: char| c == '.' || is_group_space(c);
    match groups {
        [] => Some(body(num_of(int, ""), 0, None, None)),
        [g] => {
            let d = g.digits.len() as u32;
            match g.sep {
                '.' if g.digits.len() == 3 && thousands_ok => {
                    // 1.234: decimal inglés (principal) o miles españoles (alternativa).
                    Some(body(num_of(int, &g.digits), 3, Some((num_of(&format!("{int}{}", g.digits), ""), 0)), None))
                }
                '.' => Some(body(num_of(int, &g.digits), d, None, None)),
                ',' if g.digits.len() == 3 && thousands_ok => {
                    // 1,234: miles ingleses (principal) o decimal español (alternativa).
                    Some(body(num_of(&format!("{int}{}", g.digits), ""), 0, Some((num_of(int, &g.digits), 3)), None))
                }
                ',' => Some(body(num_of(int, &g.digits), d, None, Some(Locale::Es))),
                _ if g.digits.len() == 3 && thousands_ok => Some(body(num_of(&format!("{int}{}", g.digits), ""), 0, None, None)),
                _ => None,
            }
        }
        _ => {
            if !thousands_ok {
                return None;
            }
            let (last, head) = groups.split_last()?;
            if head.iter().any(|g| g.digits.len() != 3) {
                return None;
            }
            let mut whole = int.to_string();
            head.iter().for_each(|g| whole.push_str(&g.digits));
            let class_dot = dotlike(head[0].sep) && head.iter().all(|g| dotlike(g.sep));
            let class_comma = head.iter().all(|g| g.sep == ',');
            if class_dot {
                match last.sep {
                    c if dotlike(c) && last.digits.len() == 3 => {
                        whole.push_str(&last.digits);
                        let hint = (head[0].sep == '.').then_some(Locale::Es);
                        Some(body(num_of(&whole, ""), 0, None, hint))
                    }
                    ',' => Some(body(num_of(&whole, &last.digits), last.digits.len() as u32, None, Some(Locale::Es))),
                    _ => None,
                }
            } else if class_comma {
                match last.sep {
                    ',' if last.digits.len() == 3 => {
                        whole.push_str(&last.digits);
                        Some(body(num_of(&whole, ""), 0, None, Some(Locale::En)))
                    }
                    '.' => Some(body(num_of(&whole, &last.digits), last.digits.len() as u32, None, Some(Locale::En))),
                    _ => None,
                }
            } else {
                None
            }
        }
    }
}

/// Lee la parte numérica que empieza en el dígito `i`.
fn parse_body(chars: &[(usize, char)], i: usize) -> Body {
    let digit = |k: usize| chars.get(k).is_some_and(|(_, c)| c.is_ascii_digit());
    let mut j = i;
    while digit(j) {
        j += 1;
    }
    let int: String = chars[i..j].iter().map(|(_, c)| *c).collect();
    let mut groups: Vec<Group> = Vec::new();
    let mut k = j;
    while let Some(&(_, sep)) = chars.get(k) {
        if !(sep == '.' || sep == ',' || is_group_space(sep)) {
            break;
        }
        let mut e = k + 1;
        while digit(e) {
            e += 1;
        }
        if e == k + 1 {
            break;
        }
        groups.push(Group { sep, digits: chars[k + 1..e].iter().map(|(_, c)| *c).collect(), end: e });
        k = e;
    }
    // «1,2,3»: una lista pegada, no un decimal (ni el primer elemento ni los siguientes).
    let first_comma_decimal = groups.first().is_some_and(|g| g.sep == ',' && g.digits.len() != 3);
    let after_comma = i >= 2 && chars[i - 1].1 == ',' && chars[i - 2].1.is_ascii_digit();
    let list = first_comma_decimal && (after_comma || groups.get(1).is_some_and(|g| g.sep == ','));
    let max = if list { 0 } else { groups.len() };
    for take in (0..=max).rev() {
        let end = if take == 0 { j } else { groups[take - 1].end };
        if let Some(b) = interpret(&int, &groups[..take], end) {
            return b;
        }
    }
    Body { end: j, value: num_of(&int, ""), decimals: 0, other: None, hint: None, plain: true }
}

// ---------------------------------------------------------------------------------------------
// Sufijos, exenciones y pistas del texto cercano
// ---------------------------------------------------------------------------------------------

/// Palabra de escala que empieza en `k`: (factor, longitud en chars). Más largas primero.
fn scale_word(chars: &[(usize, char)], k: usize) -> Option<(f64, usize)> {
    const WORDS: &[(&str, f64)] = &[
        ("mil millones", 1e9),
        ("millones", 1e6),
        ("millón", 1e6),
        ("millon", 1e6),
        ("billones", 1e12),
        ("billón", 1e12),
        ("billon", 1e12),
        ("millardos", 1e9),
        ("millardo", 1e9),
        ("mil", 1e3),
        ("millions", 1e6),
        ("million", 1e6),
        ("billions", 1e9),
        ("billion", 1e9),
        ("trillions", 1e12),
        ("trillion", 1e12),
        ("thousand", 1e3),
        ("bn", 1e9),
        ("mn", 1e6),
    ];
    for (w, f) in WORDS {
        let n = w.chars().count();
        let matches = w.chars().enumerate().all(|(o, c)| chars.get(k + o).is_some_and(|(_, x)| x.to_lowercase().next() == Some(c)));
        if matches && !chars.get(k + n).is_some_and(|(_, x)| x.is_alphanumeric()) {
            return Some((*f, n));
        }
    }
    None
}

/// Cuántas cifras seguidas hay a un lado del separador de una fecha (`-`, `/`): `(dígitos, es_entero)`.
fn digit_run(chars: &[(usize, char)], from: usize, forward: bool) -> (usize, bool) {
    let digit = |k: Option<usize>| k.and_then(|k| chars.get(k)).is_some_and(|(_, c)| c.is_ascii_digit());
    let mut n = 0;
    let mut pos = from;
    loop {
        let here = if forward { Some(pos) } else { pos.checked_sub(1) };
        if !digit(here) {
            break;
        }
        n += 1;
        if forward {
            pos += 1;
        } else {
            pos -= 1;
        }
    }
    // ¿Es entero? No debe tener un decimal pegado al otro lado.
    let beyond = if forward { chars.get(pos).map(|(_, c)| *c) } else { pos.checked_sub(1).and_then(|p| chars.get(p)).map(|(_, c)| *c) };
    let beyond2 = if forward { chars.get(pos + 1).map(|(_, c)| *c) } else { pos.checked_sub(2).and_then(|p| chars.get(p)).map(|(_, c)| *c) };
    let decimal = matches!(beyond, Some('.' | ',')) && beyond2.is_some_and(|c| c.is_ascii_digit());
    (n, !decimal)
}

/// ¿Es parte de una fecha (`2026-10-12`, `12-10-2026`, `2026-10`) o una hora (`14:03`)?
fn is_date_or_time(chars: &[(usize, char)], i: usize, end: usize, body: &Body) -> bool {
    let at = |k: Option<usize>| k.and_then(|k| chars.get(k)).map(|(_, c)| *c);
    let before = |n: usize| at(i.checked_sub(n));
    let after = |n: usize| at(Some(end + n));
    let is_digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    // Hora.
    if (before(1) == Some(':') && is_digit(before(2))) || (after(0) == Some(':') && is_digit(after(1))) {
        return true;
    }
    let len = end - i;
    let short = body.plain && len <= 4;
    // Fecha con barras: solo si todos los componentes son enteros cortos (50/200, 12/10/2026).
    if (before(1) == Some('/') && is_digit(before(2))) || (after(0) == Some('/') && is_digit(after(1))) {
        let left_ok = before(1) != Some('/') || {
            let (n, int) = digit_run(chars, i - 1, false);
            n <= 4 && int
        };
        let right_ok = after(0) != Some('/') || {
            let (n, int) = digit_run(chars, end + 1, true);
            n <= 4 && int
        };
        return short && left_ok && right_ok;
    }
    // Fecha con guiones: año-mes[-día] o día-mes-año; cualquier otro par de cifras es un rango.
    let hy_left = before(1) == Some('-') && is_digit(before(2));
    let hy_right = after(0) == Some('-') && is_digit(after(1));
    if hy_left || hy_right {
        if !short {
            return false;
        }
        let mut lens = vec![len];
        let (mut p, mut q) = (i, end);
        // A la izquierda.
        while p >= 2 && chars[p - 1].1 == '-' && chars[p - 2].1.is_ascii_digit() && lens.len() < 3 {
            let (n, int) = digit_run(chars, p - 1, false);
            if !int || n > 4 {
                return false;
            }
            lens.insert(0, n);
            p -= 1 + n;
        }
        // A la derecha.
        while chars.get(q).is_some_and(|(_, c)| *c == '-') && is_digit(at(Some(q + 1))) && lens.len() < 3 {
            let (n, int) = digit_run(chars, q + 1, true);
            if !int || n > 4 {
                return false;
            }
            lens.push(n);
            q += 1 + n;
        }
        return match lens.as_slice() {
            [a, b] => (*a == 4 && *b <= 2) || (*a <= 2 && *b == 4),
            [a, b, c] => (*a == 4 && *b <= 2 && *c <= 2) || (*a <= 2 && *b <= 2 && *c == 4),
            _ => false,
        };
    }
    false
}

/// Índices bursátiles cuyo número forma parte del nombre (`S&P 500`): no son datos.
fn is_index_name(text: &str, start: usize, base: f64) -> bool {
    const NAMES: &[(&str, &[f64])] = &[
        ("s&p", &[100.0, 400.0, 500.0, 600.0]),
        ("nasdaq", &[100.0]),
        ("russell", &[1000.0, 2000.0, 3000.0]),
        ("stoxx", &[50.0, 600.0]),
        ("ibex", &[35.0]),
        ("dax", &[30.0, 40.0]),
        ("ftse", &[100.0, 250.0]),
        ("cac", &[40.0]),
        ("nikkei", &[225.0]),
        ("dow", &[30.0]),
    ];
    let before = text[..start].trim_end_matches(|c: char| is_space(c) || c == '-').to_lowercase();
    NAMES.iter().any(|(n, vals)| before.ends_with(n) && vals.iter().any(|v| (base - v).abs() < 1e-9))
}

/// Palabras (por prefijo) que indican que el texto habla de una bajada.
const DECLINE_CUES: &[&str] = &[
    "caíd", "caid", "cae", "cay", "baja", "bajó", "bajad", "bajan", "bajar", "descen", "retroce", "pérdid", "perdid",
    "pierd", "desplom", "recort", "debajo", "negativ", "correcci", "contrac", "reduc", "disminu", "deprec", "drop",
    "fall", "declin", "below", "loss", "lower", "drawdown", "down",
];

/// ¿Habla de una bajada la frase cercana? Mira hasta 45 caracteres antes y 22 después, sin salir de
/// la frase (punto y seguido, `;` o salto de línea).
fn mentions_decline(text: &str, start: usize, end: usize) -> bool {
    const BOUNDS: [&str; 3] = ["\n", ";", ". "];
    let before: String = text[..start].chars().rev().take(45).collect::<Vec<_>>().into_iter().rev().collect();
    let after: String = text[end..].chars().take(22).collect();
    let from = BOUNDS.iter().filter_map(|p| before.rfind(p).map(|i| i + p.len())).max().unwrap_or(0);
    let to = BOUNDS.iter().filter_map(|p| after.find(p)).min().unwrap_or(after.len());
    let near = format!("{} {}", &before[from..], &after[..to]);
    words_of(&near).iter().any(|w| DECLINE_CUES.iter().any(|c| w.starts_with(c)))
}

/// Palabras del texto cercano (minúsculas, sin puntuación).
fn words_of(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Umbrales y periodos convencionales de los indicadores (RSI 70/30, estocástico 80/20, medias de
/// 20/50/200 sesiones, ATR14, MACD 12/26/9, Bollinger 2σ) cuando aparecen junto a su indicador: no
/// son datos del activo y no se exige que estén en el contexto.
fn is_conventional_threshold(text: &str, start: usize, end: usize, base: f64) -> bool {
    let before: String = text[..start].chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    let after: String = text[end..].chars().take(14).collect();
    let mut words = words_of(&before);
    words.extend(words_of(&after));
    // Palabra exacta o seguida solo de dígitos (`sma200`, `rsi14`); `estoc` admite cualquier cola.
    let word = |key: &str| words.iter().any(|w| w.strip_prefix(key).is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit())));
    let prefix = |key: &str| words.iter().any(|w| w.starts_with(key));
    let phrase = format!("{} {}", before.to_lowercase(), after.to_lowercase()).contains("moving average");
    let is = |vals: &[f64]| vals.iter().any(|v| (base - v).abs() < 1e-9);
    ((word("rsi") || prefix("estoc") || word("stochastic") || word("mfi")) && is(&[20.0, 30.0, 70.0, 80.0]))
        || ((word("sma") || word("ema") || word("media") || word("medias") || phrase) && is(&[5.0, 10.0, 20.0, 50.0, 100.0, 150.0, 200.0]))
        || ((word("atr") || word("adx") || word("rsi")) && is(&[14.0]))
        || (word("bollinger") && is(&[2.0, 20.0]))
        || (word("macd") && is(&[9.0, 12.0, 26.0]))
}

struct Token {
    start: usize,
    end: usize,
    mark_end: usize,
    value: f64,
    decimals: u32,
    other: Option<(f64, u32)>,
    hint: Option<Locale>,
    scale: f64,
    signed: bool,
    negative: bool,
    exempt: bool,
    neg_ok: bool,
}

/// Extrae los números de un texto.
pub fn extract_numbers(text: &str) -> Vec<Num> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        // Signo (solo si está pegado y no es un guion entre palabras/fechas).
        let prev = (i > 0).then(|| chars[i - 1].1);
        let mut start = pos;
        let mut negative = false;
        let mut signed = false;
        if matches!(prev, Some('-' | '−' | '+')) {
            let before = (i > 1).then(|| chars[i - 2].1);
            if !before.is_some_and(|b| b.is_alphanumeric() || b == ')') {
                negative = matches!(prev, Some('-' | '−'));
                signed = true;
                start = chars[i - 1].0;
            }
        }
        let body = parse_body(&chars, i);
        let body_end = body.end;
        let mut j = body_end;
        // Sufijo de escala: pegado (K/M/B/T), o tras un espacio (`3,2 T`, `450 millones`, `mil millones`).
        let mut scale = 1.0;
        if let Some(&(_, s)) = chars.get(j) {
            let sc = match s {
                'K' => 1e3,
                'M' => 1e6,
                'B' => 1e9,
                'T' => 1e12,
                _ => 0.0,
            };
            if sc > 0.0 && !chars.get(j + 1).is_some_and(|(_, n)| n.is_alphanumeric()) {
                scale = sc;
                j += 1;
            }
        }
        if scale == 1.0 && chars.get(j).is_some_and(|(_, c)| is_space(*c)) {
            let k = j + 1;
            if let Some((f, n)) = scale_word(&chars, k) {
                scale = f;
                j = k + n;
            } else if let Some(&(_, s)) = chars.get(k) {
                let sc = match s {
                    'K' => 1e3,
                    'M' => 1e6,
                    'B' => 1e9,
                    'T' => 1e12,
                    _ => 0.0,
                };
                if sc > 0.0 && !chars.get(k + 1).is_some_and(|(_, n)| n.is_alphanumeric()) {
                    scale = sc;
                    j = k + 1;
                }
            }
        }
        let suffix_len = j - body_end;
        let end = chars.get(j).map_or(text.len(), |(p, _)| *p);
        // Porcentaje: `%` pegado o tras un espacio, o «por ciento».
        let k = if chars.get(j).is_some_and(|(_, c)| is_space(*c)) { j + 1 } else { j };
        let pct_end = if chars.get(k).is_some_and(|(_, c)| *c == '%') {
            Some(k + 1)
        } else if k > j {
            let rest = &text[chars.get(k).map_or(text.len(), |(p, _)| *p)..];
            let said = rest.as_bytes().get(..10).is_some_and(|b| b.eq_ignore_ascii_case(b"por ciento"));
            (said && !rest.chars().nth(10).is_some_and(char::is_alphanumeric)).then_some(k + 10)
        } else {
            None
        };
        let has_percent = pct_end.is_some();
        let mark_end = pct_end.map_or(end, |k| chars.get(k).map_or(text.len(), |(p, _)| *p));
        let attached_before = prev.is_some_and(|p| p.is_alphabetic());
        // `52w`, `5G`: código pegado a un entero; `31.2x` o `2.1bps` sí son datos.
        let attached_after = suffix_len == 0 && body.decimals == 0 && body.plain && chars.get(j).is_some_and(|(_, n)| n.is_alphabetic());
        let base = body.value;
        let word_hyphen = matches!(prev, Some('-')) && !signed && i >= 2 && !chars[i - 2].1.is_ascii_digit();
        let date = is_date_or_time(&chars, i, body_end, &body);
        let integer = body.decimals == 0 && scale == 1.0 && !has_percent;
        let small_int = integer && body.plain && base <= 12.0;
        let year = integer && body.plain && (1990.0..=2100.0).contains(&base);
        let conventional = integer && body.plain && is_conventional_threshold(text, start, end, base);
        let index_name = integer && body.plain && is_index_name(text, start, base);
        let exempt = attached_before || attached_after || date || word_hyphen || small_int || year || conventional || index_name;
        let neg_ok = !signed && mentions_decline(text, start, mark_end);
        if (base * scale).is_finite() {
            tokens.push(Token {
                start,
                end,
                mark_end,
                value: base,
                decimals: body.decimals,
                other: body.other,
                hint: body.hint,
                scale,
                signed,
                negative,
                exempt,
                neg_ok,
            });
        }
        i = j.max(i + 1);
    }
    // Pista de formato del texto: una cifra inequívoca decide las ambiguas.
    let es = tokens.iter().any(|t| t.hint == Some(Locale::Es));
    let en = tokens.iter().any(|t| t.hint == Some(Locale::En));
    let decided = match (es, en) {
        (true, false) => Some(Locale::Es),
        (false, true) => Some(Locale::En),
        _ => None,
    };
    tokens
        .into_iter()
        .map(|t| {
            let sign = if t.negative { -1.0 } else { 1.0 };
            let (mut value, mut decimals, mut alt) = (t.value, t.decimals, t.other);
            match (decided, t.other) {
                (Some(Locale::Es), Some((v, d))) => {
                    value = v;
                    decimals = d;
                    alt = None;
                }
                (Some(Locale::En), Some(_)) => alt = None,
                _ => {}
            }
            Num {
                raw: text[t.start..t.end].to_string(),
                value: sign * value * t.scale,
                decimals,
                scale: t.scale,
                span: t.start..t.end,
                mark_end: t.mark_end,
                exempt: t.exempt,
                alt: alt.map(|(v, d)| (sign * v * t.scale, d)),
                signed: t.signed,
                neg_ok: t.neg_ok,
            }
        })
        .collect()
}

/// Índice de las cifras presentes en un contexto.
#[derive(Debug, Clone, Default)]
pub struct NumberIndex {
    entries: Vec<(Num, Segment)>,
}

impl NumberIndex {
    /// Indexa el contexto del análisis (formato de `context::build_context`): clasifica cada línea
    /// por su sección `## …` y las noticias por su etiqueta `[N3]`.
    pub fn from_context(text: &str) -> Self {
        let mut idx = Self::default();
        idx.add_context(text);
        idx
    }

    /// Añade otro contexto (p. ej. el de una etapa que mostró más noticias que el completo).
    pub fn add_context(&mut self, text: &str) {
        let mut seg = Segment::Header;
        for line in text.lines() {
            if let Some(h) = line.strip_prefix("## ") {
                seg = classify_heading(h);
            }
            let line_seg = news_tag(line).map_or_else(|| seg.clone(), Segment::News);
            self.add_text(line, line_seg);
        }
    }

    pub fn add_text(&mut self, text: &str, seg: Segment) {
        // Las exentas (fechas, códigos, enteros pequeños) no son datos: no sirven de aval.
        for n in extract_numbers(text).into_iter().filter(|n| !n.exempt) {
            self.entries.push((n, seg.clone()));
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Segmentos donde aparece una cifra equivalente. Vacío = no está en el contexto.
    pub fn find(&self, n: &Num) -> Vec<Segment> {
        let mut segs: Vec<Segment> = Vec::new();
        for (e, s) in &self.entries {
            let hit = n.readings().any(|(v, tol)| {
                let tol = tol.max(e.tolerance()) * 1.0001;
                (v - e.value).abs() <= tol || (n.neg_ok && (-v - e.value).abs() <= tol)
            });
            if hit && !segs.contains(s) {
                segs.push(s.clone());
            }
        }
        segs
    }

    /// Cifras de `text` que no están en el contexto (ignora las exentas).
    pub fn unverified(&self, text: &str) -> Vec<Num> {
        extract_numbers(text).into_iter().filter(|n| !n.exempt && self.find(n).is_empty()).collect()
    }

    /// Cifras citadas (lista `figures` del modelo): `Ok(segmentos)` si todas existen.
    pub fn check_figures(&self, figures: &[String]) -> Result<Vec<Segment>, Vec<String>> {
        let mut segs: Vec<Segment> = Vec::new();
        let mut bad = Vec::new();
        for f in figures {
            for n in extract_numbers(f) {
                if n.exempt {
                    continue;
                }
                let found = self.find(&n);
                if found.is_empty() {
                    bad.push(n.raw.clone());
                }
                for s in found {
                    if !segs.contains(&s) {
                        segs.push(s);
                    }
                }
            }
        }
        if bad.is_empty() { Ok(segs) } else { Err(bad) }
    }

    /// Reescribe `text` marcando con `⚠` cada cifra no verificable. Devuelve el texto y las cifras.
    pub fn mark_unverified(&self, text: &str) -> (String, Vec<String>) {
        let bad = self.unverified(text);
        if bad.is_empty() {
            return (text.to_string(), vec![]);
        }
        let mut out = String::with_capacity(text.len() + bad.len() * 3);
        let mut last = 0;
        for n in &bad {
            // El `%` (pegado o tras un espacio) forma parte de la cifra: la marca va después.
            let end = n.mark_end;
            out.push_str(&text[last..end]);
            out.push('⚠');
            last = end;
        }
        out.push_str(&text[last..]);
        (out, bad.into_iter().map(|n| n.raw).collect())
    }
}

fn classify_heading(h: &str) -> Segment {
    let h = h.trim_start();
    let starts = |p: &[&str]| p.iter().any(|x| h.starts_with(x));
    if starts(&["Cotización", "Quote"]) {
        Segment::Quote
    } else if starts(&["Indicadores técnicos", "Technical indicators"]) {
        Segment::Indicators
    } else if starts(&["Fundamentales", "Fundamentals"]) {
        Segment::Fundamentals
    } else if starts(&["Puntuaciones", "Quantitative scores"]) {
        Segment::Scores
    } else if starts(&["Entorno macro", "Macro environment", "Macro"]) {
        Segment::Macro
    } else if starts(&["Noticias", "News"]) {
        Segment::Other
    } else if starts(&["Datos no disponibles", "Missing data"]) {
        Segment::Gaps
    } else {
        Segment::Other
    }
}

/// `[N3] 2026-…` → `N3`.
fn news_tag(line: &str) -> Option<String> {
    let rest = line.strip_prefix("[N")?;
    let end = rest.find(']')?;
    let digits = &rest[..end];
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| format!("N{digits}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn conventional_indicator_thresholds_are_exempt_only_next_to_their_indicator() {
        let ex = |t: &str| super::extract_numbers(t).iter().filter(|n| !n.exempt).count();
        assert_eq!(ex("Si el RSI14 se sitúa por encima de 70, indicaría sobrecompra"), 0);
        assert_eq!(ex("el RSI por debajo de 30 es sobreventa; estocástico sobre 80"), 0);
        assert_eq!(ex("precio sobre la media de 200 sesiones y la SMA 50"), 0);
        // Un 70 cualquiera, sin su indicador, sigue necesitando respaldo en los datos.
        assert_eq!(ex("El beneficio sube un 70 millones"), 1);
        assert_eq!(ex("El RSI14 está en 71"), 1);
    }


    use super::*;

    const CTX: &str = "# Apple (stock:AAPL) — stock\n\
## Cotización [fuente: yahoo · 2026-10-12 14:03 UTC]\n| Precio | 212.3 USD |\n| Volume | 51.2M |\n\
## Indicadores técnicos (velas diarias)\n| RSI14 | 72.3 |\n| Rendimientos | 1m +4.5%, 1y +18.2% |\n\
## Fundamentales\n| EV | 3.21T |\n\
## Noticias (más recientes primero)\n[N1] 2026-10-10 09:00 · Reuters · +0.40 positive: Apple gana 12% más que lo previsto\n";

    fn find_first(idx: &NumberIndex, s: &str) -> Vec<Segment> {
        idx.find(&extract_numbers(s)[0])
    }

    #[test]
    fn extracts_numbers_with_suffix_decimals_and_sign() {
        let n = extract_numbers("subió -3.5% y 1,234.5 con EV 3.21T, volumen 450M");
        let raws: Vec<&str> = n.iter().map(|x| x.raw.as_str()).collect();
        assert_eq!(raws, ["-3.5", "1,234.5", "3.21T", "450M"]);
        assert_eq!(n[1].value, 1234.5);
        assert_eq!(n[2].value, 3.21e12);
        assert!(n[0].value < 0.0);
    }

    #[test]
    fn exempts_codes_dates_small_ints_and_years() {
        let n = extract_numbers("SMA200 RSI14 52w 3 razones en 2026 el 2026-10-12 a las 14:03 y N3");
        assert!(n.iter().all(|x| x.exempt), "{n:?}");
        let n = extract_numbers("precio 212.3 y 45%");
        assert!(n.iter().all(|x| !x.exempt));
    }

    #[test]
    fn index_tracks_the_segment_of_each_figure() {
        let idx = NumberIndex::from_context(CTX);
        assert_eq!(find_first(&idx, "212.3"), vec![Segment::Quote]);
        assert_eq!(find_first(&idx, "72.3"), vec![Segment::Indicators]);
        assert_eq!(find_first(&idx, "3.21T"), vec![Segment::Fundamentals]);
        assert_eq!(find_first(&idx, "51.2M"), vec![Segment::Quote]);
        assert_eq!(find_first(&idx, "12%"), vec![Segment::News("N1".into())]);
        assert!(find_first(&idx, "999.9").is_empty());
    }

    #[test]
    fn tolerance_is_half_a_unit_of_the_least_precise_figure() {
        let idx = NumberIndex::from_context(CTX);
        assert!(!find_first(&idx, "72").is_empty(), "redondeo de 72.3");
        assert!(find_first(&idx, "73").is_empty(), "73 no es 72.3");
        assert!(!find_first(&idx, "3.2T").is_empty());
        assert!(find_first(&idx, "3.5T").is_empty());
        assert!(!find_first(&idx, "212.30").is_empty());
    }

    #[test]
    fn checks_figure_lists_and_reports_the_bad_ones() {
        let idx = NumberIndex::from_context(CTX);
        let ok = idx.check_figures(&["RSI14 72.3".into(), "212.3 USD".into()]).unwrap();
        assert!(ok.contains(&Segment::Indicators) && ok.contains(&Segment::Quote));
        let bad = idx.check_figures(&["RSI14 72.3".into(), "PER 41.7".into()]).unwrap_err();
        assert_eq!(bad, vec!["41.7".to_string()]);
    }

    #[test]
    fn marks_unverified_numbers_inline() {
        let idx = NumberIndex::from_context(CTX);
        let (t, bad) = idx.mark_unverified("RSI 72.3 con objetivo en 250 y 3 razones; sube un 45%");
        assert_eq!(t, "RSI 72.3 con objetivo en 250⚠ y 3 razones; sube un 45%⚠");
        assert_eq!(bad, ["250", "45"]);
        let (same, none) = idx.mark_unverified("sin cifras raras 212.3");
        assert_eq!(same, "sin cifras raras 212.3");
        assert!(none.is_empty());
    }


    // --- Segunda revisión (R-11…R-16): formato español, porcentajes, rangos, signo ---------------

    /// Valores (primera lectura) y decimales de las cifras NO exentas de un texto.
    fn vals(t: &str) -> Vec<(f64, u32)> {
        extract_numbers(t).into_iter().filter(|n| !n.exempt).map(|n| (n.value, n.decimals)).collect()
    }

    #[test]
    fn spanish_number_formats_are_parsed_as_one_figure() {
        assert_eq!(vals("81.059,79"), [(81059.79, 2)]);
        assert_eq!(vals("el ATR es 2057,45 USD"), [(2057.45, 2)]);
        assert_eq!(vals("cae un -3,2 %"), [(-3.2, 1)]);
        assert_eq!(vals("1.234.567"), [(1_234_567.0, 0)]);
        assert_eq!(vals("1.234,5"), [(1234.5, 1)]);
        assert_eq!(vals("sube 12,5%"), [(12.5, 1)]);
        assert_eq!(vals("de 0,5 a 0,75"), [(0.5, 1), (0.75, 2)]);
        // «2057» no es un año cuando es la parte entera de 2057,45.
        assert!(!extract_numbers("2057,45")[0].exempt);
        // Con espacio fino/no separable como separador de millares.
        assert_eq!(vals("81\u{202f}059,79"), [(81059.79, 2)]);
    }

    #[test]
    fn english_formats_are_unchanged() {
        assert_eq!(vals("81059.79"), [(81059.79, 2)]);
        assert_eq!(vals("1,234.5"), [(1234.5, 1)]);
        assert_eq!(vals("1,234,567"), [(1_234_567.0, 0)]);
        assert_eq!(vals("RSI 61.3, SMA20 226.1, SMA50 221.5"), [(61.3, 1), (226.1, 1), (221.5, 1)]);
        assert_eq!(vals("0.123 y 1234.567"), [(0.123, 3), (1234.567, 3)]);
        // Una coma seguida de espacio separa cifras, no es decimal.
        assert_eq!(vals("de 61.3, 72.4 y 80.1"), [(61.3, 1), (72.4, 1), (80.1, 1)]);
        // Lista pegada: no se inventa una cifra decimal.
        assert!(vals("los puntos 1,2,3").is_empty());
    }

    #[test]
    fn ambiguous_thousands_are_decided_by_the_rest_of_the_text() {
        // Sin pistas: lectura inglesa por defecto (así escribe el contexto) y la española como alternativa.
        let n = &extract_numbers("soporte 1.234")[0];
        assert_eq!((n.value, n.decimals, n.alt), (1.234, 3, Some((1234.0, 0))));
        let n = &extract_numbers("volumen 1,234")[0];
        assert_eq!((n.value, n.alt), (1234.0, Some((1.234, 3))));
        // Con una cifra inequívocamente española en el mismo texto, «79.769» son miles.
        let n = extract_numbers("el precio 81.059,79 y el soporte 79.769");
        assert_eq!((n[1].value, n[1].alt), (79_769.0, None));
        // Con una cifra inequívocamente inglesa, «1,234» son miles.
        let n = extract_numbers("precio 81,059.79 y volumen 1,234");
        assert_eq!((n[1].value, n[1].alt), (1234.0, None));
        // Y «1.234» es un decimal.
        let n = extract_numbers("precio 81,059.79 y ratio 1.234");
        assert_eq!((n[1].value, n[1].alt), (1.234, None));
    }

    #[test]
    fn spanish_figures_match_the_english_context() {
        let ctx = "## Cotización\n| Precio | 81059.79 USD |\n## Indicadores técnicos (velas diarias)\n| ATR14 | 2057.45 |\n| Rendimientos | 1m -3.2%, 1y +18.2% |\n| Caída máxima 1 año | -13.8% |\n| Volumen | 1234 |\n";
        let idx = NumberIndex::from_context(ctx);
        let ok = |t: &str| idx.unverified(t).is_empty();
        assert!(ok("El precio es 81.059,79 USD"));
        assert!(ok("El ATR14 es 2057,45"));
        assert!(ok("el rendimiento mensual fue -3,2 %"));
        assert!(ok("el rendimiento mensual fue -3.2%"));
        assert!(ok("el rendimiento anual fue +18,2 %"));
        // Redondeo a la precisión menos exacta, también en español.
        assert!(ok("el precio ronda 81.060"), "81.060 son 81060 (miles), compatible con 81059.79 ± 0,5");
        // Cifras españolas inventadas siguen sin pasar.
        assert!(!ok("el precio es 82.059,79 USD"));
        assert!(!ok("el ATR14 es 2057,95"));
        assert!(!ok("el rendimiento mensual fue -3,9 %"));
        // «1.234» sin pistas: vale si alguna de sus dos lecturas (1.234 o 1234) está en los datos.
        assert!(ok("volumen de 1.234"));
        assert!(!ok("volumen de 1.235"));
    }

    #[test]
    fn spaced_percent_is_still_a_percentage_not_a_small_integer() {
        // «5%» pegado ya se verificaba; «5 %» (norma española) se eximía por ser un entero pequeño.
        assert_eq!(vals("sube un 5%"), [(5.0, 0)]);
        assert_eq!(vals("sube un 5 %"), [(5.0, 0)]);
        assert_eq!(vals("sube un 5\u{a0}%"), [(5.0, 0)]);
        assert_eq!(vals("sube un 5 por ciento"), [(5.0, 0)]);
        assert!(vals("tiene 5 razones").is_empty());
        let idx = NumberIndex::from_context("## Indicadores técnicos\n| Rendimientos | 1m +4.5% |\n");
        let (t, bad) = idx.mark_unverified("sube un 9 % este mes");
        assert_eq!((t.as_str(), bad), ("sube un 9 %⚠ este mes", vec!["9".to_string()]));
    }

    #[test]
    fn ranges_and_pairs_are_figures_but_dates_and_codes_are_not() {
        assert_eq!(vals("entre 81000-82000"), [(81000.0, 0), (82000.0, 0)]);
        assert_eq!(vals("rango 3.5-4.2"), [(3.5, 1), (4.2, 1)]);
        assert_eq!(vals("soporte/resistencia 81059.79/84014.13"), [(81059.79, 2), (84014.13, 2)]);
        assert!(vals("el 2026-10-12 a las 14:03 y el 12/10/2026, 12-10-2026 y 2026-10").is_empty());
        assert!(vals("COVID-19 y SMA20-SMA50, Q3-2026, 10-K").is_empty());
    }

    #[test]
    fn scale_suffixes_in_all_their_spellings() {
        let one = |t: &str| {
            let v = vals(t);
            assert_eq!(v.len(), 1, "{t}: {v:?}");
            v[0].0
        };
        assert_eq!(one("EV de 3.21T"), 3.21e12);
        assert_eq!(one("EV de 3,21 T"), 3.21e12);
        assert_eq!(one("EV de 3.21 T"), 3.21e12);
        assert_eq!(one("volumen de 450M"), 450e6);
        assert_eq!(one("volumen de 450 M"), 450e6);
        assert_eq!(one("volumen de 450 millones"), 450e6);
        assert_eq!(one("deuda de 2,5 mil millones"), 2.5e9);
        assert_eq!(one("capitalización de 3,2 billones"), 3.2e12);
        assert_eq!(one("market cap 1.5 billion"), 1.5e9);
        assert_eq!(one("market cap 3.2 trillion"), 3.2e12);
        assert_eq!(one("volumen de 35K"), 35e3);
        assert_eq!(one("100 mil"), 100e3);
        assert_eq!(one("1,234 millones"), 1234e6);
        // El `B` pegado sigue siendo 10^9 (así lo escribe el contexto).
        assert_eq!(one("deuda 4.5B"), 4.5e9);
        // Una palabra que empieza como un sufijo no lo es.
        assert_eq!(one("hay 45.5 millas de distancia y 2 milagros"), 45.5);
    }

    #[test]
    fn multiples_and_units_with_decimals_are_figures() {
        // «31.2x» es un múltiplo (dato); «52w», «200d», «5G» siguen siendo códigos.
        assert_eq!(vals("PER de 31.2x"), [(31.2, 1)]);
        assert_eq!(vals("2.1bps"), [(2.1, 1)]);
        assert!(vals("52w 200d 5G").is_empty());
    }

    #[test]
    fn index_names_are_not_figures() {
        assert!(vals("el S&P 500, el Nasdaq 100, el Russell 2000, el IBEX 35, el DAX 40, el Euro Stoxx 50 y el Nikkei 225").is_empty());
        // Un 500 suelto sigue necesitando respaldo.
        assert_eq!(vals("el beneficio sube 500 millones"), [(500e6, 0)]);
        assert_eq!(vals("el 500 de verdad"), [(500.0, 0)]);
    }

    #[test]
    fn unsigned_figures_match_a_negative_datum_only_when_the_text_says_it_fell() {
        let ctx = "## Indicadores técnicos (velas diarias)\n| Caída máxima 1 año | -13.8% |\n| Rendimientos | 1m -3.2%, 1y +18.2% |\n";
        let idx = NumberIndex::from_context(ctx);
        let ok = |t: &str| idx.unverified(t).is_empty();
        assert!(ok("La caída máxima del último año fue del 13.8%."));
        assert!(ok("El precio cayó un 3.2% en el último mes."));
        assert!(ok("pérdida mensual de 3,2 %"));
        assert!(ok("El drawdown anual es de 13.8%."));
        // Con el signo explícito, el signo debe coincidir.
        assert!(ok("rinde -3.2% en un mes"));
        assert!(!ok("rinde +3.2% en un mes"));
        // Sin palabra de caída, un 3.2 positivo no es el -3.2 de los datos: direcciones contrarias.
        assert!(!ok("El precio sube un 3.2% en el último mes."));
        assert!(!ok("El rendimiento mensual es del 13.8%."));
        // El positivo sigue verificándose igual.
        assert!(ok("El precio sube un 18.2% en un año."));
        assert!(ok("El precio sube +18.2% en un año."));
    }

    #[test]
    fn works_on_the_real_rendered_context() {
        use crate::{context, fixture, lang::Lang, scoring};
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::Es);
        let built = context::build_context(
            &snap,
            &card,
            &context::ContextOptions { max_tokens: 6000, lang: Lang::Es },
            &mut context::NewsRegistry::new(),
        );
        let idx = NumberIndex::from_context(&built.text);
        assert!(idx.len() > 50);
        let price = snap.quote.as_ref().unwrap().price;
        let n = extract_numbers(&crate::fmt::price(price));
        assert!(idx.find(&n[0]).contains(&Segment::Quote));
        let rsi = snap.indicators.as_ref().unwrap().rsi14.unwrap();
        let n = extract_numbers(&format!("{rsi:.1}"));
        assert!(!idx.find(&n[0]).is_empty());
    }
}
