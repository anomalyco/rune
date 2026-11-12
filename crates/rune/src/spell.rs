//! The platform's spell checker for text fields that ask for it (`El::spellcheck`): Windows'
//! spell checking API (`ISpellChecker`), AppKit's `NSSpellChecker` on macOS, none elsewhere.
//! A field's misspelled words get a red marker under them and their suggestions in its menu.
//!
//! Where the platform checks one language at a time (Windows), an app picks the dictionaries:
//! [`available_languages`], [`set_languages`] and the fields' Spellcheck Languages menu
//! (`El::on_spellcheck_languages`). A word is misspelled when every chosen dictionary says so,
//! as Chromium's multilingual checking does. macOS picks each text's language itself.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "spell/pause.rs"]
pub(crate) mod pause;
#[cfg(target_os = "windows")]
#[path = "spell/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "spell/macos.rs"]
mod platform;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    pub(super) const OFF_THREAD: bool = false;
    pub(super) fn misspelled(_: &str) -> Vec<(usize, usize)> { Vec::new() }
    pub(super) fn suggest(_: &str) -> Vec<String> { Vec::new() }
    pub(super) fn learn(_: &str) {}
    pub(super) fn available() -> Vec<String> { Vec::new() }
    pub(super) fn preferred() -> Vec<String> { Vec::new() }
    pub(super) fn english_name(tag: &str) -> String { tag.to_owned() }
    pub(super) fn name_in(_: &str, _: &str) -> Option<String> { None }
    pub(super) fn sort_in<T>(_: &mut [(String, T)], _: &str) -> bool { false }
}

/// The most suggestions a field's menu offers, as Chromium's.
const MAX_SUGGESTIONS: usize = 5;

/// A text's misspelled words, as byte ranges.
type Ranges = Rc<Vec<(usize, usize)>>;

/// The dictionaries [`set_languages`] chose; empty follows the system languages.
static CHOSEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Bumped on each [`set_languages`], so each thread's checkers and checked texts start over.
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Each checked text's misspelled words, by the text's hash, for one generation.
    static CHECKED: RefCell<(u64, HashMap<u64, Ranges>)> = RefCell::new((0, HashMap::new()));
    /// Each checked word's misspelled parts, for one generation (`Words`): a key typed into a
    /// long draft checks the one word it changed, not the draft.
    static WORDS: RefCell<Words> = RefCell::new(Words::default());
}

/// The most words [`WORDS`] keeps before it starts over.
const MAX_WORDS: usize = 8192;

/// The dictionaries a field's Spellcheck Languages menu offers, as language tags (`en-US`).
/// Empty where the platform finds each text's language itself (macOS) or has no checker.
pub fn available_languages() -> Vec<String> {
    let mut found = platform::available();
    found.sort_by_key(|tag| language_name(tag));
    found
}

/// The dictionaries checking text now: those [`set_languages`] chose that the platform has, else
/// one per system language (matched exactly or by base language, `de-AT` to `de`), with US
/// English added when none of them is English, as Desktop's `defaultSpellcheckLanguages`.
pub fn languages() -> Vec<String> {
    let available = platform::available();
    let chosen = CHOSEN.lock().map(|c| c.clone()).unwrap_or_default();
    let chosen: Vec<String> = available.iter().filter(|tag| chosen.contains(tag)).cloned().collect();
    if !chosen.is_empty() {
        return chosen;
    }
    default_languages(&available, &platform::preferred())
}

/// Check with these dictionaries. None, or none the platform has, follows the system languages.
pub fn set_languages(tags: &[String]) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        if chosen.as_slice() == tags {
            return;
        }
        *chosen = tags.to_vec();
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// A language tag's English name as a browser's `Intl.DisplayNames` gives it: `en-US` is
/// "American English", `fr-FR` "French (France)".
pub fn language_name(tag: &str) -> String {
    DIALECTS.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map_or_else(|| platform::english_name(tag), |(_, name)| (*name).to_owned())
}

