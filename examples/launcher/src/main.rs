//! A notes app with a ⌘Space launcher panel: three windows that `--desktop` records together.
//! `cargo run -p launcher` opens the notes window; ⌘Space shows the launcher.
//!
//! Each row resolves a Linux freedesktop themed icon off-thread via `ImageCache::themed_icon`
//! and draws a monogram fallback when `Image::is_not_found()` reports no matching icon.
//!
//! ```sh
//! target/release/launcher --desktop examples/launcher/backdrop.png \
//!   --script examples/launcher/scripts/demo.json --video demo.mp4 --keys --pointer
//! ```
//!
//! With `--desktop` the app adds a fake menu bar, so the recording reads as a desktop.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::tok::{self, type_};
use std::path::PathBuf;

use rune::{
    Align, App, AppHandle, AppSpec, CloseResponse, Color, Cx, DisplayTarget, El, Image, ImageCache,
    Key, KeyEvent, MacWindowOptions, OverlayPlacement, ResizeEdge, ScreenEdge, SurfaceFit,
    TextEdit, Tx, Waker, WindowOptions, WindowSpec, col, el, row, spacer, text, text_input,
};

/// Recording with `--desktop`: there is no native chrome, so the app draws its own.
static DESKTOP: AtomicBool = AtomicBool::new(false);

struct NoteEntry {
    title: &'static str,
    file: &'static str,
    body: &'static str,
    icon_names: &'static [&'static str],
}

const NOTES: [NoteEntry; 4] = [
    NoteEntry {
        title: "Groceries",
        file: "groceries.md",
        body: "Oat milk, lemons, basil, sourdough, two avocados.",
        icon_names: &["accessories-text-editor", "text-x-generic", "text-plain"],
    },
    NoteEntry {
        title: "Trip to Lisbon",
        file: "trip-to-lisbon.md",
        body: "Book the tram tour. Pastéis de nata first.",
        icon_names: &["applications-internet", "web-browser", "text-html"],
    },
    NoteEntry {
        title: "Reading list",
        file: "reading-list.md",
        body: "The Design of Everyday Things. Thinking in Systems.",
        icon_names: &["accessories-dictionary", "x-office-document", "text-x-generic"],
    },
    NoteEntry {
        title: "Ideas",
        file: "ideas.md",
        body: "A launcher that opens notes by typing their first letters.",
        icon_names: &["dialog-information", "emblem-favorite", "text-x-generic"],
    },
];

fn note_path(index: usize) -> PathBuf {
    PathBuf::from(format!("/tmp/rune-notes/{}", NOTES[index].file))
}

struct Notes {
    selected: usize,
    focused: bool,
    represented_attached: bool,
    edited: [bool; NOTES.len()],
    close_prompt: bool,
    cache: Option<ImageCache>,
    icons: Vec<Image>,
}

impl Notes {
    fn select_note(&mut self, index: usize, tx: &mut Tx) {
        self.selected = index;
        tx.set_represented_file(self.represented_attached.then(|| note_path(index)));
        tx.set_document_edited(self.edited[index]);
    }

    fn has_unsaved(&self) -> bool {
        self.edited.iter().any(|&dirty| dirty)
    }

    fn clear_unsaved(&mut self, tx: &mut Tx) {
        self.edited = [false; NOTES.len()];
        self.close_prompt = false;
        tx.set_document_edited(false);
    }
}

fn notes_close_request(s: &mut Notes, _tx: &mut Tx) -> CloseResponse {
    if s.has_unsaved() {
        s.close_prompt = true;
        CloseResponse::Defer
    } else {
        CloseResponse::Close
    }
}

impl Notes {
    fn load_icons(&mut self, waker: Waker, wait: bool) {
        if self.cache.is_none() {
            let cache = ImageCache::new(waker);
            self.icons = NOTES.iter().map(|n| cache.themed_icon(n.icon_names, None, 18.0, 2.0)).collect();
            self.cache = Some(cache);
        }
        if wait {
            for icon in &self.icons { icon.wait().ok(); }
        }
    }
}

struct Launcher {
    query: TextEdit,
    picked: usize,
    me: AppHandle<Launcher>,
    notes: AppHandle<Notes>,
    cache: Option<ImageCache>,
    icons: Vec<Image>,
}

