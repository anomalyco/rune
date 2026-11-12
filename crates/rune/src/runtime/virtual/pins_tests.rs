//! The pins state against origin/dev's separate fields: every order of a selection change, a
//! live pin, a commit, a builder panic and a remount fallback, compared after every event.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use super::{Pins, PinsEvent, Source};
use crate::runtime::selectable::virtual_selection::{
    collect_row_into_pins, collect_virtual_row_selection,
};
use crate::El;

/// The fixture's offscreen rows, in child order, with their indices. A longer list keys its
/// later rows `r{i}`.
const ROWS: [&str; 3] = ["a", "b", "c"];

fn index_of(key: &str) -> usize {
    ROWS.iter()
        .position(|r| *r == key)
        .or_else(|| key.strip_prefix('r')?.parse().ok())
        .expect("a fixture row")
}

fn key_of(i: usize) -> String {
    ROWS.get(i).map_or_else(|| format!("r{i}"), |key| (*key).to_owned())
}

/// The row a builder returns: one selectable text with a copy provider on the row.
fn row_el(key: &str) -> El {
    crate::col()
        .key(key.to_owned())
        .selection_copy(|_| None)
        .child(crate::text("text").key("t0").selectable(true))
}

/// Collects `key`'s texts the way a commit does, into the state itself.
fn write(pins: &mut Pins, key: &str) {
    collect_row_into_pins(pins, &row_el(key), "/list", key, index_of(key), None, false);
}

/// The texts and provider `row_el` gives for `key`, written out rather than run through the
/// collector, so the reference never calls production code.
fn collected(key: &str) -> (Vec<(String, String)>, Vec<String>) {
    (vec![(key.to_owned(), format!("/list/{key}/t0"))], vec![format!("/list/{key}")])
}

fn source(name: &'static str) -> Source {
    match name {
        "rebuild" => Source::Rebuild(Rc::new(crate::el::virtual_rows::RowSource::new::<()>(
            0,
            |_, _| "".into(),
            |_, _| crate::el(),
        ))),
        "stored" => Source::Stored,
        _ => unreachable!("{name}"),
    }
}

fn source_name(source: &Source) -> &'static str {
    if source.rebuild().is_some() { "rebuild" } else { "stored" }
}

/// Which commit dev runs, since each fills the kept rows in its own order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `populate_virtual_children` for `El::virtual_rows`: a focused row first, then pinned rows.
    Indexed,
    /// `populate_virtual_children` for rows the view built: every kept child in child order.
    Stored,
    /// `remount_virtuals`: a focused row first, then pinned rows, keeping the source.
    Remount,
}

