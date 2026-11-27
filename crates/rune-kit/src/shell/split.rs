//! A window's split layout: a resizable, collapsible sidebar, the main pane under a toolbar,
//! and an optional inspector, with the title bar region cleared for the traffic lights.

use std::rc::Rc;
use std::time::Duration;

use rune::{Anim, Color, Curve, Cursor, Drag, Easing, El, Phase, Spring, Transition, Tx, col, el, row};

use super::pane::{PaneMotion, PaneState, Peek, SidebarPeek};
use super::toolbar::{TOOLBAR_HEIGHT, Toolbar};
use crate::{ControlSize, IconButton, IconButtonVariant, Kit};

type Apply = Box<dyn FnOnce(El) -> El>;
/// Delayed hover on an element: opens a peek on rest, or holds a peek open across its surfaces
/// (the raised pane, the raised toggle, and the margin strip beside them).
type Hover = Rc<dyn Fn(El, Duration, Duration, PeekHover) -> El>;

#[derive(Clone, Copy)]
enum PeekHover {
    Open(u8),
    Hold(u8),
}

/// The resize grip's width, straddling nothing: it sits inside the pane's edge.
const GRIP: f32 = 6.0;

struct Pane {
    state: PaneState,
    content: El,
    handlers: Vec<Apply>,
    toggle: Option<Apply>,
    clear: Option<Apply>,
    hover: Option<Hover>,
}

/// Where the sidebar's toggle button sits and how it looks:
/// `SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).in_header()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarToggle {
    size: ControlSize,
    past_lights: f32,
    edge: f32,
    gap: f32,
    in_header: bool,
    unread: bool,
    shortcut: Option<&'static str>,
    show_label: Option<&'static str>,
    hide_label: Option<&'static str>,
}

const DEFAULT_TOGGLE_SHORTCUT: &str = if cfg!(target_os = "macos") { "⇧⌘B" } else { "Ctrl+Shift+B" };
const DEFAULT_SHOW_SIDEBAR: &str = "Show sidebar";
const DEFAULT_HIDE_SIDEBAR: &str = "Hide sidebar";
const DEFAULT_FRAME_INSET: f32 = 8.0;

impl SidebarToggle {
    /// A 24 point button 8 points past the traffic lights (12 from the window's edge when there
    /// are none), 8 points before the toolbar's first item while the sidebar is closed.
    pub fn new() -> SidebarToggle {
        SidebarToggle {
            size: ControlSize::Regular,
            past_lights: 8.0,
            edge: 12.0,
            gap: 8.0,
            in_header: false,
            unread: false,
            shortcut: Some(DEFAULT_TOGGLE_SHORTCUT),
            show_label: None,
            hide_label: None,
        }
    }
    /// The button's size: 16, 20, 24, 28 or 36 points by default (or the kit's
    /// [`IconButtonMetrics`](crate::IconButtonMetrics) when themed).
    pub fn size(mut self, s: ControlSize) -> Self {
        self.size = s;
        self
    }
    /// Points between the traffic lights and the button.
    pub fn past_lights(mut self, points: f32) -> Self {
        self.past_lights = points.max(0.0);
        self
    }
    /// Points from the window's leading edge when there are no traffic lights (full screen,
    /// and off macOS).
    pub fn edge(mut self, points: f32) -> Self {
        self.edge = points.max(0.0);
        self
    }
    /// Points between the button and what follows it in the title bar while the sidebar is
    /// closed.
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = points.max(0.0);
        self
    }
    /// While the sidebar is open, the button sits at the end of the sidebar's title bar row
    /// instead of past the traffic lights, and travels between the two on the sidebar's spring.
    pub fn in_header(mut self) -> Self {
        self.in_header = true;
        self
    }
    /// Marks the button with a dot while the sidebar is closed (unread activity in it).
    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }
    /// The shortcut hint shown in the toggle button's tooltip (`"⇧⌘B"` on macOS,
    /// `"Ctrl+Shift+B"` elsewhere by default; pass `""` to omit).
    pub fn shortcut(mut self, shortcut: &'static str) -> Self {
        self.shortcut = (!shortcut.is_empty()).then_some(shortcut);
        self
    }
    /// Accessible name and tooltip text for the toggle button when the sidebar is closed
    /// (`show`) and open (`hide`), replacing `"Show sidebar"` and `"Hide sidebar"`. Empty or
    /// blank strings keep the default for that state.
    ///
    /// ```
    /// # use rune_kit::shell::SidebarToggle;
    /// # let _ =
    /// SidebarToggle::new().labels("显示侧边栏", "隐藏侧边栏")
    /// # ;
    /// ```
    pub fn labels(mut self, show: &'static str, hide: &'static str) -> Self {
        self.show_label = (!show.trim().is_empty()).then_some(show);
        self.hide_label = (!hide.trim().is_empty()).then_some(hide);
        self
    }
}

