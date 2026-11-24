//! `DiffView::labels` (`DiffLabel`) and `DiffView::comment_filter`: localisation of every
//! visible and accessible diff string, English fallback on `None`/blank, memo invalidation on
//! live locale and filter changes, and side/file/line filtering of the `+` comment action in
//! unified and split modes.

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::Kit;
use rune_kit::diff::{
    DiffLabel, DiffMode, DiffState, DiffView, FileDiff, FoldRow, LineRange, LineRef, Side, parse,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Locale {
    Default,
    French,
    German,
    BlankFallback,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterMode {
    None,
    NewOnly,
    OldOnly,
    File1FromLine3,
    RejectAll,
}

struct App {
    diff: DiffState,
    fold_row: FoldRow,
    locale: Locale,
    has_comment: bool,
    filter: FilterMode,
    comments: Vec<LineRange>,
}

fn fr(label: DiffLabel<'_>) -> Option<String> {
    Some(match label {
        DiffLabel::List => "FR:Liste".into(),
        DiffLabel::Line { kind, number, text } => format!("FR:{kind:?}:{number}:{text}"),
        DiffLabel::CommentLine => "FR:Commenter".into(),
        DiffLabel::NoNewlineAtEndOfFile => "FR:PasDeFinDeLigne".into(),
        DiffLabel::UnchangedLines(n) => format!("FR:Inchangees({n})"),
        DiffLabel::UnmodifiedLines(n) => format!("FR:NonModifiees({n})"),
        DiffLabel::UnchangedLinesBefore { count, header } => format!("FR:Avant({count}):{header}"),
        DiffLabel::ShowHiddenLines(n) => format!("FR:MontrerCachees({n})"),
        DiffLabel::ShowMoreLinesAbove(n) => format!("FR:MontrerHaut({n})"),
        DiffLabel::ShowMoreLinesBelow(n) => format!("FR:MontrerBas({n})"),
        DiffLabel::ShowAllLines(n) => format!("FR:ToutMontrer({n})"),
        DiffLabel::ExpandFile => "FR:Deplier".into(),
        DiffLabel::CollapseFile => "FR:Replier".into(),
        DiffLabel::FileStatus(status) => format!("FR:Statut({status:?})"),
        DiffLabel::FileSummary { path, renamed_from, additions, deletions } => match renamed_from {
            Some(old) => format!("FR:Resume({path}<-{old},+{additions},-{deletions})"),
            None => format!("FR:Resume({path},+{additions},-{deletions})"),
        },
        DiffLabel::BinaryFile => "FR:Binaire".into(),
        DiffLabel::MovedWithoutChanges => "FR:Deplace".into(),
        DiffLabel::ModeChanged => "FR:Mode".into(),
        DiffLabel::EmptyFile => "FR:Vide".into(),
        DiffLabel::NoChanges => "FR:AucunChangement".into(),
        _ => return None,
    })
}

fn de(label: DiffLabel<'_>) -> Option<String> {
    Some(match label {
        DiffLabel::List => "DE:Liste".into(),
        DiffLabel::Line { kind, number, text } => format!("DE:{kind:?}:{number}:{text}"),
        DiffLabel::CommentLine => "DE:Kommentieren".into(),
        DiffLabel::NoNewlineAtEndOfFile => "DE:KeinZeilenumbruch".into(),
        DiffLabel::UnchangedLines(n) => format!("DE:Unveraendert({n})"),
        DiffLabel::UnmodifiedLines(n) => format!("DE:Unmodifiziert({n})"),
        DiffLabel::UnchangedLinesBefore { count, header } => format!("DE:Vor({count}):{header}"),
        DiffLabel::ShowHiddenLines(n) => format!("DE:Versteckt({n})"),
        DiffLabel::ShowMoreLinesAbove(n) => format!("DE:Oben({n})"),
        DiffLabel::ShowMoreLinesBelow(n) => format!("DE:Unten({n})"),
        DiffLabel::ShowAllLines(n) => format!("DE:Alle({n})"),
        DiffLabel::ExpandFile => "DE:Aufklappen".into(),
        DiffLabel::CollapseFile => "DE:Zuklappen".into(),
        DiffLabel::FileStatus(status) => format!("DE:Status({status:?})"),
        DiffLabel::FileSummary { path, renamed_from, additions, deletions } => match renamed_from {
            Some(old) => format!("DE:Summary({path}<-{old},+{additions},-{deletions})"),
            None => format!("DE:Summary({path},+{additions},-{deletions})"),
        },
        DiffLabel::BinaryFile => "DE:Binaer".into(),
        DiffLabel::MovedWithoutChanges => "DE:Verschoben".into(),
        DiffLabel::ModeChanged => "DE:Modus".into(),
        DiffLabel::EmptyFile => "DE:Leer".into(),
        DiffLabel::NoChanges => "DE:KeineAenderungen".into(),
        _ => return None,
    })
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    let mut diff = DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff).fold_row(s.fold_row);
    match s.locale {
        Locale::Default => {}
        Locale::French => diff = diff.labels(fr),
        Locale::German => diff = diff.labels(de),
        Locale::BlankFallback => {
            diff = diff.labels(|label| match label {
                DiffLabel::CommentLine => Some("Custom action".into()),
                DiffLabel::ExpandFile => Some("   ".into()),
                DiffLabel::CollapseFile => Some(String::new()),
                _ => None,
            });
        }
    }
    if s.has_comment {
        diff = diff.on_comment(|s: &mut App, lines, _| s.comments.push(lines));
    }
    match s.filter {
        FilterMode::None => {}
        FilterMode::NewOnly => diff = diff.comment_filter(|_, at| at.side == Side::New),
        FilterMode::OldOnly => diff = diff.comment_filter(|_, at| at.side == Side::Old),
        FilterMode::File1FromLine3 => diff = diff.comment_filter(|f, at| f == 1 && at.line >= 3),
        FilterMode::RejectAll => diff = diff.comment_filter(|_, _| false),
    }
    col().w_full().h_full().child(diff.build(s, &k))
}

fn mount(app: App, size: (f32, f32)) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, size, Env::new(true, 2.0));
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn tick(rt: &mut Runtime<App>) {
    rt.frame(1.0 / 60.0);
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..40 {
        tick(rt);
    }
}

