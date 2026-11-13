//! Drives every short sequence of streaming-text events through the runtime and checks the
//! reveal after every one against a literal transcription of the code before `RevealPhase`: one
//! `RevealState { len, words, fade }` per text and the window's bare `reveal_chain: f64`, in
//! `commit/apply.rs`, the reveal step in `motion/tracks.rs`, the clock shift in
//! `motion/loop_clock.rs` and the drop in `commit/prune.rs`.
//!
//! The reference sees only what a letter says: the text's content, the fade, whether the text
//! was in the scene at the last commit, whether the commit lands its changes, and the clock.
//! The sequence walk's window keeps every commit's motion, has no virtual list, doesn't reduce
//! motion and runs every clock at 1×, so those stay constants on both sides; the directed walks
//! below vary the window's facts instead. A phase is compared by what shows: the shown bytes,
//! the words still fading and their fade, the window's chain, and every byte's alpha at the
//! text's clock. The fade an `Idle` state carried is the one thing not compared, because nothing
//! reads it until a word fades.
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::el::col;
use crate::{TextStyle, text};

/// The gated walk's depth: every sequence up to this many letters.
const LENGTH: usize = 5;
/// The ignored deep run's depth: the same walk, one letter further.
const DEEP: usize = 6;
/// `El::reveal`: seconds each word takes to fade.
const FADE: f32 = 0.05;
/// Words one `Chunk` brings: at this fade, a chunk's run outlasts it, so a later commit meets a
/// chain longer than a fade.
const CHUNK: usize = 6;
/// The window's facts the reveal reads, as dev read them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Facts {
    /// `TextSpec::reveal >= 0`: the text tracks a reveal at all. A negative reveal skips the
    /// whole block, words, length and fade alike.
    reveal: bool,
    /// The fade dev gives each word (`TextSpec::reveal`, or 0.15 under reduced motion).
    fade: f32,
    reduced_motion: bool,
    /// Some node's clock is scaled (`El::time_scale`), which takes the text out of the chain.
    time_scaled: bool,
}

/// The walk's window: the reveal tracks the text, nothing reduces motion, every clock runs 1×.
const STEADY: Facts = Facts { reveal: true, fade: FADE, reduced_motion: false, time_scaled: false };

/// What the letters that edit change.
#[derive(Clone, Default, PartialEq)]
struct Doc {
    /// Words the stream has brought.
    words: usize,
    /// A short replacement shows instead of the stream.
    replaced: bool,
    /// The text's mount: every remount (and every reset) is a new node at a new path.
    mounts: u32,
    /// The text's parent column is in the tree; without it the keyed text leaves, and comes back
    /// under the same path when the parent returns.
    parent: bool,
}

impl Doc {
    fn content(&self) -> String {
        if self.replaced {
            return "a b".to_owned();
        }
        let mut s = String::new();
        for i in 0..self.words {
            if i > 0 {
                s.push(' ');
            }
            s.push('w');
        }
        s
    }

    fn key(&self) -> String { format!("t{}", self.mounts) }

    fn path(&self) -> String { format!("root/p/{}", self.key()) }
}

