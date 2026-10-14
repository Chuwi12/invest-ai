//! Etiquetas de métrica para la verificación de cifras (`fidelity`).
//!
//! Comprobar que una cifra «existe» en los datos no basta: «recompensa/riesgo 6,7» pasaba porque 6,7
//! era la rentabilidad a 1 mes. Aquí se reconoce de QUÉ métrica habla un texto: cada cifra del
//! contexto se indexa con la familia de su fila/etiqueta (`entry_labels`) y cada cifra de la salida
//! del modelo con la familia de la palabra que la acompaña (`Text::claim`). La cifra solo se rechaza
//! cuando coincide en valor con datos de OTRAS métricas y la etiqueta cercana es una métrica
//! conocida y distinta; sin etiqueta reconocible se mantiene la comprobación solo por valor.
//!
//! Principios (la prioridad es no producir falsos positivos):
//! - Las familias son GRUESAS (todos los niveles de precio juntos, todos los márgenes, todas las
//!   rentabilidades…): confundir SMA20 con SMA50 no es lo que se persigue.
//! - Una palabra ambigua («rentabilidad», «rendimiento») admite varias familias en la salida
//!   (rentabilidad = rendimiento del precio, ROE, dividendo…), pero en el contexto es solo una.
//! - Palabras débiles («precio», «crecimiento», «beneficio») no reclaman nada: solo evitan que una
//!   etiqueta más lejana se asigne a la cifra.
//! - La etiqueta de una cifra es la mención más cercana a su IZQUIERDA (hasta 6 palabras, sin cruzar
//!   `, ; | .` ni otra cifra); si no hay, la más cercana a su derecha (hasta 3 palabras).
//! - Un dato sin familia reconocible (noticias, texto libre) acepta cualquier etiqueta.

use crate::fidelity::{Num, Segment};

/// Conjunto de familias de métrica (bits).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Fam(u64);

