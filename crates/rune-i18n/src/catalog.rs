//! The generated catalog: sorted key names, plural bases with their per-category keys, the
//! locales, and one text blob plus span table per locale.

use core::cmp::Ordering;
use std::sync::OnceLock;

/// One supported locale: the app's id for it (`zht`), its BCP 47 tag (`zh-Hant`), the name it
/// shows in a language picker, in its own language (`繁體中文`), and whether it reads right to left.
#[derive(Clone, Copy, Debug)]
pub struct LocaleInfo {
    id: &'static str,
    tag: &'static str,
    label: &'static str,
    rtl: bool,
}

impl LocaleInfo {
    pub const fn new(id: &'static str, tag: &'static str, label: &'static str, rtl: bool) -> Self {
        Self { id, tag, label, rtl }
    }

    pub const fn id(&self) -> &'static str {
        self.id
    }

    pub const fn tag(&self) -> &'static str {
        self.tag
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub const fn rtl(&self) -> bool {
        self.rtl
    }
}

/// One locale's messages: every present message concatenated into `text`, and `spans` holding
/// a little-endian `(start, len)` pair of `u32`s per key, `start = u32::MAX` where the locale has
/// no message for the key.
///
/// A locale's messages are in the program ([`Messages::new`]) or come at run time
/// ([`Messages::later`], [`Catalog::load_json`]): a web build keeps only the source locale's in
/// its module and fetches the one a page shows.
#[derive(Debug)]
pub struct Messages {
    embedded: Option<(&'static str, &'static [u8])>,
    loaded: OnceLock<(&'static str, &'static [u8])>,
}

impl Messages {
    pub const fn new(text: &'static str, spans: &'static [u8]) -> Self {
        Self { embedded: Some((text, spans)), loaded: OnceLock::new() }
    }

    /// A locale whose messages come at run time: until then it has none, so every lookup falls
    /// back to the source locale.
    pub const fn later() -> Self {
        Self { embedded: None, loaded: OnceLock::new() }
    }

    /// Whether the messages are here: in the program, or loaded.
    pub fn present(&self) -> bool {
        self.embedded.is_some() || self.loaded.get().is_some()
    }

    /// The message for key index `key`, or `None` when this locale lacks it.
    pub fn get(&self, key: usize) -> Option<&'static str> {
        let (text, spans) = self.embedded.or_else(|| self.loaded.get().copied())?;
        let at = key * 8;
        let span = spans.get(at..at + 8)?;

        let start = u32::from_le_bytes([span[0], span[1], span[2], span[3]]);
        if start == u32::MAX {
            return None;
        }
        let len = u32::from_le_bytes([span[4], span[5], span[6], span[7]]);
        text.get(start as usize..(start + len) as usize)
    }
}

/// The messages for `keys` concatenated, and a `(start, len)` little-endian `u32` pair per key,
/// `start = u32::MAX` for a key `message` has none for: a locale's blob, made the same way at
/// build time (`codegen`) and at run time (`Catalog::load_json`). With `isolate`, a key the
/// locale lacks takes that locale's message between LRI and PDI instead: the message the lookup
/// falls back to (a missing plural category included, which `I18n::plural_form` takes from the
/// source before the locale's `other`).
pub(crate) fn blob<'m, 's>(keys: &[&str], message: impl Fn(&str) -> Option<&'m str>,
    isolate: Option<&dyn Fn(&str) -> Option<&'s str>>) -> (String, Vec<u8>) {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(keys.len() * 8);
    for key in keys {
        let own = message(key);
        let isolated = isolate.filter(|_| own.is_none()).and_then(|source| source(key)).map(|m| format!("\u{2066}{m}\u{2069}"));
        let (start, len) = match own.or(isolated.as_deref()) {
            Some(message) => {
                let start = text.len() as u32;
                text.push_str(message);
                (start, message.len() as u32)
            }
            None => (u32::MAX, 0),
        };
        spans.extend_from_slice(&start.to_le_bytes());
        spans.extend_from_slice(&len.to_le_bytes());
    }
    (text, spans)
}

/// Everything [`crate::codegen::generate`] emits, in one static.
#[derive(Debug)]
pub struct Catalog {
    keys: &'static [&'static str],
    plurals: &'static [&'static str],
    plural_forms: &'static [[u32; 6]],
    locales: &'static [LocaleInfo],
    messages: &'static [Messages],
}

