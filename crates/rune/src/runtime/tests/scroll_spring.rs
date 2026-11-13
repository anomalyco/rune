//! `Tx::scroll_to_with`: a programmatic glide on the app's spring, and glides retargeted by key
//! repeat running as one motion.
//!
//! No scroll request follows the change's motion (`Tx::motion`, `Tx::motion_curve`): each keeps
//! the spring it names, and the tests here hold that.
use super::*;

const DT: f64 = 1.0 / 120.0;
const ROW: usize = 60;
const VIEW: usize = 300;

/// Four hundred 60-point rows in a 300-point viewport.
fn view(_: &(), _: &mut Cx<()>) -> El {
    let rows = col().key("rows").w_full().children((0..400).map(|k| el().key(format!("r{k}")).w_full().h(ROW as f32)));
    col().size(200.0, VIEW as f32).child(col().key("scroll").size(200.0, VIEW as f32).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

fn offset(rt: &Runtime<()>) -> f64 {
    rt.scrolls.values().next().unwrap().offset
}

fn runtime() -> Runtime<()> {
    let mut rt = Runtime::new((), view, (200.0, VIEW as f32), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

/// ↓ held for a second at 30 Hz key repeat, at 120 frames a second: each press selects the
/// next row and `ask` keeps it in view, given the offset that lands the row on the viewport's
/// bottom and where the viewport stands. The offset's move each frame of the hold.
fn hold(ask: impl Fn(&mut Tx, f32, f32)) -> Vec<f64> {
    let mut rt = runtime();
    let mut moves = Vec::new();
    let mut last = offset(&rt);
    for f in 0..120 {
        if f % 4 == 0 {
            // Row 4 is the last that shows whole at the top: the first press goes to row 5.
            let selected = 5 + f / 4;
            let y = ((selected + 1) * ROW - VIEW) as f32;
            let at = offset(&rt) as f32;
            rt.update(|_, tx| ask(tx, y, at));
        }
        rt.frame(DT);
        moves.push(offset(&rt) - last);
        last = offset(&rt);
    }
    moves
}

/// The coefficient of variation of `moves`: their spread over their mean.
fn cv(moves: &[f64]) -> f64 {
    let mean = moves.iter().sum::<f64>() / moves.len() as f64;
    let var = moves.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / moves.len() as f64;
    var.sqrt() / mean
}

/// Over the hold's second half, once the glide has caught up with the presses from rest: the
/// coefficient of variation, and the slowest frame's move against the mean.
fn steady(moves: &[f64]) -> (f64, f64) {
    let steady = &moves[moves.len() / 2..];
    let mean = steady.iter().sum::<f64>() / steady.len() as f64;
    (cv(steady), steady.iter().copied().fold(f64::MAX, f64::min) / mean)
}

#[test]
fn key_repeat_retargets_glide_as_one_motion() {
    let quick = Spring::new(0.18, 0.0);
    let glide = hold(|tx, y, _| tx.scroll_to("scroll", 0.0, y));
    let with = hold(|tx, y, _| tx.scroll_to_with("scroll", 0.0, y, quick));
    // What steps look like: each press starts the glide again from rest.
    let stairs = hold(|tx, y, at| {
        tx.scroll_to_instant("scroll", 0.0, at);
        tx.scroll_to("scroll", 0.0, y);
    });
    for (name, moves) in [("glide", &glide), ("quick", &with), ("from rest", &stairs)] {
        let (steady_cv, slowest) = steady(moves);
        eprintln!("{name}: hold cv {:.3}, steady cv {steady_cv:.3}, slowest {slowest:.2} of the mean, frame 1 {:.2} pt", cv(moves), moves[0]);
    }
    for (name, moves) in [("glide", &glide), ("quick", &with)] {
        let (steady_cv, slowest) = steady(moves);
        assert!(steady_cv < 0.05 && slowest > 0.9, "{name}: the glide keeps its speed between presses: cv {steady_cv:.3}, slowest {slowest:.2}: {moves:.1?}");
    }
    let (steady_cv, slowest) = steady(&stairs);
    assert!(steady_cv > 0.25 && slowest < 0.5, "restarting from rest steps: cv {steady_cv:.3}, slowest {slowest:.2}");
}

#[test]
fn a_quicker_spring_covers_more_of_the_first_frame() {
    let first = |ask: &dyn Fn(&mut Tx)| {
        let mut rt = runtime();
        rt.update(|_, tx| ask(tx));
        rt.frame(DT);
        offset(&rt)
    };
    let glide = first(&|tx| tx.scroll_to("scroll", 0.0, 500.0));
    let quick = first(&|tx| tx.scroll_to_with("scroll", 0.0, 500.0, Spring::new(0.18, 0.0)));
    eprintln!("frame 1 of a 500-point glide: glide {glide:.2} pt, quick {quick:.2} pt");
    assert!(quick > glide * 2.5, "{quick:.2} against {glide:.2}");
}

#[test]
fn without_a_spring_a_glide_runs_on_glide() {
    let path = |ask: &dyn Fn(&mut Tx)| {
        let mut rt = runtime();
        rt.update(|_, tx| ask(tx));
        let mut path = Vec::new();
        for _ in 0..120 {
            rt.frame(DT);
            path.push(offset(&rt));
        }
        path
    };
    let glide = path(&|tx| tx.scroll_to("scroll", 0.0, 500.0));
    assert_eq!(glide, path(&|tx| tx.scroll_to_with("scroll", 0.0, 500.0, motion::GLIDE)));
    // The same path a spring on `GLIDE` steps from rest.
    let (mut d, mut v) = (-500.0f32, 0.0);
    for (f, y) in glide.iter().enumerate().take(30) {
        (d, v) = motion::GLIDE.sample(d, v, 0.0, DT as f32);
        assert!((y - (500.0 + f64::from(d))).abs() < 1e-3, "frame {f}: {y} against {}", 500.0 + d);
    }
    assert_eq!(*glide.last().unwrap(), 500.0);
}

#[test]
fn another_glide_after_a_springed_one_runs_on_glide_again() {
    let mut rt = runtime();
    rt.update(|_, tx| tx.scroll_to_with("scroll", 0.0, 500.0, Spring::new(0.18, 0.0)));
    for _ in 0..120 { rt.frame(DT); }
    assert_eq!(offset(&rt), 500.0);
    rt.update(|_, tx| tx.scroll_to_bottom("scroll"));
    rt.frame(DT);
    assert!(rt.scrolls.values().next().unwrap().spring.is_none(), "a glide to the bottom takes the default spring");
}

/// The viewport's offset each of 120 frames of the request `ask` makes.
fn glide_frames(ask: &dyn Fn(&mut Tx)) -> Vec<f64> {
    let mut rt = runtime();
    rt.update(|_, tx| ask(tx));
    let mut path = Vec::new();
    for _ in 0..120 {
        rt.frame(DT);
        path.push(offset(&rt));
    }
    path
}

/// `Tx::motion` and `Tx::motion_curve` move the change's own layout; the scroll requests asked
/// for in the same handler keep the spring each names, so an app tuning its layout curve can't
/// quietly retime a glide. Each path is the one the same request walks without an override.
fn same_glide(name: &str, plain: &dyn Fn(&mut Tx), with_curve: &dyn Fn(&mut Tx), with_spring: &dyn Fn(&mut Tx)) {
    let plain = glide_frames(plain);
    assert!(plain.last().unwrap() > &0.0, "{name} moved");
    assert_eq!(glide_frames(with_curve), plain, "{name}: `Tx::motion_curve` retimed the scroll");
    assert_eq!(glide_frames(with_spring), plain, "{name}: `Tx::motion` retimed the scroll");
}

#[test]
fn a_motion_override_leaves_scroll_requests_alone() {
    let curve = rune_motion::Curve::tween(0.3, rune_motion::Easing::EASE_OUT);
    let spring = Spring::new(0.18, 0.0);
    same_glide(
        "scroll_to",
        &|tx| tx.scroll_to("scroll", 0.0, 500.0),
        &|tx| { tx.motion_curve(curve); tx.scroll_to("scroll", 0.0, 500.0) },
        &|tx| { tx.motion(spring); tx.scroll_to("scroll", 0.0, 500.0) },
    );
    same_glide(
        "scroll_to_bottom",
        &|tx| tx.scroll_to_bottom("scroll"),
        &|tx| { tx.motion_curve(curve); tx.scroll_to_bottom("scroll") },
        &|tx| { tx.motion(spring); tx.scroll_to_bottom("scroll") },
    );
    same_glide(
        "scroll_center",
        &|tx| tx.scroll_center("r20"),
        &|tx| { tx.motion_curve(curve); tx.scroll_center("r20") },
        &|tx| { tx.motion(spring); tx.scroll_center("r20") },
    );
}
