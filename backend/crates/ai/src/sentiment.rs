//! Sentimiento de noticias sin LLM: léxico financiero en inglés y español con negaciones e
//! intensificadores. Es una heurística barata (microsegundos por titular) para el score y para
//! mostrar un indicador en la UI; no pretende sustituir a un modelo de lenguaje.

use std::{collections::HashMap, sync::OnceLock};

use domain::{NewsItem, Sentiment, SentimentLabel};

/// Método que se anota en `Sentiment.method`.
pub const METHOD: &str = "lexicon";

// Formato: (palabras separadas por espacios, peso). Positivo = bueno para el precio.
// Se escriben sin tildes (el texto se normaliza antes de buscar) y en forma base: el sufijo
// -s/-es/-ed/-ing/-d en inglés se resuelve en `lookup`.
const EN_POSITIVE: &[(&str, f32)] = &[
    ("surge soar skyrocket spike", 1.6),
    ("rally jump climb leap rebound", 1.2),
    ("gain rise advance recover recovery improve improvement improving", 0.8),
    ("upgrade outperform overweight breakthrough", 1.5),
    ("beat exceed top smash", 1.2),
    ("strong robust solid resilient healthy", 1.0),
    ("growth profit profitable profits expand expansion accelerate acceleration", 0.8),
    ("boost bullish optimistic optimism upside", 1.2),
    ("approve approval approved win wins success successful buyback", 1.0),
    ("raise dividend partnership momentum", 0.6),
    ("record-high", 1.5),
];
const EN_NEGATIVE: &[(&str, f32)] = &[
    ("bankruptcy insolvency", -2.5),
    ("crash collapse plunge plummet", -1.9),
    ("fraud scandal", -2.0),
    ("crisis turmoil selloff sell-off meltdown", -1.7),
    ("tumble slump sink slide tank", -1.4),
    ("downgrade underperform underweight layoffs layoff", -1.5),
    ("default recession hack breach", -1.5),
    ("loss losses lose miss disappoint disappointing disappointed shortfall worst", -1.2),
    ("lawsuit sue probe investigation recall fined penalty sanctions ban halt outage", -1.1),
    ("drop fall decline weak weaker weakness slowdown struggle trouble warning warns", -1.0),
    ("fear fears concern concerns threat tariff tariffs delay delays bearish pessimistic", -0.9),
    ("risk risks volatile volatility", -0.4),
];
const ES_POSITIVE: &[(&str, f32)] = &[
    ("dispara disparan disparo dispararse", 1.6),
    ("repunta repuntan rebota rebotan rebote", 1.2),
    ("sube suben subida subidas subio avanza avanzan avance", 0.8),
    ("gana ganan ganancia ganancias beneficio beneficios", 0.8),
    ("mejora mejoran mejoria mejor recupera recuperan recuperacion crece crecen crecimiento", 0.8),
    ("supera superan impulsa impulsan impulso", 1.2),
    ("alcista optimismo optimista exito rentable", 1.2),
    ("solido robusto resiliente", 1.0),
    ("aprueba aprueban aprobacion recompra recompras", 1.0),
    ("dividendo acuerdo alianza", 0.5),
];
const ES_NEGATIVE: &[(&str, f32)] = &[
    ("quiebra bancarrota impago", -2.5),
    ("desploma desploman desplome hunde hunden hundimiento", -1.9),
    ("fraude escandalo", -2.0),
    ("crisis caos", -1.7),
    ("despidos ere recesion hackeo ciberataque", -1.5),
    ("peor peores", -1.5),
    ("decepciona decepcionan decepcion incumple incumplen", -1.2),
    ("perdida perdidas pierde pierden", -1.2),
    ("demanda denuncia investigacion multa multas sancion sanciones suspende veta prohibe", -1.1),
    (
        "cae caen caida caidas cayo retrocede retroceden debil debilidad desaceleracion ralentizacion",
        -1.0,
    ),
    ("baja bajan bajada bajo bajista pesimismo pesimista", -0.9),
    (
        "miedo miedos temor temores preocupacion preocupa amenaza arancel aranceles retraso retrasos alerta advertencia",
        -0.9,
    ),
    ("riesgo riesgos volatil volatilidad", -0.4),
];

