use super::*;
use crate::native_surface::{NativeSurface, SurfaceFit, synthetic};
use crate::{Align, Color, Cx, El, Env, ManualClock, Runtime, Tx, col, el};
use std::time::Duration;

fn config() -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 400,
        height: 200,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    }
}

struct SurfaceState {
    primary: NativeSurface,
    secondary: NativeSurface,
    use_secondary: bool,
    show_both: bool,
    swap_order: bool,
    fit: SurfaceFit,
    anchor: [Align; 2],
    unscaled: bool,
    size: (f32, f32),
    offset_x: f32,
    radius: f32,
    clip_width: f32,
    fade_top: f32,
    opacity: f32,
    rect_before: bool,
    polls: u32,
    poll_changes: bool,
}

impl SurfaceState {
    fn new() -> Self {
        let primary = NativeSurface::new();
        primary.push_bytes(synthetic(16, 16, 1), 1).unwrap();
        let secondary = NativeSurface::new();
        secondary.push_bytes(synthetic(16, 16, 2), 2).unwrap();
        Self {
            primary,
            secondary,
            use_secondary: false,
            show_both: false,
            swap_order: false,
            fit: SurfaceFit::Fit,
            anchor: [Align::Center, Align::Center],
            unscaled: false,
            size: (120.0, 80.0),
            offset_x: 0.0,
            radius: 0.0,
            clip_width: 200.0,
            fade_top: 0.0,
            opacity: 1.0,
            rect_before: false,
            polls: 0,
            poll_changes: false,
        }
    }
}

fn surface_view(s: &SurfaceState, _: &mut Cx<SurfaceState>) -> El {
    let active = if s.use_secondary { &s.secondary } else { &s.primary };
    let make_surface = |surf: &NativeSurface, key: &'static str| {
        el().key(key)
            .size(s.size.0, s.size.1)
            .radius(s.radius)
            .opacity(s.opacity)
            .native_surface(surf, s.fit)
            .surface_anchor(s.anchor[0], s.anchor[1])
            .surface_unscaled(s.unscaled)
    };
    let mut inner = col().key("inner").w(s.clip_width).h(100.0).clip_rounded(0.0);
    if s.fade_top > 0.0 {
        inner = inner.clip_fade(0.0, s.fade_top, 0.0, 0.0);
    }
    let badge = el().key("badge").size(10.0, 10.0).fill(Color::WHITE);
    if s.rect_before {
        inner = inner.child(badge);
    }
    if s.show_both {
        let (first, second) = if s.swap_order { (&s.secondary, &s.primary) } else { (&s.primary, &s.secondary) };
        inner = inner
            .child(make_surface(first, "s1"))
            .child(make_surface(second, "s2"));
    } else {
        inner = inner.child(el().key("pad").pl(s.offset_x).child(make_surface(active, "s1")));
    }
    if !s.rect_before {
        inner = inner.child(el().key("badge-after").size(10.0, 10.0).fill(Color::WHITE));
    }
    el().key("root").size(200.0, 100.0).child(inner)
}

const PERIOD: Duration = Duration::from_secs(5);

fn surface_poll(s: &mut SurfaceState, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    tx.wake_after(PERIOD);
    s.poll_changes
}

/// Present-counting resting window harness with a `NativeSurface`, matching `Host`'s timer-wake
/// and quiet-timer paths.
struct SurfaceHost {
    rt: Runtime<SurfaceState>,
    presented: Option<Presented>,
    config: wgpu::SurfaceConfiguration,
    idle: bool,
    presents: u32,
}

impl SurfaceHost {
    fn new() -> Self {
        let mut rt = Runtime::new(SurfaceState::new(), surface_view, (200.0, 100.0), Env::new(false, 2.0));
        rt.frame.set_test_clock(ManualClock::new(1.0 / crate::frame::clock::RESUME_STEP));
        rt.poll = Some(surface_poll);
        let mut host = Self { rt, presented: None, config: config(), idle: false, presents: 0 };
        host.frame(false);
        host
    }

    fn frame(&mut self, quiet: bool) -> bool {
        self.rt.frame.test_clock_mut().unwrap().step();
        let animating = self.rt.frame(crate::frame::clock::RESUME_STEP);
        if quiet && self.presented.as_ref().is_some_and(|p| p.unchanged(&self.rt, &self.config, animating, None)) {
            return false;
        }
        self.idle = Presented::settle(&mut self.presented, &self.rt, &self.config, animating, None, true, false);
        self.presents += 1;
        true
    }

    fn timer_wake(&mut self) -> bool {
        let due = self.rt.wake_deadline().expect("a timer is armed");
        let clock = self.rt.frame.test_clock_mut().unwrap();
        clock.seek(clock.now().max(due));
        let rests = self.idle && self.presented.is_some();
        let frame = match rests.then(|| self.rt.resting_wake()).flatten() {
            Some(stale) => {
                let frame = stale || self.rt.stale();
                self.rt.resting_wake_frame(frame);
                frame
            }
            None => true,
        };
        frame && self.frame(true)
    }
}