impl Default for SidebarToggle {
    fn default() -> Self {
        Self::new()
    }
}

/// The app shell: `SplitView::new(main).sidebar(..).toolbar(..).inspector(..)
/// .titlebar_inset(cx.titlebar_inset()).build(&kit)`.
///
/// The sidebar and inspector are flush panes on the window's deep background; the main pane is
/// a rounded card beside them, inset by [`Self::frame_inset`] where no pane borders it. A 48
/// point band across the top is the title bar: it drags the window, its leading end clears the
/// traffic lights (pass [`Cx::titlebar_inset`], 0 in full screen), and the sidebar's toggle
/// button sits just past them ([`SidebarToggle`]). Panes resize from their inner edge
/// (double-click resets the width, dragging below half the minimum collapses) and open and
/// close together on one spring ([`PaneMotion`]). Under Reduce Motion they snap. Widths and
/// open states live in the app's [`PaneState`]s. With [`SidebarPeek`], a closed sidebar slides
/// in over the content while the pointer rests at the window's edge.
///
/// The band's middle is 24 points down: place the traffic lights there with
/// `Chrome { traffic_lights: Some((16.0, 17.0)), .. }` (with [`Self::toolbar_in_panel`], the
/// frame inset further down).
///
/// [`Cx::titlebar_inset`]: rune::Cx::titlebar_inset
pub struct SplitView {
    main: El,
    sidebar: Option<Pane>,
    inspector: Option<Pane>,
    toolbar: Option<Toolbar>,
    inset: f32,
    show_toggle: bool,
    toggle: SidebarToggle,
    frame: Option<f32>,
    flat: bool,
    in_panel: bool,
    padding: [f32; 2],
    motion: PaneMotion,
    peek: Option<SidebarPeek>,
    reduced: bool,
}