/// Bigramas que cambian o refuerzan el sentido de sus palabras.
const PHRASES: &[(&str, &str, f32)] = &[
    ("record", "high", 1.6),
    ("record", "highs", 1.6),
    ("record", "profit", 1.5),
    ("record", "profits", 1.5),
    ("record", "revenue", 1.4),
    ("record", "earnings", 1.5),
    ("record", "sales", 1.4),
    ("all-time", "high", 1.6),
    ("beats", "estimates", 1.5),
    ("beats", "expectations", 1.5),
    ("tops", "estimates", 1.5),
    ("raises", "guidance", 1.5),
    ("raises", "outlook", 1.5),
    ("raised", "guidance", 1.5),
    ("raises", "forecast", 1.4),
    ("raises", "target", 1.0),
    ("profit", "warning", -2.0),
    ("misses", "estimates", -1.6),
    ("misses", "expectations", -1.6),
    ("cuts", "guidance", -1.6),
    ("cuts", "outlook", -1.6),
    ("cuts", "forecast", -1.5),
    ("cuts", "jobs", -1.5),
    ("job", "cuts", -1.5),
    ("rate", "cut", 0.8),
    ("rate", "cuts", 0.8),
    ("rate", "hike", -0.8),
    ("rate", "hikes", -0.8),
    ("going", "concern", -2.0),
    ("short", "squeeze", 0.8),
    ("maximo", "historico", 1.6),
    ("maximos", "historicos", 1.6),
    ("record", "historico", 1.5),
    ("supera", "previsiones", 1.5),
    ("supera", "expectativas", 1.5),
    ("mejora", "prevision", 1.4),
    ("mejora", "previsiones", 1.4),
    ("mejora", "recomendacion", 1.4),
    ("eleva", "prevision", 1.4),
    ("eleva", "precio", 1.0),
    ("recomienda", "comprar", 1.2),
    ("sobrepondera", "valor", 1.0),
    ("rebaja", "prevision", -1.5),
    ("rebaja", "previsiones", -1.5),
    ("rebaja", "recomendacion", -1.4),
    ("recorta", "prevision", -1.5),
    ("recorta", "previsiones", -1.5),
    ("recorta", "empleo", -1.5),
    ("recorta", "plantilla", -1.5),
    ("recorte", "plantilla", -1.5),
    ("aviso", "resultados", -1.6),
    ("sube", "tipos", -0.8),
    ("bajan", "tipos", 0.8),
    ("baja", "tipos", 0.8),
    ("baja", "recomendacion", -1.4),
    ("peor", "previsto", -1.4),
    ("mejor", "previsto", 1.4),
];

const NEGATORS: &[&str] = &[
    "not", "no", "never", "without", "neither", "nor", "cannot", "dont", "doesnt", "didnt", "isnt",
    "arent", "wasnt", "werent", "wont", "cant", "couldnt", "wouldnt", "hasnt", "havent", "fails",
    "fail", "failed", "sin", "nunca", "jamas", "ni", "tampoco", "nadie", "ningun", "ninguna",
    "ninguno",
];
const INTENSIFIERS: &[&str] = &[
    "sharply", "strongly", "massively", "hugely", "significantly", "dramatically", "deeply",
    "heavily", "steeply", "record", "very", "extremely", "muy", "fuertemente", "drasticamente",
    "notablemente", "significativamente", "bruscamente", "enormemente", "fuerte", "fuertes",
    "brusca", "brusco", "sharp", "steep",
];
const DIMINISHERS: &[&str] = &[
    "slightly", "modestly", "marginally", "barely", "mildly", "somewhat", "little", "ligeramente",
    "levemente", "algo", "ligero", "leve", "modestamente", "apenas",
];

struct Lexicon {
    words: HashMap<&'static str, f32>,
    /// Primera palabra del bigrama → [(segunda, peso)].
    phrases: HashMap<&'static str, Vec<(&'static str, f32)>>,
}

fn lexicon() -> &'static Lexicon {
    static LEX: OnceLock<Lexicon> = OnceLock::new();
    LEX.get_or_init(|| {
        let mut words = HashMap::new();
        for table in [EN_POSITIVE, EN_NEGATIVE, ES_POSITIVE, ES_NEGATIVE] {
            for (list, weight) in table {
                for w in list.split_whitespace() {
                    words.insert(w, *weight);
                }
            }
        }
        let mut phrases: HashMap<&'static str, Vec<(&'static str, f32)>> = HashMap::new();
        for (a, b, w) in PHRASES {
            phrases.entry(a).or_default().push((b, *w));
        }
        Lexicon { words, phrases }
    })
}

