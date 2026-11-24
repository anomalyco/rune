//! Localisable visible and accessible strings for [`DiffView`](super::DiffView).

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::diff::model::{FileStatus, LineKind};

pub(super) type LabelsFn = Rc<dyn Fn(DiffLabel<'_>) -> Option<String>>;

/// Something [`DiffView`](super::DiffView) names on screen or for assistive technology.
///
/// Pass a closure to [`DiffView::labels`](super::DiffView::labels) to translate or rename any
/// variant. Returning `None` (or an empty or whitespace-only string) keeps the default English
/// text so controls never lose their accessible names.
///
/// ```ignore
/// use rune_kit::diff::DiffLabel;
///
/// let name = |label: DiffLabel<'_>| match label {
///     DiffLabel::ExpandFile => Some("Déplier le fichier".into()),
///     DiffLabel::CollapseFile => Some("Replier le fichier".into()),
///     DiffLabel::CommentLine => Some("Commenter cette ligne".into()),
///     _ => None,
/// };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiffLabel<'a> {
    /// Accessible name of the diff's outer list (English: `"Diff"`).
    List,
    /// Accessible name of a diff line row (English: `"added line 12: ..."`,
    /// `"removed line 12: ..."`, `"unchanged line 12: ..."`).
    Line {
        kind: LineKind,
        number: u32,
        text: &'a str,
    },
    /// Accessible name of the hover `+` button that triggers [`DiffView::on_comment`]
    /// (English: `"Comment on this line"`).
    ///
    /// [`DiffView::on_comment`]: super::DiffView::on_comment
    CommentLine,
    /// Visible marker beneath a line that has no trailing line break
    /// (English: `"No newline at end of file"`).
    NoNewlineAtEndOfFile,
    /// Visible count (and trailing-gap accessible name) for a [`FoldRow::Summary`] fold row
    /// (English: `"1 unchanged line"` / `"N unchanged lines"`).
    ///
    /// [`FoldRow::Summary`]: super::FoldRow::Summary
    UnchangedLines(u32),
    /// Visible count and accessible name for a [`FoldRow::LineInfo`] fold row
    /// (English: `"1 unmodified line"` / `"N unmodified lines"`).
    ///
    /// [`FoldRow::LineInfo`]: super::FoldRow::LineInfo
    UnmodifiedLines(u32),
    /// Accessible name of a [`FoldRow::Summary`] fold row followed by a hunk header
    /// (English: `"N unchanged lines, then @@ ..."`).
    ///
    /// [`FoldRow::Summary`]: super::FoldRow::Summary
    UnchangedLinesBefore {
        count: u32,
        header: &'a str,
    },
    /// Accessible name of the expand-all button on a short fold row
    /// (English: `"Show N hidden lines"`).
    ShowHiddenLines(u32),
    /// Accessible name of the button that reveals lines above a hunk
    /// (English: `"Show N more lines above"`).
    ShowMoreLinesAbove(u32),
    /// Accessible name of the button that reveals lines below a hunk
    /// (English: `"Show N more lines below"`).
    ShowMoreLinesBelow(u32),
    /// Accessible name of the count button that expands all lines of a long [`FoldRow::Summary`]
    /// fold row (English: `"Show all N lines"`).
    ///
    /// [`FoldRow::Summary`]: super::FoldRow::Summary
    ShowAllLines(u32),
    /// Accessible name of the file header toggle button when the file is collapsed
    /// (English: `"Expand file"`).
    ExpandFile,
    /// Accessible name of the file header toggle button when the file is expanded
    /// (English: `"Collapse file"`).
    CollapseFile,
    /// Visible file status badge in a file header (English: `"Added"`, `"Deleted"`,
    /// `"Renamed"`, `"Copied"`, `"Modified"`). Not shown for [`FileStatus::Modified`].
    FileStatus(FileStatus),
    /// Accessible name of a file header (English: `"path, 3 added, 1 removed"` or
    /// `"path, renamed from old, 3 added, 1 removed"`).
    FileSummary {
        path: &'a str,
        renamed_from: Option<&'a str>,
        additions: usize,
        deletions: usize,
    },
    /// Visible text and accessible name of the note row for a binary file
    /// (English: `"Binary file not shown"`).
    BinaryFile,
    /// Visible text and accessible name of the note row for a file moved or copied without edits
    /// (English: `"File moved without changes"`).
    MovedWithoutChanges,
    /// Visible text and accessible name of the note row for a mode-only change
    /// (English: `"File mode changed"`).
    ModeChanged,
    /// Visible text and accessible name of the note row for an added or deleted empty file
    /// (English: `"Empty file"`).
    EmptyFile,
    /// Visible text and accessible name of the note row for a file with no hunks
    /// (English: `"No changes"`).
    NoChanges,
}

