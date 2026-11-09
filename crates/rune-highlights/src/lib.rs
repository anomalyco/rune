//! Pierre's Highlights, the code highlighter OpenCode's desktop app colours diffs, files and
//! Markdown with (`@pierre/highlights`, Apache-2.0), as Rust.
//!
//! Highlights is a set of hand-written lexers, one per language (80 of them), shipped as one
//! WebAssembly module. `build.rs` translates that module, `vendor/highlights.wasm`, to Rust at
//! build time: the same lexers run natively, with no WebAssembly runtime, on every target Rune
//! builds for (web included), and give the very tokens the desktop app draws.
//!
//! ```
//! use rune_highlights::{Highlighter, Lang};
//! let mut h = Highlighter::new();
//! let lang = Lang::from_name("kotlin").unwrap();
//! for (range, token) in h.highlight(lang, "val x = 1") {
//!     println!("{range:?} {}", token.name()); // 0..4 keyword, ...
//! }
//! ```
//!
//! Token types are Zed's capture names (`keyword.control`, `string.escape`); a theme colours a
//! name or, failing that, its parent (`keyword`).
//!
//! **Cost.** About 0.1 to 0.6 ms per thousand lines in an optimised build, linear in the
//! source, and the module's memory (three 64 KiB pages, plus room for the source and its
//! tokens) per [`Highlighter`].

mod env;
mod langs;
mod rt;

include!(concat!(env!("OUT_DIR"), "/highlights.rs"));

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use rt::PAGE;

/// A language Highlights lexes, or [`Lang::PLAIN`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lang(u8);

impl Lang {
    /// Text without a lexer: no tokens.
    pub const PLAIN: Lang = Lang(0);

    /// The language `name` names, ignoring ASCII case: Highlights' names and aliases (`ts`,
    /// `typescript`, `c++`, `dockerfile`, `text`), as its `isSupportedLanguage` accepts them.
    pub fn from_name(name: &str) -> Option<Lang> {
        let name = name.to_ascii_lowercase();
        langs::ALIASES.binary_search_by(|(k, _)| (*k).cmp(name.as_str())).ok().map(|i| Lang(langs::ALIASES[i].1))
    }

    /// The language's main name (`typescript`, `cpp`, `plain`).
    pub fn name(self) -> &'static str {
        langs::NAMES[self.0 as usize]
    }

    /// Every language with a lexer, in Highlights' order.
    pub fn all() -> impl Iterator<Item = Lang> {
        (1..langs::NAMES.len() as u8).map(Lang)
    }

    /// Every name and alias of a language with a lexer, with its language.
    pub fn aliases() -> impl Iterator<Item = (&'static str, Lang)> {
        langs::ALIASES.iter().filter(|(_, l)| *l != 0).map(|&(k, l)| (k, Lang(l)))
    }
}

/// A token type: one of Highlights' Zed capture names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(u8);

impl Token {
    /// Text no lexer rule matched; [`Highlighter::highlight`] leaves it out.
    pub const NONE: Token = Token(0);
    /// How many token types there are, [`Token::NONE`] included.
    pub const COUNT: usize = langs::TOKENS.len();

    /// Zed's capture name (`keyword.control`, `punctuation.bracket`).
    pub fn name(self) -> &'static str {
        langs::TOKENS[self.0 as usize]
    }

    /// Its index in [`Token::all`], below [`Token::COUNT`].
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn from_name(name: &str) -> Option<Token> {
        langs::TOKENS.iter().position(|t| *t == name).map(|i| Token(i as u8))
    }

    pub fn all() -> impl Iterator<Item = Token> {
        (0..Self::COUNT as u8).map(Token)
    }
}

/// One instance of the lexers: their memory, reused from call to call. Not shared between
/// threads; make one per thread.
pub struct Highlighter {
    wasm: Wasm,
}

/// Memory a highlighter keeps after a large source; past this it starts over, as Highlights'
/// own host does.
const KEEP_PAGES: usize = 128;

/// Where the source starts in the module's memory, and the room it needs after its end.
const INPUT: usize = PAGE;
const SLACK: usize = 96;

/// Output mode 3: (UTF-16 end, token type) records per run, `u32::MAX` types marking where
/// each next line starts.
const RECORDS: u8 = 3;

impl Highlighter {
    pub fn new() -> Highlighter {
        Highlighter { wasm: Wasm::new() }
    }

    /// `src`'s tokens as `lang` lexes them: byte ranges in order, not overlapping, without the
    /// runs no rule matched ([`Token::NONE`]) and without line ends. Empty for
    /// [`Lang::PLAIN`].
    pub fn highlight(&mut self, lang: Lang, src: &str) -> Vec<(Range<usize>, Token)> {
        if lang == Lang::PLAIN || src.is_empty() {
            return Vec::new();
        }
        let records = catch_unwind(AssertUnwindSafe(|| self.records(lang, src)));
        if self.wasm.mem.len() > KEEP_PAGES * PAGE {
            self.wasm = Wasm::new();
        }
        match records {
            Ok(r) => runs(src, &r),
            // A trap is a bug in the lexers; the code draws plain rather than taking the app down.
            Err(_) => {
                self.wasm = Wasm::new();
                Vec::new()
            }
        }
    }

    fn records(&mut self, lang: Lang, src: &str) -> Vec<u32> {
        records(&mut self.wasm, lang, src, None)
    }
}