impl Fam {
    pub const NONE: Fam = Fam(0);
    pub const fn or(self, o: Fam) -> Fam {
        Fam(self.0 | o.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn intersects(self, o: Fam) -> bool {
        self.0 & o.0 != 0
    }
}

impl std::ops::BitOr for Fam {
    type Output = Fam;
    fn bitor(self, o: Fam) -> Fam {
        self.or(o)
    }
}

macro_rules! families {
    ($($name:ident),+ $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum Idx { $($name),+ }
        $(const $name: Fam = Fam(1u64 << Idx::$name as u32);)+
    };
}

families!(
    LEVEL, RSI, MACD, ATR, VOLATILITY, DRAWDOWN, RETURN, VOLUME, PE, PEG, PB, PS, EV, EPS, INCOME, MARGIN, ROE, BALANCE, DIVIDEND,
    BETA, SHORT, ANALYST, MCAP, SUPPLY, RR, SCORE, RATES, VIX, INFL, UNEMP, DXY, FUND,
);

/// Palabra clave. `fam`: lo que reclama al escribirla el modelo; `entry`: a qué métrica pertenece una
/// etiqueta del contexto. Patrón en minúsculas y sin tildes: ` ` = separador (espacio, guion o `_`),
/// `/` = barra con espacios opcionales, `*` final = cualquier terminación, `^` inicial = mayúsculas
/// exactas (siglas que en minúscula serían otra palabra: `^PER`, `^EV`).
struct Word {
    pat: &'static str,
    fam: Fam,
    entry: Fam,
}

const fn w(pat: &'static str, fam: Fam) -> Word {
    Word { pat, fam, entry: fam }
}
/// Palabra débil: no reclama métrica, pero en el contexto etiqueta como `entry`.
const fn weak(pat: &'static str, entry: Fam) -> Word {
    Word { pat, fam: Fam::NONE, entry }
}
/// Palabra ambigua: la salida admite `fam`, el contexto la lee como `entry`.
const fn amb(pat: &'static str, fam: Fam, entry: Fam) -> Word {
    Word { pat, fam, entry }
}

const WORDS: &[Word] = &[
    // Niveles de precio (soportes, medias, bandas, objetivos, entrada/stop, máximos y mínimos).
    w("soporte*", LEVEL),
    w("support*", LEVEL),
    w("resistencia*", LEVEL),
    w("resistance*", LEVEL),
    w("stop", LEVEL),
    w("stop loss", LEVEL),
    w("objetivo*", LEVEL),
    w("target*", LEVEL),
    w("entrada", LEVEL),
    w("entry", LEVEL),
    w("sma", LEVEL),
    w("ema", LEVEL),
    w("media movil", LEVEL),
    w("medias moviles", LEVEL),
    w("moving average*", LEVEL),
    w("bollinger", LEVEL),
    w("banda*", LEVEL),
    w("^ATH", LEVEL),
    w("maximo historico", LEVEL),
    w("all time high", LEVEL),
    w("52 semanas", LEVEL),
    w("52 week*", LEVEL),
    weak("precio*", LEVEL),
    weak("price", LEVEL),
    weak("cotizacion", LEVEL),
    weak("cotiza*", LEVEL),
    weak("cierre", LEVEL),
    weak("open", LEVEL),
    weak("high", LEVEL),
    weak("low", LEVEL),
    weak("prev", LEVEL),
    weak("close", LEVEL),
    weak("mid", LEVEL),
    weak("up", LEVEL),
    // Indicadores técnicos.
    w("rsi", RSI),
    w("indice de fuerza relativa", RSI),
    w("relative strength", RSI),
    w("macd", MACD),
    w("histograma", MACD),
    w("histogram", MACD),
    // El ATR en % del precio ES la volatilidad diaria: «volatilidad diaria de 2,5 %» respalda ATR14 (2,5 % del precio).
    amb("atr", ATR.or(VOLATILITY), ATR),
    amb("rango verdadero medio", ATR.or(VOLATILITY), ATR),
    amb("average true range", ATR.or(VOLATILITY), ATR),
    amb("volatilidad", VOLATILITY.or(ATR), VOLATILITY),
    amb("volatility", VOLATILITY.or(ATR), VOLATILITY),
    w("caida maxima", DRAWDOWN),
    w("max drawdown", DRAWDOWN),
    w("maximo drawdown", DRAWDOWN),
    w("drawdown", DRAWDOWN),
    w("volumen*", VOLUME),
    w("volume", VOLUME),
    // Rentabilidades. En la salida «rentabilidad» también se usa para ROE, dividendo y márgenes.
    amb("rentabilidad*", RETURN.or(ROE).or(DIVIDEND).or(MARGIN), RETURN),
    amb("rendimiento*", RETURN.or(ROE).or(DIVIDEND).or(RATES), RETURN),
    amb("retorno*", RETURN.or(ROE).or(DIVIDEND), RETURN),
    amb("return*", RETURN.or(ROE).or(DIVIDEND).or(RATES), RETURN),
    w("ytd", RETURN),
    w("1w", RETURN),
    w("1m", RETURN),
    w("3m", RETURN),
    w("1y", RETURN),
    w("variacion del dia", RETURN),
    w("day change", RETURN),
    w("rentabilidad sobre", ROE),
    w("rentabilidad financiera", ROE),
    w("rentabilidad economica", ROE),
    w("return on equity", ROE),
    w("return on assets", ROE),
    w("^ROE", ROE),
    w("^ROA", ROE),
    w("rentabilidad por dividendo", DIVIDEND),
    w("rendimiento por dividendo", DIVIDEND),
    w("dividend yield", DIVIDEND),
    amb("yield", DIVIDEND.or(RATES), DIVIDEND.or(RATES)),
    w("dividendo*", DIVIDEND),
    w("dividend*", DIVIDEND),
    w("payout", DIVIDEND),
    // Valoración.
    w("^PER", PE),
    w("p/e", PE),
    w("pe ratio", PE),
    w("price to earnings", PE),
    w("precio/beneficio", PE),
    w("precio beneficio", PE),
    w("precio a ganancias", PE),
    w("precio sobre beneficios", PE),
    w("multiplo de beneficios", PE),
    w("multiplo de ganancias", PE),
    w("^PEG", PEG),
    w("p/b", PB),
    w("price to book", PB),
    w("precio/valor contable", PB),
    w("precio valor contable", PB),
    w("precio sobre valor contable", PB),
    w("p/s", PS),
    w("price to sales", PS),
    w("precio/ventas", PS),
    w("precio ventas", PS),
    w("^EV", EV),
    w("ev/ebitda", EV),
    w("ebitda", EV),
    w("enterprise value", EV),
    w("valor empresa", EV),
    w("valor de empresa", EV),
    w("^EPS", EPS),
    w("bpa", EPS),
    w("beneficio por accion", EPS),
    w("ganancias por accion", EPS),
    w("earnings per share", EPS),
    // Cuentas de resultados y balance.
    w("ingreso*", INCOME),
    w("ventas", INCOME),
    w("revenue*", INCOME),
    w("facturacion", INCOME),
    w("crecimiento de ingresos", INCOME),
    w("crecimiento de beneficios", INCOME),
    w("crecimiento de ganancias", INCOME),
    w("crecimiento de ventas", INCOME),
    w("earnings growth", INCOME),
    w("earnings", INCOME),
    w("beneficio neto", INCOME),
    w("beneficios netos", INCOME),
    w("net income", INCOME),
    weak("crecimiento", INCOME),
    weak("beneficio*", INCOME),
    weak("ganancia*", INCOME),
    weak("resultados", INCOME),
    w("margen*", MARGIN),
    w("margin*", MARGIN),
    w("deuda", BALANCE),
    w("debt", BALANCE),
    w("flujo de caja", BALANCE),
    w("caja libre", BALANCE),
    w("^FCF", BALANCE),
    w("free cash flow", BALANCE),
    w("caja", BALANCE),
    w("cash", BALANCE),
    w("deuda/capital", BALANCE),
    w("deuda/patrimonio", BALANCE),
    w("ratio corriente", BALANCE),
    w("current ratio", BALANCE),
    w("apalancamiento", BALANCE),
    w("beta", BETA),
    w("interes corto", SHORT),
    w("short interest", SHORT),
    w("short float", SHORT),
    w("short % float", SHORT),
    w("posiciones cortas", SHORT),
    w("analista*", ANALYST),
    w("analyst*", ANALYST),
    w("consenso", ANALYST),
    w("consensus", ANALYST),
    weak("rating", ANALYST),
    w("capitalizacion*", MCAP),
    w("market cap", MCAP),
    w("valor de mercado", MCAP),
    w("^FDV", MCAP),
    w("valoracion totalmente diluida", MCAP),
    w("fully diluted", MCAP),
    w("oferta*", SUPPLY),
    w("supply", SUPPLY),
    w("circulating", SUPPLY),
    w("circulante*", SUPPLY),
    w("suministro", SUPPLY),
    // Fondos.
    w("ratio de gastos", FUND),
    w("expense ratio", FUND),
    w("gastos", FUND),
    w("comision de gestion", FUND),
    w("activos bajo gestion", FUND),
    w("total assets", FUND),
    w("assets", FUND),
    w("^AUM", FUND),
    w("holdings", FUND),
    w("posiciones", FUND),
    w("ponderacion", FUND),
    // Niveles calculados y puntuaciones.
    w("recompensa/riesgo", RR),
    w("recompensa riesgo", RR),
    w("riesgo/recompensa", RR),
    w("riesgo recompensa", RR),
    w("riesgo/beneficio", RR),
    w("beneficio/riesgo", RR),
    w("risk/reward", RR),
    w("reward/risk", RR),
    w("risk reward", RR),
    w("reward risk", RR),
    w("^R/R", RR),
    w("puntuacion*", SCORE),
    w("score*", SCORE),
    w("puntaje", SCORE),
    // Macro.
    w("bono*", RATES),
    w("treasury", RATES),
    w("treasuries", RATES),
    w("tipo de la fed", RATES),
    w("tipos de interes", RATES),
    w("fed funds", RATES),
    w("curva", RATES),
    w("us10y", RATES),
    w("us2y", RATES),
    w("vix", VIX),
    w("indice de volatilidad", VIX),
    w("inflacion", INFL),
    w("ipc", INFL),
    w("cpi", INFL),
    w("desempleo", UNEMP),
    w("paro", UNEMP),
    w("unemployment", UNEMP),
    w("dxy", DXY),
    w("dolar", DXY),
    w("indice del dolar", DXY),
];

/// Minúscula sin tilde (1 carácter → 1 carácter, para que los índices coincidan con el texto).
fn fold(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'á' | 'à' | 'ä' | 'â' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        o => o,
    }
}

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '\u{a0}' | '\u{202f}' | '\u{2009}')
}

