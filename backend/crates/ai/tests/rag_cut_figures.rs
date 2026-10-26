//! N2 (re-auditoría 20-oct-2026): el recorte de un fragmento recuperado a `MAX_FRAGMENT_CHARS` no
//! puede dejar una cifra partida, porque el `NumberIndex` indexa lo que el modelo vio y una cifra
//! inventada («1.23», «12,3 millones») pasaría por respaldada por el trozo `1,23…` / `$12,345.6 mil…`.

use ai::fidelity::{NumberIndex, Segment};
use chrono::{TimeZone, Utc};
use knowledge::{
    DocKind, DocumentInfo, SearchHit,
    citation::{MAX_FRAGMENT_CHARS, rendered_fragment},
};

fn hit(text: String) -> SearchHit {
    SearchHit {
        chunk_id: 1,
        section: None,
        text,
        score: 1.0,
        similarity: 1.0,
        lexical: false,
        recency: 1.0,
        source_weight: 1.0,
        doc: DocumentInfo {
            id: 1,
            doc_key: "filing:cut".into(),
            kind: DocKind::Filing,
            subtype: Some("S-1".into()),
            source: "sec".into(),
            publisher: None,
            title: "Acme prospectus".into(),
            url: None,
            issuer: None,
            language: Some("en".into()),
            published_at: Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
            assets: vec![],
            pinned: false,
        },
    }
}

/// Texto de palabras sin cifras en el que `figure` empieza justo `before_cap` caracteres antes del tope.
fn text_with_figure_before_cap(before_cap: usize, figure: &str) -> String {
    let at = MAX_FRAGMENT_CHARS - before_cap;
    let pad = format!("{}{}", if at % 2 == 1 { "aa " } else { "" }, "a ".repeat(at / 2 - usize::from(at % 2 == 1)));
    assert_eq!(pad.chars().count(), at);
    format!("{pad}{figure} {}", "zeta ".repeat(100))
}

/// ¿Respalda el fragmento renderizado esta frase inventada por el modelo?
fn backs(text: String, claim: &str) -> bool {
    let rendered = rendered_fragment(&hit(text));
    let mut idx = NumberIndex::default();
    idx.add_text(&rendered, Segment::Other);
    idx.review(claim).findings.is_empty()
}

#[test]
fn a_figure_cut_at_the_border_does_not_back_an_invented_one() {
    // (cifra original, lo que dejaba el recorte antiguo, cifra inventada que ese trozo respaldaba)
    let cases = [
        ("1,234,567", "1,23", "1.23"),
        ("$12,345.6 million", "$12,345.6 mil", "12,3 millones"),
        ("987.65", "987.6", "987.6"),
    ];
    for (figure, old_leftover, invented) in cases {
        let text = text_with_figure_before_cap(old_leftover.chars().count(), figure);
        let rendered = rendered_fragment(&hit(text.clone()));
        assert!(!rendered.contains(&figure[..3]), "{figure}: la cifra partida no debe quedar visible: …{}", &rendered[rendered.len() - 30..]);
        assert!(!backs(text, &format!("El dato fue {invented}.")), "{figure}: «{invented}» no debe quedar respaldada");
    }
}

#[test]
fn control_a_whole_figure_inside_the_visible_part_still_backs_itself() {
    // Sin esto el test anterior pasaría también si nada respaldara nada.
    for (figure, claim) in [("1,234,567", "El dato fue 1,234,567."), ("$12,345.6 million", "Fueron 12.345,6 millones."), ("987.65", "El dato fue 987.65.")] {
        let text = text_with_figure_before_cap(MAX_FRAGMENT_CHARS - 200, figure);
        assert!(backs(text, claim), "{figure}");
    }
}
