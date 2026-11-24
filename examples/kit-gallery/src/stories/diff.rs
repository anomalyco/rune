//! Rune's own `DiffView` scenes showing `DiffView::labels` (`DiffLabel`),
//! `DiffView::comment_filter`, `DiffView::file_header`, `DiffView::syntax_theme`, and
//! edge-whitespace-ignoring `DiffOptions`.

use rune::{Color, Cursor, El, col, icon, row, text};
use rune_kit::diff::{DiffLabel, DiffMode, DiffOptions, DiffState, DiffView, FileDiff, FileStatus, FoldRow, LineKind, Side, parse};
use rune_kit::syntax::{Syntax, SyntaxTheme};
use rune_kit::{Kit, ScrollBarStyle, Switch};

use crate::G;

const RENAME_PATCH: &str = "diff --git a/src/legacy.rs b/src/backoff.rs
similarity index 100%
rename from src/legacy.rs
rename to src/backoff.rs
";

pub(crate) fn sample_diff() -> DiffState {
    let middle: String = (4..=36).map(|i| format!("    let step_{i} = delay({i});\n")).collect();
    let old = format!(
        "pub fn retry(tries: u32) -> u32 {{\n    let base = 100;\n    let cap = 1000;\n{middle}    base.min(cap)\n}}\n"
    );
    let new = format!(
        "pub fn retry(tries: u32) -> u32 {{\n    let base = 150;\n    let cap = 2000;\n{middle}    base.saturating_mul(tries).min(cap)\n}}\n"
    );
    let mut files = vec![FileDiff::from_texts("src/retry.rs", &old, &new)];
    files.extend(parse(RENAME_PATCH));
    DiffState::new(files)
}

fn french_label(label: DiffLabel<'_>) -> Option<String> {
    Some(match label {
        DiffLabel::List => "Différences".into(),
        DiffLabel::Line { kind, number, text } => {
            let word = match kind {
                LineKind::Added => "ligne ajoutée",
                LineKind::Removed => "ligne supprimée",
                LineKind::Context => "ligne inchangée",
            };
            format!("{word} {number} : {text}")
        }
        DiffLabel::CommentLine => "Commenter cette ligne".into(),
        DiffLabel::NoNewlineAtEndOfFile => "Pas de saut de ligne en fin de fichier".into(),
        DiffLabel::UnchangedLines(1) => "1 ligne inchangée".into(),
        DiffLabel::UnchangedLines(n) => format!("{n} lignes inchangées"),
        DiffLabel::UnmodifiedLines(1) => "1 ligne non modifiée".into(),
        DiffLabel::UnmodifiedLines(n) => format!("{n} lignes non modifiées"),
        DiffLabel::UnchangedLinesBefore { count: 1, header } => format!("1 ligne inchangée, puis {header}"),
        DiffLabel::UnchangedLinesBefore { count, header } => format!("{count} lignes inchangées, puis {header}"),
        DiffLabel::ShowHiddenLines(1) => "Afficher 1 ligne masquée".into(),
        DiffLabel::ShowHiddenLines(n) => format!("Afficher {n} lignes masquées"),
        DiffLabel::ShowMoreLinesAbove(n) => format!("Afficher {n} lignes au-dessus"),
        DiffLabel::ShowMoreLinesBelow(n) => format!("Afficher {n} lignes en dessous"),
        DiffLabel::ShowAllLines(n) => format!("Afficher les {n} lignes"),
        DiffLabel::ExpandFile => "Déplier le fichier".into(),
        DiffLabel::CollapseFile => "Replier le fichier".into(),
        DiffLabel::FileStatus(FileStatus::Added) => "Ajouté".into(),
        DiffLabel::FileStatus(FileStatus::Deleted) => "Supprimé".into(),
        DiffLabel::FileStatus(FileStatus::Renamed) => "Renommé".into(),
        DiffLabel::FileStatus(FileStatus::Copied) => "Copié".into(),
        DiffLabel::FileStatus(FileStatus::Modified) => "Modifié".into(),
        DiffLabel::FileSummary { path, renamed_from, additions, deletions } => match renamed_from {
            Some(old) => format!("{path}, renommé depuis {old}, {additions} ajouts, {deletions} suppressions"),
            None => format!("{path}, {additions} ajouts, {deletions} suppressions"),
        },
        DiffLabel::BinaryFile => "Fichier binaire non affiché".into(),
        DiffLabel::MovedWithoutChanges => "Fichier déplacé sans modification".into(),
        DiffLabel::ModeChanged => "Permissions du fichier modifiées".into(),
        DiffLabel::EmptyFile => "Fichier vide".into(),
        DiffLabel::NoChanges => "Aucune modification".into(),
        _ => return None,
    })
}

