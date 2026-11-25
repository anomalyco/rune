//! View environment (`Env`), background wake handle (`Waker`) and view context (`Cx`).
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Env {
    pub system_dark: bool,
    pub scale: f32,
}
impl Env {
    pub const fn new(system_dark: bool, scale: f32) -> Self {
        Self { system_dark, scale }
    }
}
impl Default for Env {
    fn default() -> Self { Self::new(true, 1.0) }
}

/// What a view declares for the elements that don't style themselves.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewDefaults {
    /// [`Cx::foreground`].
    pub(crate) foreground: Option<Color>,
    /// [`Cx::focus_ring`].
    pub(crate) focus_ring: Option<crate::el::Outline>,
    /// [`Cx::help_look`].
    pub(crate) help: Option<super::HelpLook>,
    /// [`Cx::press_cursor`].
    pub(crate) press_cursor: Option<crate::el::Cursor>,
    /// [`Cx::web_focus`].
    pub(crate) web_focus: bool,
    /// [`Cx::force_focus_rings`].
    pub(crate) force_focus_rings: bool,
    /// [`Cx::mark_texts`].
    pub(crate) marks: Rc<[super::TextMark]>,
}

pub use rune_core::Waker;

thread_local! {
    pub(super) static COMMITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many commits (runs of a view that go on to the screen) have started on this thread,
/// across all its windows; 0 before the first. A cache of what views draw can keep whatever
/// the last commit or two used and evict the rest.
pub fn commit_generation() -> u64 {
    COMMITS.get()
}

/// Type-erased view context owned inside [`Cx<S>`] and constructed by non-generic
/// [`Measurement`](super::commit::Measurement).
#[derive(Clone)]
pub(crate) struct CxCore {
    pub(super) env: Env,
    pub(super) titlebar_inset: f32,
    pub(super) chrome: crate::ChromeInsets,
    pub(super) maximized: bool,
    pub(super) full_screen: bool,
    /// `Cx::caption_color`, read once the view returns.
    pub(super) caption_color: Option<Color>,
    /// `Cx::foreground` and `Cx::focus_ring`, read once the view returns.
    pub(super) defaults: ViewDefaults,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, read once the view returns.
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) system_accent: crate::SystemAccent,
    pub(super) system_contrast: Option<crate::SystemContrast>,
    pub(super) system_increased_contrast: bool,
    pub(super) system_scroll_bars: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) system_languages: Option<Rc<[String]>>,
    pub(super) system_reduced_motion: bool,
    pub(super) pointer_locked: bool,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    /// [`Cx::zoom`], read once the view returns.
    pub(super) zoom: f32,
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) caret_read: std::cell::Cell<bool>,
    pub(super) layout: layout_read::LayoutRects,
    pub(super) layout_reads: std::cell::RefCell<layout_read::LayoutReads>,
}

/// The host's safe-area insets in points, top/right/bottom/left: the browser's on the web,
/// zero on native.
pub(crate) fn safe_area_insets() -> [f32; 4] {
    #[cfg(target_arch = "wasm32")]
    { crate::platform::web::safe_area() }
    #[cfg(not(target_arch = "wasm32"))]
    { [0.0; 4] }
}

