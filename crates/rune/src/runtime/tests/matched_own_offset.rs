use super::*;
use crate::{Color, Live};

/// A matched tile at the origin, or its match placed at (200, 100); each with its own offset.
struct Tiles {
    open: bool,
    src: Live<[f32; 2]>,
    dst: Live<[f32; 2]>,
    live: bool,
}

fn view(s: &Tiles, _: &mut Cx<Tiles>) -> El {
    let tile = |key: &str, off: &Live<[f32; 2]>| {
        let t = el().key(key).matched("tile").size(40.0, 40.0).fill(Color::WHITE);
        if s.live { t.live_offset(off) } else { t.offset(off.get()[0], off.get()[1]) }
    };
    let root = col().size(400.0, 300.0);
    if s.open { root.child(tile("b", &s.dst).absolute(200.0, 100.0)) } else { root.child(tile("a", &s.src)) }
}

/// Where `a` was seen, where `b` shows on the flight's first frame, and where `b` comes to rest.
fn fly(src: [f32; 2], dst: [f32; 2], live: bool) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let state = Tiles { open: false, src: Live::new(src), dst: Live::new(dst), live };
    let mut rt = Runtime::new(state, view, (400.0, 300.0), Env::new(true, 1.0));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let seen = rt.presented("a").expect("a").0;
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 1000.0);
    let first = rt.presented("b").expect("b").0;
    for _ in 0..120 { rt.frame(1.0 / 60.0); }
    (seen, first, rt.presented("b").expect("b").0)
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5)
}

#[test]
fn a_matched_flight_starts_where_its_source_was_seen_and_ends_under_its_own_offset() {
    for live in [false, true] {
        for (src, dst) in [([30.0, 20.0], [0.0, 0.0]), ([0.0, 0.0], [-50.0, 40.0]), ([30.0, 20.0], [-50.0, 40.0])] {
            let (seen, first, rest) = fly(src, dst, live);
            assert_eq!(seen, [src[0], src[1], 40.0, 40.0], "live={live}: the source shows under its offset");
            assert!(near(first, seen), "live={live} src={src:?} dst={dst:?}: first frame {first:?}, seen at {seen:?}");
            assert_eq!(rest, [200.0 + dst[0], 100.0 + dst[1], 40.0, 40.0], "live={live}: rests under its own offset");
        }
    }
}
