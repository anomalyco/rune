//! Non-generic layout, virtualization band and assembly for [`TabStrip`](super::TabStrip).

use std::borrow::Cow;
use std::rc::Rc;

use rune::{Color, El, Icon, ScrollFade, ScrollSpec, Spring, col, el, row};

use super::drag::{Lane, TabDragState, TearOutSpec};
use super::{BAND_PAST, TabDensity, TabItemStatus, TabOrientation, TabOverflow, TabSlot, TabWords, auto_scroll, tab};
use crate::{IconButton, Kit, Spinner};

pub(super) type Wrap<'a> = Box<dyn FnOnce(El, TabDensity) -> El + 'a>;
pub(super) type Build<'a> = Box<dyn FnMut(usize) -> TabItemCore<'a> + 'a>;

pub(super) struct TabBindCtx<'a> {
    pub id: &'a str,
    pub lane: Lane,
    pub vertical: bool,
    pub lock: f32,
    pub closable: bool,
    pub select_early: bool,
    pub active: bool,
    pub scroll_key: &'a str,
    pub auto: &'a Rc<auto_scroll::View>,
}

pub(super) trait TabBind {
    fn has_close(&self) -> bool;
    fn has_access(&self) -> bool;
    fn bind_root_hover(&self, root: El) -> El;
    fn bind_torn_anchor(&self, place: El) -> El;
    fn bind_close_button(&self, button: IconButton, id: String, lock: f32) -> IconButton;
    fn bind_tab(&self, slot: El, ctx: TabBindCtx<'_>) -> El;
}

pub(super) struct TabItemCore<'a> {
    pub id: Cow<'a, str>,
    pub title: Cow<'a, str>,
    pub subtitle: Option<Cow<'a, str>>,
    pub avatar: Option<El>,
    pub icon: Option<&'static Icon>,
    pub status: Option<TabItemStatus>,
    pub pinned: bool,
    pub closable: bool,
    pub marked: bool,
    pub pulse_seq: u32,
    pub editor: Option<El>,
    pub wrap: Option<Wrap<'a>>,
}

impl<'a> TabItemCore<'a> {
    pub(super) fn new(id: Cow<'a, str>, title: Cow<'a, str>) -> Self {
        TabItemCore {
            id,
            title,
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
        }
    }
}

/// What every tab of one strip shares while it builds.
pub(super) struct Strip {
    pub drag: TabDragState,
    pub base: Color,
    pub separator: Color,
    pub gap: f32,
    pub row_height: f32,
    pub two_line_height: f32,
    pub link_inset: f32,
    pub count: usize,
    pub spring: Spring,
    pub select_on_drag_start: bool,
    /// A pointer close locks the remaining tabs' width (horizontal strips).
    pub horizontal_lock: bool,
    pub matched: Option<String>,
    pub close_icon: Option<&'static Icon>,
    /// `TabStrip::close_tab_stop`.
    pub close_tab_stop: bool,
    /// `TabStrip::hover_none`.
    pub hover_none: bool,
    pub busy: fn(&Kit) -> El,
    pub words: TabWords,
    pub scroll_key: String,
    pub list_key: String,
    /// A torn-out tab floats in the caller's layer ([`TabStrip::build_layered`](super::TabStrip::build_layered)).
    pub layered: bool,
    /// Where the scroll viewport is, for scrolling it while a tab is dragged near an end.
    pub auto: Rc<auto_scroll::View>,
}

/// OpenCode's activity ring: the kit's spinner at 14 points in the muted icon colour, the
/// base one in dark mode, where the dim cells vanish.
fn busy_ring(k: &Kit) -> El {
    Spinner::new().size(14.0).color(if k.dark { k.c.icon_base } else { k.c.icon_muted }).build(k)
}

pub(super) struct TabStripCore<'a> {
    pub key: String,
    pub orientation: TabOrientation,
    pub items: Vec<TabItemCore<'a>>,
    pub lazy: Option<(Vec<TabSlot<'a>>, Build<'a>)>,
    pub active: Option<&'a str>,
    pub viewport: Option<f32>,
    pub overflow: Option<TabOverflow>,
    pub gap: Option<f32>,
    pub row_height: f32,
    pub select_on_drag_start: Option<bool>,
    pub tear: Option<TearOutSpec>,
    pub matched: Option<&'a str>,
    pub spring: Spring,
    pub trailing: Option<El>,
    pub trailing_width: f32,
    pub drag: Option<TabDragState>,
    pub fill: Option<Color>,
    pub close_icon: Option<&'static Icon>,
    pub close_tab_stop: bool,
    pub hover_none: bool,
    pub busy: fn(&Kit) -> El,
    pub label: String,
    pub words: TabWords,
}