fn view(doc: &Doc, _: &mut Cx<Doc>) -> El {
    let text = text(doc.content()).key(doc.key()).w_full().wrap().font(TextStyle::new(14.0, 400.0)).reveal(FADE);
    col().key("root").w(200.0).children(doc.parent.then(|| col().key("p").child(text)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Letter {
    /// A chunk arrives: the stream appends words.
    Chunk,
    /// A frame ticks.
    Tick,
    /// A frame past the fade: the words that were fading finish.
    Fade,
    /// A chunk arrives while words still fade.
    MidChunk,
    /// The text is replaced with a shorter one.
    Replace,
    /// The stream ends: a commit that changes no content.
    End,
    /// The text remounts as a new node under a new key, still entering.
    Remount,
    /// Its parent unmounts instantly and returns: the keyed text comes back with the reveal state
    /// it kept, under the same key.
    Return,
}

const ALPHABET: [Letter; 8] = [
    Letter::Chunk,
    Letter::Tick,
    Letter::Fade,
    Letter::MidChunk,
    Letter::Replace,
    Letter::End,
    Letter::Remount,
    Letter::Return,
];

/// `RevealState` and `reveal_chain` as `Runtime` held them before `RevealPhase`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Dev {
    state: Option<DevReveal>,
    chain: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct DevReveal {
    len: usize,
    words: Vec<(u32, f64)>,
    fade: f32,
}

impl Dev {
    /// `commit/apply.rs`'s reveal block, as it was.
    fn commit(&mut self, content: &str, was_present: bool, instant: bool, now: f64, facts: Facts) {
        // Dev's `if spec.reveal >= 0.0`: a text with its reveal off keeps whatever it had, its
        // length included, so bytes that arrived meanwhile fade in when it comes back.
        if !facts.reveal {
            return;
        }
        let fade = facts.fade;
        // The walk's window keeps every commit's motion, so no commit is unprompted.
        let mounted_late = !was_present && !instant;
        let st = self.state.get_or_insert_with(|| DevReveal {
            len: if mounted_late { 0 } else { content.len() },
            words: Vec::new(),
            fade,
        });
        st.fade = fade;
        if instant || content.len() < st.len || !content.is_char_boundary(st.len) {
            st.len = content.len();
            st.words.clear();
        } else if content.len() > st.len && fade > 0.0 {
            let mut starts = Vec::new();
            word_starts(content, st.len, &mut starts);
            let own = st.words.last().map_or(now, |w| w.1.max(now));
            let chained = !facts.time_scaled && self.chain - now <= f64::from(fade);
            let chain = if chained { self.chain.max(now) } else { now };
            let queued = ((chain - now) / 0.0055) as usize;
            let pending = st.words.iter().filter(|w| w.1 > now).count() + starts.len() + queued;
            let stagger = if facts.reduced_motion {
                0.0
            } else {
                let k = (pending as f32 / 24.0).clamp(0.0, 1.0);
                (0.020 + (0.0055 - 0.020) * k) as f64
            };
            let mut tn = if chain > own { chain + stagger } else { own };
            for b in starts {
                st.words.push((b, tn));
                tn += stagger;
            }
            if !facts.time_scaled && let Some(last) = st.words.last() {
                self.chain = self.chain.max(last.1);
            }
            st.len = content.len();
        } else {
            st.len = content.len();
        }
    }

    /// `reveals_active`'s step, as it was.
    fn tick(&mut self, now: f64) {
        let Some(st) = &mut self.state else { return };
        let fade = st.fade as f64;
        let done = st.words.iter().take_while(|w| now - w.1 >= fade).count();
        if done > 0 && done == st.words.len() {
            st.words.clear();
        }
    }

    /// `sync_time_scales`: its clock changed scale, so its word times move by `shift`.
    fn shift(&mut self, shift: f64) {
        let Some(st) = &mut self.state else { return };
        for w in &mut st.words {
            w.1 += shift;
        }
    }

    /// `prune_exited`: the node left the scene.
    fn left(&mut self) { self.state = None; }

    /// A word's alpha, as `reveal_alpha` gave it.
    fn alpha(&self, byte: u32, now: f64) -> f32 {
        let Some(st) = &self.state else { return 1.0 };
        let i = st.words.partition_point(|w| w.0 <= byte);
        if i == 0 {
            return 1.0;
        }
        let t0 = st.words[i - 1].1;
        if st.fade <= 0.0 {
            return if now >= t0 { 1.0 } else { 0.0 };
        }
        ease_out(((now - t0) / st.fade as f64) as f32)
    }
}

/// `word_starts`, as it was.
fn word_starts(s: &str, from: usize, out: &mut Vec<u32>) {
    let mut prev_space = true;
    for (i, c) in s[from..].char_indices() {
        let space = c.is_whitespace();
        if !space && prev_space {
            out.push((from + i) as u32);
        }
        prev_space = space;
    }
}

/// `ease_out`, as it was.
fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x)
}

struct Walk {
    rt: Runtime<Doc>,
    doc: Doc,
    dev: Dev,
    /// The text was in the scene at the last commit.
    present: bool,
    /// The text's node, kept while it is out of the tree: its reveal state outlives the node.
    id: Option<NodeId>,
}

impl Walk {
    fn new() -> Self {
        let mut rt = Runtime::new(Doc::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(0.0);
        assert!(!rt.reduced_motion && !rt.system_reduced_motion, "the walk takes a window that doesn't reduce motion");
        assert!(!rt.motion_needs_cause, "the walk takes a window where every commit keeps its motion");
        assert!(!rt.time_scaled && rt.time_scale == 1.0, "the walk takes a text on the window's clock");
        let mut walk = Self { rt, doc: Doc::default(), dev: Dev::default(), present: true, id: None };
        walk.reset();
        walk
    }

    /// A fresh text node with the stream at rest and the reveal as a new window's: every sequence
    /// starts from the same state on both sides.
    fn reset(&mut self) {
        self.doc = Doc { mounts: self.doc.mounts + 1, parent: true, ..Doc::default() };
        self.rt.reveals.clear();
        self.rt.reveal_chain = RevealChain::default();
        self.dev = Dev::default();
        self.present = false;
        self.commit(true).expect("a reset starts level");
        // One long frame: the faded-out texts of the last sequence leave the scene, so their
        // nodes and reveal state don't pile up across sequences. Nothing fades on the new text.
        self.rt.frame(1.0);
        assert_eq!(self.rt.reveals.len(), 1, "only the live text keeps reveal state");
    }

    /// An edit and its frame: the commit at the frame's clock, then the frame's reveal step at
    /// the same time (a `dt` of zero).
    fn commit(&mut self, instant: bool) -> Result<(), String> { self.frame_commit(instant, true) }

    /// `commit`, with the text out of the tree: its commit never reaches the reveal, while the
    /// frame still ticks the state it kept.
    fn commit_absent(&mut self, instant: bool) -> Result<(), String> { self.frame_commit(instant, false) }

    fn frame_commit(&mut self, instant: bool, in_tree: bool) -> Result<(), String> {
        let now = self.rt.t;
        let doc = self.doc.clone();
        let content = doc.content();
        let was_present = self.present;
        let commits = self.rt.stats.commits;
        self.rt.update(|s, tx| {
            *s = doc.clone();
            if instant {
                tx.instant();
            }
        });
        self.rt.frame(0.0);
        if self.rt.stats.commits != commits + 1 {
            return Err(format!("an edit committed {} times", self.rt.stats.commits - commits));
        }
        if in_tree {
            self.dev.commit(&content, was_present, instant, now, STEADY);
            self.present = true;
        }
        self.dev.tick(now);
        self.compare()
    }

    /// A frame with no edit: only the reveal steps, at the frame's new time.
    fn tick(&mut self, dt: f64) -> Result<(), String> {
        let commits = self.rt.stats.commits;
        self.rt.frame(dt);
        if self.rt.stats.commits != commits {
            return Err(format!("a frame with no edit committed {} times", self.rt.stats.commits - commits));
        }
        let now = self.rt.t;
        self.dev.tick(now);
        self.compare()
    }

    /// Plays one letter on the runtime and the reference.
    fn play(&mut self, letter: Letter) -> Result<(), String> {
        match letter {
            Letter::Chunk => {
                self.doc.words += CHUNK;
                self.commit(false)
            }
            Letter::Tick => self.tick(1.0 / 60.0),
            Letter::Fade => self.tick(f64::from(FADE)),
            Letter::MidChunk => {
                self.tick(f64::from(FADE) / 2.0)?;
                self.doc.words += 1;
                self.commit(false)
            }
            Letter::Replace => {
                self.doc.replaced = !self.doc.replaced;
                self.commit(false)
            }
            Letter::End => self.commit(false),
            Letter::Remount => {
                self.doc.mounts += 1;
                self.present = false;
                self.dev.left();
                self.commit(false)
            }
            Letter::Return => {
                self.doc.parent = false;
                self.commit_absent(true)?;
                self.present = false;
                self.doc.parent = true;
                self.commit(false)
            }
        }
    }

    /// The visible reveal and the fade state, against dev's.
    fn compare(&mut self) -> Result<(), String> {
        let path = self.doc.path();
        let node = self.rt.nodes.iter().find(|n| n.path == path).map(|n| n.id);
        if let Some(id) = node {
            self.id = Some(id);
        }
        let id = self.id.ok_or_else(|| format!("no text at {path}, and none before it"))?;
        if node.is_none() && self.doc.parent {
            return Err(format!("no text at {path}"));
        }
        let ours = self.rt.reveals.get(&id);
        let theirs = self.dev.state.as_ref();
        match (ours, theirs) {
            (None, None) => {}
            (Some(phase), Some(st)) => {
                if phase.shown() != st.len {
                    return Err(format!("shown {}, dev {}", phase.shown(), st.len));
                }
                let words = phase.words();
                if words != st.words.as_slice() {
                    return Err(format!("words {words:?}, dev {:?}", st.words));
                }
                if let RevealPhase::Fading { fade, .. } = phase && *fade != st.fade {
                    return Err(format!("fade {fade}, dev {}", st.fade));
                }
            }
            (phase, st) => return Err(format!("reveal {phase:?}, dev {st:?}")),
        }
        if self.rt.reveal_chain.until() != self.dev.chain {
            return Err(format!("chain {}, dev {}", self.rt.reveal_chain.until(), self.dev.chain));
        }
        // Out of the tree, nothing draws: its state is all there is to compare.
        let Some(_) = node else { return Ok(()) };
        let now = self.rt.scene.store().node_time(id);
        for byte in 0..=self.doc.content().len() as u32 {
            let ours = ours.and_then(RevealPhase::fade).map_or(1.0, |fade| fade.alpha(byte, now));
            let theirs = self.dev.alpha(byte, now);
            if ours != theirs {
                return Err(format!("alpha at byte {byte}: {ours}, dev {theirs}"));
            }
        }
        Ok(())
    }
}

/// Plays every sequence that starts with `sequence`, recording the first one that differs.
fn visit(walk: &mut Walk, alphabet: &[Letter], depth: usize, sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
    if failure.is_some() {
        return;
    }
    *ran += 1;
    walk.reset();
    for &letter in sequence.iter() {
        if let Err(why) = walk.play(letter) {
            *failure = Some(format!("{sequence:?}: {why}"));
            return;
        }
    }
    if sequence.len() == depth {
        return;
    }
    for &letter in alphabet {
        sequence.push(letter);
        visit(walk, alphabet, depth, sequence, ran, failure);
        sequence.pop();
    }
}

fn walk_all(alphabet: &[Letter], depth: usize) {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walk = Walk::new();
                let (mut ran, mut failure) = (0, None);
                while let Some(&letter) = alphabet.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walk, alphabet, depth, &mut vec![letter], &mut ran, &mut failure);
                }
                results.lock().unwrap().push((ran, failure));
            });
        }
    });
    let mut total = 0;
    let mut failures = Vec::new();
    for (ran, failure) in results.into_inner().unwrap() {
        total += ran;
        failures.extend(failure);
    }
    println!("{total} sequences up to length {depth}");
    assert!(failures.is_empty(), "sequences differ from dev: {failures:#?}");
    let expected: usize = (1..=depth as u32).map(|n| alphabet.len().pow(n)).sum();
    assert_eq!(total, expected, "every sequence ran");
}

