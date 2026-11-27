//! Runtime-level settings and hooks shared across every `App` and `ConfiguredOverlay`
//! bootstrap site (native window, overlay, web, headless, virtual desktop, playground tests,
//! hot reload and HTML export).

use crate::el::Tx;
use crate::runtime::Runtime;

/// Normalizes an opt-in frame-rate cap (Hz): positive finite values clamp to at least `1.0` Hz,
/// while zero, negative, NaN and infinite values normalize to `0.0` (uncapped display rate).
#[inline]
pub(crate) fn normalize_fps_cap(fps: f32) -> f32 {
    if fps.is_finite() && fps > 0.0 {
        fps.max(1.0)
    } else {
        0.0
    }
}

/// Converts an opt-in `ambient_fps` rate (Hz) into its target frame period, or `None` when
/// uncapped (zero, negative, non-finite, or so large that its period rounds to zero).
#[inline]
pub(crate) fn ambient_period_from_fps(fps: f32) -> Option<std::time::Duration> {
    let hz = normalize_fps_cap(fps);
    (hz > 0.0)
        .then(|| std::time::Duration::from_secs_f64(1.0 / f64::from(hz)))
        .filter(|period| !period.is_zero())
}

/// Normalizes a wheel-notch scroll distance (window points per line/notch): positive finite
/// values override the platform default; zero, negative, NaN, and infinite values normalize
/// to `None` (platform default).
#[inline]
pub(crate) fn normalize_wheel_line(points: f32) -> Option<f32> {
    (points.is_finite() && points > 0.0).then_some(points)
}

/// Settings and callbacks copied from an `App` or `ConfiguredOverlay` into its `Runtime`.
///
/// Exhaustively destructured in [`Runtime::configure`] so adding a field updates every host
/// together instead of drifting across bootstrap sites.
pub(crate) struct RuntimeConfig<S: 'static> {
    pub(crate) on_focus: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_mods: Option<fn(&mut S, crate::Mods, &mut Tx)>,
    pub(crate) on_visibility: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_ime: Option<fn(&mut S, &crate::Ime, &mut Tx) -> bool>,
    pub(crate) on_field_menu: Option<fn(&mut S, crate::FieldMenuRequest, &mut Tx)>,
    pub(crate) on_popup_menu: Option<fn(&mut S, crate::PopupMenuRequest, &mut Tx)>,
    pub(crate) modifier_keys: bool,
    pub(crate) media_keys: bool,
    pub(crate) tab_visits_controls: bool,
    pub(crate) srgb_blend: bool,
    pub(crate) font_smoothing: crate::FontSmoothing,
    pub(crate) emoji_family: Option<String>,
    pub(crate) max_native_materials: usize,
    pub(crate) reveal_fps: f32,
    pub(crate) wheel_glide: bool,
    pub(crate) wheel_line: Option<f32>,
    pub(crate) dev_stats: Option<fn() -> String>,
}

impl<S: 'static> RuntimeConfig<S> {
    /// Normalizes a wake-rate cap (Hz) for `El::reveal` word fades: finite positive values clamp
    /// to at least `1.0` Hz, while zero, negative, NaN and infinite values normalize to `0.0`
    /// (uncapped display rate).
    #[inline]
    pub(crate) fn normalize_reveal_fps(fps: f32) -> f32 {
        if fps.is_finite() && fps > 0.0 {
            fps.max(1.0)
        } else {
            0.0
        }
    }
}

impl<S: 'static> Clone for RuntimeConfig<S> {
    fn clone(&self) -> Self {
        Self {
            on_focus: self.on_focus,
            on_mods: self.on_mods,
            on_visibility: self.on_visibility,
            on_ime: self.on_ime,
            on_field_menu: self.on_field_menu,
            on_popup_menu: self.on_popup_menu,
            modifier_keys: self.modifier_keys,
            media_keys: self.media_keys,
            tab_visits_controls: self.tab_visits_controls,
            srgb_blend: self.srgb_blend,
            font_smoothing: self.font_smoothing,
            emoji_family: self.emoji_family.clone(),
            max_native_materials: self.max_native_materials,
            reveal_fps: self.reveal_fps,
            wheel_glide: self.wheel_glide,
            wheel_line: self.wheel_line,
            dev_stats: self.dev_stats,
        }
    }
}