impl Launcher {
    fn load_icons(&mut self, waker: Waker, wait: bool) {
        if self.cache.is_none() {
            let cache = ImageCache::new(waker);
            self.icons = NOTES.iter().map(|n| cache.themed_icon(n.icon_names, None, 18.0, 2.0)).collect();
            self.cache = Some(cache);
        }
        if wait {
            for icon in &self.icons { icon.wait().ok(); }
        }
    }
}

fn matches(query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..NOTES.len()).filter(|&i| NOTES[i].title.to_lowercase().contains(&q)).collect()
}

fn item_icon(icon: Option<&Image>, title: &str, on: bool, t: &tok::Theme) -> El {
    if let Some(img) = icon.filter(|i| i.size().is_some()) {
        return el().size(18.0, 18.0).pixels_fit(img.pixels(), SurfaceFit::Fit);
    }
    let initial = title.chars().next().unwrap_or('•').to_string();
    let bg = if on { Color::WHITE.alpha(0.22) } else { t.control_off.alpha(0.75) };
    let fg = if on { Color::WHITE } else { t.text_secondary };
    let box_el = el().size(18.0, 18.0).radius(4.0).fill(bg).center()
        .child(text(initial).font(type_::LABEL).color(fg));
    if icon.is_some_and(Image::is_not_found) { box_el } else { box_el.opacity(0.6) }
}

fn notes_view(s: &Notes, _: &mut Cx<Notes>) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let entry = &NOTES[s.selected];
    let edited = s.edited[s.selected];
    let light = |c: (f32, f32, f32), dot: bool| {
        let base = el().size(12.0, 12.0).radius(6.0).center()
            .fill(if s.focused { Color { r: c.0, g: c.1, b: c.2, a: 1.0 } } else { t.control_off });
        if dot { base.child(el().size(4.0, 4.0).radius(2.0).fill(Color::BLACK.alpha(0.55))) } else { base }
    };
    let bar_title = if s.represented_attached { entry.file } else { "Notes" };
    let title_bar = row().h(38.0).px(14.0).gap(8.0).center_y()
        .child(light((1.0, 0.37, 0.34), edited)).child(light((1.0, 0.74, 0.18), false)).child(light((0.16, 0.79, 0.25), false))
        .child(spacer()).child(text(bar_title).font(type_::TITLE).color(t.text_secondary)).child(spacer())
        .child(el().w(52.0));
    let mut list = col().key("list").w(170.0).h_full().px(8.0).gap(2.0);
    for (i, entry) in NOTES.iter().enumerate() {
        let on = i == s.selected;
        let suffix = if s.edited[i] { " •" } else { "" };
        list = list.child(row().key(format!("note-{i}")).h(30.0).px(10.0).gap(8.0).radius(6.0).center_y()
            .fill(if on { accent.alpha(0.85) } else { Color::TRANSPARENT })
            .child(item_icon(s.icons.get(i), entry.title, on, &t))
            .child(text(format!("{}{suffix}", entry.title)).font(type_::BODY).color(if on { Color::WHITE } else { t.text }))
            .on_press_tx(move |s: &mut Notes, tx| s.select_note(i, tx)));
    }
    let chip = |key: &'static str, label: &'static str, active: bool| {
        row().key(key).role("button").label(label).focusable(true).h(26.0).px(10.0).radius(6.0).center_y()
            .fill(if active { accent.alpha(0.85) } else { t.control_off.alpha(0.75) })
            .child(text(label).font(type_::LABEL).color(if active { Color::WHITE } else { t.text }))
    };
    let toggle_edited = chip("toggle-edited", if edited { "Edited (mark saved)" } else { "Mark edited" }, edited)
        .on_press_tx(|s: &mut Notes, tx| {
            let next = !s.edited[s.selected];
            s.edited[s.selected] = next;
            tx.set_document_edited(next);
        });
    let toggle_file = chip(
        "toggle-file",
        if s.represented_attached { "Clear proxy file" } else { "Attach proxy file" },
        s.represented_attached,
    )
    .on_press_tx(|s: &mut Notes, tx| {
        s.represented_attached = !s.represented_attached;
        tx.set_represented_file(s.represented_attached.then(|| note_path(s.selected)));
    });
    let doc_status = if s.represented_attached {
        format!("{} · accepts_first_mouse: false", note_path(s.selected).display())
    } else {
        "No represented file · accepts_first_mouse: false".to_owned()
    };
    let page = col().key("page").grow(1.0).h_full().padding(20.0).gap(10.0).fill(t.bg)
        .child(text(entry.title).font(type_::TITLE.lh(22.0)).color(t.text))
        .child(text(entry.body).font(type_::BODY.lh(20.0)).color(t.text_secondary))
        .child(spacer())
        .child(row().gap(8.0).child(toggle_edited).child(toggle_file))
        .child(text(doc_status).font(type_::LABEL).color(t.text_secondary.alpha(0.75)))
        .child(text("⌘Space to search").font(type_::LABEL).color(t.text_secondary.alpha(0.7)));
    let mut root = col().w_full().h_full().fill(t.card)
        .child(if DESKTOP.load(Ordering::Relaxed) { title_bar } else { el().h(28.0) })
        .child(row().w_full().grow(1.0).child(list).child(page));
    if s.close_prompt {
        let unsaved = s.edited.iter().filter(|&&d| d).count();
        let prompt_body = if unsaved == 1 {
            "1 note has unsaved changes. Save before closing?".to_owned()
        } else {
            format!("{unsaved} notes have unsaved changes. Save before closing?")
        };
        let cancel = chip("close-cancel", "Cancel", false)
            .on_press(|s: &mut Notes| s.close_prompt = false);
        let discard = chip("close-discard", "Discard & Close", false)
            .on_press_tx(|s: &mut Notes, tx| {
                s.clear_unsaved(tx);
                tx.close_window();
            });
        let save = chip("close-save", "Save & Close", true)
            .on_press_tx(|s: &mut Notes, tx| {
                s.clear_unsaved(tx);
                tx.close_window();
            });
        let dialog = col()
            .key("close-confirm-dialog")
            .role("dialog")
            .label("Unsaved changes")
            .modal(true, |s: &mut Notes| s.close_prompt = false)
            .w(340.0)
            .padding(18.0)
            .gap(12.0)
            .radius(10.0)
            .fill(t.card)
            .border(1.0, Color::WHITE.alpha(0.14))
            .child(text("Unsaved changes").font(type_::TITLE).color(t.text))
            .child(text(prompt_body).font(type_::BODY).color(t.text_secondary))
            .child(row().gap(8.0).child(spacer()).child(cancel).child(discard).child(save));
        let scrim = el()
            .key("close-confirm-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(Color::BLACK.alpha(0.45))
            .center()
            .child(dialog);
        root = root.child(scrim);
    }
    root
}

