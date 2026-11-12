//! Drives every short sequence of streaming-text events through the runtime and checks the
//! reveal after every one against a literal transcription of the code before `RevealPhase`: one
//! `RevealState { len, words, fade }` per text and the window's bare `reveal_chain: f64`, in
//! `commit/apply.rs`, the reveal step in `motion/tracks.rs`, the clock shift in
//! `motion/loop_clock.rs` and the drop in `commit/prune.rs`.
//!
//! The reference sees only what a letter says: the text's content, the fade, whether the text
//! was in the scene at the last commit, whether the commit lands its changes, and the clock.
//! The walk's window has no virtual list, no reduced motion and no `El::time_scale`, so those
//! stay constants on both sides. A phase the walk compares by what shows: the shown bytes, the
//! words still fading and their fade, the window's chain, and every byte's alpha at the text's
//! clock. The fade an `Idle` state carried is the one thing not compared, because nothing reads
//! it until a word fades.
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::el::col;
use crate::{TextStyle, text};

/// Sequences of up to this many letters.
const LENGTH: usize = 6;
/// `El::reveal`: seconds each word takes to fade.
const FADE: f32 = 0.05;
/// Words one `Chunk` brings: at this fade, a chunk's run outlasts it, so a later commit meets a
/// chain longer than a fade.
const CHUNK: usize = 6;
/// The walk's text has no `El::time_scale`, and its window doesn't reduce motion.
const TIME_SCALED: bool = false;
const REDUCED_MOTION: bool = false;

/// What the letters that edit change.
#[derive(Clone, Default, PartialEq)]
struct Doc {
    /// Words the stream has brought.
    words: usize,
    /// A short replacement shows instead of the stream.
    replaced: bool,
    /// The text's mount: every remount (and every reset) is a new node at a new path.
    mounts: u32,
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

    fn path(&self) -> String { format!("root/t{}", self.mounts) }
}

fn view(doc: &Doc, _: &mut Cx<Doc>) -> El {
    col().key("root").w(200.0).child(
        text(doc.content()).key(format!("t{}", doc.mounts)).w_full().wrap().font(TextStyle::new(14.0, 400.0)).reveal(FADE),
    )
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
    /// An instant commit of the same text: what faded is gone.
    Instant,
    /// The stream ends: a commit that changes no content.
    End,
    /// The text remounts as a new node, still entering.
    Remount,
}

const ALPHABET: [Letter; 8] = [
    Letter::Chunk,
    Letter::Tick,
    Letter::Fade,
    Letter::MidChunk,
    Letter::Replace,
    Letter::Instant,
    Letter::End,
    Letter::Remount,
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
    fn commit(&mut self, content: &str, was_present: bool, instant: bool, now: f64) {
        let fade = if REDUCED_MOTION { 0.15 } else { FADE };
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
            let chained = !TIME_SCALED && self.chain - now <= f64::from(fade);
            let chain = if chained { self.chain.max(now) } else { now };
            let queued = ((chain - now) / 0.0055) as usize;
            let pending = st.words.iter().filter(|w| w.1 > now).count() + starts.len() + queued;
            let stagger = if REDUCED_MOTION {
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
            if !TIME_SCALED && let Some(last) = st.words.last() {
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
}

impl Walk {
    fn new() -> Self {
        let mut rt = Runtime::new(Doc::default(), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(0.0);
        assert!(!rt.reduced_motion && !rt.system_reduced_motion, "the walk takes a window that doesn't reduce motion");
        assert!(!rt.motion_needs_cause, "the walk takes a window where every commit keeps its motion");
        assert!(!rt.time_scaled && rt.time_scale == 1.0, "the walk takes a text on the window's clock");
        let mut walk = Self { rt, doc: Doc::default(), dev: Dev::default(), present: true };
        walk.reset();
        walk
    }

    /// A fresh text node with the stream at rest and the reveal as a new window's: every sequence
    /// starts from the same state on both sides.
    fn reset(&mut self) {
        self.doc = Doc { mounts: self.doc.mounts + 1, ..Doc::default() };
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
    fn commit(&mut self, instant: bool) -> Result<(), String> {
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
        self.dev.commit(&content, was_present, instant, now);
        self.dev.tick(now);
        self.present = true;
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
            Letter::Instant => self.commit(true),
            Letter::End => self.commit(false),
            Letter::Remount => {
                self.doc.mounts += 1;
                self.present = false;
                self.dev.left();
                self.commit(false)
            }
        }
    }

    /// The visible reveal and the fade state, against dev's.
    fn compare(&self) -> Result<(), String> {
        let path = self.doc.path();
        let Some(id) = self.rt.nodes.iter().find(|n| n.path == path).map(|n| n.id) else {
            return Err(format!("no text at {path}"));
        };
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
        let now = self.rt.scene.store().node_time(id);
        for byte in 0..=self.doc.content().len() as u32 {
            let ours = ours.map_or(1.0, |phase| phase.alpha(byte, now));
            let theirs = self.dev.alpha(byte, now);
            if ours != theirs {
                return Err(format!("alpha at byte {byte}: {ours}, dev {theirs}"));
            }
        }
        Ok(())
    }
}

/// Plays every sequence that starts with `sequence`, recording the first one that differs.
fn visit(walk: &mut Walk, alphabet: &[Letter], sequence: &mut Vec<Letter>, ran: &mut usize, failure: &mut Option<String>) {
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
    if sequence.len() == LENGTH {
        return;
    }
    for &letter in alphabet {
        sequence.push(letter);
        visit(walk, alphabet, sequence, ran, failure);
        sequence.pop();
    }
}

fn walk_all(alphabet: &[Letter]) {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut walk = Walk::new();
                let (mut ran, mut failure) = (0, None);
                while let Some(&letter) = alphabet.get(next.fetch_add(1, Ordering::Relaxed)) {
                    visit(&mut walk, alphabet, &mut vec![letter], &mut ran, &mut failure);
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
    println!("{total} sequences up to length {LENGTH}");
    assert!(failures.is_empty(), "sequences differ from dev: {failures:#?}");
    let expected: usize = (1..=LENGTH as u32).map(|n| alphabet.len().pow(n)).sum();
    assert_eq!(total, expected, "every sequence ran");
}

#[test]
fn every_short_sequence_of_streaming_events_leaves_what_dev_left() {
    walk_all(&ALPHABET);
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
