use super::*;

fn er(src: &str) -> ErDiagram {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Er(d)) => d,
        _ => panic!("not an ER diagram: {src:?}"),
    }
}

fn ends(d: &ErDiagram, i: usize) -> (&str, Head, Line, Head, &str) {
    let e = &d.edges[i];
    (&d.entities[e.from].id, e.start, e.line, e.end, &d.entities[e.to].id)
}

#[test]
fn entities_attributes_and_crows_feet() {
    let d = er("erDiagram\n\
         CUSTOMER ||--o{ ORDER : places\n\
         ORDER ||--|{ LINE-ITEM : contains\n\
         CUSTOMER }|..|{ DELIVERY-ADDRESS : uses\n\
         PERSON |o--o| SEAT : \"sits in\"\n\
         CUSTOMER {\n\
           string name\n\
           string custNumber PK \"the key\"\n\
           list~string~ tags\n\
           int orderId FK, UK\n\
         }\n\
         p[\"A person\"] { string first }");
    assert_eq!(d.dir, Dir::Down);
    assert_eq!(ends(&d, 0), ("CUSTOMER", Head::OnlyOne, Line::Solid, Head::ZeroOrMore, "ORDER"));
    assert_eq!(ends(&d, 1), ("ORDER", Head::OnlyOne, Line::Solid, Head::OneOrMore, "LINE-ITEM"));
    assert_eq!(ends(&d, 2), ("CUSTOMER", Head::OneOrMore, Line::Dotted, Head::OneOrMore, "DELIVERY-ADDRESS"));
    assert_eq!(ends(&d, 3), ("PERSON", Head::ZeroOrOne, Line::Solid, Head::ZeroOrOne, "SEAT"));
    assert_eq!(d.edges[3].label.as_deref(), Some("sits in"));
    let c = d.entities.iter().find(|e| e.id == "CUSTOMER").unwrap();
    assert_eq!(c.attrs.len(), 4);
    assert_eq!(c.attrs[1], Attr { ty: "string".into(), name: "custNumber".into(), keys: "PK".into(), comment: "the key".into() });
    assert_eq!(c.attrs[2].ty, "list<string>");
    assert_eq!(c.attrs[3].keys, "FK, UK");
    let p = d.entities.iter().find(|e| e.id == "p").unwrap();
    assert_eq!((p.label.as_str(), p.attrs.len()), ("A person", 1));
}

#[test]
fn worded_cardinalities_and_direction() {
    let d = er("erDiagram\n\
         direction LR\n\
         CAR 1 to zero or more NAMED-DRIVER : allows\n\
         PERSON many(0) optionally to 1+ \"Club House\" : joins");
    assert_eq!(d.dir, Dir::Right);
    assert_eq!(ends(&d, 0), ("CAR", Head::OnlyOne, Line::Solid, Head::ZeroOrMore, "NAMED-DRIVER"));
    assert_eq!(ends(&d, 1), ("PERSON", Head::ZeroOrMore, Line::Dotted, Head::OneOrMore, "Club House"));
}

#[test]
fn bad_lines_and_blocks_are_skipped() {
    let d = er("erDiagram\n\
         A ||--o{ B : ok\n\
         this is not a relation\n\
         C ??--?? D : nope\n\
         E bad name {\n\
           string x\n\
         }\n\
         B {\n\
           string fine\n\
           not-an-attribute\n\
         }");
    assert_eq!(d.entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(d.entities[1].attrs.len(), 1);
    assert!(crate::parse("erDiagram\n  nothing here at all").is_none());
}

#[test]
fn draws_tables_and_feet() {
    let paint = DiagramStyle::neutral(false);
    let s = crate::scene("erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\n string name PK\n}", &paint, None).unwrap();
    let texts: Vec<&str> = s.labels.iter().map(|l| l.text.as_str()).collect();
    for t in ["CUSTOMER", "ORDER", "places", "string", "name", "PK"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    let budget = format!("erDiagram\n{}", (0..MAX_ITEMS).map(|i| format!("E{i} ||--o{{ F{i} : r")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none(), "past the bounds stays source");
}