fn launcher_view(s: &Launcher, _: &mut Cx<Launcher>) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let found = matches(&s.query.text);
    let field = text_input(&s.query, type_::BODY.lh(26.0)).key("query").label("Search notes").single_line().grow(1.0)
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
    let mut panel = col().key("panel").w_full().grow(1.0).radius(14.0).clip_rounded(14.0)
        .fill(t.card).border(1.0, Color::WHITE.alpha(0.1))
        .child(row().h(56.0).px(18.0).gap(12.0).center_y().window_drag()
            .child(text("⌕").font(type_::TITLE.lh(26.0)).color(t.text_secondary))
            .child(field));
    if !found.is_empty() {
        let mut results = col().key("results").padding(6.0).gap(2.0);
        for (row_i, &i) in found.iter().enumerate() {
            let on = row_i == s.picked;
            results = results.child(row().key(format!("result-{i}")).h(36.0).px(12.0).gap(10.0).radius(8.0).center_y()
                .fill(if on { accent.alpha(0.85) } else { Color::TRANSPARENT })
                .child(item_icon(s.icons.get(i), NOTES[i].title, on, &t))
                .child(text(NOTES[i].title).font(type_::BODY).color(if on { Color::WHITE } else { t.text })));
        }
        panel = panel.child(el().h(1.0).w_full().fill(t.separator)).child(results);
    }
    // 8-way interactive resize strips around the borderless launcher window.
    const EDGE: f32 = 6.0;
    const CORNER: f32 = 12.0;
    let corner_grip = el().key("resize-se").pin(None, None, Some(0.0), Some(0.0)).size(CORNER, CORNER)
        .window_resize(ResizeEdge::SouthEast)
        .child(el().pin(None, None, Some(4.0), Some(4.0)).size(5.0, 5.0).radius(2.5).fill(Color::WHITE.alpha(0.22)));
    col().w_full().h_full()
        .child(panel)
        .child(el().key("resize-n").pin(Some(CORNER), Some(0.0), Some(CORNER), None).h(EDGE).window_resize(ResizeEdge::North))
        .child(el().key("resize-s").pin(Some(CORNER), None, Some(CORNER), Some(0.0)).h(EDGE).window_resize(ResizeEdge::South))
        .child(el().key("resize-w").pin(Some(0.0), Some(CORNER), None, Some(CORNER)).w(EDGE).window_resize(ResizeEdge::West))
        .child(el().key("resize-e").pin(None, Some(CORNER), Some(0.0), Some(CORNER)).w(EDGE).window_resize(ResizeEdge::East))
        .child(el().key("resize-nw").pin(Some(0.0), Some(0.0), None, None).size(CORNER, CORNER).window_resize(ResizeEdge::NorthWest))
        .child(el().key("resize-ne").pin(None, Some(0.0), Some(0.0), None).size(CORNER, CORNER).window_resize(ResizeEdge::NorthEast))
        .child(el().key("resize-sw").pin(Some(0.0), None, None, Some(0.0)).size(CORNER, CORNER).window_resize(ResizeEdge::SouthWest))
        .child(corner_grip)
}

