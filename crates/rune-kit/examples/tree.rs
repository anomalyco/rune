//! A repository's changed files as a tree, with a filter field and line counts, beside the
//! file the tree has selected. `cargo run -p rune-kit --example tree` opens the window;
//! `-- --big` loads 50,000 files instead. The stills in `demo/kit/tree-{dark,light}.png`:
//! `-- --snap tree.png --theme dark --size 720x480 --scale 2`.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::{AppSpec, Cx, El, TextEdit, col, row, text};
use rune_kit::{FileNode, FileTree, Kit, TextInput, Tree, TreeItem, TreeModel};

#[derive(Clone, Copy)]
struct Change {
    added: u32,
    removed: u32,
    status: Status,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Added,
    Modified,
    Deleted,
}

struct State {
    files: TreeModel<FileNode<Change>>,
    query: TextEdit,
}

const REPO: &[(&str, u32, u32, Status)] = &[
    ("crates/rune/src/el.rs", 48, 0, Status::Modified),
    ("crates/rune/src/focus.rs", 2, 1, Status::Modified),
    ("crates/rune/src/runtime.rs", 43, 2, Status::Modified),
    ("crates/rune/src/tooling.rs", 9, 0, Status::Modified),
    ("crates/rune/src/on_key_tests.rs", 212, 0, Status::Added),
    ("crates/rune-kit/src/lib.rs", 4, 0, Status::Modified),
    ("crates/rune-kit/src/glyphs.rs", 44, 0, Status::Modified),
    ("crates/rune-kit/src/tree.rs", 1140, 0, Status::Added),
    ("crates/rune-kit/src/table.rs", 780, 0, Status::Added),
    ("crates/rune-kit/src/list_old.rs", 0, 96, Status::Deleted),
    ("crates/rune-kit/examples/tree.rs", 120, 0, Status::Added),
    ("crates/rune-kit/examples/table.rs", 110, 0, Status::Added),
    ("crates/rune-kit/tests/tree.rs", 290, 0, Status::Added),
    ("crates/rune-kit/tests/table.rs", 230, 0, Status::Added),
    ("crates/rune-kit/tests/glyphs.rs", 7, 1, Status::Modified),
    ("demo/kit/tree-dark.png", 0, 0, Status::Added),
    ("demo/kit/tree-light.png", 0, 0, Status::Added),
    ("site/src/content/docs/kit/tree.md", 64, 0, Status::Added),
    ("CHANGELOG.md", 12, 0, Status::Modified),
    ("README.md", 18, 3, Status::Modified),
];

fn state() -> State {
    let big = std::env::args().any(|a| a == "--big");
    let mut files = if big {
        let paths: Vec<String> = (0..500).flat_map(|d| (0..100).map(move |f| format!("pkg{:03}/src/module{f}.rs", d))).collect();
        FileTree::new().build(paths.into_iter().enumerate().map(|(i, p)| (p, Change { added: (i % 37) as u32, removed: (i % 11) as u32, status: Status::Modified })))
    } else {
        FileTree::new().build(REPO.iter().map(|&(p, added, removed, status)| (p, Change { added, removed, status })))
    };
    files.expand_all();
    if !big {
        files.set_expanded("demo/kit", false);
        files.select("crates/rune-kit/src/tree.rs");
    }
    State { files, query: TextEdit::new("") }
}

fn label(n: &FileNode<Change>) -> &str {
    &n.name
}

/// A folder's total change counts.
fn totals(m: &TreeModel<FileNode<Change>>, id: &str) -> (u32, u32) {
    m.subtree(id).filter_map(|n| n.data).fold((0, 0), |(a, r), c| (a + c.added, r + c.removed))
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let c = k.c.clone();
    let row_query = s.query.text.trim().to_lowercase();
    let status_font = k.ty.text_xsmall_medium;
    let tree = Tree::new(|s: &State| &s.files, |s: &mut State| &mut s.files, label)
        .name("Changed files")
        .row(move |n, r| {
            let mut item = TreeItem::file(n, r).highlight(row_query.clone());
            match n.data {
                Some(ch) => {
                    let (color, letter) = match ch.status {
                        Status::Added => (c.success.icon, "A"),
                        Status::Modified => (c.warning.icon, "M"),
                        Status::Deleted => (c.danger.icon, "D"),
                    };
                    if ch.status == Status::Deleted {
                        item = item.label_color(c.text_faint);
                    }
                    item.changes(ch.added, ch.removed).trailing(text(letter).key("status").font(status_font).color(color).w(10.0))
                }
                None => item,
            }
        })
        .build(s, &k)
        .grow(1.0)
        .min_h(0.0);
    let filter = TextInput::new(&s.query)
        .placeholder("Filter files")
        .leading_icon(k.glyph.search)
        .clear_button()
        .on_edit(|s: &mut State| &mut s.query)
        .on_change(|s: &mut State, _| {
            let q = s.query.text.trim().to_lowercase();
            if q.is_empty() {
                s.files.clear_filter();
            } else {
                s.files.filter(|n| n.name.to_lowercase().contains(&q));
            }
        })
        .build(&k);
    let sidebar = col()
        .key("sidebar")
        .w(300.0)
        .h_full()
        .padding(8.0)
        .gap(8.0)
        .fill(k.c.bg_base)
        .child(filter)
        .child(tree);
    let selected = s.files.cursor().map(str::to_owned);
    let detail = match selected.as_deref() {
        Some(id) => {
            let (a, r) = totals(&s.files, id);
            let n = s.files.get(id).unwrap();
            col()
                .gap(6.0)
                .child(text(n.path.clone()).font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(format!("{} · +{a} \u{2212}{r}", if n.dir { "Folder" } else { "File" })).font(k.ty.text_small).color(k.c.text_muted))
        }
        None => col().child(text("Select a file").font(k.ty.ui).color(k.c.text_muted)),
    };
    row()
        .w_full()
        .h_full()
        .fill(k.c.page)
        .child(sidebar)
        .child(col().key("detail").grow(1.0).h_full().padding(20.0).child(detail))
}

fn main() {
    rune::run(AppSpec::new("Tree", (720.0, 480.0), state, view));
}
