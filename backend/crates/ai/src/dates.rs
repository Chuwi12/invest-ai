//! Fechas escritas en la prosa del modelo («12 oct 2026», «2026-11-02», «2 de noviembre»,
//! «noviembre de 2026», «Nov 2, 2026», «12/10/2026») y su comprobación contra las fechas de los
//! datos. Las fechas relativas («la próxima semana», «mañana») y los años sueltos no se tocan.
//!
//! Criterio de coincidencia (`DateIndex::contains`):
//! - fecha completa: debe estar tal cual entre las fechas de los datos;
//! - `12/10/2026` se lee día/mes (castellano) y, si no es válida así, mes/día: vale cualquiera;
//! - día y mes sin año («2 de noviembre»): vale si algún dato tiene ese día y mes (en cualquier año);
//! - mes y año («noviembre de 2026»): vale si algún dato cae en ese mes de ese año.

use std::{collections::HashSet, ops::Range};

use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq)]
pub enum DateKind {
    Full(NaiveDate),
    /// Dos lecturas posibles (`dd/mm/aaaa` o `mm/dd/aaaa`).
    Either(Vec<NaiveDate>),
    DayMonth(u32, u32),
    MonthYear(i32, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WrittenDate {
    pub raw: String,
    /// Posición en bytes.
    pub span: Range<usize>,
    pub kind: DateKind,
}

fn fold(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'á' => 'a',
        'é' => 'e',
        'í' => 'i',
        'ó' => 'o',
        'ú' => 'u',
        o => o,
    }
}

fn month_of(word: &str) -> Option<u32> {
    Some(match word {
        "enero" | "ene" | "january" | "jan" => 1,
        "febrero" | "feb" | "february" => 2,
        "marzo" | "mar" | "march" => 3,
        "abril" | "abr" | "april" | "apr" => 4,
        "mayo" | "may" => 5,
        "junio" | "jun" | "june" => 6,
        "julio" | "jul" | "july" => 7,
        "agosto" | "ago" | "august" | "aug" => 8,
        "septiembre" | "setiembre" | "sep" | "sept" | "september" => 9,
        "octubre" | "oct" | "october" => 10,
        "noviembre" | "nov" | "november" => 11,
        "diciembre" | "dic" | "december" | "dec" => 12,
        _ => return None,
    })
}

struct Scan {
    c: Vec<char>,
    f: Vec<char>,
    bytes: Vec<usize>,
}

impl Scan {
    fn digits(&self, i: usize, min: usize, max: usize) -> Option<(u32, usize)> {
        let mut j = i;
        while j < self.c.len() && self.c[j].is_ascii_digit() && j - i < max + 1 {
            j += 1;
        }
        let n = j - i;
        (n >= min && n <= max).then(|| (self.c[i..j].iter().collect::<String>().parse().unwrap_or(0), j))
    }

    fn word(&self, i: usize) -> (String, usize) {
        let mut j = i;
        while j < self.f.len() && self.f[j].is_alphabetic() {
            j += 1;
        }
        (self.f[i..j].iter().collect(), j)
    }

    fn skip_spaces(&self, mut i: usize) -> usize {
        while i < self.c.len() && matches!(self.c[i], ' ' | '\u{a0}') {
            i += 1;
        }
        i
    }

    /// Mes con punto opcional: `(mes, fin)`.
    fn month(&self, i: usize) -> Option<(u32, usize)> {
        let (w, mut j) = self.word(i);
        let m = month_of(&w)?;
        if self.c.get(j) == Some(&'.') && w.len() <= 4 && w != "mayo" {
            j += 1;
        }
        Some((m, j))
    }

    /// Separador entre día y mes: espacios, guion, «de», «del».
    fn day_month_sep(&self, i: usize) -> usize {
        let mut j = i;
        if matches!(self.c.get(j), Some('º' | '°' | 'ª')) {
            j += 1;
        }
        let k = self.skip_spaces(j);
        if self.c.get(k) == Some(&'-') {
            return k + 1;
        }
        let (w, e) = self.word(k);
        if (w == "de" || w == "del") && e > k {
            return self.skip_spaces(e);
        }
        k
    }

