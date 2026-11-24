use super::*;

#[test]
fn posix_languages_follows_priority_normalizes_and_deduplicates() {
    let vars = |name: &str| match name {
        "LANGUAGE" => Some("ar_EG:de_AT.UTF-8@euro:C::ar_EG".to_owned()),
        "LC_ALL" => Some("POSIX".to_owned()),
        "LC_MESSAGES" => Some("en_US.UTF-8".to_owned()),
        "LANG" => Some("de_AT.UTF-8".to_owned()),
        _ => None,
    };
    assert_eq!(posix_languages(vars), vec!["ar-EG", "de-AT", "en-US"]);
}

#[test]
fn posix_languages_returns_empty_for_c_posix_and_missing_vars() {
    assert!(posix_languages(|_| None).is_empty());
    assert!(posix_languages(|name| match name {
        "LANGUAGE" => Some("C:POSIX:".to_owned()),
        "LANG" => Some("C.UTF-8".to_owned()),
        _ => None,
    }).is_empty());
}

#[test]
fn normalize_trims_skips_empty_and_preserves_first_occurrence() {
    assert_eq!(
        normalize(["  ar-EG  ", "", "   ", "en-US", "ar-EG", "he-IL"]),
        vec!["ar-EG", "en-US", "he-IL"],
    );
    assert!(normalize(["", " \t "]).is_empty());
}

#[test]
fn cached_platform_languages_contains_only_non_empty_trimmed_tags() {
    let langs = cached();
    for tag in langs {
        assert!(!tag.is_empty());
        assert_eq!(tag.trim(), tag);
    }
}
