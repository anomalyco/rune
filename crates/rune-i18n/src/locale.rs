//! Matching preferred languages to supported locales.

use icu_locale::{LocaleExpander, LanguageIdentifier, subtags::Language};

/// The first supported locale that one of `preferred` (BCP 47 tags, most wanted first) names, by
/// language and likely script: `de-AT` picks `de`, `zh-TW` picks `zh-Hant` over `zh-Hans`, `pa-PK`
/// picks `pa-Arab-PK` and `pa-IN` does not. Norwegian `no`, `nb` and `nn` match one another.
/// Returns the index into `supported`, or `None` when nothing matches.
pub fn negotiate<'a>(preferred: impl IntoIterator<Item = &'a str>, supported: &[&str]) -> Option<usize> {
    let expander = LocaleExpander::new_extended();
    let maximize = |tag: &str| {
        let mut id = LanguageIdentifier::try_from_str(&tag.replace('_', "-")).ok()?;
        expander.maximize(&mut id);
        Some(id)
    };
    let targets: Vec<Option<LanguageIdentifier>> = supported.iter().map(|tag| maximize(tag)).collect();
    preferred.into_iter().filter_map(maximize).find_map(|source| {
        let norwegian = is_norwegian(source.language);
        targets.iter().position(|target| {
            target.as_ref().is_some_and(|target| {
                if norwegian {
                    return is_norwegian(target.language);
                }
                target.language == source.language && target.script == source.script
            })
        })
    })
}

fn is_norwegian(language: Language) -> bool {
    matches!(language.as_str(), "no" | "nb" | "nn")
}
