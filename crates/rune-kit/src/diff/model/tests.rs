use super::*;

#[test]
fn texts_make_hunks_with_three_lines_of_context() {
    let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let new = old.replace("line 5\n", "line five\n").replace("line 17\n", "").replace("line 20\n", "line 20\nline 21\n");
    let f = FileDiff::from_texts("a.txt", &old, &new);
    let headers: Vec<String> = f.hunks.iter().map(Hunk::header).collect();
    assert_eq!(headers, vec!["@@ -2,7 +2,7 @@ line 1", "@@ -14,7 +14,7 @@ line 13"]);
    let second: Vec<(LineKind, Option<u32>, Option<u32>)> = f.hunks[1].lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
    assert_eq!(second[3], (LineKind::Removed, Some(17), None));
    assert_eq!(second.last(), Some(&(LineKind::Added, None, Some(20))));
    assert_eq!((f.additions(), f.deletions()), (2, 2));
    let five = &f.hunks[0].lines;
    let added = five.iter().find(|l| l.kind == LineKind::Added).unwrap();
    assert_eq!(&added.text[added.changes[0].clone()], "five");
}

#[test]
fn a_missing_final_newline_is_a_change() {
    let f = FileDiff::from_texts("a", "one\ntwo", "one\ntwo\n");
    let l: Vec<(LineKind, &str, bool)> = f.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str(), l.no_newline)).collect();
    assert_eq!(l, vec![(LineKind::Context, "one", false), (LineKind::Removed, "two", true), (LineKind::Added, "two", false)]);
    let both_missing = FileDiff::from_texts("a", "one\ntwo", "one\nthree");
    let l2: Vec<(LineKind, &str, bool)> = both_missing.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str(), l.no_newline)).collect();
    assert_eq!(l2, vec![(LineKind::Context, "one", false), (LineKind::Removed, "two", true), (LineKind::Added, "three", true)]);
    assert!(FileDiff::from_texts("a", "same\n", "same\n").hunks.is_empty());
    assert!(FileDiff::from_texts("a", "same", "same").hunks.is_empty());
    // CRLF normalizes line endings while preserving changes.
    assert!(FileDiff::from_texts("a", "one\r\ntwo\r\n", "one\ntwo\n").hunks.is_empty());
    let crlf = FileDiff::from_texts("a", "one\r\ntwo\r\n", "one\r\nthree\r\n");
    assert_eq!(crlf.hunks[0].lines[1].text, "two");
    assert_eq!(crlf.hunks[0].lines[2].text, "three");
    // NUL bytes mark a binary file.
    let bin = FileDiff::from_texts("img.bin", "a\0b", "c\0d");
    assert!(bin.binary && bin.hunks.is_empty());
}

#[test]
fn hostile_and_fuzzed_patches_never_panic_or_blow_up() {
    let check_state = |state: &mut crate::diff::DiffState| {
        for mode in [crate::diff::DiffMode::Unified, crate::diff::DiffMode::Split] {
            state.set_mode(mode);
            let unique: std::collections::HashSet<&str> = state.keys.iter().map(String::as_str).collect();
            assert_eq!(unique.len(), state.keys.len(), "duplicate virtual row keys: {:?}", state.keys);
            for &r in &state.rows {
                let f = r.file();
                for side in [Side::Old, Side::New] {
                    if let Some(n) = state.row_line(r, side) {
                        let _ = state.side_line(f, side, n);
                        let _ = state.highlights(f, side, n);
                    }
                }
            }
        }
    };
    let cases = [
        "",
        "diff --git\n--- \n+++ \n@@ -4294967295,4294967295 +4294967295,4294967295 @@\n-a\n+b\n\\ No newline",
        "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -4294967295,2 +4294967295,2 @@\n let a = 1;\n-let b = 2;\n+let c = 3;\n let d = 4;\n",
        "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n let a = 1;\n-old\n+new\n@@ -1,2 +1,2 @@\n let a = 1;\n-old2\n+new2\n",
        "diff --git \"a/unclosed b/unclosed\n--- \"a/\\777\\x\n+++ b/\n@@ -0,0 +0,0 @@\n",
        "diff --git 🚀 a/x 🚀 b/y\n@@ -1,999999 +1,999999 @@\n+truncated",
        "--- a/f\n+++ b/f\n@@ -10,2 +10,2 @@\n ctx\n-old\n+new\n@@ -2,1 +2,1 @@\n-out_of_order\n+ok\n",
    ];
    for patch in cases {
        let files = parse(patch);
        let mut state = crate::diff::DiffState::new(files);
        check_state(&mut state);
    }

    // Fuzz-style random line soup of headers, hunks, escapes and markers.
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed as usize) % n
    };
    let fragments = [
        "diff --git a/f.rs b/f.rs\n",
        "diff --git \"a/sp ace\\303\\251\" \"b/sp ace\\n\"\n",
        "--- a/f.rs\n",
        "+++ b/f.rs\n",
        "--- /dev/null\n",
        "+++ /dev/null\n",
        "@@ -1,3 +1,3 @@ fn f()\n",
        "@@ -0,0 +1,5 @@\n",
        "@@ -4294967295,10 +0,4294967295 @@\n",
        "@@ malformed @@\n",
        " context line\n",
        "-removed line\n",
        "+added line\n",
        "\\ No newline at end of file\n",
        "rename from \"a/\\123\"\n",
        "similarity index 999%\n",
        "Binary files a/x and b/x differ\n",
        "\r\n",
    ];
    for _ in 0..300 {
        let len = 1 + next(24);
        let patch: String = (0..len).map(|_| fragments[next(fragments.len())]).collect();
        let files = parse(&patch);
        let mut state = crate::diff::DiffState::new(files);
        check_state(&mut state);
        for f in 0..state.files().len() {
            let _ = state.side_line(f, Side::Old, 1);
            let _ = state.side_line(f, Side::New, 1);
            let _ = state.side_line(f, Side::Old, u32::MAX);
            let _ = state.side_line(f, Side::New, u32::MAX);
        }
    }
}