impl SplitView {
    pub fn new(main: El) -> SplitView {
        SplitView {
            main,
            sidebar: None,
            inspector: None,
            toolbar: None,
            inset: 0.0,
            show_toggle: true,
            toggle: SidebarToggle::new(),
            frame: None,
            flat: false,
            in_panel: false,
            padding: [0.0; 2],
            motion: PaneMotion::new(),
            peek: None,
            reduced: false,
        }
    }
    /// The leading pane. `access` finds its state in the app's; the grip and the toggle button
    /// write to it. `content` is usually a built `Sidebar`.
    pub fn sidebar<S: 'static>(mut self, state: &PaneState, access: impl for<'a> Fn(&'a mut S) -> &'a mut PaneState + 'static, content: El) -> Self {
        self.sidebar = Some(pane(*state, Rc::new(access), content, 1.0, true));
        self
    }
    /// The trailing pane, resized from its leading edge.
    pub fn inspector<S: 'static>(mut self, state: &PaneState, access: impl for<'a> Fn(&'a mut S) -> &'a mut PaneState + 'static, content: El) -> Self {
        self.inspector = Some(pane(*state, Rc::new(access), content, -1.0, false));
        self
    }
    /// The band above the main pane (and the inspector). Without one the band is empty and
    /// still drags the window.
    pub fn toolbar(mut self, t: Toolbar) -> Self {
        self.toolbar = Some(t);
        self
    }
    /// Points the traffic lights take at the window's leading edge: `Cx::titlebar_inset`.
    pub fn titlebar_inset(mut self, points: f32) -> Self {
        self.inset = points.max(0.0);
        self
    }
    /// Leaves out the sidebar's toggle button (the app shows its own, or none).
    pub fn hide_sidebar_toggle(mut self) -> Self {
        self.show_toggle = false;
        self
    }
    /// Where the sidebar's toggle button sits and how it looks.
    pub fn sidebar_toggle(mut self, t: SidebarToggle) -> Self {
        self.toggle = t;
        self
    }
    /// The window background showing around the main pane where no pane borders it (default
    /// 8 points, or 0 points when [`Self::flat_main`] is enabled).
    pub fn frame_inset(mut self, points: f32) -> Self {
        self.frame = Some(points.max(0.0));
        self
    }
    /// Draws the main pane flat instead of as a rounded, bordered card: omits its corner
    /// radius and rounded clip, defaults [`Self::frame_inset`] to `0.0` unless set explicitly,
    /// and draws 1-point hairlines only on edges meeting an open pane or an outer toolbar band
    /// (`left` for an open sidebar, `right` for an open inspector, `top` when a toolbar sits
    /// above the pane). Default `false`.
    ///
    /// ```
    /// # use rune::el;
    /// # use rune_kit::{Kit, shell::{PaneState, SplitView}};
    /// # struct App { sidebar: PaneState }
    /// # let s = App { sidebar: PaneState::new(240.0) };
    /// # let (main, sidebar) = (el(), el());
    /// # let kit = Kit::neutral(true);
    /// # let _ =
    /// SplitView::new(main)
    ///     .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar)
    ///     .flat_main(true)
    ///     .build(&kit)
    /// # ;
    /// ```
    pub fn flat_main(mut self, on: bool) -> Self {
        self.flat = on;
        self
    }
    /// Puts the toolbar inside the main pane, at its top, and insets the pane from the window's
    /// top too. The strip above it drags the window, and the sidebar's title bar row lines up
    /// with the toolbar.
    pub fn toolbar_in_panel(mut self) -> Self {
        self.in_panel = true;
        self
    }
    /// Points between the sidebar's edges and its contents: `start` at the window's edge, `end`
    /// at the main pane.
    pub fn sidebar_padding(mut self, start: f32, end: f32) -> Self {
        self.padding = [start.max(0.0), end.max(0.0)];
        self
    }
    /// How the panes open and close. Both move on the spring for the sidebar's width (the
    /// inspector's without one), so they move as one.
    pub fn pane_motion(mut self, m: PaneMotion) -> Self {
        self.motion = m;
        self
    }
    /// Lets the closed sidebar peek over the content.
    pub fn sidebar_peek(mut self, p: SidebarPeek) -> Self {
        self.peek = Some(p);
        self
    }
    /// Pass [`Cx::is_reduced_motion`]: fades replace the slides of the panes' contents and of
    /// a peek, as [`PaneMotion::reduced_motion_fade`] and [`SidebarPeek::reduced_motion_fade`]
    /// set. Widths snap under Reduce Motion either way.
    ///
    /// [`Cx::is_reduced_motion`]: rune::Cx::is_reduced_motion
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced = reduced;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let t = self.toggle;
        let side = k.icon_button_metrics(t.size).size;
        let frame = self.frame.unwrap_or(if self.flat { 0.0 } else { DEFAULT_FRAME_INSET });
        let top = if self.in_panel { frame } else { 0.0 };
        let sidebar_state = self.sidebar.as_ref().map(|p| p.state);
        let sidebar_open = sidebar_state.is_some_and(|s| s.is_open());
        let inspector_state = self.inspector.as_ref().map(|p| p.state);
        let inspector_open = inspector_state.is_some_and(|s| s.is_open());
        let spring = self.motion.spring_for(k, sidebar_state.or(inspector_state).map_or(0.0, |s| s.width()));
        let lights = if self.inset > 0.0 { self.inset + t.past_lights } else { t.edge };
        let toggle = self.show_toggle && self.sidebar.is_some();
        let past_toggle = if toggle { lights + side + t.gap } else { lights };
        let peek = self.peek;
        let d = Dock { k, spring, motion: self.motion, reduced: self.reduced };

