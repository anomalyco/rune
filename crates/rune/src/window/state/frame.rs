//! Window frame changes app code asks for (`Tx::resize_window`, `Tx::center_window`): where
//! the frame lands, and the spring that carries it there one display frame at a time.

use rune_motion::Spring;

impl<S: 'static> crate::App<S> {
    /// Which edge or corner stays fixed when Rune resizes this window
    /// ([`Self::fit_content_height`], [`Tx::resize_window`](crate::Tx::resize_window),
    /// [`Tx::resize_window_instant`](crate::Tx::resize_window_instant),
    /// [`WindowHandle::resize`](crate::WindowHandle::resize),
    /// [`WindowHandle::resize_instant`](crate::WindowHandle::resize_instant)).
    ///
    /// Defaults to [`ResizeAnchor::TopLeft`](crate::ResizeAnchor::TopLeft). Update at runtime with
    /// [`Tx::set_resize_anchor`](crate::Tx::set_resize_anchor) or
    /// [`WindowHandle::set_resize_anchor`](crate::WindowHandle::set_resize_anchor).
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, ResizeAnchor, el};
    ///
    /// App::new(AppSpec::new("Launcher", (600.0, 400.0), || (), |_, _| el()))
    ///     .fit_content_height()
    ///     .resize_anchor(ResizeAnchor::Bottom)
    ///     .run();
    /// ```
    pub fn resize_anchor(mut self, anchor: crate::ResizeAnchor) -> Self {
        self.resize_anchor = anchor;
        self
    }
}

/// What handlers asked of their window's frame, merged across a commit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameRequest {
    /// Content size in points.
    pub size: Option<(f32, f32)>,
    /// Asked with `Tx::resize_window` rather than its `_instant` form.
    pub animate: bool,
    /// The spring the frame moves on; `None` jumps. Resolved when the runtime latches the
    /// request (`Tx::instant`, `Tx::motion`, reduced motion).
    pub spring: Option<Spring>,
    pub center: bool,
    /// Start an interactive OS/compositor window resize from this edge (`Tx::drag_resize_window`).
    pub resize_drag: Option<crate::ResizeEdge>,
}

impl FrameRequest {
    /// A later request's size or resize drag replaces an earlier one's; centring sticks.
    pub fn merge(self, later: FrameRequest) -> FrameRequest {
        let (size, animate, spring) = if later.size.is_some() { (later.size, later.animate, later.spring) } else { (self.size, self.animate, self.spring) };
        FrameRequest {
            size,
            animate,
            spring,
            center: self.center || later.center,
            resize_drag: later.resize_drag.or(self.resize_drag),
        }
    }
}

/// Where a frame request lands, in top-down screen coordinates `[x, y, w, h]`. `current` is the
/// window's outer frame, `outer` the outer size for the requested content size, `visible` the
/// usable area of its screen, and `anchor` which edge or corner stays fixed when `center` is
/// false. Centring follows AppKit's `center` (centred across, a quarter of the spare height
/// above). A frame never moves further off the visible area than it already was.
pub(crate) fn target(
    current: [f32; 4],
    outer: Option<(f32, f32)>,
    center: bool,
    visible: [f32; 4],
    anchor: crate::ResizeAnchor,
) -> [f32; 4] {
    let (w, h) = outer.unwrap_or((current[2], current[3]));
    let [vx, vy, vw, vh] = visible;
    let (mut x, mut y) = if center {
        (vx + ((vw - w) / 2.0).floor(), vy + ((vh - h) / 4.0).floor())
    } else {
        let x = if anchor.pins_right() {
            current[0] + current[2] - w
        } else if anchor.centers_x() {
            current[0] + (current[2] - w) / 2.0
        } else {
            current[0]
        };
        let y = if anchor.pins_bottom() {
            current[1] + current[3] - h
        } else if anchor.centers_y() {
            current[1] + (current[3] - h) / 2.0
        } else {
            current[1]
        };
        (x, y)
    };
    let (left, top) = if center { (vx, vy) } else { (current[0].min(vx), current[1].min(vy)) };
    let right = if center { vx + vw } else { (current[0] + current[2]).max(vx + vw) };
    let bottom = if center { vy + vh } else { (current[1] + current[3]).max(vy + vh) };
    x = if !center && anchor.pins_right() {
        x.max(left).min(right - w)
    } else {
        x.min(right - w).max(left)
    };
    y = if !center && anchor.pins_bottom() {
        y.max(top).min(bottom - h)
    } else {
        y.min(bottom - h).max(top)
    };
    [x, y, w, h]
}

