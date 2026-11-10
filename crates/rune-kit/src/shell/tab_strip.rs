//! A strip of tabs across a titlebar or down a sidebar: keyed tabs that open and close on
//! springs, shrink then scroll when they overflow, reorder by drag, tear out, and close with a
//! middle click.

use std::borrow::Cow;
use std::rc::Rc;

use rune::{Color, El, Icon, ScrollFade, ScrollSpec, Spring, Tx, col, el, row};

use crate::{Kit, Spinner};

mod auto_scroll;
mod drag;
mod tab;
#[cfg(test)]
mod tests;

pub use drag::{TabDragMode, TabDragPhase, TabDragState, TearOutSpec};

/// Which way a [`TabStrip`] runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabOrientation {
    /// Across a titlebar, browser style.
    #[default]
    Horizontal,
    /// Down a sidebar, one tab per row.
    Vertical,
}

/// How much a horizontal tab shows, from its width ([`TabOverflow::density_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabDensity {
    /// Over 64 points: avatar, title fading at its end, and the status or close slot.
    Full,
    /// 44 to 64 points: the title hides while the tab is active, hovered or shows a status, so
    /// the slot never covers the avatar.
    Compact,
    /// 44 points or less: the avatar alone, centred, with a status dot on its corner; the close
    /// button centres over the tab under the pointer, and a busy tab spins in the avatar's place.
    IconOnly,
}

/// What a strip does when its tabs don't fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabOverflow {
    /// Horizontal tabs share the strip equally, from `max_width` down to `min_width`, then the
    /// strip scrolls sideways with a `fade_width` linear fade at each edge while anything is
    /// past it (Desktop's `titlebar-tabs-fade-*`). Pinned tabs keep `pinned_width`.
    ShrinkThenScroll { min_width: f32, max_width: f32, pinned_width: f32, fade_width: f32 },
    /// Full-width rows that scroll, the top and bottom feathering by as much as is left to scroll
    /// past them, up to `fade_width` (Desktop's `--vertical-tabs-fade-*`).
    Scroll { fade_width: f32 },
}

impl TabOverflow {
    /// OpenCode's titlebar tabs: 28 to 224 points, pinned 36, 24 point fades.
    pub const HORIZONTAL_DEFAULT: Self = Self::ShrinkThenScroll { min_width: 28.0, max_width: 224.0, pinned_width: 36.0, fade_width: 24.0 };
    /// OpenCode's side tabs: 16 point fades.
    pub const VERTICAL_DEFAULT: Self = Self::Scroll { fade_width: 16.0 };

    pub fn density_for(width: f32) -> TabDensity {
        if width <= 44.0 {
            TabDensity::IconOnly
        } else if width <= 64.0 {
            TabDensity::Compact
        } else {
            TabDensity::Full
        }
    }

    /// The width of each unpinned tab when `unpinned` of them and `pinned` pinned ones share
    /// `available` points with `gap` between each: equal shares between the policy's bounds.
    /// Past the minimum the tabs overflow and the strip scrolls. `max_width` for `Scroll`.
    pub fn tab_width(&self, available: f32, unpinned: usize, pinned: usize, gap: f32) -> f32 {
        let Self::ShrinkThenScroll { min_width, max_width, pinned_width, .. } = *self else { return f32::INFINITY };
        if unpinned == 0 {
            return max_width;
        }
        let gaps = (unpinned + pinned).saturating_sub(1) as f32 * gap;
        ((available - pinned as f32 * pinned_width - gaps) / unpinned as f32).clamp(min_width, max_width)
    }

    fn fade(&self) -> f32 {
        match *self {
            Self::ShrinkThenScroll { fade_width, .. } | Self::Scroll { fade_width } => fade_width,
        }
    }
}

/// A tab's status mark, in the slot its close button takes under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabItemStatus {
    /// Working: the busy indicator ([`TabStrip::busy_indicator`]), shown on the active tab too.
    Busy,
    /// Waiting on the user: a warning dot.
    Attention,
    /// Something new: an accent dot.
    Unread,
    /// Something new that failed: a danger dot.
    Failed,
}

/// A drop that moved tab `id` from index `from` to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabReorder {
    pub id: String,
    pub from: usize,
    pub to: usize,
}