impl CxCore {
    pub(super) fn absorb(&mut self, other: CxCore) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.zoom = other.zoom;
        self.defaults = other.defaults;
        self.caret_read.set(self.caret_read.get() || other.caret_read.get());
        let mut reads = self.layout_reads.borrow_mut();
        for read in other.layout_reads.into_inner() {
            layout_read::note_read_at(&mut reads, &read.key, read.same, read.at);
        }
    }

    pub(crate) fn set_env(&mut self, env: Env) {
        self.env = env;
    }

    pub(crate) fn clear_chrome(&mut self) {
        self.titlebar_inset = 0.0;
        self.chrome = crate::ChromeInsets::default();
        self.maximized = false;
        self.full_screen = false;
    }

    pub(crate) fn env(&self) -> Env { self.env }
    pub(crate) fn system_dark(&self) -> bool { self.env.system_dark }
    pub(crate) fn titlebar_inset(&self) -> f32 { self.titlebar_inset }
    pub(crate) fn chrome_insets(&self) -> crate::ChromeInsets { self.chrome }
    pub(crate) fn is_maximized(&self) -> bool { self.maximized }
    pub(crate) fn is_full_screen(&self) -> bool { self.full_screen }
    pub(crate) fn caption_color(&mut self, color: Color) { self.caption_color = Some(color); }
    pub(crate) fn foreground(&mut self, color: Color) { self.defaults.foreground = Some(color); }
    pub(crate) fn focus_ring(&mut self, width: f32, offset: f32, color: Color) {
        self.defaults.focus_ring = Some(crate::el::Outline { width, offset, color });
    }
    pub(crate) fn help_look(&mut self, look: crate::HelpLook) { self.defaults.help = Some(look); }
    pub(crate) fn press_cursor(&mut self, cursor: crate::el::Cursor) { self.defaults.press_cursor = Some(cursor); }
    pub(crate) fn force_focus_rings(&mut self) { self.defaults.force_focus_rings = true; }
    pub(crate) fn web_focus(&mut self) { self.defaults.web_focus = true; }
    pub(crate) fn titlebar_height(&mut self, points: f32) { self.titlebar.height = Some(points.max(1.0)); }
    pub(crate) fn set_titlebar(&mut self, titlebar: crate::Titlebar) { self.titlebar.titlebar = Some(titlebar); }
    pub(crate) fn accent(&self) -> Color { self.system_accent.accent(self.env.system_dark) }
    pub(crate) fn focus_color(&self) -> Color { self.system_accent.focus(self.env.system_dark) }
    pub(crate) fn safe_area(&self) -> [f32; 4] { safe_area_insets() }
    pub(crate) fn hover_none(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        { crate::platform::web::hover_none() }
        #[cfg(not(target_arch = "wasm32"))]
        { false }
    }
    pub(crate) fn touch_controls(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        { crate::platform::web::touch_controls() }
        #[cfg(not(target_arch = "wasm32"))]
        { false }
    }
    pub(crate) fn system_contrast(&self) -> Option<crate::SystemContrast> { self.system_contrast }
    pub(crate) fn system_increased_contrast(&self) -> bool { self.system_increased_contrast }
    pub(crate) fn system_reduced_motion(&self) -> bool { self.system_reduced_motion }
    pub(crate) fn pointer_locked(&self) -> bool { self.pointer_locked }
    pub(crate) fn system_languages(&self) -> &[String] {
        match &self.system_languages {
            Some(langs) => langs,
            None => crate::platform::languages::cached(),
        }
    }
    pub(crate) fn locale(&self) -> Option<&str> {
        self.system_languages().first().map(String::as_str)
    }
    pub(crate) fn scroll_bars(&self) -> crate::ScrollBars { self.system_scroll_bars.bars }
    pub(crate) fn overlay_scroll_bars(&self) -> bool { self.system_scroll_bars.is_overlay() }
    pub(crate) fn is_reduced_motion(&self) -> bool { self.reduced_motion }
    pub(crate) fn reduced_motion(&mut self, on: bool) { self.reduced_motion = on; }
    pub(crate) fn time_scale(&mut self, scale: f32) { self.time_scale = scale.clamp(0.0, 8.0); }
    pub(crate) fn zoom(&mut self, k: f32) { self.zoom = super::zoom::clamp(k); }
    pub(crate) fn caret(&self, key: &str) -> Option<[f32; 4]> {
        self.caret_read.set(true);
        let rect = caret_of(&self.caret, key);
        crate::memo::read_caret(key, rect);
        rect
    }
}

/// Passed to view functions: the environment, plus runtime settings a view may declare.
pub struct Cx<S> {
    pub(crate) core: CxCore,
    pub(super) _s: PhantomData<fn(&S)>,
}