fn find(rt: &Runtime<App>, end: &str) -> ElementInfo {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting).unwrap_or_else(|| panic!("no {end}"))
}

fn find_opt(rt: &Runtime<App>, end: &str) -> Option<ElementInfo> {
    let suffix = format!("/{end}");
    rt.elements().into_iter().find(|e| e.path.ends_with(&suffix) && !e.exiting)
}

fn press(rt: &mut Runtime<App>, end: &str) {
    let e = find(rt, end);
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    tick(rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    tick(rt);
}

#[test]
fn diff_labels_cover_every_variant_and_invalidate_memo_on_locale_switch() {
    // File 0: two hunks separated by 30 unchanged lines (> EXPAND_STEP) and a short trailing gap
    // of 5 unchanged lines (<= EXPAND_STEP), plus missing newline at EOF.
    let gap_long: String = (2..=37).map(|i| format!("mid {i}\n")).collect();
    let tail: String = (39..=46).map(|i| format!("tail {i}\n")).collect();
    let old = format!("old_first\n{gap_long}old_mid\n{tail}eof_old");
    let new = format!("new_first\n{gap_long}new_mid\n{tail}eof_new");
    let mut files = vec![FileDiff::from_texts("main.rs", &old, &new)];

    // Add patch files exercising every FileStatus and Note variant:
    // BinaryFile, MovedWithoutChanges, ModeChanged, EmptyFile (Added + Deleted), Copied, NoChanges.
    let patch = concat!(
        "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n",
        "diff --git a/old.rs b/moved.rs\nsimilarity index 100%\nrename from old.rs\nrename to moved.rs\n",
        "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n",
        "diff --git a/src.rs b/copied.rs\nsimilarity index 100%\ncopy from src.rs\ncopy to copied.rs\n",
    );
    files.extend(parse(patch));
    // Added/Deleted empty files (equal modes) exercise DiffLabel::EmptyFile and Added/Deleted status badges.
    files.push(FileDiff::between(None, Some("empty_new.txt".into()), "", ""));
    files.push(FileDiff::between(Some("empty_del.txt".into()), None, "", ""));
    // A Modified file with zero hunks exercises DiffLabel::NoChanges.
    files.push(FileDiff::from_texts("same.txt", "equal\n", "equal\n"));

    let app = App {
        diff: DiffState::new(files),
        fold_row: FoldRow::Summary,
        locale: Locale::French,
        has_comment: true,
        filter: FilterMode::None,
        comments: Vec::new(),
    };
    let mut rt = mount(app, (900.0, 2400.0));

    let els = rt.elements();
    let has_label = |els: &[ElementInfo], role: &str, l: &str| els.iter().any(|e| e.role == role && e.label.as_deref() == Some(l));
    let has_text = |els: &[ElementInfo], t: &str| els.iter().any(|e| e.text.as_deref() == Some(t));

    // List, file header, toggle, line rows, comment button, no-newline marker.
    assert!(has_label(&els, "list", "FR:Liste"));
    assert!(has_label(&els, "heading", "FR:Resume(main.rs,+3,-3)"));
    assert!(has_label(&els, "button", "FR:Replier"));
    assert!(has_label(&els, "listitem", "FR:Removed:1:old_first"));
    assert!(has_label(&els, "listitem", "FR:Added:1:new_first"));
    assert!(has_label(&els, "listitem", "FR:Context:2:mid 2"));
    assert!(has_label(&els, "button", "FR:Commenter"));
    assert!(has_text(&els, "FR:PasDeFinDeLigne"));

    // Summary fold row: long gap (> EXPAND_STEP) between hunks and short gap (<= EXPAND_STEP).
    assert!(has_text(&els, "FR:Inchangees(30)"));
    assert!(has_label(&els, "button", "FR:MontrerBas(20)"));
    assert!(has_label(&els, "button", "FR:MontrerHaut(20)"));
    assert!(has_label(&els, "button", "FR:ToutMontrer(30)"));
    assert!(els.iter().any(|e| e.role == "group" && e.label.as_deref().is_some_and(|l| l.starts_with("FR:Avant(30):@@ "))));
    assert!(has_text(&els, "FR:Inchangees(2)"));
    assert!(has_label(&els, "button", "FR:MontrerCachees(2)"));

    // Note rows & file statuses (Modified renders no status badge; Added/Deleted/Renamed/Copied do).
    assert!(has_label(&els, "note", "FR:Binaire"));
    assert!(has_text(&els, "FR:Binaire"));
    assert!(has_label(&els, "note", "FR:Deplace"));
    assert!(has_label(&els, "note", "FR:Mode"));
    assert!(has_label(&els, "note", "FR:Vide"));
    assert!(has_label(&els, "note", "FR:AucunChangement"));
    assert!(has_text(&els, "FR:Statut(Renamed)"));
    assert!(has_text(&els, "FR:Statut(Added)"));
    assert!(has_text(&els, "FR:Statut(Deleted)"));
    assert!(has_text(&els, "FR:Statut(Copied)"));
    assert!(!has_text(&els, "FR:Statut(Modified)"), "Modified files show no status badge");
    assert!(has_label(&els, "heading", "FR:Resume(moved.rs<-old.rs,+0,-0)"));

    // Live locale switch to German WITHOUT mutating DiffState invalidates every memoized row.
    rt.state.locale = Locale::German;
    rt.invalidate();
    settle(&mut rt);
    let els = rt.elements();
    assert!(has_label(&els, "list", "DE:Liste"));
    assert!(has_label(&els, "listitem", "DE:Added:1:new_first"));
    assert!(has_label(&els, "button", "DE:Kommentieren"));
    assert!(has_text(&els, "DE:KeinZeilenumbruch"));
    assert!(has_text(&els, "DE:Unveraendert(30)"));
    assert!(has_label(&els, "button", "DE:Alle(30)"));
    assert!(has_label(&els, "note", "DE:Binaer"));
    assert!(has_label(&els, "note", "DE:KeineAenderungen"));
    assert!(!els.iter().any(|e| e.label.as_deref().is_some_and(|l| l.starts_with("FR:"))), "no stale FR labels remain");

    // LineInfo fold row uses UnmodifiedLines and ShowMore/ShowHidden buttons, and updates live.
    rt.state.fold_row = FoldRow::LineInfo;
    rt.invalidate();
    settle(&mut rt);
    let els = rt.elements();
    assert!(has_text(&els, "DE:Unmodifiziert(30)"));
    assert!(has_label(&els, "group", "DE:Unmodifiziert(30)"));
    assert!(has_label(&els, "button", "DE:Oben(20)"));
    assert!(has_label(&els, "button", "DE:Unten(20)"));
    assert!(has_label(&els, "button", "DE:Versteckt(2)"));

    // Switch back to default English without mutating DiffState: every memoized row returns to English.
    rt.state.locale = Locale::Default;
    rt.invalidate();
    settle(&mut rt);
    let els = rt.elements();
    assert!(has_label(&els, "list", "Diff"));
    assert!(has_label(&els, "listitem", "added line 1: new_first"));
    assert!(has_label(&els, "button", "Comment on this line"));
    assert!(has_text(&els, "No newline at end of file"));
    assert!(has_text(&els, "30 unmodified lines"));
    assert!(has_label(&els, "note", "Binary file not shown"));
    assert!(has_label(&els, "note", "No changes"));
}

#[test]
fn partial_and_blank_labels_fall_back_to_english_defaults() {
    let app = App {
        diff: DiffState::new(vec![FileDiff::from_texts("a.txt", "old\n", "new\n")]),
        fold_row: FoldRow::Summary,
        locale: Locale::BlankFallback,
        has_comment: true,
        filter: FilterMode::None,
        comments: Vec::new(),
    };
    let mut rt = mount(app, (800.0, 400.0));
    let els = rt.elements();
    // Overridden variant uses custom text.
    assert!(els.iter().any(|e| e.role == "button" && e.label.as_deref() == Some("Custom action")));
    // Blank strings and None fall back to exact English defaults.
    assert!(els.iter().any(|e| e.role == "list" && e.label.as_deref() == Some("Diff")));
    assert!(els.iter().any(|e| e.role == "button" && e.label.as_deref() == Some("Collapse file")));

    // Toggle collapse and verify blank ExpandFile also falls back to "Expand file".
    press(&mut rt, "df0/inner/toggle");
    settle(&mut rt);
    assert!(rt.elements().iter().any(|e| e.role == "button" && e.label.as_deref() == Some("Expand file")));
}

#[test]
fn empty_diff_with_custom_labels_and_comment_filter_builds_cleanly() {
    let app = App {
        diff: DiffState::new(Vec::new()),
        fold_row: FoldRow::Summary,
        locale: Locale::French,
        has_comment: true,
        filter: FilterMode::NewOnly,
        comments: Vec::new(),
    };
    let rt = mount(app, (600.0, 300.0));
    let els = rt.elements();
    assert!(els.iter().any(|e| e.role == "list" && e.label.as_deref() == Some("FR:Liste")));
    assert!(!els.iter().any(|e| e.role == "listitem"));
}

#[test]
fn singular_fold_counts_and_trailing_gap_use_unchanged_and_unmodified_labels() {
    // 8 lines, change at line 1 -> hunk covers lines 1..=4, leaving 1 line (line 5) hidden if we
    // have 5 lines: lines 1..=4 in hunk (1 changed + 3 context) and line 5 is a 1-line trailing gap!
    let old = "old\nl2\nl3\nl4\nl5\n";
    let new = "new\nl2\nl3\nl4\nl5\n";
    let app = App {
        diff: DiffState::new(vec![FileDiff::from_texts("one.txt", old, new)]),
        fold_row: FoldRow::Summary,
        locale: Locale::Default,
        has_comment: false,
        filter: FilterMode::None,
        comments: Vec::new(),
    };
    let mut rt = mount(app, (800.0, 400.0));
    let els = rt.elements();
    assert!(els.iter().any(|e| e.text.as_deref() == Some("1 unchanged line")));
    assert!(els.iter().any(|e| e.role == "group" && e.label.as_deref() == Some("1 unchanged line")));

    rt.state.fold_row = FoldRow::LineInfo;
    rt.invalidate();
    settle(&mut rt);
    let els = rt.elements();
    assert!(els.iter().any(|e| e.text.as_deref() == Some("1 unmodified line")));
    assert!(els.iter().any(|e| e.role == "group" && e.label.as_deref() == Some("1 unmodified line")));
}

#[test]
fn comment_filter_restricts_plus_in_unified_and_split_and_updates_cached_rows() {
    // Line 1: context ("ctx1"), Line 2: removed ("old2") / added ("new2"), Line 3: context ("ctx3").
    // Add an extra deleted line before line 3 so old and new line numbers diverge on ctx3!
    let old = "ctx1\nold2a\nold2b\nctx3\n";
    let new = "ctx1\nnew2\nctx3\n";
    let mut rt = mount(
        App {
            diff: DiffState::new(vec![FileDiff::from_texts("a.txt", old, new)]),
            fold_row: FoldRow::Summary,
            locale: Locale::Default,
            has_comment: true,
            filter: FilterMode::None,
            comments: Vec::new(),
        },
        (800.0, 500.0),
    );

    // Default (no filter): unified shows + on removed, added, and context lines.
    // Note: context rows are keyed by their old line number, so ctx3 (old 4, new 3) is df0.c4.
    assert!(find_opt(&rt, "df0.o2/gutter/plus").is_some());
    assert!(find_opt(&rt, "df0.n2/gutter/plus").is_some());
    assert!(find_opt(&rt, "df0.c4/gutter/plus").is_some());
    press(&mut rt, "df0.o2/gutter/plus");
    assert_eq!(rt.state.comments.last(), Some(&LineRange::line(0, LineRef::new(Side::Old, 2))));

    // Live switch to NewOnly (without mutating DiffState): removed lines drop +, added/context keep +.
    rt.state.filter = FilterMode::NewOnly;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0.o2/gutter/plus").is_none(), "removed line 2 omits +");
    assert!(find_opt(&rt, "df0.o3/gutter/plus").is_none(), "removed line 3 omits +");
    assert!(find_opt(&rt, "df0.n2/gutter/plus").is_some(), "added line 2 keeps +");
    assert!(find_opt(&rt, "df0.c4/gutter/plus").is_some(), "context line keeps + on Side::New");
    press(&mut rt, "df0.n2/gutter/plus");
    assert_eq!(rt.state.comments.last(), Some(&LineRange::line(0, LineRef::new(Side::New, 2))));
    press(&mut rt, "df0.c4/gutter/plus");
    assert_eq!(rt.state.comments.last(), Some(&LineRange::line(0, LineRef::new(Side::New, 3))));

    // Live switch to OldOnly: added line drops +, removed keeps +, context falls back to Side::Old (old line 4!).
    rt.state.filter = FilterMode::OldOnly;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0.n2/gutter/plus").is_none(), "added line omits +");
    assert!(find_opt(&rt, "df0.o2/gutter/plus").is_some(), "removed line keeps +");
    assert!(find_opt(&rt, "df0.c4/gutter/plus").is_some(), "context line falls back to Side::Old");
    press(&mut rt, "df0.c4/gutter/plus");
    assert_eq!(rt.state.comments.last(), Some(&LineRange::line(0, LineRef::new(Side::Old, 4))));

    // Live switch to RejectAll: zero + buttons anywhere.
    rt.state.filter = FilterMode::RejectAll;
    rt.invalidate();
    settle(&mut rt);
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("plus") && !e.exiting));

    // Split mode: check default, NewOnly, OldOnly, and selection-range commenting on an eligible line.
    rt.state.diff.set_mode(DiffMode::Split);
    rt.state.filter = FilterMode::None;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0.o2/old/plus").is_some());
    assert!(find_opt(&rt, "df0.o2/new/plus").is_some());
    assert!(find_opt(&rt, "df0.c4/old/plus").is_some());
    assert!(find_opt(&rt, "df0.c4/new/plus").is_some());

    rt.state.filter = FilterMode::NewOnly;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0.o2/old/plus").is_none(), "split old half omits +");
    assert!(find_opt(&rt, "df0.o3/old/plus").is_none(), "split unpaired removed line omits +");
    assert!(find_opt(&rt, "df0.c4/old/plus").is_none(), "split context old half omits +");
    assert!(find_opt(&rt, "df0.o2/new/plus").is_some(), "split new half keeps +");
    assert!(find_opt(&rt, "df0.c4/new/plus").is_some(), "split context new half keeps +");
    let sel = LineRange { file: 0, start: LineRef::new(Side::New, 2), end: LineRef::new(Side::New, 3) };
    rt.state.diff.set_selection(Some(sel));
    rt.invalidate();
    settle(&mut rt);
    press(&mut rt, "df0.o2/new/plus");
    assert_eq!(rt.state.comments.last(), Some(&sel), "eligible + inside selection comments on the range");

    rt.state.diff.set_selection(None);
    rt.state.filter = FilterMode::OldOnly;
    rt.invalidate();
    settle(&mut rt);
    assert!(find_opt(&rt, "df0.o2/old/plus").is_some());
    assert!(find_opt(&rt, "df0.o2/new/plus").is_none());
    assert!(find_opt(&rt, "df0.c4/old/plus").is_some());
    assert!(find_opt(&rt, "df0.c4/new/plus").is_none());
    press(&mut rt, "df0.c4/old/plus");
    assert_eq!(rt.state.comments.last(), Some(&LineRange::line(0, LineRef::new(Side::Old, 4))));
}