#[test]
fn every_short_sequence_of_streaming_events_leaves_what_dev_left() {
    walk_all(&ALPHABET, LENGTH);
}

// The deep run: `cargo test -p rune --lib -- --ignored every_deeper_sequence`.
#[ignore = "the deep run"]
#[test]
fn every_deeper_sequence_of_streaming_events_leaves_what_dev_left() {
    walk_all(&ALPHABET, DEEP);
}

/// The state the walk compares against is the one dev left: a text whose stream a chunk filled
/// fades word by word, and the window's chain follows its newest word.
#[test]
fn a_chunk_fades_its_words_and_the_chain_follows() {
    let mut walk = Walk::new();
    walk.play(Letter::Chunk).unwrap();
    let id = walk.rt.nodes.iter().find(|n| n.path == walk.doc.path()).unwrap().id;
    let RevealPhase::Fading { words, .. } = walk.rt.reveals.get(&id).unwrap() else {
        panic!("a chunk's words fade");
    };
    assert_eq!(words.len(), CHUNK, "every word of the chunk fades");
    assert!(walk.rt.reveal_chain.until() >= words.last().unwrap().1, "the chain holds the newest word");
    let now = walk.rt.scene.store().node_time(id);
    assert!(walk.rt.reveal_chain.until() - now > f64::from(FADE), "a chunk's run outlasts a fade, so a later one meets a chain longer than it");
}

