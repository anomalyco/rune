use winit::dpi::{LogicalPosition, LogicalSize};
use winit::window::ResizeDirection;

use crate::runtime::Runtime;
use super::Win;

/// What a built frame hands the window besides its pixels: secure input, the IME, the cursor,
/// help tags and the clipboard.
pub(super) fn sync_platform<S: 'static>(l: &mut Win, rt: &mut Runtime<S>, secure_input: &mut crate::input::secure_input::SecureInput,
    ime: &mut bool, clipboard: &crate::clipboard::Client) {
    let focused = l.window.has_focus();
    secure_input.set(rt.has_secure_focus() && focused);
    let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus() && focused;
    if std::mem::take(&mut rt.ime_reset) || ime_allowed != *ime {
        l.window.set_ime_allowed(false);
        l.window.set_ime_allowed(ime_allowed);
        *ime = ime_allowed;
    }
    l.apply_cursor(rt, focused);
    #[cfg(target_os = "macos")]
    l.native.help.sync(&l.window, rt.help_regions());
    clipboard.flush(rt);
    if let Some(a) = rt.ime_area {
        l.window.set_ime_cursor_area(
            LogicalPosition::new(a[0] as f64, a[1] as f64),
            LogicalSize::new(a[2] as f64, a[3] as f64),
        );
    }
}

impl Win {
    /// The six-point resize band is window chrome, ahead of even a full-window drag region.
    /// A gesture that started in the content keeps its cursor and capture as it crosses it.
    pub(super) fn resize_edge<S>(&self, rt: &Runtime<S>) -> Option<ResizeDirection> {
        if !self.resize_edges || rt.pointer_captured() { return None; }
        let edge = edge_at(rt.pointer()?, rt.viewport)?;
        (!self.window.is_maximized() && self.window.fullscreen().is_none()).then_some(edge)
    }

    /// The runtime's cursor while the pointer is in the window, the arrow once it leaves.
    /// `Hidden` needs focus, so it can't outlive the window being active.
    pub(super) fn apply_cursor<S>(&mut self, rt: &Runtime<S>, focused: bool) {
        if let Some(edge) = self.resize_edge(rt) {
            self.cursor.apply_resize(&self.window, edge);
            return;
        }
        let want = match rt.cursor() { crate::el::Cursor::Hidden if !focused => crate::el::Cursor::Default, c => c };
        self.cursor.apply(&self.window, want, rt.pointer().is_some());
    }
}

fn edge_at((x, y): (f32, f32), (width, height): (f32, f32)) -> Option<ResizeDirection> {
    use ResizeDirection::*;
    const BAND: f32 = 6.0;
    if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) { return None; }
    let side = |p: f32, size: f32| {
        if p < BAND.min(size / 2.0) { -1 } else if p >= (size - BAND).max(size / 2.0) { 1 } else { 0 }
    };
    Some(match (side(x, width), side(y, height)) {
        (-1, -1) => NorthWest, (0, -1) => North, (1, -1) => NorthEast,
        (-1,  0) => West,                        (1,  0) => East,
        (-1,  1) => SouthWest, (0,  1) => South, (1,  1) => SouthEast,
        _ => return None,
    })
}
