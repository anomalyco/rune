//! Syntax highlighting for code: kinds of token ([`Syntax`]) over byte ranges, coloured by a
//! [`SyntaxTheme`]. [`Markdown`](crate::Markdown) colours its code fences with it, and the same
//! output paints any other text, such as code in a composer or a diff.
//!
//! ```ignore
//! // The built-in lexers, cached per (language, code):
//! let found = rune_kit::syntax::highlight(Some("rust"), code);
//! // Colour-only spans, shifted to where the code starts in a longer text:
//! let spans: Vec<rune::Span> = SyntaxTheme::from_kit(&kit).spans(&found, code_start);
//! // An app's own highlighter, falling back to the built-in one:
//! let mine = Highlighter::new(|lang, code| match lang {
//!     Some("brainfuck") => my_brainfuck(code),
//!     _ => rune_kit::syntax::builtin(lang, code),
//! });
//! ```
//!
//! **Languages.** The built-in highlighter is the one OpenCode's desktop app draws with:
//! Pierre's Highlights ([`rune_highlights`]), 80 hand-written lexers translated to Rust at
//! build time, under the same names and aliases (see [`known`]): C, C++, C#, Java, Kotlin, Go,
//! Rust, Swift, Python, Ruby, PHP, Lua, shell, PowerShell, SQL, the web languages and their
//! frameworks, data and config formats, Dockerfiles and Makefiles, and more. [`lang_for_path`]
//! names a file's language as Desktop's diffs and file views do. Anything else, and code with
//! no language, stays plain: a fence without a language is as often program output or prose as
//! code, so guessing would colour text that isn't code.
//!
//! **Kinds.** Each lexer token (a Zed capture name such as `keyword.control`) maps to the
//! [`Syntax`] kind whose colour Desktop's theme gives it ([`kind`]), so a theme that colours
//! the kinds as Desktop's `syntax-*` tokens matches it token for token.
//!
//! **Cost.** The lexers move only forward and run in linear time, 0.1 to 0.6 ms per thousand
//! lines in an optimised build. Only the first 128 KiB of a source (cut at a line end) and its
//! first 32,768 coloured ranges get colour; the rest draws plain. An app's highlighter gets the
//! same 128 KiB. Results are cached per highlighter, language and code (every source the last
//! two commits drew, and others up to a memory budget), so rebuilding a view highlights nothing.

mod paths;

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::OnceLock;

use rune::{Color, Span, oklch};
use rune_highlights::{Lang, Stream, Token};

use crate::Kit;
use crate::budget::Budget;

pub use paths::lang_for_path;

/// A kind of token in code, each with its own colour in a [`SyntaxTheme`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Syntax {
    /// Keywords, preprocessor directives, markup tags, CSS selectors and a link's target.
    Keyword,
    /// String and character literals, and literal text.
    String,
    /// Numbers, booleans and other constants, escapes, labels and namespaces.
    Number,
    /// Comments.
    Comment,
    /// Types, classes, constructors, enums and components.
    Type,
    /// Functions where they are called or defined, and CSS ids.
    Function,
    /// Properties and object or JSON keys.
    Property,
    /// Brackets, separators and list markers.
    Punctuation,
    /// A diff's added lines.
    Inserted,
    /// A diff's removed lines.
    Deleted,
    /// A function's parameters where it's defined (TextMate's `variable.parameter`), in the
    /// text colour by default.
    Parameter,
    /// A Markdown heading's whole line (TextMate's `markup.heading`, Zed's `title`), which
    /// editors often draw bold as well.
    Heading,
    /// A Markdown link's text.
    Link,
    /// Operators, and CSS pseudo-classes.
    Operator,
    /// Regular expression literals.
    Regex,
    /// Markup attributes and CSS classes.
    Attribute,
    /// Variables.
    Variable,
}

impl Syntax {
    pub(crate) const COUNT: usize = 17;
}

/// What colours a highlighted range: a kind of token, coloured by the [`SyntaxTheme`] in use,
/// or a colour of the highlighter's own.
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

/// Bytes coloured per source; past this (cut at a line end) the code draws plain.
const MAX_SOURCE: usize = 128 * 1024;
/// Coloured ranges per source; past this the code draws plain.
const MAX_TOKENS: usize = 32 * 1024;

