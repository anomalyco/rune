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
fn notes_sit_beside_their_state_on_a_dotted_line() {
    let s = state("stateDiagram-v2\n  Idle --> Busy\n  note right of Idle : waits here\n  note left of Busy\n    does the work\n    then stops\n  end note");
    let notes: Vec<&str> = s.states.iter().filter(|st| st.kind == StateKind::Note).map(|st| st.label.as_str()).collect();
    assert_eq!(notes, ["waits here", "does the work\nthen stops"]);
    let dotted: Vec<(&str, &str)> =
        s.edges.iter().filter(|e| e.line == Line::Dotted).map(|e| (s.states[e.from].id.as_str(), s.states[e.to].id.as_str())).collect();
    assert_eq!(dotted.len(), 2);
    assert_eq!(dotted[0].0, "Idle", "a right note follows its state");
    assert_eq!(dotted[1].1, "Busy", "a left note comes before it");
    assert!(s.edges.iter().filter(|e| e.line == Line::Dotted).all(|e| e.end == Head::None));
    assert!(draw(&s, &DiagramStyle::neutral(false), None).is_some());
    let open = state("stateDiagram-v2\nnote right of A\n  never closed");
    assert_eq!(open.states.iter().filter(|st| st.kind == StateKind::Note).count(), 1, "a note left open closes at the end");
}

#[test]
fn concurrent_regions_split_a_composite() {
    let s = state(
        "stateDiagram-v2\n  [*] --> Active\n  state Active {\n    [*] --> NumOff\n    NumOff --> NumOn\n    --\n    [*] --> CapsOff\n    CapsOff --> CapsOn\n    --\n    [*] --> ScrollOff\n  }\n  Active --> Done",
    );
    let regions: Vec<usize> = (0..s.composites.len()).filter(|&i| s.composites[i].region).collect();
    assert_eq!(regions.len(), 3);
    assert!(regions.iter().all(|&r| s.composites[r].parent == Some(0)));
    let region_of = |id: &str| s.states.iter().find(|st| st.id == id).unwrap().cluster;
    assert_eq!(region_of("NumOn"), Some(regions[0]));
    assert_eq!(region_of("CapsOn"), Some(regions[1]));
    assert_eq!(region_of("ScrollOff"), Some(regions[2]));
    assert_eq!(region_of("Done"), None, "the closing brace closes the region and the composite");
    assert!(!s.states.iter().any(|st| st.id == "Active"), "the state named before its composite is the composite");
    let id = |i: usize| s.states[i].id.as_str();
    let edges = |line: Line| s.edges.iter().filter(move |e| e.line == line);
    let drawn: Vec<(&str, &str)> = edges(Line::Solid).filter(|e| id(e.to) == "Done").map(|e| (id(e.from), id(e.to))).collect();
    assert_eq!(drawn, [("NumOn", "Done")], "an edge out leaves from the first region's last state");
    let held: Vec<(&str, &str)> = edges(Line::Invisible).filter(|e| id(e.to) == "Done").map(|e| (id(e.from), id(e.to))).collect();
    assert_eq!(held, [("CapsOn", "Done"), ("ScrollOff", "Done")], "the other regions are held above it");
    assert_eq!(edges(Line::Invisible).filter(|e| s.states[e.from].kind == StateKind::Start && s.states[e.from].cluster.is_none()).count(), 2);
    assert!(draw(&s, &DiagramStyle::neutral(true), None).is_some());
    let back = state("stateDiagram-v2\n  [*] --> Busy\n  state Busy {\n    [*] --> A\n    --\n    [*] --> B\n  }\n  Busy --> Check\n  Check --> Busy : retry");
    let held = |s: &StateDiagram| s.edges.iter().filter(|e| e.line == Line::Invisible).count();
    assert_eq!(held(&back), 2, "the entry and the exit hold the second region; the retry back into it holds nothing");
}

#[test]
fn fork_join_and_choice() {
    let s = state("stateDiagram-v2\n  state f <<fork>>\n  state j <<join>>\n  state c <<choice>>\n  [*] --> f\n  f --> A\n  f --> B\n  A --> j\n  B --> j\n  j --> c\n  c --> [*]");
    let kind = |id: &str| s.states.iter().find(|st| st.id == id).unwrap().kind;
    assert_eq!((kind("f"), kind("j"), kind("c")), (StateKind::Bar, StateKind::Bar, StateKind::Choice));
    assert!(draw(&s, &DiagramStyle::neutral(false), None).is_some());
}

#[test]
fn only_sources_where_nothing_reads_are_rejected() {
    for src in ["stateDiagram-v2", "stateDiagram\n}", "stateDiagram-v2\n-->\n--"] {
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
    crate::tests::assert_prefixes_never_panic(
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : prompt\n  Busy --> Idle : done\n  Idle --> [*]",
        false,
    );
}
