use super::*;

fn cls(src: &str) -> ClassDiagram {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Class(c)) => c,
        _ => panic!("not a class diagram: {src:?}"),
    }
}

#[test]
fn classes_members_stereotypes_namespaces_and_relations() {
    let c = cls(
        "classDiagram\n\
         direction LR\n\
         namespace core {\n\
           class Runtime~S~ {\n\
             <<struct>>\n\
             +S state\n\
             +Scene motion\n\
             +frame(dt: f64)\n\
             +update(f)\n\
           }\n\
           class El {\n\
             +key(k)\n\
             +child(el)\n\
           }\n\
         }\n\
         Runtime \"1\" *-- \"*\" El : builds\n\
         Widget <|.. Button : implements\n\
         style Runtime fill:#dcf5dc",
    );
    assert_eq!(c.dir, Dir::Right);
    assert_eq!(c.namespaces.len(), 1);
    assert_eq!(c.namespaces[0].title, "core");
    let rt = c.classes.iter().find(|n| n.id == "Runtime").unwrap();
    assert_eq!(rt.label, "Runtime<S>");
    assert_eq!(rt.stereo.as_deref(), Some("struct"));
    assert_eq!(rt.fields.len(), 2);
    assert_eq!(rt.methods.len(), 2);
    assert!(rt.style.fill.is_some());
    assert_eq!(c.edges.len(), 2);
    assert_eq!(c.edges[0].start, Head::DiamondFilled);
    assert_eq!(c.edges[1].start, Head::OpenTriangle);
    let paint = DiagramStyle::neutral(true);
    assert!(draw(&c, &paint, None).is_some());
}

#[test]
fn malformed_class_diagrams_are_rejected() {
    for src in [
        "classDiagram",
        "classDiagram\nclass Open {\n  +x",
        "classDiagram\n}",
        "classDiagram\njust garbage words here",
    ] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
}

#[test]
fn every_prefix_draws_or_falls_back_without_panicking() {
    let src = "classDiagram\n  class A {\n    +i32 x\n    +run()\n  }\n  A --> B : calls";
    let paint = DiagramStyle::neutral(false);
    for (i, _) in src.char_indices() {
        let _ = crate::scene(&src[..i], &paint, None);
    }
    assert!(crate::scene(src, &paint, None).is_some());
}
