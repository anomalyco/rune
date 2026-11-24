//! Rune's own `DiffView` scene showing `DiffView::labels` (via `DiffLabel`) and
//! `DiffView::comment_filter`.

use rune::{El, col, row, text};
use rune_kit::diff::{DiffLabel, DiffMode, DiffState, DiffView, FileDiff, FileStatus, FoldRow, LineKind, Side, parse};
use rune_kit::{Kit, Switch};

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
