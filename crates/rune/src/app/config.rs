//! Runtime-level settings and hooks shared across every `App` and `ConfiguredOverlay`
//! bootstrap site (native window, overlay, web, headless, virtual desktop, playground tests,
//! hot reload and HTML export).

use crate::el::Tx;
use crate::runtime::Runtime;

/// Settings and callbacks copied from an `App` or `ConfiguredOverlay` into its `Runtime`.
///
/// Exhaustively destructured in [`Runtime::configure`] so adding a field updates every host
/// together instead of drifting across bootstrap sites.
pub(crate) struct RuntimeConfig<S: 'static> {
    pub(crate) on_focus: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_mods: Option<fn(&mut S, crate::Mods, &mut Tx)>,
    pub(crate) on_visibility: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_ime: Option<fn(&mut S, &crate::Ime, &mut Tx) -> bool>,
    pub(crate) modifier_keys: bool,
    pub(crate) tab_visits_controls: bool,
    pub(crate) srgb_blend: bool,
    pub(crate) font_smoothing: crate::FontSmoothing,
    pub(crate) dev_stats: Option<fn() -> String>,
}

impl<S: 'static> Clone for RuntimeConfig<S> {
    fn clone(&self) -> Self { *self }
}

impl<S: 'static> Copy for RuntimeConfig<S> {}

impl<S: 'static> Default for RuntimeConfig<S> {
    fn default() -> Self {
        Self {
            on_focus: None,
            on_mods: None,
            on_visibility: None,
            on_ime: None,
            modifier_keys: false,
            tab_visits_controls: false,
            srgb_blend: true,
            font_smoothing: crate::FontSmoothing::default(),
            dev_stats: None,
        }
    }
}

#[cfg(feature = "hot")]
impl<S: 'static> RuntimeConfig<S> {
    /// Copies the state-independent settings onto a `RuntimeConfig` for another state type
    /// (hot-reload's type-erased `HostState`).
    pub(crate) fn retype_settings<T: 'static>(self) -> RuntimeConfig<T> {
        let Self {
            on_focus: _,
            on_mods: _,
            on_visibility: _,
            on_ime: _,
            modifier_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            dev_stats,
        } = self;
        RuntimeConfig {
            on_focus: None,
            on_mods: None,
            on_visibility: None,
            on_ime: None,
            modifier_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            dev_stats,
        }
    }
}

impl crate::runtime::RuntimeCore {
    pub(crate) fn configure_core(
        &mut self,
        modifier_keys: bool,
        tab_visits_controls: bool,
        srgb_blend: bool,
        font_smoothing: crate::FontSmoothing,
        dev_stats: Option<fn() -> String>,
    ) {
        self.set_modifier_keys(modifier_keys);
        self.set_tab_visits_controls(tab_visits_controls);
        self.set_srgb_blend(srgb_blend);
        self.set_font_smoothing(font_smoothing);
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
            modifier_keys,
            tab_visits_controls,
            srgb_blend,
            font_smoothing,
            dev_stats,
        } = config;
        self.hooks.on_focus = on_focus;
        self.hooks.on_mods = on_mods;
        self.hooks.on_visibility = on_visibility;
        self.hooks.on_ime = on_ime;
        self.core_mut().configure_core(modifier_keys, tab_visits_controls, srgb_blend, font_smoothing, dev_stats);
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
            .modifier_keys()
            .tab_visits_controls(true)
            .srgb_blend(false)
            .font_smoothing(FontSmoothing::Off)
            .dev_stats(stats)
    }

    #[test]
    fn headless_runtime_applies_every_runtime_config_field() {
        let mut rt = configured_app().headless_runtime((200.0, 120.0), Env::default());
        assert!(rt.hooks.on_focus.is_some());
        assert!(rt.hooks.on_mods.is_some());
        assert!(rt.hooks.on_visibility.is_some());
        assert!(rt.hooks.on_ime.is_some());
        assert!(rt.modifier_keys());
        assert!(!rt.srgb_blend());
        assert_eq!(rt.font_smoothing(), FontSmoothing::Off);
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
    }

    #[test]
    fn runtime_config_defaults_match_app_and_overlay() {
        let cfg = super::RuntimeConfig::<()>::default();
        assert!(cfg.srgb_blend);
        assert!(!cfg.modifier_keys && !cfg.tab_visits_controls);
        assert_eq!(cfg.font_smoothing, FontSmoothing::default());
        assert!(cfg.on_focus.is_none() && cfg.on_mods.is_none() && cfg.on_visibility.is_none() && cfg.on_ime.is_none() && cfg.dev_stats.is_none());
    }
}
