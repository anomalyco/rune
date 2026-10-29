//! `Tx::launch`: a sent message's new row starts on the composer's text at its own size and
//! travels one smooth path into its slot in the scroll view.
use rune::{Cx, El, Env, GrowthSpec, Key, KeyEvent, Runtime, ScrollSpec, TextEdit, TextStyle, Transition, Tx, col, text, text_input};

struct State { draft: TextEdit, sent: Vec<String>, relaunch: bool, zoom: f32, ticks: u32, tray: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut rows = col().w_full().growth(GrowthSpec::default());
    for (i, m) in s.sent.iter().enumerate() {
        let row = col().key(format!("msg-{i}")).w_full().py(8.0).items_end().child(text(m.clone()).key("body"));
        // A new row's container entering with it, as a queue appears with its first prompt.
        let tray = || col().key("tray").w_full().transition(Transition::scale(0.9).combined(Transition::opacity()));
        rows = rows.child(if s.tray && i == 2 { tray().child(row) } else { row });
    }
    rows = rows.child(text(format!("{}", s.ticks)).key("ticks"));
    let page = col().w_full().h_full()
        .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true }).child(rows))
        .child(text_input(&s.draft, TextStyle::new(14.0, 400.0)).key("composer").w_full().h(60.0)
            .on_edit(|s: &mut State| &mut s.draft)
            .on_submit(|s: &mut State, tx: &mut Tx| {
                let last = if s.relaunch { s.sent.len() - 1 } else { s.sent.len() };
                if !s.relaunch { s.sent.push(s.draft.text.clone()); }
                s.draft.clear();
                tx.launch("composer", format!("msg-{last}"));
            }));
    if s.zoom == 1.0 { return page; }
    // An app-wide zoom, as a whole window scaled about its centre.
    col().w_full().h_full().center().child(col().w(400.0 / s.zoom).h(300.0 / s.zoom).scale(s.zoom).child(page))
}

fn app(relaunch: bool) -> Runtime<State> {
    app_at(relaunch, 1.0)
}

fn app_at(relaunch: bool, zoom: f32) -> Runtime<State> {
    let mut draft = TextEdit::new("third");
    draft.focused = true;
    let sent = vec!["first".to_string(), "second".to_string()];
    let mut rt = Runtime::new(State { draft, sent, relaunch, zoom, ticks: 0, tray: false }, view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn rect(rt: &Runtime<State>, key: &str, text: Option<&str>) -> ([f32; 4], [f32; 4]) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && (text.is_none() || e.text.as_deref() == text)).unwrap();
    (e.rect, e.target)
}

fn send(rt: &mut Runtime<State>) -> Vec<([f32; 4], [f32; 4])> {
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    (0..120).map(|_| {
        rt.frame(1.0 / 120.0);
        rect(rt, "body", Some("third"))
    }).collect()
}

#[test]
fn a_launched_row_leaves_from_the_source_text_and_glides_into_place() {
    let mut rt = app(false);
    let composer = rect(&rt, "composer", None).0;
    let frames = send(&mut rt);
    let (first, target) = frames[0];
    assert!((first[0] - composer[0]).abs() < 6.0 && first[1] >= composer[1] - 3.0 && first[1] + first[3] <= composer[1] + composer[3] + 3.0,
        "the first frame's text sits on the composer's: {first:?} in {composer:?}");
    assert!(target[1] + target[3] < composer[1], "its place is up in the scroll view: {target:?}");
    for (r, t) in &frames {
        assert_eq!((r[2], r[3]), (t[2], t[3]), "full size throughout");
    }
    let (last, end) = frames[frames.len() - 1];
    assert_eq!(last, end, "lands on its place");
    let land = frames.iter().position(|(r, _)| (r[0] - last[0]).abs() < 0.5 && (r[1] - last[1]).abs() < 0.5).unwrap();
    assert!((24..72).contains(&land), "glides over about 0.2-0.6 s: {land} frames");
    // One monotonic path per axis that slows into its slot: past its fastest frame no step
    // outgrows the one before, so it never ends short and snaps the rest.
    for axis in 0..2 {
        let at: Vec<f32> = frames.iter().map(|(r, _)| r[axis]).collect();
        let total = at[at.len() - 1] - at[0];
        let steps: Vec<f32> = at.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|s| s * total.signum() >= -0.01), "axis {axis} turns back: {at:?}");
        let peak = (0..steps.len()).max_by(|&a, &b| steps[a].abs().total_cmp(&steps[b].abs())).unwrap();
        for k in peak + 1..steps.len() {
            assert!(steps[k].abs() <= steps[k - 1].abs() * 1.5 + 0.25, "axis {axis} snaps at frame {k}: {at:?}");
        }
    }
}

#[test]
fn reduced_motion_or_a_mounted_destination_does_not_launch() {
    let mut rt = app(false);
    rt.set_system_reduced_motion(true);
    let frames = send(&mut rt);
    assert!(frames.iter().all(|(r, t)| r == t), "Reduce Motion puts it in place");

    let mut rt = app(true);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    let frames: Vec<_> = (0..30).map(|_| { rt.frame(1.0 / 120.0); rect(&rt, "body", Some("second")) }).collect();
    assert!(frames.iter().all(|(r, t)| r == t), "an already mounted row stays put");
}

