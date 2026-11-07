//! The app-wide translator over a generated [`Catalog`].

use core::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::catalog::{Catalog, Key, LocaleInfo, PluralKey};
use crate::format::{Formats, PluralCategory};

/// A translator: the catalog, the current locale, and each locale's formatters once used. Apps
/// keep one in a `static` and wrap it in macros that resolve keys at compile time.
///
/// Locale 0 is the source locale: every key has a message there, and a lookup the current locale
/// lacks falls back to it.
pub struct I18n {
    catalog: &'static Catalog,
    current: AtomicUsize,
    formats: OnceLock<Box<[Formats]>>,
}

impl I18n {
    pub const fn new(catalog: &'static Catalog) -> Self {
        Self { catalog, current: AtomicUsize::new(0), formats: OnceLock::new() }
    }

    pub fn catalog(&self) -> &'static Catalog {
        self.catalog
    }

    pub fn locales(&self) -> &'static [LocaleInfo] {
        self.catalog.locales()
    }

    /// The current locale's index into [`I18n::locales`].
    pub fn locale(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    pub fn info(&self) -> LocaleInfo {
        self.locales()[self.locale()]
    }

    /// The index of the locale whose id is `id`.
    pub fn find_locale(&self, id: &str) -> Option<usize> {
        self.locales().iter().position(|locale| locale.id() == id)
    }

    /// Makes `locale` current for every later lookup. Returns whether it changed; an index past the
    /// locales is ignored.
    pub fn set_locale(&self, locale: usize) -> bool {
        if locale >= self.locales().len() {
            return false;
        }
        self.current.swap(locale, Ordering::Relaxed) != locale
    }

    /// The supported locale that best matches `preferred` (see [`crate::negotiate`]), or the source
    /// locale.
    pub fn negotiate<'a>(&self, preferred: impl IntoIterator<Item = &'a str>) -> usize {
        let tags: Vec<&str> = self.locales().iter().map(LocaleInfo::tag).collect();
        crate::negotiate(preferred, &tags).unwrap_or(0)
    }

    /// The current locale's message for `key`, else the source locale's.
    pub fn text(&self, key: Key) -> &'static str {
        self.text_in(self.locale(), key)
    }

    pub fn text_in(&self, locale: usize, key: Key) -> &'static str {
        self.get(locale, key.index()).unwrap_or_else(|| self.catalog.keys().get(key.index()).copied().unwrap_or(""))
    }

    /// The message for `key` with its `{{name}}` placeholders filled from `args`.
    pub fn format(&self, key: Key, args: &[(&str, &dyn Display)]) -> String {
        crate::render(self.text(key), args)
    }

    /// The message of plural base `key` for `count`'s CLDR category in the current locale: the
    /// category's message, else `other`'s (each from the current locale, else the source locale).
    /// Its placeholders are left for the caller, which usually passes `count` too.
    pub fn plural(&self, key: PluralKey, count: u64) -> &'static str {
        self.plural_form(key, self.plural_category(count))
    }

    pub fn plural_form(&self, key: PluralKey, category: PluralCategory) -> &'static str {
        let forms = self.catalog.plural_forms(key);
        let locale = self.locale();
        let pick = |category: PluralCategory| forms[category.index()].and_then(|k| self.get(locale, k));
        pick(category).or_else(|| pick(PluralCategory::Other)).unwrap_or_else(|| self.catalog.plural_name(key))
    }

    /// The plural message for `count` with `{{count}}` and `args` filled in.
    pub fn plural_format(&self, key: PluralKey, count: u64, args: &[(&str, &dyn Display)]) -> String {
        let template = self.plural(key, count);
        if args.is_empty() {
            return crate::render(template, &[("count", &count)]);
        }
        let mut all: Vec<(&str, &dyn Display)> = Vec::with_capacity(args.len() + 1);
        all.extend_from_slice(args);
        all.push(("count", &count));
        crate::render(template, &all)
    }

    pub fn plural_category(&self, count: u64) -> PluralCategory {
        self.formats().plural(count)
    }

    /// `n` with the current locale's digits and grouping (`1,234`, `1.234`, `१,२३४`).
    pub fn integer(&self, n: i64) -> String {
        self.formats().integer(n)
    }

    /// `n` with at most `max_fraction` fraction digits in the current locale (`1.5`, `1,5`).
    pub fn decimal(&self, n: f64, max_fraction: u8) -> String {
        self.formats().decimal_to(n, max_fraction)
    }

    /// `items` as the current locale's long conjunction list (`a, b, and c`, `a, b und c`).
    pub fn list<S: AsRef<str>>(&self, items: &[S]) -> String {
        self.formats().list(items.iter().map(AsRef::as_ref))
    }

    fn get(&self, locale: usize, key: usize) -> Option<&'static str> {
        let local = self.catalog.messages(locale).and_then(|messages| messages.get(key));
        local.or_else(|| self.catalog.messages(0).and_then(|messages| messages.get(key)))
    }

    fn formats(&self) -> &Formats {
        let all = self.formats.get_or_init(|| self.locales().iter().map(|locale| Formats::new(locale.tag())).collect());
        &all[self.locale().min(all.len().saturating_sub(1))]
    }
}
