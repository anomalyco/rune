//! CLDR formatting for one locale from ICU4X's compiled data, each formatter built on first use.

use std::sync::OnceLock;

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::options::DecimalFormatterOptions;
use icu_decimal::DecimalFormatter;
use icu_list::ListFormatter;
use icu_list::options::{ListFormatterOptions, ListLength};
use icu_locale::Locale;
use icu_plurals::PluralRules;
use writeable::Writeable;

/// A CLDR plural category, in the order the generated plural tables use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl PluralCategory {
    pub const ALL: [PluralCategory; 6] = [Self::Zero, Self::One, Self::Two, Self::Few, Self::Many, Self::Other];

    /// The suffix a catalog key carries for this category (`one` in `files.one`).
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    fn from_icu(category: icu_plurals::PluralCategory) -> Self {
        match category {
            icu_plurals::PluralCategory::Zero => Self::Zero,
            icu_plurals::PluralCategory::One => Self::One,
            icu_plurals::PluralCategory::Two => Self::Two,
            icu_plurals::PluralCategory::Few => Self::Few,
            icu_plurals::PluralCategory::Many => Self::Many,
            icu_plurals::PluralCategory::Other => Self::Other,
        }
    }
}

/// One locale's formatters. A formatter whose data is missing for the locale falls back to the
/// root locale's; without either, plurals read `other`, numbers show ASCII digits and lists join
/// with commas.
pub(crate) struct Formats {
    locale: Locale,
    plurals: OnceLock<Option<PluralRules>>,
    decimal: OnceLock<Option<DecimalFormatter>>,
    list: OnceLock<Option<ListFormatter>>,
}

impl Formats {
    pub(crate) fn new(tag: &str) -> Self {
        Self {
            locale: Locale::try_from_str(tag).unwrap_or(Locale::UNKNOWN),
            plurals: OnceLock::new(),
            decimal: OnceLock::new(),
            list: OnceLock::new(),
        }
    }

    pub(crate) fn plural(&self, count: u64) -> PluralCategory {
        let rules = self.plurals.get_or_init(|| {
            PluralRules::try_new_cardinal((&self.locale).into())
                .or_else(|_| PluralRules::try_new_cardinal(Locale::UNKNOWN.into()))
                .ok()
        });
        rules.as_ref().map_or(PluralCategory::Other, |rules| PluralCategory::from_icu(rules.category_for(count)))
    }

    /// `n` with the locale's digits and grouping, as `Intl.NumberFormat(locale)` shows an integer.
    pub(crate) fn integer(&self, n: i64) -> String {
        self.format(&Decimal::from(n))
    }

    /// `n` rounded half away from zero to at most `max_fraction` digits, trailing zeros dropped,
    /// as `Intl.NumberFormat(locale, { maximumFractionDigits })` shows it.
    pub(crate) fn decimal_to(&self, n: f64, max_fraction: u8) -> String {
        let Ok(mut value) = Decimal::try_from_f64(n, FloatPrecision::RoundTrip) else {
            return format!("{n}");
        };
        value.round_with_mode(-i16::from(max_fraction), SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand));
        value.absolute.trim_end();
        self.format(&value)
    }

    /// The items joined as the locale's long conjunction (`a, b and c`, `a, b und c`).
    pub(crate) fn list<'a>(&self, items: impl Iterator<Item = &'a str> + Clone) -> String {
        let formatter = self.list.get_or_init(|| {
            let options = ListFormatterOptions::default().with_length(ListLength::Wide);
            ListFormatter::try_new_and((&self.locale).into(), options)
                .or_else(|_| ListFormatter::try_new_and(Locale::UNKNOWN.into(), options))
                .ok()
        });
        match formatter {
            Some(formatter) => formatter.format(items).write_to_string().into_owned(),
            None => items.collect::<Vec<_>>().join(", "),
        }
    }

    fn format(&self, value: &Decimal) -> String {
        let formatter = self.decimal.get_or_init(|| {
            DecimalFormatter::try_new((&self.locale).into(), DecimalFormatterOptions::default())
                .or_else(|_| DecimalFormatter::try_new(Locale::UNKNOWN.into(), DecimalFormatterOptions::default()))
                .ok()
        });
        match formatter {
            Some(formatter) => formatter.format(value).write_to_string().into_owned(),
            None => value.to_string(),
        }
    }
}