/// One thing that can change what a list keeps.
#[derive(Clone, Copy)]
enum Ev {
    Clear,
    Select(Option<&'static str>, Option<&'static str>),
    Pin(&'static str),
    Commit(Kind, Option<&'static str>),
    /// A row builder panics at build step `at`; `catch_unwind` catches it like the runtime does.
    Panic(Kind, Option<&'static str>, usize),
    /// A remount falls back when an entering row holds a nested virtual list, either in this list
    /// or in one before it.
    Fallback(Boundary),
}

/// Where a remount's fallback falls relative to this list. It is found among a list's entering
/// rows, which build after that list's focused and pinned rows, and it skips every later list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// This list's entering row (with the given focused row): its pins are already collected.
    Here(Option<&'static str>),
    /// An earlier list's entering row: this list was cleared while planning and builds nothing.
    Earlier,
}

const EVENTS: [Ev; 12] = [
    Ev::Clear,
    Ev::Select(Some("a"), Some("b")),
    Ev::Select(Some("a"), None),
    Ev::Pin("a"),
    Ev::Pin("b"),
    Ev::Commit(Kind::Indexed, None),
    Ev::Commit(Kind::Indexed, Some("b")),
    Ev::Commit(Kind::Stored, None),
    Ev::Commit(Kind::Remount, None),
    Ev::Panic(Kind::Stored, None, 1),
    Ev::Fallback(Boundary::Here(Some("b"))),
    Ev::Fallback(Boundary::Earlier),
];

fn name(event: Ev) -> &'static str {
    match event {
        Ev::Clear => "clear",
        Ev::Select(Some("a"), Some("b")) => "select(a,b)",
        Ev::Select(Some("a"), None) => "select(a,-)",
        Ev::Select(..) => "select(-,-)",
        Ev::Pin("a") => "pin(a)",
        Ev::Pin(_) => "pin(b)",
        Ev::Commit(Kind::Indexed, None) => "commit(indexed)",
        Ev::Commit(Kind::Indexed, _) => "commit(indexed,b)",
        Ev::Commit(Kind::Stored, _) => "commit(stored)",
        Ev::Commit(Kind::Remount, _) => "commit(remount)",
        Ev::Panic(Kind::Indexed, ..) => "panic(indexed,1)",
        Ev::Panic(Kind::Stored, ..) => "panic(stored,1)",
        Ev::Panic(..) => "panic(remount,1)",
        Ev::Fallback(Boundary::Here(_)) => "fallback(here,b)",
        Ev::Fallback(Boundary::Earlier) => "fallback(earlier)",
    }
}

/// The steps a commit builds, in dev's order.
fn steps(pins: &Pins, kind: Kind, focused: Option<&str>) -> Vec<String> {
    match kind {
        // `measure.rs` enumerates the stored children, so the kept texts come in child order
        // however the pins were made.
        Kind::Stored => ROWS.iter().map(|r| (*r).to_owned()).collect(),
        // `measure.rs` and `remount.rs` build the focused row first, then the pinned rows in pin
        // order (anchor then focus), skipping the focused one again.
        Kind::Indexed | Kind::Remount => {
            let mut steps: Vec<String> = focused.map(str::to_owned).into_iter().collect();
            steps.extend(pins.rows().iter().filter(|r| Some(r.as_str()) != focused).cloned());
            steps
        }
    }
}

/// One commit through the state under test, stopping at `stop` the way a panic does: what it
/// already wrote stays in the state.
fn commit(pins: &mut Pins, kind: Kind, focused: Option<&'static str>, stop: Option<usize>) {
    pins.clear_texts();
    if kind == Kind::Stored {
        // `measure.rs` installs the stored source before the children loop.
        pins.set_source(source("stored"));
    }
    for (step, key) in steps(pins, kind, focused).into_iter().enumerate() {
        if stop == Some(step) {
            let _ = catch_unwind(AssertUnwindSafe(|| panic!("row builder")));
            return;
        }
        if pins.rows().iter().any(|r| *r == key) {
            write(pins, &key);
        }
    }
    if kind == Kind::Indexed {
        pins.set_source(source("rebuild"));
    }
}

fn apply(pins: &mut Pins, event: Ev) {
    match event {
        Ev::Clear => pins.sync(PinsEvent::Clear),
        Ev::Select(anchor, focus) => pins.sync(PinsEvent::Select { anchor, focus }),
        Ev::Pin(key) => {
            let (texts, providers) = if pins.has_text_row(key) {
                (Vec::new(), Vec::new())
            } else {
                let (mut texts, mut providers) = (Vec::new(), Vec::new());
                collect_virtual_row_selection(
                    &row_el(key), "/list", key, index_of(key), None, false, &mut texts, &mut providers,
                );
                (texts, providers)
            };
            pins.sync(PinsEvent::Pin { key: key.to_owned(), texts, providers });
        }
        Ev::Commit(kind, focused) => commit(pins, kind, focused, None),
        Ev::Panic(kind, focused, at) => commit(pins, kind, focused, Some(at)),
        // The remount collects this list's rows and then meets the nested list among its
        // entering rows, which write nothing to the pins.
        Ev::Fallback(Boundary::Here(focused)) => commit(pins, Kind::Remount, focused, None),
        // Planning cleared this list; the loop broke out before reaching it.
        Ev::Fallback(Boundary::Earlier) => pins.clear_texts(),
    }
}

/// origin/dev's pin state, transcribed as it was: `VirtualList::pinned_rows` beside
/// `PinnedSelection { texts, copy_providers, source }`, cleared and filled by
/// `sync_selection_pins`, `populate_virtual_children`, `remount_virtuals` and
/// `pin_offscreen_row`. It is never edited to fit the branch: a failed walk means the branch
/// differs from dev.
#[derive(Default)]
struct Dev {
    pinned_rows: Vec<String>,
    /// Each kept text as `(row key, path)`, which is all dev's state decides for a walk.
    texts: Vec<(String, String)>,
    providers: Vec<String>,
    source: Option<&'static str>,
}

impl Dev {
    fn push(&mut self, key: &str) {
        let (texts, providers) = collected(key);
        self.texts.extend(texts);
        self.providers.extend(providers);
    }

    fn pinned_indices(&self) -> Vec<usize> {
        self.pinned_rows.iter().map(|r| index_of(r)).collect()
    }

    fn apply(&mut self, event: Ev) {
        match event {
            // `clear_text_selection` -> `sync_selection_pins` with no selection.
            Ev::Clear => {
                self.pinned_rows.clear();
                self.texts.clear();
                self.providers.clear();
            }
            // `sync_selection_pins` with a selection.
            Ev::Select(anchor, focus) => {
                self.pinned_rows.clear();
                if let Some(key) = anchor {
                    self.pinned_rows.push(key.to_owned());
                }
                if let Some(key) = focus
                    && !self.pinned_rows.iter().any(|r| r == key)
                {
                    self.pinned_rows.push(key.to_owned());
                }
                self.texts.retain(|(row, _)| self.pinned_rows.iter().any(|r| r == row));
            }
            // `pin_offscreen_row`, which pushes the row and adds its texts unless they are kept.
            Ev::Pin(key) => {
                let already = self.texts.iter().any(|(row, _)| row == key);
                if !self.pinned_rows.iter().any(|r| r == key) {
                    self.pinned_rows.push(key.to_owned());
                }
                if !already {
                    self.push(key);
                }
            }
            Ev::Commit(kind, focused) => self.commit(kind, focused, None),
            Ev::Panic(kind, focused, at) => self.commit(kind, focused, Some(at)),
            Ev::Fallback(at) => self.remount_falling_back(at),
        }
    }

    /// `remount_virtuals` (remount.rs:202–203 and 266–320) when an entering row holds a nested
    /// virtual list. Planning clears every list's texts and providers first; the build loop then
    /// runs list by list in tree order, and within a list builds the focused row, then the
    /// pinned rows, then the entering rows, where the nested list sets `nested_virtual` and
    /// breaks out of that list and every later one. The source is never touched.
    fn remount_falling_back(&mut self, at: Boundary) {
        // The plan loop, for every list.
        self.texts.clear();
        self.providers.clear();
        let pinned = self.pinned_indices();
        // The build loop broke out in an earlier list, so it never reaches this one.
        let Boundary::Here(focused) = at else { return };
        let focused = focused.map(index_of);
        if let Some(i) = focused
            && pinned.contains(&i)
        {
            self.push(&key_of(i));
        }
        for &i in &pinned {
            if Some(i) == focused {
                continue;
            }
            self.push(&key_of(i));
        }
        // The entering rows: the first one holding a nested virtual list breaks here, with
        // this list's focused and pinned rows already collected.
    }

    /// Dev's three commit paths, written as they are: each clears the texts and providers in
    /// place and writes each built row's texts straight into the retained list.
    fn commit(&mut self, kind: Kind, focused: Option<&'static str>, stop: Option<usize>) {
        self.texts.clear();
        self.providers.clear();
        match kind {
            // measure.rs:565 installs the source before the children loop, then enumerates the
            // children, collecting only the pinned ones.
            Kind::Stored => {
                self.source = Some("stored");
                let pinned = self.pinned_indices();
                for i in 0..ROWS.len() {
                    if stop == Some(i) {
                        return;
                    }
                    if pinned.contains(&i) {
                        self.push(&key_of(i));
                    }
                }
            }
            // measure.rs builds a focused offscreen row first, then the pinned rows, and
            // installs the source after collecting (line 550).
            Kind::Indexed | Kind::Remount => {
                let pinned = self.pinned_indices();
                let mut steps: Vec<usize> = focused.map(index_of).into_iter().collect();
                steps.extend(pinned.iter().copied().filter(|i| Some(*i) != focused.map(index_of)));
                for (step, i) in steps.iter().enumerate() {
                    if stop == Some(step) {
                        return;
                    }
                    if pinned.contains(i) {
                        self.push(&key_of(*i));
                    }
                }
                if kind == Kind::Indexed {
                    self.source = Some("rebuild");
                }
            }
        }
    }

    fn view(&self) -> (Vec<String>, Vec<(String, String)>, Vec<String>, Option<&'static str>) {
        (self.pinned_rows.clone(), self.texts.clone(), self.providers.clone(), self.source)
    }
}

fn view(pins: &Pins) -> (Vec<String>, Vec<(String, String)>, Vec<String>, Option<&'static str>) {
    let rows = pins.rows().to_vec();
    let texts = pins.texts().iter().map(|t| (t.row_key.clone(), t.path.clone())).collect();
    let providers = match pins {
        Pins::None => Vec::new(),
        Pins::Pinned { providers, .. } => providers.iter().map(|(path, _)| path.clone()).collect(),
    };
    (rows, texts, providers, pins.source().map(source_name))
}

/// Every sequence of up to five events after the commit a list must have before it can pin,
/// each driven through the one state and compared with dev's fields after every event.
#[test]
fn every_order_of_pins_commits_panics_and_fallbacks_keeps_what_dev_keeps() {
    const LONGEST: u32 = 5;
    for len in 1..=LONGEST {
        for bits in 0..(EVENTS.len() as u32).pow(len) {
            let mut n = bits;
            let order: Vec<Ev> = (0..len)
                .map(|_| {
                    let i = (n % EVENTS.len() as u32) as usize;
                    n /= EVENTS.len() as u32;
                    EVENTS[i]
                })
                .collect();
            let walk = order.iter().map(|e| name(*e)).collect::<Vec<_>>().join(", ");
            // A virtual list exists only after a commit, so its source is set before any
            // selection can pin a row.
            let (mut pins, mut dev) = (Pins::None, Dev::default());
            apply(&mut pins, Ev::Commit(Kind::Stored, None));
            dev.apply(Ev::Commit(Kind::Stored, None));
            assert_eq!(view(&pins), dev.view(), "the first commit");
            for (step, &event) in order.iter().enumerate() {
                apply(&mut pins, event);
                dev.apply(event);
                assert_eq!(view(&pins), dev.view(), "step {step} of {walk}");
            }
        }
    }
}

/// `measure.rs` enumerates stored children, so a stored commit keeps its texts in child order
/// however the pins were made; an indexed one builds a focused pinned row first.
#[test]
fn a_stored_commit_orders_texts_by_child_while_an_indexed_one_builds_the_focused_row_first() {
    let (mut pins, mut dev) = (Pins::None, Dev::default());
    for event in [
        Ev::Commit(Kind::Stored, None),
        Ev::Pin("b"),
        Ev::Pin("a"),
        Ev::Commit(Kind::Stored, None),
    ] {
        apply(&mut pins, event);
        dev.apply(event);
        assert_eq!(view(&pins), dev.view(), "{}", name(event));
    }
    let stored: Vec<&str> = pins.texts().iter().map(|t| t.row_key.as_str()).collect();
    assert_eq!(stored, ["a", "b"], "stored children in index order, not pin order");

    apply(&mut pins, Ev::Commit(Kind::Indexed, Some("b")));
    dev.apply(Ev::Commit(Kind::Indexed, Some("b")));
    assert_eq!(view(&pins), dev.view());
    let indexed: Vec<&str> = pins.texts().iter().map(|t| t.row_key.as_str()).collect();
    assert_eq!(indexed, ["b", "a"], "the focused pinned row builds first");
}

/// A builder that panics keeps the pinned rows' texts and providers collected before it, and
/// the source dev had installed by then.
#[test]
fn a_panicking_builder_keeps_the_pins_collected_before_it() {
    for kind in [Kind::Indexed, Kind::Stored] {
        let (mut pins, mut dev) = (Pins::None, Dev::default());
        for event in [Ev::Commit(Kind::Stored, None), Ev::Pin("a"), Ev::Pin("b")] {
            apply(&mut pins, event);
            dev.apply(event);
        }
        // An indexed commit starts from the stored source, so a late installed one differs.
        if kind == Kind::Indexed {
            apply(&mut pins, Ev::Commit(Kind::Indexed, None));
            dev.apply(Ev::Commit(Kind::Indexed, None));
        }
        let event = Ev::Panic(kind, None, 1);
        apply(&mut pins, event);
        dev.apply(event);
        assert_eq!(view(&pins), dev.view(), "{}", name(event));
        assert_eq!(pins.texts().len(), 1, "row a survives the panic building b");
        assert_eq!(pins.texts()[0].row_key, "a", "the already collected pin stays first");
        // A stored commit installs its source before the loop; an indexed one only after it, so
        // an interrupted indexed commit keeps the stored source it began with.
        let expected = if kind == Kind::Indexed { "rebuild" } else { "stored" };
        assert_eq!(pins.source().map(source_name), Some(expected), "{}", name(event));
    }
}

/// Clearing for a commit, a remount fallback and a source change keep the texts' and
/// providers' allocations; nothing is taken out of the state.
#[test]
fn a_commit_fallback_and_source_change_keep_the_texts_allocation() {
    let mut pins = Pins::None;
    pins.set_source(Source::Stored);
    write(&mut pins, "a");
    write(&mut pins, "b");
    let capacity = pins.text_buffers().expect("a committed list").0.capacity();
    assert!(capacity >= 2);

    pins.clear_texts();
    assert!(pins.texts().is_empty());
    assert!(
        pins.text_buffers().expect("a committed list").0.capacity() >= capacity,
        "a commit clears in place"
    );

    pins.set_source(source("rebuild"));
    assert!(
        pins.text_buffers().expect("a committed list").0.capacity() >= capacity,
        "a source change keeps the buffers"
    );
    write(&mut pins, "a");
    assert_eq!(pins.texts().len(), 1, "and the next commit refills them");
}

/// Dev's remount collects every pinned row of the list that falls back before it reaches the
/// entering row holding a nested list: a focused pinned `b`, then `a`.
#[test]
fn a_fallback_in_this_list_keeps_all_its_pins_and_one_before_it_keeps_none() {
    let start = [Ev::Commit(Kind::Stored, None), Ev::Select(Some("a"), Some("b")), Ev::Commit(Kind::Indexed, None)];
    for (at, expected) in [(Boundary::Here(Some("b")), vec!["b", "a"]), (Boundary::Earlier, vec![])] {
        let (mut pins, mut dev) = (Pins::None, Dev::default());
        for event in start.into_iter().chain([Ev::Fallback(at)]) {
            apply(&mut pins, event);
            dev.apply(event);
            assert_eq!(view(&pins), dev.view(), "{}", name(event));
        }
        let kept: Vec<&str> = pins.texts().iter().map(|t| t.row_key.as_str()).collect();
        assert_eq!(kept, expected, "{}", name(Ev::Fallback(at)));
    }
}

/// Rows of the two-list fixture. From `NESTED` on, every row holds a nested virtual list, so a
/// remount that has one enter falls back.
const LIST_ROWS: usize = 400;
const NESTED: usize = 150;
const ROW_H: f32 = 20.0;

struct Lists;

fn list_row(_: &Lists, i: usize) -> El {
    let row = crate::col()
        .w_full()
        .h(ROW_H)
        .selection_copy(|_| None)
        .child(crate::text(format!("row {i}")).key("t0").w_full().selectable(true));
    if i < NESTED { row } else { row.child(crate::col().key("inner").virtualize(ROW_H)) }
}

fn lists_view(_: &Lists, _: &mut crate::Cx<Lists>) -> El {
    let list = |name: &'static str| {
        crate::col().key(format!("s{name}")).w_full().grow(1.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            crate::col()
                .key(name)
                .w_full()
                .growth(crate::GrowthSpec::default())
                .selection_group(true)
                .virtual_rows(LIST_ROWS, ROW_H, |_: &Lists, i| key_of(i).into(), list_row),
        )
    };
    crate::col().w_full().h_full().children([list("one"), list("two")])
}

fn lists() -> crate::Runtime<Lists> {
    let mut rt = crate::Runtime::new(Lists, lists_view, (640.0, 420.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    rt
}

fn list_path(rt: &crate::Runtime<Lists>, name: &str) -> String {
    let suffix = format!("/{name}");
    rt.virtuals.keys().find(|p| p.ends_with(&suffix)).expect("a mounted list").clone()
}

/// Selects all of list `name`, pinning its first and last rows, and wheels it (at height `y`)
/// to the middle, where neither is mounted and no nested row is yet.
fn select_all_and_scroll(rt: &mut crate::Runtime<Lists>, name: &'static str, y: f32) {
    rt.update(|_, tx| tx.select_all_text(name));
    rt.frame(1.0 / 60.0);
    let path = list_path(rt, name);
    for _ in 0..40 {
        if rt.virtuals[&path].range.start > 40 {
            break;
        }
        rt.wheel(320.0, y, -200.0);
        rt.frame(1.0 / 60.0);
    }
    for _ in 0..40 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    let range = rt.virtuals[&path].range.clone();
    assert!(range.start > 1 && range.end < NESTED, "{name} scrolled to the middle: {range:?}");
}

/// Moves list `name` straight into its nested rows and runs the real remount on its own.
fn remount_into_nested(rt: &mut crate::Runtime<Lists>, name: &str) -> bool {
    let scroll = rt.virtuals[&list_path(rt, name)].scroll_path.clone();
    let id = rt.path_ids[&scroll];
    rt.scrolls.get_mut(&id).expect("the list's scroll").offset = ((NESTED + 20) as f32 * ROW_H) as f64;
    rt.remount_falls_back()
}

/// The pinned rows, the rows of the kept texts and the rows of the kept providers.
type Kept = (Vec<String>, Vec<String>, Vec<String>);

fn kept(pins: &Pins) -> Kept {
    let (_, texts, providers, _) = view(pins);
    let row = |path: String| path.rsplit('/').next().unwrap_or_default().to_owned();
    (pins.rows().to_vec(), texts.into_iter().map(|(key, _)| key).collect(), providers.into_iter().map(row).collect())
}

fn dev_kept(dev: &Dev) -> Kept {
    let row = |path: &String| path.rsplit('/').next().unwrap_or_default().to_owned();
    (dev.pinned_rows.clone(), dev.texts.iter().map(|(key, _)| key.clone()).collect(), dev.providers.iter().map(row).collect())
}

/// The real remount, falling back in the list whose pins it is collecting, keeps what dev's
/// remount keeps: every pinned row, collected before the entering rows.
#[test]
fn the_real_remount_collects_this_lists_pins_before_it_falls_back() {
    let mut rt = lists();
    select_all_and_scroll(&mut rt, "one", 100.0);
    let path = list_path(&rt, "one");
    let mut dev = Dev::default();
    for event in [Ev::Select(Some("a"), Some("r399")), Ev::Commit(Kind::Remount, None)] {
        dev.apply(event);
    }
    assert_eq!(kept(&rt.virtuals[&path].selection.pins), dev_kept(&dev), "before the fallback");

    assert!(remount_into_nested(&mut rt, "one"), "a nested list entering falls back");
    dev.apply(Ev::Fallback(Boundary::Here(None)));
    assert_eq!(kept(&rt.virtuals[&path].selection.pins), dev_kept(&dev), "after the fallback");
}

/// A list after the one that falls back keeps its pins, and its texts are cleared in place,
/// as dev's planning loop leaves them.
#[test]
fn the_real_remount_clears_later_lists_in_place_when_an_earlier_one_falls_back() {
    let mut rt = lists();
    select_all_and_scroll(&mut rt, "two", 310.0);
    let path = list_path(&rt, "two");
    let mut dev = Dev::default();
    for event in [Ev::Select(Some("a"), Some("r399")), Ev::Commit(Kind::Remount, None)] {
        dev.apply(event);
    }
    assert_eq!(kept(&rt.virtuals[&path].selection.pins), dev_kept(&dev), "before the fallback");
    let pins = &mut rt.virtuals.get_mut(&path).expect("list two").selection.pins;
    let capacity = pins.text_buffers().expect("a committed list").0.capacity();

    assert!(remount_into_nested(&mut rt, "one"), "a nested list entering list one falls back");
    dev.apply(Ev::Fallback(Boundary::Earlier));
    let pins = &mut rt.virtuals.get_mut(&path).expect("list two").selection.pins;
    assert_eq!(kept(pins), dev_kept(&dev), "after the fallback");
    assert!(pins.text_buffers().expect("a committed list").0.capacity() >= capacity, "cleared in place");
}