/// Resolves `label` through `namer`, falling back to English when `namer` is absent or returns
/// `None` or a blank string.
pub(super) fn resolve(namer: Option<&LabelsFn>, label: DiffLabel<'_>) -> String {
    namer
        .and_then(|f| f(label))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| english(label))
}

/// Deterministic fingerprint of `namer`'s outputs for memoised diff rows (notes, gaps and lines).
/// Returns `0` when no custom namer is installed so default views pay zero per-commit work; when
/// installed, hashes representative samples across every variant that can appear in a memoised
/// row so locale or label changes invalidate cached rows while steady frames hit the memo cache.
pub(super) fn fingerprint(namer: Option<&LabelsFn>) -> u64 {
    let Some(f) = namer else { return 0 };
    let mut h = std::hash::DefaultHasher::new();
    1u8.hash(&mut h);
    let samples = [
        DiffLabel::Line { kind: LineKind::Added, number: 1, text: "" },
        DiffLabel::Line { kind: LineKind::Removed, number: 1, text: "" },
        DiffLabel::Line { kind: LineKind::Context, number: 1, text: "" },
        DiffLabel::CommentLine,
        DiffLabel::NoNewlineAtEndOfFile,
        DiffLabel::UnchangedLines(1),
        DiffLabel::UnchangedLines(2),
        DiffLabel::UnchangedLines(5),
        DiffLabel::UnmodifiedLines(1),
        DiffLabel::UnmodifiedLines(2),
        DiffLabel::UnmodifiedLines(5),
        DiffLabel::UnchangedLinesBefore { count: 2, header: "" },
        DiffLabel::ShowHiddenLines(2),
        DiffLabel::ShowMoreLinesAbove(2),
        DiffLabel::ShowMoreLinesBelow(2),
        DiffLabel::ShowAllLines(2),
        DiffLabel::BinaryFile,
        DiffLabel::MovedWithoutChanges,
        DiffLabel::ModeChanged,
        DiffLabel::EmptyFile,
        DiffLabel::NoChanges,
    ];
    for sample in samples {
        resolve(Some(f), sample).hash(&mut h);
    }
    h.finish()
}

pub(super) fn english(label: DiffLabel<'_>) -> String {
    match label {
        DiffLabel::List => "Diff".into(),
        DiffLabel::Line { kind, number, text } => {
            let kind = match kind {
                LineKind::Added => "added",
                LineKind::Removed => "removed",
                LineKind::Context => "unchanged",
            };
            format!("{kind} line {number}: {text}")
        }
        DiffLabel::CommentLine => "Comment on this line".into(),
        DiffLabel::NoNewlineAtEndOfFile => "No newline at end of file".into(),
        DiffLabel::UnchangedLines(1) => "1 unchanged line".into(),
        DiffLabel::UnchangedLines(n) => format!("{n} unchanged lines"),
        DiffLabel::UnmodifiedLines(1) => "1 unmodified line".into(),
        DiffLabel::UnmodifiedLines(n) => format!("{n} unmodified lines"),
        DiffLabel::UnchangedLinesBefore { count, header } => format!("{count} unchanged lines, then {header}"),
        DiffLabel::ShowHiddenLines(n) => format!("Show {n} hidden lines"),
        DiffLabel::ShowMoreLinesAbove(n) => format!("Show {n} more lines above"),
        DiffLabel::ShowMoreLinesBelow(n) => format!("Show {n} more lines below"),
        DiffLabel::ShowAllLines(n) => format!("Show all {n} lines"),
        DiffLabel::ExpandFile => "Expand file".into(),
        DiffLabel::CollapseFile => "Collapse file".into(),
        DiffLabel::FileStatus(FileStatus::Added) => "Added".into(),
        DiffLabel::FileStatus(FileStatus::Deleted) => "Deleted".into(),
        DiffLabel::FileStatus(FileStatus::Renamed) => "Renamed".into(),
        DiffLabel::FileStatus(FileStatus::Copied) => "Copied".into(),
        DiffLabel::FileStatus(FileStatus::Modified) => "Modified".into(),
        DiffLabel::FileSummary { path, renamed_from, additions, deletions } => match renamed_from {
            Some(old) => format!("{path}, renamed from {old}, {additions} added, {deletions} removed"),
            None => format!("{path}, {additions} added, {deletions} removed"),
        },
        DiffLabel::BinaryFile => "Binary file not shown".into(),
        DiffLabel::MovedWithoutChanges => "File moved without changes".into(),
        DiffLabel::ModeChanged => "File mode changed".into(),
        DiffLabel::EmptyFile => "Empty file".into(),
        DiffLabel::NoChanges => "No changes".into(),
    }
}
