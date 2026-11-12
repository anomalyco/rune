//! In a browser, a face of last resort that the platform draws. A page can't read the system's
//! fonts, and the web build carries only the app's (Latin, Greek, Cyrillic), so Japanese,
//! Chinese, Korean, Arabic, Hebrew, Hindi, Thai and emoji would draw as empty boxes. Every
//! family stack ends in "Rune Platform", a font with no outlines whose character map covers
//! everything; the run Parley gives it isn't shaped by HarfRust but by the browser
//! (`parley_engine::platform`): each unit a line can't break inside (a word, an ideograph, an
//! emoji) is measured on an offscreen 2D canvas at the run's size, weight and style, its
//! graphemes' advances from measuring its prefixes. It's drawn there grapheme by grapheme, or
//! whole where its letters join (Arabic, Indic scripts), its first glyph in visual order
//! carrying the word's image. An image is an atlas mask like any glyph's (text) or an
//! RGBA image like an emoji's (colour), so the CPU and GPU renderers draw it as they draw any.
//! The system's fonts, shaping and colour emoji, at zero bytes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parley::fontique::{Blob, Collection};
use parley_engine::platform::{PlatformGlyph, PlatformText};
use wasm_bindgen::prelude::*;

/// The family every stack ends in.
pub(super) const FAMILY: &[&str] = &["Rune Platform"];
/// The family the browser draws with: its UI font, and its own fallback for every script.
const CSS_FAMILY: &str = "system-ui, sans-serif";
/// The glyph that draws nothing: a unit's graphemes after the one carrying its image, a space.
const BLANK: u32 = 1;

#[wasm_bindgen(module = "/src/platform/web/platform_text.js")]
extern "C" {
    #[wasm_bindgen(js_name = measurePrefixes)]
    fn measure_prefixes(font: &str, text: &str, rtl: bool, lang: &str, ends: &[u32]) -> Vec<f64>;
    fn rasterize(font: &str, text: &str, rtl: bool, lang: &str, phase: f64) -> Vec<u8>;
}

static BLOB: OnceLock<Blob<u8>> = OnceLock::new();

fn blob() -> Blob<u8> {
    BLOB.get_or_init(|| Blob::new(Arc::new(font()))).clone()
}

/// Adds the face to `collection` and gives Parley its shaper.
pub(super) fn register(collection: &mut Collection) {
    let blob = blob();
    parley_engine::platform::set_platform_face(blob.id(), shape);
    collection.register_fonts(blob, None);
}

/// Whether `font` is the platform's face.
pub(super) fn is(font: &parley::FontData) -> bool {
    BLOB.get().is_some_and(|b| b.id() == font.data.id())
}

/// What a glyph id draws: a unit of text in a style.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Unit {
    text: String,
    weight: u16,
    italic: bool,
    rtl: bool,
    lang: String,
}

#[derive(Default)]
struct Units {
    ids: HashMap<Unit, u32>,
    units: Vec<Unit>,
}

thread_local! {
    static UNITS: RefCell<Units> = RefCell::default();
}

fn id_of(unit: Unit) -> u32 {
    UNITS.with_borrow_mut(|u| {
        if let Some(&id) = u.ids.get(&unit) { return id; }
        let id = u.units.len() as u32 + BLANK + 1;
        u.units.push(unit.clone());
        u.ids.insert(unit, id);
        id
    })
}

fn css(size: f32, weight: u16, italic: bool) -> String {
    format!("{}{weight} {size}px {CSS_FAMILY}", if italic { "italic " } else { "" })
}