#[test]
fn quiet_timer_on_resting_window_with_unchanged_native_surface_skips_present_and_presents_on_change() {
    let mut host = SurfaceHost::new();
    assert!(host.idle && host.presented.is_some(), "resting frame records Presented");
    assert_eq!(host.rt.surfaces.len(), 1);
    assert_eq!(host.presents, 1);

    // Waiting work still keeps due timers on the quiet-timer path (so pull_video runs).
    assert!(!host.rt.rests(), "a native surface keeps due timers on the quiet-timer path");

    // Repeated quiet timer wakes with no visual change present nothing (whether poll returns false
    // or returns true without changing pixels).
    for poll_changes in [false, true] {
        host.rt.state.poll_changes = poll_changes;
        let presents = host.presents;
        let polls = host.rt.state.polls;
        for _ in 0..4 {
            assert!(!host.timer_wake(), "unchanged NativeSurface must not present (poll_changes={poll_changes})");
        }
        assert_eq!(host.presents, presents, "present count stays unchanged while surface is still");
        assert_eq!(host.rt.state.polls, polls + 4, "each timer still polled");
        let p = host.presented.as_ref().unwrap();
        assert!(p.same(&host.rt, &host.config));
        assert!(p.unchanged(&host.rt, &host.config, false, None));
    }

    // Pushing a new picture presents once, then rests again.
    host.rt.state.poll_changes = false;
    host.rt.state.primary.push_bytes(synthetic(16, 16, 9), 2).unwrap();
    assert!(!host.presented.as_ref().unwrap().same(&host.rt, &host.config));
    assert!(host.timer_wake(), "a newly pushed frame must present");
    assert_eq!(host.presents, 2);
    assert!(!host.timer_wake(), "after presenting the new frame, subsequent unchanged wakes rest");
    assert_eq!(host.presents, 2);

    // Mirroring flips the stamp and presents once; repeating the same mirror value rests.
    host.rt.state.primary.set_mirrored(true);
    assert!(host.timer_wake(), "set_mirrored(true) must present");
    assert_eq!(host.presents, 3);
    host.rt.state.primary.set_mirrored(true);
    assert!(!host.timer_wake(), "redundant set_mirrored(true) must not present");
    assert_eq!(host.presents, 3);

    // Clearing a populated surface presents once; clearing again while empty rests.
    host.rt.state.primary.clear();
    assert!(host.timer_wake(), "clearing a populated surface must present");
    assert_eq!(host.presents, 4);
    host.rt.state.primary.clear();
    assert!(!host.timer_wake(), "clearing an already-empty surface must not present");
    assert_eq!(host.presents, 4);

    // Re-pushing a frame presents again (no ABA collision with prior cleared frame).
    host.rt.state.primary.push_bytes(synthetic(16, 16, 1), 1).unwrap();
    assert!(host.timer_wake(), "re-pushing after clear must present");
    assert_eq!(host.presents, 5);
    assert!(!host.timer_wake());
    assert_eq!(host.presents, 5);
}

#[test]
fn presented_same_detects_every_native_surface_draw_property_change() {
    let mut host = SurfaceHost::new();
    let assert_changed_and_rests = |host: &mut SurfaceHost, label: &str, mutate: fn(&mut SurfaceState)| {
        let before = host.presents;
        host.rt.update(|s, tx| {
            mutate(s);
            tx.instant();
        });
        assert!(host.timer_wake(), "{label} must present");
        assert_eq!(host.presents, before + 1, "{label} presents exactly once");
        assert!(host.idle && host.presented.is_some(), "{label} settles after instant change");
        assert!(!host.timer_wake(), "{label} rests on the next unchanged timer wake");
        assert_eq!(host.presents, before + 1);
    };

    // Different NativeSurface handle.
    assert_changed_and_rests(&mut host, "different surface", |s| s.use_secondary = true);
    // Geometry (rect position and size).
    assert_changed_and_rests(&mut host, "rect position", |s| s.offset_x = 12.0);
    assert_changed_and_rests(&mut host, "rect size", |s| s.size = (100.0, 60.0));
    // Radii.
    assert_changed_and_rests(&mut host, "radii", |s| s.radius = 8.0);
    // Fit.
    assert_changed_and_rests(&mut host, "fit", |s| s.fit = SurfaceFit::Fill);
    // Place (anchor and unscaled).
    assert_changed_and_rests(&mut host, "place anchor", |s| s.anchor = [Align::Start, Align::End]);
    assert_changed_and_rests(&mut host, "place unscaled", |s| s.unscaled = true);
    // Clip.
    assert_changed_and_rests(&mut host, "clip", |s| s.clip_width = 90.0);
    // Fade.
    assert_changed_and_rests(&mut host, "fade", |s| s.fade_top = 16.0);
    // Opacity.
    assert_changed_and_rests(&mut host, "opacity", |s| s.opacity = 0.5);
    // Painter order relative to instances (before_instance).
    assert_changed_and_rests(&mut host, "before_instance", |s| s.rect_before = true);
    // Surface count and order.
    assert_changed_and_rests(&mut host, "add second surface", |s| s.show_both = true);
    assert_changed_and_rests(&mut host, "swap surface order", |s| s.swap_order = true);
}

#[test]
fn composed_or_unrested_native_surface_frame_never_blocks_next_present() {
    let mut host = SurfaceHost::new();
    assert!(host.presented.is_some());

    // When surfaces_pending is true during settle, rests is false so Presented is cleared even
    // though the window goes idle waiting for the pipeline build notification.
    let idle = Presented::settle(&mut host.presented, &host.rt, &host.config, false, None, false, false);
    assert!(idle && host.presented.is_none(), "unrested frame (e.g. surfaces_pending) clears Presented");

    // A composed frame clears stored surface stamps and never matches as unchanged.
    Presented::record(&mut host.presented, &host.rt, &host.config, true);
    let p = host.presented.as_ref().unwrap();
    assert!(p.surfaces.is_empty(), "composed frame clears recorded surface stamps");
    assert!(!p.same(&host.rt, &host.config), "composed frame never counts as same");
}