impl<S> Cx<S> {
    /// A copy of this context for a sub-view over another state type, or a scratch run of
    /// the same view: environment, settings and the reads made so far. Pass it back with
    /// [`Self::absorb`] to keep what the sub-view did, or drop it to discard that, as a host
    /// does for a sub-view (a plugin's panel) that panicked partway.
    #[inline(always)]
    pub fn retype<T>(&self) -> Cx<T> {
        Cx { core: self.core.clone(), _s: PhantomData }
    }

    /// Takes back a [`Self::retype`]d context a nested view ran with: its overrides, and the
    /// caret and layout reads it added. The window's chrome stays the outer view's: a staged
    /// story's `caption_color` or titlebar isn't the window's.
    #[inline(always)]
    pub fn absorb<T>(&mut self, other: Cx<T>) {
        self.core.absorb(other.core);
    }

    #[inline(always)]
    pub(crate) fn set_env(&mut self, env: Env) {
        self.core.set_env(env);
    }

    /// A nested view on a stage, which no window controls cover.
    #[inline(always)]
    pub(crate) fn clear_chrome(&mut self) {
        self.core.clear_chrome();
    }

    #[inline(always)]
    pub fn env(&self) -> Env { self.core.env() }
    #[inline(always)]
    pub fn system_dark(&self) -> bool { self.core.system_dark() }
    /// Points from the window's leading edge to the trailing edge of the macOS traffic lights
    /// drawn over the content, for a top bar's leading inset: 0 when nothing is drawn there (full
    /// screen, a titled window whose content starts below its titlebar, other platforms). On
    /// macOS it drops to 0 as soon as entering full screen begins
    /// (`NSWindowWillEnterFullScreenNotification`), and stays 0 while exiting until AppKit
    /// clears full screen and reattaches the traffic lights to the window. The view re-runs when
    /// it changes. Headless runs report [`Chrome::titlebar_inset`](crate::Chrome::titlebar_inset).
    /// A `memo` closure that reads it needs it in its version.
    #[inline(always)]
    pub fn titlebar_inset(&self) -> f32 { self.core.titlebar_inset() }
    /// What the window's own controls take of a titlebar the app draws (`App::titlebar`):
    /// the macOS traffic lights at the leading edge, the Windows caption buttons at the
    /// trailing edge, and the band's height. Zero where the native frame stays. Keep the
    /// titlebar's own controls clear of both ends. Headless runs report what this platform's
    /// window would.
    #[inline(always)]
    pub fn chrome_insets(&self) -> crate::ChromeInsets { self.core.chrome_insets() }
    /// Whether the host window is currently maximized (zoomed on macOS), as confirmed by the
    /// window system. The view and any `memo` closures rebuild automatically when this changes;
    /// `Tx::toggle_maximized` never flips it optimistically before the OS confirms the change.
    /// Returns `false` in headless runs unless set with [`Runtime::set_maximized`], and for
    /// overlays, adopted macOS panels, and staged playground subviews.
    ///
    /// ```
    /// # use rune::{Cx, El, text};
    /// fn titlebar(_: &(), cx: &mut Cx<()>) -> El {
    ///     text(if cx.is_maximized() { "Restore" } else { "Maximize" })
    /// }
    /// ```
    #[inline(always)]
    pub fn is_maximized(&self) -> bool { self.core.is_maximized() }
    /// Whether the host window is currently in full screen (a native full-screen Space on
    /// macOS, borderless full screen over its display on Windows and Linux).
    ///
    /// When the value changes by platform:
    /// - **macOS**: flips at the start of the native Space transition on AppKit's
    ///   `NSWindowWillEnterFullScreenNotification` and
    ///   `NSWindowWillExitFullScreenNotification` (matching Chrome's full-screen state), and
    ///   reconciles on `NSWindowDidEnterFullScreenNotification`,
    ///   `NSWindowDidExitFullScreenNotification`, or a failed transition.
    /// - **Windows and Linux**: updates when the window system confirms the borderless
    ///   full-screen state (on create, resize, scale change, or focus).
    /// - **Web, overlays, adopted macOS panels, and staged playground subviews**: always
    ///   `false`.
    /// - **Headless runs**: `false` unless set with [`Runtime::set_full_screen`].
    ///
    /// The view and any `memo` closures rebuild automatically when this changes, including
    /// when the user enters or leaves full screen via the green button, Esc, keyboard
    /// shortcuts, the window manager, or [`Tx::set_full_screen`] / [`Tx::toggle_full_screen`].
    ///
    /// ```
    /// # use rune::{Cx, El, row, text};
    /// fn titlebar(_: &(), cx: &mut Cx<()>) -> El {
    ///     let full = cx.is_full_screen();
    ///     row()
    ///         .pl(cx.titlebar_inset())
    ///         .child(text(if full { "Exit full screen" } else { "Full screen" }))
    ///         .on_press_tx(move |_: &mut (), tx| tx.set_full_screen(!full))
    /// }
    /// ```
    #[inline(always)]
    pub fn is_full_screen(&self) -> bool { self.core.is_full_screen() }
    /// The colour of the Windows caption buttons' symbols (Electron's `titleBarOverlay`
    /// `symbolColor`), for a titlebar whose colours follow the app's theme rather than the
    /// system's light or dark setting: by default black in light mode, white in dark. Their
    /// hover and press washes are this colour at 10% and 20%; close turns red either way.
    #[inline(always)]
    pub fn caption_color(&mut self, color: Color) { self.core.caption_color(color); }
    /// The theme's text colour, as CSS `color` on the page: text, icons and text fields that
    /// aren't given a colour (`El::color`, `El::placeholder`, `El::caret`) draw in it, the
    /// placeholder at 40% and the selection at 20%, so a field left uncoloured stays readable in
    /// a light theme. Without it they draw white. Set it on every run of the view; a `memo`
    /// whose text takes it rebuilds when it changes.
    #[inline(always)]
    pub fn foreground(&mut self, color: Color) { self.core.foreground(color); }
    /// The keyboard focus ring a focusable element draws when it doesn't set its own
    /// ([`El::focus_ring`](crate::El::focus_ring)), in place of AppKit's: `width` points wide,
    /// `offset` points outside its border box, following its corner radii, e.g. `2.0, 2.5` for
    /// CSS `outline: 2px solid; outline-offset: 2.5px`. It shows only for keyboard focus, as
    /// `:focus-visible` does. Set it on every run of the view.
    #[inline(always)]
    pub fn focus_ring(&mut self, width: f32, offset: f32, color: Color) { self.core.focus_ring(width, offset, color); }
    /// How [`El::help`](crate::El::help) tooltips look, where they sit and how long they wait,
    /// in place of the system's ([`HelpLook::system`](crate::HelpLook::system)): a design
    /// system's own tooltip, as a web page draws one. Rune then draws it on macOS too, instead
    /// of AppKit's. Set it on every run of the view.
    #[inline(always)]
    pub fn help_look(&mut self, look: crate::HelpLook) { self.core.help_look(look); }
    /// Default cursor for pressable elements (those with [`El::on_press`](crate::El::on_press)
    /// or [`El::on_double_press`](crate::El::on_double_press)) that do not set their own
    /// [`El::cursor`](crate::El::cursor): e.g. `Cursor::Pointer` for a web-style pointing hand
    /// over buttons, menu items and clickable rows. Wins over a cursor inherited from an
    /// ancestor, while an element's own `.cursor(..)` still wins, [`El::disabled`](crate::El::disabled)
    /// / [`El::aria_disabled`](crate::El::aria_disabled) controls show `Cursor::Default`, and
    /// text fields and links keep their own cursors. Off by default. Set it on every run of the
    /// view.
    ///
    /// ```
    /// # use rune::{Cursor, Cx, El, Env, Runtime, col, el};
    /// fn view(_: &(), cx: &mut Cx<()>) -> El {
    ///     cx.press_cursor(Cursor::Pointer);
    ///     col().size(120.0, 40.0).child(el().size(120.0, 40.0).on_press(|_: &mut ()| {}))
    /// }
    /// # let mut rt = Runtime::new((), view, (120.0, 40.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # rt.pointer_move(20.0, 20.0);
    /// # assert_eq!(rt.cursor(), Cursor::Pointer);
    /// ```
    #[inline(always)]
    pub fn press_cursor(&mut self, cursor: crate::el::Cursor) { self.core.press_cursor(cursor); }
    /// Every control that can take focus draws its keyboard focus ring (and its focus fill) as
    /// if the keyboard had focused it, whether or not it has focus: a developer tool for checking
    /// focus styles at a glance, as forcing `:focus` and `:focus-visible` on every focusable
    /// element of a web page does (Chrome DevTools' `CSS.forcePseudoState`). Focus itself doesn't
    /// move. Set it on every run of the view while the tool is on.
    #[inline(always)]
    pub fn force_focus_rings(&mut self) { self.core.force_focus_rings(); }
    /// Keyboard focus as a web page has it, for a view that ports one: every pressable control
    /// (an `on_press` element whose role is button, link, tab, checkbox, switch, radio or
    /// combobox) is a Tab stop in document order and takes focus when clicked, as an HTML
    /// `<button>` does, unless it says [`El::focusable(false)`](crate::El::focusable) or is
    /// disabled. A modal that opens with no [`El::autofocus`](crate::El::autofocus) control and
    /// no text field focuses itself, as a web dialog does, so Tab enters it at its first control
    /// rather than landing on its close button. Set it on every run of the view.
    #[inline(always)]
    pub fn web_focus(&mut self) { self.core.web_focus(); }
    /// The height of the titlebar the app draws (`App::titlebar`), in window points, in place of
    /// `Titlebar::new`'s while the view keeps setting it: a band that scales with the app's UI
    /// zoom, as Electron's `setTitleBarOverlay({ height })` does. The Windows caption buttons
    /// take it (their width stays), macOS centres the traffic lights in it unless
    /// `Titlebar::traffic_lights` or `Chrome::traffic_lights` places them, and
    /// [`Self::chrome_insets`] reports it from the next frame on, so the buttons line up with
    /// the app's row at every zoom: for a 44 point row, `cx.titlebar_height(44.0 * zoom.max(1.0))`
    /// keeps the row 44 points tall zoomed out, as Electron apps size their `titleBarOverlay`.
    /// The height is in window points; [`Self::chrome_insets`] reports it in the view's, over
    /// [`Self::zoom`]. Over [`Self::set_titlebar`], it sets that titlebar's height.
    #[inline(always)]
    pub fn titlebar_height(&mut self, points: f32) { self.core.titlebar_height(points); }
    /// The titlebar the app draws (`App::titlebar`), in place of the app's while the view keeps
    /// setting it: its height and, on macOS, where the traffic lights sit
    /// (`Titlebar::traffic_lights`), for a switch between layouts whose titlebars differ. The
    /// window moves the traffic lights and resizes the Windows caption buttons to it, and
    /// [`Self::chrome_insets`] and [`Self::titlebar_inset`] report the new controls from the
    /// next frame on. A window without `App::titlebar` keeps its native frame.
    #[inline(always)]
    pub fn set_titlebar(&mut self, titlebar: crate::Titlebar) { self.core.set_titlebar(titlebar); }
    /// The user's accent colour (System Settings › Appearance on macOS) for this window's
    /// appearance, [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT)'s blue elsewhere and
    /// headless. The view re-runs, memos included, when the user picks another accent.
    #[inline(always)]
    pub fn accent(&self) -> Color { self.core.accent() }
    /// The keyboard focus ring colour for this window's appearance: AppKit's
    /// `keyboardFocusIndicatorColor`, the accent at 50% elsewhere. Elements with no ring of
    /// their own draw their default focus ring in it.
    #[inline(always)]
    pub fn focus_color(&self) -> Color { self.core.focus_color() }
    /// Browser safe-area insets in points, in top/right/bottom/left order. Zero on native.
    /// Apply these inside a full-canvas root so backgrounds can extend to the screen edges.
    #[inline(always)]
    pub fn safe_area(&self) -> [f32; 4] { self.core.safe_area() }
    /// The primary pointer can't hover: a browser's `(hover: none)`, a phone's or tablet's touch
    /// screen. What a mouse would reveal on hover (a code block's Copy, a row's actions) should
    /// show without it. False natively. A change re-runs the view.
    #[inline(always)]
    pub fn hover_none(&self) -> bool { self.core.hover_none() }
    /// A page's primary pointer is coarse and can't hover. Use this for a phone or tablet's
    /// controls rather than the viewport width: a narrow window can still have a mouse.
    #[inline(always)]
    pub fn touch_controls(&self) -> bool { self.core.touch_controls() }
    /// The system's contrast theme when one is on (Windows' Contrast themes, high contrast):
    /// the colours it asks every app to paint with, which a view uses in place of its own
    /// (CSS `forced-colors: active`). None otherwise. A change re-runs the view.
    #[inline(always)]
    pub fn system_contrast(&self) -> Option<crate::SystemContrast> { self.core.system_contrast() }
    /// Whether the system asks for more contrast: macOS's Increase Contrast, or a Windows contrast
    /// theme (when [`Self::system_contrast`] is also there). A view can draw its stronger edges for
    /// it while keeping its own colours. A change re-runs the view.
    #[inline(always)]
    pub fn system_increased_contrast(&self) -> bool { self.core.system_increased_contrast() }
    /// The host's OS accessibility preference, independent of this commit's override.
    #[inline(always)]
    pub fn system_reduced_motion(&self) -> bool { self.core.system_reduced_motion() }
    /// Whether an element in this window currently holds pointer lock
    /// ([`El::pointer_lock`](crate::El::pointer_lock)).
    ///
    /// Views can toggle a "Click to look around" or "Press Esc to release" hint from this;
    /// the view (and memos) rebuild whenever it changes. On web, becomes `true` once the browser
    /// grants pointer lock (`pointerlockchange`) and stays `false` if refused. Always `false` on
    /// iOS and native overlays.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, el, text};
    /// # fn view(_: &(), cx: &mut Cx<()>) -> El {
    /// #     el().key("view").size(200.0, 200.0)
    /// #         .pointer_lock(|_: &mut (), _, _| {})
    /// #         .child(text(if cx.pointer_locked() { "Locked" } else { "Click to look" }))
    /// # }
    /// let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// assert!(!rt.pointer_locked());
    /// rt.pointer_down(100.0, 100.0);
    /// rt.frame(0.0);
    /// assert!(rt.pointer_locked());
    /// ```
    #[inline(always)]
    pub fn pointer_locked(&self) -> bool { self.core.pointer_locked() }
    /// The user's preferred languages as BCP 47 tags (RFC 5646), most preferred first (for
    /// example `["ar-EG", "en-US"]`). Queries the platform lazily on first read
    /// (`GetUserPreferredUILanguages` on Windows, `NSLocale.preferredLanguages` on macOS/iOS,
    /// `navigator.languages` on web, and POSIX `LANGUAGE`/`LC_ALL`/`LC_MESSAGES`/`LANG`
    /// elsewhere), performing no work before the first frame. Tests and hosts can override
    /// the list with [`Runtime::set_languages`](crate::Runtime::set_languages).
    ///
    /// ```
    /// use std::cell::RefCell;
    /// use std::rc::Rc;
    /// use rune::{Env, Runtime, el};
    ///
    /// let seen = Rc::new(RefCell::new(Vec::new()));
    /// let mut rt = Runtime::new(
    ///     seen.clone(),
    ///     |out: &Rc<RefCell<Vec<String>>>, cx| {
    ///         *out.borrow_mut() = cx.system_languages().to_vec();
    ///         el()
    ///     },
    ///     (200.0, 100.0),
    ///     Env::default(),
    /// );
    /// rt.set_languages(["ar-EG", "en-US"]);
    /// rt.frame(0.0);
    /// assert_eq!(*seen.borrow(), vec!["ar-EG", "en-US"]);
    /// ```
    #[inline(always)]
    pub fn system_languages(&self) -> &[String] { self.core.system_languages() }
    /// The user's primary preferred language tag (`Self::system_languages().first()`), as a
    /// BCP 47 tag (RFC 5646), or `None` when none is configured. Useful for deriving layout
    /// direction or passing to [`rune::text::set_language`](crate::text::set_language).
    ///
    /// ```
    /// use std::cell::RefCell;
    /// use std::rc::Rc;
    /// use rune::{Env, Runtime, el};
    ///
    /// let seen = Rc::new(RefCell::new(None));
    /// let mut rt = Runtime::new(
    ///     seen.clone(),
    ///     |out: &Rc<RefCell<Option<String>>>, cx| {
    ///         *out.borrow_mut() = cx.locale().map(str::to_owned);
    ///         el()
    ///     },
    ///     (200.0, 100.0),
    ///     Env::default(),
    /// );
    /// rt.set_languages(["ar-EG", "en-US"]);
    /// rt.frame(0.0);
    /// assert_eq!(seen.borrow().as_deref(), Some("ar-EG"));
    /// ```
    #[inline(always)]
    pub fn locale(&self) -> Option<&str> { self.core.locale() }
    /// The user's "Show scroll bars" preference (System Settings › Appearance on macOS;
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling) elsewhere and headless).
    /// The view and its memos re-run when it changes.
    #[inline(always)]
    pub fn scroll_bars(&self) -> crate::ScrollBars { self.core.scroll_bars() }
    /// Whether scroll bars currently overlay and fade out at rest (`true` for
    /// [`ScrollBars::WhenScrolling`](crate::ScrollBars::WhenScrolling), `false` for
    /// [`ScrollBars::Always`](crate::ScrollBars::Always), and `NSScroller.preferredScrollerStyle`
    /// under [`ScrollBars::Automatic`](crate::ScrollBars::Automatic); always `true` on iOS).
    #[inline(always)]
    pub fn overlay_scroll_bars(&self) -> bool { self.core.overlay_scroll_bars() }
    /// Whether this commit reduces motion: the host preference, or the view's override from
    /// [`Self::reduced_motion`]. Pass it to builders that pick a different motion under it.
    #[inline(always)]
    pub fn is_reduced_motion(&self) -> bool { self.core.is_reduced_motion() }
    /// Override the host preference for this commit: travel jumps, opacity/colour briefly fade.
    #[inline(always)]
    pub fn reduced_motion(&mut self, on: bool) { self.core.reduced_motion(on); }
    /// Scale the window's animation clock (slow motion below 1, paused at 0). To slow or freeze
    /// only part of the window, use [`El::time_scale`](crate::El::time_scale), which multiplies
    /// with this.
    #[inline(always)]
    pub fn time_scale(&mut self, scale: f32) { self.core.time_scale(scale); }
    /// The UI zoom, as a browser's page zoom (Electron's `setZoomFactor`): `k` times the size,
    /// 1 unzoomed. The view lays out in a window `1 / k` as many points across, drawn at `k`
    /// times the window's scale, so borders, icons, text and pixel snapping land on the device
    /// grid at every zoom as Chromium's do, and motion runs in the space the layout is in. Set it
    /// on every run of the view; a run that doesn't is unzoomed. A change lands at once, in the
    /// commit that makes it. [`Self::env`]'s scale, layout reads and pointer positions are in
    /// the zoomed view's points; [`Self::chrome_insets`] and [`Self::titlebar_inset`] too.
    #[inline(always)]
    pub fn zoom(&mut self, k: f32) { self.core.zoom(k); }
    /// The focused text field's caret line in window points `[x, y, w, h]`, as last drawn (after
    /// scrolling, at the cursor inside an IME composition), when that field is keyed `key` or sits
    /// inside an element keyed `key`. A view that reads it re-runs when the caret moves, so a
    /// popup placed from it lands the frame after each edit.
    #[inline(always)]
    pub fn caret(&self, key: &str) -> Option<[f32; 4]> { self.core.caret(key) }
}

/// [`Cx::caret`] for the caret `caret` (the focused field's path and rect).
pub(crate) fn caret_of(caret: &Option<(String, [f32; 4])>, key: &str) -> Option<[f32; 4]> {
    let (path, rect) = caret.as_ref()?;
    path.split('/').any(|seg| element_path::segment_matches_key(seg, key)).then_some(*rect)
}
