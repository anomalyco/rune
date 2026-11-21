//! A notes app with a ⌘Space launcher panel: three windows that `--desktop` records together.
//! `cargo run -p launcher` opens the notes window; ⌘Space shows the launcher.
//!
//! ```sh
//! target/release/launcher --desktop examples/launcher/backdrop.png \
//!   --script examples/launcher/scripts/demo.json --video demo.mp4 --keys --pointer
//! ```
//!
//! With `--desktop` the app adds a fake menu bar, so the recording reads as a desktop.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};

use rune::tok::{self, type_};
use rune::{
    Align, AppHandle, AppSpec, App, Color, Cx, DisplayTarget, El, Key, KeyEvent, MacWindowOptions, OverlayPlacement,
    ScreenEdge, TextEdit, Tx, WindowOptions, WindowSpec, col, el, row, spacer, text, text_input,
};

/// Recording with `--desktop`: there is no native chrome, so the app draws its own.
static DESKTOP: AtomicBool = AtomicBool::new(false);

const NOTES: [(&str, &str); 4] = [
    ("Groceries", "Oat milk, lemons, basil, sourdough, two avocados."),
    ("Trip to Lisbon", "Book the tram tour. Pastéis de nata first."),
    ("Reading list", "The Design of Everyday Things. Thinking in Systems."),
    ("Ideas", "A launcher that opens notes by typing their first letters."),
];

struct Notes {
    selected: usize,
    focused: bool,
}

struct Launcher {
    query: TextEdit,
    picked: usize,
    me: AppHandle<Launcher>,
    notes: AppHandle<Notes>,
}

fn matches(query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..NOTES.len()).filter(|&i| NOTES[i].0.to_lowercase().contains(&q)).collect()
}

fn notes_view(s: &Notes, _: &mut Cx<Notes>) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let light = |c: (f32, f32, f32)| el().size(12.0, 12.0).radius(6.0)
        .fill(if s.focused { Color { r: c.0, g: c.1, b: c.2, a: 1.0 } } else { t.control_off });
    let title_bar = row().h(38.0).px(14.0).gap(8.0).center_y()
        .child(light((1.0, 0.37, 0.34))).child(light((1.0, 0.74, 0.18))).child(light((0.16, 0.79, 0.25)))
        .child(spacer()).child(text("Notes").font(type_::TITLE).color(t.text_secondary)).child(spacer())
        .child(el().w(52.0));
    let mut list = col().key("list").w(170.0).h_full().px(8.0).gap(2.0);
    for (i, (title, _)) in NOTES.iter().enumerate() {
        let on = i == s.selected;
        list = list.child(row().key(format!("note-{i}")).h(30.0).px(10.0).radius(6.0).center_y()
            .fill(if on { accent.alpha(0.85) } else { Color::TRANSPARENT })
            .child(text(*title).font(type_::BODY).color(if on { Color::WHITE } else { t.text }))
            .on_press(move |s: &mut Notes| s.selected = i));
    }
    let (title, body) = NOTES[s.selected];
    let page = col().key("page").grow(1.0).h_full().padding(20.0).gap(10.0).fill(t.bg)
        .child(text(title).font(type_::TITLE.lh(22.0)).color(t.text))
        .child(text(body).font(type_::BODY.lh(20.0)).color(t.text_secondary))
        .child(spacer())
        .child(text("⌘Space to search").font(type_::LABEL).color(t.text_secondary.alpha(0.7)));
    col().w_full().h_full().fill(t.card)
        .child(if DESKTOP.load(Ordering::Relaxed) { title_bar } else { el().h(12.0) })
        .child(row().w_full().grow(1.0).child(list).child(page))
}

