use crate::catalog::{Catalog, Key, LocaleInfo, Messages, PluralKey};
use crate::format::{Formats, PluralCategory};
use crate::i18n::I18n;

#[test]
fn empty_catalog_falls_back_to_root_locale_without_panicking() {
    static EMPTY: Catalog = Catalog::new(&[], &[], &[], &[], &[]);
    let i18n = I18n::new(&EMPTY);
    let root = Formats::root();

    let info = i18n.info();
    assert_eq!(info.id(), "und");
    assert_eq!(info.tag(), "und");
    assert_eq!(info.label(), "und");
    assert!(!info.rtl());

    assert_eq!(i18n.integer(1234), root.integer(1234));
    assert_eq!(i18n.decimal(1234.5, 2), root.decimal_to(1234.5, 2));
    assert_eq!(i18n.list(&["a", "b", "c"]), root.list(["a", "b", "c"].into_iter()));
    assert_eq!(i18n.plural_category(1), root.plural(1));
    assert_eq!(i18n.plural_category(2), root.plural(2));
    assert_eq!(i18n.plural(PluralKey(0), 1), "");
    assert_eq!(i18n.plural_format(PluralKey(0), 1, &[]), "");
    assert_eq!(i18n.text(Key(0)), "");
    assert!(!i18n.set_locale(0));
    assert_eq!(i18n.negotiate(["en"]), 0);
}

#[test]
fn nonempty_catalog_uses_current_locale_info_and_formats() {
    static LOCALES: [LocaleInfo; 2] = [
        LocaleInfo::new("en", "en", "English", false),
        LocaleInfo::new("de", "de", "Deutsch", false),
    ];
    static MESSAGES: [Messages; 2] = [Messages::new("", &[]), Messages::new("", &[])];
    static CATALOG: Catalog = Catalog::new(&[], &[], &[], &LOCALES, &MESSAGES);
    let i18n = I18n::new(&CATALOG);

    assert_eq!(i18n.info().tag(), "en");
    assert_eq!(i18n.integer(1234), "1,234");
    assert_eq!(i18n.decimal(1.5, 2), "1.5");
    assert_eq!(i18n.plural_category(1), PluralCategory::One);
    assert_eq!(i18n.plural_category(2), PluralCategory::Other);

    assert!(i18n.set_locale(1));
    assert_eq!(i18n.info().tag(), "de");
    assert_eq!(i18n.integer(1234), "1.234");
    assert_eq!(i18n.decimal(1.5, 2), "1,5");
}