/// A remounted text fades in from none: it is new, and its entrance is still playing.
#[test]
fn a_remount_starts_its_words_from_none() {
    let mut walk = Walk::new();
    walk.play(Letter::Chunk).unwrap();
    walk.play(Letter::Remount).unwrap();
    let id = walk.rt.nodes.iter().find(|n| n.path == walk.doc.path()).unwrap().id;
    let phase = walk.rt.reveals.get(&id).unwrap();
    let RevealPhase::Fading { words, .. } = phase else { panic!("a remount fades its words in") };
    assert_eq!(words.first().unwrap().0, 0, "it starts at its first word");
}

/// A keyed child that comes back after its parent unmounted instantly keeps its words: the
/// runtime kept its reveal state, so nothing fades again.
#[test]
fn a_returning_child_keeps_its_words_after_an_instant_parent_unmount() {
    #[derive(Default)]
    struct S {
        parent: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").w(300.0).children(
            s.parent.then(|| col().key("p").child(text("a b c").key("t").w_full().wrap().reveal(FADE))),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.parent = true);
    rt.frame(0.0);
    rt.frame(0.1);
    let id = rt.nodes.iter().find(|n| n.path == "root/p/t").unwrap().id;
    assert!(rt.reveals.get(&id).is_some_and(|p| p.caught_up(5)), "the first words have shown");
    rt.update(|s, tx| {
        s.parent = false;
        tx.instant();
    });
    rt.frame(0.0);
    assert!(rt.reveals.contains_key(&id), "the unmounted child keeps its reveal state");
    rt.update(|s, _| s.parent = true);
    rt.frame(0.0);
    let back = rt.nodes.iter().find(|n| n.path == "root/p/t").unwrap().id;
    assert_eq!(back, id, "the same key returns under the same id");
    let phase = rt.reveals.get(&id).unwrap();
    assert!(phase.caught_up(5), "it comes back showing its words, not replaying them: {phase:?}");
}