pub(super) fn localized(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let fr = !s.is("diff-english");
    let new_only = !s.is("diff-both-sides");
    let line_info = s.is("diff-line-info");
    let split = s.diff.mode() == DiffMode::Split;

    let mut view = DiffView::new(|s: &G| &s.diff, |s: &mut G| &mut s.diff)
        .key("diff-localized")
        .fold_row(if line_info { FoldRow::LineInfo } else { FoldRow::Summary })
        .on_comment(|s: &mut G, lines, _| s.diff_comment = Some(lines));
    if fr {
        view = view.labels(french_label);
    }
    if new_only {
        view = view.comment_filter(|_, at| at.side == Side::New);
    }

    let note = match s.diff_comment {
        Some(r) => format!("Commented: file {} {:?} {}..={}", r.file, r.start.side, r.start.line, r.end.line),
        None => if new_only {
            "Hover an added or unchanged line for + (removed lines filtered out)".into()
        } else {
            "Hover any line for + (both sides enabled)".into()
        },
    };

    let controls = row()
        .gap(16.0)
        .center_y()
        .flex_wrap()
        .child(
            Switch::new(fr)
                .label("Localized (fr)")
                .on_toggle(|s: &mut G| s.flip("diff-english"))
                .build(k)
                .key("toggle-locale"),
        )
        .child(
            Switch::new(new_only)
                .label("New side only")
                .on_toggle(|s: &mut G| s.flip("diff-both-sides"))
                .build(k)
                .key("toggle-filter"),
        )
        .child(
            Switch::new(split)
                .label("Split")
                .on_toggle(|s: &mut G| {
                    let next = if s.diff.mode() == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
                    s.diff.set_mode(next);
                })
                .build(k)
                .key("toggle-split"),
        )
        .child(
            Switch::new(line_info)
                .label("LineInfo fold")
                .on_toggle(|s: &mut G| s.flip("diff-line-info"))
                .build(k)
                .key("toggle-fold"),
        );

    col()
        .key("diff-story")
        .w(672.0)
        .h(420.0)
        .gap(10.0)
        .child(controls)
        .child(text(note).font(k.ty.small).color(c.text_muted).key("diff-status"))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .border(1.0, c.border_weak)
                .clip_soft(0.0)
                .child(view.build(s, k)),
        )
}

fn custom_files(ignore_edge_ws: bool) -> Vec<FileDiff> {
    let opts = DiffOptions::new().ignore_edge_whitespace(ignore_edge_ws);
    let old_rs = "pub fn greet(name: &str) -> String {\n   let prefix = \"Hello\";\n  let target = name.trim();\n    format!(\"{prefix}, {target}!\")\n}\n";
    let new_rs = "pub fn greet(name: &str) -> String {\n\tlet prefix = \"Hello\";\n  let target = name.trim().to_owned();\n\tformat!(\"{prefix}, {target}!\")\n}\n";
    let old_cfg = "   host = \"localhost\"\n   port = 8080\n   retries = 3\n";
    let new_cfg = "\thost = \"localhost\"\n   port = 9090\n\tretries = 3\n";
    vec![
        FileDiff::from_texts_with("src/greet.rs", old_rs, new_rs, opts),
        FileDiff::from_texts_with("config/server.toml", old_cfg, new_cfg, opts),
    ]
}

pub(crate) fn custom_diff() -> DiffState {
    let mut st = DiffState::new(custom_files(true));
    st.set_mode(DiffMode::Split);
    st
}