/// A language tag's name in `locale`'s language as a browser's `Intl.DisplayNames(locale)`
/// gives it, from the platform's ICU: `en-US` in `de` is "Amerikanisches Englisch". `None`
/// where the platform has no ICU to ask (only Windows has dictionaries to name).
pub(crate) fn language_name_in(tag: &str, locale: &str) -> Option<String> {
    platform::name_in(tag, locale)
}

/// Sorts `items` by their names as `locale` sorts text (`String.localeCompare`), else by their
/// lowercased names.
pub(crate) fn sort_names_in<T>(items: &mut [(String, T)], locale: &str) {
    if !platform::sort_in(items, locale) {
        items.sort_by_cached_key(|(name, _)| name.to_lowercase());
    }
}

/// CLDR's English dialect names, which `Intl.DisplayNames` uses over "Language (Region)".
const DIALECTS: &[(&str, &str)] = &[
    ("ar-001", "Modern Standard Arabic"),
    ("de-AT", "Austrian German"),
    ("de-CH", "Swiss High German"),
    ("en-AU", "Australian English"),
    ("en-CA", "Canadian English"),
    ("en-GB", "British English"),
    ("en-US", "American English"),
    ("es-419", "Latin American Spanish"),
    ("es-ES", "European Spanish"),
    ("es-MX", "Mexican Spanish"),
    ("fa-AF", "Dari"),
    ("fr-CA", "Canadian French"),
    ("fr-CH", "Swiss French"),
    ("nl-BE", "Flemish"),
    ("pt-BR", "Brazilian Portuguese"),
    ("pt-PT", "European Portuguese"),
    ("ro-MD", "Moldavian"),
    ("sw-CD", "Congo Swahili"),
    ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"),
];

/// Desktop's `defaultSpellcheckLanguages` (spellcheck-languages.ts): one dictionary per preferred
/// language, then US English when none of them is English, as prompts mix prose with English
/// code terms.
fn default_languages(available: &[String], preferred: &[String]) -> Vec<String> {
    let base = |tag: &str| tag.split('-').next().unwrap_or(tag).to_ascii_lowercase();
    let find = |tag: &str| available.iter().find(|a| a.eq_ignore_ascii_case(tag)).cloned();
    let mut found: Vec<String> = Vec::new();
    for tag in preferred {
        if let Some(language) = find(tag).or_else(|| find(&base(tag)))
            && !found.iter().any(|f| base(f) == base(&language))
        {
            found.push(language);
        }
    }
    if found.iter().any(|f| base(f) == "en") || !available.iter().any(|a| a == "en-US") {
        return found;
    }
    found.push("en-US".to_owned());
    found
}

/// The current generation, for a thread's checkers to know when to start over.
pub(crate) fn generation() -> u64 { GENERATION.load(Ordering::Relaxed) }

/// The byte ranges of `text`'s misspelled words. Where the platform's checker is slow to call
/// (Windows' answers from another process, a cross-process call per misspelled word), the words
/// it hasn't seen go to a thread of its own and show marked once they're back: the frame never
/// waits on it. `wake` is the window to draw again then.
pub(crate) fn misspelled(text: &str, wake: Option<&std::sync::Arc<crate::live::Wake>>) -> Ranges {
    if text.trim().is_empty() {
        return Rc::default();
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        text.hash(&mut h);
        h.finish()
    };
    CHECKED.with_borrow_mut(|(generation, checked)| {
        if *generation != self::generation() {
            *generation = self::generation();
            checked.clear();
        }
        if let Some(found) = checked.get(&key) {
            return found.clone();
        }
        // Typing checks a new text per key: keep the cache to the latest few hundred.
        if checked.len() > 256 {
            checked.clear();
        }
        // macOS finds each text's language from the text, so it checks the whole of it.
        let (found, whole) = if cfg!(target_os = "windows") { by_words(text, wake) } else { (platform::misspelled(text), true) };
        let found = Rc::new(found);
        // A text with words still on the checker thread is put together again when they're back.
        if whole {
            checked.insert(key, found.clone());
        }
        found
    })
}