/// Fin del texto de `word` si empieza en `i`.
fn match_at(orig: &[char], fold_: &[char], i: usize, word: &Word) -> Option<usize> {
    if i > 0 && fold_[i - 1].is_alphanumeric() {
        return None;
    }
    let mut pat = word.pat;
    let exact = pat.starts_with('^');
    if exact {
        pat = &pat[1..];
    }
    let (pat, stem) = match pat.strip_suffix('*') {
        Some(p) => (p, true),
        None => (pat, false),
    };
    let mut j = i;
    for pc in pat.chars() {
        match pc {
            ' ' => {
                let s = j;
                while fold_.get(j).is_some_and(|c| is_sep(*c)) {
                    j += 1;
                }
                if j == s {
                    return None;
                }
            }
            '/' => {
                while fold_.get(j) == Some(&' ') {
                    j += 1;
                }
                if fold_.get(j) != Some(&'/') {
                    return None;
                }
                j += 1;
                while fold_.get(j) == Some(&' ') {
                    j += 1;
                }
            }
            c => {
                let got = if exact { orig.get(j) } else { fold_.get(j) };
                if got != Some(&c) {
                    return None;
                }
                j += 1;
            }
        }
    }
    if stem {
        while fold_.get(j).is_some_and(|c| c.is_alphabetic()) {
            j += 1;
        }
    } else {
        // `rsi14`, `sma200`: dígitos pegados al final.
        while fold_.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
        if fold_.get(j).is_some_and(|c| c.is_alphabetic()) {
            return None;
        }
    }
    Some(j)
}

