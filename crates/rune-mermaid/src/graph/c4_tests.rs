use super::*;

fn c4(src: &str) -> C4 {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::C4(c)) => c,
        _ => panic!("not a C4 diagram: {src:?}"),
    }
}

#[test]
fn elements_boundaries_and_relationships() {
    let d = c4(r#"C4Context
    title System Context diagram for Internet Banking System
    Enterprise_Boundary(b0, "BankBoundary0") {
      Person(customerA, "Banking Customer A", "A customer of the bank, with personal bank accounts.")
      System(SystemAA, "Internet Banking System", "Allows customers to view information.", $tags="v1.0")
      Boundary(b1, "BankBoundary", "boundary") {
        SystemDb_Ext(SystemE, "Mainframe Banking System", "Stores all of the core banking information.")
      }
    }
    ContainerQueue(q, "Events", "Kafka", "Domain events")
    Person_Ext(other, "Visitor")
    Rel(customerA, SystemAA, "Uses")
    BiRel(SystemAA, SystemE, "Uses", "XML/HTTPS")
    Rel_Back(q, SystemAA, "Feeds")
    Rel_U(other, customerA, "Asks")
    Rel(customerA, nobody, "Lost")
    UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="1")"#);
    assert_eq!(d.title.as_deref(), Some("System Context diagram for Internet Banking System"));
    let labels: Vec<&str> = d.elements.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["Banking Customer A", "Internet Banking System", "Mainframe Banking System", "Events", "Visitor"]);
    assert_eq!(d.boundaries.len(), 2);
    assert_eq!((d.boundaries[0].title.as_str(), d.boundaries[1].parent), ("BankBoundary0\n[Enterprise]", Some(0)));
    assert_eq!((d.elements[0].boundary, d.elements[2].boundary, d.elements[3].boundary), (Some(0), Some(1), None));
    assert_eq!((d.elements[2].store, d.elements[2].external), (Store::Db, true));
    assert_eq!((d.elements[3].techn.as_str(), d.elements[3].descr.as_str()), ("Kafka", "Domain events"));
    assert_eq!(stereotype(&d.elements[2]), "«external_system_db»");
    assert_eq!(d.edges.len(), 4, "a relationship to an unknown alias is skipped");
    assert_eq!(d.edges[1].label.as_deref(), Some("Uses\n[XML/HTTPS]"));
    assert_eq!((d.edges[1].start, d.edges[1].end), (Head::Arrow, Head::Arrow));
    assert_eq!((d.edges[2].from, d.edges[2].start, d.edges[2].end), (3, Head::Arrow, Head::None), "`Rel_Back` points at its first alias");
    assert_eq!((d.edges[3].from, d.edges[3].to, d.edges[3].start), (0, 4, Head::Arrow), "`Rel_U` puts its target above");
    assert!(crate::parse("C4Context\n    title Empty").is_none());
}

#[test]
fn dynamic_diagrams_number_their_steps() {
    let d = c4("C4Dynamic\n    Container(a, \"App\")\n    ContainerDb(b, \"DB\")\n    RelIndex(1, a, b, \"Reads\")\n    Rel(b, a, \"Rows\")");
    let labels: Vec<_> = d.edges.iter().map(|e| e.label.clone().unwrap()).collect();
    assert_eq!(labels, ["1: Reads", "2: Rows"]);
    let deep = format!("C4Deployment\n{}Node(x, \"X\")", "Deployment_Node(n, \"N\") {\n".repeat(MAX_DEPTH + 1));
    assert!(crate::parse(&deep).is_none());
}

#[test]
fn draws_cards_inside_their_frames() {
    let s = crate::scene(
        "C4Container\nSystem_Boundary(s, \"Shop\") {\nContainer(web, \"Web\", \"Rust\", \"Serves pages\")\n}\nPerson(u, \"User\")\nRel(u, web, \"Visits\", \"HTTPS\")",
        &DiagramStyle::neutral(true),
        None,
    )
    .unwrap();
    let at = |t: &str| s.labels.iter().find(|l| l.text == t).unwrap_or_else(|| panic!("{t}")).at;
    for t in ["«container»", "[Rust]", "Serves pages", "«person»", "Shop\n[System]", "Visits\n[HTTPS]"] {
        at(t);
    }
    assert!(at("User")[1] < at("Web")[1], "the person comes first");
}