/// An instant commit clears the words that were fading: dev's reset, whatever the content.
#[test]
fn an_instant_commit_clears_a_fade() {
    #[derive(Default)]
    struct S {
        longer: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let content = if s.longer { "a b c" } else { "a" };
        col().key("root").w(300.0).child(text(content).key("t").w_full().wrap().reveal(FADE))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.longer = true);
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.path == "root/t").unwrap().id;
    assert!(rt.reveals.get(&id).is_some_and(RevealPhase::fading), "the new words fade");
    rt.update(|_, tx| tx.instant());
    rt.frame(0.0);
    assert!(rt.reveals.get(&id).is_some_and(|p| p.caught_up(5)), "the instant commit shows them at once");
}

/// A commit no input caused lands a new text whole: `Tx::unprompted` takes dev's reset, not the
/// fade a prompted mount gets.
#[test]
fn an_unprompted_commit_lands_a_new_text_whole() {
    #[derive(Default)]
    struct S {
        shown: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").w(200.0).children(s.shown.then(|| text("a b c d").key("t").w_full().wrap().reveal(FADE)))
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.motion_needs_cause = true;
    rt.frame(0.0);
    rt.update(|s, _| s.shown = true);
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.path == "root/t").unwrap().id;
    assert!(rt.reveals.get(&id).is_some_and(|p| p.caught_up(7)), "no input caused it, so it shows whole");
}

/// A byte that no longer starts a word shows whole: dev's boundary check clears the fade rather
/// than slicing through a character.
#[test]
fn a_cut_through_a_character_shows_whole() {
    #[derive(Default)]
    struct S {
        content: String,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").w(200.0).child(text(s.content.clone()).key("t").w_full().wrap().reveal(FADE))
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.content = "aaa bbbb".to_owned());
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.path == "root/t").unwrap().id;
    assert!(rt.reveals.get(&id).is_some_and(RevealPhase::fading), "the first chunk fades");
    // Byte 8 is inside "é" (bytes 7 and 8) now, so the old end still fading can't be kept.
    rt.update(|s, _| s.content = "aaaaaaaé".to_owned());
    rt.frame(0.0);
    assert!(rt.reveals.get(&id).is_some_and(|p| p.caught_up("aaaaaaaé".len())), "a moved boundary shows whole");
}

/// A directed walk's window: one streaming text whose reveal, clock and window can change between
/// commits, stepped against the same transcription of dev.
#[derive(Clone, Default, PartialEq)]
struct Stream {
    content: String,
    /// `El::reveal`; negative leaves the text out of the reveal, as an app that stops streaming
    /// its words does.
    reveal: f32,
    /// `El::time_scale(0)` on the text: its clock stops while the window's runs on.
    paused: bool,
    /// The window reduces motion (`Cx::reduced_motion`).
    reduced: bool,
}

