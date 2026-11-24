use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, Position, Size};
use winit::window::Fullscreen;

use super::{CreatePlan, LiveWindowState, ReopenState, apply_create_plan};
use crate::window::state::{Found, Life, Reveal, RevealEffect};
use crate::windows::{OpenAs, WindowOptions};

/// Drives the host's off-screen lifecycle (`Life`, `Reveal`, `ReopenState`, and
/// `WindowOptions::attributes`) through create, live updates, hide/release, and recreate without
/// opening a native window.
struct LifecycleHarness {
    options: WindowOptions,
    initial_size: (f32, f32),
    life: Life,
    shown: bool,
    has_win: bool,
    reopen: ReopenState,
    use_physical_size: bool,
    allow_position: bool,
    track_size: bool,
    panel: bool,
    windows_deferred_open_as: bool,
}

impl LifecycleHarness {
    fn new(initial: OpenAs) -> Self {
        Self {
            options: WindowOptions::default(),
            initial_size: (480.0, 420.0),
            life: Life::Unopened,
            shown: false,
            has_win: false,
            reopen: ReopenState::new(initial),
            use_physical_size: true,
            allow_position: true,
            track_size: true,
            panel: false,
            windows_deferred_open_as: false,
        }
    }

    fn open(&mut self) -> (winit::window::WindowAttributes, CreatePlan) {
        let found = if !self.has_win {
            Found::Missing
        } else if self.life.shown() {
            Found::Shown { covered: self.life.covered(), minimized: false }
        } else {
            Found::Hidden { minimized: false }
        };
        let effects: Vec<_> = Reveal::Open.effects(found).into_iter().collect();
        assert!(effects.contains(&RevealEffect::Create), "expected Create for missing window: {effects:?}");
        let plan = self.reopen.create_plan(self.options.resizable, self.panel);
        let base = self.options.attributes("Test", self.initial_size, None, true);
        let attrs = apply_create_plan(base, plan, self.windows_deferred_open_as);
        self.shown = true;
        self.has_win = true;
        self.life.show();
        (attrs, plan)
    }

    fn live(&mut self, state: LiveWindowState) {
        assert!(self.has_win && self.life.shown());
        self.reopen.record_live(state, self.use_physical_size, self.allow_position, self.track_size);
    }

    /// Simulates `Host::hide` with `release_gpu_when_closed` (or Wayland unmapping): captures live
    /// state before hiding, leaves `Life::Shown`, and destroys the native window.
    fn hide_and_release(&mut self, final_state: LiveWindowState) {
        if self.has_win && self.life.shown() {
            self.reopen.record_live(final_state, self.use_physical_size, self.allow_position, self.track_size);
        }
        self.life.hide(|| 1);
        self.has_win = false;
    }
}

#[test]
fn maximized_window_reopens_maximized_after_release_and_preserves_restored_size() {
    let mut h = LifecycleHarness::new(OpenAs::Maximized);
    let (first_attrs, first_plan) = h.open();
    assert_eq!(first_plan.open_as, OpenAs::Maximized);
    assert!(first_attrs.maximized);
    assert_eq!(first_attrs.inner_size, Some(Size::Logical(LogicalSize::new(480.0, 420.0))));

    // OS maximizes the window and reports maximized dimensions; restored size must not be overwritten.
    let maximized_live = LiveWindowState {
        maximized: true,
        full_screen: false,
        minimized: false,
        logical_size: Some((1920.0, 1040.0)),
        physical_size: Some((1920, 1040)),
        position: Some(Position::Physical(PhysicalPosition::new(0, 0))),
        restore_frame: None,
    };
    h.live(maximized_live);
    h.hide_and_release(maximized_live);

    let (reopened_attrs, reopened_plan) = h.open();
    assert_eq!(reopened_plan.open_as, OpenAs::Maximized);
    assert!(reopened_attrs.maximized);
    assert_eq!(
        reopened_attrs.inner_size,
        Some(Size::Logical(LogicalSize::new(480.0, 420.0))),
        "reopened maximized window keeps its restored size instead of maximized bounds",
    );
    assert_eq!(reopened_attrs.position, None);
}

#[test]
fn maximized_window_restored_by_user_before_hide_reopens_restored_at_new_bounds() {
    let mut h = LifecycleHarness::new(OpenAs::Maximized);
    let _ = h.open();

    let restored_live = LiveWindowState {
        maximized: false,
        full_screen: false,
        minimized: false,
        logical_size: Some((900.0, 640.0)),
        physical_size: Some((1350, 960)),
        position: Some(Position::Physical(PhysicalPosition::new(240, 180))),
        restore_frame: None,
    };
    h.live(restored_live);
    h.hide_and_release(restored_live);

    let (reopened_attrs, reopened_plan) = h.open();
    assert_eq!(reopened_plan.open_as, OpenAs::Normal);
    assert!(!reopened_attrs.maximized);
    assert_eq!(reopened_attrs.fullscreen, None);
    assert_eq!(reopened_attrs.inner_size, Some(Size::Physical(PhysicalSize::new(1350, 960))));
    assert_eq!(reopened_attrs.position, Some(Position::Physical(PhysicalPosition::new(240, 180))));
}

