//! Drop files into a Rune window from Finder, Explorer or a file manager. `cargo run -p
//! drop-files`. The pictures zone takes images only; the folder zone uses `El::accepts_drag`
//! to refuse drops when read-only or carrying more than two files; the anything zone takes any
//! file; the strip around all three follows the drag passively with `El::on_drop_hover_within`,
//! `El::on_drop_move_within`, and `Tx::drag_paths`; a drop anywhere else lands on the window.
//! Each drop prints `drop <target> <files>`.
//!
//! Headless, with the highlight held before each drop:
//! `cargo run -p drop-files -- --script examples/drop-files/scripts/drop.json --frames 120 --every 8 --cols 5 --strip strip.png`

#![forbid(unsafe_code)]

use std::path::PathBuf;

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Color, Cx, El, col, row, text};

const REJECT_RED: Color = Color { r: 0.92, g: 0.34, b: 0.34, a: 1.0 };

#[derive(Default)]
struct State {
    /// The zone a file drag is over.
    over: Option<&'static str>,
    /// The folder zone refused the active drag (`accepts_drag`).
    folder_rejected: bool,
    /// When true, `folder` refuses every drag via `El::accepts_drag`.
    read_only: bool,
    /// Ancestor strip passive hover (`El::on_drop_hover_within`).
    strip_over: bool,
    /// Ancestor strip local drag position (`El::on_drop_move_within`).
    strip_point: Option<(f32, f32)>,
    /// Live preview of the paths carried by the active drag (`Tx::drag_paths`).
    dragging: Vec<PathBuf>,
    last: Option<(&'static str, Vec<PathBuf>)>,
    rejected_msg: Option<String>,
}

fn names(files: &[PathBuf]) -> String {
    files.iter().map(|f| f.file_name().map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .collect::<Vec<_>>().join(", ")
}

fn dropped(s: &mut State, target: &'static str, files: &[PathBuf]) {
    println!("drop {target} {}", names(files));
    s.last = Some((target, files.to_vec()));
    s.rejected_msg = None;
}

fn zone(s: &State, accent: Color, key: &'static str, title: &str, detail: &str, rejected: bool) -> El {
    let t = tok::theme(true);
    let over = s.over == Some(key);
    let fill = if rejected { REJECT_RED.alpha(0.18) } else if over { accent.alpha(0.18) } else { t.card };
    let border = if rejected { REJECT_RED } else if over { accent } else { t.card_border };
    col().key(key).size(190.0, 116.0).padding(space::S3).gap(space::S1).radius(radius::LG).center()
        .fill(fill)
        .border(if over || rejected { 2.0 } else { 1.0 }, border)
        .child(text(title).font(type_::TITLE).color(t.text))
        .child(text(detail).font(type_::LABEL).color(t.text_secondary))
        .on_drop_files(move |s: &mut State, files, _| dropped(s, key, files))
        .on_drop_hover(move |s: &mut State, over, _| {
            if over { s.over = Some(key) } else if s.over == Some(key) { s.over = None }
        })
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let accent = cx.accent();
    let status = if let Some(msg) = &s.rejected_msg {
        msg.clone()
    } else if let Some((target, files)) = &s.last {
        format!("Dropped on {target}: {}", names(files))
    } else {
        "Drag files from another app onto a zone".into()
    };
    let preview = if s.dragging.is_empty() {
        "Drag preview: idle".into()
    } else if let Some((x, y)) = s.strip_point {
        format!("Dragging ({} files) at ({:.0}, {:.0}): {}", s.dragging.len(), x, y, names(&s.dragging))
    } else {
        format!("Dragging ({} files): {}", s.dragging.len(), names(&s.dragging))
    };
    let folder_detail = if s.read_only { "Read-only (refuses drops)" } else { "Writable (max 2 files)" };
    let toggle_label = if s.read_only { "Folder: Read-only" } else { "Folder: Writable" };

    let strip = row().key("strip").padding(space::S3).gap(space::S3).radius(radius::LG)
        .fill(if s.strip_over { accent.alpha(0.08) } else { Color::TRANSPARENT })
        .border(if s.strip_over { 1.5 } else { 1.0 }, if s.strip_over { accent.alpha(0.7) } else { t.card_border })
        .on_drop_hover_within(|s: &mut State, over, tx| {
            s.strip_over = over;
            if over {
                s.dragging = tx.drag_paths().map(<[PathBuf]>::to_vec).unwrap_or_default();
            } else {
                s.dragging.clear();
                s.strip_point = None;
            }
        })
        .on_drop_move_within(|s: &mut State, p, tx| {
            s.strip_point = Some((p.x, p.y));
            if let Some(paths) = tx.drag_paths() {
                s.dragging = paths.to_vec();
            }
        })
        .child(zone(s, accent, "pictures", "Pictures", "PNG, JPEG, GIF or HEIC", false)
            .accepts_extensions(&["png", "jpg", "jpeg", "gif", "heic"]))
        .child(zone(s, accent, "folder", "Folder", folder_detail, s.folder_rejected)
            .accepts_drag(|s: &State, paths| !s.read_only && paths.len() <= 2)
            .on_drop_rejected_hover(|s: &mut State, over, _| s.folder_rejected = over)
            .on_drop_rejected(|s: &mut State, files, _| {
                s.rejected_msg = Some(if s.read_only {
                    format!("Folder is read-only; refused {}", names(files))
                } else {
                    format!("Folder takes at most 2 files (got {})", files.len())
                });
            }))
        .child(zone(s, accent, "anything", "Anything", "Any file", false));

    let toggle = row().key("read-only").px(space::S3).py(space::S2).radius(radius::MD).center()
        .role("switch").label("Read-only folder").checked(s.read_only).focusable(true)
        .fill(if s.read_only { REJECT_RED.alpha(0.22) } else { t.card })
        .border(1.0, if s.read_only { REJECT_RED } else { t.card_border })
        .child(text(toggle_label).font(type_::LABEL).color(t.text))
        .on_press(|s: &mut State| s.read_only = !s.read_only);

    col().w_full().h_full().center().gap(space::S3).padding(space::S3).fill(t.bg)
        .child(toggle)
        .child(strip)
        .child(text(preview).key("preview").font(type_::LABEL).color(if s.strip_over { accent } else { t.text_secondary }))
        .child(text(status).key("status").font(type_::BODY).color(t.text_secondary))
        .on_drop_files(|s: &mut State, files, _| dropped(s, "window", files))
}

fn main() {
    rune::run(AppSpec::new("Drop Files", (680.0, 320.0), State::default, view));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn run() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (680.0, 320.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        rt
    }

    #[test]
    fn pictures_go_to_the_pictures_zone_and_the_rest_to_the_zone_around_them() {
        let mut rt = run();
        let at = rt.find("pictures");
        rt.drag_files(vec!["a.png".into(), "notes.txt".into()], at);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.over, Some("pictures"), "the zone lights up under the drag");
        assert!(rt.state.strip_over, "the ancestor strip lights up passively");
        assert_eq!(rt.state.dragging, vec![PathBuf::from("a.png"), PathBuf::from("notes.txt")]);
        rt.drop_files(vec!["a.png".into(), "notes.txt".into()], at);
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.over, None);
        assert!(!rt.state.strip_over);
        assert!(rt.state.dragging.is_empty());
        assert_eq!(rt.state.last, Some(("pictures", vec!["a.png".into()])));

        rt.drop_files(vec!["notes.txt".into()], at);
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.state.last, Some(("window", vec!["notes.txt".into()])), "the window takes what the zone refuses");
    }

    #[test]
    fn folder_zone_uses_accepts_drag_and_strip_follows_drag_across_zones() {
        let mut rt = run();
        let folder = rt.find("folder").expect("folder zone");
        let anything = rt.find("anything").expect("anything zone");

        // Over folder with 3 files: accepts_drag refuses (>2 files) and shows rejected hover.
        rt.drag_files(vec!["a.txt".into(), "b.txt".into(), "c.txt".into()], Some(folder));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.folder_rejected);
        assert!(rt.state.strip_over);
        assert!(rt.state.strip_point.is_some());
        assert_eq!(rt.state.dragging.len(), 3);

        // Move across to anything: strip stays hovered and tracks new coordinates.
        let p0 = rt.state.strip_point;
        assert!(rt.drag_files_to(Some(anything)));
        rt.frame(2.0 / 60.0);
        assert!(!rt.state.folder_rejected);
        assert_eq!(rt.state.over, Some("anything"));
        assert!(rt.state.strip_over);
        assert_ne!(rt.state.strip_point, p0);
        rt.drag_files_leave();
        rt.frame(3.0 / 60.0);

        // Toggle read-only on and drop 1 file on folder: refused by state check.
        let toggle = rt.find("read-only").expect("read-only toggle");
        rt.pointer_down(toggle.0, toggle.1);
        rt.pointer_up(toggle.0, toggle.1);
        rt.frame(4.0 / 60.0);
        assert!(rt.state.read_only);
        rt.drag_files(vec!["one.txt".into()], Some(folder));
        rt.drop_files(vec!["one.txt".into()], Some(folder));
        rt.frame(5.0 / 60.0);
        assert_eq!(rt.state.last, None);
        assert!(rt.state.rejected_msg.as_deref().is_some_and(|m| m.contains("read-only")));
    }
}
