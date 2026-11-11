//! rune-kit's `TabStrip` both ways over one list of sessions, as OpenCode lays them out: tabs
//! across the titlebar and side tabs down the left, with the new-tab button after the last
//! titlebar tab. Drag a tab along either strip to reorder it (Escape puts it back), drag a
//! titlebar tab down off the strip to tear it out, middle-click or hover and press × to close
//! one, and keep closing: the tabs keep their width until the pointer leaves the strip. The
//! card shows the three densities at fixed widths.
//!
//! `cargo run -p rune-kit --example tab_strip` opens the window. `-- --scene overflow` starts
//! with forty tabs, so the titlebar strip scrolls with feathered edges and the side tabs do too.
//! Stills: `-- --snap tabs.png --theme dark --size 1280x800`.
use rune::{AppSpec, Color, Cx, El, Tx, col, row, text};
use rune_kit::shell::{TabDragState, TabItem, TabItemStatus, TabOrientation, TabReorder, TabStrip, TabTearOut, TearOutSpec, WindowTitlebar};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};
use rune_theme_opencode::icons as ic;

const PROJECTS: [(&str, &str); 5] = [("acme-web", "gray"), ("tally", "orange"), ("notes", "blue"), ("rune", "green"), ("site", "purple")];
const TITLES: [&str; 12] = [
    "Formatting tour",
    "Fix price formatting",
    "Profile the frame loop",
    "Port the settings pane",
    "Theme tokens for dark mode",
    "Spring presets for sheets",
    "Draft the changelog",
    "Window restoration bug",
    "Keyboard navigation audit",
    "Sidebar resize behaviour",
    "Scroll jank in the list",
    "Snap layouts on Windows",
];

#[derive(Clone)]
struct Tab {
    id: String,
    title: String,
    project: usize,
    status: Option<TabItemStatus>,
    draft: bool,
    pinned: bool,
}

struct App {
    tabs: Vec<Tab>,
    active: Option<String>,
    top: TabDragState,
    side: TabDragState,
    demo: [TabDragState; 3],
    next: usize,
    note: String,
}

fn tab(i: usize) -> Tab {
    let status = match i % 7 {
        1 => Some(TabItemStatus::Busy),
        3 => Some(TabItemStatus::Unread),
        4 => Some(TabItemStatus::Attention),
        6 => Some(TabItemStatus::Failed),
        _ => None,
    };
    Tab { id: format!("s{i}"), title: TITLES[i % TITLES.len()].into(), project: i % PROJECTS.len(), status, draft: false, pinned: false }
}

fn with_tabs(n: usize, active: usize) -> App {
    let mut tabs: Vec<Tab> = (0..n).map(tab).collect();
    if let Some(t) = tabs.get_mut(5) {
        t.draft = true;
        t.status = None;
    }
    App {
        active: tabs.get(active).map(|t| t.id.clone()),
        tabs,
        top: TabDragState::new(),
        side: TabDragState::new(),
        demo: Default::default(),
        next: n,
        note: String::new(),
    }
}

fn init() -> App {
    with_tabs(8, 0)
}

fn select(s: &mut App, id: &str, _: &mut Tx) {
    s.active = Some(id.to_owned());
}

fn close(s: &mut App, id: &str, _: &mut Tx) {
    let Some(i) = s.tabs.iter().position(|t| t.id == id) else { return };
    s.tabs.remove(i);
    if s.active.as_deref() == Some(id) {
        s.active = s.tabs.get(i.min(s.tabs.len().saturating_sub(1))).map(|t| t.id.clone());
    }
}

fn reorder(s: &mut App, r: TabReorder, _: &mut Tx) {
    let t = s.tabs.remove(r.from);
    s.tabs.insert(r.to, t);
}

fn tear_out(s: &mut App, t: TabTearOut, _: &mut Tx) {
    let title = s.tabs.get(t.index).map_or("", |x| x.title.as_str());
    s.note = match t.window_point {
        Some([x, y]) => format!("Torn out: \u{201c}{title}\u{201d}, dropped at {x:.0}, {y:.0}"),
        None => format!("Torn out: \u{201c}{title}\u{201d}"),
    };
}

fn new_tab(s: &mut App) {
    let id = format!("d{}", s.next);
    s.next += 1;
    s.tabs.push(Tab { id: id.clone(), title: "New session".into(), project: 0, status: None, draft: true, pinned: false });
    s.active = Some(id);
}

/// `@opencode/ui`'s project avatar: a 16 point rounded square, the project's colour under a
/// light sheen, its initial in 11 point semibold white.
fn avatar(name: &str, variant: &str, k: &Kit) -> El {
    let t = rune_theme_opencode::theme(k.dark);
    let initial = name.chars().next().unwrap_or(' ').to_ascii_uppercase().to_string();
    row()
        .size(16.0, 16.0)
        .radius(4.0)
        .center()
        .fill(t.color(&format!("v2-avatar-bg-{variant}")))
        .gradient(t.color("v2-alpha-light-16"), Color::TRANSPARENT)
        .border(0.5, t.color(&format!("v2-avatar-border-{variant}")))
        .child(text(initial).font(rune::TextStyle { size: 11.0, weight: 530.0, line_height: 11.0, tracking: 0.05, ..k.ty.small }).color(t.color("v2-avatar-fg")))
}

