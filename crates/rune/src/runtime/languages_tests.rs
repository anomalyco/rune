use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{Cx, El, Env, Runtime, col, memo, text};

#[derive(Default)]
struct Capture {
    runs: Cell<usize>,
    memo_builds: Cell<usize>,
    languages: RefCell<Vec<String>>,
    locale: RefCell<Option<String>>,
    retyped_locale: RefCell<Option<String>>,
}

fn capture_view(s: &Rc<Capture>, cx: &mut Cx<Rc<Capture>>) -> El {
    s.runs.set(s.runs.get() + 1);
    *s.languages.borrow_mut() = cx.system_languages().to_vec();
    *s.locale.borrow_mut() = cx.locale().map(str::to_owned);
    let sub: Cx<()> = cx.retype();
    *s.retyped_locale.borrow_mut() = sub.locale().map(str::to_owned);
    let label = memo("lang-label", 1u32, || {
        s.memo_builds.set(s.memo_builds.get() + 1);
        text(cx.locale().unwrap_or("none").to_owned())
    });
    col().size(200.0, 80.0).child(label)
}

#[test]
fn set_languages_updates_cx_accessors_retype_and_memos_without_dirtying_identical_sets() {
    let cap = Rc::new(Capture::default());
    let mut rt = Runtime::new(cap.clone(), capture_view, (200.0, 80.0), Env::default());
    rt.set_languages(["  ar-EG ", "", "en-US", "ar-EG", "   "]);
    rt.frame(0.0);

    assert_eq!(*cap.languages.borrow(), vec!["ar-EG", "en-US"]);
    assert_eq!(cap.locale.borrow().as_deref(), Some("ar-EG"));
    assert_eq!(cap.retyped_locale.borrow().as_deref(), Some("ar-EG"));
    assert_eq!(cap.runs.get(), 1);
    assert_eq!(cap.memo_builds.get(), 1);

    // Re-applying the same normalized language list must not dirty an idle runtime.
    rt.set_languages(["ar-EG", "en-US"]);
    assert!(!rt.needs_commit(), "identical set_languages must not dirty an idle runtime");
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(cap.runs.get(), 1);
    assert_eq!(cap.memo_builds.get(), 1);

    // Switching languages dirties the view and rebuilds memos.
    rt.set_languages(["he-IL", "en-GB"]);
    assert!(rt.needs_commit(), "changed set_languages must schedule a commit");
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(*cap.languages.borrow(), vec!["he-IL", "en-GB"]);
    assert_eq!(cap.locale.borrow().as_deref(), Some("he-IL"));
    assert_eq!(cap.runs.get(), 2);
    assert_eq!(cap.memo_builds.get(), 2);

    // Setting an empty (or whitespace-only) list yields empty slice and None locale.
    rt.set_languages(["", "  "]);
    assert!(rt.needs_commit());
    assert!(!rt.frame(1.0 / 60.0));
    assert!(cap.languages.borrow().is_empty());
    assert_eq!(*cap.locale.borrow(), None);
    assert_eq!(cap.runs.get(), 3);
    assert_eq!(cap.memo_builds.get(), 3);

    // Repeating an empty list keeps the runtime idle.
    rt.set_languages(std::iter::empty::<&str>());
    assert!(!rt.needs_commit());
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(cap.runs.get(), 3);
}

#[test]
fn unset_languages_lazily_reads_platform_languages_and_unused_views_stay_idle() {
    let seen_runs = Rc::new(Cell::new(0usize));
    let mut unused = Runtime::new(
        seen_runs.clone(),
        |runs: &Rc<Cell<usize>>, _: &mut Cx<Rc<Cell<usize>>>| {
            runs.set(runs.get() + 1);
            col().size(120.0, 40.0).child(text("plain"))
        },
        (120.0, 40.0),
        Env::default(),
    );
    unused.frame(0.0);
    assert_eq!(seen_runs.get(), 1);
    assert!(!unused.frame(1.0 / 60.0), "view that ignores locale stays idle");
    assert_eq!(seen_runs.get(), 1);

    let cap = Rc::new(Capture::default());
    let mut rt = Runtime::new(cap.clone(), capture_view, (200.0, 80.0), Env::default());
    rt.frame(0.0);
    assert_eq!(cap.locale.borrow().as_deref(), cap.languages.borrow().first().map(String::as_str));
    for tag in cap.languages.borrow().iter() {
        assert!(!tag.is_empty());
        assert_eq!(tag.trim(), tag);
    }
}