#[test]
fn an_added_file_and_an_insertion_count_from_zero() {
    let f = FileDiff::between(None, Some("new.rs".into()), "", "fn main() {}\n");
    assert_eq!((f.status, f.hunks[0].header()), (FileStatus::Added, "@@ -0,0 +1 @@".to_owned()));
    let reparsed = parse("--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+fn main() {}\n");
    assert_eq!(reparsed[0].hunks[0].lines, f.hunks[0].lines);
    assert_eq!(reparsed[0].status, FileStatus::Added);
}

#[test]
fn languages_come_from_the_extension() {
    let f = |p: &str| FileDiff::from_texts(p, "", "x").language().map(str::to_owned);
    assert_eq!(f("src/lib.rs").as_deref(), Some("rust"));
    assert_eq!(f("web/App.tsx").as_deref(), Some("tsx"));
    assert_eq!(f(".gitignore"), None);
    assert_eq!(f("Makefile").as_deref(), Some("makefile"));
}

#[test]
fn diff_options_default_matches_existing_constructors() {
    assert_eq!(DiffOptions::default(), DiffOptions::new());
    let old = "   keep\n  old\n";
    let new = "\tkeep\n  new\n";
    let plain = FileDiff::from_texts("a.txt", old, new);
    let with_default = FileDiff::from_texts_with("a.txt", old, new, DiffOptions::default());
    assert_eq!(plain.hunks, with_default.hunks);
    assert_eq!((plain.additions(), plain.deletions()), (2, 2));

    let renamed = FileDiff::between_with(Some("old.txt".into()), Some("new.txt".into()), old, new, DiffOptions::new().context(0));
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(renamed.hunks.len(), 1);
}

#[test]
fn ignore_edge_whitespace_ignores_leading_and_trailing_spaces_tabs_but_not_interior_or_eof() {
    let opts = DiffOptions::new().ignore_edge_whitespace(true);
    // Leading spaces vs tab become context; real change stays a change; per-side text is kept.
    let f = FileDiff::from_texts_with("a.txt", "   keep\n  old\n", "\tkeep\n  new\n", opts);
    assert_eq!((f.additions(), f.deletions()), (1, 1));
    assert_eq!(f.hunks[0].lines[0].kind, LineKind::Context);
    assert_eq!(f.side_text_line(Side::Old, 1), Some("   keep"));
    assert_eq!(f.side_text_line(Side::New, 1), Some("\tkeep"));
    assert_eq!(f.side_text_line(Side::Old, 2), Some("  old"));
    assert_eq!(f.side_text_line(Side::New, 2), Some("  new"));
    assert_eq!(f.side_text_line(Side::Old, 0), None);
    assert_eq!(f.side_text_line(Side::Old, 3), None);

    // Trailing spaces/tabs and whitespace-only lines match.
    let trailing = FileDiff::from_texts_with("a.txt", "keep   \n   \nold\n", "keep\t \n\t\nnew\n", opts);
    assert_eq!((trailing.additions(), trailing.deletions()), (1, 1));
    assert_eq!(trailing.hunks[0].lines[0].kind, LineKind::Context);
    assert_eq!(trailing.hunks[0].lines[1].kind, LineKind::Context);
    assert_eq!(trailing.side_text_line(Side::Old, 1), Some("keep   "));
    assert_eq!(trailing.side_text_line(Side::New, 1), Some("keep\t "));

    // Interior whitespace changes remain changes.
    let interior = FileDiff::from_texts_with("a.txt", "a b\n", "a  b\n", opts);
    assert_eq!((interior.additions(), interior.deletions()), (1, 1));
    let interior_tab = FileDiff::from_texts_with("a.txt", "a b\n", "a\tb\n", opts);
    assert_eq!((interior_tab.additions(), interior_tab.deletions()), (1, 1));

    // Non-ASCII whitespace (NBSP) is not ASCII space/tab and stays a change.
    let nbsp = FileDiff::from_texts_with("a.txt", "\u{00A0}keep\n", "keep\n", opts);
    assert_eq!((nbsp.additions(), nbsp.deletions()), (1, 1));

    // Missing final newline still differs even when edge whitespace matches.
    let eof = FileDiff::from_texts_with("a.txt", "   keep", "\tkeep\n", opts);
    assert_eq!((eof.additions(), eof.deletions()), (1, 1));
    let both_eof = FileDiff::from_texts_with("a.txt", "   keep", "\tkeep", opts);
    assert!(both_eof.hunks.is_empty());

    // Empty and binary inputs, plus parsed patches returning None for side_text_line.
    assert!(FileDiff::from_texts_with("empty.txt", "", "", opts).hunks.is_empty());
    assert!(FileDiff::from_texts_with("bin.dat", "a\0b", "c\0d", opts).binary);
    let parsed = parse("--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n");
    assert_eq!(parsed[0].side_text_line(Side::Old, 1), None);
    assert_eq!(parsed[0].side_text_line(Side::New, 1), None);
}
