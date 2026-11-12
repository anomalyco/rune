//! A `Tx::scroll_center` glide stops once its target is removed for good, and glides on
//! exactly as before while the target is only away: a virtual row out of the window, a child
//! remounted under its key, or one whose exit is undone the next frame.
use super::*;

const DT: f64 = 1.0 / 60.0;

/// 50-point rows keyed `r{k}` in a 200-point viewport, as plain children or virtual rows.
struct S { rows: Vec<usize>, virtual_rows: bool }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").w_full();
    let rows = if s.virtual_rows {
        rows.growth(crate::el::GrowthSpec::default()).virtual_rows(s.rows.len(), 50.0, |s: &S, i| format!("r{}", s.rows[i]).into(), |_: &S, _| el().w_full().h(50.0))
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
    assert!(!rt.nodes.iter().any(|n| n.named("r30")), "it was unmounted");
    path.push(offset(&rt));
    rt.update(|s, tx| { tx.instant(); s.rows = (0..40).collect(); });
    path.extend(glide(&mut rt).into_iter().skip(1));
    assert_eq!(path, undisturbed(30));
}

#[test]
fn a_target_whose_exit_is_undone_the_next_frame_glides_on_as_before() {
    // A row in view, so its removal plays an exit rather than unmounting unseen.
    let mut rt = centring(3);
    let mut path = vec![offset(&rt)];
    // An animated commit: row 3 starts its exit fade, hidden from the reveal while it leaves.
    rt.update(|s, _| s.rows.retain(|&k| k != 3));
    rt.frame(DT);
    let leaving = rt.nodes.iter().find(|n| n.key.as_deref() == Some("r3")).expect("it is still leaving").id;
    assert!(rt.is_exiting(leaving));
    path.push(offset(&rt));
    rt.update(|s, _| s.rows = (0..40).collect());
    path.extend(glide(&mut rt).into_iter().skip(1));
    assert_eq!(path, undisturbed(3));
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