    /// Año de cuatro cifras (1900-2100) en `i`, tras « de », coma o espacios.
    fn year_after(&self, i: usize, allow_hyphen: bool) -> Option<(i32, usize)> {
        let mut k = self.skip_spaces(i);
        if allow_hyphen && self.c.get(k) == Some(&'-') {
            k += 1;
        } else if self.c.get(k) == Some(&',') {
            k = self.skip_spaces(k + 1);
        } else {
            let (w, e) = self.word(k);
            if w == "de" || w == "del" {
                k = self.skip_spaces(e);
            }
        }
        let (y, e) = self.digits(k, 4, 4)?;
        ((1900..=2100).contains(&y) && !self.c.get(e).is_some_and(|c| c.is_alphanumeric())).then_some((y as i32, e))
    }
}

/// Fechas escritas de un texto, en orden.
pub fn extract_dates(text: &str) -> Vec<WrittenDate> {
    let c: Vec<char> = text.chars().collect();
    let mut bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    bytes.push(text.len());
    let s = Scan { f: c.iter().map(|x| fold(*x)).collect(), c, bytes };
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.c.len() {
        let before_ok = i == 0 || !(s.c[i - 1].is_alphanumeric() || (matches!(s.c[i - 1], '.' | ',' | '/' | '-') && i >= 2 && s.c[i - 2].is_ascii_digit()));
        if !before_ok {
            i += 1;
            continue;
        }
        let found = if s.c[i].is_ascii_digit() { numeric_start(&s, i) } else if s.f[i].is_alphabetic() { month_start(&s, i) } else { None };
        match found {
            Some((kind, end)) => {
                out.push(WrittenDate { raw: s.c[i..end].iter().collect(), span: s.bytes[i]..s.bytes[end], kind });
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

fn ymd(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(y, m, d)
}

/// Fechas que empiezan por cifra: `2026-11-02`, `12/10/2026`, `12 oct 2026`, `2 de noviembre`.
fn numeric_start(s: &Scan, i: usize) -> Option<(DateKind, usize)> {
    // ISO: aaaa-mm-dd.
    if let Some((y, j)) = s.digits(i, 4, 4)
        && matches!(s.c.get(j), Some('-' | '/'))
        && let Some((m, k)) = s.digits(j + 1, 1, 2)
        && s.c.get(k) == s.c.get(j)
        && let Some((d, e)) = s.digits(k + 1, 1, 2)
        && !s.c.get(e).is_some_and(|c| c.is_alphanumeric())
    {
        return ymd(y as i32, m, d).map(|dt| (DateKind::Full(dt), e));
    }
    let (a, j) = s.digits(i, 1, 2)?;
    // dd/mm/aaaa o dd-mm-aaaa.
    if let Some(sep) = s.c.get(j).filter(|c| matches!(c, '/' | '-'))
        && let Some((b, k)) = s.digits(j + 1, 1, 2)
        && s.c.get(k) == Some(sep)
        && let Some((y, e)) = s.digits(k + 1, 4, 4)
        && !s.c.get(e).is_some_and(|c| c.is_alphanumeric())
    {
        let cands: Vec<NaiveDate> = [ymd(y as i32, b, a), ymd(y as i32, a, b)].into_iter().flatten().collect();
        return (!cands.is_empty()).then_some((DateKind::Either(cands), e));
    }
    // día [de] mes [[de] año].
    let m0 = s.day_month_sep(j);
    if m0 == j {
        return None;
    }
    let (m, end) = s.month(m0)?;
    if let Some((y, e)) = s.year_after(end, true) {
        return ymd(y, m, a).map(|dt| (DateKind::Full(dt), e));
    }
    if ymd(2000, m, a).is_none() && !(m == 2 && a == 29) {
        return None;
    }
    Some((DateKind::DayMonth(m, a), end))
}

/// Fechas que empiezan por mes: `Nov 2, 2026`, `November 2`, `noviembre de 2026`.
fn month_start(s: &Scan, i: usize) -> Option<(DateKind, usize)> {
    let (m, end) = s.month(i)?;
    // Mes día [, año] (inglés).
    let d0 = s.skip_spaces(end);
    if d0 > end
        && let Some((d, mut e)) = s.digits(d0, 1, 2)
    {
        while s.f.get(e).is_some_and(|c| c.is_alphabetic()) && matches!(s.word(e).0.as_str(), "st" | "nd" | "rd" | "th") {
            e = s.word(e).1;
        }
        if !s.c.get(e).is_some_and(|c| c.is_ascii_digit()) {
            if let Some((y, e2)) = s.year_after(e, false) {
                return ymd(y, m, d).map(|dt| (DateKind::Full(dt), e2));
            }
            return (ymd(2000, m, d).is_some()).then_some((DateKind::DayMonth(m, d), e));
        }
    }
    // Mes [de] año.
    let (y, e) = s.year_after(end, false)?;
    Some((DateKind::MonthYear(y, m), e))
}

/// Fechas conocidas (las de los datos y las escritas en el contexto que vio el modelo).
#[derive(Debug, Clone, Default)]
pub struct DateIndex {
    full: HashSet<NaiveDate>,
    day_month: HashSet<(u32, u32)>,
    month_year: HashSet<(i32, u32)>,
}

impl DateIndex {
    pub fn add(&mut self, d: NaiveDate) {
        use chrono::Datelike;
        self.full.insert(d);
        self.day_month.insert((d.month(), d.day()));
        self.month_year.insert((d.year(), d.month()));
    }

    /// Recoge las fechas escritas en un texto (noticias, cabeceras con marcas de tiempo…).
    pub fn add_text(&mut self, text: &str) {
        for d in extract_dates(text) {
            match d.kind {
                DateKind::Full(x) => self.add(x),
                DateKind::Either(v) => v.into_iter().for_each(|x| self.add(x)),
                DateKind::DayMonth(m, dd) => {
                    self.day_month.insert((m, dd));
                }
                DateKind::MonthYear(y, m) => {
                    self.month_year.insert((y, m));
                }
            }
        }
    }

    pub fn contains(&self, d: &WrittenDate) -> bool {
        match &d.kind {
            DateKind::Full(x) => self.full.contains(x),
            DateKind::Either(v) => v.iter().any(|x| self.full.contains(x)),
            DateKind::DayMonth(m, dd) => self.day_month.contains(&(*m, *dd)),
            DateKind::MonthYear(y, m) => self.month_year.contains(&(*y, *m)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(t: &str) -> Vec<DateKind> {
        extract_dates(t).into_iter().map(|d| d.kind).collect()
    }
    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn written_date_formats_are_recognised() {
        assert_eq!(kinds("resultados el 12 oct 2026 y"), [DateKind::Full(day(2026, 10, 12))]);
        assert_eq!(kinds("el 12 oct. 2026"), [DateKind::Full(day(2026, 10, 12))]);
        assert_eq!(kinds("publica el 2026-11-02 sus cuentas"), [DateKind::Full(day(2026, 11, 2))]);
        assert_eq!(kinds("el 2 de noviembre de 2026"), [DateKind::Full(day(2026, 11, 2))]);
        assert_eq!(kinds("el 2 de noviembre"), [DateKind::DayMonth(11, 2)]);
        assert_eq!(kinds("el 1º de mayo, el 25-oct y el 30 nov"), [DateKind::DayMonth(5, 1), DateKind::DayMonth(10, 25), DateKind::DayMonth(11, 30)]);
        assert_eq!(kinds("en noviembre de 2026 habrá"), [DateKind::MonthYear(2026, 11)]);
        assert_eq!(kinds("Nov 2, 2026 y November 5"), [DateKind::Full(day(2026, 11, 2)), DateKind::DayMonth(11, 5)]);
        assert_eq!(kinds("el 12/10/2026"), [DateKind::Either(vec![day(2026, 10, 12), day(2026, 12, 10)])]);
        assert_eq!(kinds("el 25/10/2026"), [DateKind::Either(vec![day(2026, 10, 25)])]);
    }

    #[test]
    fn relative_dates_years_and_look_alikes_are_not_dates() {
        for t in [
            "la próxima semana, mañana y el mes que viene",
            "en 2026 y desde 2019",
            "RSI14 en 52.9 y un 12 % más",
            "81.059,79 y 2057,45",
            "en noviembre",
            "20 sesiones y 30 días",
        ] {
            let d = kinds(t);
            assert!(d.is_empty(), "{t}: {d:?}");
        }
    }

    #[test]
    fn matching_follows_the_documented_criterion() {
        let mut idx = DateIndex::default();
        idx.add(day(2026, 11, 2));
        idx.add_text("Los resultados son el 30 de octubre.");
        let has = |t: &str| extract_dates(t).iter().all(|d| idx.contains(d));
        assert!(has("2026-11-02") && has("2 de noviembre de 2026") && has("2 nov") && has("noviembre de 2026"));
        assert!(has("30 oct") && has("30 de octubre"));
        assert!(!has("2026-11-05") && !has("5 de noviembre") && !has("diciembre de 2026") && !has("2 de noviembre de 2025"));
    }
}