/// Parley's shaper for the face: one glyph per grapheme. Each unit's trailing white space is
/// blank glyphs of their own, so a space a line hangs past its end never moves the image.
fn shape(run: &PlatformText<'_>, out: &mut Vec<PlatformGlyph>) {
    let weight = run.weight.round().clamp(1.0, 1000.0) as u16;
    let lang = run.language.unwrap_or("");
    let font = css(run.size, weight, run.italic);
    let end_of = |i: usize, ends: &[u32]| ends.get(i).map_or(run.text.len(), |&e| e as usize);
    for (u, &start) in run.units.iter().enumerate() {
        let (start, end) = (start as usize, end_of(u + 1, run.units));
        let first = run.graphemes.partition_point(|&g| (g as usize) < start);
        let last = run.graphemes.partition_point(|&g| (g as usize) < end);
        let graphemes = &run.graphemes[first..last];
        // The word without its trailing white space.
        let mut inked = graphemes.len();
        while inked > 0 && run.text[graphemes[inked - 1] as usize..end_of(first + inked, run.graphemes)].chars().all(char::is_whitespace) {
            inked -= 1;
        }
        let word_end = end_of(first + inked, run.graphemes).min(end);
        let word = &run.text[start..word_end];
        let mut ends: Vec<u32> = Vec::with_capacity(graphemes.len());
        let mut utf16 = 0u32;
        for (i, _) in graphemes.iter().enumerate().take(inked) {
            let to = end_of(first + i + 1, run.graphemes).min(word_end);
            utf16 += run.text[end_of(first + i, run.graphemes).min(to).max(start)..to].encode_utf16().count() as u32;
            ends.push(utf16);
        }
        let widths = if word.is_empty() { Vec::new() } else { measure_prefixes(&font, word, run.rtl, lang, &ends) };
        let unit = |text: &str| id_of(Unit { text: text.to_owned(), weight, italic: run.italic, rtl: run.rtl, lang: lang.to_owned() });
        // A word whose letters join or stack across graphemes is drawn whole, its image on its
        // first glyph in visual order; any other draws grapheme by grapheme where the word's
        // prefixes put each, so a selection or a colour can start inside it.
        let whole = word.chars().any(joins);
        let id = (whole && !word.is_empty()).then(|| unit(word));
        let carrier = if run.rtl { inked.saturating_sub(1) } else { 0 };
        let mut before = 0.0f32;
        for i in 0..inked {
            let width = widths.get(i).map_or(before, |&w| w as f32);
            let id = match id {
                Some(id) if i == carrier => id,
                Some(_) => BLANK,
                None => unit(&run.text[graphemes[i] as usize..end_of(first + i + 1, run.graphemes).min(word_end)]),
            };
            out.push(PlatformGlyph { id, advance: width - before, x_offset: 0.0 });
            before = width;
        }
        for i in inked..graphemes.len() {
            let space = &run.text[graphemes[i] as usize..end_of(first + i + 1, run.graphemes).min(end)];
            let ends = [space.encode_utf16().count() as u32];
            let width = measure_prefixes(&font, space, run.rtl, lang, &ends).first().copied().unwrap_or(0.0) as f32;
            out.push(PlatformGlyph { id: BLANK, advance: width, x_offset: 0.0 });
        }
    }
}

/// Whether `c` is in a script whose letters join, or stack and reorder across graphemes: Arabic,
/// Syriac, N'Ko, Mandaic, Mongolian, Adlam, Hanifi Rohingya, the Indic scripts and Tibetan,
/// Myanmar, Khmer and the other Brahmic scripts of Southeast Asia.
fn joins(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x08FF | 0x0900..=0x0DFF | 0x0F00..=0x0FFF | 0x1000..=0x109F | 0x1780..=0x18AF
        | 0x1A00..=0x1AAF | 0x1B00..=0x1C4F | 0xA800..=0xA8FF | 0xA980..=0xAAFF | 0xABC0..=0xABFF | 0xFB50..=0xFDFF
        | 0xFE70..=0xFEFF | 0x10D00..=0x10D3F | 0x11000..=0x11FFF | 0x1E900..=0x1E95F)
}

/// A glyph's image at `size` device px, its pen `phase` px right of a whole pixel: `None` for
/// one that draws nothing. Text is a coverage mask; colour (emoji) premultiplied RGBA.
pub(super) fn image(glyph: u32, size: f32, phase: f32) -> Option<Image> {
    let unit = UNITS.with_borrow(|u| u.units.get((glyph.checked_sub(BLANK + 1)?) as usize).cloned())?;
    let bytes = rasterize(&css(size, unit.weight, unit.italic), &unit.text, unit.rtl, &unit.lang, f64::from(phase));
    let head = |i: usize| bytes.get(i * 4..i * 4 + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (w, h, left, top, color) = (head(0)?, head(1)?, head(2)?, head(3)?, head(4)? == 1);
    Some(Image { w: w as u32, h: h as u32, left, top, color, data: bytes[20..].to_vec() })
}

pub(super) struct Image {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) color: bool,
    pub(super) data: Vec<u8>,
}

