//! Drop files into a Rune window from Finder, Explorer or a file manager. `cargo run -p
//! drop-files`. The pictures zone takes images only; the folder zone uses `El::accepts_drag`
//! to refuse drops when read-only or carrying more than two files; the anything zone takes any
//! file; the strip around all three follows the drag passively with `El::on_drop_hover_within`,
//! `El::on_drop_move_within`, and `Tx::drag_paths`; a drop anywhere else lands on the window.
//! Each drop prints `drop <target> <files>` and shows Open and Reveal buttons for each dropped
//! path (`Tx::open_path`, `Tx::reveal_path`).
//!
//! Headless, with the highlight held before each drop:
//! `cargo run -p drop-files -- --script examples/drop-files/scripts/drop.json --frames 120 --every 8 --cols 5 --strip strip.png`
//! Or inspect the dropped-file controls directly: `cargo run -p drop-files -- --snap drop-files.png --scene dropped`

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Color, Cursor, Cx, El, PathAction, col, el, row, text};

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
    /// Feedback from the last Open or Reveal button press.
    action_status: Option<String>,
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn names(files: &[PathBuf]) -> String {
    files.iter().map(|f| display_name(f)).collect::<Vec<_>>().join(", ")
}

fn dropped(s: &mut State, target: &'static str, files: &[PathBuf]) {
    println!("drop {target} {}", names(files));
    s.last = Some((target, files.to_vec()));
    s.rejected_msg = None;
    s.action_status = None;
}

fn trigger_path(s: &mut State, tx: &mut rune::Tx, path: &Path, action: PathAction) {
    let name = display_name(path);
    let (verb, result) = match action {
        PathAction::Open => ("Opened", tx.open_path(path)),
        PathAction::Reveal => ("Revealed", tx.reveal_path(path)),
    };
    s.action_status = Some(match result {
        Ok(()) => format!("{verb} {name}"),
        Err(e) => format!("Failed for {name}: {e}"),
    });
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

fn action_button(key: String, label: &str, accessible: String, path: PathBuf, action: PathAction) -> El {
    let t = tok::theme(true);
    el().key(key).role("button").label(accessible).focusable(true).cursor(Cursor::Pointer)
        .px(space::S2).py(space::S1).radius(radius::SM)
        .fill(t.card).border(1.0, t.card_border)
        .child(text(label).font(type_::LABEL).color(t.text))
        .on_press_tx(move |s: &mut State, tx| trigger_path(s, tx, &path, action))
}

fn file_row(index: usize, path: &Path) -> El {
    let t = tok::theme(true);
    let name = display_name(path);
    row().key(format!("file-{index}")).w(594.0).items_center().gap(space::S2)
        .px(space::S2).py(space::S1).radius(radius::MD).fill(t.card).border(1.0, t.card_border)
        .child(text(&name).font(type_::LABEL).color(t.text).grow(1.0))
        .child(action_button(format!("open-{index}"), "Open", format!("Open {name}"), path.to_path_buf(), PathAction::Open))
        .child(action_button(format!("reveal-{index}"), "Reveal", format!("Reveal {name}"), path.to_path_buf(), PathAction::Reveal))
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

    let mut root = col().w_full().h_full().center().gap(space::S3).padding(space::S3).fill(t.bg)
        .child(toggle)
        .child(strip)
        .child(text(preview).key("preview").font(type_::LABEL).color(if s.strip_over { accent } else { t.text_secondary }))
        .child(text(status).key("status").font(type_::BODY).color(t.text_secondary));
    if let Some((_, files)) = &s.last {
        let mut list = col().key("dropped-list").gap(space::S1);
        for (i, path) in files.iter().take(3).enumerate() {
            list = list.child(file_row(i, path));
        }
        root = root.child(list);
    }
    if let Some(msg) = &s.action_status {
        root = root.child(text(msg).key("action-status").font(type_::LABEL).color(accent));
    }
    root.on_drop_files(|s: &mut State, files, _| dropped(s, "window", files))
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("dropped", |s| {
            s.last = Some(("anything", vec![PathBuf::from("photo.png"), PathBuf::from("notes.txt")]));
        })],
        ..AppSpec::new("Drop Files", (680.0, 420.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, PathRequest, Runtime};

    fn run() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (680.0, 420.0), Env { system_dark: true, scale: 2.0 });
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

    #[test]
    fn dropped_file_buttons_queue_open_and_reveal_requests_and_expose_a11y() {
        let mut rt = run();
        let at = rt.find("anything");
        rt.drop_files(vec!["/example/report.pdf".into()], at);
        rt.frame(1.0 / 60.0);

        let tree = rt.accessibility_tree();
        let has_button = |name: &str| tree.nodes.iter().any(|(_, node)| {
            node.role() == rune::accesskit::Role::Button && node.label() == Some(name)
        });
        assert!(has_button("Open report.pdf"), "Open button must appear in accessibility tree");
        assert!(has_button("Reveal report.pdf"), "Reveal button must appear in accessibility tree");

        let open_at = rt.find("open-0").expect("open-0 button");
        rt.pointer_down(open_at.0, open_at.1);
        rt.pointer_up(open_at.0, open_at.1);
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.action_status.as_deref(), Some("Opened report.pdf"));

        let reveal_at = rt.find("reveal-0").expect("reveal-0 button");
        rt.pointer_down(reveal_at.0, reveal_at.1);
        rt.pointer_up(reveal_at.0, reveal_at.1);
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.state.action_status.as_deref(), Some("Revealed report.pdf"));

        assert_eq!(
            rt.take_path_requests(),
            [
                PathRequest { path: PathBuf::from("/example/report.pdf"), action: PathAction::Open },
                PathRequest { path: PathBuf::from("/example/report.pdf"), action: PathAction::Reveal },
            ],
        );
    }
}