fn items<'a>(s: &'a App, k: &Kit, two_line: bool) -> Vec<TabItem<'a, App>> {
    s.tabs
        .iter()
        .map(|t| {
            let (project, variant) = PROJECTS[t.project];
            let active = s.active.as_deref() == Some(t.id.as_str());
            // The dots stay off the active tab; the busy ring stays on.
            let status = t.status.filter(|st| !active || *st == TabItemStatus::Busy);
            let item = TabItem::new(t.id.as_str(), t.title.as_str()).status(status).pinned(t.pinned);
            let item = if t.draft { item.icon(&ic::EDIT) } else { item.avatar(avatar(project, variant, k)) };
            if two_line && !t.draft { item.subtitle(project) } else { item }
        })
        .collect()
}

fn titlebar(s: &App, cx: &mut Cx<App>, k: &Kit, layer: &mut Vec<El>) -> El {
    let nav = |glyph, label: &str| IconButton::new(glyph).control_size(ControlSize::Large).variant(IconButtonVariant::GhostMuted).label(label).build(k).w(36.0);
    let plus = IconButton::new(&ic::PLUS)
        .control_size(ControlSize::Large)
        .variant(IconButtonVariant::GhostMuted)
        .label("New session")
        .on_press(new_tab)
        .build(k)
        .key("new-tab");
    let strip = TabStrip::new("top", TabOrientation::Horizontal)
        .tabs(items(s, k, false))
        .active(s.active.as_deref())
        .viewport_width(cx.layout_size("top").map(|z| z[0]))
        .drag_state(&s.top, |s: &mut App| &mut s.top)
        .tear_out(Some(TearOutSpec::default()))
        .close_icon(&ic::XMARK_SMALL)
        .trailing_action(plus)
        .on_select(select)
        .on_close(close)
        .on_reorder(reorder)
        .on_tear_out(tear_out)
        .build_layered(k, layer);
    WindowTitlebar::from_insets(cx.chrome_insets())
        .leading(row().key("nav").gap(2.0).center_y().child(nav(&ic::GRID_PLUS, "Home")).child(nav(&ic::MAGNIFYING_GLASS, "Search sessions")))
        .tabs(strip)
        .build(k)
}

fn sidebar(s: &App, k: &Kit) -> El {
    let strip = TabStrip::new("side", TabOrientation::Vertical)
        .tabs(items(s, k, false))
        .active(s.active.as_deref())
        .drag_state(&s.side, |s: &mut App| &mut s.side)
        .close_icon(&ic::XMARK_SMALL)
        .on_select(select)
        .on_close(close)
        .on_reorder(reorder)
        .build(k);
    col().key("sidebar").w(240.0).h_full().min_h(0.0).pl(11.0).pr(3.0).pb(10.0).child(strip)
}

/// One strip at a fixed width, to show a density.
fn sample(s: &App, k: &Kit, i: usize, width: f32, label: &str) -> El {
    let access: fn(&mut App) -> &mut TabDragState = match i {
        0 => |s: &mut App| &mut s.demo[0],
        1 => |s: &mut App| &mut s.demo[1],
        _ => |s: &mut App| &mut s.demo[2],
    };
    let strip = TabStrip::new(format!("sample-{i}"), TabOrientation::Horizontal)
        .tabs(items(s, k, false).into_iter().take(8))
        .active(s.active.as_deref())
        .viewport_width(Some(width))
        .drag_state(&s.demo[i], access)
        .close_icon(&ic::XMARK_SMALL)
        .on_select(select)
        .build(k);
    col()
        .gap(6.0)
        .child(text(label.to_owned()).font(k.ty.text_xsmall_medium).color(k.c.text_muted))
        .child(row().w(width + 16.0).h(44.0).px(8.0).center_y().radius(8.0).fill(k.c.bg_deep).child(strip))
}

fn card(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let title = s.tabs.iter().find(|t| Some(&t.id) == s.active.as_ref()).map_or("No tab", |t| t.title.as_str());
    let more = Button::new("Add 12 tabs")
        .variant(ButtonVariant::Neutral)
        .on_press(|s: &mut App| (0..12).for_each(|_| {
            let t = tab(s.next);
            s.next += 1;
            s.tabs.push(Tab { id: format!("x{}", s.next), ..t });
        }))
        .build(k)
        .key("add-many");
    let reset = Button::new("Reset").variant(ButtonVariant::Ghost).on_press(|s: &mut App| *s = init()).build(k).key("reset");
    col()
        .key("card")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .ml(8.0)
        .mr(8.0)
        .mb(8.0)
        .mt(1.0)
        .radius(10.0)
        .fill(c.bg_base)
        .padding(24.0)
        .gap(20.0)
        .child(text(title.to_owned()).key("heading").font(k.ty.body_medium).color(c.text_base).motion_intent(rune::MotionIntent::CUT))
        .child(row().gap(8.0).child(more).child(reset).child(text(s.note.clone()).key("note").font(k.ty.ui).color(c.text_muted)))
        .child(sample(s, k, 0, 720.0, "Full (over 64 points)"))
        .child(sample(s, k, 1, 470.0, "Compact (44 to 64 points)"))
        .child(sample(s, k, 2, 290.0, "Icon only (44 points or less)"))
}

fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let mut layer = Vec::new();
    let bar = titlebar(s, cx, k, &mut layer);
    rune_kit::root(
        col().key("window").w_full().h_full().fill(k.c.bg_deep).child(bar).child(row().key("body").w_full().grow(1.0).min_h(0.0).child(sidebar(s, k)).child(card(s, k))),
        layer,
    )
}

fn main() {
    rune::App::new(AppSpec {
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[("tabs", |s: &mut App| *s = init()), ("overflow", |s: &mut App| *s = with_tabs(40, 2)), ("single", |s: &mut App| *s = with_tabs(1, 0))],
        ..AppSpec::new("Tab strip", (1280.0, 800.0), init, view)
    })
    .titlebar(rune::Titlebar::new(44.0))
    .run();
}
