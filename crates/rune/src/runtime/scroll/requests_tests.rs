use super::super::sticky::{Hold, Keep};
use super::*;

/// What a frame measured about the keeps' elements: the one element in the tree, whether a
/// viewport surrounds it, whether that viewport is pinned, and how far down it was drawn.
#[derive(Clone, Copy, Debug)]
struct Scene { mounted: Option<&'static str>, viewport: bool, pinned: bool, top: f32 }

#[derive(Clone, Copy, Debug)]
enum Input {
    CaptureKey { viewport: &'static str, virtualized: bool, header: Option<&'static str> }, Fold,
    FoldOpen { viewport: &'static str, mounted: Option<&'static str>, end: usize },
    Place { viewport: &'static str, first: bool, to: f64, at_end: bool }, Tail,
    EndGives { header: &'static str, pinned: &'static str }, EndClamp(Scene),
    Frame { glide: &'static str, moving: &'static str, mounted: bool }, InputScroll, Destination,
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
            CaptureKey { viewport, virtualized, header } => {
                self.keeps.retain(|k| k.viewport != viewport);
                self.scroll_reveals.retain(|r| r.viewport != viewport);
                let row = virtualized.then(|| ("list".into(), "row".into(), 3.0));
                self.keeps.push(OldKeep { path: "item".into(), viewport: viewport.into(), was: 7.0, virtual_row: row,
                    hold: header.map(|h| (h.into(), 1.0)) });
            }
            Fold => {
                self.keeps.retain(|k| k.viewport != "a");
                self.keeps.push(OldKeep { path: "below".into(), viewport: "a".into(), was: 7.0, virtual_row: None, hold: None });
            }
            // fold_hold.rs: a fold opening.
            FoldOpen { viewport, mounted, end } => {
                let nodes = ["root", mounted.unwrap_or("absent")];
                self.keeps.retain(|k| k.viewport != viewport || nodes.iter().position(|n| *n == k.path).is_none_or(|i| i >= end));
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
            // fold_hold.rs: end_takes_over.
            EndGives { header, pinned: on } => {
                let header = header.to_string();
                let giving: Vec<&String> = vec![&header];
                let pinned = |viewport: &str| viewport == on;
                let drop: Vec<usize> = self.keeps.iter().enumerate()
                    .filter(|(_, k)| k.hold.as_ref().is_some_and(|h| giving.contains(&&h.0)) && pinned(&k.viewport)).map(|(i, _)| i).collect();
                for i in drop.into_iter().rev() { self.keeps.remove(i); }
            }
            // sticky.rs: the end of follow_keeps.
            EndClamp(Scene { mounted, viewport, pinned, top }) => {
                let nodes = [mounted.unwrap_or("absent"), "viewport"];
                let drawn = [top, 0.0];
                let viewport_of = |_: usize| viewport.then_some(1);
                let scrolls = [(1, pinned)];
                let mut keeps = std::mem::take(&mut self.keeps);
                keeps.retain(|k| {
                    let Some(i) = nodes.iter().position(|n| *n == k.path) else { return true };
                    let Some(s) = viewport_of(i) else { return false };
                    !(scrolls.iter().find(|st| st.0 == s).is_some_and(|st| st.1)
                        && drawn[i] - drawn[s] > k.was + 0.5)
                });
                self.keeps = keeps;
            }
            Frame { glide, moving, mounted } => {
                if self.keeps.is_empty() && self.scroll_reveals.is_empty() { return Placement::None; }
                let animating = |viewport: &str| viewport == moving;
                let gliding = |viewport: &str| viewport == glide;
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
    let anchor = |v: &str, p: &str, header: Option<&str>| Keep { path: p.into(), viewport: v.into(), was: 7.0, virtual_row: None,
        hold: header.map(|h| Hold { path: h.into(), inset: 1.0 }) };
    let effects = match input {
        CaptureKey { viewport, virtualized, header } => requests.step(Event::Arrive(Request::Capture(Keep { virtual_row:
            virtualized.then(|| ("list".into(), "row".into(), 3.0)), ..anchor(viewport, "item", header) }))),
        Fold => requests.step(Event::Arrive(Request::FoldAnchor(anchor("a", "below", None)))),
        FoldOpen { viewport, mounted, end } => requests.step(FoldOpens { viewport, end,
            index: |path: &str| (Some(path) == mounted).then_some(1) }),
        Place { viewport, first, to, at_end } => requests.step(Event::Place { viewport, scope: Some("scope"),
            key: "mark", center: viewport == "a", first, at_end, from: 5.0, to }),
        Tail => requests.step(Event::NewTail { viewport: "a", key: "new" }),
        EndGives { header, pinned } => requests.step(EndTakesOver { giving: &[&header.to_string()],
            pinned: |v: &str| v == pinned }),
        EndClamp(Scene { mounted, viewport, pinned, top }) => requests.step(EndClamps { shown: |k: &Keep| {
            if mounted != Some(k.path.as_str()) { Shown::Unmounted }
            else if !viewport { Shown::Loose }
            else if !pinned { Shown::Free }
            else { Shown::Pinned(top) }
        } }),
        Frame { glide, moving, mounted } => requests.step(Finish { gliding: |v: &str| v == glide,
            animating: |v: &str| v == moving, mounted: |_: &Reveal| mounted }),
        InputScroll => requests.step(Event::Cancel),
        Destination => requests.step(Event::Destination),
    };
    match effects {
        Effects::None => Placement::None, Effects::One(Effect::Rest) => Placement::Rest,
        Effects::One(Effect::Move(to)) => Placement::Move(to), _ => panic!("unexpected placements"),
    }
}

fn walk(inputs: &[Input], old: &Old, real: &Requests, sequence: &mut Vec<Input>) {
    if sequence.len() == 5 { return; }
    for &input in inputs {
        sequence.push(input);
        let (mut old, mut real) = (old.clone(), real.clone());
        assert_eq!(drive(&mut real, input), old.apply(input), "{sequence:?}");
        let snapshot = Old { keeps: real.keeps().map(|k| OldKeep { path: k.path.clone(), viewport: k.viewport.clone(),
            was: k.was, virtual_row: k.virtual_row.clone(), hold: k.hold.as_ref().map(|h| (h.path.clone(), h.inset)) }).collect(),
            scroll_reveals: real.reveals().map(|r| OldReveal { viewport: r.viewport.clone(), scope: r.scope.clone(),
                key: r.key.clone(), center: r.center, at_end: r.at_end }).collect() };
        assert_eq!(snapshot, old, "{sequence:?}");
        walk(inputs, &old, &real, sequence);
        sequence.pop();
    }
}

const HELD_A: Input = CaptureKey { viewport: "a", virtualized: false, header: Some("header") };
const PINNED_PAST: Scene = Scene { mounted: Some("below"), viewport: true, pinned: true, top: 7.75 };

#[test]
fn every_request_sequence_matches_the_separate_keep_and_reveal_lists() {
    const INPUTS: &[Input] = &[HELD_A, CaptureKey { viewport: "b", virtualized: true, header: Some("header") }, Fold,
        Place { viewport: "a", first: true, to: 4.5, at_end: false },
        Place { viewport: "b", first: true, to: 5.25, at_end: true },
        Place { viewport: "a", first: false, to: 5.0, at_end: true }, Tail,
        EndGives { header: "header", pinned: "a" }, EndClamp(PINNED_PAST),
        Frame { glide: "b", moving: "", mounted: false }, Frame { glide: "", moving: "a", mounted: false },
        Frame { glide: "", moving: "a", mounted: true }, Frame { glide: "a", moving: "b", mounted: true },
        InputScroll, Destination];
    walk(INPUTS, &Old::default(), &Requests::default(), &mut Vec::new());
}

/// Each release condition, on anchors that must stay and anchors that must go.
#[test]
fn every_keep_release_matches_the_old_conditions() {
    const INPUTS: &[Input] = &[HELD_A, CaptureKey { viewport: "a", virtualized: false, header: None },
        CaptureKey { viewport: "b", virtualized: false, header: Some("header") }, Fold,
        Place { viewport: "a", first: true, to: 4.5, at_end: false },
        EndGives { header: "header", pinned: "a" }, EndGives { header: "header", pinned: "" },
        EndGives { header: "other", pinned: "a" },
        EndClamp(Scene { mounted: None, viewport: true, pinned: true, top: 9.0 }),
        EndClamp(Scene { mounted: Some("item"), viewport: false, pinned: false, top: 7.0 }),
        EndClamp(Scene { mounted: Some("item"), viewport: true, pinned: false, top: 9.0 }),
        EndClamp(Scene { mounted: Some("below"), viewport: true, pinned: true, top: 7.5 }),
        EndClamp(PINNED_PAST),
        FoldOpen { viewport: "a", mounted: Some("item"), end: 2 },
        FoldOpen { viewport: "a", mounted: Some("item"), end: 1 },
        FoldOpen { viewport: "a", mounted: None, end: 2 },
        FoldOpen { viewport: "b", mounted: Some("item"), end: 2 }];
    walk(INPUTS, &Old::default(), &Requests::default(), &mut Vec::new());
}