/// Whether the built-in highlighter colours `lang`, a fence's language name or alias
/// (case-insensitive): `rust`/`rs`, `ts`/`typescript`, `c++`/`cpp`, `c#`/`cs`, `kotlin`/`kt`,
/// `rb`, `php`, `lua`, `sh`/`bash`/`zsh`, `ps1`, `dockerfile`, `makefile`, `yml` and the rest
/// of Highlights' 211 names ([`rune_highlights::Lang::aliases`]). Not `text` or `plain`.
pub fn known(lang: &str) -> bool {
    Lang::from_name(lang).is_some_and(|l| l != Lang::PLAIN)
}

/// The built-in highlighter's highlights of `code`, in order and not overlapping, uncached:
/// empty for a language it doesn't know or none. For an app's [`Highlighter`] to fall back
/// on; to highlight directly, use the cached [`highlight`].
pub fn builtin(lang: Option<&str>, code: &str) -> Vec<Highlight> {
    let Some(lang) = lang.and_then(Lang::from_name) else { return Vec::new() };
    let code = &code[..coloured_len(code)];
    let tokens = LEXERS.with_borrow_mut(|l| l.tokens(lang, code));
    let mut out: Vec<Highlight> = Vec::with_capacity(tokens.len());
    for (r, token) in tokens {
        let Some(k) = kind(token) else { continue };
        if let Some((last, Paint::Kind(prev))) = out.last_mut()
            && last.end == r.start
            && *prev == k
        {
            last.end = r.end;
            continue;
        }
        if out.len() == MAX_TOKENS {
            break;
        }
        out.push((r, Paint::Kind(k)));
    }
    out
}

/// The kind Desktop's highlight theme (`highlight-theme.tsx`) colours a lexer token as: its
/// capture name's, or failing that its nearest parent's (`keyword.control` is `keyword`).
/// `None` for text drawn in the code's own colour (embedded code, emphasis, JSX text).
pub fn kind(token: Token) -> Option<Syntax> {
    static TABLE: OnceLock<Vec<Option<Syntax>>> = OnceLock::new();
    TABLE.get_or_init(|| Token::all().map(|t| resolve(t.name())).collect())[token.index()]
}

fn resolve(mut name: &str) -> Option<Syntax> {
    loop {
        if let Some((_, k)) = KINDS.iter().find(|(n, _)| *n == name) {
            return *k;
        }
        name = &name[..name.rfind('.')?];
    }
}

/// Desktop's theme, capture name to kind; names not here take their parent's.
const KINDS: &[(&str, Option<Syntax>)] = {
    use Syntax::*;
    &[
        ("attribute", Some(Attribute)),
        ("boolean", Some(Number)),
        ("comment", Some(Comment)),
        ("constant", Some(Number)),
        ("constructor", Some(Type)),
        ("diff.minus", Some(Deleted)),
        ("diff.plus", Some(Inserted)),
        ("embedded", None),
        ("emphasis", None),
        ("enum", Some(Type)),
        ("function", Some(Function)),
        ("hint", Some(Comment)),
        ("keyword", Some(Keyword)),
        ("keyword.operator", Some(Operator)),
        ("label", Some(Number)),
        ("link_text", Some(Link)),
        ("link_uri", Some(Keyword)),
        ("namespace", Some(Number)),
        ("number", Some(Number)),
        ("operator", Some(Operator)),
        ("predictive", Some(Comment)),
        ("preproc", Some(Keyword)),
        ("primary", Some(Link)),
        ("property", Some(Property)),
        ("punctuation", Some(Punctuation)),
        ("punctuation.special", Some(Keyword)),
        ("selector", Some(Keyword)),
        ("selector.class", Some(Attribute)),
        ("selector.id", Some(Function)),
        ("selector.pseudo", Some(Operator)),
        ("string", Some(String)),
        ("string.escape", Some(Number)),
        ("string.regex", Some(Regex)),
        ("string.special", Some(Number)),
        ("tag", Some(Keyword)),
        ("tag.component.jsx", Some(Type)),
        ("text", None),
        ("text.literal", Some(String)),
        ("title", Some(Heading)),
        ("type", Some(Type)),
        ("variable", Some(Variable)),
        ("variable.parameter", Some(Parameter)),
        ("variable.special", Some(Number)),
        ("variant", Some(Number)),
    ]
};

thread_local! {
    static LEXERS: RefCell<Lexers> = RefCell::new(Lexers::default());
}

/// The thread's lexers: one for whole sources, and a [`Stream`] for a source growing at its end
/// (a code block streaming in), which then costs only its new lines.
#[derive(Default)]
struct Lexers {
    whole: rune_highlights::Highlighter,
    stream: Option<Stream>,
    /// The last source lexed whole, to spot one that grows.
    last: Option<(Lang, String)>,
}

