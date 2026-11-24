//! FileIcon and Icon, as `file-icon.stories.tsx` and `icon.stories.tsx` lay them out, and Rune's own
//! catalogs of every OpenCode icon, file-type badge and file tree icon, and its file tree.

use rune::icon;
use rune_theme_opencode::{file_icons, tree_icons};

use super::*;

/// `display: grid; grid-template-columns: repeat(auto-fill, minmax(min, 1fr)); gap` at the
/// story's 672-point width.
fn grid(min: f32, gap: f32, cells: Vec<El>) -> El {
    let columns = ((672.0 + gap) / (min + gap)).floor().max(1.0) as usize;
    let width = (672.0 - gap * (columns - 1) as f32) / columns as f32;
    let mut rows = col().gap(gap);
    let mut cells = cells.into_iter().peekable();
    while cells.peek().is_some() {
        rows = rows.child(row().gap(gap).children(cells.by_ref().take(columns).map(|c| c.w(width))));
    }
    rows
}

/// The stories' `font-size: 12px; color: var(--text-weak)` names.
fn name(k: &Kit, label: &str) -> El {
    text(label).font(TextStyle::new(12.0, 440.0).lh(18.0)).color(k.c.text_weak).truncate()
}

pub(super) fn file_icon_samples(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let items = [
        ("README.md", false),
        ("package.json", false),
        ("tsconfig.json", false),
        ("index.ts", false),
        ("styles.css", false),
        ("logo.svg", false),
        ("photo.png", false),
        ("Dockerfile", false),
        (".env", false),
        ("src", true),
        ("public", true),
    ];
    let cells = items
        .iter()
        .map(|&(path, dir)| {
            let glyph = if dir { file_icons::folder(path, false) } else { file_icons::file(path) };
            row().gap(8.0).center_y().child(icon(glyph, 16.0).color(k.c.text_base)).child(name(k, path))
        })
        .collect();
    grid(120.0, 12.0, cells)
}

pub(super) fn file_icon_folder(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(icon(file_icons::folder("src", true), 16.0).color(k.c.text_base))
}

pub(super) fn icon_gallery(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let names = [
        "archive", "arrow-left", "arrow-right", "arrow-up-right", "branch", "check", "chevron-down", "close", "code-slash", "edit",
        "folder", "graduation-cap", "help", "magnifying-glass", "menu", "monitor", "outline-arrow-to-corner-top-right",
        "outline-copy", "outline-dots", "outline-eye", "outline-eye-slash", "outline-hexagonal-warning", "plus", "review",
        "settings-gear", "trash", "workspace",
    ];
    // A name the catalog lacks draws `plus`, as `Icon` does.
    let find = |n: &str| ic::ALL.iter().copied().find(|i| i.name == n).unwrap_or(&ic::PLUS);
    let cells = names
        .iter()
        .map(|n| {
            col().gap(6.0).items_center().child(icon(find(n), 16.0).color(k.c.icon_base)).child(
                text(*n).font(TextStyle::new(10.0, 440.0).lh(15.0)).color(k.c.text_weak).truncate(),
            )
        })
        .collect();
    grid((672.0 - 48.0) / 5.0, 12.0, cells)
}

/// Every icon in OpenCode's catalog, by name.
pub(super) fn icon_catalog(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let cells = ic::ALL
        .iter()
        .map(|i| row().gap(8.0).center_y().child(icon(i, 16.0).color(k.c.icon_base)).child(name(k, i.name)))
        .collect();
    grid(128.0, 8.0, cells)
}

/// Every FileIcon badge and folder icon.
pub(super) fn file_icon_catalog(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let cells = file_icons::all().iter().map(|i| row().gap(8.0).center_y().child(icon(i, 16.0).color(k.c.text_base)).child(name(k, i.name))).collect();
    grid(128.0, 8.0, cells)
}

/// Every file tree icon, at rest in `icon-muted` and in its type's colour as a selected row draws it.
pub(super) fn tree_icon_catalog(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let muted = rune_theme_opencode::theme(k.dark).color("v2-icon-icon-muted");
    let cells = tree_icons::all()
        .map(|t| {
            row()
                .gap(6.0)
                .center_y()
                .child(icon(t.icon(), 16.0).color(muted))
                .child(icon(t.icon(), 16.0).color(t.color(k.dark)))
                .child(name(k, t.token()))
        })
        .collect();
    grid(128.0, 8.0, cells)
}