        let mut overlay = None;
        let mut zones = Vec::new();
        let mut clears = Vec::new();
        let sidebar = self.sidebar.map(|mut p| {
            let s = p.state;
            let open = s.is_open();
            let shown = peek.is_some() && s.is_peeking();
            let floating = peek.is_some() && s.peek_phase() != Peek::None;
            if let Some(c) = p.clear.take() {
                clears.push(c);
            }
            let hover = p.hover.take();
            if let Some(tg) = p.toggle.take().filter(|_| toggle) {
                let label = if open {
                    t.hide_label.unwrap_or(DEFAULT_HIDE_SIDEBAR)
                } else {
                    t.show_label.unwrap_or(DEFAULT_SHOW_SIDEBAR)
                };
                let mut button = IconButton::new(&super::SIDEBAR)
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(t.size)
                    .label(label)
                    .help(label);
                if let Some(sc) = t.shortcut {
                    button = button.shortcut(sc);
                }
                let mut button = button.build(k).expanded(open);
                if t.unread {
                    // A tween ends where it says; a spring's exit would run on unseen for a while.
                    let dot = el().key("sidebar-toggle-unread").size(7.0, 7.0).radius(3.5).fill(c.bg_accent);
                    let fade = Curve::tween(DOT_FADE, Easing::EASE_IN_OUT);
                    button = button.child(dot.absolute(side - 5.0, -2.0).opacity(if open { 0.0 } else { 1.0 }).opacity_motion(fade));
                }
                let x = if open && t.in_header { s.width() - self.padding[1] - side } else { lights };
                let mut b = tg(button).key("sidebar-toggle").absolute(x, top + (TOOLBAR_HEIGHT - side) / 2.0).motion(spring);
                if let (Some(pk), Some(h)) = (peek, &hover)
                    && !open
                {
                    b = if shown {
                        h(b, Duration::ZERO, pk.grace, PeekHover::Hold(PaneState::PEEK_TOGGLE))
                    } else {
                        h(b, pk.toggle_delay, Duration::ZERO, PeekHover::Open(PaneState::PEEK_TOGGLE))
                    };
                }
                if floating {
                    b = b.raise();
                }
                overlay = Some(b);
            }
            let mut pane_hold = None;
            if let (Some(pk), Some(h)) = (peek, &hover) {
                let instant = Transition::none();
                if shown {
                    let hold = el().key("sidebar-peek-hold").pin(Some(0.0), Some(0.0), None, Some(0.0)).w(s.width() + pk.margin);
                    zones.push(h(hold.transition(instant), Duration::ZERO, pk.grace, PeekHover::Hold(PaneState::PEEK_HOLD)));
                    let h = h.clone();
                    pane_hold = Some(Box::new(move |e: El| h(e, Duration::ZERO, pk.grace, PeekHover::Hold(PaneState::PEEK_PANE))) as Apply);
                } else if !open && !s.is_resizing() {
                    let below = pk.below.unwrap_or(top + TOOLBAR_HEIGHT);
                    let edge = el().key("sidebar-peek-edge").pin(Some(0.0), Some(below), None, Some(0.0)).w(frame + pk.edge);
                    zones.push(h(edge.transition(instant), pk.edge_delay, Duration::ZERO, PeekHover::Open(0)));
                }
            }
            // The title bar row clears the traffic lights, and the toggle wherever it floats
            // over the row.
            let clear = if toggle && (!open || !t.in_header) {
                past_toggle
            } else if self.inset > 0.0 {
                self.inset + t.past_lights
            } else {
                0.0
            };
            let mut band = row()
                .key("sidebar-titlebar")
                .w_full()
                .h(top + TOOLBAR_HEIGHT)
                .pt(top)
                .center_y()
                .window_drag()
                .child(el().key("sidebar-titlebar-clear").w((clear - self.padding[0]).max(0.0)).motion(spring).a11y_hidden());
            if toggle && t.in_header {
                band = band.child(el().key("sidebar-spacer").grow(1.0).a11y_hidden()).child(el().key("sidebar-toggle-slot").w(side).ml(8.0).a11y_hidden());
            }
            let content = Contents { pad: self.padding, slide: peek.is_some(), peek, band: Some(band), hold: pane_hold };
            dock(&d, "sidebar-pane", p, true, content)
        });
        let has_toolbar = self.toolbar.is_some();
        let toolbar = self
            .toolbar
            .unwrap_or_default()
            .padding(if sidebar_open { 12.0 } else if self.in_panel { (past_toggle - frame).max(12.0) } else { past_toggle })
            .build(k)
            .motion(spring);
        let (toolbar, above) = if self.in_panel { (Some(toolbar), None) } else { (None, Some(toolbar)) };
        let mut card = col()
            .key("main-pane")
            .role("main")
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .ml(if sidebar_open { 0.0 } else { frame })
            .mr(if inspector_open { 0.0 } else { frame })
            .mb(frame)
            .fill(c.bg_base)
            .motion(spring)
            .children(toolbar)
            .child(self.main.grow(1.0).min_h(0.0));
        if self.flat {
            let sides = [
                if !self.in_panel && has_toolbar { 1.0 } else { 0.0 },
                if inspector_open { 1.0 } else { 0.0 },
                0.0,
                if sidebar_open { 1.0 } else { 0.0 },
            ];
            if sides.iter().any(|&w| w > 0.0) {
                card = card.border_sides(sides, c.border_muted);
            }
        } else {
            card = card.radius(k.radius.lg).border(1.0, c.border_muted).clip_rounded(k.radius.lg);
        }
        let mut below = row().key("main-row").w_full().grow(1.0).min_h(0.0).motion(spring).child(card);
        if let Some(mut p) = self.inspector {
            if let Some(c) = p.clear.take() {
                clears.push(c);
            }
            let content = Contents { pad: [0.0; 2], slide: false, peek: None, band: None, hold: None };
            below = below.child(dock(&d, "inspector-pane", p, false, content));
        }
        let strip = self.in_panel.then(|| el().key("titlebar-strip").w_full().h(frame).window_drag());
        let main = col().key("main-column").grow(1.0).min_w(0.0).h_full().motion(spring).children(above).children(strip).child(below);
        let mut split = row().key("split").w_full().h_full().motion(spring);
        if let Some(s) = sidebar {
            split = split.child(s);
        }
        let mut root = el().key("shell").w_full().h_full().fill(c.bg_deep).child(split.child(main)).children(zones);
        if let Some(o) = overlay {
            root = root.child(o);
        }
        for clear in clears {
            root = clear(root);
        }
        root
    }
}