/// A tab dragged off the strip and released: the app opens it in a window of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct TabTearOut {
    pub id: String,
    pub index: usize,
    /// Pointer travel since the press, in points.
    pub offset: [f32; 2],
    /// Where it was released, in window points.
    pub window_point: Option<[f32; 2]>,
    /// The same point on the desktop in physical pixels (`Drag::screen_point`), for placing the
    /// new window under the pointer. `None` headless.
    pub screen_point: Option<(i32, i32)>,
}

/// The pointer moving a torn-out tab, or bringing it back to the strip (`torn` false), while
/// the button is still down: the app can open the tab's window now and move it with the
/// pointer, so the drag carries on in the new window ([`TabDragState::hand_off`]).
#[derive(Clone, Debug, PartialEq)]
pub struct TabTearMove {
    pub id: String,
    pub index: usize,
    /// Off the strip; false once the tab came back within the reattach distance.
    pub torn: bool,
    /// Where the press landed in the tab, in points from its top-left: where the pointer sits
    /// in the tab the new window shows.
    pub grab: [f32; 2],
    /// The pointer in window points.
    pub window_point: [f32; 2],
    /// The pointer on the desktop in physical pixels. `None` headless.
    pub screen_point: Option<(i32, i32)>,
}

type Each<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;
type Hover<S> = Rc<dyn Fn(&mut S, &str, bool, &mut Tx)>;
type Commit<S, T> = Rc<dyn Fn(&mut S, T, &mut Tx)>;
type Access<S> = fn(&mut S) -> &mut TabDragState;
type Wrap<'a> = Box<dyn FnOnce(El, TabDensity) -> El + 'a>;

/// One tab: `TabItem::new(id, title).avatar(..).status(..)`.
pub struct TabItem<'a, S: 'static> {
    id: Cow<'a, str>,
    title: Cow<'a, str>,
    subtitle: Option<Cow<'a, str>>,
    avatar: Option<El>,
    icon: Option<&'static Icon>,
    status: Option<TabItemStatus>,
    pinned: bool,
    closable: bool,
    marked: bool,
    pulse_seq: u32,
    editor: Option<El>,
    wrap: Option<Wrap<'a>>,
    _s: std::marker::PhantomData<fn(&mut S)>,
}

impl<'a, S: 'static> TabItem<'a, S> {
    pub fn new(id: impl Into<Cow<'a, str>>, title: impl Into<Cow<'a, str>>) -> Self {
        TabItem {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            avatar: None,
            icon: None,
            status: None,
            pinned: false,
            closable: true,
            marked: false,
            pulse_seq: 0,
            editor: None,
            wrap: None,
            _s: std::marker::PhantomData,
        }
    }
    /// A second, muted line under the title in a vertical strip (a project's name); the row
    /// grows to 45 points.
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'a, str>>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
    /// A 16 point leading element: a project's avatar.
    pub fn avatar(mut self, avatar: El) -> Self {
        self.avatar = Some(avatar);
        self
    }
    /// A 16 point leading icon in place of an avatar (a draft's pencil), muted until active.
    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    /// Its status mark. The strip shows what it's given; OpenCode leaves the dots off the
    /// active tab and keeps the busy ring.
    pub fn status(mut self, status: Option<TabItemStatus>) -> Self {
        self.status = status;
        self
    }
    /// Pinned tabs come first in the strip, keep [`TabOverflow`]'s `pinned_width` as icons,
    /// have no close button and reorder only among themselves.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }
    /// Whether the close button and a middle click close it (default true).
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }
    /// Highlights the tab while its context menu is open.
    pub fn marked(mut self, marked: bool) -> Self {
        self.marked = marked;
        self
    }
    /// Raise it to ring a background tab once (900 ms), as a prompt arriving does.
    pub fn pulse(mut self, seq: u32) -> Self {
        self.pulse_seq = seq;
        self
    }
    /// A field in place of the title while renaming in place. The tab doesn't drag meanwhile.
    pub fn editor(mut self, field: El) -> Self {
        self.editor = Some(field);
        self
    }
    /// Wraps the built tab: a context menu handler, a hover card's anchor.
    pub fn decorate(mut self, f: impl FnOnce(El, TabDensity) -> El + 'a) -> Self {
        self.wrap = Some(Box::new(f));
        self
    }
}