/// `text`'s misspelled words, from each line's and word's cached answers, and whether every word
/// had one. The words not yet checked go to the platform together, one per line (on the checker
/// thread where it's slow to call). Words are what whitespace separates: the checker marks
/// nothing across it, and repeated words, its only check that reads neighbours, go unmarked. A
/// line's answer is kept once all its words have one, so a key in a long draft looks up the
/// lines it didn't change, not their words.
fn by_words(text: &str, wake: Option<&std::sync::Arc<crate::live::Wake>>) -> (Vec<(usize, usize)>, bool) {
    use std::hash::{Hash, Hasher};
    let line_key = |line: &str| {
        let mut h = rustc_hash::FxHasher::default();
        line.hash(&mut h);
        h.finish()
    };
    WORDS.with_borrow_mut(|w| {
        if w.generation != self::generation() {
            *w = Words { generation: self::generation(), ..Words::default() };
        }
        w.take_done();
        if w.lines.len() > MAX_WORDS { w.lines.clear(); }
        // The lines without a kept answer, and their words.
        let mut open: Vec<(usize, &str, u64)> = Vec::new();
        let mut at = 0;
        for line in text.split('\n') {
            let k = line_key(line);
            if !w.lines.contains_key(&k) { open.push((at, line, k)); }
            at += line.len() + 1;
        }
        let mut fresh: Vec<&str> = open.iter().flat_map(|&(_, l, _)| words(l).map(|(_, s)| s)).filter(|s| !w.known.contains_key(*s) && !w.asked.contains(*s)).collect();
        fresh.sort_unstable();
        fresh.dedup();
        if !fresh.is_empty() {
            if w.known.len() + fresh.len() > MAX_WORDS { w.known.clear(); }
            if platform::OFF_THREAD {
                w.asked.extend(fresh.iter().map(|&s| Box::<str>::from(s)));
                worker::ask(w.generation, fresh.iter().map(|&s| s.to_owned()).collect(), wake);
            } else {
                w.known.extend(check_words(&fresh));
            }
        }
        let mut whole = true;
        for &(_, line, k) in &open {
            if words(line).all(|(_, s)| w.known.contains_key(s)) {
                let parts: Box<[(u32, u32)]> = words(line).flat_map(|(o, s)| w.known.get(s).into_iter().flatten().map(move |&(a, b)| (o as u32 + a, o as u32 + b))).collect();
                w.lines.insert(k, parts);
            } else {
                whole = false;
            }
        }
        let mut found = Vec::new();
        let mut at = 0;
        for line in text.split('\n') {
            match w.lines.get(&line_key(line)) {
                Some(parts) => found.extend(parts.iter().map(|&(a, b)| (at + a as usize, at + b as usize))),
                None => found.extend(words(line).flat_map(|(o, s)| w.known.get(s).into_iter().flatten().map(move |&(a, b)| (at + o + a as usize, at + o + b as usize)))),
            }
            at += line.len() + 1;
        }
        (found, whole)
    })
}
/// `line`'s words and where each starts in it.
fn words(line: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    line.split_whitespace().map(move |s| (s.as_ptr() as usize - line.as_ptr() as usize, s))
}

/// `words`' misspelled parts, as byte ranges in each word, from one call to the checker: each
/// call costs about the same whatever it's given.
fn check_words<S: AsRef<str>>(words: &[S]) -> Vec<(Box<str>, Box<[(u32, u32)]>)> {
    let batch = words.iter().map(AsRef::as_ref).collect::<Vec<_>>().join("\n");
    let found = platform::misspelled(&batch);
    let mut start = 0;
    words.iter().map(|word| {
        let word = word.as_ref();
        let end = start + word.len();
        let parts = found.iter().filter(|&&(a, b)| a >= start && b <= end).map(|&(a, b)| ((a - start) as u32, (b - start) as u32)).collect();
        start = end + 1;
        (word.into(), parts)
    }).collect()
}

/// A thread's checked words, for one generation of the chosen dictionaries.
#[derive(Default)]
struct Words {
    generation: u64,
    /// Each checked word's misspelled parts, as byte ranges in the word.
    known: HashMap<Box<str>, Box<[(u32, u32)]>>,
    /// Each line's misspelled words once all its words are checked, by the line's hash.
    lines: HashMap<u64, Box<[(u32, u32)]>>,
    /// Words on the checker thread.
    asked: std::collections::HashSet<Box<str>>,
}