#[test]
fn normal_window_preserves_user_size_and_position_across_release_and_wayland_uses_logical_size() {
    // Windows / X11: physical size + physical outer position survive release.
    let mut h = LifecycleHarness::new(OpenAs::Normal);
    let _ = h.open();
    let moved_resized = LiveWindowState {
        maximized: false,
        full_screen: false,
        minimized: false,
        logical_size: Some((720.0, 540.0)),
        physical_size: Some((1080, 810)),
        position: Some(Position::Physical(PhysicalPosition::new(320, 140))),
        restore_frame: None,
    };
    h.live(moved_resized);
    h.hide_and_release(moved_resized);

    let (attrs, plan) = h.open();
    assert_eq!(plan.open_as, OpenAs::Normal);
    assert_eq!(attrs.inner_size, Some(Size::Physical(PhysicalSize::new(1080, 810))));
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(320, 140))));

    // Wayland: no window positions, and inner_size must stay logical so winit does not double-scale.
    let mut wayland = LifecycleHarness::new(OpenAs::Normal);
    wayland.use_physical_size = false;
    wayland.allow_position = false;
    let _ = wayland.open();
    let wayland_live = LiveWindowState {
        maximized: false,
        full_screen: false,
        minimized: false,
        logical_size: Some((800.0, 600.0)),
        physical_size: Some((1600, 1200)),
        position: None,
        restore_frame: None,
    };
    wayland.live(wayland_live);
    wayland.hide_and_release(wayland_live);

    let (w_attrs, w_plan) = wayland.open();
    assert_eq!(w_plan.open_as, OpenAs::Normal);
    assert_eq!(w_attrs.inner_size, Some(Size::Logical(LogicalSize::new(800.0, 600.0))));
    assert_eq!(w_attrs.position, None);

    // macOS: logical size + logical position survive across mixed-DPI displays.
    let mut mac = LifecycleHarness::new(OpenAs::Normal);
    mac.use_physical_size = false;
    let _ = mac.open();
    let mac_live = LiveWindowState {
        maximized: false,
        full_screen: false,
        minimized: false,
        logical_size: Some((700.0, 500.0)),
        physical_size: Some((1400, 1000)),
        position: Some(Position::Logical(LogicalPosition::new(150.0, 110.0))),
        restore_frame: None,
    };
    mac.live(mac_live);
    mac.hide_and_release(mac_live);

    let (m_attrs, _) = mac.open();
    assert_eq!(m_attrs.inner_size, Some(Size::Logical(LogicalSize::new(700.0, 500.0))));
    assert_eq!(m_attrs.position, Some(Position::Logical(LogicalPosition::new(150.0, 110.0))));
}

#[test]
fn full_screen_survives_release_and_remembers_whether_window_was_maximized_underneath() {
    // Normal -> full screen -> release -> reopen: full screen with maximized=false, normal frame kept.
    let mut h = LifecycleHarness::new(OpenAs::Normal);
    let _ = h.open();
    let normal = LiveWindowState {
        logical_size: Some((840.0, 560.0)),
        physical_size: Some((840, 560)),
        position: Some(Position::Physical(PhysicalPosition::new(1920 + 100, 80))),
        ..LiveWindowState::default()
    };
    h.live(normal);
    let full = LiveWindowState {
        maximized: false,
        full_screen: true,
        minimized: false,
        logical_size: Some((2560.0, 1440.0)),
        physical_size: Some((2560, 1440)),
        position: Some(Position::Physical(PhysicalPosition::new(1920, 0))),
        restore_frame: None,
    };
    h.live(full);
    h.hide_and_release(full);

    let (attrs, plan) = h.open();
    assert_eq!(plan.open_as, OpenAs::FullScreen { maximized: false });
    assert_eq!(attrs.fullscreen, Some(Fullscreen::Borderless(None)));
    assert!(!attrs.maximized);
    assert_eq!(attrs.inner_size, Some(Size::Physical(PhysicalSize::new(840, 560))));
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(2020, 80))));

    // Maximized -> full screen (OS clears is_maximized while full screen, as on Windows) -> release -> reopen.
    let mut h2 = LifecycleHarness::new(OpenAs::Normal);
    h2.windows_deferred_open_as = true;
    let _ = h2.open();
    h2.live(normal);
    h2.live(LiveWindowState {
        maximized: true,
        restore_frame: Some(((2020, 80), (840, 560))),
        ..LiveWindowState::default()
    });
    h2.live(full);
    h2.hide_and_release(full);

    let (win_attrs, win_plan) = h2.open();
    assert_eq!(win_plan.open_as, OpenAs::FullScreen { maximized: true });
    // On Windows, attrs carry the restored frame while open_as is applied after cloaking.
    assert!(!win_attrs.maximized);
    assert_eq!(win_attrs.fullscreen, None);
    assert_eq!(win_attrs.inner_size, Some(Size::Physical(PhysicalSize::new(840, 560))));
    assert_eq!(win_attrs.position, Some(Position::Physical(PhysicalPosition::new(2020, 80))));

    // Exiting full screen before hide clears FullScreen on the next recreate.
    h2.live(normal);
    h2.hide_and_release(normal);
    let (_, after_exit) = h2.open();
    assert_eq!(after_exit.open_as, OpenAs::Normal);
}

