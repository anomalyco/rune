//! The rune-kit gallery: one scene per `@opencode/ui` Storybook story, in the story's id, so
//! `scripts/kit-diff.py` can render both and diff them. `--kit neutral` renders the same
//! components on Rune's core neutral theme instead of OpenCode's; `--kit palette` on
//! `Kit::from_palette` with a near-black, off-white and red seed (a dark palette with a red accent);
//! `--kit palette black` (dark) and `--kit palette warm` (light) use pure-black and
//! paper seeds with ink focus rings. `--reduced-motion` renders in the runtime's reduced-motion
//! mode.
//!
//! ```sh
//! kit-gallery --list-scenes
//! kit-gallery --scene ui-button--variants --snap out.png --theme dark --size 720x300
//! ```

#![forbid(unsafe_code)]

mod stories;

#[cfg(test)]
mod gallery_tests;
#[cfg(test)]
mod layer_timing;

use std::collections::{HashMap, HashSet};

use rune::{AppSpec, Cx, El, TextEdit, col};
use rune_kit::Kit;

pub struct G {
    pub story: &'static str,
    /// Toggled things by key (switches, checkboxes, expanded items).
    pub on: HashSet<String>,
    /// Picked values by group key (radio groups, selects, segmented controls, tabs).
    pub pick: HashMap<String, String>,
    /// Numeric values by key (sliders).
    pub values: HashMap<String, f32>,
    /// The open overlay, by key.
    pub open: Option<String>,
    /// Context menu anchor from a story's `on_secondary_press`.
    pub menu_at: Option<rune::PointerPress>,
    /// Link popover state (`url`, `[left, top, width, height]`) from `Tx::link_rect`.
    pub link_popover: Option<(String, [f32; 4])>,
    /// Link hover preview state (`url`, `[left, top, width, height]`) from `Markdown::on_link_hover`.
    pub link_hover: Option<(String, [f32; 4])>,
    pub edits: [TextEdit; 12],
    pub read_only_edits: [TextEdit; 2],
    pub nowrap_code: TextEdit,
    pub align_y_edits: [TextEdit; 4],
    pub key_preview_edit: TextEdit,
    pub field_menu_edit: TextEdit,
    pub field_menu: Option<rune::FieldMenuRequest>,
    pub field_menu_last: Option<String>,
    pub focus_caret_edit: TextEdit,
    pub aria_combo_edit: TextEdit,
    pub combo: rune_kit::ComboboxState,
    pub combo_animated: rune_kit::ComboboxState,
    pub combo_localized: rune_kit::ComboboxState,
    /// The sidebar reorder story's state.
    pub reorder: stories::sidebar::ReorderDemo,
    /// The file tree story's files.
    pub files: rune_kit::TreeModel<rune_kit::FileNode<()>>,
    /// The scrollable tree story's files.
    pub scroll_files: rune_kit::TreeModel<rune_kit::FileNode<()>>,
    /// The lazy-branches tree story's files and loaded folder ids.
    pub lazy_files: rune_kit::TreeModel<rune_kit::FileNode<()>>,
    pub lazy_loaded: HashSet<String>,
    /// Open tree context menu: (story key, row id, anchor point).
    pub tree_menu: Option<(&'static str, String, rune::PointerPress)>,
    /// Last context-menu action picked in the tree story.
    pub tree_action: Option<String>,
    /// The split-view story's sidebar and inspector panes.
    pub split_sidebar: rune_kit::PaneState,
    pub split_inspector: rune_kit::PaneState,
    /// The table options story's state, row count and open row context menu.
    pub table: rune_kit::TableState,
    pub table_rows: usize,
    pub table_menu: Option<(&'static str, String, rune::PointerPress)>,
    /// The diff stories' state and most recent comment anchor.
    pub diff: rune_kit::diff::DiffState,
    pub diff_custom: rune_kit::diff::DiffState,
    pub diff_scroll: rune_kit::diff::DiffState,
    pub diff_comment: Option<rune_kit::diff::LineRange>,
    /// The image decode story's cache and sample handles.
    pub images: stories::images::ImageDemo,
    pub neutral: bool,
    pub palette: bool,
    /// `--reduced-motion`: the runtime's reduced-motion mode.
    pub reduced: bool,
}

impl G {
    pub fn is(&self, key: &str) -> bool {
        self.on.contains(key)
    }
    pub fn flip(&mut self, key: &str) {
        if !self.on.remove(key) {
            self.on.insert(key.to_owned());
        }
    }
    pub fn picked<'a>(&'a self, group: &str, default: &'a str) -> &'a str {
        self.pick.get(group).map_or(default, String::as_str)
    }
    pub fn value(&self, key: &str, default: f32) -> f32 {
        self.values.get(key).copied().unwrap_or(default)
    }
    pub fn set_value(&mut self, key: &str, v: f32) {
        self.values.insert(key.to_owned(), v);
    }
    pub fn toggle_open(&mut self, key: &str) {
        self.open = if self.open.as_deref() == Some(key) { None } else { Some(key.to_owned()) };
    }
}

fn init() -> G {
    G {
        story: "index",
        on: HashSet::new(),
        pick: HashMap::new(),
        values: HashMap::new(),
        open: None,
        menu_at: None,
        link_popover: None,
        link_hover: None,
        edits: Default::default(),
        read_only_edits: [
            TextEdit::new("https://api.example.com/v1/workspaces/rune"),
            TextEdit::new("fn preview(input: &str) -> usize {
    // Move with arrows, Shift+arrows, Home/End, or word steps.
    input.lines().count()
}"),
        ],
        nowrap_code: TextEdit::new(stories::NOWRAP_CODE),
        align_y_edits: [
            TextEdit::new("Vertically centred single-line text"),
            TextEdit::new(""),
            TextEdit::new("Bottom-aligned (Align::End)"),
            TextEdit::new("Default top-aligned (Align::Start)"),
        ],
        key_preview_edit: TextEdit::new(""),
        field_menu_edit: TextEdit::new("Select text, right-click or press Shift+F10"),
        field_menu: None,
        field_menu_last: None,
        focus_caret_edit: TextEdit::new(stories::focus_caret_sample()),
        aria_combo_edit: TextEdit::new("Ap"),
        combo: Default::default(),
        combo_animated: Default::default(),
        combo_localized: Default::default(),
        reorder: Default::default(),
        files: files(),
        scroll_files: scroll_files(),
        lazy_files: lazy_files(),
        lazy_loaded: HashSet::new(),
        tree_menu: None,
        tree_action: None,
        split_sidebar: rune_kit::PaneState::new(180.0).range(120.0, 280.0),
        split_inspector: rune_kit::PaneState::new(170.0).range(120.0, 260.0),
        table: {
            let mut t = rune_kit::TableState::new().key("log-table").multi_select(true);
            t.select("log-15");
            t
        },
        table_rows: 16,
        table_menu: None,
        diff: stories::sample_diff(),
        diff_custom: stories::custom_diff(),
        diff_scroll: stories::scroll_bars_diff(),
        diff_comment: None,
        images: stories::images::ImageDemo::default(),
        neutral: rune::extra_args().iter().any(|a| a == "neutral"),
        palette: rune::extra_args().iter().any(|a| a == "palette"),
        reduced: rune::extra_args().iter().any(|a| a == "--reduced-motion"),
    }
}

/// The acme-web checkout Desktop's Files panel shows, `src/util` open on `format.ts`.
fn files() -> rune_kit::TreeModel<rune_kit::FileNode<()>> {
    let paths = [".git/HEAD", "src/util/format.ts", "src/money.ts", "src/index.ts", "package.json", "README.md"];
    let mut m = rune_kit::FileTree::new().compact_folders(false).build(paths.map(|p| (p, ())));
    m.expand_to("src/util/format.ts");
    m.select("src/util/format.ts");
    m
}

/// A deeper checkout that overflows its viewport, for the scrollable tree and context-menu story.
fn scroll_files() -> rune_kit::TreeModel<rune_kit::FileNode<()>> {
    let paths = [
        "crates/rune/src/el.rs",
        "crates/rune/src/focus.rs",
        "crates/rune/src/runtime.rs",
        "crates/rune-kit/src/scroll_bar.rs",
        "crates/rune-kit/src/tree.rs",
        "crates/rune-kit/src/tree/meta.rs",
        "crates/rune-kit/src/tree/view.rs",
        "crates/rune-kit/src/overlays/menu.rs",
        "crates/rune-kit/src/overlays/popover.rs",
        "examples/kit-gallery/src/main.rs",
        "examples/kit-gallery/src/stories.rs",
        "themes/opencode/src/icons.rs",
        "themes/opencode/src/tree_icons.rs",
        "Cargo.toml",
        "CHANGELOG.md",
        "README.md",
    ];
    let mut m = rune_kit::FileTree::new().key("scroll-files").build(paths.map(|p| (p, ())));
    m.expand_all();
    m.select("crates/rune-kit/src/tree/view.rs");
    m
}

/// A file tree with unloaded and empty folder branches for `rune-tree--lazy-branches`.
pub(crate) fn lazy_files() -> rune_kit::TreeModel<rune_kit::FileNode<()>> {
    use rune_kit::{FileNode, TreeModel};
    let dir = |name: &str, path: &str| FileNode { name: name.into(), path: path.into(), dir: true, data: None };
    let file = |name: &str, path: &str| FileNode { name: name.into(), path: path.into(), dir: false, data: Some(()) };
    let mut m = TreeModel::new().key("lazy-files");
    m.insert_branch(None, "remote", dir("remote", "remote"));
    m.insert_branch(None, "empty", dir("empty", "empty"));
    m.insert_branch(None, "src", dir("src", "src"));
    m.insert(Some("src"), "src/main.rs", file("main.rs", "src/main.rs"));
    m.insert(None, "README.md", file("README.md", "README.md"));
    m.set_expanded("src", true);
    m.select("remote");
    m
}

pub fn kit(s: &G, dark: bool) -> &'static Kit {
    if s.palette {
        static D: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        static L: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        let hex = |c: &str| rune::color::hex(c).unwrap();
        let seed = |bg: &str, fg: &str, accent: &str| rune_kit::PaletteSeed { bg: hex(bg), fg: hex(fg), accent: hex(accent) };
        // `--kit palette black|warm`: pure-black and paper seeds with ink focus.
        let variant = |name: &str| rune::extra_args().iter().any(|a| a == name);
        let ink = |seed| Kit::from_palette_focus(seed, rune_kit::PaletteFocus::Ink);
        return if dark {
            D.get_or_init(|| match variant("black") {
                true => ink(seed("#000000", "#ffffff", "#ff2a2a")),
                false => Kit::from_palette(seed("#0a0a0a", "#ededed", "#e5484d")),
            })
        } else {
            L.get_or_init(|| match variant("warm") {
                true => ink(seed("#e8e4dc", "#000000", "#d81414")),
                false => Kit::from_palette(seed("#ffffff", "#171717", "#e5484d")),
            })
        };
    }
    if s.neutral {
        static D: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        static L: std::sync::OnceLock<Kit> = std::sync::OnceLock::new();
        return if dark { D.get_or_init(|| Kit::neutral(true)) } else { L.get_or_init(|| Kit::neutral(false)) };
    }
    rune_theme_opencode::kit(dark)
}

fn view(s: &G, cx: &mut Cx<G>) -> El {
    if s.reduced {
        cx.reduced_motion(true);
    }
    let k = kit(s, cx.system_dark());
    if s.story == "rune-tooltip--help-arrow" {
        cx.help_look(
            rune_kit::TooltipStyle::from_kit(k, rune_kit::TooltipAppearance::Compact)
                .help_look()
                .side(rune::Side::Top)
                .arrow(5.0),
        );
    }
    let mut overlays = Vec::new();
    let body = stories::render(s, k, &mut overlays);
    stories::field_menu::mount_live_field_menu(s, k, &mut overlays);
    let frame = if s.story == "index" {
        col().key("frame").w_full().h_full().fill(k.c.page).child(body)
    } else {
        // Storybook's decorator: the page colour and 24 px of padding around the story.
        col().key("frame").fill(k.c.page).padding(24.0).child(body)
    };
    rune_kit::root(frame, overlays)
}

fn build_app() -> rune::App<G> {
    rune::App::new(AppSpec {
        scenes: stories::SCENES,
        setup: Some(|s, waker| s.images.set_waker(waker)),
        poll: Some(|s, _, _| {
            if s.story == "index" && !s.images.is_loaded() {
                s.images.enter(false);
            }
            false
        }),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        ..AppSpec::new("rune-kit gallery", (720.0, 480.0), init, view)
    })
    .on_field_menu(|s: &mut G, req, _| {
        s.field_menu = Some(req);
        s.on.remove("fm-preview");
    })
}

fn main() {
    build_app().run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every story builds on both appearances of both themes (no GPU needed).
    #[test]
    fn every_story_builds() {
        for (neutral, palette) in [(false, false), (true, false), (false, true)] {
            for dark in [false, true] {
                let mut s = init();
                s.neutral = neutral;
                s.palette = palette;
                for (id, setup) in stories::SCENES {
                    setup(&mut s);
                    let k = kit(&s, dark);
                    let mut layer = Vec::new();
                    let _ = stories::render(&s, k, &mut layer);
                    assert_eq!(s.story, *id);
                }
            }
        }
    }

    /// Drives the `rune-menu--field-menu-fallback` story headless: opens the field menu,
    /// chooses Cut and Paste (through the modal `rune_kit::Menu` on platforms without native
    /// pop-ups, or `resolve_popup` on platforms with native pop-ups), and checks the edit.
    #[test]
    fn field_menu_fallback_story_drives_cut_and_paste() {
        let board = rune::clipboard::memory();
        let mut rt = build_app().headless_runtime((720.0, 480.0), rune::Env::new(true, 1.0));
        rt.use_clipboard(&board);
        let (_, setup) = stories::SCENES
            .iter()
            .find(|(id, _)| *id == "rune-menu--field-menu-fallback")
            .expect("field-menu-fallback scene");
        rt.update(|s, _| {
            setup(s);
            s.on.remove("fm-preview");
            s.field_menu_edit.focused = true;
            s.field_menu_edit.set_selection(0, 11);
        });
        rt.frame(0.0);

        rt.key(rune::KeyEvent::from(rune::Key::ContextMenu));
        rt.frame(0.0);
        if rune::popup_menu_supported() {
            assert!(rt.popup_menu().is_some());
            rt.resolve_popup(Some("cut")).unwrap();
            rt.frame(0.0);
            assert_eq!(board.content().as_ref().map(|c| c.text()), Some("Select text"));
            assert_eq!(rt.state.field_menu_edit.text, ", right-click or press Shift+F10");
        } else {
            assert!(rt.state.field_menu.is_some(), "on_field_menu mounts modal rune_kit::Menu");
            let (x, y) = rt.find("fm-cut").expect("fm-cut item in modal Menu");
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(0.0);
            assert!(rt.state.field_menu.is_none());
            assert_eq!(rt.state.field_menu_last.as_deref(), Some("cut"));
            assert!(rt.state.field_menu_edit.focused);
            assert_eq!(board.content().as_ref().map(|c| c.text()), Some("Select text"));
            assert_eq!(rt.state.field_menu_edit.text, ", right-click or press Shift+F10");

            rt.key(rune::KeyEvent::from(rune::Key::ContextMenu));
            rt.frame(0.0);
            let (px, py) = rt.find("fm-paste").expect("fm-paste item in modal Menu");
            rt.pointer_down(px, py);
            rt.pointer_up(px, py);
            rt.frame(0.0);
            assert_eq!(rt.state.field_menu_edit.text, "Select text, right-click or press Shift+F10");
        }
    }
}