pub(super) fn custom(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let custom_header = !s.is("diff-default-header");
    let custom_syntax = !s.is("diff-default-syntax");
    let ignore_edge_ws = !s.is("diff-exact-ws");
    let split = s.diff_custom.mode() == DiffMode::Split;

    let mut view = DiffView::new(|s: &G| &s.diff_custom, |s: &mut G| &mut s.diff_custom).key("diff-custom");
    if custom_header {
        let muted = c.text_muted;
        let base = c.text_base;
        let faint = c.text_faint;
        let add_fg = c.success.fg;
        let del_fg = c.danger.fg;
        let hover = k.alpha(c.text_base, if k.dark { 0.06 } else { 0.035 });
        let ui_med = k.ty.ui_medium;
        let small = k.ty.small;
        let tri_down = k.glyph.triangle_down;
        let tri_right = k.glyph.triangle_right;
        view = view.file_header(move |s: &G, f, collapsed| {
            let file = &s.diff_custom.files()[f];
            let full = file.path();
            let (dir, name) = match full.rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), n.to_owned()),
                None => (String::new(), full.to_owned()),
            };
            let status = match file.status {
                FileStatus::Added => "added",
                FileStatus::Deleted => "deleted",
                FileStatus::Renamed => "renamed",
                FileStatus::Copied => "copied",
                FileStatus::Modified => "modified",
            };
            let (adds, dels) = (file.additions(), file.deletions());
            let mut bar = row()
                .key(format!("custom-hdr-{f}"))
                .w_full()
                .h_full()
                .px(10.0)
                .gap(8.0)
                .items_center()
                .hover(hover)
                .cursor(Cursor::Pointer)
                .role("button")
                .label(if collapsed { format!("Expand {full}") } else { format!("Collapse {full}") })
                .expanded(!collapsed)
                .on_press(move |s: &mut G| s.diff_custom.set_collapsed(f, !collapsed))
                .child(icon(if collapsed { tri_right } else { tri_down }, 14.0).color(muted));
            if !dir.is_empty() {
                bar = bar.child(text(dir).font(ui_med).color(muted));
            }
            bar = bar
                .child(text(name).font(ui_med).color(base))
                .child(text(status).font(small).color(faint))
                .child(row().grow(1.0).a11y_hidden());
            if adds > 0 {
                bar = bar.child(text(format!("+{adds}")).font(small).color(add_fg));
            }
            if dels > 0 {
                bar = bar.child(text(format!("−{dels}")).font(small).color(del_fg));
            }
            bar
        });
    }
    if custom_syntax {
        let theme = SyntaxTheme::from_kit(k)
            .with(Syntax::Keyword, Color { r: 0.86, g: 0.28, b: 0.62, a: 1.0 })
            .with(Syntax::String, Color { r: 0.18, g: 0.72, b: 0.48, a: 1.0 })
            .with(Syntax::Function, Color { r: 0.24, g: 0.56, b: 0.94, a: 1.0 })
            .with(Syntax::Number, Color { r: 0.92, g: 0.58, b: 0.18, a: 1.0 });
        view = view.syntax_theme(theme);
    }

    let controls = row()
        .gap(16.0)
        .center_y()
        .flex_wrap()
        .child(
            Switch::new(custom_header)
                .label("Custom header")
                .on_toggle(|s: &mut G| s.flip("diff-default-header"))
                .build(k)
                .key("toggle-header"),
        )
        .child(
            Switch::new(custom_syntax)
                .label("Custom syntax")
                .on_toggle(|s: &mut G| s.flip("diff-default-syntax"))
                .build(k)
                .key("toggle-syntax"),
        )
        .child(
            Switch::new(ignore_edge_ws)
                .label("Ignore edge ws")
                .on_toggle(|s: &mut G| {
                    s.flip("diff-exact-ws");
                    let ignore = !s.is("diff-exact-ws");
                    s.diff_custom.set_files(custom_files(ignore));
                })
                .build(k)
                .key("toggle-ws"),
        )
        .child(
            Switch::new(split)
                .label("Split")
                .on_toggle(|s: &mut G| {
                    let next = if s.diff_custom.mode() == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
                    s.diff_custom.set_mode(next);
                })
                .build(k)
                .key("toggle-split"),
        );

    let hint = if ignore_edge_ws {
        "Spaces vs tab indentation on unchanged lines stays context while each split side keeps its original text"
    } else {
        "Exact whitespace comparison marks space-vs-tab indentation lines as changed"
    };

    col()
        .key("diff-custom-story")
        .w(672.0)
        .h(420.0)
        .gap(10.0)
        .child(controls)
        .child(text(hint).font(k.ty.small).color(c.text_muted).key("diff-custom-status"))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .border(1.0, c.border_weak)
                .clip_soft(0.0)
                .child(view.build(s, k)),
        )
}

