use super::*;

fn s(tags: &[&str]) -> Vec<String> {
    tags.iter().map(|t| (*t).to_owned()).collect()
}

#[test]
fn regional_variants_and_underscores_match_available_dictionaries() {
    assert_eq!(
        default_languages(&s(&["de-DE", "en-US"]), &s(&["de-AT"])),
        s(&["de-DE", "en-US"]),
        "de-AT should fall back to de-DE when de is not listed"
    );
    assert_eq!(
        default_languages(&s(&["en-GB"]), &s(&["en-AU"])),
        s(&["en-GB"]),
        "en-AU should fall back to en-GB when only en-GB is available"
    );
    assert_eq!(
        default_languages(&s(&["en-US"]), &s(&["en_US"])),
        s(&["en-US"]),
        "underscore separator should match hyphenated dictionary"
    );
    assert_eq!(
        default_languages(&s(&["en_US", "fr-FR"]), &s(&["fr-CA"])),
        s(&["fr-FR", "en_US"]),
        "en-US fallback should match case/underscore variants and preserve the dictionary's own tag"
    );
}

#[test]
fn exact_and_bare_matches_win_over_regional_fallback() {
    assert_eq!(
        default_languages(&s(&["de-DE", "de", "de-AT"]), &s(&["de-AT"])),
        s(&["de-AT"])
    );
    assert_eq!(
        default_languages(&s(&["de-DE", "de"]), &s(&["de-AT"])),
        s(&["de"]),
        "bare base tag wins over another regional variant"
    );
}

#[test]
fn prefix_similar_language_tags_do_not_cross_match() {
    assert_eq!(
        default_languages(&s(&["est-EE", "en-US"]), &s(&["es-MX"])),
        s(&["en-US"]),
        "es must not match est-EE by prefix"
    );
    assert_eq!(
        default_languages(&s(&["eng-GB"]), &s(&["en-AU"])),
        Vec::<String>::new(),
        "en must not match eng-GB by prefix"
    );
}

#[test]
fn chosen_languages_match_case_underscores_and_regional_variants() {
    assert_eq!(
        resolve_languages(&s(&["en-US", "de-DE"]), &s(&["en_us", "de-AT"]), &s(&["fr-FR"])),
        s(&["en-US", "de-DE"])
    );
    assert_eq!(
        resolve_languages(&s(&["en-GB", "en-US"]), &s(&["en-GB", "en-US"]), &s(&[])),
        s(&["en-GB", "en-US"]),
        "explicit selection keeps multiple regional variants of the same language"
    );
    assert_eq!(
        resolve_languages(&s(&["en-US"]), &s(&["fr-FR"]), &s(&["en-AU"])),
        s(&["en-US"]),
        "unmatched chosen list falls back to preferred languages"
    );
}

#[test]
fn active_dicts_excludes_deselected_loaded_dictionaries() {
    let loaded = vec![("en-US".to_owned(), 1u8), ("de-DE".to_owned(), 2u8)];
    assert_eq!(active_dicts(&loaded, &s(&["en-US"])), vec![&1]);
    assert_eq!(active_dicts(&loaded, &s(&["de_de"])), vec![&2]);
    assert_eq!(active_dicts(&loaded, &s(&["en-US", "de-DE"])), vec![&1, &2]);
    assert!(active_dicts(&loaded, &s(&["fr-FR"])).is_empty());
}

#[test]
fn dialect_names_normalize_case_and_underscores() {
    assert_eq!(language_name("en_US"), "American English");
    assert_eq!(language_name("en-au"), "Australian English");
}