/// A full-window screen without side panes — a settings page, lock screen or onboarding view
/// in the same window as a [`SplitView`]. Shares [`SplitView`]'s title bar height, traffic-light
/// clearance, window-drag band and framed content card so switching screens matches pixel for
/// pixel: `Page::new(content).title("Settings").leading(back).trailing(done).titlebar_inset(cx.titlebar_inset()).build(&kit)`.
pub struct Page {
    main: El,
    toolbar: Toolbar,
    inset: f32,
    toggle: SidebarToggle,
    frame: Option<f32>,
    flat: bool,
    in_panel: bool,
}

impl Page {
    pub fn new(main: El) -> Page {
        Page {
            main,
            toolbar: Toolbar::new(),
            inset: 0.0,
            toggle: SidebarToggle::new(),
            frame: None,
            flat: false,
            in_panel: false,
        }
    }
    /// A centred title in the toolbar band.
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.toolbar = self.toolbar.title(t);
        self
    }
    /// The toolbar's centre slot (replaces [`Self::title`]).
    pub fn center(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.center(e);
        self
    }
    /// Adds a leading control to the toolbar (a Back button, say), placed past the traffic lights.
    pub fn leading(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.leading(e);
        self
    }
    /// Adds a trailing control to the toolbar.
    pub fn trailing(mut self, e: El) -> Self {
        self.toolbar = self.toolbar.trailing(e);
        self
    }
    /// Replaces the toolbar with a pre-built [`Toolbar`].
    pub fn toolbar(mut self, t: Toolbar) -> Self {
        self.toolbar = t;
        self
    }
    /// Points the traffic lights take at the window's leading edge: `Cx::titlebar_inset`.
    pub fn titlebar_inset(mut self, points: f32) -> Self {
        self.inset = points.max(0.0);
        self
    }
    /// Points past the traffic lights (or the window's edge in full screen) where the toolbar's
    /// leading slot starts, matching [`SplitView::sidebar_toggle`].
    pub fn past_lights(mut self, past_lights: f32, edge: f32) -> Self {
        self.toggle = self.toggle.past_lights(past_lights).edge(edge);
        self
    }
    /// The window background showing around the content pane (default 8 points, or 0 points
    /// when [`Self::flat_main`] is enabled, matching [`SplitView::frame_inset`]).
    pub fn frame_inset(mut self, points: f32) -> Self {
        self.frame = Some(points.max(0.0));
        self
    }
    /// Draws the content pane flat instead of as a rounded card, matching
    /// [`SplitView::flat_main`].
    ///
    /// ```
    /// # use rune::el;
    /// # use rune_kit::{Kit, shell::Page};
    /// # let body = el();
    /// # let kit = Kit::neutral(true);
    /// # let _ =
    /// Page::new(body).title("Settings").flat_main(true).build(&kit)
    /// # ;
    /// ```
    pub fn flat_main(mut self, on: bool) -> Self {
        self.flat = on;
        self
    }
    /// Puts the toolbar inside the framed content pane at its top, matching
    /// [`SplitView::toolbar_in_panel`].
    pub fn toolbar_in_panel(mut self) -> Self {
        self.in_panel = true;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let mut split = SplitView::new(self.main)
            .toolbar(self.toolbar)
            .titlebar_inset(self.inset)
            .sidebar_toggle(self.toggle)
            .flat_main(self.flat)
            .hide_sidebar_toggle();
        if let Some(frame) = self.frame {
            split = split.frame_inset(frame);
        }
        if self.in_panel {
            split = split.toolbar_in_panel();
        }
        split.build(k)
    }
}

