use super::*;

fn seq(src: &str) -> Sequence {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Sequence(s)) => s,
        _ => panic!("not a sequence diagram: {src:?}"),
    }
}

#[test]
fn participants_messages_and_activation() {
    let s = seq(
        "sequenceDiagram\n  autonumber\n  actor U as User\n  participant A as API server\n  U->>+A: GET /items\n  A-->>-U: 200 OK\n  A->B: plain\n  B-->A: dotted\n  A-xB: lost\n  B--)A: async\n  A<<->>B: both\n  A->>A: think",
    );
    assert!(s.autonumber);
    let names: Vec<(&str, &str, bool)> = s.participants.iter().map(|p| (p.id.as_str(), p.label.as_str(), p.actor)).collect();
    assert_eq!(names, [("U", "User", true), ("A", "API server", false), ("B", "B", false)]);
    let msgs: Vec<&Message> = s.events.iter().filter_map(|e| if let Event::Message(m) = e { Some(m) } else { None }).collect();
    assert_eq!((msgs[0].from, msgs[0].to, msgs[0].text.as_str(), msgs[0].activate), (0, 1, "GET /items", true));
    assert_eq!((msgs[1].dotted, msgs[1].tip, msgs[1].deactivate), (true, Tip::Filled, true));
    assert_eq!((msgs[2].dotted, msgs[2].tip), (false, Tip::None));
    assert_eq!((msgs[3].dotted, msgs[3].tip), (true, Tip::None));
    assert_eq!(msgs[4].tip, Tip::Cross);
    assert_eq!((msgs[5].dotted, msgs[5].tip), (true, Tip::Open));
    assert!(msgs[6].both);
    assert_eq!((msgs[7].from, msgs[7].to), (1, 1));
}

#[test]
fn notes_frames_boxes_and_explicit_activation() {
    let s = seq(
        "sequenceDiagram\n\
         title Frame commit\n\
         box rgb(30, 60, 90) Desk\n\
           participant web-app\n\
         end\n\
         box Storage\n\
           participant db\n\
         end\n\
         Note right of web-app: thinks\n\
         note over web-app,db: shared<br>two lines\n\
         loop Every minute\n\
           web-app->>db: ping\n\
         end\n\
         alt ok\n\
           activate db\n\
           db-->>web-app: rows\n\
           deactivate db\n\
         else error\n\
           db-->>web-app: 500\n\
         end\n\
         rect rgb(40, 80, 60)\n\
           opt maybe\n\
             web-app->db: x\n\
           end\n\
         end",
    );
    assert_eq!(s.title.as_deref(), Some("Frame commit"));
    assert_eq!(s.boxes.len(), 2);
    assert_eq!(s.boxes[0].title, "Desk");
    assert!(s.boxes[0].color.is_some());
    assert_eq!(s.participants[0].group, Some(0));
    assert_eq!(s.participants[1].group, Some(1));
    assert_eq!(s.participants[0].id, "web-app", "dashes in names");
    assert!(matches!(s.events[0], Event::Note(NotePos::Right(0), ref t) if t == "thinks"));
    assert!(matches!(s.events[1], Event::Note(NotePos::Over(0, 1), ref t) if t == "shared\ntwo lines"));
    assert!(matches!(s.events[2], Event::Open { kind: Some(ref k), ref label, .. } if k == "loop" && label == "Every minute"));
    assert!(s.events.iter().any(|e| matches!(e, Event::Else(l) if l == "error")));
    assert!(s.events.iter().any(|e| matches!(e, Event::Activate(1))));
    assert!(s.events.iter().any(|e| matches!(e, Event::Open { kind: None, fill: Some(_), .. })));
    assert_eq!(s.events.iter().filter(|e| matches!(e, Event::End)).count(), 4);
    assert!(draw(&s, &DiagramStyle::neutral(true)).is_some());
}

#[test]
fn only_sources_where_nothing_reads_are_rejected() {
    for src in ["sequenceDiagram", "sequenceDiagram\nA something B", "sequenceDiagram\n->>B: no sender\nend\nelse x"] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
}

#[test]
fn a_line_that_does_not_read_is_skipped() {
    let msgs = |src: &str| seq(src).events.iter().filter(|e| matches!(e, Event::Message(_))).count();
    for src in [
        "sequenceDiagram\nA something B\nA->>B: hi",
        "sequenceDiagram\n->>B: no sender\nA->>B: hi",
        "sequenceDiagram\nA->>: no receiver\nA->>B: hi",
        "sequenceDiagram\nend\nelse x\nA->>B: hi",
        "sequenceDiagram\nNote sideways of A: x\nNote over A no colon\nA->>B: hi",
    ] {
        assert_eq!(msgs(src), 1, "{src:?}");
        assert_eq!(seq(src).participants.len(), 2, "a line that fails adds no participants: {src:?}");
    }
    let s = seq("sequenceDiagram\nloop unclosed\n  A->>B: hi\nbox unclosed\n  participant C");
    assert!(matches!(s.events.last(), Some(Event::End)), "an open frame closes at the end");
    assert_eq!(s.participants[2].group, Some(0));
    let s = seq("sequenceDiagram\nparticipant A@{ \"type\": \"database\" }\nA->>B: hi");
    assert_eq!(s.participants[0].label, "A");
}

#[test]
fn nesting_past_the_bound_still_rejects() {
    let deep = "loop x\n".repeat(crate::MAX_DEPTH + 1);
    assert!(crate::parse(&format!("sequenceDiagram\n{deep}A->>B: hi")).is_none());
}

#[test]
fn every_prefix_draws_or_falls_back_without_panicking() {
    let src = "sequenceDiagram\n  autonumber\n  actor U as User\n  U->>+A: hi\n  loop retry\n    A->>B: call\n    B--xA: fail\n  end\n  Note left of U: waits\n  A-->>-U: done";
    let paint = DiagramStyle::neutral(true);
    for (i, _) in src.char_indices() {
        let _ = crate::scene(&src[..i], &paint, None);
    }
    assert!(crate::scene(src, &paint, None).is_some());
}

#[test]
fn long_messages_widen_their_gap() {
    let paint = DiagramStyle::neutral(false);
    let narrow = crate::scene("sequenceDiagram\nA->>B: hi", &paint, None).unwrap();
    let wide = crate::scene("sequenceDiagram\nA->>B: a much longer message that needs room", &paint, None).unwrap();
    assert!(wide.size[0] > narrow.size[0] + 100.0, "{:?} vs {:?}", wide.size, narrow.size);
}
