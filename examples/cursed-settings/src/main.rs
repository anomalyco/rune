//! Cursed Settings: a believable settings window where every control does exactly what its
//! label says. The Firewall switch builds a wall of fire, the electric fence electrifies the
//! perimeter, Dark mode turns the lights off. Effects come from rune-fx and rune-light, drawn
//! inside Rune's frame through the compositor hook (`gfx.rs`); `gags.rs` decides what burns.

mod gags;
mod gfx;
mod icons;
mod ui;

use rune::{AppSpec, TextEdit, Tx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Appearance,
    Network,
    Security,
    Accessibility,
    Developer,
}

pub const PAGES: [Page; 6] = [Page::General, Page::Appearance, Page::Network, Page::Security, Page::Accessibility, Page::Developer];

impl Page {
    fn meta(self) -> (&'static str, &'static str) {
        match self {
            Page::General => ("General", "general"),
            Page::Appearance => ("Appearance", "appearance"),
            Page::Network => ("Network", "network"),
            Page::Security => ("Security", "security"),
            Page::Accessibility => ("Accessibility", "accessibility"),
            Page::Developer => ("Developer", "developer"),
        }
    }
    pub fn title(self) -> &'static str { self.meta().0 }
    pub fn id(self) -> &'static str { self.meta().1 }
}

pub const APPEARANCES: [&str; 3] = ["Light", "Dark", "Auto"];
pub const HAPTICS: [&str; 3] = ["Off", "Light", "Strong"];

/// Every setting, at its default.
#[derive(Clone, Debug)]
pub struct Prefs {
    pub launch_at_login: bool,
    pub fast_startup: bool,
    pub spotlight: bool,
    pub auto_update: bool,
    /// 0 Light, 1 Dark, 2 Auto.
    pub appearance: usize,
    pub brightness: f32,
    pub firewall: bool,
    pub stealth: bool,
    pub static_ip: bool,
    pub fence: bool,
    pub lock_on_sleep: bool,
    pub contrast: bool,
    pub reduce_transparency: bool,
    pub reduce_motion: bool,
    /// 0 Off, 1 Light, 2 Strong.
    pub haptics: usize,
    pub hot_reload: bool,
    pub flushed: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            launch_at_login: true,
            fast_startup: false,
            spotlight: false,
            auto_update: true,
            appearance: 2,
            brightness: 1.0,
            firewall: false,
            stealth: false,
            static_ip: false,
            fence: false,
            lock_on_sleep: true,
            contrast: false,
            reduce_transparency: false,
            reduce_motion: false,
            haptics: 1,
            hot_reload: false,
            flushed: false,
        }
    }
}

pub struct Cursed {
    pub page: Page,
    pub p: Prefs,
    pub search: TextEdit,
    pub ip: TextEdit,
    /// Presses so far; the compositor turns new ones into haptic feedback at the pointer.
    pub taps: u32,
    /// Flush requested by the button; the compositor stamps `flush_t0` on its clock.
    pub flush_requested: bool,
    pub flush_t0: Option<f32>,
    /// When the restore-defaults toast goes away (compositor clock).
    pub toast_until: Option<f64>,
    /// The compositor's clock at the last frame.
    pub now: f64,
    /// Demo recordings draw a pointer (`--cursor`).
    pub show_cursor: bool,
    pub cursor: Option<[f32; 2]>,
}

impl Cursed {
    fn new() -> Cursed {
        Cursed {
            page: Page::General,
            p: Prefs::default(),
            search: TextEdit::default(),
            ip: TextEdit::new("192.168.1.42"),
            taps: 0,
            flush_requested: false,
            flush_t0: None,
            toast_until: None,
            now: 0.0,
            show_cursor: rune::extra_args().iter().any(|a| a == "--cursor"),
            cursor: None,
        }
    }

    /// Everything back to its default, calmly: each gag cools down on its own envelope.
    pub fn restore(&mut self) {
        self.p = Prefs::default();
        self.flush_t0 = None;
        self.toast_until = Some(self.now + 4.5);
    }
}

/// While the toast shows or the cache drains, time moves state forward on its own.
fn poll(s: &mut Cursed, _clock: f64, _tx: &mut Tx) -> bool {
    let mut changed = false;
    if s.toast_until.is_some_and(|u| s.now >= u) {
        s.toast_until = None;
        changed = true;
    }
    if let Some(t0) = s.flush_t0
        && s.now as f32 >= t0 + gags::FLUSH_SECONDS
        && !s.p.flushed
    {
        s.p.flushed = true;
        changed = true;
    }
    changed
}

fn page(p: Page) -> impl Fn(&mut Cursed) {
    move |s| s.page = p
}

fn main() {
    rune::run_with(
        AppSpec {
            title: "Settings",
            size: (960.0, 660.0),
            min_size: Some((860.0, 600.0)),
            init: Cursed::new,
            view: ui::view,
            scenes: &[
                ("default", |_| {}),
                ("general", |s| page(Page::General)(s)),
                ("appearance", |s| page(Page::Appearance)(s)),
                ("network", |s| page(Page::Network)(s)),
                ("security", |s| page(Page::Security)(s)),
                ("accessibility", |s| page(Page::Accessibility)(s)),
                ("developer", |s| page(Page::Developer)(s)),
                ("fire", |s| {
                    s.page = Page::Network;
                    s.p.firewall = true;
                    s.p.static_ip = true;
                }),
                ("fence", |s| {
                    s.page = Page::Security;
                    s.p.fence = true;
                }),
                ("dark", |s| {
                    s.page = Page::Appearance;
                    s.p.appearance = 1;
                }),
                ("spotlight", |s| s.p.spotlight = true),
                ("contrast", |s| {
                    s.page = Page::Accessibility;
                    s.p.contrast = true;
                }),
                ("frosted", |s| {
                    s.page = Page::Accessibility;
                    s.p.reduce_transparency = true;
                }),
                ("hot", |s| {
                    s.page = Page::Developer;
                    s.p.hot_reload = true;
                }),
                ("flushed", |s| {
                    s.page = Page::Developer;
                    s.p.flushed = true;
                }),
                ("reduced-fire", |s| {
                    s.page = Page::Network;
                    s.p.firewall = true;
                    s.p.reduce_motion = true;
                }),
            ],
            fonts: rune_theme_opencode::FONTS,
            families: rune_theme_opencode::FAMILIES,
            setup: None,
            poll: Some(poll),
            on_key: None,
        },
        gfx::Gfx::default(),
    );
}