/// Palabra clave encontrada en un texto (posiciones en caracteres).
#[derive(Debug, Clone, Copy)]
pub struct Mention {
    pub start: usize,
    pub end: usize,
    pub fam: Fam,
    pub entry: Fam,
}

const LEFT_GAP: usize = 6;
const RIGHT_GAP: usize = 3;

/// Texto con sus menciones de métrica ya localizadas.
pub struct Text {
    orig: Vec<char>,
    starts: Vec<usize>,
    ments: Vec<Mention>,
}

impl Text {
    pub fn new(s: &str) -> Self {
        let orig: Vec<char> = s.chars().collect();
        let folded: Vec<char> = orig.iter().map(|c| fold(*c)).collect();
        let starts: Vec<usize> = s.char_indices().map(|(b, _)| b).collect();
        let mut ments = Vec::new();
        let mut i = 0;
        while i < folded.len() {
            let best = WORDS.iter().filter_map(|wd| match_at(&orig, &folded, i, wd).map(|e| (e, wd))).max_by_key(|(e, _)| *e);
            match best {
                Some((end, wd)) => {
                    ments.push(Mention { start: i, end, fam: wd.fam, entry: wd.entry });
                    i = end;
                }
                None => i += 1,
            }
        }
        Self { orig, starts, ments }
    }

    pub fn len(&self) -> usize {
        self.orig.len()
    }

    /// Índice de carácter de un desplazamiento en bytes.
    pub fn ci(&self, byte: usize) -> usize {
        self.starts.partition_point(|&b| b < byte)
    }

    pub fn slice(&self, lo: usize, hi: usize) -> String {
        self.orig[lo.min(self.orig.len())..hi.min(self.orig.len())].iter().collect()
    }

    fn is_hard(&self, k: usize) -> bool {
        match self.orig[k] {
            '\n' | '|' | ';' | ',' => true,
            '.' | '!' | '?' => self.orig.get(k + 1).is_none_or(|c| c.is_whitespace()),
            _ => false,
        }
    }

    fn tokens(&self, lo: usize, hi: usize) -> usize {
        let mut n = 0;
        let mut inside = false;
        for c in &self.orig[lo.min(hi)..hi] {
            if c.is_whitespace() {
                inside = false;
            } else if c.is_alphanumeric() && !inside {
                inside = true;
                n += 1;
            }
        }
        n
    }