/// Desktop's file tree: folders show only a chevron, files their type's icon, coloured on the
/// selected row.
pub(super) fn file_tree(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let dark = k.dark;
    let tree = Tree::new(|s: &G| &s.files, |s: &mut G| &mut s.files, FileNode::label)
        .row(move |n, r| tree_icons::item(&n.name, &n.path, n.dir, r.selected, dark))
        .style(tree_icons::style(k))
        .name("Files")
        .scroll_bar(true)
        .on_menu(|s: &mut G, id, p, _| s.tree_menu = Some(("files", id.to_owned(), p)))
        .build(s, k)
        .h(8.0 * 28.0 + 8.0);
    let panel = col().w(240.0).fill(k.c.bg_base).pt(4.0).px(8.0).child(tree);
    let menu = s.tree_menu.as_ref().filter(|(which, _, _)| *which == "files");
    let target = menu.map(|(_, id, _)| id.clone()).unwrap_or_default();
    let copy_id = target.clone();
    let reveal_id = target.clone();
    // The tree's scroller: 4 points above the rows and 8 either side (`TREE_UNSAFE_CSS`).
    context_menu("tree-files-menu", panel, menu.map(|(_, _, p)| *p))
        .item(MenuItem::new("Copy path").on_select_tx(move |s: &mut G, tx: &mut Tx| {
            tx.copy(&copy_id);
            s.tree_action = Some(format!("Copied {copy_id}"));
            s.tree_menu = None;
        }))
        .item(MenuItem::new("Select row").on_select(move |s: &mut G| {
            s.files.select(&reveal_id);
            s.tree_action = Some(format!("Selected {reveal_id}"));
            s.tree_menu = None;
        }))
        .on_close(|s: &mut G| s.tree_menu = None)
        .build(k, layer)
}

/// A scrollable file tree with a kit scroll bar and a row context menu (right-click, Menu key
/// or Shift+F10).
pub(super) fn file_tree_scroll_menu(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let dark = k.dark;
    let tree = Tree::new(|s: &G| &s.scroll_files, |s: &mut G| &mut s.scroll_files, FileNode::label)
        .row(move |n, r| tree_icons::item(&n.name, &n.path, n.dir, r.selected, dark))
        .style(tree_icons::style(k))
        .name("Project files")
        .scroll_bar(true)
        .on_menu(|s: &mut G, id, p, _| s.tree_menu = Some(("scroll", id.to_owned(), p)))
        .build(s, k)
        .h(8.0 * 28.0 + 8.0);
    let menu = s.tree_menu.as_ref().filter(|(which, _, _)| *which == "scroll");
    let target = menu.map(|(_, id, _)| id.clone()).unwrap_or_default();
    let copy_id = target.clone();
    let toggle_id = target.clone();
    let is_folder = !target.is_empty() && s.scroll_files.has_children(&target);
    let mut ctx = context_menu("tree-scroll-menu", col().w(260.0).fill(k.c.bg_base).pt(4.0).px(8.0).child(tree), menu.map(|(_, _, p)| *p))
        .item(MenuItem::new("Copy path").on_select_tx(move |s: &mut G, tx: &mut Tx| {
            tx.copy(&copy_id);
            s.tree_action = Some(format!("Copied {copy_id}"));
            s.tree_menu = None;
        }));
    if is_folder {
        ctx = ctx.item(MenuItem::new("Toggle folder").on_select(move |s: &mut G| {
            s.scroll_files.toggle(&toggle_id);
            s.tree_action = Some(format!("Toggled {toggle_id}"));
            s.tree_menu = None;
        }));
    } else {
        ctx = ctx.item(MenuItem::new("Select row").on_select(move |s: &mut G| {
            s.scroll_files.select(&toggle_id);
            s.tree_action = Some(format!("Selected {toggle_id}"));
            s.tree_menu = None;
        }));
    }
    let panel = ctx.on_close(|s: &mut G| s.tree_menu = None).build(k, layer);
    let status = s.tree_action.as_deref().unwrap_or("Right-click a row or press Shift+F10");
    col().gap(8.0).child(panel).child(name(k, status))
}

/// A file tree with unloaded and empty folder branches that populate on expand via `on_toggle`.
pub(super) fn file_tree_lazy_branches(s: &G, k: &Kit, _layer: &mut Vec<El>) -> El {
    let loaded_remote = s.lazy_loaded.contains("remote");
    let tree = Tree::new(|s: &G| &s.lazy_files, |s: &mut G| &mut s.lazy_files, FileNode::label)
        .row(move |n, r| {
            let item = TreeItem::file(n, r);
            match n.path.as_str() {
                "remote" if !loaded_remote => item.badge("unloaded"),
                "empty" | "remote/empty-sub" => item.badge("empty"),
                _ => item,
            }
        })
        .name("Lazy branches")
        .on_toggle(|s: &mut G, id, open, _| {
            if open && id == "remote" && s.lazy_loaded.insert(id.to_owned()) {
                s.lazy_files.insert(
                    Some("remote"),
                    "remote/api.ts",
                    FileNode { name: "api.ts".into(), path: "remote/api.ts".into(), dir: false, data: Some(()) },
                );
                s.lazy_files.insert_branch(
                    Some("remote"),
                    "remote/empty-sub",
                    FileNode { name: "empty-sub".into(), path: "remote/empty-sub".into(), dir: true, data: None },
                );
                s.lazy_files.insert(
                    Some("remote"),
                    "remote/schema.json",
                    FileNode { name: "schema.json".into(), path: "remote/schema.json".into(), dir: false, data: Some(()) },
                );
                s.tree_action = Some("Expanded remote (loaded 3 entries)".into());
            } else {
                s.tree_action = Some(format!("{} {id}", if open { "Expanded" } else { "Collapsed" }));
            }
        })
        .build(s, k)
        .h(8.0 * 24.0 + 8.0);
    let panel = col().w(260.0).fill(k.c.bg_base).pt(4.0).px(8.0).child(tree);
    let status = s.tree_action.as_deref().unwrap_or("Expand 'remote' to lazy-load, or toggle 'empty'");
    col().gap(8.0).child(panel).child(name(k, status))
}
