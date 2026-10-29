//! An app shell from rune-kit's `shell` components, set up like a desktop agent-chat app: a
//! framed main pane with the toolbar inside it, a padded sidebar of two-line session rows with
//! a status slot that turns into a close button under the pointer, and an inspector. Drag a
//! session to reorder it (Escape puts it back), double-click one to rename it. ⇧⌘B shows and
//! hides the sidebar; with it closed, rest the pointer at the window's left edge or on the
//! toggle to peek, and click to keep it. ⌥⌘I shows the inspector, and a pane's inner edge
//! resizes it. `-- virtual` swaps the sessions for 5,000 in a virtualised list.
//!
//! `cargo run -p rune-kit --example shell` opens the window. The stills in `demo/kit/` are
//! `-- --snap demo/kit/shell-light.png --theme light --size 1040x640` (and `dark`).
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use std::borrow::Cow;

use rune::{AppSpec, Chrome, Cx, El, Icon, KeyEvent, Spring, TextEdit, Tx, col, oklch, row, text};
use rune_kit::shell::{
    self, Page, PaneMotion, PaneState, Reorder, RowStatus, Sidebar, SidebarPeek, SidebarRow, SidebarToggle, SplitView, Toolbar,
};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Switch, glyphs};

static HOME: Icon = Icon {
    name: "shell-home",
    view: 16.0,
    body: r#"<path d="M3 7L8 3L13 7V12.5H9.5V9.5H6.5V12.5H3Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};
static PLUS: Icon = Icon {
    name: "shell-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static SEARCH: Icon = Icon {
    name: "shell-search",
    view: 16.0,
    body: r#"<circle cx="7" cy="7" r="3.75" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M10 10L12.75 12.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
static SLIDERS: Icon = Icon {
    name: "shell-sliders",
    view: 16.0,
    body: r#"<path d="M3 5H13M3 11H13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><circle cx="6" cy="5" r="1.6" fill="currentColor"/><circle cx="10.5" cy="11" r="1.6" fill="currentColor"/>"#,
};

/// The rows' height, and the points between them.
const ROW: f32 = 45.0;
const GAP: f32 = 4.0;

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Idle,
    Busy,
    Attention,
    Unread,
}

struct Session {
    id: String,
    title: String,
    project: usize,
    status: Status,
}

const PROJECTS: [(&str, f32); 4] = [("rune", 250.0), ("site", 150.0), ("motion", 30.0), ("notes", 320.0)];

struct App {
    sidebar: PaneState,
    inspector: PaneState,
    sessions: Vec<Session>,
    reorder: Reorder,
    selected: String,
    renaming: Option<String>,
    rename: TextEdit,
    virtualised: bool,
    page: bool,
    page_pair: bool,
}

fn init() -> App {
    let s = |id: &str, title: &str, project, status| Session { id: id.into(), title: title.into(), project, status };
    let mut sessions = vec![
        s("s1", "Fix scroll jank in the list", 0, Status::Busy),
        s("s2", "Profile the frame loop", 0, Status::Attention),
        s("s3", "Port the settings pane", 1, Status::Unread),
        s("s4", "Sidebar resize behaviour", 0, Status::Idle),
        s("s5", "Theme tokens for dark mode", 1, Status::Idle),
        s("s6", "Spring presets for sheets", 2, Status::Unread),
        s("s7", "Draft the changelog", 3, Status::Idle),
        s("s8", "Window restoration bug", 0, Status::Idle),
        s("s9", "Keyboard navigation audit", 1, Status::Idle),
    ];
    let args = rune::extra_args();
    let virtualised = args.iter().any(|a| a == "virtual");
    let page = args.iter().any(|a| a == "page");
    let page_pair = args.iter().any(|a| a == "page-pair");
    if virtualised {
        sessions = (0..5000).map(|i| s(&format!("v{i}"), &format!("Session {}", 5000 - i), i % 4, Status::Idle)).collect();
    }
    App {
        sidebar: PaneState::new(276.0).range(136.0, 640.0),
        inspector: PaneState::new(280.0).range(220.0, 420.0).collapsed(),
        selected: sessions[0].id.clone(),
        sessions,
        // Virtualised rows take no gap between them: the pitch is the row's height.
        reorder: Reorder::new().gap(if virtualised { 0.0 } else { GAP }),
        renaming: None,
        rename: TextEdit::new(""),
        virtualised,
        page,
        page_pair,
    }
}

fn avatar(name: &str, hue: f32, k: &Kit) -> El {
    let initial = name.chars().next().unwrap_or(' ').to_ascii_uppercase().to_string();
    row()
        .size(16.0, 16.0)
        .radius(4.0)
        .center()
        .fill(oklch(0.62, 0.12, hue))
        .child(text(initial).font(k.ty.small_medium).color(rune::Color::WHITE))
}

fn session_row(s: &App, i: usize, k: &Kit) -> El {
    let session = &s.sessions[i];
    let (project, hue) = PROJECTS[session.project];
    let id = session.id.clone();
    let mut r = SidebarRow::new(session.title.clone())
        .height(ROW)
        .avatar(avatar(project, hue, k))
        .subtitle(project)
        .selected(s.selected == session.id)
        .on_select({
            let id = id.clone();
            move |s: &mut App, _| s.selected = id.clone()
        })
        .on_close({
            let id = id.clone();
            move |s: &mut App, _| s.sessions.retain(|x| x.id != id)
        })
        .on_double_click({
            let id = id.clone();
            move |s: &mut App, _| {
                let title = s.sessions.iter().find(|x| x.id == id).map(|x| x.title.clone()).unwrap_or_default();
                s.rename = TextEdit::new(title);
                s.rename.anchor = 0;
                s.rename.focused = true;
                s.renaming = Some(id.clone());
            }
        })
        .reorder(&s.reorder, i, s.sessions.len(), |s: &mut App| &mut s.reorder, |s: &mut App, from, to| {
            let x = s.sessions.remove(from);
            s.sessions.insert(to, x);
        });
    match session.status {
        Status::Busy => r = r.status(RowStatus::Busy),
        Status::Attention => r = r.status(RowStatus::Attention),
        Status::Unread => r = r.status(RowStatus::Unread),
        Status::Idle => {}
    }
    if s.renaming.as_deref() == Some(session.id.as_str()) {
        r = r.renaming(
            &s.rename,
            |s: &mut App| &mut s.rename,
            |s: &mut App, _| {
                let (id, title) = (s.renaming.take(), s.rename.text.trim().to_owned());
                if let Some(x) = s.sessions.iter_mut().find(|x| Some(&x.id) == id.as_ref()).filter(|_| !title.is_empty()) {
                    x.title = title;
                }
            },
            |s: &mut App, _| s.renaming = None,
        );
    }
    r.build(k).key(session.id.clone())
}

fn sidebar(s: &App, k: &Kit) -> El {
    let sb = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(4.0)
        .header_padding(0.0, 16.0)
        .row_gap(GAP)
        .header(SidebarRow::new("Home").icon(&HOME).height(32.0).build(k).key("home"))
        .header(SidebarRow::new("Search").icon(&SEARCH).detail("⌘K").height(32.0).build(k).key("search"))
        .header(SidebarRow::new("New session").icon(&PLUS).detail("⌘N").height(32.0).build(k).key("new"))
        .footer(
            SidebarRow::new("Settings")
                .icon(&SLIDERS)
                .height(32.0)
                .on_select(|s: &mut App, _| s.page = true)
                .build(k)
                .key("settings"),
        );
    if s.virtualised {
        let kit = k.clone();
        return sb
            .row_height(ROW)
            .virtual_rows(s.sessions.len(), |s: &App, i| Cow::Borrowed(s.sessions[i].id.as_str()), move |s: &App, i| session_row(s, i, &kit))
            .build(k);
    }
    (0..s.sessions.len()).fold(sb, |sb, i| sb.child(session_row(s, i, k))).build(k)
}

fn settings_page(dark: bool, inset: f32) -> El {
    let k = Kit::neutral(dark);
    let c = &k.c;
    let setting = |title: &str, desc: &str, on: bool| {
        row()
            .w_full()
            .py(12.0)
            .px(16.0)
            .radius(k.radius.md)
            .fill(c.layer_01)
            .border(1.0, c.border_weak)
            .center_y()
            .justify_between()
            .child(
                col()
                    .gap(2.0)
                    .child(text(title.to_owned()).font(k.ty.ui_medium).color(c.text_base))
                    .child(text(desc.to_owned()).font(k.ty.small).color(c.text_muted)),
            )
            .child(Switch::new(on).build(&k))
    };
    let body = col()
        .key("settings-body")
        .w_full()
        .h_full()
        .padding(24.0)
        .items_center()
        .child(
            col()
                .w_full()
                .max_w(520.0)
                .gap(12.0)
                .child(text("Workspace Settings").font(k.ty.body_medium).color(c.text_base))
                .child(setting("Unlock with Touch ID", "Prompt for biometrics when returning after idle", true))
                .child(setting("Clear clipboard automatically", "Remove copied secrets after 30 seconds", true))
                .child(setting("Show sidebar unread badges", "Highlight active background tasks in the toggle", false)),
        );
    let back = Button::new("Back")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut App| s.page = false)
        .build(&k)
        .key("back");
    let lock = IconButton::new(&glyphs::LOCK)
        .variant(IconButtonVariant::GhostMuted)
        .help("Lock")
        .shortcut("⌘L")
        .build(&k)
        .key("lock");
    Page::new(body)
        .title("Settings")
        .leading(back)
        .trailing(lock)
        .titlebar_inset(inset)
        .frame_inset(8.0)
        .toolbar_in_panel()
        .build(&k)
}