impl<S: 'static> Default for RuntimeConfig<S> {
    fn default() -> Self {
        Self {
            on_focus: None,
            on_mods: None,
            on_visibility: None,
            on_ime: None,
            on_field_menu: None,
            on_popup_menu: None,
            modifier_keys: false,
            media_keys: false,
            tab_visits_controls: false,
            srgb_blend: true,
            font_smoothing: crate::FontSmoothing::default(),
            emoji_family: None,
            max_native_materials: crate::MAX_NATIVE_MATERIALS,
            reveal_fps: 0.0,
            wheel_glide: true,
            wheel_line: None,
            dev_stats: None,
        }
    }
}

pub(crate) struct CoreConfig {
    pub(crate) modifier_keys: bool,
    pub(crate) media_keys: bool,
    pub(crate) tab_visits_controls: bool,
    pub(crate) srgb_blend: bool,
    pub(crate) font_smoothing: crate::FontSmoothing,
    pub(crate) emoji_family: Option<String>,
    pub(crate) max_native_materials: usize,
    pub(crate) wheel_glide: bool,
    pub(crate) wheel_line: Option<f32>,
    pub(crate) dev_stats: Option<fn() -> String>,
}

impl crate::runtime::RuntimeCore {
    pub(crate) fn configure_core(&mut self, core: CoreConfig) {
        let CoreConfig {
            modifier_keys,
            media_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            emoji_family,
            max_native_materials,
            wheel_glide,
            wheel_line,
            dev_stats,
        } = core;
        self.set_modifier_keys(modifier_keys);
        self.set_media_keys(media_keys);
        self.set_tab_visits_controls(tab_visits_controls);
        self.set_srgb_blend(srgb_blend);
        self.set_font_smoothing(font_smoothing);
        if let Some(emoji) = emoji_family {
            self.set_emoji_family(Some(&emoji));
        }
        self.set_max_native_materials(max_native_materials);
        self.set_wheel_glide(wheel_glide);
        self.set_wheel_line(wheel_line);
        self.host.set_app_stats(dev_stats);
    }
}

impl<S: 'static> Runtime<S> {
    /// Applies every runtime-level setting and callback collected by `App` or `ConfiguredOverlay`.
    pub(crate) fn configure(&mut self, config: RuntimeConfig<S>) {
        let RuntimeConfig {
            on_focus,
            on_mods,
            on_visibility,
            on_ime,
            on_field_menu,
            on_popup_menu,
            modifier_keys,
            media_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            emoji_family,
            max_native_materials,
            reveal_fps,
            wheel_glide,
            wheel_line,
            dev_stats,
        } = config;
        self.core_mut().set_reveal_fps(reveal_fps);
        self.hooks.on_focus = on_focus;
        self.hooks.on_mods = on_mods;
        self.hooks.on_visibility = on_visibility;
        self.hooks.on_ime = on_ime;
        self.hooks.on_field_menu = on_field_menu;
        self.hooks.on_popup_menu = on_popup_menu;
        self.core_mut().configure_core(CoreConfig {
            modifier_keys,
            media_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            emoji_family,
            max_native_materials,
            wheel_glide,
            wheel_line,
            dev_stats,
        });
    }
}