#[test]
fn a_launch_whose_slot_moves_mid_flight_lands_there_without_a_lurch() {
    let mut rt = app(false);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    let frames: Vec<_> = (0..120).map(|f| {
        // A row above grows while the message is in the air, as a prompt does when the steer
        // under it stops joining its run.
        if f == 4 { rt.update(|s, _| s.sent[1] = "second\nand a line\nand another".into()); }
        rt.frame(1.0 / 120.0);
        rect(&rt, "body", Some("third"))
    }).collect();
    let (last, end) = frames[frames.len() - 1];
    assert_eq!(last, end, "lands on its moved place");
    for axis in 0..2 {
        let at: Vec<f32> = frames.iter().map(|(r, _)| r[axis]).collect();
        let steps: Vec<f32> = at.windows(2).map(|w| w[1] - w[0]).collect();
        for (k, w) in steps.windows(2).enumerate() {
            assert!((w[1] - w[0]).abs() <= 10.0, "axis {axis} lurches {} pt at frame {}: {at:?}", w[1] - w[0], k + 1);
        }
    }
}

#[test]
fn a_launch_under_a_scaled_ancestor_flies_at_its_drawn_size() {
    let zoom = 1.6;
    let mut rt = app_at(false, zoom);
    let composer = rect(&rt, "composer", None).0;
    let frames = send(&mut rt);
    let (first, _) = frames[0];
    assert!((first[0] - composer[0]).abs() < 6.0 * zoom && first[1] >= composer[1] - 3.0 && first[1] + first[3] <= composer[1] + composer[3] + 3.0,
        "the first frame's text sits on the composer's: {first:?} in {composer:?}");
    for (f, (r, t)) in frames.iter().enumerate() {
        assert!((r[2] - t[2] * zoom).abs() < 0.01 && (r[3] - t[3] * zoom).abs() < 0.01, "frame {f}: drawn at the zoom: {r:?} vs {t:?}");
    }
    let (last, _) = frames[frames.len() - 1];
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rect(&rt, "body", Some("third")).0, last, "landed where it rests");
    for axis in 0..2 {
        let at: Vec<f32> = frames.iter().map(|(r, _)| r[axis]).collect();
        let total = at[at.len() - 1] - at[0];
        for w in at.windows(2) {
            assert!((w[1] - w[0]) * total.signum() >= -0.01, "axis {axis} turns back: {at:?}");
            assert!((w[1] - w[0]).abs() <= total.abs().max(20.0) / 10.0, "axis {axis} snaps: {at:?}");
        }
    }
}

#[test]
fn a_launch_after_another_commit_in_its_frame_still_leaves_from_the_source() {
    // Something else changes state in the frame Enter lands in, as a server event or a cleared
    // attachment does: that commit comes first, and the launch must still map through the zoom.
    let zoom = 1.6;
    let mut rt = app_at(false, zoom);
    let composer = rect(&rt, "composer", None).0;
    rt.update(|s, _| s.ticks += 1);
    let frames = send(&mut rt);
    let (first, _) = frames[0];
    assert!((first[0] - composer[0]).abs() < 6.0 * zoom && first[1] >= composer[1] - 3.0 && first[1] + first[3] <= composer[1] + composer[3] + 3.0,
        "the first frame's text sits on the composer's: {first:?} in {composer:?}");
    for axis in 0..2 {
        let at: Vec<f32> = frames.iter().map(|(r, _)| r[axis]).collect();
        let total = at[at.len() - 1] - at[0];
        assert!(at.windows(2).all(|w| (w[1] - w[0]) * total.signum() >= -0.01), "axis {axis} turns back: {at:?}");
    }
}

#[test]
fn a_launch_into_an_entering_container_keeps_the_zoom_above_it() {
    // The container's entrance scale is dropped while the row flies; the app's still zoom above
    // it is not, so the row flies at the size it lands at and doesn't snap when the entrance ends.
    let zoom = 1.6;
    let mut rt = app_at(false, zoom);
    rt.update(|s, _| s.tray = true);
    rt.frame(0.0);
    let mut frames = send(&mut rt);
    frames.extend((0..120).map(|_| { rt.frame(1.0 / 120.0); rect(&rt, "body", Some("third")) }));
    for (f, (r, t)) in frames.iter().enumerate() {
        assert!((r[2] - t[2] * zoom).abs() < 0.01 && (r[3] - t[3] * zoom).abs() < 0.01, "frame {f}: drawn at the zoom: {r:?} vs {t:?}");
    }
    for axis in 0..2 {
        let at: Vec<f32> = frames.iter().map(|(r, _)| r[axis]).collect();
        let total = at[at.len() - 1] - at[0];
        for w in at.windows(2) {
            assert!((w[1] - w[0]) * total.signum() >= -0.01, "axis {axis} turns back: {at:?}");
            assert!((w[1] - w[0]).abs() <= total.abs().max(20.0) / 10.0, "axis {axis} snaps: {at:?}");
        }
    }
}