#[test]
fn windows_restore_frame_updates_restored_bounds_when_maximized_window_moves_monitors() {
    let mut h = LifecycleHarness::new(OpenAs::Maximized);
    h.windows_deferred_open_as = true;
    let _ = h.open();
    let moved_max = LiveWindowState {
        maximized: true,
        full_screen: false,
        minimized: false,
        logical_size: Some((1706.0, 960.0)),
        physical_size: Some((2560, 1440)),
        position: Some(Position::Physical(PhysicalPosition::new(1920, 0))),
        restore_frame: Some(((2100, 120), (1200, 900))),
    };
    h.live(moved_max);
    h.hide_and_release(moved_max);

    let (attrs, plan) = h.open();
    assert_eq!(plan.open_as, OpenAs::Maximized);
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(2100, 120))));
    assert_eq!(attrs.inner_size, Some(Size::Physical(PhysicalSize::new(1200, 900))));
}

#[test]
fn minimized_window_does_not_clobber_open_as_or_restored_bounds_with_zero_or_offscreen_rect() {
    let mut h = LifecycleHarness::new(OpenAs::Normal);
    let _ = h.open();
    h.live(LiveWindowState {
        logical_size: Some((700.0, 500.0)),
        physical_size: Some((700, 500)),
        position: Some(Position::Physical(PhysicalPosition::new(100, 100))),
        ..LiveWindowState::default()
    });
    h.live(LiveWindowState { maximized: true, ..LiveWindowState::default() });

    let minimized = LiveWindowState {
        maximized: false,
        full_screen: false,
        minimized: true,
        logical_size: None,
        physical_size: None,
        position: Some(Position::Physical(PhysicalPosition::new(-32000, -32000))),
        restore_frame: None,
    };
    h.live(minimized);
    h.hide_and_release(minimized);

    let (attrs, plan) = h.open();
    assert_eq!(plan.open_as, OpenAs::Maximized);
    assert_eq!(attrs.inner_size, Some(Size::Physical(PhysicalSize::new(700, 500))));
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(100, 100))));
}

#[test]
fn adopted_panel_and_non_resizable_window_ignore_maximized_and_full_screen() {
    let mut panel = LifecycleHarness::new(OpenAs::FullScreen { maximized: true });
    panel.panel = true;
    let (attrs, plan) = panel.open();
    assert_eq!(plan.open_as, OpenAs::Normal);
    assert!(!attrs.maximized);
    assert_eq!(attrs.fullscreen, None);

    let mut fixed = LifecycleHarness::new(OpenAs::Maximized);
    fixed.options.resizable = false;
    let (attrs, plan) = fixed.open();
    assert_eq!(plan.open_as, OpenAs::Normal);
    assert!(!attrs.maximized);
    assert_eq!(attrs.fullscreen, None);
}

#[test]
fn fit_height_and_programmatic_overrides_win_over_captured_bounds() {
    // fit_content_height never locks in an older captured size.
    let mut fit = LifecycleHarness::new(OpenAs::Normal);
    fit.track_size = false;
    let _ = fit.open();
    fit.hide_and_release(LiveWindowState {
        logical_size: Some((480.0, 300.0)),
        physical_size: Some((480, 300)),
        position: Some(Position::Physical(PhysicalPosition::new(90, 80))),
        ..LiveWindowState::default()
    });
    fit.initial_size = (480.0, 520.0);
    let (attrs, plan) = fit.open();
    assert_eq!(plan.size, None);
    assert_eq!(attrs.inner_size, Some(Size::Logical(LogicalSize::new(480.0, 520.0))));
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(90, 80))));

    // Explicit clear_position / clear_size (set_position / place / resize_window while hidden) wins.
    let mut h = LifecycleHarness::new(OpenAs::Normal);
    let _ = h.open();
    h.hide_and_release(LiveWindowState {
        logical_size: Some((700.0, 500.0)),
        physical_size: Some((700, 500)),
        position: Some(Position::Physical(PhysicalPosition::new(100, 100))),
        ..LiveWindowState::default()
    });
    h.reopen.clear_position();
    h.reopen.clear_size();
    h.options.position = Some((50, 60));
    h.initial_size = (640.0, 480.0);
    let (attrs, plan) = h.open();
    assert_eq!(plan.size, None);
    assert_eq!(plan.position, None);
    assert_eq!(attrs.inner_size, Some(Size::Logical(LogicalSize::new(640.0, 480.0))));
    assert_eq!(attrs.position, Some(Position::Physical(PhysicalPosition::new(50, 60))));
}

#[test]
fn hide_before_first_show_preserves_initial_open_as() {
    let mut h = LifecycleHarness::new(OpenAs::Maximized);
    // Windowless app receives hide() before any native window has shown.
    h.hide_and_release(LiveWindowState::default());
    let (attrs, plan) = h.open();
    assert_eq!(plan.open_as, OpenAs::Maximized);
    assert!(attrs.maximized);
}