impl<S: 'static> crate::App<S> {
    /// Fallback handler for pop-up and context menus on platforms where
    /// [`crate::popup_menu_supported`] is `false` and not on the web (such as Linux).
    ///
    /// When set and `!popup_menu_supported() && !cfg!(target_arch = "wasm32")`, Rune hands
    /// every validated pop-up menu request (app [`Tx::popup_menu`], selectable-text Copy menus,
    /// default link/image menus, and text-field menus when [`Self::on_field_menu`] is unset) as
    /// a [`crate::PopupMenuRequest`] to `f` instead of answering `None` immediately. Render it
    /// in-window (for example with `rune_kit::Menu::from_popup_menu`) and answer with
    /// [`Tx::resolve_popup_menu`]. On macOS, Windows, and web, this hook is never called.
    ///
    /// ```
    /// use rune::{App, AppSpec, PopupMenuRequest, Tx, el};
    ///
    /// struct S { menu: Option<PopupMenuRequest> }
    /// let _app = App::new(AppSpec::new("demo", (480.0, 320.0), || S { menu: None }, |_, _| el()))
    ///     .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _tx: &mut Tx| {
    ///         s.menu = Some(req);
    ///     });
    /// ```
    pub fn on_popup_menu(mut self, f: fn(&mut S, crate::PopupMenuRequest, &mut Tx)) -> Self {
        self.runtime.on_popup_menu = Some(f);
        self
    }

    /// Controls whether Rune calls glibc's `malloc_trim(0)` once off the UI thread after a
    /// window or overlay finishes resting on Linux (`target_env = "gnu"`).
    ///
    /// Enabled by default (`true`). At rest, Rune releases unused text layouts, glyph runs,
    /// images, dormant scene slots, and GPU caches; on Linux with glibc, fragmented free arena
    /// chunks remain resident in `VmRSS` until `malloc_trim(0)` runs. Rune runs it once per
    /// rest on its shared task pool, coalesced across windows and overlays with a 30 s cooldown,
    /// never on a periodic idle timer.
    ///
    /// While `malloc_trim(0)` runs off the UI thread after rest, glibc locks each arena in turn
    /// while trimming it: on very large freed heaps (~387 MiB), another thread allocating in that
    /// same arena can wait ~14 ms (longer than one 120 Hz frame at 8.3 ms). Apps with allocating
    /// real-time threads can opt out with `.trim_heap_at_rest(false)` or `RUNE_MALLOC_TRIM=0`.
    /// (Rune's own `AudioOutput` worker thread allocates nothing while playing.)
    ///
    /// At runtime Rune probes once whether Rust's global allocator is backed by glibc `malloc`;
    /// binaries using a custom `#[global_allocator]` (such as mimalloc or jemalloc) or an
    /// `LD_PRELOAD` allocator skip the trim automatically. Ignored on macOS, Windows, musl,
    /// Android, iOS, and `wasm32`.
    ///
    /// ```
    /// use rune::{App, AppSpec, el};
    ///
    /// let _app = App::new(AppSpec::new("Audio", (640.0, 480.0), || (), |_, _| el()))
    ///     .trim_heap_at_rest(false);
    /// ```
    pub fn trim_heap_at_rest(self, enabled: bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        crate::window::rest::set_heap_trim_enabled(enabled);
        #[cfg(target_arch = "wasm32")]
        let _ = enabled;
        self
    }

    /// Controls whether whole mouse-wheel notches ease smoothly rather than jumping immediately
    /// for this app's window and overlays (default `true`; reduced motion still jumps).
    ///
    /// Overlays added with [`Self::overlay`] (before or after this call) inherit this setting
    /// unless overridden with [`ConfiguredOverlay::wheel_glide`](crate::ConfiguredOverlay::wheel_glide).
    /// Touchpad scrolling is unaffected.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Launcher", (480.0, 320.0), || (), |_, _| el()))
    ///     .wheel_glide(false)
    ///     .run();
    /// ```
    pub fn wheel_glide(mut self, on: bool) -> Self {
        self.runtime.wheel_glide = on;
        #[cfg(not(target_arch = "wasm32"))]
        for overlay in &mut self.overlays {
            overlay.inherit_wheel_glide(on);
        }
        self
    }

    /// Sets how many window points one mouse-wheel line/notch scrolls for this app's window and
    /// overlays, overriding the platform default (Windows `SPI_GETWHEELSCROLLLINES`, 120 pt on
    /// Linux, 40 pt on macOS and web).
    ///
    /// Positive finite values override the platform default; zero, negative, or non-finite values
    /// restore the platform default. Overlays added with [`Self::overlay`] (before or after this
    /// call) inherit this setting unless overridden with
    /// [`ConfiguredOverlay::wheel_line`](crate::ConfiguredOverlay::wheel_line). Touchpad point
    /// deltas and Ctrl+wheel pinch-zoom (`on_magnify`) are unaffected.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Launcher", (480.0, 320.0), || (), |_, _| el()))
    ///     .wheel_line(60.0)
    ///     .wheel_glide(false)
    ///     .run();
    /// ```
    pub fn wheel_line(mut self, points: f32) -> Self {
        let points = normalize_wheel_line(points);
        self.runtime.wheel_line = points;
        #[cfg(not(target_arch = "wasm32"))]
        for overlay in &mut self.overlays {
            overlay.inherit_wheel_line(points);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::{App, AppSpec, Cx, Env, FontSmoothing, Ime, Key, KeyEvent, Mods, PhysicalInput, el};

    #[derive(Default)]
    struct Seen {
        focus: Vec<bool>,
        mods: Vec<Mods>,
        visibility: Vec<bool>,
        ime: usize,
    }

    fn stats() -> String { "ok".into() }

    fn configured_app() -> App<Seen> {
        let spec = AppSpec::new("config", (200.0, 120.0), Seen::default, |_: &Seen, _: &mut Cx<Seen>| {
            el().key("btn").size(40.0, 20.0).focusable(true)
        });
        App::new(spec)
            .on_focus(|s, f, _| s.focus.push(f))
            .on_modifiers(|s, m, _| s.mods.push(m))
            .on_visibility(|s, v, _| s.visibility.push(v))
            .on_ime(|s, _: &Ime, _| { s.ime += 1; true })
            .on_field_menu(|_, _, _| {})
            .on_popup_menu(|_, _, _| {})
            .modifier_keys()
            .media_keys()
            .tab_visits_controls(true)
            .srgb_blend(false)
            .font_smoothing(FontSmoothing::Off)
            .emoji_family("Twemoji")
            .max_native_materials(6)
            .reveal_fps(30.0)
            .wheel_glide(false)
            .wheel_line(60.0)
            .dev_stats(stats)
    }

    #[test]
    fn headless_runtime_applies_every_runtime_config_field() {
        let _guard = crate::text::with_families("Inter", "ui-monospace");
        let mut rt = configured_app().headless_runtime((200.0, 120.0), Env::default());
        assert!(rt.hooks.on_focus.is_some());
        assert!(rt.hooks.on_mods.is_some());
        assert!(rt.hooks.on_visibility.is_some());
        assert!(rt.hooks.on_ime.is_some());
        assert!(rt.hooks.on_field_menu.is_some());
        assert!(rt.hooks.on_popup_menu.is_some());
        assert!(rt.modifier_keys());
        assert!(rt.media_keys());
        assert!(!rt.srgb_blend());
        assert_eq!(rt.font_smoothing(), FontSmoothing::Off);
        assert_eq!(rt.emoji_family(), Some("Twemoji"));
        assert_eq!(rt.max_native_materials(), 6);
        assert_eq!(rt.reveal_fps(), 30.0);
        assert!(!rt.wheel_glide());
        assert_eq!(rt.wheel_line(), Some(60.0));
        assert_eq!(rt.host.app_stats.map(|f| f()).as_deref(), Some("ok"));

        // tab_visits_controls overrides full_keyboard_access(false) so Tab reaches a focusable control.
        rt.set_full_keyboard_access(false);
        rt.frame(0.0);
        rt.window_focus(true);
        let alt = Mods { alt: true, ..Mods::default() };
        rt.physical_observation(PhysicalInput::Modifiers(alt));
        rt.set_live_visible(false);
        rt.ime(Ime::Commit("x".into()));
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(0.0);

        assert_eq!(rt.state.focus, [true]);
        assert_eq!(rt.state.mods, [alt]);
        assert_eq!(rt.state.visibility, [false]);
        assert_eq!(rt.state.ime, 1);
        assert!(rt.has_focus(), "tab_visits_controls lets Tab focus a control with full keyboard access off");
        rt.set_emoji_family(None);
        assert_eq!(rt.emoji_family(), None);
        for invalid in [0.0, -10.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            rt.set_wheel_line(Some(invalid));
            assert_eq!(rt.wheel_line(), None, "invalid wheel_line {invalid} normalizes to None");
        }
        rt.set_wheel_line(Some(45.0));
        assert_eq!(rt.wheel_line(), Some(45.0));
        rt.set_wheel_line(None);
        assert_eq!(rt.wheel_line(), None);
    }

    #[test]
    fn runtime_config_defaults_match_app_and_overlay() {
        let cfg = super::RuntimeConfig::<()>::default();
        assert!(cfg.srgb_blend);
        assert!(!cfg.modifier_keys && !cfg.media_keys && !cfg.tab_visits_controls);
        assert_eq!(cfg.font_smoothing, FontSmoothing::default());
        assert_eq!(cfg.emoji_family, None);
        assert_eq!(cfg.max_native_materials, crate::MAX_NATIVE_MATERIALS);
        assert_eq!(cfg.reveal_fps, 0.0);
        assert!(cfg.wheel_glide);
        assert_eq!(cfg.wheel_line, None);
        assert!(cfg.on_focus.is_none() && cfg.on_mods.is_none() && cfg.on_visibility.is_none() && cfg.on_ime.is_none() && cfg.on_field_menu.is_none() && cfg.on_popup_menu.is_none() && cfg.dev_stats.is_none());
    }
}