impl Catalog {
    pub const fn new(
        keys: &'static [&'static str],
        plurals: &'static [&'static str],
        plural_forms: &'static [[u32; 6]],
        locales: &'static [LocaleInfo],
        messages: &'static [Messages],
    ) -> Self {
        Self { keys, plurals, plural_forms, locales, messages }
    }

    pub const fn keys(&self) -> &'static [&'static str] {
        self.keys
    }

    pub const fn locales(&self) -> &'static [LocaleInfo] {
        self.locales
    }

    pub(crate) fn messages(&self, locale: usize) -> Option<&'static Messages> {
        self.messages.get(locale)
    }

    /// Whether `locale`'s messages are here (in the program, or loaded).
    pub fn present(&self, locale: usize) -> bool {
        self.messages.get(locale).is_some_and(Messages::present)
    }

    /// Fills a locale the build left for run time (`Messages::later`) from its messages as one
    /// JSON object of key to message (the locale's layers merged, a later layer winning). Keys
    /// the program doesn't have are skipped, and a key the object lacks falls back to the source
    /// locale as ever. A locale that's here already is left as it is.
    pub fn load_json(&self, locale: usize, json: &str) -> Result<(), String> {
        let Some(messages) = self.messages.get(locale) else { return Err(format!("no locale {locale}")) };
        if messages.present() { return Ok(()); }
        let serde_json::Value::Object(map) = serde_json::from_str(json).map_err(|e| e.to_string())? else {
            return Err("not a JSON object".into());
        };
        // As the build makes a right-to-left locale's blob: its missing messages are the
        // source's, isolated.
        let source = |key: &str| find(self.keys, key).and_then(|k| self.messages.first()?.get(k));
        let rtl = self.locales.get(locale).is_some_and(LocaleInfo::rtl) && !self.locales.first().is_some_and(LocaleInfo::rtl);
        let isolate: Option<&dyn Fn(&str) -> Option<&'static str>> = if rtl { Some(&source) } else { None };
        let (text, spans) = blob(self.keys, |key| map.get(key).and_then(serde_json::Value::as_str), isolate);
        let _ = messages.loaded.set((Box::leak(text.into_boxed_str()), Box::leak(spans.into_boxed_slice())));
        Ok(())
    }

    /// The key index for each CLDR category of a plural base, in [`crate::PluralCategory`] order.
    pub(crate) fn plural_forms(&self, key: PluralKey) -> [Option<usize>; 6] {
        let forms = self.plural_forms.get(key.0 as usize).copied().unwrap_or([u32::MAX; 6]);
        forms.map(|k| (k != u32::MAX).then_some(k as usize))
    }

    pub(crate) fn plural_name(&self, key: PluralKey) -> &'static str {
        self.plurals.get(key.0 as usize).copied().unwrap_or("")
    }
}

/// A message key: an index into the catalog's sorted key list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(pub(crate) u32);

impl Key {
    /// The key called `name` in `keys`, the generated sorted key list. Called in a `const`, a name
    /// missing from the catalog fails the build.
    pub const fn named(keys: &[&str], name: &str) -> Key {
        match find(keys, name) {
            Some(index) => Key(index as u32),
            None => panic!("unknown i18n key"),
        }
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A plural base: `base` names the messages `base.one`, `base.other` and the locale's other
/// CLDR categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluralKey(pub(crate) u32);

impl PluralKey {
    /// The plural base `name` in `plurals`, the generated sorted base list. Called in a `const`,
    /// a base without `.one` and `.other` messages fails the build.
    pub const fn named(plurals: &[&str], name: &str) -> PluralKey {
        match find(plurals, name) {
            Some(index) => PluralKey(index as u32),
            None => panic!("unknown i18n plural key"),
        }
    }
}

/// Binary search for `name` in byte-sorted `keys`, usable in `const`.
pub const fn find(keys: &[&str], name: &str) -> Option<usize> {
    let (mut lo, mut hi) = (0, keys.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        match compare(keys[mid].as_bytes(), name.as_bytes()) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Some(mid),
        }
    }
    None
}

const fn compare(a: &[u8], b: &[u8]) -> Ordering {
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] < b[i] {
            return Ordering::Less;
        }
        if a[i] > b[i] {
            return Ordering::Greater;
        }
        i += 1;
    }
    if a.len() < b.len() {
        return Ordering::Less;
    }
    if a.len() > b.len() {
        return Ordering::Greater;
    }
    Ordering::Equal
}
