//! The pins state against origin/dev's separate fields: every order of a selection change, a
//! live pin and a commit, compared after every event.
use std::rc::Rc;

use super::{OffscreenText, Pins, PinsEvent, Source};
use crate::el::TextSpec;

/// A row's offscreen texts: a deterministic stand-in for `build_unmounted_row`, which the walk
/// must not run (the reference never calls production code).
fn built(key: &str) -> Vec<OffscreenText> {
    (0..2)
        .map(|i| OffscreenText {
            list_path: "/list".into(),
            row_key: key.into(),
            row_index: i,
            order_in_row: i,
            rel_path: format!("t{i}"),
            path: format!("/list/{key}/t{i}"),
            key: None,
            group_path: None,
            spec: TextSpec::new(format!("{key}{i}")),
            tab_before: false,
        })
        .collect()
}

/// The `selection_copy` provider the walk's rows carry, by path.
fn provider(key: &str) -> (String, crate::el::SelectionCopy) {
    (format!("/list/{key}"), Rc::new(|_| None))
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

/// One thing that can change what a list keeps.
#[derive(Clone, Copy)]
enum Ev {
    Clear,
    Select(Option<&'static str>, Option<&'static str>),
    Pin(&'static str),
    Commit(&'static str),
}

const EVENTS: [Ev; 10] = [
    Ev::Clear,
    Ev::Select(Some("a"), Some("b")),
    Ev::Select(Some("a"), Some("a")),
    Ev::Select(Some("a"), None),
    Ev::Select(None, Some("a")),
    Ev::Select(None, None),
    Ev::Pin("a"),
    Ev::Pin("b"),
    Ev::Commit("rebuild"),
    Ev::Commit("stored"),
];

fn name(event: Ev) -> &'static str {
    match event {
        Ev::Clear => "clear",
        Ev::Select(Some("a"), Some("b")) => "select(a,b)",
        Ev::Select(Some("a"), Some("a")) => "select(a,a)",
        Ev::Select(Some("a"), None) => "select(a,-)",
        Ev::Select(None, Some("a")) => "select(-,a)",
        Ev::Select(..) => "select(-,-)",
        Ev::Pin("a") => "pin(a)",
        Ev::Pin(_) => "pin(b)",
        Ev::Commit("rebuild") => "commit(rebuild)",
        Ev::Commit(_) => "commit(stored)",
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
                    self.texts.extend(built(key).into_iter().map(|t| (t.row_key, t.path)));
                    self.providers.push(provider(key).0);
                }
            }
            // A commit: every pinned row is offscreen here, so all of them are built.
            Ev::Commit(source) => {
                self.texts.clear();
                self.providers.clear();
                for key in self.pinned_rows.clone() {
                    self.texts.extend(built(&key).into_iter().map(|t| (t.row_key, t.path)));
                    self.providers.push(provider(&key).0);
                }
                self.source = Some(source);
            }
        }
    }

    fn view(&self) -> (Vec<String>, Vec<(String, String)>, Vec<String>, Option<&'static str>) {
        (self.pinned_rows.clone(), self.texts.clone(), self.providers.clone(), self.source)
    }
}

/// The same event through the one state, building the pinned rows a commit would.
fn apply(pins: &mut Pins, event: Ev) {
    match event {
        Ev::Clear => pins.sync(PinsEvent::Clear),
        Ev::Select(anchor, focus) => pins.sync(PinsEvent::Select { anchor, focus }),
        Ev::Pin(key) => {
            let (texts, providers) = if pins.has_text_row(key) {
                (Vec::new(), Vec::new())
            } else {
                (built(key), vec![provider(key)])
            };
            pins.sync(PinsEvent::Pin { key: key.to_owned(), texts, providers });
        }
        Ev::Commit(name) => {
            let (mut texts, mut providers) = pins.take_texts();
            texts.clear();
            providers.clear();
            for key in pins.rows().to_vec() {
                texts.extend(built(&key));
                providers.push(provider(&key));
            }
            pins.sync(PinsEvent::Commit { source: Some(source(name)), texts, providers });
        }
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
fn every_order_of_selection_pins_and_commits_keeps_what_dev_keeps() {
    const LONGEST: u32 = 5;
    for len in 1..=LONGEST {
        for bits in 0..(EVENTS.len() as u32).pow(len) {
            let mut n = bits;
            let order: Vec<usize> = (0..len)
                .map(|_| {
                    let i = (n % EVENTS.len() as u32) as usize;
                    n /= EVENTS.len() as u32;
                    i
                })
                .collect();
            let walk = order.iter().map(|&i| name(EVENTS[i])).collect::<Vec<_>>().join(", ");
            // A virtual list exists only after a commit, so its source is set before any
            // selection can pin a row.
            let (mut pins, mut dev) = (Pins::None, Dev::default());
            apply(&mut pins, Ev::Commit("stored"));
            dev.apply(Ev::Commit("stored"));
            assert_eq!(view(&pins), dev.view(), "the first commit");
            for (step, &i) in order.iter().enumerate() {
                apply(&mut pins, EVENTS[i]);
                dev.apply(EVENTS[i]);
                assert_eq!(view(&pins), dev.view(), "step {step} of {walk}");
            }
        }
    }
}

/// A commit hands the kept texts' allocation back, so the next commit refills it in place.
#[test]
fn a_commit_reuses_the_kept_texts_allocation() {
    let mut pins = Pins::None;
    pins.sync(PinsEvent::Commit { source: Some(Source::Stored), texts: built("a"), providers: Vec::new() });
    let (texts, providers) = pins.take_texts();
    assert!(texts.capacity() >= 2, "the commit hands its allocation back");
    assert_eq!(texts.len(), 2);
    assert!(pins.texts().is_empty(), "taken out for the commit to refill");
    pins.sync(PinsEvent::Commit { source: None, texts, providers });
    assert_eq!(pins.texts().len(), 2);
    assert!(matches!(pins.source(), Some(Source::Stored)), "a remount keeps the source");
}
