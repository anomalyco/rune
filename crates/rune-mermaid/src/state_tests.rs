use super::*;

fn state(src: &str) -> StateDiagram {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::State(s)) => s,
        _ => panic!("not a state diagram: {src:?}"),
    }
}

#[test]
fn transitions_pseudostates_descriptions_and_composites() {
    let s = state(
        "stateDiagram-v2\n\
         direction LR\n\
         [*] --> Queued\n\
         state \"In flight\" as Working {\n\
           Queued --> Applying : turn starts\n\
           Applying --> Check\n\
           state Check <<choice>>\n\
           Check --> Applied : commit landed\n\
           Check --> Unapplied : stalled\n\
         }\n\
         Applied : Card moves to Review\n\
         Applied --> [*]",
    );
    assert_eq!(s.dir, Dir::Right);
    assert_eq!(s.composites.len(), 1);
    assert_eq!(s.composites[0].title, "In flight");
    assert!(s.states.iter().any(|st| st.kind == StateKind::Start));
    assert!(s.states.iter().any(|st| st.kind == StateKind::End));
    assert!(s.states.iter().any(|st| st.id == "Check" && st.kind == StateKind::Choice));
    assert_eq!(s.states.iter().find(|st| st.id == "Applied").unwrap().desc, vec!["Card moves to Review"]);
    let paint = DiagramStyle::neutral(true);
    assert!(draw(&s, &paint, None).is_some());
}

#[test]
fn only_sources_where_nothing_reads_are_rejected() {
    for src in ["stateDiagram-v2", "stateDiagram\n}", "stateDiagram-v2\nnote right of A\n  unclosed"] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
}

#[test]
fn a_statement_that_does_not_read_is_skipped() {
    let s = state("stateDiagram-v2\nstate Open {\n  A --> B");
    assert_eq!((s.states.len(), s.edges.len(), s.composites.len()), (2, 1, 1), "an open composite closes at the end");
    let s = state("stateDiagram-v2\ndirection diagonal\nA --> B");
    assert_eq!((s.dir, s.edges.len()), (Dir::Down, 1));
    let s = state("stateDiagram-v2\nstate \"no id\" {\n  A --> B\n}\nC --> A");
    assert_eq!(s.composites[0].title, "no id");
    assert_eq!(s.states.iter().filter(|st| st.cluster.is_none()).count(), 1, "its closing brace closes the unnamed composite");
}

#[test]
fn every_prefix_draws_or_falls_back_without_panicking() {
    let src = "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : prompt\n  Busy --> Idle : done\n  Idle --> [*]";
    let paint = DiagramStyle::neutral(false);
    for (i, _) in src.char_indices() {
        let _ = crate::scene(&src[..i], &paint, None);
    }
    assert!(crate::scene(src, &paint, None).is_some());
}
