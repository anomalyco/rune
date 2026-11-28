use std::time::Duration;

use super::{Redraw, RedrawEffect, RedrawEvent, present_mode};

#[test]
fn frame_callback_paced_wayland_prefers_mailbox_when_offered() {
    use wgpu::PresentMode::{AutoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

    assert_eq!(present_mode(&[Mailbox, Fifo], true), Mailbox);
    assert_eq!(present_mode(&[Fifo, Mailbox], true), Mailbox);
    assert_eq!(present_mode(&[Immediate, Mailbox, Fifo, FifoRelaxed], true), Mailbox);

    // Without Mailbox, never pick Immediate (tearing) or FifoRelaxed; keep AutoVsync.
    assert_eq!(present_mode(&[Fifo], true), AutoVsync);
    assert_eq!(present_mode(&[Fifo, Immediate], true), AutoVsync);
    assert_eq!(present_mode(&[FifoRelaxed, Fifo], true), AutoVsync);
    assert_eq!(present_mode(&[], true), AutoVsync);
}

#[test]
fn unpaced_surfaces_keep_auto_vsync_even_when_mailbox_is_offered() {
    use wgpu::PresentMode::{AutoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

    // X11, Windows, macOS, window/XWayland overlays, and embedded views rely on AutoVsync.
    assert_eq!(present_mode(&[Mailbox, Fifo], false), AutoVsync);
    assert_eq!(present_mode(&[Immediate, Mailbox, Fifo, FifoRelaxed], false), AutoVsync);
    assert_eq!(present_mode(&[Fifo], false), AutoVsync);
    assert_eq!(present_mode(&[], false), AutoVsync);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn wayland_animation_redraw_loop_rearms_each_frame_without_dropping() {
    let t0 = crate::time::now();
    let mut redraw = Redraw::Idle;

    // Opening/closing fade starts: first redraw requested.
    let (next, effect) = redraw.step(RedrawEvent::Ask(t0));
    assert_eq!(effect, RedrawEffect::Request);
    redraw = next;

    // Across multiple animated frames, asking for the next frame while painting defers
    // `Window::request_redraw()` to `TurnEnds` so every commit arms the next Wayland frame.
    for frame in 0..5u64 {
        let now = t0 + Duration::from_millis(frame * 16);
        let (next, effect) = redraw.step(RedrawEvent::Paint);
        assert_eq!(effect, RedrawEffect::None);
        let (next, effect) = next.step(RedrawEvent::Ask(now));
        assert_eq!(effect, RedrawEffect::None);
        let (next, effect) = next.step(RedrawEvent::Painted);
        assert_eq!(effect, RedrawEffect::None);
        let (next, effect) = next.step(RedrawEvent::TurnEnds { now, paints: false });
        assert_eq!(effect, RedrawEffect::Request, "frame {frame} must re-arm redraw at turn end");
        redraw = next;
    }

    // Final settling frame does not ask again: returns to Idle.
    let (next, effect) = redraw.step(RedrawEvent::Paint);
    assert_eq!(effect, RedrawEffect::None);
    let (next, effect) = next.step(RedrawEvent::Painted);
    assert_eq!(effect, RedrawEffect::None);
    let (next, effect) = next.step(RedrawEvent::TurnEnds {
        now: t0 + Duration::from_millis(96),
        paints: false,
    });
    assert_eq!(effect, RedrawEffect::None);
    assert_eq!(next, Redraw::Idle);
}