fn view(s: &App, cx: &mut Cx<App>) -> El {
    if s.page_pair {
        return row()
            .w_full()
            .h_full()
            .child(col().grow(1.0).h_full().child(settings_page(false, 74.0)))
            .child(col().grow(1.0).h_full().child(settings_page(true, 74.0)));
    }
    if s.page {
        return settings_page(cx.system_dark(), cx.titlebar_inset());
    }
    let k = Kit::neutral(cx.system_dark());
    let c = &k.c;
    let title = s.sessions.iter().find(|x| x.id == s.selected).map_or("", |x| x.title.as_str());
    let line = |t: &str, ink| text(t.to_owned()).font(k.ty.body).color(ink).wrap();
    let main = col()
        .key("transcript")
        .w_full()
        .padding(24.0)
        .gap(14.0)
        .child(text(title.to_owned()).key("heading").font(k.ty.body_medium).color(c.text_base))
        .child(line("The list stutters when a long session scrolls past the fold. Frames 40 to 52 spend most of their time laying out rows that never draw.", c.text_muted))
        .child(line("Virtualising the rows keeps layout to the viewport and two screens either side, so a commit costs the rows that changed.", c.text_muted))
        .child(line("Next: keep the selected row's height when its preview text grows, so the list doesn't jump under the pointer.", c.text_muted));
    let field = |label: &str, value: &str| {
        col().gap(2.0).child(text(label.to_owned()).font(k.ty.small).color(c.text_faint)).child(text(value.to_owned()).font(k.ty.ui).color(c.text_base))
    };
    let inspector = col()
        .key("details")
        .w_full()
        .padding(16.0)
        .gap(14.0)
        .child(text("Details").font(k.ty.ui_medium).color(c.text_base))
        .child(field("Model", "Neutral 2"))
        .child(field("Branch", "dev"))
        .child(field("Messages", "38"))
        .child(field("Updated", "4 minutes ago"));
    let toolbar = Toolbar::new().title(title).trailing(
        IconButton::new(&shell::INSPECTOR)
            .variant(IconButtonVariant::GhostMuted)
            .expanded(s.inspector.is_open())
            .label(if s.inspector.is_open() { "Hide inspector" } else { "Show inspector" })
            .on_press(|s: &mut App| s.inspector.toggle())
            .build(&k)
            .key("inspector-toggle"),
    );
    let unread = s.sessions.iter().any(|x| x.status == Status::Unread);
    SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar(s, &k))
        .inspector(&s.inspector, |s: &mut App| &mut s.inspector, inspector)
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(18.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header().unread(unread))
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15))
        .sidebar_peek(
            SidebarPeek::new()
                .edge(16.0, ms(40))
                .toggle_delay(ms(80))
                .below(56.0)
                .hold(15.0, ms(100))
                .spring(Spring::new(0.18, 0.0)),
        )
        .reduced_motion(cx.is_reduced_motion())
        .build(&k)
}

fn ms(n: u64) -> std::time::Duration {
    std::time::Duration::from_millis(n)
}

fn on_key(s: &mut App, ev: &KeyEvent, _: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.is_command('i') && ev.mods.alt {
        s.inspector.toggle();
        return true;
    }
    false
}

fn main() {
    // The traffic lights centred on the toolbar, 8 points down in the framed pane: 32 points.
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 25.0)), drag_regions: true };
    rune::App::new(AppSpec { on_key: Some(on_key), ..AppSpec::new("Shell", (1040.0, 640.0), init, view) }).chrome(chrome).run();
}
