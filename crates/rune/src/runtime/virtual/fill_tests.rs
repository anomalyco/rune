//! The fill machine against origin/dev's counter: every sequence of frames, every answer.
use super::{FILL_FRAMES, Fill, FillEffect, FillEvent};

/// origin/dev's `Runtime::virtual_filling`, transcribed as it was: `Runtime.virtual_fill: u32`
/// counted one more for each frame that found an index-built list wanting its window and reset
/// to zero for each frame that found none, and it answered `filling && virtual_fill <= 12` —
/// `VIRTUAL_FILL_FRAMES` at `runtime/virtual/window.rs:8`. It is never edited to fit the branch:
/// a walk that fails means the branch differs from dev.
struct Dev {
    virtual_fill: u32,
}

impl Dev {
    fn frame(&mut self, filling: bool) -> bool {
        self.virtual_fill = if filling { self.virtual_fill + 1 } else { 0 };
        filling && self.virtual_fill <= 12
    }
}

fn sequence(bits: u32, len: usize) -> String {
    (0..len).map(|i| if bits >> i & 1 == 1 { 'F' } else { '.' }).collect()
}

/// Every sequence of frames up to two past the bound (the bound is where the answers diverge, and
/// one past it lets a settled run restart and diverge too, so a shorter walk would miss both),
/// each driven through `step` and its answer compared with dev's after every frame.
#[test]
fn every_run_of_frames_draws_what_dev_draws() {
    let longest = FILL_FRAMES as usize + 2;
    for len in 1..=longest {
        for bits in 0..1u32 << len {
            let walk = sequence(bits, len);
            let (mut fill, mut dev) = (Fill::Idle, Dev { virtual_fill: 0 });
            for (i, wanted) in (0..len).map(|i| (i, bits >> i & 1 == 1)) {
                let drew = fill.step(FillEvent::Frame { wanted }).into_iter().any(|e| e == FillEffect::Draw);
                assert_eq!(
                    drew,
                    dev.frame(wanted),
                    "frame {i} of {walk}: wanted {wanted}, state {:?}",
                    fill
                );
                assert!(
                    fill.frames() <= FILL_FRAMES + 1,
                    "frame {i} of {walk}: the state must stay within the bound, was {:?}",
                    fill
                );
            }
        }
    }
}

/// One frame's answer is all the runtime keeps of the fill, so a settled list asks again at once,
/// while a run keeps asking until the bound and not after it.
#[test]
fn a_settled_frame_forgets_the_run_and_the_bound_stops_it() {
    let mut fill = Fill::Idle;
    let frames = |fill: &mut Fill| {
        (0..20)
            .map(|_| !fill.step(FillEvent::Frame { wanted: true }).is_empty())
            .collect::<Vec<_>>()
    };
    let drawn = frames(&mut fill);
    assert!(drawn[..FILL_FRAMES as usize].iter().all(|drew| *drew), "the first {FILL_FRAMES} frames draw: {drawn:?}");
    assert!(drawn[FILL_FRAMES as usize..].iter().all(|drew| !drew), "the bound stops drawing: {drawn:?}");
    fill.step(FillEvent::Frame { wanted: false });
    assert!(!fill.step(FillEvent::Frame { wanted: true }).is_empty());
}