impl Default for Highlighter {
    fn default() -> Highlighter {
        Highlighter::new()
    }
}

/// Lexes a source that grows at its end, such as a code block streaming in: each call lexes
/// only the lines after those an earlier call lexed, carrying the lexer's state over, as
/// Highlights' own `StreamTokenizer` does. The tokens are the same as [`Highlighter::highlight`]
/// gives for the whole source.
pub struct Stream {
    lang: Lang,
    /// The lexers' state after `text`.
    wasm: Wasm,
    /// The lines lexed so far, each with its line end.
    text: String,
    runs: Vec<(Range<usize>, Token)>,
    /// Where the last line, still without its line end, is lexed from a copy of `wasm`.
    tail: Option<Box<Wasm>>,
}

impl Stream {
    pub fn new(lang: Lang) -> Stream {
        Stream { lang, wasm: Wasm::new(), text: String::new(), runs: Vec::new(), tail: None }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Whether `src` extends what this stream has lexed, so [`highlight`](Self::highlight)
    /// lexes only what it adds.
    pub fn continues(&self, src: &str) -> bool {
        !self.text.is_empty() && src.starts_with(&self.text)
    }

    /// `src`'s tokens, as [`Highlighter::highlight`] gives them. When `src` doesn't extend the
    /// source of earlier calls, the stream starts over.
    pub fn highlight(&mut self, src: &str) -> Vec<(Range<usize>, Token)> {
        if self.lang == Lang::PLAIN {
            return Vec::new();
        }
        match catch_unwind(AssertUnwindSafe(|| self.lex(src))) {
            Ok(runs) => runs,
            Err(_) => {
                *self = Stream::new(self.lang);
                Vec::new()
            }
        }
    }

    fn lex(&mut self, src: &str) -> Vec<(Range<usize>, Token)> {
        if !self.continues(src) {
            self.wasm = Wasm::new();
            self.text.clear();
            self.runs.clear();
        }
        let lines = src.rfind('\n').map_or(0, |i| i + 1);
        if lines > self.text.len() {
            let (at, chunk) = (self.text.len(), &src[self.text.len()..lines]);
            let r = records(&mut self.wasm, self.lang, chunk, Some(at == 0));
            self.runs.extend(runs(chunk, &r).into_iter().map(|(range, t)| (range.start + at..range.end + at, t)));
            self.text.push_str(chunk);
        }
        let mut out = self.runs.clone();
        if lines < src.len() {
            let tail = self.tail.get_or_insert_with(|| Box::new(self.wasm.clone()));
            (**tail).clone_from(&self.wasm);
            let chunk = &src[lines..];
            let r = records(tail, self.lang, chunk, Some(lines == 0));
            out.extend(runs(chunk, &r).into_iter().map(|(range, t)| (range.start + lines..range.end + lines, t)));
        }
        out
    }
}

/// Runs `src` through the module in output mode [`RECORDS`]: whole (`stream: None`), or as the
/// next chunk of a stream (`Some(true)` for its first), and returns the records.
fn records(w: &mut Wasm, lang: Lang, src: &str, stream: Option<bool>) -> Vec<u32> {
    let len = src.len();
    let need = (1 + (len + SLACK).div_ceil(PAGE)) * PAGE;
    if w.mem.len() < need {
        w.mem.resize(need, 0);
    }
    w.mem[INPUT..INPUT + len].copy_from_slice(src.as_bytes());
    w.mem[INPUT + len] = 0;
    w.mem[0] = lang.0;
    w.mem[1] = RECORDS;
    w.mem[2..6].copy_from_slice(&(len as u32).to_le_bytes());
    match stream {
        None => w.highlight(),
        Some(first) => w.highlight_stream(first as i32),
    }
    let word = |at: usize| u32::from_le_bytes([w.mem[at], w.mem[at + 1], w.mem[at + 2], w.mem[at + 3]]) as usize;
    let (at, n) = (word(6), word(10));
    w.mem[at..at + n].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// The records' runs as byte ranges of `src`, typed runs only.
fn runs(src: &str, records: &[u32]) -> Vec<(Range<usize>, Token)> {
    let mut out = Vec::with_capacity(records.len() / 2);
    let mut cursor = Utf16Cursor { src, byte: 0, unit: 0 };
    let mut start = 0usize;
    for pair in records.chunks_exact(2) {
        let (end, ty) = (pair[0] as usize, pair[1]);
        if ty == u32::MAX {
            start = cursor.byte_at(end);
            continue;
        }
        let end = cursor.byte_at(end);
        if ty != 0 && end > start && (ty as usize) < Token::COUNT {
            out.push((start..end, Token(ty as u8)));
        }
        start = end;
    }
    out
}

/// Walks `src` forward, turning UTF-16 offsets into byte offsets.
struct Utf16Cursor<'a> {
    src: &'a str,
    byte: usize,
    unit: usize,
}

impl Utf16Cursor<'_> {
    fn byte_at(&mut self, unit: usize) -> usize {
        if unit < self.unit {
            *self = Utf16Cursor { src: self.src, byte: 0, unit: 0 };
        }
        for c in self.src[self.byte..].chars() {
            if self.unit >= unit {
                break;
            }
            self.unit += c.len_utf16();
            self.byte += c.len_utf8();
        }
        self.byte
    }
}