#[test]
fn comment_filter_supports_per_file_and_line_predicates_and_ignores_without_on_comment() {
    let files = vec![
        FileDiff::from_texts("f0.txt", "a\nb\nc\nd\n", "a\nB\nc\nD\n"),
        FileDiff::from_texts("f1.txt", "a\nb\nc\nd\n", "a\nB\nc\nD\n"),
    ];
    let mut rt = mount(
        App {
            diff: DiffState::new(files),
            fold_row: FoldRow::Summary,
            locale: Locale::Default,
            has_comment: true,
            filter: FilterMode::File1FromLine3,
            comments: Vec::new(),
        },
        (800.0, 900.0),
    );

    // File 0 gets no + buttons; file 1 gets + only on lines >= 3.
    assert!(find_opt(&rt, "df0.n2/gutter/plus").is_none());
    assert!(find_opt(&rt, "df0.n4/gutter/plus").is_none());
    assert!(find_opt(&rt, "df1.n2/gutter/plus").is_none());
    assert!(find_opt(&rt, "df1.c3/gutter/plus").is_some());
    assert!(find_opt(&rt, "df1.n4/gutter/plus").is_some());
    press(&mut rt, "df1.n4/gutter/plus");
    assert_eq!(rt.state.comments, vec![LineRange::line(1, LineRef::new(Side::New, 4))]);

    // Without on_comment, no + buttons appear even if comment_filter is installed.
    rt.state.has_comment = false;
    rt.invalidate();
    settle(&mut rt);
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("plus") && !e.exiting));
}

#[test]
fn custom_labels_and_comment_filter_keep_live_only_events_clean() {
    let old: String = (1..=30).map(|i| format!("let v{i} = {i};\n")).collect();
    let new: String = (1..=30).map(|i| if i % 5 == 0 { format!("let v{i} = {i} + 1;\n") } else { format!("let v{i} = {i};\n") }).collect();
    let mut rt = mount(
        App {
            diff: DiffState::new(vec![FileDiff::from_texts("a.rs", &old, &new)]),
            fold_row: FoldRow::Summary,
            locale: Locale::French,
            has_comment: true,
            filter: FilterMode::NewOnly,
            comments: Vec::new(),
        },
        (900.0, 600.0),
    );
    rt.on_key = Some(|_, _, tx| {
        tx.live_only();
        true
    });
    rt.check_live_only(true);
    rt.frame(0.3);
    rt.key(KeyEvent { key: Key::Function(5), mods: Mods::default() });
    tick(&mut rt);
    assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
}