    /// Mención más cercana a la izquierda de `from`, sin pasar de `floor`.
    pub fn left(&self, from: usize, floor: usize) -> Option<Mention> {
        let mut lo = floor;
        for k in (floor..from.min(self.orig.len())).rev() {
            if self.is_hard(k) {
                lo = k + 1;
                break;
            }
        }
        let m = self.ments.iter().filter(|m| m.start >= lo && m.end <= from).max_by_key(|m| m.end)?;
        (self.tokens(m.end, from) <= LEFT_GAP).then_some(*m)
    }

    /// Mención más cercana a la derecha de `from`, sin pasar de `ceil`.
    pub fn right(&self, from: usize, ceil: usize) -> Option<Mention> {
        let mut hi = ceil.min(self.orig.len());
        for k in from..hi {
            if self.is_hard(k) {
                hi = k;
                break;
            }
        }
        let m = self.ments.iter().filter(|m| m.start >= from && m.end <= hi).min_by_key(|m| m.start)?;
        (self.tokens(from, m.start) <= RIGHT_GAP).then_some(*m)
    }

    /// Etiqueta de una cifra: la mención de su izquierda o, si no hay, la de su derecha.
    pub fn label(&self, start: usize, end: usize, floor: usize, ceil: usize) -> Option<Mention> {
        self.left(start, floor).or_else(|| self.right(end, ceil))
    }
}

/// Familia y rótulo legible de cada cifra de una línea del contexto (`nums`: las cifras reales de la
/// línea, en orden). Filas de tabla `| fila | valor |` y líneas `Etiqueta: valor` aportan la fila.
pub fn entry_labels(line: &str, nums: &[&Num], seg: &Segment) -> Vec<(Fam, String)> {
    let blank = || (Fam::NONE, String::new());
    let trimmed = line.trim_start();
    if nums.is_empty() || trimmed.starts_with("[N") || trimmed.starts_with('#') || matches!(seg, Segment::News(_)) {
        return nums.iter().map(|_| blank()).collect();
    }
    let t = Text::new(line);
    let pipes: Vec<usize> = (0..t.len()).filter(|k| t.orig[*k] == '|').collect();
    let table = trimmed.starts_with('|') && pipes.len() >= 2;
    let row: Option<(usize, usize)> = if table {
        Some((pipes[0] + 1, pipes[1]))
    } else {
        let body = trimmed.trim_start_matches(['-', ' ']);
        let off = t.len() - body.chars().count();
        body.chars().position(|c| c == ':').filter(|p| *p <= 40).map(|p| (off, off + p))
    };
    let row_fam = row.map_or(Fam::NONE, |(lo, hi)| t.ments.iter().filter(|m| m.start >= lo && m.end <= hi).fold(Fam::NONE, |a, m| a | m.entry));
    let row_text = row.map(|(lo, hi)| t.slice(lo, hi).trim().trim_matches('*').trim().to_string()).unwrap_or_default();
    nums.iter()
        .enumerate()
        .map(|(k, n)| {
            let (s, e) = (t.ci(n.span.start), t.ci(n.mark_end));
            let floor = if k > 0 { t.ci(nums[k - 1].mark_end) } else { 0 };
            let ceil = nums.get(k + 1).map_or(t.len(), |m| t.ci(m.span.start));
            let in_row = row.is_some_and(|(lo, hi)| s >= lo && s < hi);
            let mut fam = if in_row { Fam::NONE } else { row_fam };
            let mut label = if in_row || row_text.is_empty() || row_fam.is_empty() { String::new() } else { row_text.clone() };
            for m in [t.left(s, floor), t.right(e, ceil)].into_iter().flatten() {
                fam = fam | m.entry;
                if !m.fam.is_empty() && !in_row {
                    let inline = t.slice(m.start, m.end);
                    if !label.to_lowercase().contains(&inline.to_lowercase()) {
                        label = if label.is_empty() { inline } else { format!("{label} ({inline})") };
                    }
                }
            }
            if table && matches!(seg, Segment::Scores) && pipes.iter().filter(|p| **p < s).count() == 2 {
                fam = fam | SCORE;
                if label.is_empty() {
                    label = row_text.clone();
                }
            }
            (fam, label)
        })
        .collect()
}