/// Quita tildes y pasa a minúsculas; elimina apostrofes (`don't` → `dont`).
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\'' | '’' | '`' => {}
            'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' => out.push('a'),
            'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' => out.push('e'),
            'í' | 'ì' | 'ï' | 'î' | 'Í' => out.push('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' => out.push('o'),
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' => out.push('u'),
            'ñ' | 'Ñ' => out.push('n'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Busca una palabra con una derivación morfológica sencilla (plurales, -ed, -ing).
fn lookup(word: &str) -> Option<f32> {
    let lex = lexicon();
    if let Some(w) = lex.words.get(word) {
        return Some(*w);
    }
    if word.len() < 4 {
        return None;
    }
    let mut stems: Vec<String> = Vec::with_capacity(6);
    for suf in ["ing", "ed", "es", "s", "d"] {
        if let Some(stem) = word.strip_suffix(suf) {
            if stem.len() < 3 {
                continue;
            }
            stems.push(stem.to_string());
            stems.push(format!("{stem}e")); // surging → surge, rising → rise
            let b = stem.as_bytes();
            if b.len() >= 2 && b[b.len() - 1] == b[b.len() - 2] {
                stems.push(stem[..stem.len() - 1].to_string()); // dropped → drop
            }
        }
    }
    stems.iter().find_map(|s| lex.words.get(s.as_str()).copied())
}

fn phrase_weight(a: &str, b: &str) -> Option<f32> {
    lexicon().phrases.get(a)?.iter().find(|(second, _)| *second == b).map(|(_, w)| *w)
}

/// Suma bruta de polaridad de un texto (sin normalizar).
fn raw_score(text: &str) -> f32 {
    let norm = normalize(text);
    let mut total = 0.0f32;
    // Cada cláusula se evalúa por separado: la negación no cruza comas ni puntos.
    for clause in norm.split(['.', ',', ';', ':', '!', '?', '(', ')', '"', '|']) {
        let tokens: Vec<&str> = clause
            .split(|c: char| !(c.is_alphanumeric() || c == '-'))
            .filter(|t| !t.is_empty())
            .collect();
        let mut negate_until = 0usize; // índice (exclusivo) hasta el que aplica una negación
        let mut boost: f32 = 1.0;
        let mut boost_until = 0usize;
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i];
            if NEGATORS.contains(&t) {
                negate_until = i + 4;
                i += 1;
                continue;
            }
            if INTENSIFIERS.contains(&t)
                && !tokens.get(i + 1).is_some_and(|n| phrase_weight(t, n).is_some())
            {
                boost = 1.5;
                boost_until = i + 3;
                i += 1;
                continue;
            }
            if DIMINISHERS.contains(&t) {
                boost = 0.6;
                boost_until = i + 3;
                i += 1;
                continue;
            }
            let (w, consumed) = match tokens.get(i + 1).and_then(|n| phrase_weight(t, n)) {
                Some(w) => (Some(w), 2),
                None => (lookup(t), 1),
            };
            if let Some(mut w) = w {
                if w != 0.0 {
                    if i < boost_until {
                        w *= boost;
                    }
                    // Modificador pospuesto: «cae bruscamente», «sube ligeramente».
                    if let Some(next) = tokens.get(i + consumed) {
                        if INTENSIFIERS.contains(next) {
                            w *= 1.5;
                        } else if DIMINISHERS.contains(next) {
                            w *= 0.6;
                        }
                    }
                    if i < negate_until {
                        // «not bad» no es tan positivo como «good»: se atenúa al invertir.
                        w *= -0.8;
                    }
                    total += w;
                }
            }
            i += consumed;
        }
    }
    total
}

/// Polaridad -1…1 de un texto suelto.
#[allow(dead_code)] // polaridad de texto suelto: la usan los tests y futuras herramientas
pub fn score_text(text: &str) -> f32 {
    (raw_score(text) / 2.5).tanh()
}

fn label_for(score: f32) -> SentimentLabel {
    match score {
        s if s <= -0.5 => SentimentLabel::VeryNegative,
        s if s <= -0.15 => SentimentLabel::Negative,
        s if s < 0.15 => SentimentLabel::Neutral,
        s if s < 0.5 => SentimentLabel::Positive,
        _ => SentimentLabel::VeryPositive,
    }
}

/// Sentimiento de una noticia: titular (peso 1) + inicio del resumen (peso 0,5).
pub fn sentiment_of(item: &NewsItem) -> Sentiment {
    let mut raw = raw_score(&item.title);
    if let Some(summary) = item.summary.as_deref() {
        let head: String = summary.chars().take(400).collect();
        raw += 0.5 * raw_score(&head);
    }
    let score = ((raw / 2.5).tanh() * 100.0).round() / 100.0;
    Sentiment { score, label: label_for(score), method: METHOD.to_string() }
}