type PaneAccess<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut PaneState>;

/// Handlers for a pane's grip and toggle, typed by the app's state.
fn pane<S: 'static>(state: PaneState, access: PaneAccess<S>, content: El, sign: f32, toggle: bool) -> Pane {
    let drag_access = access.clone();
    let drag: Apply = Box::new(move |e: El| {
        let (a1, a2, a3, a4) = (drag_access.clone(), drag_access.clone(), drag_access.clone(), drag_access);
        e.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            let p = a1(s);
            match d.phase {
                Phase::Down => p.press_grip(d.window_point()),
                Phase::Move => {
                    if p.drag_grip(d.window_point(), sign * d.dx) {
                        tx.instant();
                    } else {
                        tx.live_only();
                    }
                }
                Phase::Up => p.release_grip(d.cancelled()),
            }
        })
        .on_double_press(move |s: &mut S| a2(s).reset_width())
        .on_step(move |s: &mut S, steps, _| {
            let p = a3(s);
            p.set_width(p.width() + steps as f32 * GRIP_STEP);
        })
        .on_set_value(move |s: &mut S, value, _| {
            a4(s).set_width(value as f32);
        })
    });
    let toggle: Option<Apply> = toggle.then(|| {
        let toggle_access = access.clone();
        Box::new(move |e: El| e.on_press(move |s: &mut S| toggle_access(s).toggle())) as Apply
    });
    // A drag whose release is lost (the grip unmounted, a modal took the pointer) ends on blur
    // or once the pointer moves where the drag doesn't hear it. Blur also ends a peek.
    let resizing = state.is_resizing();
    let peeking = state.is_peeking();
    let clear = (resizing || peeking).then(|| {
        let clear_access = access.clone();
        Box::new(move |mut e: El| {
            let blur_access = clear_access.clone();
            e = e.on_window_focus(move |s: &mut S, focused, tx| {
                if focused || !blur_access(s).blur_clear() {
                    tx.live_only();
                }
            });
            if resizing {
                e = e.on_pointer_move_local(move |s: &mut S, p, tx| {
                    if !clear_access(s).pointer_grip([p.window_x, p.window_y]) {
                        tx.live_only();
                    }
                });
            }
            e
        }) as Apply
    });
    let hover = (sign > 0.0).then(|| {
        Rc::new(move |e: El, enter: Duration, leave: Duration, mode: PeekHover| {
            let hover_access = access.clone();
            e.hover_delays(enter, leave).on_hover_change(move |s: &mut S, inside, tx: &mut Tx| match mode {
                PeekHover::Open(zone) => {
                    if inside { hover_access(s).peek_from(zone); } else { tx.live_only(); }
                }
                PeekHover::Hold(zone) => {
                    if !hover_access(s).peek_hover(zone, inside) { tx.live_only(); }
                }
            })
        }) as Hover
    });
    Pane { state, content, handlers: vec![drag], toggle, clear, hover }
}

