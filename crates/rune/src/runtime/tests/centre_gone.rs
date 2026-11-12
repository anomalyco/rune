//! A `Tx::scroll_center` glide stops once its target is removed for good, and glides on
//! exactly as before while the target is only away: a virtual row out of the window, or a child
//! of one, a child remounted under its key, or one whose exit is undone while it plays.
use super::*;

const DT: f64 = 1.0 / 60.0;

/// 50-point rows keyed `r{k}` in a 200-point viewport, as plain children or virtual rows.
struct S { rows: Vec<usize>, virtual_rows: bool }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").w_full();
    let rows = if s.virtual_rows {
        // Row 10 holds an element keyed `mark`.
        rows.growth(crate::el::GrowthSpec::default()).virtual_rows(s.rows.len(), 50.0, |s: &S, i| format!("r{}", s.rows[i]).into(),
            |s: &S, i| col().w_full().h(50.0).children((s.rows[i] == 10).then(|| el().key("mark").w_full().h(20.0))))
    } else {
        rows.children(s.rows.iter().map(|k| el().key(format!("r{k}")).w_full().h(50.0)))
    };
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

fn app(rows: usize, virtual_rows: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { rows: (0..rows).collect(), virtual_rows }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn offset(rt: &Runtime<S>) -> f64 {
    rt.scrolls.values().next().unwrap().offset
}

fn named(rt: &Runtime<S>, key: &str) -> bool {
    rt.nodes.iter().any(|n| n.named(key))
}

fn gliding(rt: &Runtime<S>) -> bool {
    rt.scrolls.values().any(|s| s.glide().is_some())
}

/// The offset each frame until the glide stops.
fn glide(rt: &mut Runtime<S>) -> Vec<f64> {
    let mut path = vec![offset(rt)];
    for _ in 0..240 {
        rt.frame(DT);
        path.push(offset(rt));
        if !gliding(rt) { break; }
    }
    path
}

/// Centres row `k` of 40 and takes the glide's first frame.
fn centring(k: usize) -> Runtime<S> {
    let mut rt = app(40, false);
    rt.update(move |_, tx| tx.scroll_center(format!("r{k}")));
    rt.frame(DT);
    assert!(gliding(&rt));
    rt
}

/// Where the undisturbed glide to row `k` goes, frame by frame.
fn undisturbed(k: usize) -> Vec<f64> {
    let mut rt = centring(k);
    glide(&mut rt)
}

#[test]
fn a_centring_glide_stops_once_its_target_is_removed_for_good() {
    let mut rt = centring(30);
    // Only row 30 goes: the rest keep its old place in bounds.
    rt.update(|s, _| s.rows.retain(|&k| k != 30));
    for _ in 0..3 { rt.frame(DT); }
    assert!(!gliding(&rt), "the glide stopped");
    assert!(rt.requests.is_empty(), "its reveal went with it");
    let stopped = offset(&rt);
    assert!(stopped < 30.0 * 50.0 - 75.0 - 50.0, "short of the old target: {stopped}");
    for _ in 0..60 { rt.frame(DT); }
    assert_eq!(offset(&rt), stopped, "nothing pulls it on");
}

#[test]
fn a_target_remounted_under_its_key_the_next_frame_glides_on_as_before() {
    let mut rt = centring(30);
    let mut path = vec![offset(&rt)];
    rt.update(|s, tx| { tx.instant(); s.rows.retain(|&k| k != 30); });
    rt.frame(DT);
    assert!(!named(&rt, "r30"), "it was unmounted");
    path.push(offset(&rt));
    rt.update(|s, tx| { tx.instant(); s.rows = (0..40).collect(); });
    path.extend(glide(&mut rt).into_iter().skip(1));
    assert_eq!(path, undisturbed(30));
}

/// Removes row 3, in view, in an animated commit, so it plays its exit rather than unmounting
/// unseen, and takes `frames` frames of it.
fn leaving(frames: usize) -> (Runtime<S>, Vec<f64>) {
    let mut rt = centring(3);
    let mut path = vec![offset(&rt)];
    rt.update(|s, _| s.rows.retain(|&k| k != 3));
    for _ in 0..frames {
        rt.frame(DT);
        let r3 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("r3")).expect("it is still leaving").id;
        assert!(rt.is_exiting(r3));
        path.push(offset(&rt));
    }
    (rt, path)
}