#[derive(Default)]
struct Handlers<S> {
    select: Option<Each<S>>,
    close: Option<Each<S>>,
    double: Option<Each<S>>,
    hover: Option<Hover<S>>,
    reorder: Option<Commit<S, TabReorder>>,
    tear_out: Option<Commit<S, TabTearOut>>,
    tear_move: Option<Commit<S, TabTearMove>>,
}

/// What every tab of one strip shares while it builds.
struct Strip<S> {
    handlers: Handlers<S>,
    drag: TabDragState,
    access: Option<Access<S>>,
    base: Color,
    separator: Color,
    gap: f32,
    row_height: f32,
    two_line_height: f32,
    link_inset: f32,
    count: usize,
    zoom: f32,
    spring: Spring,
    select_on_drag_start: bool,
    /// A pointer close locks the remaining tabs' width (horizontal strips).
    horizontal_lock: bool,
    matched: Option<String>,
    close_icon: Option<&'static Icon>,
    /// `TabStrip::close_tab_stop`.
    close_tab_stop: bool,
    busy: fn(&Kit) -> El,
    scroll_key: String,
    list_key: String,
    /// A torn-out tab floats in the caller's layer ([`TabStrip::build_layered`]).
    layered: bool,
    /// Where the scroll viewport is, for scrolling it while a tab is dragged near an end.
    auto: Rc<auto_scroll::View>,
}

/// OpenCode's activity ring: the kit's spinner at 14 points in the muted icon colour, the
/// base one in dark mode, where the dim cells vanish.
fn busy_ring(k: &Kit) -> El {
    Spinner::new().size(14.0).color(if k.dark { k.c.icon_base } else { k.c.icon_muted }).build(k)
}

/// A keyed strip of tabs: `TabStrip::new("tabs", TabOrientation::Horizontal).tabs(..)
/// .active(..).drag_state(&s.drag, |s| &mut s.drag).on_select(..).build(&kit)`.
///
/// Horizontal tabs are 28 points tall with 6 between them, share the strip from 224 points
/// down to 28 and then scroll; a 1.5 point separator stands between neighbours that aren't
/// active or hovered. In a `.window_drag()` titlebar the gaps between tabs, the bands above and
/// below them and the space after the trailing action move the window, as only the tabs and
/// buttons take presses. Vertical tabs fill the row with 3 points between them, each reaching
/// across half the gap so no press misses.
///
/// A press selects a horizontal tab at once; a vertical one selects on release, so dragging it
/// leaves the selection where it is ([`Self::select_on_drag_start`]). Dragging 4 points along
/// the strip reorders, Escape puts the tab back, and with [`Self::tear_out`] dragging it off
/// the strip tears it out. A middle click closes a tab.
pub struct TabStrip<'a, S: 'static> {
    key: String,
    orientation: TabOrientation,
    items: Vec<TabItem<'a, S>>,
    active: Option<&'a str>,
    viewport: Option<f32>,
    zoom: f32,
    overflow: Option<TabOverflow>,
    gap: Option<f32>,
    row_height: f32,
    select_on_drag_start: Option<bool>,
    tear: Option<TearOutSpec>,
    matched: Option<&'a str>,
    spring: Spring,
    trailing: Option<El>,
    trailing_width: f32,
    drag: Option<(TabDragState, Access<S>)>,
    fill: Option<Color>,
    close_icon: Option<&'static Icon>,
    close_tab_stop: bool,
    busy: fn(&Kit) -> El,
    label: String,
    handlers: Handlers<S>,
}

