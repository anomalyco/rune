//! Syntax highlighting for code: kinds of token ([`Syntax`]) over byte ranges, coloured by a
//! [`SyntaxTheme`]. [`Markdown`](crate::Markdown) colours its code fences with it, and the same
//! output paints any other text, such as code in a composer.
//!
//! ```ignore
//! // The built-in scanners, cached per (language, code):
//! let found = rune_kit::syntax::highlight(Some("rust"), code);
//! // Colour-only spans, shifted to where the code starts in a longer text:
//! let spans: Vec<rune::Span> = SyntaxTheme::from_kit(&kit).spans(&found, code_start);
//! // An app's own highlighter, falling back to the built-in one:
//! let mine = Highlighter::new(|lang, code| match lang {
//!     Some("kotlin") => my_kotlin(code),
//!     _ => rune_kit::syntax::builtin(lang, code),
//! });
//! ```
//!
//! **Languages.** The built-in scanners (no dependencies) know Rust, JavaScript and JSX,
//! TypeScript and TSX, JSON, Python, Go, Swift, shell, SQL, HTML and XML, CSS, YAML and unified
//! diffs, under their common fence names (see [`known`]). Anything else, and code with no
//! language, stays plain: a fence without a language is as often program output or prose as
//! code, so guessing would colour text that isn't code.
//!
//! **Cost.** Each scanner is hand-written, moves only forward and runs in linear time: 0.01 to
//! 0.5 ms per thousand lines on an Apple-silicon Mac in an optimised build, depending on the
//! language. Highlighting is capped rather than lazy: only the first 32 KiB of a source (cut at
//! a line end) and its first 8192 coloured ranges get colour, so a 10,000-line block costs what
//! its first several hundred lines do (under 0.5 ms) and the rest draws plain.
//! An app's highlighter gets the same 32 KiB. Results are cached per highlighter, language and
//! code (every source the last two commits drew, and others up to a memory budget), so
//! rebuilding a view highlights nothing.

mod scan;

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Span, oklch};

use crate::Kit;
use crate::budget::Budget;
use scan::Lang;

/// A kind of token in code, each with its own colour in a [`SyntaxTheme`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Syntax {
    /// Keywords, and markup tag names.
    Keyword,
    /// String, character and regex literals, and YAML's unquoted values.
    String,
    /// Numbers, and constants such as `true`, `null` and `ALL_CAPS` names.
    Number,
    /// Comments, and a diff's header lines.
    Comment,
    /// Type names (capitalised names and built-in types), CSS classes and ids, YAML tags.
    Type,
    /// Functions where they are called or defined, macros, and shell commands.
    Function,
    /// JSON and YAML keys, CSS properties, markup attributes, and shell and SQL variables.
    Property,
    /// Brackets, operators and separators.
    Punctuation,
    /// A diff's added lines.
    Inserted,
    /// A diff's removed lines.
    Deleted,
    /// A function's parameters where it's defined (TextMate's `variable.parameter`), in the
    /// text colour by default.
    Parameter,
}

impl Syntax {
    pub(crate) const COUNT: usize = 11;
}

/// What colours a highlighted range: a kind of token, coloured by the [`SyntaxTheme`] in use,
/// or a colour of the highlighter's own. Colour is all a highlight changes, never weight or
/// style, so highlighting never moves a glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Kind(Syntax),
    Color(Color),
}

impl From<Syntax> for Paint {
    fn from(k: Syntax) -> Paint {
        Paint::Kind(k)
    }
}

impl From<Color> for Paint {
    fn from(c: Color) -> Paint {
        Paint::Color(c)
    }
}

/// A highlighted byte range of the code.
pub type Highlight = (Range<usize>, Paint);

/// Whether the built-in scanners colour `lang`, a fence's language name (case-insensitive):
/// `rust`/`rs`, `js`/`javascript`/`jsx`/`mjs`/`cjs`, `ts`/`typescript`/`mts`/`cts`, `tsx`,
/// `json`/`jsonc`/`json5`, `python`/`py`/`python3`/`py3`/`pyi`, `go`/`golang`, `swift`,
/// `sh`/`bash`/`zsh`/`shell`/`shellscript`, `sql`/`postgres`/`postgresql`/`mysql`/`sqlite`,
/// `html`/`htm`/`xhtml`/`xml`/`svg`, `css`, `yaml`/`yml` and `diff`/`patch`.
pub fn known(lang: &str) -> bool {
    Lang::from_name(lang).is_some()
}

