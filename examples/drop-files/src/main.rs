//! Drop files into a Rune window from Finder, Explorer or a file manager. `cargo run -p
//! drop-files`. The pictures zone takes images only; the other zone takes anything; a drop
//! anywhere else lands on the window. Each drop prints `drop <target> <files>`.
//!
//! Headless, with the highlight held before each drop:
//! `cargo run -p drop-files -- --script examples/drop-files/scripts/drop.json --frames 90 --every 6 --cols 5 --strip strip.png`

#![forbid(unsafe_code)]

use std::path::PathBuf;

use rune::tok::{self, radius, space, type_};
use rune::{AppSpec, Color, Cx, El, col, row, text};

#[derive(Default)]
struct State {
    /// The zone a file drag is over.
    over: Option<&'static str>,
    last: Option<(&'static str, Vec<PathBuf>)>,
}

fn names(files: &[PathBuf]) -> String {
    files.iter().map(|f| f.file_name().map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .collect::<Vec<_>>().join(", ")
}

fn dropped(s: &mut State, target: &'static str, files: &[PathBuf]) {
    println!("drop {target} {}", names(files));
    s.last = Some((target, files.to_vec()));
}

fn zone(s: &State, accent: Color, key: &'static str, title: &str, detail: &str) -> El {
    let t = tok::theme(true);
    let over = s.over == Some(key);
    col().key(key).size(200.0, 120.0).padding(space::S3).gap(space::S1).radius(radius::LG).center()
        .fill(if over { accent.alpha(0.18) } else { t.card })
        .border(if over { 2.0 } else { 1.0 }, if over { accent } else { t.card_border })
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
    let status = match &s.last {
        Some((target, files)) => format!("Dropped on {target}: {}", names(files)),
        None => "Drag files from another app onto a zone".into(),
    };
    col().w_full().h_full().center().gap(space::S6).fill(t.bg)
        .child(row().gap(space::S3)
            .child(zone(s, accent, "pictures", "Pictures", "PNG, JPEG, GIF or HEIC")
                .accepts_extensions(&["png", "jpg", "jpeg", "gif", "heic"]))
            .child(zone(s, accent, "anything", "Anything", "Any file")))
        .child(text(status).key("status").font(type_::BODY).color(t.text_secondary))
        .on_drop_files(|s: &mut State, files, _| dropped(s, "window", files))
}

fn main() {
    rune::run(AppSpec::new("Drop Files", (520.0, 240.0), State::default, view));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn run() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (520.0, 240.0), Env { system_dark: true, scale: 2.0 });
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
        rt.drop_files(vec!["a.png".into(), "notes.txt".into()], at);
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.over, None);
        assert_eq!(rt.state.last, Some(("pictures", vec!["a.png".into()])));

        rt.drop_files(vec!["notes.txt".into()], at);
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.state.last, Some(("window", vec!["notes.txt".into()])), "the window takes what the zone refuses");
    }
}
