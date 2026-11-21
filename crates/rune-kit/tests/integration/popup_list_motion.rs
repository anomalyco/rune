//! A `PopupList` above its field (`Side::Top`) holds its bottom while its height springs, so its
//! rows ride its top. Filtering to one row and then growing again mid-exit must move every row
//! one way to its slot.
use rune::{Align, Cx, El, Env, Runtime, Side, col, el};
use rune_kit::{Kit, PopupItem, PopupList, root};

struct S {
    ids: Vec<String>,
    padding: Option<f32>,
    row_radius: Option<f32>,
    animate: bool,
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const SIX: [&str; 6] = ["a0", "a1", "a2", "a3", "a4", "a5"];
const TEN: [&str; 10] = ["a1", "b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8"];

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let mut list = PopupList::<S>::new("p", "field")
        .anchored("field", Side::Top, Align::Start, 8.0)
        .max_height(300.0)
        .items(s.ids.iter().map(|i| PopupItem::new(i.clone(), i.clone())))
        .animate_filtering(s.animate)
        .active(Some(0));
    if let Some(p) = s.padding {
        list = list.padding(p);
    }
    if let Some(r) = s.row_radius {
        list = list.row_radius(r);
    }
    list.build(&k, &mut layer);
    root(col().pt(400.0).pl(40.0).child(el().key("field").w(300.0).h(60.0)), layer)
}

fn settled(items: &[&str]) -> Runtime<S> {
    settled_with(items, true)
}

fn settled_with(items: &[&str], animate: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { ids: ids(items), padding: None, row_radius: None, animate }, view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
    }
    rt
}

fn y(rt: &Runtime<S>, key: &str) -> f32 {
    rt.presented(key).unwrap_or_else(|| panic!("{key} is presented")).0[1]
}

/// Grows the list to `TEN` and steps 90 frames, checking that the popover's top and every row
/// of the new list only rise, and land where the list settles.
fn grows_without_overshoot(rt: &mut Runtime<S>) {
    rt.update(|s, _| s.ids = ids(&TEN));
    let mut keys: Vec<String> = TEN.iter().map(|id| format!("p-option-{id}")).collect();
    keys.push("p-content".into());
    let mut tracks: Vec<Vec<f32>> = vec![Vec::new(); keys.len()];
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        for (key, track) in keys.iter().zip(&mut tracks) {
            track.push(y(rt, key));
        }
    }
    for (key, track) in keys.iter().zip(&tracks) {
        let lowest = track.iter().copied().fold(f32::MAX, f32::min);
        let end = *track.last().unwrap();
        for (f, pair) in track.windows(2).enumerate() {
            assert!(pair[1] <= pair[0] + 0.05, "{key} sank {:.2} -> {:.2} at frame {}: {track:?}", pair[0], pair[1], f + 1);
        }
        assert!(end - lowest < 0.05, "{key} rose {:.1} pt past its slot", end - lowest);
    }
    // The popover's bottom sits 8 points over the field at y 400; rows start inside its 4 point padding.
    assert!((y(rt, "p-content") - 104.0).abs() < 0.5, "popover top {}", y(rt, "p-content"));
    for (i, id) in TEN.iter().enumerate() {
        let want = 108.0 + 28.0 * i as f32;
        assert!((y(rt, &format!("p-option-{id}")) - want).abs() < 0.5, "{id} settles at {want}");
    }
}

#[test]
fn rows_rise_to_their_slots_while_filtered_out_rows_still_leave() {
    // As a chat composer's mention popover: six rows, a keystroke filters them to one, and
    // 0.17 s later more results arrive under it while the five removed rows still collapse.
    let mut rt = settled(&SIX);
    rt.update(|s, _| s.ids = ids(&["a1"]));
    for _ in 0..20 {
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.presented("p-option-a2").is_some(), "removed rows are still leaving when the results arrive");
    grows_without_overshoot(&mut rt);
}

#[test]
fn rows_rise_to_their_slots_when_nothing_is_leaving() {
    let mut rt = settled(&SIX);
    rt.update(|s, _| s.ids = ids(&["a1"]));
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.presented("p-option-a2").is_none());
    grows_without_overshoot(&mut rt);
}

#[test]
fn an_instant_commit_swaps_rows_and_height_at_once() {
    swaps_rows_and_height_at_once(true);
}

#[test]
fn by_default_any_commit_swaps_rows_and_height_at_once() {
    swaps_rows_and_height_at_once(false);
}