/// The built-in scanners' highlights of `code`, in order and not overlapping, uncached: empty
/// for a language they don't know or none. For an app's [`Highlighter`] to fall back on; to
/// highlight directly, use the cached [`highlight`].
pub fn builtin(lang: Option<&str>, code: &str) -> Vec<Highlight> {
    let Some(lang) = lang.and_then(Lang::from_name) else { return Vec::new() };
    scan::highlight(lang, code).into_iter().map(|(a, b, k)| (a as usize..b as usize, Paint::Kind(k))).collect()
}

/// [`builtin`], cached per language and code: the same call with the same code returns the
/// same `Rc` without scanning again.
pub fn highlight(lang: Option<&str>, code: &str) -> Rc<[Highlight]> {
    Highlighter::builtin().highlight(lang, code)
}

type HighlightFn = dyn Fn(Option<&str>, &str) -> Vec<Highlight>;

/// Turns code into highlights: the built-in scanners ([`Highlighter::builtin`], the default),
/// or an app's own function, such as a syntect or tree-sitter highlighter.
///
/// The function gets the code's language as written (a fence's first info word, `None` when
/// it has none) and the code, and returns byte ranges of it with a [`Paint`]. Ranges may come
/// in any order; ones out of bounds, inside a character, empty or overlapping an earlier one
/// are dropped.
///
/// Output is cached per highlighter, language and code. Highlighters made from the same closure
/// share that cache, so one whose output depends on what it captures (a light or dark theme,
/// say) must name that with [`key`](Self::key).
#[derive(Clone)]
pub struct Highlighter {
    f: Option<Rc<HighlightFn>>,
    id: u64,
}

impl Highlighter {
    /// The built-in scanners (see the module docs for the languages).
    pub fn builtin() -> Highlighter {
        Highlighter { f: None, id: 0 }
    }

    pub fn new<F: Fn(Option<&str>, &str) -> Vec<Highlight> + 'static>(f: F) -> Highlighter {
        Highlighter { f: Some(Rc::new(f)), id: hash_of(TypeId::of::<F>()) | 1 }
    }

    /// Keeps this highlighter's cached output apart from others made from the same closure:
    /// give it whatever its output depends on besides the language and code.
    pub fn key(mut self, key: impl Hash) -> Highlighter {
        self.id = hash_of((self.id, key)) | 1;
        self
    }

    /// The highlights of `code`'s first 32 KiB (cut at a line end), cached.
    pub fn highlight(&self, lang: Option<&str>, code: &str) -> Rc<[Highlight]> {
        let code = &code[..scan::coloured_len(code)];
        let key = hash_of((self.id, lang, code));
        if let Some(out) = CACHE.with_borrow_mut(|c| {
            c.get(&key)
                .filter(|e| e.id == self.id && e.lang.as_deref() == lang && e.src == code)
                .map(|e| e.out.clone())
        }) {
            return out;
        }
        let out: Rc<[Highlight]> = match &self.f {
            None => builtin(lang, code).into(),
            Some(f) => tidy(f(lang, code), code).into(),
        };
        let bytes = code.len() + out.len() * size_of::<Highlight>() + 96;
        CACHE.with_borrow_mut(|c| {
            c.insert(key, Entry { id: self.id, lang: lang.map(Into::into), src: code.to_owned(), out: out.clone() }, bytes);
        });
        out
    }

    /// Whether this is [`Highlighter::builtin`], which leaves languages [`known`] doesn't list plain.
    pub fn is_builtin(&self) -> bool {
        self.f.is_none()
    }
}

impl Default for Highlighter {
    fn default() -> Highlighter {
        Highlighter::builtin()
    }
}

impl fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Highlighter").field("builtin", &self.f.is_none()).field("id", &self.id).finish()
    }
}

