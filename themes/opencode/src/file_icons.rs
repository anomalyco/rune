//! `@opencode/ui`'s `FileIcon`: the colour file-type badge Desktop shows beside a path in the
//! command palette, the `@` mention list, tool rows, review headers and file tabs, chosen as its
//! `chooseIconName` does. The badges paint their own colours, so `rune::icon` draws them as they
//! are in either appearance; draw them at 16 points (`file-icon.css`).
//!
//! ```ignore
//! row().gap(8.0).child(icon(file_icons::file("src/util/format.ts"), 16.0)).child(text("format.ts"))
//! ```

mod catalog;

use rune::Icon;

/// The badge for a file at `path` (`/` or `\` separated, or just a name): its whole name first
/// (`package.json`, `Dockerfile`), then its dotted suffixes longest first (`d.ts` before `ts`), else
/// the plain document.
pub fn file(path: &str) -> &'static Icon {
    let name = basename(path).to_lowercase();
    if let Some(icon) = catalog::by_file_name(&name) {
        return icon;
    }
    // The whole name counts as a suffix too, so `dockerfile` matches without a dot.
    std::iter::once(0)
        .chain(name.match_indices('.').map(|(i, _)| i + 1).filter(|&i| i < name.len()))
        .find_map(|i| catalog::by_extension(&name[i..]))
        .unwrap_or(&catalog::DOCUMENT)
}

/// The icon for a folder at `path`, open or closed: by its name as written, or with a leading
/// `.` or `_`, or wrapped in `__` (`src`, `.github`, `__tests__`), else the plain folder.
pub fn folder(path: &str, expanded: bool) -> &'static Icon {
    let name = basename(path).to_lowercase();
    let variants = [name.clone(), format!(".{name}"), format!("_{name}"), format!("__{name}__")];
    let (closed, open) =
        variants.iter().find_map(|v| catalog::by_folder_name(v)).unwrap_or((&catalog::FOLDER, &catalog::FOLDER_OPEN));
    if expanded { open } else { closed }
}

/// Every badge and folder icon, for a gallery.
pub fn all() -> &'static [&'static Icon] {
    catalog::ALL
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("")
}