fn launcher_view(s: &Launcher, _: &mut Cx<Launcher>) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let found = matches(&s.query.text);
    let field = text_input(&s.query, type_::BODY.lh(26.0)).key("query").single_line().grow(1.0)
        .color(t.text).placeholder("Search notes", t.text_secondary)
        .on_edit(|s: &mut Launcher| &mut s.query)
        .on_input_key(|s: &mut Launcher, ev: &KeyEvent, tx: &mut Tx| {
            let n = matches(&s.query.text).len();
            match ev.key {
                Key::Down if n > 0 => s.picked = (s.picked + 1) % n,
                Key::Up if n > 0 => s.picked = (s.picked + n - 1) % n,
                Key::Escape => tx.close_window(),
                _ => return false,
            }
            true
        })
        .on_input_change(|s: &mut Launcher, _| s.picked = 0)
        .on_submit(open_picked);
    let mut panel = col().key("panel").w_full().radius(14.0).clip_rounded(14.0)
        .fill(t.card).border(1.0, Color::WHITE.alpha(0.1))
        .child(row().h(56.0).px(18.0).gap(12.0).center_y()
            .child(text("⌕").font(type_::TITLE.lh(26.0)).color(t.text_secondary))
            .child(field));
    if !found.is_empty() {
        let mut results = col().key("results").padding(6.0).gap(2.0);
        for (row_i, &i) in found.iter().enumerate() {
            let on = row_i == s.picked;
            results = results.child(row().key(format!("result-{i}")).h(36.0).px(12.0).radius(8.0).center_y()
                .fill(if on { accent.alpha(0.85) } else { Color::TRANSPARENT })
                .child(text(NOTES[i].0).font(type_::BODY).color(if on { Color::WHITE } else { t.text })));
        }
        panel = panel.child(el().h(1.0).w_full().fill(t.separator)).child(results);
    }
    col().w_full().h_full().child(panel)
}

fn open_picked(s: &mut Launcher, tx: &mut Tx) {
    let Some(&i) = matches(&s.query.text).get(s.picked) else { return };
    s.notes.update(move |n, _| n.selected = i);
    s.notes.focus();
    s.query.clear();
    tx.close_window();
}

fn menu_bar_view(_: &(), _: &mut Cx<()>) -> El {
    let ink = Color::WHITE.alpha(0.92);
    let item = |s: &'static str| text(s).font(type_::BODY).color(ink);
    row().w_full().h_full().px(14.0).gap(18.0).center_y().fill(Color::BLACK.alpha(0.22))
        .child(text("◆").font(type_::TITLE).color(ink))
        .child(text("Notes").font(type_::TITLE).color(ink))
        .child(item("File")).child(item("Edit")).child(item("View")).child(item("Window"))
        .child(spacer())
        .child(item("Sat 9:41"))
}

fn main() {
    DESKTOP.store(std::env::args().any(|a| a == "--desktop"), Ordering::Relaxed);
    rune::run_windows(|displays| {
        let notes = App::new(AppSpec::new("Notes", (520.0, 320.0), || Notes { selected: 0, focused: true }, notes_view))
            .keep_running()
            .on_focus(|s, focused, _| s.focused = focused);
        let launcher = App::new(AppSpec::new("Launcher", (560.0, 220.0), || unreachable!(), launcher_view)).windowless();
        let (notes_handle, me) = (notes.handle(), launcher.handle());
        let launcher = launcher
            .init(move || {
                let mut query = TextEdit::new("");
                query.focused = true;
                Launcher { query, picked: 0, me, notes: notes_handle }
            })
            .global_shortcut("cmd+space", |s, _| {
                s.me.place(DisplayTarget::Primary, OverlayPlacement::Edge {
                    edge: ScreenEdge::Top, margin: 150.0, align: Align::Center, inset: 0.0,
                });
                s.me.show();
            })
            .on_focus(|_, focused, tx| if !focused { tx.close_window() });
        let mut windows: Vec<rune::AnyWindowSpec> = vec![
            WindowSpec::from_app("notes", notes).into(),
            WindowSpec::from_app("launcher", launcher).configured_options(WindowOptions::launcher_panel()).into(),
        ];
        if DESKTOP.load(Ordering::Relaxed) && let Some(d) = displays.iter().find(|d| d.primary).or(displays.first()) {
            let bar = WindowOptions {
                transparent: true, borderless: true, click_through: true, always_on_top: true,
                position: Some(d.position), physical_size: Some((d.size.0, (24.0 * d.scale).round() as u32)),
                min_size: Some((0.0, 0.0)), ..Default::default()
            };
            windows.push(WindowSpec::new("menu-bar", AppSpec::new("Menu bar", (0.0, 24.0), || (), menu_bar_view))
                .configured_options(bar.macos(MacWindowOptions::default().shadow(false))).into());
        }
        windows
    });
}