/// An app highlighter's ranges, sorted, in bounds, on character boundaries and not
/// overlapping, at most [`scan::MAX_TOKENS`] of them.
fn tidy(mut v: Vec<Highlight>, src: &str) -> Vec<Highlight> {
    v.retain(|(r, _)| r.start < r.end && r.end <= src.len() && src.is_char_boundary(r.start) && src.is_char_boundary(r.end));
    v.sort_by_key(|(r, _)| r.start);
    let mut last = 0;
    v.retain(|(r, _)| {
        let keep = r.start >= last;
        if keep {
            last = r.end;
        }
        keep
    });
    v.truncate(scan::MAX_TOKENS);
    v
}

/// Estimated bytes of highlights kept beyond those on screen (see [`Budget`]).
const CACHE_BYTES: usize = 4 << 20;

struct Entry {
    id: u64,
    lang: Option<Box<str>>,
    src: String,
    out: Rc<[Highlight]>,
}

thread_local! {
    static CACHE: RefCell<Budget<u64, Entry>> = RefCell::new(Budget::new(CACHE_BYTES));
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// A colour per kind of token. [`SyntaxTheme::from_kit`] follows the kit's light or dark
/// theme; [`SyntaxTheme::with`] overrides one kind.
#[derive(Clone, Copy, Debug, PartialEq, Hash)]
pub struct SyntaxTheme {
    colors: [Color; Syntax::COUNT],
}

impl SyntaxTheme {
    /// Muted token colours: the familiar hues (violet keywords, green strings, orange numbers,
    /// teal types, blue functions, rose properties) at one lightness a step from the text's, so
    /// contrast follows the theme. Comments and punctuation are the theme's quieter text greys,
    /// and a diff's lines its success and danger colours.
    ///
    /// The hues are fixed rather than taken from the theme's status colours: those vary too much
    /// between themes (OpenCode's "info" is magenta) to keep the token kinds apart.
    pub fn from_kit(k: &Kit) -> SyntaxTheme {
        let c = &k.c;
        let text_l = c.text_base.to_oklab()[0];
        let (l, chroma) =
            if k.dark { ((text_l - 0.16).clamp(0.62, 0.84), 0.1) } else { ((text_l + 0.3).clamp(0.38, 0.52), 0.12) };
        let paint = |h: f32| oklch(l, chroma, h);
        let mut s = [c.text_base; Syntax::COUNT];
        s[Syntax::Keyword as usize] = paint(305.0);
        s[Syntax::String as usize] = paint(145.0);
        s[Syntax::Number as usize] = paint(60.0);
        s[Syntax::Comment as usize] = c.text_muted.mix(c.text_faint, 0.4);
        s[Syntax::Type as usize] = paint(195.0);
        s[Syntax::Function as usize] = paint(255.0);
        s[Syntax::Property as usize] = paint(15.0);
        s[Syntax::Punctuation as usize] = c.text_muted;
        s[Syntax::Inserted as usize] = c.success.icon;
        s[Syntax::Deleted as usize] = c.danger.icon;
        SyntaxTheme { colors: s }
    }

    /// Rune's neutral light or dark look ([`Kit::neutral`]'s tokens), for apps without a kit.
    pub fn neutral(dark: bool) -> SyntaxTheme {
        SyntaxTheme::from_kit(&Kit::neutral(dark))
    }

    pub fn color(&self, kind: Syntax) -> Color {
        self.colors[kind as usize]
    }

    /// Colours one kind of token `c`.
    pub fn with(mut self, kind: Syntax, c: Color) -> SyntaxTheme {
        self.colors[kind as usize] = c;
        self
    }

    pub fn paint(&self, p: Paint) -> Color {
        match p {
            Paint::Kind(k) => self.color(k),
            Paint::Color(c) => c,
        }
    }

    /// Colour-only [`Span`]s for `highlights`, each range moved `offset` bytes on: where the
    /// code starts in the text the spans style.
    pub fn spans(&self, highlights: &[Highlight], offset: usize) -> Vec<Span> {
        highlights
            .iter()
            .map(|(r, p)| {
                let mut sp = Span::new(r.start + offset, r.end + offset);
                sp.color = Some(self.paint(*p));
                sp
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_cache_is_keyed_by_highlighter_language_and_source() {
        let a = highlight(Some("js"), "let x = 1");
        assert!(Rc::ptr_eq(&a, &highlight(Some("js"), "let x = 1")));
        assert!(!Rc::ptr_eq(&a, &highlight(Some("sql"), "let x = 1")));
        assert!(!Rc::ptr_eq(&a, &highlight(Some("js"), "let x = 2")));
        let mine = Highlighter::new(|_, code| vec![(0..code.len(), Paint::Kind(Syntax::Comment))]);
        assert!(!Rc::ptr_eq(&a, &mine.highlight(Some("js"), "let x = 1")));
    }

    #[test]
    fn an_app_highlighter_runs_once_per_language_and_code() {
        thread_local!(static RUNS: Cell<usize> = const { Cell::new(0) });
        let make = || {
            Highlighter::new(|lang: Option<&str>, code: &str| {
                RUNS.set(RUNS.get() + 1);
                vec![(0..code.len().min(3), Paint::Color(if lang == Some("x") { Color::WHITE } else { Color::BLACK }))]
            })
        };
        for _ in 0..3 {
            // A fresh closure each time, as a view rebuilt every frame makes.
            make().highlight(Some("x"), "abcdef");
        }
        assert_eq!(RUNS.get(), 1);
        make().highlight(None, "abcdef");
        make().highlight(Some("x"), "abcdeg");
        assert_eq!(RUNS.get(), 3);
        make().key("dark").highlight(Some("x"), "abcdef");
        assert_eq!(RUNS.get(), 4, "a keyed highlighter has its own entries");
        let delegating = Highlighter::new(|lang, code| highlight(lang, code).to_vec());
        assert_eq!(delegating.highlight(Some("rust"), "let x = 1"), highlight(Some("rust"), "let x = 1"));
    }

    #[test]
    fn an_app_highlighters_ranges_are_tidied() {
        let h = Highlighter::new(|_, _| {
            let k = |s: Syntax| Paint::Kind(s);
            vec![(6..9, k(Syntax::Number)), (0..3, k(Syntax::Keyword)), (2..5, k(Syntax::String)), (4..4, k(Syntax::Type)),
                (9..40, k(Syntax::Comment)), (11..12, k(Syntax::Comment)), (13..15, k(Syntax::Type))]
        });
        // "é" is bytes 10..12, so 11..12 starts inside it.
        let got: Vec<_> = h.highlight(None, "let x = 1 é ok").iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(got, [0..3, 6..9, 13..15]);
    }

    #[test]
    fn builtin_knows_its_languages_and_leaves_the_rest_plain() {
        for (name, code) in [("rust", "x = 1"), ("TS", "x = 1"), ("tsx", "x = 1"), ("py", "x = 1"), ("Go", "x = 1"),
            ("json", "1"), ("swift", "x = 1"), ("bash", "x"), ("sql", "1"), ("html", "<a>"), ("css", "a{b:1}"), ("yml", "a: 1"),
            ("diff", "+a")]
        {
            assert!(known(name), "{name}");
            assert!(!builtin(Some(name), code).is_empty(), "{name}");
        }
        for lang in [None, Some("brainfuck"), Some(""), Some("text")] {
            assert!(builtin(lang, "let x = 1").is_empty(), "{lang:?}");
        }
    }

    #[test]
    fn spans_colour_and_offset() {
        let theme = SyntaxTheme::neutral(true).with(Syntax::Keyword, Color::WHITE);
        let spans = theme.spans(&[(0..3, Paint::Kind(Syntax::Keyword)), (4..5, Paint::Color(Color::BLACK))], 10);
        assert_eq!((spans[0].start, spans[0].end, spans[0].color), (10, 13, Some(Color::WHITE)));
        assert_eq!((spans[1].start, spans[1].end, spans[1].color), (14, 15, Some(Color::BLACK)));
        assert!(spans.iter().all(|s| s.weight.is_none() && !s.italic && !s.mono && s.size.is_none() && s.bg.is_none()));
    }

    #[test]
    fn default_token_colours_are_distinct_in_both_themes() {
        for dark in [false, true] {
            let s = SyntaxTheme::neutral(dark).colors;
            for i in 0..s.len() {
                for j in 0..i {
                    assert_ne!(s[i], s[j], "dark {dark}: tokens {i} and {j}");
                }
            }
        }
    }
}