/// An animated list in `Tx::instant` commits, or a default one in commits that animate.
fn swaps_rows_and_height_at_once(animate: bool) {
    let mut rt = settled_with(&SIX, animate);
    rt.update(|s, tx| {
        s.ids = ids(&["a1"]);
        if animate { tx.instant(); }
    });
    rt.frame(1.0 / 120.0);
    for id in ["a0", "a2", "a3", "a4", "a5"] {
        assert!(rt.presented(&format!("p-option-{id}")).is_none(), "{id} is gone at once");
    }
    // One row: a 36 point popover (28 + 2 × 4) whose bottom stays at 392.
    assert!((y(&rt, "p-content") - 356.0).abs() < 0.05, "popover top {}", y(&rt, "p-content"));
    assert!((y(&rt, "p-option-a1") - 360.0).abs() < 0.05);

    rt.update(|s, tx| {
        s.ids = ids(&TEN);
        if animate { tx.instant(); }
    });
    rt.frame(1.0 / 120.0);
    assert!((y(&rt, "p-content") - 104.0).abs() < 0.05, "popover top {}", y(&rt, "p-content"));
    for (i, id) in TEN.iter().enumerate() {
        assert!((y(&rt, &format!("p-option-{id}")) - (108.0 + 28.0 * i as f32)).abs() < 0.05, "{id} lands at once");
    }
}

#[test]
fn padding_and_row_radius_default_to_4_and_follow_their_builders() {
    let row = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("p-option-a0")).unwrap();
    let content = |rt: &Runtime<S>| rt.presented("p-content").unwrap().0;

    let rt = settled(&SIX);
    let (c, r) = (content(&rt), row(&rt));
    assert_eq!(r.radius, 4.0);
    assert!((r.rect[0] - c[0] - 4.0).abs() < 0.05 && (r.rect[1] - c[1] - 4.0).abs() < 0.05);
    assert!((c[3] - (6.0 * 28.0 + 8.0)).abs() < 0.05);

    let mut rt = Runtime::new(S { ids: ids(&SIX), padding: Some(6.0), row_radius: Some(6.0), animate: false }, view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (c, r) = (content(&rt), row(&rt));
    assert_eq!(r.radius, 6.0);
    assert!((r.rect[0] - c[0] - 6.0).abs() < 0.05 && (r.rect[1] - c[1] - 6.0).abs() < 0.05);
    assert!((r.rect[2] - (c[2] - 12.0)).abs() < 0.05, "rows fill the width inside the padding");
    assert!((c[3] - (6.0 * 28.0 + 12.0)).abs() < 0.05);
    assert!((c[1] + c[3] - 392.0).abs() < 0.05, "the bottom stays 8 points over the field");
}

/// Sixty rows: the 300 point popover shows the first ten and keeps the rows below it mounted,
/// clipped away.
fn settled_long() -> Runtime<S> {
    let ids = (0..60).map(|i| format!("a{i}")).collect();
    let mut rt = Runtime::new(S { ids, padding: None, row_radius: None, animate: true }, view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
    }
    rt
}

#[test]
fn filtered_out_rows_clipped_away_leave_at_once() {
    let mut rt = settled_long();
    assert!(rt.presented("p-option-a20").is_some(), "a row below the viewport is mounted");
    rt.update(|s, _| s.ids = ids(&["a0"]));
    rt.frame(1.0 / 120.0);
    assert!(rt.presented("p-option-a20").is_none(), "a row nobody sees has no fade to show");
    for _ in 0..4 {
        rt.frame(1.0 / 120.0);
    }
    let (_, opacity) = rt.presented("p-option-a5").expect("a row in view is still leaving");
    assert!(opacity > 0.05 && opacity < 1.0, "it fades: {opacity}");
}

#[test]
fn rows_a_filter_brings_into_view_fade_in() {
    let mut rt = settled_long();
    assert!(rt.presented("p-option-a50").is_none(), "far down the list, not mounted");
    rt.update(|s, _| s.ids = ids(&["a50", "a51"]));
    rt.frame(1.0 / 120.0);
    let (_, opacity) = rt.presented("p-option-a50").expect("the filter brings it into view");
    assert!(opacity < 0.9, "it enters as its leaving neighbours exit, not at full opacity: {opacity}");
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.presented("p-option-a50").unwrap().1 > 0.99);
}