const DOT_FADE: f32 = 0.15;
/// Points one increment from assistive technology widens a pane by.
const GRIP_STEP: f32 = 8.0;

/// What the panes share.
struct Dock<'a> {
    k: &'a Kit,
    spring: Spring,
    motion: PaneMotion,
    reduced: bool,
}

/// How one pane holds its contents.
struct Contents {
    pad: [f32; 2],
    slide: bool,
    peek: Option<SidebarPeek>,
    band: Option<El>,
    hold: Option<Apply>,
}

/// The pane's slot: its width springs between 0 and the pane's width while the content keeps
/// its full width, clipped, pinned to the leading edge (or the trailing one, sliding). The
/// content fades as the motion says. A peeking sidebar's content slides out past the slot's
/// edge, unclipped and raised over the main pane, on a floating surface.
fn dock(d: &Dock, key: &str, p: Pane, leading: bool, content: Contents) -> El {
    let (k, spring) = (d.k, d.spring);
    let s = p.state;
    let open = s.is_open();
    let w = s.width();
    let phase = if content.peek.is_some() { s.peek_phase() } else { Peek::None };
    let floating = phase != Peek::None;
    let fade_peek = |pk: &SidebarPeek| -> Anim {
        let sec = pk.reduced_fade_for(pk.spring.unwrap_or(k.motion.control));
        if d.reduced && sec > 0.0 { Curve::tween(sec, Easing::EASE_IN_OUT).into() } else { Anim::INSTANT }
    };
    let (opacity, fade) = match (open, phase, &content.peek) {
        (false, Peek::Shown, Some(pk)) => (1.0, fade_peek(pk)),
        // It slides out past the edge; under Reduce Motion it fades where it is.
        (false, Peek::Left, Some(pk)) => (if d.reduced { 0.0 } else { 1.0 }, fade_peek(pk)),
        (true, ..) => (1.0, d.motion.fade(spring, true, d.reduced)),
        (false, ..) => (0.0, d.motion.fade(spring, false, d.reduced)),
    };
    let mut body = col()
        .key("pane-content")
        .w(w)
        .h_full()
        .pl(content.pad[0])
        .pr(content.pad[1])
        .opacity(opacity)
        .opacity_motion(fade);
    body = if content.slide {
        body.pin(None, Some(0.0), Some(0.0), Some(0.0)).motion(spring)
    } else {
        body.pin(Some(0.0), Some(0.0), None, Some(0.0))
    };
    if let Some(pk) = content.peek {
        let peek_spring: Anim = pk.spring.unwrap_or(k.motion.control).into();
        // Peeking, the contents sit at the window's edge: a whole width past the closed slot.
        let shown = !open && phase == Peek::Shown;
        let (x, slide) = match phase {
            Peek::Shown | Peek::Left => (if shown { w } else { 0.0 }, peek_spring),
            // Docking, the slot's edge carries the contents as far as the offset lets go of
            // them, on the same spring, so they hold still.
            _ => (0.0, spring.into()),
        };
        let surface_motion: Anim = if d.reduced {
            fade_peek(&pk)
        } else if phase == Peek::Docked {
            spring.into()
        } else {
            peek_spring
        };
        let surface = el()
            .key("pane-surface")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(k.c.bg_deep)
            .opacity(if shown { 1.0 } else { 0.0 })
            .opacity_motion(surface_motion)
            // Its size only changes while it's hidden (a resize of the open pane).
            .motion(Spring::NONE);
        body = body.offset(x, 0.0).offset_motion(slide).child(k.elevate(surface, &k.elevation.floating));
    }
    if floating {
        body = body.raise();
    }
    if let Some(hold) = content.hold {
        body = hold(body);
    }
    let body = body
        .children(content.band)
        .child(col().key("pane-body").w_full().grow(1.0).min_h(0.0).child(p.content.w_full().h_full()));
    let mut slot = el().key(key).role("region").label(if leading { "Sidebar" } else { "Inspector" })
        .w(s.shown_width()).h_full().motion(spring).child(body);
    if !floating {
        slot = slot.clip_soft(0.0);
    }
    if !open && !floating && !s.is_resizing() {
        slot = slot.a11y_hidden();
    }
    if open {
        let instant = Transition::none();
        if leading {
            slot = slot
                .child(el().key("pane-open-main").size(0.0, 0.0).a11y_hidden().transition(instant).focus_return("main-column"))
                .child(el().key("pane-open").size(0.0, 0.0).a11y_hidden().transition(instant).focus_return("sidebar-toggle"));
        } else {
            slot = slot
                .child(el().key("pane-open").size(0.0, 0.0).a11y_hidden().transition(instant).focus_return("sidebar-toggle"))
                .child(el().key("pane-open-main").size(0.0, 0.0).a11y_hidden().transition(instant).focus_return("main-pane"));
        }
    }
    if open || s.is_resizing() {
        let mut grip = el()
            .key(if leading { "sidebar-grip" } else { "inspector-grip" })
            .role("separator")
            .label(if leading { "Resize sidebar" } else { "Resize inspector" })
            // Its value is the pane's width; a reader's increment and decrement step it.
            .range_value(f64::from(s.width()), f64::from(s.min_width()), f64::from(s.max_width()))
            .range_step(f64::from(GRIP_STEP))
            .w(GRIP)
            .cursor(Cursor::ResizeColumn)
            .fill(Color::TRANSPARENT)
            .opacity(0.02)
            .transition(Transition::none());
        grip = if leading { grip.pin(None, Some(0.0), Some(0.0), Some(0.0)) } else { grip.pin(Some(0.0), Some(0.0), None, Some(0.0)) };
        if floating {
            grip = grip.raise();
        }
        for h in p.handlers {
            grip = h(grip);
        }
        // Not a Tab stop: AppKit's dividers aren't either.
        slot = slot.child(grip.focusable(false));
    }
    slot
}