impl Stream {
    fn facts(&self) -> Facts {
        Facts {
            reveal: self.reveal >= 0.0,
            fade: if self.reduced { 0.15 } else { self.reveal },
            reduced_motion: self.reduced,
            time_scaled: self.paused,
        }
    }

    fn streamed(content: &str) -> Self { Self { content: content.to_owned(), reveal: FADE, ..Self::default() } }
}

fn stream_view(s: &Stream, cx: &mut Cx<Stream>) -> El {
    cx.reduced_motion(s.reduced);
    let mut t = text(s.content.clone()).key("t").w_full().wrap();
    if s.reveal >= 0.0 {
        t = t.reveal(s.reveal);
    }
    if s.paused {
        t = t.time_scale(0.0);
    }
    col().key("root").w(300.0).child(t)
}

struct Driven {
    rt: Runtime<Stream>,
    state: Stream,
    dev: Dev,
    present: bool,
}

impl Driven {
    fn new(state: Stream) -> Self {
        let mut rt = Runtime::new(state.clone(), stream_view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        // The first commit is an instant one: the text lands whole.
        let mut dev = Dev::default();
        dev.commit(&state.content, false, true, 0.0, state.facts());
        Self { rt, state, dev, present: true }
    }

    fn id(&self) -> Option<NodeId> {
        self.rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).map(|n| n.id)
    }

    /// The text's clock, which a commit and the frame step both read.
    fn now(&self) -> f64 { self.id().map_or(self.rt.t, |id| self.rt.scene.store().node_time(id)) }

    /// Changes the text, commits, and checks against dev.
    fn set(&mut self, f: impl FnOnce(&mut Stream)) -> Result<(), String> {
        f(&mut self.state);
        self.commit(false)
    }

    fn commit(&mut self, instant: bool) -> Result<(), String> {
        let before = self.now();
        let state = self.state.clone();
        let was_present = self.present;
        self.rt.update(|s, tx| {
            *s = state.clone();
            if instant {
                tx.instant();
            }
        });
        self.rt.frame(0.0);
        let now = self.now();
        // Its clock changed scale this commit, so its words move with it.
        if now != before {
            self.dev.shift(now - before);
        }
        self.dev.commit(&state.content, was_present, instant, now, state.facts());
        self.dev.tick(now);
        self.present = true;
        self.check()
    }

    fn tick(&mut self, dt: f64) -> Result<(), String> {
        self.rt.frame(dt);
        let now = self.now();
        self.dev.tick(now);
        self.check()
    }

    /// Dev's state against the runtime's, every byte's alpha, and the wake the reveal keeps: a
    /// phase fades exactly while dev has words, and only a fade on a running clock asks for frames.
    fn check(&self) -> Result<(), String> {
        let Some(id) = self.id() else { return Err("no text".to_owned()) };
        let ours = self.rt.reveals.get(&id);
        let theirs = self.dev.state.as_ref();
        match (ours, theirs) {
            (None, None) => {}
            (Some(phase), Some(st)) => {
                if phase.shown() != st.len {
                    return Err(format!("shown {}, dev {}", phase.shown(), st.len));
                }
                if phase.words() != st.words.as_slice() {
                    return Err(format!("words {:?}, dev {:?}", phase.words(), st.words));
                }
                if phase.fading() != !st.words.is_empty() {
                    return Err(format!("fading {}, dev words {:?}", phase.fading(), st.words));
                }
                if let RevealPhase::Fading { fade, .. } = phase && *fade != st.fade {
                    return Err(format!("fade {fade}, dev {}", st.fade));
                }
            }
            (phase, st) => return Err(format!("reveal {phase:?}, dev {st:?}")),
        }
        if self.rt.reveal_chain.until() != self.dev.chain {
            return Err(format!("chain {}, dev {}", self.rt.reveal_chain.until(), self.dev.chain));
        }
        let now = self.now();
        for byte in 0..=self.state.content.len() as u32 {
            let ours = ours.and_then(RevealPhase::fade).map_or(1.0, |fade| fade.alpha(byte, now));
            let theirs = self.dev.alpha(byte, now);
            if ours != theirs {
                return Err(format!("alpha at byte {byte}: {ours}, dev {theirs}"));
            }
        }
        let fading = theirs.is_some_and(|st| !st.words.is_empty());
        let wakes = fading && !self.state.paused;
        let source = self.rt.anim_sources & AnimSource::REVEAL != 0;
        if source != wakes {
            return Err(format!("REVEAL source {source}, dev fading {fading}, paused {}", self.state.paused));
        }
        Ok(())
    }
}

