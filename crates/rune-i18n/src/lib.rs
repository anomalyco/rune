//! # rune-i18n
//!
//! Message catalogs for Rune apps, laid out for lookups that cost an index:
//!
//! - **Catalogs are data generated at build time.** An app keeps its catalogs as flat JSON files,
//!   one per locale and layer, and its build script runs [`codegen::generate`] (feature
//!   `codegen`). Each locale becomes one text blob plus a span table that `include_str!` and
//!   `include_bytes!` embed, so nothing parses, allocates or relocates at launch, and a locale
//!   added to the folder is in the next build.
//! - **Keys are checked at compile time.** [`Key::named`] and [`PluralKey::named`] are `const fn`s
//!   that binary-search the generated key list, so an app macro turns a key name into an index in a
//!   `const` and a typo fails the build.
//! - **[`I18n`]** holds the current locale in an atomic: [`I18n::text`] reads a slice of the
//!   locale's blob and falls back to the first (source) locale, [`I18n::set_locale`] switches every
//!   later lookup at once.
//! - **Templates** use `{{name}}` placeholders ([`render`], [`parts`]); plurals pick the key for the
//!   CLDR category of a count (`base.one`, `base.few`, `base.other`, ...).
//! - **Formatting** comes from ICU4X's CLDR data, built per locale on first use: plural rules,
//!   grouped and compact numbers, and conjunction lists.
//! - **Locale detection**: [`system_languages`] reads the OS's preferred languages and
//!   [`negotiate`] matches them to the supported locales by language and likely script.

mod catalog;
#[cfg(feature = "codegen")]
pub mod codegen;
mod format;
mod i18n;
mod locale;
mod system;
mod template;

pub use catalog::Catalog;
pub use catalog::Key;
pub use catalog::LocaleInfo;
pub use catalog::Messages;
pub use catalog::PluralKey;
pub use catalog::find;
pub use format::PluralCategory;
pub use i18n::I18n;
pub use locale::negotiate;
pub use system::posix_languages;
pub use system::system_languages;
pub use template::Part;
pub use template::parts;
pub use template::render;
