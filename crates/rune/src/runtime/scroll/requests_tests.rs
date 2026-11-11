use super::*;

#[derive(Clone, Copy, Debug)]
enum Input {
    CaptureKey(&'static str, bool), Fold, Place { viewport: &'static str, first: bool, to: f64, at_end: bool },
    Tail, EndOwns, Frame { glide: bool, moving: bool, mounted: bool }, InputScroll, Destination,
}
use Input::*;

#[derive(Clone, Debug, Default, PartialEq)]
struct Old {
    keeps: Vec<OldKeep>,
    scroll_reveals: Vec<OldReveal>,
}
#[derive(Clone, Debug, PartialEq)]
struct OldKeep { path: String, viewport: String, was: f32, virtual_row: Option<(String, String, f32)>, hold: Option<(String, f32)> }
#[derive(Clone, Debug, PartialEq)]
struct OldReveal { viewport: String, scope: Option<String>, key: String, center: bool, at_end: bool }

#[derive(Debug, PartialEq)]
enum Placement { None, Rest, Move(f64) }

// The old fields and mutations, independent of the request machine. The placement event
// supplies the already measured offset, just as reveal_row_once did after clamping it.
impl Old {
    fn apply(&mut self, input: Input) -> Placement {
        match input {
            CaptureKey(viewport, virtualized) => {
                self.keeps.retain(|k| k.viewport != viewport);
                self.scroll_reveals.retain(|r| r.viewport != viewport);
                let row = virtualized.then(|| ("list".into(), "row".into(), 3.0));
                self.keeps.push(OldKeep { path: "item".into(), viewport: viewport.into(), was: 7.0, virtual_row: row,
                    hold: Some(("header".into(), 1.0)) });
            }
            Fold => {
                self.keeps.retain(|k| k.viewport != "a");
                self.keeps.push(OldKeep { path: "below".into(), viewport: "a".into(), was: 7.0, virtual_row: None, hold: None });
            }
            Place { viewport, first, to, at_end } => {
                if first {
                    self.scroll_reveals.retain(|r| r.viewport != viewport);
                    self.scroll_reveals.push(OldReveal { viewport: viewport.into(), scope: Some("scope".into()),
                        key: "mark".into(), center: viewport == "a", at_end });
                } else if let Some(r) = self.scroll_reveals.iter_mut().find(|r| r.viewport == viewport) {
                    r.at_end |= at_end;
                }
                if (to - 5.0).abs() < 0.5 {
                    if first { return Placement::Rest; }
                    return Placement::None;
                }
                return Placement::Move(to);
            }
            Tail => if let Some(r) = self.scroll_reveals.iter_mut().find(|r| r.viewport == "a" && r.at_end) {
                r.scope = None;
                r.key = "new".into();
            },
            EndOwns => self.keeps.retain(|k| k.viewport != "a"),
            Frame { glide, moving, mounted } => {
                if self.keeps.is_empty() && self.scroll_reveals.is_empty() { return Placement::None; }
                let animating = |viewport: &str| viewport == "a" && moving;
                let gliding = |viewport: &str| viewport == "b" && glide;
                self.keeps.retain(|k| animating(&k.viewport));
                self.scroll_reveals.retain(|r| gliding(&r.viewport) || (animating(&r.viewport) && mounted));
            }
            InputScroll => { self.keeps.clear(); self.scroll_reveals.clear(); }
            Destination => self.scroll_reveals.clear(),
        }
        Placement::None
    }
}

fn drive(requests: &mut Requests, input: Input) -> Placement {
    let anchor = |v: &str, p: &str| super::Keep { path: p.into(), viewport: v.into(), was: 7.0,
        hold: (p == "item").then(|| super::super::sticky::Hold { path: "header".into(), inset: 1.0 }) };
    let effects = match input {
        CaptureKey(v, row) => requests.step(Event::Capture(anchor(v, "item"), row.then(|| ("list".into(), "row".into(), 3.0)))),
        Fold => requests.step(Event::FoldAnchor(anchor("a", "below"))),
        Place { viewport, first, to, at_end } => requests.step(Event::Place { viewport, scope: Some("scope"),
            key: "mark", center: viewport == "a", first, at_end, from: 5.0, to }),
        Tail => requests.step(Event::NewTail { viewport: "a", key: "new" }),
        EndOwns => requests.step(Event::RetainKeeps(&|k| k.viewport != "a")),
        Frame { glide, moving, mounted } => requests.step(Event::Finish { gliding: &|v| v == "b" && glide,
            animating: &|v| v == "a" && moving, mounted: &|_| mounted }),
        InputScroll => requests.step(Event::Cancel),
        Destination => requests.step(Event::Destination),
    };
    match effects {
        Effects::None => Placement::None, Effects::One(Effect::Rest) => Placement::Rest,
        Effects::One(Effect::Move(to)) => Placement::Move(to), _ => panic!("unexpected placements"),
    }
}

#[test]
fn every_request_sequence_matches_the_separate_keep_and_reveal_lists() {
    const INPUTS: &[Input] = &[CaptureKey("a", false), CaptureKey("b", true), Fold,
        Place { viewport: "a", first: true, to: 4.5, at_end: false },
        Place { viewport: "b", first: true, to: 5.25, at_end: true },
        Place { viewport: "a", first: false, to: 5.0, at_end: true }, Tail, EndOwns,
        Frame { glide: true, moving: false, mounted: false }, Frame { glide: false, moving: true, mounted: false },
        Frame { glide: false, moving: true, mounted: true }, InputScroll, Destination];
    fn walk(old: &Old, real: &Requests, sequence: &mut Vec<Input>) {
        if sequence.len() == 5 { return; }
        for &input in INPUTS {
            sequence.push(input);
            let (mut old, mut real) = (old.clone(), real.clone());
            assert_eq!(drive(&mut real, input), old.apply(input), "{sequence:?}");
            let snapshot = Old { keeps: real.anchors().map(|(k, row)| OldKeep { path: k.path.clone(), viewport: k.viewport.clone(),
                was: k.was, virtual_row: row.cloned(), hold: k.hold.as_ref().map(|h| (h.path.clone(), h.inset)) }).collect(),
                scroll_reveals: real.reveals().map(|r| OldReveal { viewport: r.viewport.clone(), scope: r.scope.clone(),
                    key: r.key.clone(), center: r.center, at_end: r.at_end }).collect() };
            assert_eq!(snapshot, old, "{sequence:?}");
            walk(&old, &real, sequence);
            sequence.pop();
        }
    }
    walk(&Old::default(), &Requests::default(), &mut Vec::new());
}