pub(crate) fn scroll_bars_diff() -> DiffState {
    let mut old = String::new();
    let mut new = String::new();
    for i in 1..=48 {
        let line = format!(
            "    let metric_{i:02} = pipeline.measure_stage(\"stage_{i:02}\", config.batch_limit, config.timeout_ms, telemetry.span(\"diff_scroll_{i:02}\"));\n"
        );
        old.push_str(&line);
        if i == 6 || i == 24 || i == 42 {
            new.push_str(&line.replace("config.batch_limit", "config.effective_batch_limit(retry_budget)"));
        } else {
            new.push_str(&line);
        }
    }
    let second_old = "pub fn summarize(items: &[u64]) -> u64 {\n    items.iter().copied().sum()\n}\n";
    let second_new = "pub fn summarize(items: &[u64]) -> u64 {\n    items.iter().copied().fold(0u64, u64::saturating_add)\n}\n";
    DiffState::new(vec![
        FileDiff::from_texts("src/pipeline/metrics.rs", &old, &new),
        FileDiff::from_texts("src/pipeline/summary.rs", second_old, second_new),
    ])
}

/// `DiffView::scroll_bar(true)` and `DiffView::scroll_bar_styled` with both axes overflowing in
/// unwrapped unified mode, switching to vertical-only when wrapped or split.
pub(super) fn scroll_bars(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let enabled = !s.is("diff-sb-off");
    let styled = s.is("diff-sb-styled");
    let wrap = s.diff_scroll.wrap();
    let split = s.diff_scroll.mode() == DiffMode::Split;

    let mut view = DiffView::new(|s: &G| &s.diff_scroll, |s: &mut G| &mut s.diff_scroll).key("diff-scroll-bars");
    if enabled {
        view = if styled {
            view.scroll_bar_styled(
                ScrollBarStyle::native(c.border_base, c.border_strong)
                    .inside()
                    .always_visible()
                    .track(c.layer_02)
                    .track_hover(c.surface_hover),
            )
        } else {
            view.scroll_bar(true)
        };
    }

    let controls = row()
        .gap(16.0)
        .center_y()
        .flex_wrap()
        .child(
            Switch::new(enabled)
                .label("Scroll bars")
                .on_toggle(|s: &mut G| s.flip("diff-sb-off"))
                .build(k)
                .key("toggle-sb"),
        )
        .child(
            Switch::new(styled)
                .label("Styled always")
                .on_toggle(|s: &mut G| s.flip("diff-sb-styled"))
                .build(k)
                .key("toggle-styled"),
        )
        .child(
            Switch::new(wrap)
                .label("Wrap")
                .on_toggle(|s: &mut G| {
                    let next = !s.diff_scroll.wrap();
                    s.diff_scroll.set_wrap(next);
                })
                .build(k)
                .key("toggle-wrap"),
        )
        .child(
            Switch::new(split)
                .label("Split")
                .on_toggle(|s: &mut G| {
                    let next = if s.diff_scroll.mode() == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
                    s.diff_scroll.set_mode(next);
                })
                .build(k)
                .key("toggle-split"),
        );

    let hint = if !enabled {
        "Scroll bars disabled (default DiffView)"
    } else if wrap || split {
        "Wrapped / split mode: vertical scroll bar only, below the 44 pt sticky file header"
    } else {
        "Unwrapped unified mode: vertical and horizontal scroll bars with a clear bottom-right corner"
    };

    col()
        .key("diff-scroll-bars-story")
        .w(672.0)
        .h(420.0)
        .gap(10.0)
        .child(controls)
        .child(text(hint).font(k.ty.small).color(c.text_muted).key("diff-sb-status"))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .border(1.0, c.border_weak)
                .clip_soft(0.0)
                .child(view.build(s, k)),
        )
}
