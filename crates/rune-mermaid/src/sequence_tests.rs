use super::*;

fn seq(src: &str) -> Sequence {
    match crate::parse(src) {
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
fn notes_frames_and_explicit_activation() {
    let s = seq(
        "sequenceDiagram\nparticipant web-app\nNote right of web-app: thinks\nnote over web-app,db: shared<br>two lines\nloop Every minute\n  web-app->>db: ping\nend\nalt ok\n  activate db\n  db-->>web-app: rows\n  deactivate db\nelse error\n  db-->>web-app: 500\nend\nrect rgb(0,0,0)\nopt maybe\nweb-app->db: x\nend\nend",
    );
    assert_eq!(s.participants[0].id, "web-app", "dashes in names");
    assert!(matches!(s.events[0], Event::Note(NotePos::Right(0), ref t) if t == "thinks"));
    assert!(matches!(s.events[1], Event::Note(NotePos::Over(0, 1), ref t) if t == "shared\ntwo lines"));
    assert!(matches!(s.events[2], Event::Open(Some(ref k), ref l) if k == "loop" && l == "Every minute"));
    assert!(s.events.iter().any(|e| matches!(e, Event::Else(l) if l == "error")));
    assert!(s.events.iter().any(|e| matches!(e, Event::Activate(1))));
    assert!(s.events.iter().any(|e| matches!(e, Event::Open(None, _))));
    assert_eq!(s.events.iter().filter(|e| matches!(e, Event::End)).count(), 4);
}

#[test]
fn malformed_and_unsupported_input_is_rejected() {
    for src in [
        "sequenceDiagram",
        "sequenceDiagram\nA something B",
        "sequenceDiagram\n->>B: no sender",
        "sequenceDiagram\nA->>: no receiver",
        "sequenceDiagram\nend",
        "sequenceDiagram\nelse x",
        "sequenceDiagram\nNote sideways of A: x",
        "sequenceDiagram\nNote over A no colon",
        "sequenceDiagram\nparticipant A@{ \"type\": \"database\" }",
    ] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
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