/// A frame moving to `to` on a spring, sampled on the host's clock (seconds). Each component
/// is its own closed-form track, so a retarget keeps position and velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameAnim {
    pub spring: Option<Spring>,
    start: f64,
    from: [f32; 4],
    velocity: [f32; 4],
    pub to: [f32; 4],
    end: f64,
}

/// Half a device pixel at 2x: the frame rounds to whole pixels, so closer is landed.
const EPSILON: f32 = 0.25;

impl FrameAnim {
    pub fn new(spring: Option<Spring>, now: f64, from: [f32; 4], velocity: [f32; 4], to: [f32; 4]) -> FrameAnim {
        let end = spring.map_or(0.0, |s| (0..4).map(|i| s.settling_duration_from(from[i], velocity[i], to[i], EPSILON)).fold(0.0f32, f32::max));
        FrameAnim { spring, start: now, from, velocity, to, end: now + f64::from(end) }
    }

    /// Starts from where an earlier animation is now, at its speed, or from rest at `from`.
    pub fn retarget(previous: Option<&FrameAnim>, spring: Option<Spring>, now: f64, from: [f32; 4], to: [f32; 4]) -> FrameAnim {
        let (from, velocity) = previous.map_or((from, [0.0; 4]), |p| p.sample(now));
        FrameAnim::new(spring, now, from, if spring.is_some() { velocity } else { [0.0; 4] }, to)
    }

    pub fn sample(&self, now: f64) -> ([f32; 4], [f32; 4]) {
        let Some(spring) = self.spring.filter(|_| now < self.end) else { return (self.to, [0.0; 4]) };
        let t = (now - self.start).max(0.0) as f32;
        let mut at = [0.0; 4];
        let mut velocity = [0.0; 4];
        for i in 0..4 {
            (at[i], velocity[i]) = spring.sample(self.from[i], self.velocity[i], self.to[i], t);
        }
        (at, velocity)
    }

    pub fn done(&self, now: f64) -> bool { self.spring.is_none() || now >= self.end }

    /// Moves the whole path by `(dx, dy)`: someone dragged the window mid-flight, and the
    /// size carries on from where they put it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn shift(&mut self, dx: f32, dy: f32) {
        for rect in [&mut self.from, &mut self.to] {
            rect[0] += dx;
            rect[1] += dy;
        }
    }
}

/// A requested content size inside the window's minimum and maximum sizes.
pub(crate) fn clamp_size(size: (f32, f32), min: (f32, f32), max: Option<(f32, f32)>) -> (f32, f32) {
    let (max_w, max_h) = max.unwrap_or((f32::INFINITY, f32::INFINITY));
    (size.0.max(min.0).min(max_w), size.1.max(min.1).min(max_h))
}

/// Rounds a frame to whole device pixels, so content and drawable sizes agree.
pub(crate) fn snap(rect: [f32; 4], scale: f32) -> [f32; 4] {
    rect.map(|v| (v * scale).round() / scale)
}

/// Rounds a frame to whole device pixels while keeping `anchor`'s pinned right/bottom edge
/// invariant (avoiding 1-pixel jitter when `round(y) + round(h)` rounds both halves up).
pub(crate) fn snap_anchored(rect: [f32; 4], scale: f32, anchor: crate::ResizeAnchor) -> [f32; 4] {
    let r = |v: f32| (v * scale).round() / scale;
    let w = r(rect[2]);
    let h = r(rect[3]);
    let x = if anchor.pins_right() { r(rect[0] + rect[2]) - w } else { r(rect[0]) };
    let y = if anchor.pins_bottom() { r(rect[1] + rect[3]) - h } else { r(rect[1]) };
    [x, y, w, h]
}

/// The drawable size a window takes when its scale changes from `from` to `to` and it keeps its
/// logical size, rounded as winit rounds it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn rescaled(size: (u32, u32), from: f64, to: f64) -> (u32, u32) {
    let scale = |v: u32| (f64::from(v) / from * to).round() as u32;
    (scale(size.0), scale(size.1))
}

/// The drawable size a Windows window takes on `ScaleFactorChanged`. A normal window's
/// `inner_size` is still at `from` because winit resizes it after the event; a maximised or
/// full-screen window keeps `size` because winit does not resize it and Windows already sent
/// `Resized` with the new monitor's size.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn scale_change_size(size: (u32, u32), from: f64, to: f64, maximized: bool, fullscreen: bool) -> (u32, u32) {
    if maximized || fullscreen { size } else { rescaled(size, from, to) }
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
