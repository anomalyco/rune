use super::present_mode;
use wgpu::PresentMode::{AutoVsync, Fifo, Immediate, Mailbox};

/// A Wayland window paces on frame callbacks, so it presents Mailbox when offered and never waits
/// on the compositor latching a FIFO commit; every other surface stays vsynced.
#[test]
fn only_a_window_paced_on_frame_callbacks_presents_mailbox_and_only_when_offered() {
    assert_eq!(present_mode(&[Mailbox, Fifo], true), Mailbox, "Wayland: Mailbox when the surface offers it");
    assert_eq!(present_mode(&[Fifo, Immediate], true), AutoVsync, "Wayland without Mailbox: vsynced");
    assert_eq!(present_mode(&[Mailbox, Fifo], false), AutoVsync, "X11, Windows, macOS and overlays: vsynced");
    assert_eq!(present_mode(&[], true), AutoVsync);
}