fn open_picked(s: &mut Launcher, tx: &mut Tx) {
    let Some(&i) = matches(&s.query.text).get(s.picked) else { return };
    s.notes.update(move |n, ntx| n.select_note(i, ntx));
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
        let notes_spec = AppSpec::<Notes> {
            setup: Some(|s, w| s.load_icons(w, false)),
            scenes: &[
                ("themed-icons", |s| s.load_icons(Arc::new(|| {}), true)),
                ("document-edited", |s| {
                    s.load_icons(Arc::new(|| {}), true);
                    s.edited[0] = true;
                }),
                ("unsaved-close", |s| {
                    s.load_icons(Arc::new(|| {}), true);
                    s.edited[0] = true;
                    s.close_prompt = true;
                }),
            ],
            ..AppSpec::new(
                "Notes",
                (560.0, 340.0),
                || Notes {
                    selected: 0,
                    focused: true,
                    represented_attached: true,
                    edited: [false; NOTES.len()],
                    close_prompt: false,
                    cache: None,
                    icons: Vec::new(),
                },
                notes_view,
            )
        };
        let notes = App::new(notes_spec)
            .keep_running()
            .macos_window(
                MacWindowOptions::default()
                    .represented_file(note_path(0))
                    .title_from_file(true)
                    .show_title(true)
                    .accepts_first_mouse(false),
            )
            .on_close_request(notes_close_request)
            .on_focus(|s, focused, _| s.focused = focused);
        let launcher_spec = AppSpec::<Launcher> {
            setup: Some(|s, w| s.load_icons(w, false)),
            scenes: &[("themed-icons", |s| s.load_icons(Arc::new(|| {}), true))],
            ..AppSpec::new("Launcher", (560.0, 220.0), || unreachable!(), launcher_view)
        };
        let launcher = App::new(launcher_spec).windowless();
        let (notes_handle, me) = (notes.handle(), launcher.handle());
        let launcher = launcher
            .init(move || {
                let mut query = TextEdit::new("");
                query.focused = true;
                Launcher { query, picked: 0, me, notes: notes_handle, cache: None, icons: Vec::new() }
            })
            .global_shortcut("cmd+space", |s, _| {
                s.me.place(DisplayTarget::Primary, OverlayPlacement::Edge {
                    edge: ScreenEdge::Top, margin: 150.0, align: Align::Center, inset: 0.0,
                });
                s.me.show();
            })
            .on_focus(|_, focused, tx| if !focused { tx.close_window() });
        let launcher_options = if cfg!(target_os = "macos") {
            WindowOptions::launcher_panel()
        } else {
            WindowOptions {
                visible: false,
                borderless: true,
                transparent: true,
                resizable: true,
                min_size: Some((360.0, 140.0)),
                ..Default::default()
            }.into()
        };
        let mut windows: Vec<rune::AnyWindowSpec> = vec![
            WindowSpec::from_app("notes", notes).into(),
            WindowSpec::from_app("launcher", launcher).configured_options(launcher_options).into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn notes_view_renders_themed_icon_or_not_found_fallback_cleanly() {
        let mut state = Notes {
            selected: 0,
            focused: true,
            represented_attached: true,
            edited: [false; NOTES.len()],
            close_prompt: false,
            cache: None,
            icons: Vec::new(),
        };
        state.load_icons(Arc::new(|| {}), true);
        assert_eq!(state.icons.len(), NOTES.len());
        for icon in &state.icons {
            assert!(icon.is_ready());
            assert!(icon.size().is_some() || icon.is_not_found());
        }
        let mut rt = Runtime::new(state, notes_view, (560.0, 340.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        assert!(rt.find("note-0").is_some());
        assert!(rt.find("note-3").is_some());
        assert!(rt.find("toggle-edited").is_some());
        assert!(rt.find("toggle-file").is_some());
    }

    #[test]
    fn notes_document_controls_toggle_edited_and_represented_file() {
        let state = Notes {
            selected: 0,
            focused: true,
            represented_attached: true,
            edited: [false; NOTES.len()],
            close_prompt: false,
            cache: None,
            icons: Vec::new(),
        };
        let mut rt = Runtime::new(state, notes_view, (560.0, 340.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        assert!(!rt.state.edited[0]);
        assert!(rt.state.represented_attached);

        let (x, y) = rt.find("toggle-edited").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.1);
        assert!(rt.state.edited[0]);

        let (fx, fy) = rt.find("toggle-file").unwrap();
        rt.pointer_down(fx, fy);
        rt.pointer_up(fx, fy);
        rt.frame(0.2);
        assert!(!rt.state.represented_attached);
    }

    #[test]
    fn notes_close_request_defers_when_edited_and_dialog_cancels_or_closes() {
        let state = Notes {
            selected: 0,
            focused: true,
            represented_attached: true,
            edited: [false; NOTES.len()],
            close_prompt: false,
            cache: None,
            icons: Vec::new(),
        };
        let mut rt = Runtime::new(state, notes_view, (560.0, 340.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);

        // Clean notes allow immediate close without opening the confirmation dialog.
        let mut decision = CloseResponse::Defer;
        rt.update(|s, tx| decision = notes_close_request(s, tx));
        assert_eq!(decision, CloseResponse::Close);
        assert!(!rt.state.close_prompt);

        // Mark a note edited, then trigger close request -> defers and displays modal dialog.
        rt.update(|s, tx| {
            s.edited[0] = true;
            tx.set_document_edited(true);
            decision = notes_close_request(s, tx);
        });
        assert_eq!(decision, CloseResponse::Defer);
        assert!(rt.state.close_prompt);
        rt.frame(0.1);
        assert!(rt.find("close-confirm-dialog").is_some());

        // Accessibility tree exposes the dialog and its action buttons.
        let tree = rt.accessibility_tree();
        assert!(tree.nodes.iter().any(|(_, n)| n.role() == rune::accesskit::Role::Dialog && n.label() == Some("Unsaved changes")));
        for expected in ["Cancel", "Discard & Close", "Save & Close"] {
            assert!(
                tree.nodes.iter().any(|(_, n)| n.role() == rune::accesskit::Role::Button && n.label() == Some(expected)),
                "missing dialog button {expected}",
            );
        }

        // Cancel dismisses the dialog and keeps the window open without requesting close.
        let (cx, cy) = rt.find("close-cancel").unwrap();
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
        rt.frame(0.2);
        assert!(!rt.state.close_prompt);
        assert!(rt.find("close-confirm-dialog").is_none());
        assert_eq!(rt.shell_request, None);

        // Trigger again and confirm Save & Close -> clears unsaved state and requests programmatic close.
        rt.update(|s, tx| decision = notes_close_request(s, tx));
        assert_eq!(decision, CloseResponse::Defer);
        rt.frame(0.3);
        let (sx, sy) = rt.find("close-save").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(0.4);
        assert!(!rt.state.close_prompt);
        assert!(!rt.state.has_unsaved());
        assert_eq!(rt.shell_request, Some(rune::Shell::Close));
    }
}