impl Lexers {
    fn tokens(&mut self, lang: Lang, code: &str) -> Vec<(Range<usize>, Token)> {
        if let Some(s) = self.stream.as_mut().filter(|s| s.lang() == lang && s.continues(code)) {
            return s.highlight(code);
        }
        if self.last.as_ref().is_some_and(|(l, prev)| *l == lang && code.len() > prev.len() && code.starts_with(prev.as_str())) {
            self.last = None;
            return self.stream.insert(Stream::new(lang)).highlight(code);
        }
        let last = self.last.get_or_insert_with(|| (lang, String::new()));
        last.0 = lang;
        last.1.clear();
        last.1.push_str(code);
        self.whole.highlight(lang, code)
    }
}

/// How much of `src` gets colour: all of it, or up to the last line end within [`MAX_SOURCE`].
fn coloured_len(src: &str) -> usize {
    if src.len() <= MAX_SOURCE {
        return src.len();
    }
    src.as_bytes()[..MAX_SOURCE].iter().rposition(|&b| b == b'\n').unwrap_or(0)
}

/// [`builtin`], cached per language and code: the same call with the same code returns the
/// same `Rc` without highlighting again.
pub fn highlight(lang: Option<&str>, code: &str) -> Rc<[Highlight]> {
    Highlighter::builtin().highlight(lang, code)
}

type HighlightFn = dyn Fn(Option<&str>, &str) -> Vec<Highlight>;

/// Turns code into highlights: the built-in one ([`Highlighter::builtin`], the default), or an
/// app's own function, such as a syntect or tree-sitter highlighter.
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
    /// The built-in highlighter (see the module docs for the languages).
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

    /// The highlights of `code`'s first 128 KiB (cut at a line end), cached.
    pub fn highlight(&self, lang: Option<&str>, code: &str) -> Rc<[Highlight]> {
        let code = &code[..coloured_len(code)];
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
/// overlapping, at most [`MAX_TOKENS`] of them.
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
    v.truncate(MAX_TOKENS);
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

/// A colour per kind of token, and optionally a weight. [`SyntaxTheme::from_kit`] follows the
/// kit's light or dark theme; [`SyntaxTheme::with`] overrides one kind's colour.
///
/// Highlighting changes colour only unless a theme gives a kind a weight
/// ([`with_weight`](Self::with_weight)), as Desktop bolds Markdown headings; in a monospace
/// face whose bold keeps the same advances, that still moves no glyph.
#[derive(Clone, Copy, Debug, PartialEq, Hash)]
pub struct SyntaxTheme {
    colors: [Color; Syntax::COUNT],
    /// Font weight per kind; 0 keeps the text's.
    weights: [u16; Syntax::COUNT],
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
        s[Syntax::Heading as usize] = paint(335.0);
        s[Syntax::Link as usize] = paint(225.0);
        s[Syntax::Operator as usize] = c.text_muted.mix(c.text_base, 0.5);
        s[Syntax::Regex as usize] = paint(170.0);
        s[Syntax::Attribute as usize] = paint(40.0);
        s[Syntax::Variable as usize] = c.text_base.mix(c.text_contrast, 0.25);
        SyntaxTheme { colors: s, weights: [0; Syntax::COUNT] }
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

    /// Draws one kind of token at font weight `weight` (100 to 900).
    pub fn with_weight(mut self, kind: Syntax, weight: f32) -> SyntaxTheme {
        self.weights[kind as usize] = weight.clamp(1.0, 1000.0) as u16;
        self
    }

    /// The weight [`with_weight`](Self::with_weight) gave `kind`, if any.
    pub fn weight(&self, kind: Syntax) -> Option<f32> {
        Some(self.weights[kind as usize]).filter(|&w| w > 0).map(f32::from)
    }

    pub fn paint(&self, p: Paint) -> Color {
        match p {
            Paint::Kind(k) => self.color(k),
            Paint::Color(c) => c,
        }
    }

    /// [`Span`]s for `highlights`, each range moved `offset` bytes on: where the code starts in
    /// the text the spans style. They set a colour, and a weight for kinds the theme gives one.
    pub fn spans(&self, highlights: &[Highlight], offset: usize) -> Vec<Span> {
        highlights
            .iter()
            .map(|(r, p)| {
                let mut sp = Span::new(r.start + offset, r.end + offset);
                sp.color = Some(self.paint(*p));
                if let Paint::Kind(k) = p {
                    sp.weight = self.weight(*k);
                }
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