/// A reveal turned off mid-fade stops tracking the text: the bytes that arrive meanwhile fade in
/// from their start when it comes back, whitespace and half words included.
#[test]
fn a_reveal_turned_off_mid_fade_keeps_what_arrives_for_later() {
    let mut w = Driven::new(Stream::streamed("a b"));
    w.set(|s| s.content = "a b c d".to_owned()).unwrap();
    w.set(|s| s.reveal = -1.0).unwrap();
    w.set(|s| s.content = "a b c d  ".to_owned()).unwrap();
    w.tick(1.0).unwrap();
    w.set(|s| s.content = "a b c d  ef".to_owned()).unwrap();
    w.set(|s| s.reveal = FADE).unwrap();
    assert!(w.rt.reveals.values().any(|p| p.words().iter().any(|w| w.0 >= 8)), "the bytes that arrived while off fade in: {:?}", w.rt.reveals);
    w.set(|s| s.content = "a b c d  efg".to_owned()).unwrap();
    w.tick(0.3).unwrap();
}

/// A reveal of zero shows every word at once: a chunk's bytes are shown and no word waits.
#[test]
fn a_reveal_of_zero_shows_future_words_whole() {
    let mut w = Driven::new(Stream::streamed("a b"));
    w.set(|s| s.reveal = 0.0).unwrap();
    w.set(|s| s.content = "a b c d".to_owned()).unwrap();
    w.set(|s| s.content = "a b c d e".to_owned()).unwrap();
    w.tick(0.1).unwrap();
}

/// Under reduced motion every word of a chunk starts together, over the one fade the window uses.
#[test]
fn reduced_motion_fades_every_word_together() {
    let mut w = Driven::new(Stream::streamed("a b"));
    w.set(|s| s.reduced = true).unwrap();
    w.set(|s| s.content = "a b c d e f".to_owned()).unwrap();
    let times: Vec<f64> = w.rt.reveals.values().flat_map(|p| p.words().iter().map(|w| w.1)).collect();
    assert!(times.len() > 1, "the chunk's words fade: {times:?}");
    assert!(times.windows(2).all(|p| p[0] == p[1]), "no stagger, so they start together: {times:?}");
    w.tick(0.2).unwrap();
    w.set(|s| s.reduced = false).unwrap();
    w.set(|s| s.content = "a b c d e f g".to_owned()).unwrap();
    w.tick(0.2).unwrap();
}

/// A clock paused on the text freezes its fade, and its resume moves every word time with the
/// clock: nothing behind it waits, and the chain is left alone while the clock is scaled.
#[test]
fn a_paused_clock_freezes_the_fade_and_its_resume_shifts_the_words() {
    let mut w = Driven::new(Stream::streamed("a b"));
    w.set(|s| s.content = "a b c d".to_owned()).unwrap();
    let before: Vec<f64> = w.rt.reveals.values().flat_map(|p| p.words().iter().map(|w| w.1)).collect();
    let chain = w.rt.reveal_chain.until();
    w.set(|s| s.paused = true).unwrap();
    w.tick(0.2).unwrap();
    assert_eq!(before, w.rt.reveals.values().flat_map(|p| p.words().iter().map(|w| w.1)).collect::<Vec<_>>(), "a frozen clock doesn't fade");
    w.set(|s| s.content = "a b c d e".to_owned()).unwrap();
    let frozen = w.now();
    w.set(|s| s.content = "a b c d e f".to_owned()).unwrap();
    let added: Vec<f64> = w.rt.reveals.values().flat_map(|p| p.words().iter().filter(|w| w.0 >= 9).map(|w| w.1)).collect();
    assert!(added.iter().all(|t| *t >= frozen), "a chunk on a scaled clock starts at its own time: {added:?} from {frozen}");
    assert_eq!(chain, w.rt.reveal_chain.until(), "a scaled clock never joins the chain");
    w.set(|s| s.paused = false).unwrap();
    let resumed = w.now();
    assert!(resumed > frozen, "the clock jumped on resume: {resumed} from {frozen}");
    let shifted: Vec<f64> = w.rt.reveals.values().flat_map(|p| p.words().iter().map(|w| w.1)).collect();
    assert!(shifted.iter().zip(&before).all(|(a, b)| a - b >= resumed - frozen - 1e-9), "every word moved with the clock: {shifted:?} from {before:?}");
    w.tick(0.2).unwrap();
    w.set(|s| s.content = "a b c d e f g".to_owned()).unwrap();
    w.tick(0.2).unwrap();
}

