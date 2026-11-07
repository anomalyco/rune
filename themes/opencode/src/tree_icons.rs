//! The file icons of Desktop's file trees and file lists (Files, Review, the files filter): the
//! monochrome "complete" set of `@pierre/trees`, which Desktop draws in `--v2-icon-icon-muted` and,
//! on the selected row, in the file type's colour (`file-tree.css`). Folders show only the
//! [`CHEVRON`], turned a quarter clockwise while open.
//!
//! ```ignore
//! let t = tree_icons::file(&node.path);
//! icon(t.icon(), 16.0).color(if selected { t.color(dark) } else { k.c.icon_muted })
//! ```

mod catalog;

use rune::{Color, Icon, TextStyle};
use rune_kit::{Kit, TreeItem, TreeStyle};

/// A folder row's disclosure (`outline-chevron-right`, 16 points), drawn 1 point low.
pub static CHEVRON: &Icon = &crate::icons::OUTLINE_CHEVRON_RIGHT;

/// A file's icon in a file tree and its type (`typescript`, `json`, `default`).
#[derive(Clone, Copy, Debug)]
pub struct TreeIcon {
    token: &'static str,
}

impl TreeIcon {
    pub fn icon(&self) -> &'static Icon {
        catalog::icon(self.token)
    }
    /// The file type, as `@pierre/trees` names it (`data-icon-token`).
    pub fn token(&self) -> &'static str {
        self.token
    }
    /// The type's colour for one appearance (`--trees-file-icon-color-<token>`), which a selected
    /// row draws its icon in.
    pub fn color(&self, dark: bool) -> Color {
        let (light, night) = catalog::tint(self.token);
        rune::color::hex(if dark { night } else { light }).unwrap_or(Color::TRANSPARENT)
    }
}

/// The icon for a file at `path` (`/` or `\` separated, or just a name): its whole name first
/// (`bun.lock`, `Dockerfile`), then each dotted suffix from the first dot (`config.ts`, then `ts`),
/// `.tsx` and `.jsx` as React and `.scss` and `.sass` as Sass, else the plain file.
pub fn file(path: &str) -> TreeIcon {
    let name = path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("").to_lowercase();
    let suffix = || {
        name.match_indices('.')
            .map(|(i, _)| &name[i + 1..])
            .find_map(|s| catalog::complete_override(s).or_else(|| catalog::by_extension(s)))
    };
    TreeIcon { token: catalog::by_file_name(&name).or_else(suffix).unwrap_or("default") }
}

/// Desktop's file tree rows (`pierre-tree.tsx` over `@pierre/trees`): 28 points tall with 8 at
/// the ends, 6 between the chevron or icon and the name, 21.5 per level, 6-point corners, 12-point
/// medium names in `text-muted` (`text-base` and the pressed overlay when selected, the hover
/// overlay under the pointer), chevron-only folders with the chevron 1 point low, guides at 0.75
/// only while the pointer is over the tree, and long names fading out over 16 points.
pub fn style(k: &Kit) -> TreeStyle {
    let t = crate::theme(k.dark);
    let v = |n: &str| t.color(&format!("v2-{n}"));
    let mut s = TreeStyle::from_kit(k)
        .hover(v("overlay-simple-overlay-hover"))
        .gap(6.0)
        .radius(6.0)
        .label(TextStyle::new(12.0, 500.0).lh(28.0), v("text-text-muted"), v("text-text-base"))
        .fade_long_labels(16.0)
        .chevron_only_folders(true)
        .chevron(&crate::icons::CHEVRON_DOWN)
        .chevron_nudge(1.0)
        .chevron_colors(v("icon-icon-muted"), v("text-text-base"))
        .guides_on_hover(0.75);
    (s.row_height, s.indent, s.padding) = (28.0, 21.5, 8.0);
    s.guide = v("border-border-muted").alpha(v("border-border-muted").a * 0.5);
    s.selected = v("overlay-simple-overlay-pressed");
    s
}

/// A file tree row as Desktop draws it under [`style`]: the name, and for a file its icon in
/// `icon-muted`, or in its type's colour while selected. Folders show only the chevron.
pub fn item(name: &str, path: &str, folder: bool, selected: bool, dark: bool) -> TreeItem {
    let item = TreeItem::new(name);
    if folder {
        return item;
    }
    let t = file(path);
    let muted = crate::theme(dark).color("v2-icon-icon-muted");
    item.icon(t.icon()).icon_color(if selected { t.color(dark) } else { muted })
}

/// Every file type with an icon of its own, for a gallery.
pub fn all() -> impl Iterator<Item = TreeIcon> {
    catalog::TOKENS.iter().map(|&token| TreeIcon { token })
}