#[test]
fn a_target_whose_exit_is_undone_the_next_frame_glides_on_as_before() {
    let (mut rt, mut path) = leaving(1);
    rt.update(|s, _| s.rows = (0..40).collect());
    path.extend(glide(&mut rt).into_iter().skip(1));
    assert_eq!(path, undisturbed(3));
}

#[test]
fn a_target_whose_exit_is_undone_frames_later_glides_on_as_before() {
    // Still mounted while it fades, however many frame ends pass: it isn't gone.
    let (mut rt, mut path) = leaving(4);
    assert!(gliding(&rt));
    rt.update(|s, _| s.rows = (0..40).collect());
    path.extend(glide(&mut rt).into_iter().skip(1));
    assert_eq!(path, undisturbed(3));
}

#[test]
fn a_target_whose_exit_runs_out_stops_the_glide_once_unmounted() {
    let (mut rt, _) = leaving(1);
    for _ in 0..60 {
        if !named(&rt, "r3") { break; }
        rt.frame(DT);
    }
    assert!(!named(&rt, "r3") && gliding(&rt), "its exit ended mid-glide");
    rt.frame(DT);
    assert!(!gliding(&rt), "the glide stopped the frame after");
    assert!(rt.requests.is_empty());
}

#[test]
fn a_virtual_row_out_of_the_window_while_the_glide_travels_keeps_it_going() {
    let mut rt = app(200, true);
    rt.update(|_, tx| tx.scroll_center("r150"));
    rt.frame(DT);
    assert!(gliding(&rt));
    // Rows arrive ahead of it, pushing it out of the list's window: it's still a row of the
    // list, there to follow, so the glide carries on to where it was going.
    rt.update(|s, _| { s.rows.splice(147..147, 1000..1200); });
    let mut away = 0;
    for _ in 0..240 {
        rt.frame(DT);
        away += usize::from(!rt.nodes.iter().any(|n| n.named("r150")));
        if !gliding(&rt) { break; }
    }
    assert!(away >= 3, "the row was out of the window for {away} frames");
    assert_eq!(offset(&rt), 150.0 * 50.0 - 75.0, "it landed where it was going");
}

#[test]
fn a_child_of_a_virtual_row_out_of_the_window_keeps_its_glide() {
    let start = || {
        let mut rt = app(200, true);
        // The overscan fills in over a few frames, mounting row 10 below the viewport.
        for _ in 0..10 { rt.frame(DT); }
        assert!(named(&rt, "mark"));
        rt.update(|_, tx| tx.scroll_center("mark"));
        rt.frame(DT);
        assert!(gliding(&rt));
        rt
    };
    let mut rt = start();
    let mut path = vec![offset(&rt)];
    // Rows arrive ahead of row 10, taking it and `mark` out of the list's window, then go again.
    rt.update(|s, tx| { tx.instant(); s.rows.splice(10..10, 1000..1100); });
    for _ in 0..3 {
        rt.frame(DT);
        assert!(!named(&rt, "mark") && gliding(&rt), "out of the window {} while gliding {} at {}", !named(&rt, "mark"), gliding(&rt), offset(&rt));
        path.push(offset(&rt));
    }
    rt.update(|s, tx| { tx.instant(); s.rows = (0..200).collect(); });
    path.extend(glide(&mut rt).into_iter().skip(1));
    let mut rt = start();
    assert_eq!(path, glide(&mut rt));
}

#[test]
fn a_glide_with_its_target_mounted_looks_for_it_only_while_following_it() {
    let mut rt = centring(30);
    crate::runtime::sticky::scans::take();
    for _ in 0..10 {
        rt.frame(DT);
        assert!(gliding(&rt));
    }
    assert_eq!(crate::runtime::sticky::scans::take(), 0, "the frame's end scanned the tree again");
}