/// A child that never comes back leaves its reveal behind, and so keeps the path that maps to
/// it: only the newest `REVEALS_KEPT` that left are kept for a return, the oldest go.
#[test]
fn removed_children_keep_at_most_the_newest_reveals() {
    use crate::runtime::prune::REVEALS_KEPT;
    #[derive(Default)]
    struct S {
        n: usize,
        present: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().key("root").w(300.0).children(
            s.present.then(|| col().key(format!("p{}", s.n)).child(text("a b c").key("t").w_full().wrap().reveal(FADE))),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mounted = rt.path_ids.len();
    let count = REVEALS_KEPT * 2;
    for n in 0..count {
        rt.update(|s, _| {
            s.n = n;
            s.present = true;
        });
        rt.frame(0.0);
        rt.frame(0.1);
        rt.update(|s, tx| {
            s.present = false;
            tx.instant();
        });
        rt.frame(0.0);
    }
    assert_eq!(rt.reveals.len(), REVEALS_KEPT, "the newest keys' reveals stay and the oldest left");
    assert!(rt.reveals_kept.len() <= REVEALS_KEPT, "{} orphans tracked", rt.reveals_kept.len());
    // A dropped reveal's path leaves with the next commit's `prune_path_ids`.
    rt.update(|s, _| s.n += 1);
    rt.frame(0.0);
    assert!(rt.path_ids.len() <= mounted + REVEALS_KEPT, "{} ids kept for {mounted} mounted paths", rt.path_ids.len());
    let newest = format!("root/p{}/t", count.saturating_sub(1));
    assert!(rt.path_ids.contains_key(&newest), "the newest key's reveal stays for a return");
    assert!(!rt.path_ids.contains_key("root/p0/t"), "the oldest key's reveal left with its path");
}

/// A child that comes back after its parent unmounted instantly keeps its words while its reveal
/// is still one of the newest `REVEALS_KEPT`, however many other keys have left since.
#[test]
fn a_child_returning_within_the_bound_keeps_its_words() {
    use crate::runtime::prune::REVEALS_KEPT;
    #[derive(Default)]
    struct S {
        parent: bool,
        other: usize,
        other_present: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut kids = Vec::new();
        if s.parent {
            kids.push(col().key("p").child(text("a b c").key("t").w_full().wrap().reveal(FADE)));
        }
        if s.other_present {
            kids.push(col().key(format!("o{}", s.other)).child(text("a b c").key("t").w_full().wrap().reveal(FADE)));
        }
        col().key("root").w(300.0).children(kids)
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.parent = true);
    rt.frame(0.0);
    rt.frame(0.1);
    let id = rt.nodes.iter().find(|n| n.path == "root/p/t").unwrap().id;
    assert!(rt.reveals.get(&id).is_some_and(|p| p.caught_up(5)), "the first words have shown");
    rt.update(|s, tx| {
        s.parent = false;
        tx.instant();
    });
    rt.frame(0.0);
    for n in 0..REVEALS_KEPT / 2 {
        rt.update(|s, _| {
            s.other = n;
            s.other_present = true;
        });
        rt.frame(0.0);
        rt.frame(0.1);
        rt.update(|s, tx| {
            s.other_present = false;
            tx.instant();
        });
        rt.frame(0.0);
    }
    assert!(rt.reveals.contains_key(&id), "the returning child's reveal is still within the bound");
    rt.update(|s, _| s.parent = true);
    rt.frame(0.0);
    let back = rt.nodes.iter().find(|n| n.path == "root/p/t").unwrap().id;
    assert_eq!(back, id, "the same key returns under the same id");
    let phase = rt.reveals.get(&back).unwrap();
    assert!(phase.caught_up(5), "it comes back showing its words, not replaying them: {phase:?}");
}