/// The face's file: no outlines, 65,535 glyphs (none drawn), a character map that sends every
/// character but controls, joiners, variation selectors, tags and private use to a glyph, and a
/// `wght` axis, so each run says which weight it asked for. Metrics are Inter's, so a line with
/// a platform run in it is as tall as one without.
fn font() -> Vec<u8> {
    const UPEM: u16 = 16384;
    const ASCENT: i16 = 15872;
    const DESCENT: i16 = -3968;
    const GLYPHS: u16 = 65535;
    let be16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_be_bytes());
    let be32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_be_bytes());

    let mut os2 = Vec::new();
    for x in [4u16, UPEM / 2, 400, 5, 0] { be16(&mut os2, x); }
    for x in [UPEM / 2, UPEM / 2, 0, 2048, UPEM / 2, UPEM / 2, 0, 6144, 819, 4096] { be16(&mut os2, x); }
    be16(&mut os2, 0);
    os2.extend_from_slice(&[0; 10]);
    for _ in 0..4 { be32(&mut os2, u32::MAX); }
    os2.extend_from_slice(b"RUNE");
    for x in [0x00C0u16, 0x20, 0xFFFF, ASCENT as u16, DESCENT as u16, 0, ASCENT as u16, (-DESCENT) as u16] { be16(&mut os2, x); }
    be32(&mut os2, 1);
    be32(&mut os2, 0);
    for x in [8192u16, 11469, 0, 32, 1] { be16(&mut os2, x); }

    // Every character a font could draw, in runs of at most 65,534, each to glyphs 1 on.
    const COVERED: &[(u32, u32)] = &[(0x20, 0x7E), (0xA0, 0x200A), (0x2010, 0x2027), (0x202F, 0x205F), (0x2070, 0xD7FF),
        (0xF900, 0xFDFF), (0xFE10, 0xFEFE), (0xFF00, 0xFFFD), (0x10000, 0xDFFFF)];
    let groups: Vec<(u32, u32)> = COVERED.iter().flat_map(|&(a, b)| (a..=b).step_by(65534).map(move |s| (s, (s + 65533).min(b)))).collect();
    let mut cmap = Vec::new();
    for x in [0u16, 2, 0, 4] { be16(&mut cmap, x); }
    be32(&mut cmap, 20);
    for x in [3u16, 10] { be16(&mut cmap, x); }
    be32(&mut cmap, 20);
    for x in [12u16, 0] { be16(&mut cmap, x); }
    be32(&mut cmap, 16 + 12 * groups.len() as u32);
    be32(&mut cmap, 0);
    be32(&mut cmap, groups.len() as u32);
    for (s, e) in &groups {
        be32(&mut cmap, *s);
        be32(&mut cmap, *e);
        be32(&mut cmap, 1);
    }

    let mut fvar = Vec::new();
    for x in [1u16, 0, 16, 2, 1, 20, 0, 8] { be16(&mut fvar, x); }
    fvar.extend_from_slice(b"wght");
    for x in [100u32, 400, 900] { be32(&mut fvar, x << 16); }
    for x in [0u16, 256] { be16(&mut fvar, x); }

    let mut head = Vec::new();
    for x in [0x0001_0000u32, 0x0001_0000, 0, 0x5F0F_3CF5] { be32(&mut head, x); }
    for x in [0x0003u16, UPEM] { be16(&mut head, x); }
    head.extend_from_slice(&[0; 16]);
    for x in [0i16, DESCENT, UPEM as i16, ASCENT] { be16(&mut head, x as u16); }
    for x in [0u16, 8, 2, 0, 0] { be16(&mut head, x); }

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for x in [ASCENT, DESCENT, 0] { be16(&mut hhea, x as u16); }
    for x in [UPEM, 0, 0, UPEM, 1, 0, 0, 0, 0, 0, 0, 0, 1] { be16(&mut hhea, x); }

    let mut hmtx = vec![0u8; 4 + 2 * (GLYPHS as usize - 1)];
    hmtx[..2].copy_from_slice(&UPEM.to_be_bytes());

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0000_5000);
    be16(&mut maxp, GLYPHS);

    let names: [(u16, &str); 5] = [(1, "Rune Platform"), (2, "Regular"), (4, "Rune Platform"), (6, "RunePlatform"), (256, "Weight")];
    let mut name = Vec::new();
    let mut strings = Vec::new();
    for x in [0u16, names.len() as u16, 6 + 12 * names.len() as u16] { be16(&mut name, x); }
    for (id, s) in names {
        let at = strings.len() as u16;
        for unit in s.encode_utf16() { be16(&mut strings, unit); }
        for x in [3u16, 1, 0x409, id, strings.len() as u16 - at, at] { be16(&mut name, x); }
    }
    name.extend(strings);

    let mut post = Vec::new();
    for x in [0x0003_0000u32, 0] { be32(&mut post, x); }
    for x in [-1638i16, 819] { be16(&mut post, x as u16); }
    for _ in 0..5 { be32(&mut post, 0); }

    let tables: [(&[u8; 4], Vec<u8>); 9] = [(b"OS/2", os2), (b"cmap", cmap), (b"fvar", fvar), (b"head", head), (b"hhea", hhea),
        (b"hmtx", hmtx), (b"maxp", maxp), (b"name", name), (b"post", post)];
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    for x in [tables.len() as u16, 128, 3, 16 * tables.len() as u16 - 128] { be16(&mut out, x); }
    let mut offset = 12 + 16 * tables.len() as u32;
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0);
        be32(&mut out, offset);
        be32(&mut out, data.len() as u32);
        offset += (data.len() as u32).div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    out
}