impl Words {
    /// Takes in what the checker thread brought back for this generation.
    fn take_done(&mut self) {
        let done = std::mem::take(&mut *worker::DONE.lock().unwrap_or_else(|e| e.into_inner()));
        for (generation, word, parts) in done {
            self.asked.remove(&word);
            if generation == self.generation {
                self.known.insert(word, parts);
            }
        }
    }
}

/// How many times the checker thread has brought words back, for a window to draw them.
pub(crate) fn arrivals() -> u64 { worker::ARRIVED.load(Ordering::Acquire) }

/// The checker thread, where the platform's checker is slow to call.
mod worker {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};

    /// Words to check: the generation they're for, and the window to wake.
    pub(super) struct Job {
        generation: u64,
        words: Vec<String>,
        wake: Option<Weak<crate::live::Wake>>,
    }

    /// Words checked and not taken in yet: generation, word, its misspelled parts.
    pub(super) static DONE: Mutex<Vec<(u64, Box<str>, Box<[(u32, u32)]>)>> = Mutex::new(Vec::new());
    pub(super) static ARRIVED: AtomicU64 = AtomicU64::new(0);
    static JOBS: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();

    pub(super) fn ask(generation: u64, words: Vec<String>, wake: Option<&Arc<crate::live::Wake>>) {
        let jobs = JOBS.get_or_init(|| {
            let (send, take) = mpsc::channel::<Job>();
            std::thread::Builder::new().name("rune-spell".into()).spawn(move || run(take)).ok().map(|_| send)
        });
        if let Some(send) = jobs {
            let _ = send.send(Job { generation, words, wake: wake.map(Arc::downgrade) });
        }
    }

    fn run(take: mpsc::Receiver<Job>) {
        while let Ok(first) = take.recv() {
            // A burst of keys asks for each word they made: check what's waiting in one call.
            let mut jobs = vec![first];
            jobs.extend(take.try_iter());
            let generation = super::generation();
            jobs.retain(|j| j.generation == generation);
            let words: Vec<&str> = jobs.iter().flat_map(|j| j.words.iter().map(String::as_str)).collect();
            // A word still out draws unmarked, as a correct one does: only an answer that marks
            // one changes what a window shows, so only that draws it again. The others are
            // taken when the window next builds (`take_done`).
            let mut marks = false;
            if !words.is_empty() {
                let found = super::check_words(&words);
                marks = found.iter().any(|(_, parts)| !parts.is_empty());
                DONE.lock().unwrap_or_else(|e| e.into_inner()).extend(found.into_iter().map(|(w, p)| (generation, w, p)));
                // The count a window's next frame compares to draw again (`pause::tick`): only
                // what marks a word counts.
                if marks {
                    ARRIVED.fetch_add(1, Ordering::AcqRel);
                }
            }
            if marks {
                for wake in jobs.iter().filter_map(|j| j.wake.as_ref()?.upgrade()) {
                    wake.notify();
                }
            }
        }
    }
}

/// The misspelled word of `text` at byte `at` (in it or at its end), once its words are checked.
pub(crate) fn misspelled_at(text: &str, at: usize) -> Option<(usize, usize)> {
    misspelled(text, None).iter().copied().find(|&(a, b)| a <= at && at <= b)
}
/// The checker's suggestions for misspelled `word`, best first.
pub(crate) fn suggest(word: &str) -> Vec<String> {
    let mut found = platform::suggest(word);
    found.retain(|s| s != word);
    found.truncate(MAX_SUGGESTIONS);
    found
}

/// Adds `word` to the user's dictionary, so it's no longer marked anywhere.
pub(crate) fn learn(word: &str) {
    platform::learn(word);
    CHECKED.with_borrow_mut(|(_, checked)| checked.clear());
    WORDS.with_borrow_mut(|w| *w = Words::default());
}