/// Rellena `sentiment` de las noticias que no lo traigan del proveedor (method = "lexicon").
/// Respeta el sentimiento ya presente: el de un proveedor con entidades es más fiable.
pub fn annotate_sentiment(items: &mut [NewsItem]) {
    for item in items.iter_mut().filter(|i| i.sentiment.is_none()) {
        item.sentiment = Some(sentiment_of(item));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn item(title: &str, summary: Option<&str>) -> NewsItem {
        NewsItem {
            id: "x".into(),
            title: title.into(),
            summary: summary.map(String::from),
            url: "https://example.com".into(),
            publisher: None,
            published_at: Utc::now(),
            image_url: None,
            tickers: vec![],
            sentiment: None,
            provider: "test".into(),
            language: None,
        }
    }

    fn s(title: &str) -> f32 {
        sentiment_of(&item(title, None)).score
    }

    #[test]
    fn positive_english_headlines() {
        assert!(s("Apple shares surge after record profit") > 0.5);
        assert!(s("Nvidia beats estimates and raises guidance") > 0.5);
        assert!(s("Analyst upgrades Tesla to outperform") > 0.4);
    }

    #[test]
    fn negative_english_headlines() {
        assert!(s("Stocks plunge as recession fears grow") < -0.5);
        assert!(s("Company files for bankruptcy amid fraud probe") < -0.8);
        assert!(s("Intel misses estimates, cuts guidance") < -0.5);
        assert!(s("Analyst downgrades bank after losses") < -0.4);
    }

    #[test]
    fn spanish_headlines() {
        assert!(s("El Ibex se dispara y marca máximos históricos") > 0.5);
        assert!(s("Inditex supera previsiones y eleva el dividendo") > 0.4);
        assert!(s("Fuerte caída en bolsa por el temor a una recesión") < -0.5);
        assert!(s("La compañía anuncia despidos y un aviso de resultados") < -0.5);
        assert!(s("Banco recorta previsiones tras la quiebra de un rival") < -0.5);
    }

    #[test]
    fn negation_flips_polarity() {
        assert!(s("Results are strong") > 0.2);
        assert!(s("Results are not strong") < 0.0);
        assert!(s("Shares did not fall") > 0.0);
        assert!(s("Sin pérdidas en el trimestre") > 0.0);
        assert!(s("La empresa no supera las expectativas") < 0.0);
    }

    #[test]
    fn negation_does_not_cross_clauses() {
        // La coma corta la negación: «no» no afecta a «sube».
        assert!(s("Sin acuerdo, la bolsa sube con fuerza") > 0.0);
    }

    #[test]
    fn intensifiers_and_diminishers() {
        assert!(s("Shares fall sharply") < s("Shares fall"));
        assert!(s("Shares fall slightly") > s("Shares fall"));
        assert!(s("La bolsa cae muy fuerte") <= s("La bolsa cae"));
    }

    #[test]
    fn stemming_handles_inflections() {
        assert!(s("Shares surged") > 0.3);
        assert!(s("Prices are dropping") < -0.2);
        assert!(s("Profits rising") > 0.2);
        assert!(s("Stock dropped 5%") < -0.2);
    }

    #[test]
    fn neutral_and_empty_text() {
        assert_eq!(sentiment_of(&item("", None)).label, SentimentLabel::Neutral);
        let n = sentiment_of(&item("Company announces date of annual meeting", None));
        assert_eq!(n.label, SentimentLabel::Neutral);
        assert_eq!(n.score, 0.0);
    }

    #[test]
    fn score_is_bounded_and_labels_match() {
        let text = "crash plunge collapse bankruptcy fraud scandal crisis ".repeat(10);
        let se = sentiment_of(&item(&text, None));
        assert!(se.score >= -1.0 && se.score <= -0.99);
        assert_eq!(se.label, SentimentLabel::VeryNegative);
        assert_eq!(label_for(0.3), SentimentLabel::Positive);
        assert_eq!(label_for(-0.2), SentimentLabel::Negative);
        assert_eq!(label_for(0.9), SentimentLabel::VeryPositive);
    }

    #[test]
    fn summary_adds_weight_but_less_than_title() {
        let with = sentiment_of(&item("Company update", Some("Shares plunge on fraud probe")));
        assert!(with.score < -0.3);
        let title_only = sentiment_of(&item("Shares plunge on fraud probe", None));
        assert!(title_only.score < with.score);
    }

    #[test]
    fn annotate_fills_only_missing() {
        let mut items = vec![item("Stocks rally", None), item("Stocks crash", None)];
        items[1].sentiment = Some(Sentiment {
            score: 0.9,
            label: SentimentLabel::VeryPositive,
            method: "provider:x".into(),
        });
        annotate_sentiment(&mut items);
        assert_eq!(items[0].sentiment.as_ref().unwrap().method, "lexicon");
        assert!(items[0].sentiment.as_ref().unwrap().score > 0.0);
        assert_eq!(items[1].sentiment.as_ref().unwrap().method, "provider:x");
    }

    #[test]
    fn is_cheap() {
        let items: Vec<NewsItem> = (0..2000)
            .map(|i| {
                item(
                    &format!("Stocks rally {i} as inflation fears fade, no recession seen"),
                    Some("Analysts expect strong earnings growth next quarter despite tariffs"),
                )
            })
            .collect();
        let mut items = items;
        let t = std::time::Instant::now();
        annotate_sentiment(&mut items);
        // 2000 titulares muy por debajo de medio segundo incluso en debug.
        assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
    }
}