impl<'a, S: 'static> TabStrip<'a, S> {
    pub fn new(key: impl Into<String>, orientation: TabOrientation) -> Self {
        TabStrip {
            key: key.into(),
            orientation,
            items: Vec::new(),
            active: None,
            viewport: None,
            zoom: 1.0,
            overflow: None,
            gap: None,
            row_height: 28.0,
            select_on_drag_start: None,
            tear: None,
            matched: None,
            spring: rune_motion::motion::SNAP,
            trailing: None,
            trailing_width: 28.0,
            drag: None,
            fill: None,
            close_icon: None,
            close_tab_stop: false,
            busy: busy_ring,
            label: "Tabs".into(),
            handlers: Handlers { select: None, close: None, double: None, hover: None, reorder: None, tear_out: None, tear_move: None },
        }
    }
    pub fn tabs(mut self, tabs: impl IntoIterator<Item = TabItem<'a, S>>) -> Self {
        self.items.extend(tabs);
        self
    }
    /// The selected tab, by id.
    pub fn active(mut self, active_id: Option<&'a str>) -> Self {
        self.active = active_id;
        self
    }
    /// The points a horizontal strip has, trailing action included: `cx.layout_size(key)`'s
    /// width (the strip's root grows into the space it's given). It sets the tabs' shared
    /// width and density; without it they take their widest.
    pub fn viewport_width(mut self, width: Option<f32>) -> Self {
        self.viewport = width;
        self
    }
    /// The UI zoom the strip draws at (default 1), so drags convert from window points.
    pub fn zoom(mut self, zoom: f32) -> Self {
        self.zoom = if zoom > 0.0 { zoom } else { 1.0 };
        self
    }
    /// The overflow policy (default [`TabOverflow::HORIZONTAL_DEFAULT`] or
    /// [`TabOverflow::VERTICAL_DEFAULT`]).
    pub fn overflow(mut self, policy: TabOverflow) -> Self {
        self.overflow = Some(policy);
        self
    }
    /// Points between tabs (default 6 horizontal, 3 vertical).
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = Some(points.max(0.0));
        self
    }
    /// A tab's height (default 28).
    pub fn row_height(mut self, points: f32) -> Self {
        self.row_height = points.max(1.0);
        self
    }
    /// Whether a press selects the tab at once, so a drag selects it too (default true
    /// horizontal), or the release does and a drag leaves the selection (default false vertical).
    pub fn select_on_drag_start(mut self, on: bool) -> Self {
        self.select_on_drag_start = Some(on);
        self
    }
    /// Lets a drag across the strip tear a tab out ([`Self::on_tear_out`]). Off by default, and
    /// never for the only tab unless [`TearOutSpec::lone`].
    pub fn tear_out(mut self, spec: Option<TearOutSpec>) -> Self {
        self.tear = spec;
        self
    }
    /// Gives each tab `El::matched("{prefix}-{id}")`, so a tab flies between strips when the
    /// layout swaps one for another.
    pub fn matched_prefix(mut self, prefix: Option<&'a str>) -> Self {
        self.matched = prefix;
        self
    }
    /// The spring tabs step aside on during a drag (default `SNAP`, 0.25 s). Drops, tabs opening and
    /// closing, and widths changing reflow on `REFLOW`.
    pub fn reorder_spring(mut self, spring: Spring) -> Self {
        self.spring = spring;
        self
    }
    /// An element right after the last horizontal tab (the new-tab button), outside the
    /// scrolling, with the space past it left bare.
    pub fn trailing_action(mut self, action: El) -> Self {
        self.trailing = Some(action);
        self
    }
    /// The trailing action's width, which the tabs leave it (default 28, a large icon button).
    pub fn trailing_width(mut self, points: f32) -> Self {
        self.trailing_width = points.max(0.0);
        self
    }
    /// The strip's gestures, kept in the app's state; `access` finds them there. Without it the
    /// tabs select and close but don't drag, and closing doesn't lock their width.
    pub fn drag_state(mut self, state: &TabDragState, access: fn(&mut S) -> &mut TabDragState) -> Self {
        self.drag = Some((state.clone(), access));
        self
    }
    /// The colour under the strip (default `bg_deep`), which a dragged tab and the close slot
    /// paint opaque.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }
    /// The close button's glyph (default the kit's close glyph).
    pub fn close_icon(mut self, icon: &'static Icon) -> Self {
        self.close_icon = Some(icon);
        self
    }
    /// Each tab's close button is a Tab stop after its tab with the kit's focus ring, as a web
    /// page's `<button>` is. Off, it's pointer only: the strip's arrows move between tabs.
    pub fn close_tab_stop(mut self, on: bool) -> Self {
        self.close_tab_stop = on;
        self
    }
    /// What a busy tab shows (default the kit's spinner at 14 points).
    pub fn busy_indicator(mut self, f: fn(&Kit) -> El) -> Self {
        self.busy = f;
        self
    }
    /// The tab list's accessible name (default "Tabs").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.select = Some(Rc::new(f));
        self
    }
    /// The close button, a middle click.
    pub fn on_close(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.close = Some(Rc::new(f));
        self
    }
    pub fn on_double_click(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.double = Some(Rc::new(f));
        self
    }
    /// The pointer entering (`true`) and leaving a tab.
    pub fn on_hover(mut self, f: impl Fn(&mut S, &str, bool, &mut Tx) + 'static) -> Self {
        self.handlers.hover = Some(Rc::new(f));
        self
    }
    pub fn on_reorder(mut self, f: impl Fn(&mut S, TabReorder, &mut Tx) + 'static) -> Self {
        self.handlers.reorder = Some(Rc::new(f));
        self
    }
    pub fn on_tear_out(mut self, f: impl Fn(&mut S, TabTearOut, &mut Tx) + 'static) -> Self {
        self.handlers.tear_out = Some(Rc::new(f));
        self
    }
    /// Each move of a torn-out tab, and its return to the strip, while the button is down.
    pub fn on_tear_move(mut self, f: impl Fn(&mut S, TabTearMove, &mut Tx) + 'static) -> Self {
        self.handlers.tear_move = Some(Rc::new(f));
        self
    }

    /// The strip. A torn-out tab stays inside the strip's clip; [`Self::build_layered`] lifts it out.
    pub fn build(self, k: &Kit) -> El {
        self.assemble(k, None)
    }

    /// The strip, with a torn-out tab floating over the window in `layer`, which the view mounts
    /// at its root with [`crate::root`] as it does popovers and menus.
    pub fn build_layered(self, k: &Kit, layer: &mut Vec<El>) -> El {
        self.assemble(k, Some(layer))
    }

    fn assemble(self, k: &Kit, mut layer: Option<&mut Vec<El>>) -> El {
        let vertical = self.orientation == TabOrientation::Vertical;
        let overflow = self.overflow.unwrap_or(if vertical { TabOverflow::VERTICAL_DEFAULT } else { TabOverflow::HORIZONTAL_DEFAULT });
        let gap = self.gap.unwrap_or(if vertical { 3.0 } else { 6.0 });
        let count = self.items.len();
        let pinned = self.items.iter().filter(|t| t.pinned).count();
        let mut drag = self.drag.as_ref().map(|d| d.0.clone()).unwrap_or_default();
        let access = self.drag.as_ref().map(|d| d.1);
        let trailing = if self.trailing.is_some() { self.trailing_width + gap } else { 0.0 };
        // A tab coming in from another strip takes its share already, so it lands in its gap as is.
        let incoming = usize::from(drag.incoming().is_some());
        let (tab_w, pinned_w) = match overflow {
            TabOverflow::ShrinkThenScroll { max_width, pinned_width, min_width, .. } => {
                let w = self.viewport.map_or(max_width, |v| overflow.tab_width(v - trailing, count - pinned + incoming, pinned, gap));
                (drag.width_lock.map_or(w, |l| l.max(min_width)), pinned_width)
            }
            TabOverflow::Scroll { .. } => (f32::INFINITY, f32::INFINITY),
        };
        // Each tab's length along the strip with the gap after it: its width across, its row down.
        let two_line_height = 45.0;
        let pitch = |pin: bool, two_line: bool| match (vertical, pin) {
            (false, true) => pinned_w + gap,
            (false, false) => tab_w + gap,
            (true, _) => (if two_line { two_line_height } else { self.row_height }) + gap,
        };
        let pitches: Vec<f32> = self.items.iter().map(|t| pitch(t.pinned, t.subtitle.is_some())).collect();
        let auto = auto_scroll::view(&format!("{}-scroll", self.key));
        if let Some(point) = drag.incoming() {
            let slot = landing_slot(&pitches, pinned, point, &auto, vertical, self.zoom);
            drag.open_gap(slot, pitch(false, false));
        }
        let st = Strip {
            handlers: self.handlers,
            access,
            base: self.fill.unwrap_or(k.c.bg_deep),
            separator: if k.dark { k.c.layer_02 } else { k.c.layer_03 },
            gap,
            row_height: self.row_height,
            two_line_height,
            link_inset: 8.0,
            count,
            zoom: self.zoom,
            spring: self.spring,
            select_on_drag_start: self.select_on_drag_start.unwrap_or(!vertical),
            horizontal_lock: !vertical,
            matched: self.matched.map(str::to_owned),
            close_icon: self.close_icon,
            close_tab_stop: self.close_tab_stop,
            busy: self.busy,
            auto,
            scroll_key: format!("{}-scroll", self.key),
            list_key: format!("{}-list", self.key),
            layered: layer.is_some(),
            drag,
        };
        let active = self.active;
        let hovered = st.drag.hovered().map(str::to_owned);
        let ids: Vec<String> = self.items.iter().map(|t| t.id.clone().into_owned()).collect();
        let quiet = |i: usize| active == Some(ids[i].as_str()) || hovered.as_deref() == Some(ids[i].as_str());
        let tear = self.tear.filter(|t| count > 1 || t.lone);
        let mut tabs: Vec<El> = self
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let pin = item.pinned;
                let width = (!vertical).then_some(if pin { pinned_w } else { tab_w });
                let (lo, hi) = if pin { (0, pinned) } else { (pinned, count) };
                let at = tab::Place {
                    index: i,
                    lane: drag::Lane { index: i, lo, hi, pitch: pitches[i], pinned: pin, tear },
                    width,
                    density: if pin { TabDensity::IconOnly } else { width.map_or(TabDensity::Full, TabOverflow::density_for) },
                    active: active == Some(ids[i].as_str()),
                    separator: !vertical && i > 0 && !quiet(i) && !quiet(i - 1),
                };
                let (tab, ghost) = tab::build(item, at, &st, k);
                if let (Some(layer), Some(ghost)) = (layer.as_deref_mut(), ghost) {
                    layer.push(ghost);
                }
                tab
            })
            .collect();
        // The incoming tab's place: an empty slot its own length, which the tabs after it make
        // room for on the reorder spring and the tab takes when it's let go.
        if let Some((slot, pitch)) = st.drag.gap() {
            let place = el().key(format!("{}-incoming", st.list_key)).a11y_hidden();
            let place = if vertical { place.w_full().h(pitch) } else { place.w(pitch - gap).h(st.row_height) };
            tabs.insert(slot.min(tabs.len()), place);
        }
        let fade = overflow.fade();
        let mut root = if vertical {
            let list = col().key(st.list_key.clone()).role("tablist").label(self.label).focus_group().w_full().pb(fade).children(tabs);
            let scroll = col()
                .key(st.scroll_key.clone())
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .scroll_fade(ScrollFade::feather(fade))
                .scroll_margin(fade)
                .child(list);
            let scroll = auto_scroll::watch(scroll, &st.auto);
            col().key(self.key).w_full().grow(1.0).min_h(0.0).child(scroll).children(self.trailing)
        } else {
            let list = row().key(st.list_key.clone()).role("tablist").label(self.label).focus_group().gap(gap).center_y().children(tabs);
            let scroll = row().key(st.scroll_key.clone()).h(st.row_height).shrink().min_w(0.0).scroll_x().scroll_fade(ScrollFade::strength(fade).linear().full_at(0.1)).child(list);
            let scroll = auto_scroll::watch(scroll, &st.auto);
            // It grows into the space it's given, so its width is what the tabs may share.
            row().key(self.key).grow(1.0).shrink().min_w(0.0).gap(gap).center_y().child(scroll).children(self.trailing)
        };
        if let Some(access) = access {
            root = root.on_hover_change(move |s: &mut S, inside, _: &mut Tx| {
                if !inside {
                    access(s).leave();
                }
            });
        }
        root
    }
}

/// The slot a tab dragged in from another strip lands in at `point` (window points): after each
/// unpinned tab whose middle the point has passed, measured with each tab's own length (`pitches`)
/// from where the strip's viewport drew and how far it has scrolled.
fn landing_slot(pitches: &[f32], pinned: usize, point: [f32; 2], auto: &auto_scroll::View, vertical: bool, zoom: f32) -> usize {
    let a = usize::from(vertical);
    let along = (point[a] - auto_scroll::rect(auto)[a]) / zoom + auto_scroll::offset(auto, vertical);
    let mut start = 0.0;
    for (i, pitch) in pitches.iter().enumerate() {
        if i >= pinned && along < start + pitch / 2.0 {
            return i;
        }
        start += pitch;
    }
    pitches.len()
}