impl<'a> TabStripCore<'a> {
    pub(super) fn new(key: String, orientation: TabOrientation) -> Self {
        TabStripCore {
            key,
            orientation,
            items: Vec::new(),
            lazy: None,
            active: None,
            viewport: None,
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
            hover_none: false,
            busy: busy_ring,
            label: "Tabs".into(),
            words: TabWords::default(),
        }
    }

    pub(super) fn assemble(mut self, k: &Kit, mut layer: Option<&mut Vec<El>>, bind: &dyn TabBind) -> El {
        let vertical = self.orientation == TabOrientation::Vertical;
        let overflow = self.overflow.unwrap_or(if vertical { TabOverflow::VERTICAL_DEFAULT } else { TabOverflow::HORIZONTAL_DEFAULT });
        let gap = self.gap.unwrap_or(if vertical { 3.0 } else { 6.0 });
        // Tabs given whole take the lazy path too: each is built once, in order.
        let (slots, mut build): (Vec<TabSlot<'a>>, Build<'a>) = match self.lazy.take() {
            Some(lazy) => lazy,
            None => {
                let slots = self.items.iter().map(|t| TabSlot { id: t.id.clone(), pinned: t.pinned, two_line: t.subtitle.is_some(), keep: t.marked || t.editor.is_some() }).collect();
                let mut items: Vec<Option<TabItemCore<'a>>> = std::mem::take(&mut self.items).into_iter().map(Some).collect();
                (slots, Box::new(move |i| items[i].take().expect("a tab builds once")))
            }
        };
        let count = slots.len();
        let pinned = slots.iter().filter(|t| t.pinned).count();
        let mut drag = self.drag.take().unwrap_or_default();
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
        let pitches: Vec<f32> = slots.iter().map(|t| pitch(t.pinned, t.two_line)).collect();
        let auto = auto_scroll::view(&format!("{}-scroll", self.key));
        let mut landing = None;
        if let Some(point) = drag.incoming() {
            let slot = landing_slot(&pitches, pinned, point, &auto, vertical);
            landing = Some(slot);
            drag.open_gap(slot, pitch(false, false));
        }
        let st = Strip {
            base: self.fill.unwrap_or(k.c.bg_deep),
            separator: if k.dark { k.c.layer_02 } else { k.c.layer_03 },
            gap,
            row_height: self.row_height,
            two_line_height,
            link_inset: 8.0,
            count,
            spring: self.spring,
            select_on_drag_start: self.select_on_drag_start.unwrap_or(true),
            horizontal_lock: !vertical,
            matched: self.matched.map(str::to_owned),
            close_icon: self.close_icon,
            close_tab_stop: self.close_tab_stop,
            hover_none: self.hover_none,
            busy: self.busy,
            words: self.words,
            auto,
            scroll_key: format!("{}-scroll", self.key),
            list_key: format!("{}-list", self.key),
            layered: layer.is_some(),
            drag,
        };
        let active = self.active;
        // A hover hides its tab's separators by hover groups (`tab::build`); the active tab has none.
        let quiet = |i: usize| active == Some(slots[i].id.as_ref());
        let tear = self.tear.filter(|t| count > 1 || t.lone);
        // Which tabs are built: past `BAND_PAST`, those near what the strip shows, and the ones a
        // gesture, a key or a menu is on wherever they are.
        let shown = (count > BAND_PAST).then(|| {
            let along = if vertical { (self.row_height + gap) * 6.0 } else { (tab_w + gap) * 4.0 };
            let fallback = if vertical { 1200.0 } else { self.viewport.unwrap_or(2000.0) };
            let (lo, hi) = band(&st.list_key, &st.scroll_key, vertical, along.max(64.0), fallback);
            let mut start = 0.0;
            let mut built: Vec<bool> = pitches
                .iter()
                .map(|p| {
                    let near = start < hi && start + p > lo;
                    start += p;
                    near
                })
                .collect();
            let keep = [Some(0), Some(count - 1), slots.iter().position(|t| active == Some(t.id.as_ref())), st.drag.dragging()];
            let near_landing = landing.map(|s| [s.checked_sub(1), Some(s)]).unwrap_or_default();
            for i in keep.into_iter().chain(near_landing).flatten().chain(slots.iter().enumerate().filter(|(_, t)| t.keep || t.pinned).map(|(i, _)| i)) {
                if let Some(b) = built.get_mut(i) {
                    *b = true;
                }
            }
            built
        });
        let mut tabs: Vec<El> = Vec::with_capacity(count.min(BAND_PAST * 2));
        // Where each built tab's element is in `tabs`.
        let mut at_child: Vec<Option<usize>> = vec![None; count];
        // A run of tabs not built: where it starts, and its length along the strip.
        let mut run: Option<(usize, f32)> = None;
        let space = |tabs: &mut Vec<El>, run: &mut Option<(usize, f32)>, end: usize| {
            let Some((from, len)) = run.take() else { return };
            let el = el().key(format!("{}-unbuilt-{from}", st.list_key)).a11y_hidden();
            tabs.push(if vertical {
                // A row reaches half the gap past each neighbour, but the first and last don't.
                let ends = (if from == 0 { gap / 2.0 } else { 0.0 }) + (if end == count { gap / 2.0 } else { 0.0 });
                el.w_full().h(len - ends)
            } else {
                // The row's own gap stands after it, as after a tab.
                el.w(len - gap).h(st.row_height)
            });
        };
        for i in 0..count {
            if shown.as_ref().is_some_and(|b| !b[i]) {
                let (from, len) = run.unwrap_or((i, 0.0));
                run = Some((from, len + pitches[i]));
                continue;
            }
            space(&mut tabs, &mut run, i);
            let item = build(i);
            let pin = slots[i].pinned;
            let width = (!vertical).then_some(if pin { pinned_w } else { tab_w });
            let (lo, hi) = if pin { (0, pinned) } else { (pinned, count) };
            let at = tab::Place {
                index: i,
                lane: Lane { index: i, lo, hi, pitch: pitches[i], pinned: pin, tear },
                width,
                density: if pin { TabDensity::IconOnly } else { width.map_or(TabDensity::Full, TabOverflow::density_for) },
                active: quiet(i),
                separator: !vertical && i > 0 && !quiet(i) && !quiet(i - 1),
            };
            let (tab, ghost) = tab::build(item, at, (i > 0).then(|| slots[i - 1].id.as_ref()), &st, bind, k);
            if let (Some(layer), Some(ghost)) = (layer.as_deref_mut(), ghost) {
                layer.push(ghost);
            }
            at_child[i] = Some(tabs.len());
            tabs.push(tab);
        }
        space(&mut tabs, &mut run, count);
        // The incoming tab's place: an empty slot its own length, which the tabs after it make
        // room for on the reorder spring and the tab takes when it's let go.
        if let Some((slot, pitch)) = st.drag.gap() {
            let place = el().key(format!("{}-incoming", st.list_key)).a11y_hidden();
            let place = if vertical { place.w_full().h(pitch) } else { place.w(pitch - gap).h(st.row_height) };
            let child = at_child.get(slot).copied().flatten().unwrap_or(tabs.len());
            tabs.insert(child, place);
        }
        let fade = overflow.fade();
        let root = if vertical {
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
        bind.bind_root_hover(root)
    }
}

/// The stretch of the strip whose tabs are built, in points along it from its first tab: what
/// the scroll viewport shows, rounded out to whole steps of `step` and a step more each side. The
/// view reads where the list is scrolled to in those steps, so a scroll runs it once a step, not
/// every frame; before the first layout it's the strip's start and `fallback` long.
fn band(list_key: &str, scroll_key: &str, vertical: bool, step: f32, fallback: f32) -> (f32, f32) {
    let a = usize::from(vertical);
    let start = rune::memo::layout_with(list_key, move |r| r[a]);
    let at = start.and_then(|start| rune::memo::window_layout_with(list_key, move |w| ((start - w[a]) / step).floor() as i32)).unwrap_or(0);
    let shows = rune::memo::layout_with(scroll_key, move |r| (r[2 + a] / step).ceil() as i32).unwrap_or((fallback / step).ceil() as i32);
    ((at - 1) as f32 * step, (at + 2 + shows) as f32 * step)
}

/// The slot a tab dragged in from another strip lands in at `point` (view points): after each
/// unpinned tab whose middle the point has passed, measured with each tab's own length (`pitches`)
/// from where the strip's viewport drew and how far it has scrolled.
fn landing_slot(pitches: &[f32], pinned: usize, point: [f32; 2], auto: &auto_scroll::View, vertical: bool) -> usize {
    let a = usize::from(vertical);
    let along = point[a] - auto_scroll::rect(auto)[a] + auto_scroll::offset(auto, vertical);
    let mut start = 0.0;
    for (i, pitch) in pitches.iter().enumerate() {
        if i >= pinned && along < start + pitch / 2.0 {
            return i;
        }
        start += pitch;
    }
    pitches.len()
}
